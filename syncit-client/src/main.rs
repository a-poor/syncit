//! Tiny CLI client for exercising the sync server.
//!
//! Usage: cargo run -p syncit-client -- [increment|edit|watch]
//!
//! - increment: sync, bump the counter by 1, push the change, print the doc
//! - edit: sync, append to the desc text, push the change, print the doc
//! - watch: stay connected and print the doc whenever it changes

use std::time::Duration;

use anyhow::{Context, Result, bail};
use automerge::{AutoCommit, sync::SyncDoc};
use futures_util::{SinkExt, StreamExt};
use syncit_core::SyncItDoc;
use tokio_tungstenite::tungstenite::Message;

const SERVER_URL: &str = "ws://127.0.0.1:3003/api/ws";
const RECV_TIMEOUT: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "watch".into());

    let (mut socket, _) = tokio_tungstenite::connect_async(SERVER_URL)
        .await
        .context("failed to connect (is the server running?)")?;

    let mut doc = AutoCommit::new();
    let mut sync_state = automerge::sync::State::new();

    // Initial sync: pump until we generate nothing and hear nothing
    pump_until_quiet(&mut socket, &mut doc, &mut sync_state).await?;
    let sd: SyncItDoc = autosurgeon::hydrate(&doc)?;
    println!("Synced with server: {sd:?}");

    match mode.as_str() {
        "increment" => {
            let mut sd: SyncItDoc = autosurgeon::hydrate(&doc)?;
            sd.count.increment(1);
            autosurgeon::reconcile(&mut doc, &sd)?;
            pump_until_quiet(&mut socket, &mut doc, &mut sync_state).await?;
            let sd: SyncItDoc = autosurgeon::hydrate(&doc)?;
            println!("Pushed increment. Final doc: {sd:?}");
        }
        "edit" => {
            let mut sd: SyncItDoc = autosurgeon::hydrate(&doc)?;
            let end = sd.desc.as_str().len();
            sd.desc.splice(end, 0, " (edited)");
            autosurgeon::reconcile(&mut doc, &sd)?;
            pump_until_quiet(&mut socket, &mut doc, &mut sync_state).await?;
            let sd: SyncItDoc = autosurgeon::hydrate(&doc)?;
            println!("Pushed edit. Final doc: {sd:?}");
        }
        "watch" => {
            println!("Watching for changes (ctrl-c to quit)...");
            loop {
                let heads_before = doc.get_heads();
                pump_once(&mut socket, &mut doc, &mut sync_state, None).await?;
                if doc.get_heads() != heads_before {
                    let sd: SyncItDoc = autosurgeon::hydrate(&doc)?;
                    println!("Doc updated: {sd:?}");
                }
            }
        }
        other => bail!("unknown mode {other:?}; expected increment, edit, or watch"),
    }

    socket.close(None).await.ok();
    Ok(())
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Alternate send/recv until we have nothing to send and the server goes quiet.
async fn pump_until_quiet(
    socket: &mut WsStream,
    doc: &mut AutoCommit,
    sync_state: &mut automerge::sync::State,
) -> Result<()> {
    loop {
        let quiet = pump_once(socket, doc, sync_state, Some(RECV_TIMEOUT)).await?;
        if quiet {
            return Ok(());
        }
    }
}

/// Send all pending sync messages, then wait for one incoming frame and apply it.
/// With a timeout, returns Ok(true) if nothing was pending and nothing arrived.
/// With no timeout, blocks until a frame arrives.
async fn pump_once(
    socket: &mut WsStream,
    doc: &mut AutoCommit,
    sync_state: &mut automerge::sync::State,
    timeout: Option<Duration>,
) -> Result<bool> {
    let mut sent = false;
    while let Some(msg) = doc.sync().generate_sync_message(sync_state) {
        socket.send(Message::Binary(msg.encode().into())).await?;
        sent = true;
    }

    let incoming = match timeout {
        Some(t) => match tokio::time::timeout(t, socket.next()).await {
            Ok(frame) => frame,
            Err(_) => return Ok(!sent),
        },
        None => socket.next().await,
    };

    match incoming {
        Some(Ok(Message::Binary(bytes))) => {
            let msg = automerge::sync::Message::decode(&bytes)?;
            doc.sync().receive_sync_message(sync_state, msg)?;
            Ok(false)
        }
        Some(Ok(Message::Close(_))) | None => bail!("server closed the connection"),
        Some(Ok(_)) => Ok(false), // ignore text/ping/pong frames
        Some(Err(err)) => Err(err.into()),
    }
}
