use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use warp::ws::{Message, WebSocket};
use warp::Filter;
use futures::{FutureExt, StreamExt};

type Clients = Arc<Mutex<HashMap<String, HashSet<mpsc::UnboundedSender<Message>>>>>;
type Creators = Arc<Mutex<HashMap<String, String>>>;

#[tokio::main]
async fn main() {
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let creators: Creators = Arc::new(Mutex::new(HashMap::new()));

    let clients_filter = warp::any().map(move || Arc::clone(&clients));
    let creators_filter = warp::any().map(move || Arc::clone(&creators));

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(clients_filter)
        .and(creators_filter)
        .map(|ws: warp::ws::Ws, clients, creators| {
            ws.on_upgrade(move |socket| handle_connection(socket, clients, creators))
        });

    let routes = ws_route.with(warp::cors().allow_any_origin());

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

async fn handle_connection(
    ws: WebSocket,
    clients: Clients,
    creators: Creators,
) {
    let (user_ws_tx, mut user_ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);

    let mut group_id = None;
    let mut is_system = false;

    tokio::task::spawn(rx.forward(user_ws_tx).map(|result| {
        if let Err(e) = result {
            eprintln!("websocket send error: {}", e);
        }
    }));

    while let Some(result) = user_ws_rx.next().await {
        match result {
            Ok(msg) => {
                if group_id.is_none() {
                    if let Ok(payload) = msg.to_str() {
                        let parts: Vec<&str> = payload.splitn(3, ' ').collect();
                        if parts.len() == 3 && parts[0] == "/auth" {
                            group_id = Some(parts[1].to_string());
                            is_system = parts[2] == "SYSTEM";
                            create_or_join_group(&group_id.as_ref().unwrap(), &tx, &clients, &creators, is_system).await;
                        } else {
                            break;
                        }
                    }
                } else if let Some(ref gid) = group_id {
                    broadcast_message(gid, msg, &clients).await;
                }
            }
            Err(e) => {
                eprintln!("websocket error: {}", e);
                break;
            }
        }
    }

    if let Some(gid) = group_id {
        handle_disconnect(&gid, &tx, &clients, &creators, is_system).await;
    }
}

async fn create_or_join_group(
    group_id: &str,
    tx: &mpsc::UnboundedSender<Message>,
    clients: &Clients,
    creators: &Creators,
    is_system: bool,
) {
    let mut clients = clients.lock().unwrap();
    let mut creators = creators.lock().unwrap();

    if is_system {
        if creators.contains_key(group_id) {
            let _ = tx.send(Message::text("/error UUID already in use by another system"));
            return;
        }

        creators.insert(group_id.to_string(), group_id.to_string());
    } else {
        if !creators.contains_key(group_id) {
            let _ = tx.send(Message::text("/error No system found for the provided UUID"));
            return;
        }
    }

    let group_clients = clients.entry(group_id.to_string()).or_insert_with(HashSet::new);
    group_clients.insert(tx.clone());
}


async fn handle_disconnect(
    group_id: &str,
    tx: &mpsc::UnboundedSender<Message>,
    clients: &Clients,
    creators: &Creators,
    is_system: bool,
) {
    let mut clients = clients.lock().unwrap();
    if let Some(group_clients) = clients.get_mut(group_id) {
        group_clients.remove(tx);
        if is_system || group_clients.is_empty() {
            clients.remove(group_id);
            if is_system {
                let mut creators = creators.lock().unwrap();
                creators.remove(group_id);
                send_disconnect_to_services(group_id, &clients).await;
            }
        }
    }
}

async fn send_disconnect_to_services(group_id: &str, clients: &Clients) {
    let clients = clients.lock().unwrap();
    if let Some(group_clients) = clients.get(group_id) {
        for client in group_clients {
            let _ = client.send(Message::text("/disconnect"));
        }
    }
}

async fn broadcast_message(group_id: &str, msg: Message, clients: &Clients) {
    let clients = clients.lock().unwrap();
    if let Some(group_clients) = clients.get(group_id) {
        for client in group_clients {
            let _ = client.send(msg.clone());
        }
    }
}
