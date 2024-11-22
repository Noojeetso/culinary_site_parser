mod fileio;
mod html_loader;

use common_utils::connections::{
    InfluxDB2Connection, RabbitMQConnection, RabbitQueue, RedisConnection,
};
use html_loader::{HtmlDownloader, HtmlLoader};
use redis::Commands;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use amqprs::{
    channel::{BasicAckArguments, ConsumerMessage},
    BasicProperties,
};
use dotenv::dotenv;
use tokio::runtime;
use tokio::sync::mpsc::{channel, Receiver, Sender, UnboundedReceiver};
use tokio::sync::Mutex;
use url::Url;
use warp::Filter;

static HEALTHY: AtomicUsize = AtomicUsize::new(0);
static LOADED_PAGES_LIMIT_AMOUNT: AtomicUsize = AtomicUsize::new(0);

struct StopMessage;
struct StopMessageAcknowledgement;

struct LoadAutomaton {
    redis_connection: RedisConnection,
    rabbitmq_connection: RabbitMQConnection,
    new_links_queue: RabbitQueue,
    new_documents_queue: RabbitQueue,
    influxdb2_connection: InfluxDB2Connection,
    // loader: html_loader::HtmLoaderFS,
    loader: HtmlDownloader,
}

impl LoadAutomaton {
    pub fn run() {
        let (sender, receiver) = channel::<StopMessage>(1024);
        let (ack_sender, ack_receiver) = channel::<StopMessageAcknowledgement>(1024);

        thread::scope(|s| {
            s.spawn(|| {
                let runtime = runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let load_automaton = Self::new().await;
                    load_automaton.run_worker(receiver, ack_sender).await
                });
            });
            s.spawn(|| {
                let runtime = runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .unwrap();
                runtime
                    .block_on(async { LoadAutomaton::run_web_server(sender, ack_receiver).await });
            });
        });
    }

    async fn new() -> Self {
        println!("[worker] starting");
        let redis_connection = RedisConnection::new().await;
        let influxdb2_connection = InfluxDB2Connection::new().await;
        let mut rabbitmq_connection = RabbitMQConnection::new().await;
        let new_links_queue = rabbitmq_connection.new_queue("amqprs.new_links").await;
        rabbitmq_connection.purge_queue(&new_links_queue).await;
        let mut new_documents_queue = rabbitmq_connection.new_queue("amqprs.new_documents").await;
        rabbitmq_connection
            .bind_queue_to_publish(
                &mut new_documents_queue,
                "amq.direct",
                "amqprs.push_to_new_documents",
            )
            .await;
        println!("[worker] configured all the connections");
        let loader = HtmlDownloader::new(None);
        // let loader = html_loader::HtmLoaderFS::new(Some(&PathBuf::from("tests")));
        Self {
            redis_connection,
            rabbitmq_connection,
            new_links_queue,
            new_documents_queue,
            influxdb2_connection,
            loader,
        }
    }

    async fn message_loop(&mut self, mut messages_rx: UnboundedReceiver<ConsumerMessage>) {
        println!("[worker] entering message handler loop");
        HEALTHY.store(1, Ordering::Relaxed);
        while let Some(message) = messages_rx.recv().await {
            let limit = LOADED_PAGES_LIMIT_AMOUNT.load(Ordering::Relaxed);
            if limit > 0 {
                let started_loading_pages_amount = self
                    .redis_connection
                    .connection
                    .incr::<&str, u64, u64>("started_loading_pages_amount", 1)
                    .unwrap();
                println!(
                    "[worker] started loading: {}; load limit: {}",
                    started_loading_pages_amount, limit
                );
                if started_loading_pages_amount as usize > limit {
                    println!(
                        "[worker] reached load limit ({}/{})",
                        started_loading_pages_amount - 1,
                        limit
                    );
                    break;
                }
            }
            let message_properties = message.basic_properties.unwrap();
            let message_id = message_properties.message_id().unwrap();
            let task_id = message_id;
            self.influxdb2_connection
                .store_timestamp("load_automaton_start", task_id.as_str())
                .await;
            let message_content_raw = message.content.unwrap();
            let new_link = std::str::from_utf8(&message_content_raw).unwrap();
            println!("[worker] new_link: {}", new_link);
            let new_url = Url::parse(new_link).unwrap();
            println!("[worker] parsed: {}", new_url.as_str());
            let document_string = self.loader.load(&new_url).await.unwrap();
            println!("[worker] document_string: {}", &document_string[0..10]);
            let new_message_content = format!("{}:{}", new_link, document_string);
            self.influxdb2_connection
                .store_timestamp("load_automaton_end", task_id.as_str())
                .await;
            let mut properties: BasicProperties = BasicProperties::default();
            properties.with_message_id(&message_id);
            self.rabbitmq_connection
                .channel
                .basic_publish(
                    properties,
                    new_message_content.into_bytes(),
                    self.new_documents_queue.publish_args.clone().unwrap(),
                )
                .await
                .unwrap();
            self.rabbitmq_connection
                .channel
                .basic_ack(BasicAckArguments::new(
                    message.deliver.unwrap().delivery_tag(),
                    false,
                ))
                .await
                .unwrap();
        }
        println!("[worker] exited message handler loop");
    }

    async fn run_worker(
        mut self,
        _receiver: Receiver<StopMessage>,
        _ack_sender: Sender<StopMessageAcknowledgement>,
    ) {
        let (_consumer_tag, messages_rx) = self
            .rabbitmq_connection
            .subscribe(&self.new_links_queue, "load_automaton")
            .await;
        self.message_loop(messages_rx).await;

        self.rabbitmq_connection.channel.close().await.unwrap();
        self.rabbitmq_connection.connection.close().await.unwrap();
    }

    async fn handle_stop_request(
        sender: Arc<Mutex<Sender<StopMessage>>>,
        ack_receiver_cell: Arc<Mutex<Receiver<StopMessageAcknowledgement>>>,
    ) -> Result<impl warp::Reply, std::convert::Infallible> {
        println!("[web] sending stop message");
        let sender = sender.clone();
        sender.lock().await.send(StopMessage {}).await.unwrap();
        println!("[web] stop message has been sent");
        println!("[web] waiting for acknowledgement message");
        ack_receiver_cell.lock().await.recv().await.unwrap();
        println!("[web] acknowledgement message has been received");
        Ok(Box::new("Stopped!"))
    }

    async fn run_web_server(
        sender: Sender<StopMessage>,
        ack_receiver: Receiver<StopMessageAcknowledgement>,
    ) {
        let health_check =
            warp::get().and(warp::path!("health").and(warp::path::end()).map(|| {
                let healthy_bit = HEALTHY.load(Ordering::Relaxed);
                println!("[web] checking health");
                if healthy_bit == 1 {
                    "Healthy!"
                } else {
                    "Not healthy"
                }
            }));
        let set_limit = warp::put()
            .and(warp::path("set_limit"))
            .and(warp::path::param::<usize>())
            .and(warp::path::end())
            .map(move |x: usize| {
                LOADED_PAGES_LIMIT_AMOUNT.store(x, Ordering::Relaxed);
                println!("[web] updated limit: {}", x);
                x.to_string()
            });
        let get_limit = warp::get()
            .and(warp::path!("get_limit"))
            .and(warp::path::end())
            .map(move || {
                let x = LOADED_PAGES_LIMIT_AMOUNT.load(Ordering::Relaxed);
                x.to_string()
            });
        let ack_receiver_cell = Arc::new(Mutex::new(ack_receiver));
        let sender: Arc<Mutex<Sender<StopMessage>>> = Arc::new(Mutex::new(sender));

        let stop = warp::path!("stop")
            .and(warp::path::end())
            .and_then(move || {
                let ack_receiver_cell: Arc<Mutex<Receiver<StopMessageAcknowledgement>>> =
                    ack_receiver_cell.clone();
                let sender = sender.clone();
                async move { Self::handle_stop_request(sender, ack_receiver_cell).await }
            });

        let routes = health_check.or(stop).or(set_limit).or(get_limit);
        println!("[web] starting server");
        warp::serve(routes).run(([0, 0, 0, 0], 3031)).await;
    }
}

fn main() {
    dotenv().ok();
    LoadAutomaton::run();
}
