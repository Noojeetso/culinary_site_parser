use common_utils::connections::{
    InfluxDB2Connection, RabbitMQConnection, RabbitQueue, RedisConnection,
};
use influxdb2::api::organization::ListOrganizationRequest;

use std::env;

use futures_util::stream::SplitSink;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use amqprs::BasicProperties;
use dotenv::dotenv;
use futures_util::{SinkExt, StreamExt, TryFutureExt};
use influxdb2::models::PostBucketRequest;
// use lazy_static::lazy_static;
use redis::{Commands, ConnectionLike};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use url::Url;
use warp::ws::{Message, WebSocket};
use warp::Filter;

static NEXT_USER_ID: AtomicUsize = AtomicUsize::new(1);
static IS_RUNNING: AtomicUsize = AtomicUsize::new(0);

type Users = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Message>>>>;

// lazy_static! {
//     static ref index_html: String = fs::read_to_string("index.html").unwrap();
// }

#[tokio::main(flavor = "current_thread")]
async fn main() {
    dotenv().ok();
    let users = Users::default();
    let users = warp::any().map(move || users.clone());
    println!("[server] starting");
    let influxdb2_connection = InfluxDB2Connection::new().await;
    let org_id = influxdb2_connection
        .client
        .list_organizations(ListOrganizationRequest::new())
        .await
        .unwrap()
        .orgs
        .iter()
        .filter(|org| org.name == influxdb2_connection.org)
        .map(|org| org.id.clone())
        .next()
        .unwrap()
        .unwrap();
    println!("[server] org_id: {}", org_id);
    match influxdb2_connection
        .client
        .create_bucket(Some(PostBucketRequest::new(
            org_id,
            env::var("INFLUXDB2_BUCKET").unwrap(),
        )))
        .await
    {
        Ok(_) => (),
        Err(e) => {
            if !e.to_string().contains("422") {
                Err(e).unwrap()
            }
        }
    }
    let mut redis_connection = RedisConnection::new().await;
    let mut rabbitmq_connection = RabbitMQConnection::new().await;
    let mut new_links_queue = rabbitmq_connection.new_queue("amqprs.new_links").await;
    rabbitmq_connection
        .bind_queue_to_publish(
            &mut new_links_queue,
            "amq.direct",
            "amqprs.push_to_new_links",
        )
        .await;
    println!("[worker] configured all the connections");

    rabbitmq_connection.purge_queue(&new_links_queue).await;
    redis_connection
        .connection
        .del::<&str, bool>("target_root_url")
        .unwrap();
    redis_connection
        .connection
        .del::<&str, bool>("excluded_links")
        .unwrap();
    redis_connection
        .connection
        .del::<&str, bool>("added_links")
        .unwrap();
    redis_connection
        .connection
        .del::<&str, bool>("loaded_pages_amount_limit")
        .unwrap();
    redis_connection
        .connection
        .del::<&str, bool>("started_loading_pages_amount")
        .unwrap();

    let new_links_queue = Arc::new(new_links_queue);
    let redis_connection = Arc::new(Mutex::new(redis_connection));
    let rabbitmq_connection = Arc::new(rabbitmq_connection);
    let influxdb2_connection = Arc::new(Mutex::new(influxdb2_connection));

    let websocket_path =
        warp::path("ws")
            .and(warp::ws())
            .and(users)
            .map(move |ws: warp::ws::Ws, users| {
                let redis_connection = redis_connection.clone();
                let influxdb2_connection = influxdb2_connection.clone();
                let rabbitmq_connection = rabbitmq_connection.clone();
                let new_links_queue = new_links_queue.clone();
                ws.on_upgrade(move |socket| {
                    user_connected(
                        socket,
                        users,
                        influxdb2_connection,
                        redis_connection,
                        rabbitmq_connection,
                        new_links_queue,
                    )
                })
            });

    let index_html = std::include_str!("index.html");

    let index = warp::path::end().map(move || {
        // let index_html: String = fs::read_to_string("index.html").unwrap();
        warp::reply::html(index_html)
    });

    let routes = index.or(websocket_path);

    warp::serve(routes).run(([0, 0, 0, 0], 3030)).await;
}

fn parse_request(message: &str) -> &str {
    let comma_index = message.find(':').unwrap();
    let (_prefix, mut link) = message.split_at(comma_index);
    link = &link[2..];
    link
}

async fn publish_start_url(
    rabbitmq_connection: Arc<RabbitMQConnection>,
    new_links_queue: Arc<RabbitQueue>,
    new_link: String,
    influxdb2_connection: Arc<Mutex<InfluxDB2Connection>>,
    redis_connection: Arc<Mutex<RedisConnection>>,
) {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let message_id = timestamp.as_nanos().to_string();
    let mut properties = BasicProperties::default();
    properties.with_message_id(message_id.as_str());
    let task_id = message_id.as_str();
    redis_connection
        .lock()
        .await
        .connection
        .set::<&str, &str, bool>("target_root_url", new_link.as_str())
        .unwrap();
    redis_connection
        .lock()
        .await
        .connection
        .set::<&str, u64, bool>("started_loading_pages_amount", 0u64)
        .unwrap();
    let new_message_content: String = new_link;
    rabbitmq_connection
        .channel
        .basic_publish(
            properties,
            new_message_content.into_bytes(),
            new_links_queue.publish_args.clone().unwrap(),
        )
        .await
        .unwrap();
    influxdb2_connection
        .lock()
        .await
        .store_timestamp("load_automaton_queue_in", task_id)
        .await;
}

async fn send_reply(user_ws_tx: &mut SplitSink<WebSocket, Message>, text: &str) {
    let new_message = warp::ws::Message::text(text);
    user_ws_tx
        .send(new_message)
        .unwrap_or_else(|e| {
            eprintln!("[server] websocket send error: {}", e);
        })
        .await;
}

fn parse_link_to_exclude(link: &str) -> Option<String> {
    let link = link.trim();
    let link = link
        .strip_suffix("\r\n")
        .or(link.strip_suffix("\n"))
        .unwrap_or(link)
        .trim_end_matches('/');
    println!("[server] parsed link: '{}'", link);
    let url = Url::parse(link);
    if url.is_err() {
        eprintln!("[server] Warning: incorrect path: {}", link);
        return None;
    }
    Some(link.to_owned())
}

async fn user_connected(
    ws: WebSocket,
    users: Users,
    influxdb2_connection: Arc<Mutex<InfluxDB2Connection>>,
    redis_connection: Arc<Mutex<RedisConnection>>,
    rabbitmq_connection: Arc<RabbitMQConnection>,
    new_links_queue: Arc<RabbitQueue>,
) {
    let my_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);

    eprintln!("[server] new user: {}", my_id);

    let (mut user_ws_tx, mut user_ws_rx) = ws.split();

    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    let mut rx = UnboundedReceiverStream::new(rx);

    tokio::task::spawn({
        async move {
            while let Some(message) = rx.next().await {
                let message_contents = message.to_str().unwrap();
                println!("[server] content: {}", message_contents);
                if message_contents.starts_with("start_url: ") {
                    let is_running = IS_RUNNING.fetch_or(1, Ordering::Relaxed);
                    if is_running == 0 {
                        let link = parse_request(message_contents);
                        println!("[server] link: {}", link);

                        publish_start_url(
                            rabbitmq_connection.clone(),
                            new_links_queue.clone(),
                            link.to_owned(),
                            influxdb2_connection.clone(),
                            redis_connection.clone(),
                        )
                        .await;
                    }
                    send_reply(&mut user_ws_tx, "state: working").await;
                } else if message_contents.starts_with("excluded: ") {
                    let link: &str = parse_request(message_contents);
                    match parse_link_to_exclude(link) {
                        Some(link) => {
                            redis_connection
                                .lock()
                                .await
                                .connection
                                .sadd::<&str, &str, usize>("excluded_links", link.as_str())
                                .unwrap();
                            send_reply(&mut user_ws_tx, link.as_str()).await;
                        }
                        None => continue,
                    }
                } else if message_contents.to_lowercase().starts_with("set_limit") {
                    let link: &str = parse_request(message_contents);
                    match link.parse::<usize>() {
                        Ok(amount) => {
                            let mut response_text = format!("Current limit: {}", amount);
                            if amount == 0 {
                                response_text.push_str(" (no restrictions)");
                            }
                            let client = reqwest::Client::new();
                            let resp = client
                                .put(format!("http://load_automaton:3031/set_limit/{}", amount))
                                // .body(amount.to_string())
                                .send()
                                .await
                                .unwrap();
                            println!(
                                "[server] load_automaton response: {}",
                                resp.text().await.unwrap()
                            );
                            redis_connection
                                .lock()
                                .await
                                .connection
                                .set::<&str, usize, bool>("loaded_pages_amount_limit", amount)
                                .unwrap();
                            send_reply(&mut user_ws_tx, response_text.as_str()).await;
                        }
                        Err(_) => continue,
                    }
                } else if message_contents.to_lowercase() == "flush Redis" {
                    let cmd = redis::cmd("FLUSHDB");
                    match redis_connection.lock().await.connection.req_command(&cmd) {
                        Ok(_) => {
                            send_reply(&mut user_ws_tx, "Flushed: Redis").await;
                        }
                        Err(_) => {
                            send_reply(&mut user_ws_tx, "Flush error: Redis").await;
                        }
                    };
                } else if message_contents.to_lowercase() == "flush influxDB" {
                    let query = influxdb2::models::query::Query::new(format!(
                        "influx delete --bucket {}",
                        influxdb2_connection.lock().await.bucket,
                    ));
                    influxdb2_connection
                        .lock()
                        .await
                        .client
                        .query_raw(Some(query))
                        .await
                        .unwrap();
                }
            }
        }
    });

    users.write().await.insert(my_id, tx);
    while let Some(result) = user_ws_rx.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("[server] websocket error(uid={}): {}", my_id, e);
                break;
            }
        };
        user_message(my_id, msg, &users).await;
    }

    user_disconnected(my_id, &users).await;
}

async fn user_message(_my_id: usize, msg: Message, users: &Users) {
    let msg = if let Ok(s) = msg.to_str() {
        s
    } else {
        return;
    };
    if msg.is_empty() {
        return;
    }
    let new_msg = format!("{}", msg);
    for (&_uid, tx) in users.read().await.iter() {
        if let Err(_disconnected) = tx.send(Message::text(new_msg.clone())) {}
    }
}

async fn user_disconnected(my_id: usize, users: &Users) {
    eprintln!("[server] disconnected user: {}", my_id);
    users.write().await.remove(&my_id);
}
