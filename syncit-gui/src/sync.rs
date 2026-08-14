//! Background sync actor: owns the automerge doc and the websocket
//! connection to the sync server, bridging to the GUI over tokio channels
//! (tokio's sync primitives are executor-agnostic, so the GUI side can await
//! them from gpui's executor with no extra glue).

use std::time::Duration;

use anyhow::{Result, bail};
use automerge::{AutoCommit, sync::SyncDoc};
use futures_util::{SinkExt, StreamExt};
use syncit_core::SyncItDoc;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const SERVER_URL: &str = "ws://127.0.0.1:3003/api/ws";
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// A local edit made in the GUI, to be applied to the shared doc.
#[derive(Debug)]
pub enum LocalChange {
    SetName(String),
    SetActive(bool),
    IncrementCount(i64),
    SetDesc(String),
}

/// Events the actor pushes back to the GUI.
#[derive(Debug)]
pub enum SyncEvent {
    Status(SyncStatus),
    Doc(SyncItDoc),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Connecting,
    Connected,
    Offline,
}

/// Spawn the sync actor on the tokio runtime. Returns the channel to send
/// local changes into and the channel remote updates arrive on.
pub fn start(
    handle: &tokio::runtime::Handle,
) -> (
    mpsc::UnboundedSender<LocalChange>,
    mpsc::UnboundedReceiver<SyncEvent>,
) {
    let (change_tx, change_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    handle.spawn(run(change_rx, event_tx));
    (change_tx, event_rx)
}

async fn run(
    mut changes: mpsc::UnboundedReceiver<LocalChange>,
    events: mpsc::UnboundedSender<SyncEvent>,
) {
    // The doc outlives individual connections so edits made while offline
    // survive and get pushed on reconnect.
    let mut doc = AutoCommit::new();
    loop {
        let _ = events.send(SyncEvent::Status(SyncStatus::Connecting));
        match tokio_tungstenite::connect_async(SERVER_URL).await {
            Ok((socket, _)) => {
                let _ = events.send(SyncEvent::Status(SyncStatus::Connected));
                match run_connected(socket, &mut doc, &mut changes, &events).await {
                    Ok(()) => return, // GUI is gone
                    Err(err) => eprintln!("sync: connection lost: {err:#}"),
                }
            }
            Err(err) => eprintln!("sync: connect failed: {err:#}"),
        }
        let _ = events.send(SyncEvent::Status(SyncStatus::Offline));

        // While offline, keep applying local edits and wait before retrying.
        let retry = tokio::time::sleep(RETRY_DELAY);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                _ = &mut retry => break,
                change = changes.recv() => match change {
                    Some(change) => apply_change(&mut doc, change),
                    None => return, // GUI is gone
                },
            }
        }
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Drive one connection: push local changes as they arrive, apply incoming
/// sync messages, and notify the GUI whenever the doc actually changes.
/// Returns Ok(()) only when the GUI side has hung up.
async fn run_connected(
    mut socket: WsStream,
    doc: &mut AutoCommit,
    changes: &mut mpsc::UnboundedReceiver<LocalChange>,
    events: &mpsc::UnboundedSender<SyncEvent>,
) -> Result<()> {
    let mut sync_state = automerge::sync::State::new();
    send_pending(&mut socket, doc, &mut sync_state).await?;

    loop {
        tokio::select! {
            change = changes.recv() => match change {
                Some(change) => {
                    apply_change(doc, change);
                    send_pending(&mut socket, doc, &mut sync_state).await?;
                }
                None => {
                    socket.close(None).await.ok();
                    return Ok(());
                }
            },
            frame = socket.next() => match frame {
                Some(Ok(Message::Binary(bytes))) => {
                    let heads_before = doc.get_heads();
                    let msg = automerge::sync::Message::decode(&bytes)?;
                    doc.sync().receive_sync_message(&mut sync_state, msg)?;
                    send_pending(&mut socket, doc, &mut sync_state).await?;
                    if doc.get_heads() != heads_before {
                        let sd: SyncItDoc = autosurgeon::hydrate(doc)?;
                        let _ = events.send(SyncEvent::Doc(sd));
                    }
                }
                Some(Ok(Message::Close(_))) | None => bail!("server closed the connection"),
                Some(Ok(_)) => {} // ignore text/ping/pong frames
                Some(Err(err)) => return Err(err.into()),
            },
        }
    }
}

/// Send every sync message the doc currently has queued up.
async fn send_pending(
    socket: &mut WsStream,
    doc: &mut AutoCommit,
    sync_state: &mut automerge::sync::State,
) -> Result<()> {
    while let Some(msg) = doc.sync().generate_sync_message(sync_state) {
        socket.send(Message::Binary(msg.encode().into())).await?;
    }
    Ok(())
}

/// Apply a local edit to the doc. Before the first successful sync the doc is
/// empty and can't hydrate; seed it with the default doc so edits made at
/// startup while offline still land (automerge merges with the server's copy
/// once we connect).
fn apply_change(doc: &mut AutoCommit, change: LocalChange) {
    let mut sd: SyncItDoc = autosurgeon::hydrate(doc).unwrap_or_else(|_| SyncItDoc::new());
    match change {
        LocalChange::SetName(name) => sd.name = name,
        LocalChange::SetActive(active) => sd.active = active,
        LocalChange::IncrementCount(delta) => sd.count.increment(delta),
        LocalChange::SetDesc(desc) => sd.desc.update(&desc),
    }
    if let Err(err) = autosurgeon::reconcile(doc, &sd) {
        eprintln!("sync: failed to apply local change: {err:#}");
    }
}
