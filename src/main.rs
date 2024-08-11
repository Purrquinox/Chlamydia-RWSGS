use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;
use warp::ws::{Message, WebSocket};
use warp::Filter;
use futures::{FutureExt, StreamExt};

type Clients = Arc<Mutex<HashMap<Uuid, mpsc::UnboundedSender<Message>>>>;
type Groups = Arc<Mutex<HashMap<String, HashSet<Uuid>>>>;

#[tokio::main]
async fn main() {
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let groups: Groups = Arc::new(Mutex::new(HashMap::new()));

    let clients_filter = warp::any().map(move || Arc::clone(&clients));
    let groups_filter = warp::any().map(move || Arc::clone(&groups));

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(clients_filter)
        .and(groups_filter)
        .map(|ws: warp::ws::Ws, clients, groups| {
            ws.on_upgrade(move |socket| handle_connection(socket, clients, groups))
        });

    let routes = ws_route.with(warp::cors().allow_any_origin());

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

async fn handle_connection(
    ws: WebSocket,
    clients: Clients,
    groups: Groups,
) {
    let (user_ws_tx, mut user_ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);

    let user_id = Uuid::new_v4();

    clients.lock().unwrap().insert(user_id, tx);

    tokio::task::spawn(rx.forward(user_ws_tx).map(|result| {
        if let Err(e) = result {
            eprintln!("websocket send error: {}", e);
        }
    }));

    while let Some(result) = user_ws_rx.next().await {
        match result {
            Ok(msg) => handle_message(user_id, msg, &clients, &groups).await,
            Err(e) => {
                eprintln!("websocket error(uid={}): {}", user_id, e);
                break;
            }
        }
    }

    clients.lock().unwrap().remove(&user_id);
    remove_user_from_groups(user_id, &groups);
}

async fn handle_message(
    user_id: Uuid,
    msg: Message,
    clients: &Clients,
    groups: &Groups,
) {
    let msg_text = if let Ok(text) = msg.to_str() {
        text
    } else {
        return;
    };

    let parts: Vec<&str> = msg_text.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return;
    }

    match parts[0] {
        "/join" => {
            let group_name = parts[1];
            let mut groups = groups.lock().unwrap();
            let group = groups.entry(group_name.to_string()).or_insert_with(HashSet::new);
            group.insert(user_id);
        }
        "/leave" => {
            let group_name = parts[1];
            let mut groups = groups.lock().unwrap();
            if let Some(group) = groups.get_mut(group_name) {
                group.remove(&user_id);
                if group.is_empty() {
                    groups.remove(group_name);
                }
            }
        }
        "/msg" => {
            let group_name = parts[1];
            let groups = groups.lock().unwrap();
            if let Some(group) = groups.get(group_name) {
                let clients = clients.lock().unwrap();
                for uid in group {
                    if let Some(client) = clients.get(uid) {
                        let _ = client.send(Message::text(msg_text));
                    }
                }
            }
        }
        _ => {}
    }
}

fn remove_user_from_groups(user_id: Uuid, groups: &Groups) {
    let mut groups = groups.lock().unwrap();
    for (_, group) in groups.iter_mut() {
        group.remove(&user_id);
    }
}
