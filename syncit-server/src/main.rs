use std::sync::{Arc, Mutex};

use anyhow::Result;
use automerge::{AutoCommit, sync::SyncDoc};
use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    routing::get,
};
use serde::Serialize;
use tokio::sync::broadcast;
use tower_http::trace::{self, TraceLayer};
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    doc: Arc<Mutex<AutoCommit>>,
    /// Fired whenever the doc's heads move, so every connection re-syncs.
    changed: broadcast::Sender<()>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,syncit_server=debug")),
        )
        .with_target(false)
        .compact()
        .init();

    tracing::debug!("Setting up automerge document...");

    // Create an automerge document in-memory
    let mut doc = AutoCommit::new();

    // Initialize the document with our state
    let sd = syncit_core::SyncItDoc::new();
    autosurgeon::reconcile(&mut doc, &sd)?;

    tracing::debug!("Automerge document set up.");

    let (changed, _) = broadcast::channel(16);
    let state = AppState {
        doc: Arc::new(Mutex::new(doc)),
        changed,
    };

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/api/data", get(get_data))
        .route("/api/ws", get(ws_handler))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3003").await?;

    tracing::info!(port = "3003", "Starting");

    axum::serve(listener, app).await?;

    Ok(())
}

async fn root() -> &'static str {
    "Hello, World!"
}

/// JSON snapshot of the current document, for debugging.
async fn get_data(State(state): State<AppState>) -> Json<DocSnapshot> {
    let sd: syncit_core::SyncItDoc = {
        let doc = state.doc.lock().unwrap();
        autosurgeon::hydrate(&*doc).expect("doc should always hydrate to SyncItDoc")
    };
    Json(DocSnapshot {
        name: sd.name,
        active: sd.active,
        count: sd.count.value(),
        desc: sd.desc.as_str().to_string(),
    })
}

#[derive(Serialize)]
struct DocSnapshot {
    name: String,
    active: bool,
    count: i64,
    desc: String,
}

async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|socket| handle_sync(socket, state))
}

/// Run the automerge sync protocol with one peer until the socket closes.
async fn handle_sync(mut socket: WebSocket, state: AppState) {
    let mut sync_state = automerge::sync::State::new();
    let mut changed = state.changed.subscribe();

    tracing::debug!("Peer connected");

    if !send_pending(&mut socket, &state, &mut sync_state).await {
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    let msg = match automerge::sync::Message::decode(&bytes) {
                        Ok(msg) => msg,
                        Err(err) => {
                            tracing::warn!(?err, "Ignoring undecodable sync message");
                            continue;
                        }
                    };
                    {
                        let mut doc = state.doc.lock().unwrap();
                        let heads_before = doc.get_heads();
                        if let Err(err) = doc.sync().receive_sync_message(&mut sync_state, msg) {
                            tracing::warn!(?err, "Failed to apply sync message; closing");
                            return;
                        }
                        if doc.get_heads() != heads_before {
                            tracing::debug!("Doc changed by peer; notifying connections");
                            let _ = state.changed.send(());
                        }
                    }
                    if !send_pending(&mut socket, &state, &mut sync_state).await {
                        return;
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    tracing::debug!("Peer disconnected");
                    return;
                }
                Some(Ok(_)) => {} // ignore text/ping/pong frames
                Some(Err(err)) => {
                    tracing::debug!(?err, "WebSocket error");
                    return;
                }
            },
            recv = changed.recv() => {
                if matches!(recv, Err(broadcast::error::RecvError::Closed)) {
                    return;
                }
                // Lagged is fine: sync messages are generated from current state
                if !send_pending(&mut socket, &state, &mut sync_state).await {
                    return;
                }
            }
        }
    }
}

/// Send all pending sync messages to the peer. Returns false if the socket died.
async fn send_pending(
    socket: &mut WebSocket,
    state: &AppState,
    sync_state: &mut automerge::sync::State,
) -> bool {
    loop {
        // Scope the lock so it's released before sending on the socket
        let msg = {
            let mut doc = state.doc.lock().unwrap();
            doc.sync().generate_sync_message(sync_state)
        };
        match msg {
            Some(msg) => {
                if socket.send(Message::Binary(msg.encode().into())).await.is_err() {
                    return false;
                }
            }
            None => return true,
        }
    }
}
