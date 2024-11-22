use std::env;

use amqprs::{
    callbacks::{DefaultChannelCallback, DefaultConnectionCallback},
    channel::{
        BasicConsumeArguments, BasicPublishArguments, ConsumerMessage, QueueBindArguments,
        QueueDeclareArguments, QueuePurgeArguments,
    },
    connection::OpenConnectionArguments,
};
use futures::prelude::*;
use influxdb2::models::DataPoint;
use mongodb::{Client, Collection};
use tokio::sync::mpsc::UnboundedReceiver;

pub struct RedisConnection {
    pub client: redis::Client,
    pub connection: redis::Connection,
}

impl RedisConnection {
    pub async fn new() -> Self {
        println!("[worker] connecting to redis");
        let client = redis::Client::open("redis://redis:6379/").unwrap();
        let connection = client.get_connection().unwrap();
        println!("[worker] connected to redis");

        Self { client, connection }
    }
}

pub struct InfluxDB2Connection {
    pub org: String,
    pub bucket: String,
    pub client: influxdb2::Client,
}

impl InfluxDB2Connection {
    pub async fn new() -> Self {
        println!("[worker] connecting to influxdb");
        let org = env::var("INFLUXDB2_ORG").unwrap();
        let bucket = env::var("INFLUXDB2_BUCKET").unwrap();
        let token = env::var("INFLUXDB_ADMIN_TOKEN").unwrap();
        let influx_url = "http://influxdb:8086";
        let client = influxdb2::Client::new(influx_url, org.clone(), token);
        println!("[worker] connected to influxdb");

        Self {
            org,
            bucket,
            client,
        }
    }

    pub async fn store_timestamp<'a>(&self, measure: &str, task_id: &'a str) {
        let data_point = DataPoint::builder(measure)
            .field("task_id", task_id)
            .build()
            .unwrap();

        println!("[server] new task_id: {}", task_id);
        self.client
            .write(
                self.bucket.as_str(),
                stream::iter(std::iter::once(data_point)),
            )
            .await
            .unwrap();
    }
}

pub struct RabbitQueue {
    pub name: String,
    pub routing_key: Option<String>,
    pub exchange_name: Option<String>,
    pub publish_args: Option<BasicPublishArguments>,
}

pub struct RabbitMQConnection {
    pub connection: amqprs::connection::Connection,
    pub channel: amqprs::channel::Channel,
}

impl RabbitMQConnection {
    pub async fn new() -> Self {
        println!("[worker] connecting to rabbitmq");
        let connection = amqprs::connection::Connection::open(&OpenConnectionArguments::new(
            "rabbitmq",
            5672,
            env::var("RABBITMQ_DEFAULT_USER").unwrap().as_str(),
            env::var("RABBITMQ_DEFAULT_PASS").unwrap().as_str(),
        ))
        .await
        .unwrap();
        connection
            .register_callback(DefaultConnectionCallback)
            .await
            .unwrap();
        println!("[worker] connected to rabbitmq");

        println!("[worker] creating rabbitmq channel");
        let channel = connection.open_channel(None).await.unwrap();
        channel
            .register_callback(DefaultChannelCallback)
            .await
            .unwrap();
        println!("[worker] rabbitmq channel created");

        Self {
            connection,
            channel,
        }
    }

    pub async fn new_queue(&self, name: &str) -> RabbitQueue {
        self.channel
            .queue_declare(QueueDeclareArguments::durable_client_named(name))
            .await
            .unwrap()
            .unwrap();
        RabbitQueue {
            name: name.to_owned(),
            routing_key: None,
            exchange_name: None,
            publish_args: None,
        }
    }

    pub async fn bind_queue_to_publish(
        &mut self,
        queue: &mut RabbitQueue,
        exchange_name: &str,
        routing_key: &str,
    ) {
        queue.routing_key = Some(routing_key.to_owned());
        queue.exchange_name = Some(exchange_name.to_owned());
        queue.publish_args = Some(BasicPublishArguments::new(exchange_name, routing_key));
        self.channel
            .queue_bind(QueueBindArguments::new(
                queue.name.as_str(),
                exchange_name,
                routing_key,
            ))
            .await
            .unwrap();
    }

    pub async fn subscribe(
        &self,
        new_links_queue: &RabbitQueue,
        consumer_tag: &str,
    ) -> (String, UnboundedReceiver<ConsumerMessage>) {
        println!(
            "[subscribe]: rmq channel open: {} {} {} {}",
            self.connection.is_open(),
            self.channel.is_open(),
            self.channel.is_connection_open(),
            self.channel.connection_name()
        );
        let args = BasicConsumeArguments::new(&new_links_queue.name, consumer_tag); // "load_automaton");
        self.channel.basic_consume_rx(args).await.unwrap()
    }

    pub async fn purge_queue(&self, queue: &RabbitQueue) {
        self.channel
            .queue_purge(QueuePurgeArguments::new(queue.name.as_str()))
            .await
            .unwrap();
    }
}

pub struct MongoDBConnection<T: Send + Sync> {
    pub client: mongodb::Client,
    pub collection: mongodb::Collection<T>,
}

impl<T: Send + Sync> MongoDBConnection<T> {
    pub async fn new() -> Self {
        println!("[worker] connecting to mongodb");
        let mongodb_connection_uri = format!(
            "mongodb://{}:{}@mongodb:27017",
            env::var("MONGO_INITDB_ROOT_USERNAME").unwrap().as_str(),
            env::var("MONGO_INITDB_ROOT_PASSWORD").unwrap().as_str(),
        );
        let client = Client::with_uri_str(mongodb_connection_uri).await.unwrap();
        let collection: Collection<T> = client.database("parsed_recipes").collection("recipes");
        println!("[worker] connected to mongodb");
        Self { client, collection }
    }
}
