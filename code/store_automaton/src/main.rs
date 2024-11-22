use common_utils::connections::{
    InfluxDB2Connection, MongoDBConnection, RabbitMQConnection, RabbitQueue,
};
use common_utils::recipe_parser::Recipe;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use amqprs::channel::{BasicAckArguments, ConsumerMessage};
use bincode;
use dotenv::dotenv;
use tokio::runtime;
use tokio::sync::mpsc::{channel, Receiver, Sender, UnboundedReceiver};
use tokio::sync::Mutex;
use warp::Filter;

static HEALTHY: AtomicUsize = AtomicUsize::new(0);

struct StopMessage;
struct StopMessageAcknowledgement;

struct StoreAutomaton {
    rabbitmq_connection: RabbitMQConnection,
    new_recipes_queue: RabbitQueue,
    influxdb2_connection: InfluxDB2Connection,
    mongodb_connection: MongoDBConnection<Recipe>,
}

impl StoreAutomaton {
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
                    let store_automaton = Self::new().await;
                    store_automaton.handle_messages(receiver, ack_sender).await
                });
            });
            s.spawn(|| {
                let runtime = runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()
                    .unwrap();
                runtime.block_on(async { Self::handle_web_server(sender, ack_receiver).await });
            });
        });
    }

    async fn new() -> Self {
        println!("[worker] starting");
        let influxdb2_connection = InfluxDB2Connection::new().await;
        let rabbitmq_connection = RabbitMQConnection::new().await;
        let mongodb_connection = MongoDBConnection::new().await;
        let new_recipes_queue = rabbitmq_connection.new_queue("amqprs.new_recipes").await;
        rabbitmq_connection.purge_queue(&new_recipes_queue).await;
        println!("[worker] configured all the connections");
        Self {
            rabbitmq_connection,
            new_recipes_queue,
            influxdb2_connection,
            mongodb_connection,
        }
    }

    async fn message_loop(&self, mut messages_rx: UnboundedReceiver<ConsumerMessage>) {
        println!("[worker] entering message handler loop");
        HEALTHY.store(1, Ordering::Relaxed);
        while let Some(message) = messages_rx.recv().await {
            let message_properties = message.basic_properties.unwrap();
            let message_id = message_properties.message_id().unwrap();
            let task_id = message_id;
            self.influxdb2_connection
                .store_timestamp("store_automaton_start", task_id.as_str())
                .await;
            let message_content_raw = message.content.unwrap();
            let recipe: Recipe = bincode::deserialize(&message_content_raw).unwrap();
            println!("[worker] recipe: {:#?}", recipe);
            let insert_one_result = self
                .mongodb_connection
                .collection
                .insert_one(recipe)
                .await
                .unwrap();
            println!(
                "[worker] inserted recipe with id: {}",
                insert_one_result.inserted_id
            );
            self.influxdb2_connection
                .store_timestamp("store_automaton_end", task_id.as_str())
                .await;
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

    async fn handle_messages(
        self,
        _receiver: Receiver<StopMessage>,
        _ack_sender: Sender<StopMessageAcknowledgement>,
    ) {
        let (_consumer_tag, messages_rx) = self
            .rabbitmq_connection
            .subscribe(&self.new_recipes_queue, "store_automaton")
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

    async fn handle_web_server(
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
        let ack_receiver_cell = Arc::new(Mutex::new(ack_receiver));
        let sender = Arc::new(Mutex::new(sender));

        let stop = warp::path!("stop").and_then(move || {
            let ack_receiver_cell: Arc<Mutex<Receiver<StopMessageAcknowledgement>>> =
                ack_receiver_cell.clone();
            let sender = sender.clone();
            async move { Self::handle_stop_request(sender, ack_receiver_cell).await }
        });

        let routes = warp::get().and(health_check.or(stop));
        println!("[web] starting server");
        warp::serve(routes).run(([0, 0, 0, 0], 3033)).await;
    }
}

fn main() {
    dotenv().ok();
    StoreAutomaton::run();
}
