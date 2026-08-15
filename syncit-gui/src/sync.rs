//! Network shuttle: owns only the websocket connection to the sync server and
//! ferries encoded automerge sync messages between the GUI (which owns the
//! doc) and the server. Bridged over tokio channels, which are
//! executor-agnostic so the GUI side can await them from gpui's executor.

use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const SERVER_URL: &str = "ws://127.0.0.1:3003/api/ws";
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Commands the GUI sends to the shuttle.
#[derive(Debug)]
pub enum NetCmd {
    /// An encoded automerge sync message to forward to the server.
    Send(Vec<u8>),
    /// Enable or disable syncing. Disabling drops the connection, so it
    /// exercises the same path as a real network failure.
    SetEnabled(bool),
}

/// Events the shuttle pushes to the GUI.
#[derive(Debug)]
pub enum NetEvent {
    Status(SyncStatus),
    /// An encoded automerge sync message arrived from the server.
    Recv(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Connecting,
    Connected,
    Offline,
    Paused,
}

/// Spawn the shuttle on the tokio runtime. Returns the channel to send
/// commands into and the channel network events arrive on.
pub fn start(
    handle: &tokio::runtime::Handle,
) -> (mpsc::UnboundedSender<NetCmd>, mpsc::UnboundedReceiver<NetEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    handle.spawn(run(cmd_rx, event_tx));
    (cmd_tx, event_rx)
}

/// How a connection ended, when it wasn't a network error.
enum ConnEnd {
    GuiGone,
    Disabled,
}

async fn run(mut cmds: mpsc::UnboundedReceiver<NetCmd>, events: mpsc::UnboundedSender<NetEvent>) {
    let mut enabled = true;
    loop {
        if !enabled {
            let _ = events.send(NetEvent::Status(SyncStatus::Paused));
            loop {
                match cmds.recv().await {
                    None => return, // GUI is gone
                    Some(NetCmd::SetEnabled(true)) => {
                        enabled = true;
                        break;
                    }
                    Some(_) => {} // drop sends & redundant disables while paused
                }
            }
        }

        let _ = events.send(NetEvent::Status(SyncStatus::Connecting));
        let socket = match tokio_tungstenite::connect_async(SERVER_URL).await {
            Ok((socket, _)) => socket,
            Err(err) => {
                eprintln!("sync: connect failed: {err:#}");
                let _ = events.send(NetEvent::Status(SyncStatus::Offline));
                match wait_retry(&mut cmds).await {
                    Some(e) => enabled = e,
                    None => return,
                }
                continue;
            }
        };

        // Discard sends generated against a previous connection's sync state;
        // the GUI starts fresh once it sees Connected. Commands still apply.
        while let Ok(cmd) = cmds.try_recv() {
            if let NetCmd::SetEnabled(e) = cmd {
                enabled = e;
            }
        }
        if !enabled {
            continue;
        }

        let _ = events.send(NetEvent::Status(SyncStatus::Connected));
        match run_connected(socket, &mut cmds, &events).await {
            Ok(ConnEnd::GuiGone) => return,
            Ok(ConnEnd::Disabled) => enabled = false,
            Err(err) => {
                eprintln!("sync: connection lost: {err:#}");
                let _ = events.send(NetEvent::Status(SyncStatus::Offline));
                match wait_retry(&mut cmds).await {
                    Some(e) => enabled = e,
                    None => return,
                }
            }
        }
    }
}

/// Wait out the retry delay, still processing commands. Returns the desired
/// enabled state (early on disable), or None if the GUI hung up.
async fn wait_retry(cmds: &mut mpsc::UnboundedReceiver<NetCmd>) -> Option<bool> {
    let retry = tokio::time::sleep(RETRY_DELAY);
    tokio::pin!(retry);
    loop {
        tokio::select! {
            _ = &mut retry => return Some(true),
            cmd = cmds.recv() => match cmd {
                None => return None,
                Some(NetCmd::SetEnabled(false)) => return Some(false),
                Some(_) => {} // stale sends & redundant enables, drop
            },
        }
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Forward bytes in both directions until the connection drops, sync is
/// disabled, or the GUI hangs up.
async fn run_connected(
    mut socket: WsStream,
    cmds: &mut mpsc::UnboundedReceiver<NetCmd>,
    events: &mpsc::UnboundedSender<NetEvent>,
) -> Result<ConnEnd> {
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(NetCmd::Send(bytes)) => socket.send(Message::Binary(bytes.into())).await?,
                Some(NetCmd::SetEnabled(false)) => {
                    socket.close(None).await.ok();
                    return Ok(ConnEnd::Disabled);
                }
                Some(NetCmd::SetEnabled(true)) => {}
                None => {
                    socket.close(None).await.ok();
                    return Ok(ConnEnd::GuiGone);
                }
            },
            frame = socket.next() => match frame {
                Some(Ok(Message::Binary(bytes))) => {
                    let _ = events.send(NetEvent::Recv(bytes.into()));
                }
                Some(Ok(Message::Close(_))) | None => bail!("server closed the connection"),
                Some(Ok(_)) => {} // ignore text/ping/pong frames
                Some(Err(err)) => return Err(err.into()),
            },
        }
    }
}
