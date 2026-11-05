//! WS attach handler: upgraded socket → retained scrollback → DEC-2026
//! snapshot → live broadcast, with `i`→PTY stdin and `r`→PTY+VT resize.
//!
//! `http::routes::ws_attach` delegates here after the registry lookup:
//! `ws.on_upgrade(|socket| attach(socket, session))`.
//!
//! Attach contract (order is part of the protocol):
//! 1. retained log-tail scrollback as `o` frames,
//! 2. one `s` snapshot frame,
//! 3. live `o` stream until detach or an `e` end frame.
//!
//! A `Dead` session never spawns: its retained log replays, then one `e`
//! frame names the ended state and offers the respawn route.

use axum::extract::ws::{Message, WebSocket};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::model::{Session, Status};
use crate::replay::log::Event;
use crate::replay::reader;

use super::frame::Frame;
use super::runtime::{self, LiveEvent, SessionRuntime};

/// `on_upgrade` entry point. Runs the attach to its end — detach, end
/// frame, or a socket/spawn failure (logged, never fatal to the server).
pub async fn attach(socket: WebSocket, session: Session) {
    if let Err(err) = run(socket, &session).await {
        warn!(session = %session.id, error = %err, "ws attach failed");
    }
}

async fn run(mut socket: WebSocket, session: &Session) -> anyhow::Result<()> {
    if session.status == Status::Dead {
        dead_attach(&mut socket, session).await
    } else {
        live_attach(&mut socket, session).await
    }
}

/// Live path: scrollback → snapshot → live stream, input and resize back
/// to the shell.
async fn live_attach(socket: &mut WebSocket, session: &Session) -> anyhow::Result<()> {
    let (rt, scrollback) = runtime::attach_runtime(session).await?;
    for bytes in scrollback {
        send(socket, Frame::Output(bytes)).await?;
    }
    // Subscribe before the snapshot so output landing after the snapshot
    // build is delivered as a live frame right after it.
    let mut rx = rt.subscribe();
    send(socket, Frame::Snapshot(rt.snapshot())).await?;

    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(data))) => handle_client_frame(&rt, &data),
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                // Text frames are not in the protocol; ping/pong is
                // answered by the underlying tungstenite layer.
                Some(Ok(_)) => {}
                Some(Err(err)) => {
                    debug!(session = %session.id, error = %err, "ws recv error; closing attach");
                    return Ok(());
                }
            },
            event = rx.recv() => match event {
                Ok(LiveEvent::Output(bytes)) => send(socket, Frame::Output(bytes)).await?,
                Ok(LiveEvent::Ended(notice)) => {
                    // The end frame is best-effort — the peer may already
                    // be gone; the attach ends either way.
                    let _ = send(socket, Frame::End(notice.into_bytes())).await;
                    return Ok(());
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        session = %session.id,
                        skipped, "attach lagged behind live broadcast; dropping chunks"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
        }
    }
}

/// Dead path: replay the retained log, then end with a notice that names
/// the ended state and offers the respawn route.
async fn dead_attach(socket: &mut WebSocket, session: &Session) -> anyhow::Result<()> {
    let mut exit_code: Option<i32> = None;
    match reader::read_events(&session.log_path) {
        Ok(events) => {
            for event in events {
                match event {
                    Event::Output(bytes) => send(socket, Frame::Output(bytes)).await?,
                    Event::Exit(code) => exit_code = Some(code),
                    Event::Resize { .. } | Event::Marker(_) => {}
                }
            }
        }
        // A dead session without a readable log still gets its end frame.
        Err(err) => {
            warn!(
                session = %session.id,
                error = %err, "cannot read replay log for dead-session attach"
            );
        }
    }
    let notice = match exit_code {
        Some(code) => format!(
            "session '{}' ended (exit {code}); respawn: POST /api/sessions/{}/respawn",
            session.name, session.id
        ),
        None => format!(
            "session '{}' is dead; respawn: POST /api/sessions/{}/respawn",
            session.name, session.id
        ),
    };
    send(socket, Frame::End(notice.into_bytes())).await
}

/// One inbound client frame: input to the pty, resize to pty + emulator +
/// log. Anything else — a server opcode, an undecodable frame — is
/// dropped: one bad frame must not kill the attach.
fn handle_client_frame(rt: &SessionRuntime, data: &[u8]) {
    match Frame::decode(data) {
        Ok(Frame::Input(bytes)) => {
            if let Err(err) = rt.write_input(&bytes) {
                warn!(session_pty = %rt.pty_id(), error = %err, "pty input write failed");
            }
        }
        Ok(Frame::Resize { cols, rows }) => rt.resize(cols, rows),
        Ok(other) => debug!(frame = ?other, "client sent a server-side frame; dropped"),
        Err(err) => debug!(error = %err, "undecodable client frame; dropped"),
    }
}

async fn send(socket: &mut WebSocket, frame: Frame) -> anyhow::Result<()> {
    socket
        .send(Message::Binary(frame.encode().into()))
        .await
        .map_err(|err| anyhow::anyhow!("ws send failed: {err}"))
}
