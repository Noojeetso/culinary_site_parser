mod elementaree_parser;
mod link_parser;

use common_utils::connections::{
    InfluxDB2Connection, RabbitMQConnection, RabbitQueue, RedisConnection,
};
use common_utils::recipe_parser::{RecipeParser, Task};
use elementaree_parser::ElementareeRecipeParser;
use link_parser::LinkParser;

use std::env;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;

use amqprs::{
    channel::{BasicAckArguments, ConsumerMessage},
    BasicProperties,
};
use bincode;
use dotenv::dotenv;
use redis;
use redis::Commands;
use select::document::Document;
use tokio::runtime;
use tokio::sync::mpsc::{channel, Receiver, Sender, UnboundedReceiver};
use tokio::sync::Mutex;
use url::Url;
use warp::Filter;

static HEALTHY: AtomicUsize = AtomicUsize::new(0);

struct StopMessage;
struct StopMessageAcknowledgement;

struct ParseAutomaton {
    issue_id: u64,
    redis_connection: RedisConnection,
    rabbitmq_connection: RabbitMQConnection,
    new_documents_queue: RabbitQueue,
    new_links_queue: RabbitQueue,
    new_recipes_queue: RabbitQueue,
    influxdb2_connection: InfluxDB2Connection,
}

impl ParseAutomaton {
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
                    let parse_automaton = Self::new().await;
                    parse_automaton.run_worker(receiver, ack_sender).await
                });
            });
            s.spawn(|| {
                let runtime = runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()
                    .unwrap();
                runtime
                    .block_on(async { ParseAutomaton::run_web_server(sender, ack_receiver).await });
            });
        });
    }

    async fn new() -> Self {
        let issue_id: u64 = env::var("ISSUE_ID").unwrap().parse().unwrap();
        println!("[worker] starting");
        let redis_connection = RedisConnection::new().await;
        let influxdb2_connection = InfluxDB2Connection::new().await;
        let mut rabbitmq_connection = RabbitMQConnection::new().await;
        let new_documents_queue = rabbitmq_connection.new_queue("amqprs.new_documents").await;
        rabbitmq_connection.purge_queue(&new_documents_queue).await;
        let mut new_links_queue = rabbitmq_connection.new_queue("amqprs.new_links").await;
        rabbitmq_connection
            .bind_queue_to_publish(
                &mut new_links_queue,
                "amq.direct",
                "amqprs.push_to_new_links",
            )
            .await;
        let mut new_recipes_queue = rabbitmq_connection.new_queue("amqprs.new_recipes").await;
        rabbitmq_connection
            .bind_queue_to_publish(
                &mut new_recipes_queue,
                "amq.direct",
                "amqprs.push_to_new_recipes",
            )
            .await;
        println!("[worker] configured all the connections");
        Self {
            issue_id,
            redis_connection,
            rabbitmq_connection,
            new_documents_queue,
            new_links_queue,
            new_recipes_queue,
            influxdb2_connection,
        }
    }

    async fn run_worker(
        mut self,
        _receiver: Receiver<StopMessage>,
        _ack_sender: Sender<StopMessageAcknowledgement>,
    ) {
        let (_consumer_tag, messages_rx) = self
            .rabbitmq_connection
            .subscribe(&self.new_documents_queue, "parse_automaton")
            .await;

        let excluded_links = self
            .redis_connection
            .connection
            .smembers::<&str, Vec<String>>("excluded_links")
            .unwrap();
        self.message_loop(messages_rx, &excluded_links).await;

        self.rabbitmq_connection.channel.close().await.unwrap();
        self.rabbitmq_connection.connection.close().await.unwrap();
    }

    async fn message_loop(
        &mut self,
        mut messages_rx: UnboundedReceiver<ConsumerMessage>,
        excluded_links: &Vec<String>,
    ) {
        println!("[worker] entering message handler loop");
        while let Some(message) = messages_rx.recv().await {
            let message_properties = message.basic_properties.unwrap();
            let message_id = message_properties.message_id().unwrap();
            let task_id = message_id.as_str();
            self.influxdb2_connection
                .store_timestamp("parse_automaton_start", task_id)
                .await;
            let message_content_raw = message.content.unwrap();
            let (new_link, new_document_string) =
                Self::parse_message(std::str::from_utf8(&message_content_raw).unwrap());
            let task = Task {
                id: message_id.parse().unwrap(),
                issue_id: self.issue_id,
                document_string: new_document_string.to_owned(),
                url: new_link.to_owned(),
            };
            match ElementareeRecipeParser::parse_recipe(&task) {
                Ok(recipe_opt) => {
                    if let Some(recipe) = recipe_opt {
                        let recipe_bytes = bincode::serialize(&recipe).unwrap();
                        self.influxdb2_connection
                            .store_timestamp("parse_automaton_end", task_id)
                            .await;
                        let mut properties = BasicProperties::default();
                        properties.with_message_id(message_id);
                        self.rabbitmq_connection
                            .channel
                            .basic_publish(
                                properties,
                                recipe_bytes,
                                self.new_recipes_queue.publish_args.clone().unwrap(),
                            )
                            .await
                            .unwrap();
                    } else {
                        println!("[worker] no recipe :(");
                    }
                }
                Err(e) => eprintln!("Error while parsing recipe: {}", e),
            }
            println!("[worker] parsing doc via select");

            let document = Document::from(new_document_string);
            let target_root_url: String = self
                .redis_connection
                .connection
                .get("target_root_url")
                .unwrap();
            let mut new_urls = LinkParser::parse_links(
                &document,
                &Url::parse(target_root_url.as_str()).unwrap(),
                &excluded_links,
            )
            .unwrap();
            new_urls.retain(|url| {
                !self
                    .redis_connection
                    .connection
                    .sismember::<&str, &str, bool>("added_links", url.as_str())
                    .unwrap()
            });
            for link in new_urls {
                let res = self
                    .redis_connection
                    .connection
                    .sadd::<&str, &str, bool>("added_links", link.as_str())
                    .unwrap();
                if res {
                    println!("[worker] new_link: {}", link.as_str());
                    let (properties, new_task_id) = Self::generate_message_properties().await;
                    self.influxdb2_connection
                        .store_timestamp("load_automaton_queue_in", new_task_id.as_str())
                        .await;
                    self.rabbitmq_connection
                        .channel
                        .basic_publish(
                            properties,
                            link.as_str().as_bytes().to_owned(),
                            self.new_links_queue.publish_args.clone().unwrap(),
                        )
                        .await
                        .unwrap();
                }
            }
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

    fn parse_message(message: &str) -> (&str, &str) {
        let first_comma_index = message.find(':').unwrap();
        let second_comma_index =
            message[first_comma_index + 1..].find(':').unwrap() + first_comma_index + 1;
        let (link, mut document) = message.split_at(second_comma_index);
        document = &document[1..];
        (link, document)
    }

    async fn generate_message_properties() -> (BasicProperties, String) {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let message_id = timestamp.as_nanos().to_string();
        let mut properties = BasicProperties::default();
        properties.with_message_id(message_id.as_str());
        (properties, message_id)
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
        let health_check = warp::path!("health").map(|| {
            let healthy_bit = HEALTHY.load(Ordering::Relaxed);
            println!("[web] checking health");
            if healthy_bit == 1 {
                "Healthy!"
            } else {
                "Not healthy"
            }
        });
        let statistics = warp::path!("statistics").map(|| "Statistics!");
        let ack_receiver_cell = Arc::new(Mutex::new(ack_receiver));
        let sender = Arc::new(Mutex::new(sender));

        let stop = warp::path!("stop").and_then(move || {
            let ack_receiver_cell: Arc<Mutex<Receiver<StopMessageAcknowledgement>>> =
                ack_receiver_cell.clone();
            let sender = sender.clone();
            async move { Self::handle_stop_request(sender, ack_receiver_cell).await }
        });

        let routes = warp::get().and(statistics.or(health_check).or(stop));
        println!("[web] starting server");
        warp::serve(routes).run(([0, 0, 0, 0], 3032)).await;
    }
}

fn main() {
    dotenv().ok();
    ParseAutomaton::run();
}
