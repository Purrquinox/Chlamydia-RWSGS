use futures::{FutureExt, StreamExt};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::time::{self, Duration};
use warp::ws::{Message, WebSocket};
use warp::Filter;

#[derive(Clone)]
struct ClientSender {
    sender: Arc<mpsc::UnboundedSender<Message>>,
}

impl PartialEq for ClientSender {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.sender, &other.sender)
    }
}

impl Eq for ClientSender {}

impl Hash for ClientSender {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.sender).hash(state);
    }
}

type Clients = Arc<Mutex<HashMap<String, HashSet<ClientSender>>>>;
type Systems = Arc<Mutex<HashMap<String, String>>>;

#[tokio::main]
async fn main() {
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let systems: Systems = Arc::new(Mutex::new(HashMap::new()));

    let clients_filter = warp::any().map(move || Arc::clone(&clients));
    let systems_filter = warp::any().map(move || Arc::clone(&systems));

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(clients_filter)
        .and(systems_filter)
        .map(|ws: warp::ws::Ws, clients, systems| {
            ws.on_upgrade(move |socket| handle_connection(socket, clients, systems))
        });

    let routes = ws_route.with(warp::cors().allow_any_origin());

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

async fn handle_connection(ws: WebSocket, clients: Clients, systems: Systems) {
    let (user_ws_tx, mut user_ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    let rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);

    let tx = ClientSender {
        sender: Arc::new(tx),
    };

    let mut group_id = None;
    let mut is_system = false;
    let mut last_pong = time::Instant::now();
    let ping_interval = Duration::from_secs(30);
    let timeout = Duration::from_secs(60);

    tokio::task::spawn(rx.map(Ok).forward(user_ws_tx).map(|result| {
        if let Err(e) = result {
            eprintln!("websocket send error: {}", e);
        }
    }));

    // PING loop
    let ping_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(ping_interval);
        loop {
            interval.tick().await;
            if last_pong.elapsed() > timeout {
                eprintln!("Connection timed out, disconnecting.");
                break;
            }
            if ping_tx.sender.send(Message::ping(vec![])).is_err() {
                break;
            }
        }
    });

    while let Some(result) = user_ws_rx.next().await {
        match result {
            Ok(msg) => {
                if msg.is_pong() {
                    last_pong = time::Instant::now();
                } else if let Ok(payload) = msg.to_str() {
                    handle_incoming_message(
                        payload,
                        &mut group_id,
                        &tx,
                        clients.clone(),
                        systems.clone(),
                        &mut is_system,
                    )
                    .await;
                }
            }
            Err(e) => {
                eprintln!("websocket error: {}", e);
                break;
            }
        }
    }

    if let Some(gid) = group_id {
        handle_disconnect(gid, &tx, clients.clone(), systems.clone(), is_system).await;
    }
}

async fn handle_incoming_message(
    payload: &str,
    group_id: &mut Option<String>,
    tx: &ClientSender,
    clients: Clients,
    systems: Systems,
    is_system: &mut bool,
) {
    let message: serde_json::Value = match serde_json::from_str(payload) {
        Ok(val) => val,
        Err(_) => {
            let _ = tx.sender.send(Message::text(
                json!({
                    "type": "ERROR",
                    "message": "Invalid message format"
                })
                .to_string(),
            ));
            return;
        }
    };

    match message["type"].as_str() {
        Some("CONNECT") => {
            if let Some(id) = message["group_id"].as_str() {
                if let Some(role) = message["role"].as_str() {
                    if role == "SYSTEM" || role == "SERVICE" {
                        *group_id = Some(id.to_string());
                        *is_system = role == "SYSTEM";
                        create_or_join_group(id, tx, clients, systems, *is_system).await;
                        let _ = tx.sender.send(Message::text(
                            json!({
                                "type": "LOG",
                                "message": "Connected to group"
                            })
                            .to_string(),
                        ));
                    } else {
                        let _ = tx.sender.send(Message::text(
                            json!({
                                "type": "ERROR",
                                "message": "Invalid role, must be 'SYSTEM' or 'SERVICE'"
                            })
                            .to_string(),
                        ));
                    }
                }
            }
        }
        Some("LOG") | Some("DEBUG") | Some("ERROR") => {
            if let Some(ref gid) = group_id {
                broadcast_message(gid, Message::text(payload.to_string()), clients).await;
            }
        }
        Some(_) => {
            let _ = tx.sender.send(Message::text(
                json!({
                    "type": "ERROR",
                    "message": "Unknown message type"
                })
                .to_string(),
            ));
        }
        None => {
            let _ = tx.sender.send(Message::text(
                json!({
                    "type": "ERROR",
                    "message": "Invalid message, 'type' field missing"
                })
                .to_string(),
            ));
        }
    }
}

async fn create_or_join_group(
    group_id: &str,
    tx: &ClientSender,
    clients: Clients,
    systems: Systems,
    is_system: bool,
) {
    // Lock the mutexes locally inside the function
    let mut clients_lock = clients.lock().unwrap();
    let mut systems_lock = systems.lock().unwrap();

    if is_system {
        if systems_lock.contains_key(group_id) {
            let _ = tx.sender.send(Message::text(
                json!({
                    "type": "ERROR",
                    "message": "UUID already in use by another system"
                })
                .to_string(),
            ));
            return;
        }
        systems_lock.insert(group_id.to_string(), group_id.to_string());
    } else {
        if !systems_lock.contains_key(group_id) {
            let _ = tx.sender.send(Message::text(
                json!({
                    "type": "ERROR",
                    "message": "No system found for the provided UUID"
                })
                .to_string(),
            ));
            return;
        }
    }

    let group_clients = clients_lock
        .entry(group_id.to_string())
        .or_insert_with(HashSet::new);
    group_clients.insert(tx.clone());
}

async fn handle_disconnect(
    group_id: String,
    tx: &ClientSender,
    clients: Clients,
    systems: Systems,
    is_system: bool,
) {
    {
        let mut clients_lock = clients.lock().unwrap();
        if let Some(group_clients) = clients_lock.get_mut(&group_id) {
            group_clients.remove(tx);
            if is_system || group_clients.is_empty() {
                clients_lock.remove(&group_id);

                if is_system {
                    let mut systems_lock = systems.lock().unwrap();
                    systems_lock.remove(&group_id);
                }
            }
        }
    }

    broadcast_message(
        &group_id,
        Message::text(
            json!({
                "type": "DISCONNECT",
                "message": "System disconnected"
            })
            .to_string(),
        ),
        clients,
    )
    .await;
}

async fn broadcast_message(group_id: &str, msg: Message, clients: Clients) {
    // Lock the mutex locally inside the function
    let clients_lock = clients.lock().unwrap();
    if let Some(group_clients) = clients_lock.get(group_id) {
        for client in group_clients {
            let _ = client.sender.send(msg.clone());
        }
    }
}
