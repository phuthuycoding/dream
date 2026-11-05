//! Tests for the WS attach protocol: the binary frame codec and the
//! attach → scrollback → snapshot → live-stream contract.
//!
//! A real axum server is spawned per integration test on a loopback
//! ephemeral port and driven with tokio-tungstenite, mirroring the
//! `src/http/tests.rs` pattern.
//!
//! Assumed surface (the dev pass reconciles — task-007-tests-report.md):
//!   - `crate::ws::frame::Frame` — `Debug + Clone + PartialEq` enum:
//!     client → `Input(Vec<u8>)` 'i', `Resize { cols, rows }` 'r'
//!     server → `Output(Vec<u8>)` 'o', `Snapshot(Vec<u8>)` 's', `End(Vec<u8>)` 'e', `Attention` 'a'
//!     wire layout: byte 0 = ASCII opcode, remainder = payload verbatim
//!   - `Frame::encode(&self) -> Vec<u8>`
//!   - `Frame::decode(&[u8]) -> Result<Frame, _>` — clean `Err` on unknown
//!     opcode or empty input, never a panic
//!   - attach handler behind the committed `GET /api/sessions/{id}/ws`
//!     route: attaching a Live session runs its `cmd` (lazy spawn or
//!     equivalent — nothing else can produce a live PTY in this task's
//!     scope), then sends retained log-tail scrollback ('o' frames) → one
//!     's' snapshot frame → live 'o' stream; attaching a Dead session
//!     replays the retained log, then sends 'e' whose payload names the
//!     ended state and offers respawn
//!   - `futures-util` must be added to [dev-dependencies] — tokio-tungstenite
//!     is already a dev-dep but its Stream/Sink extension traits live there

#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::Message;

use crate::model::Session;
use crate::replay::log::ReplayLog;
use crate::state::registry::Registry;
use crate::vt::snapshot;

use super::frame::Frame;

const TOKEN: &str = "test-token-9f8e7d6c5b4a3210";
const WAIT: Duration = Duration::from_secs(15);

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ------------------------------------------------------------- frame codec

#[test]
fn input_and_resize_frames_round_trip_with_their_opcodes() {
    let input = Frame::Input(b"ls -la\n".to_vec());
    let encoded = input.encode();
    assert_eq!(encoded[0], b'i', "client input frame opcode");
    assert_eq!(
        &encoded[1..],
        b"ls -la\n",
        "input payload is the raw bytes after the opcode"
    );
    assert_eq!(
        Frame::decode(&encoded).expect("input frame must decode"),
        input
    );

    let resize = Frame::Resize {
        cols: 100,
        rows: 30,
    };
    let encoded = resize.encode();
    assert_eq!(encoded[0], b'r', "client resize frame opcode");
    assert_eq!(
        Frame::decode(&encoded).expect("resize frame must decode"),
        resize
    );
    assert_ne!(
        Frame::Resize {
            cols: 30,
            rows: 100
        }
        .encode(),
        encoded,
        "swapped dimensions must produce different wire bytes"
    );
}

#[test]
fn output_snapshot_end_and_attention_frames_round_trip_with_their_opcodes() {
    for (frame, opcode) in [
        (Frame::Output(b"prompt$ ".to_vec()), b'o'),
        (
            Frame::Snapshot(b"\x1bc\x1b[?2026hdump\x1b[?2026l".to_vec()),
            b's',
        ),
        (Frame::End(b"session ended".to_vec()), b'e'),
        (Frame::Attention, b'a'),
    ] {
        let encoded = frame.encode();
        assert_eq!(encoded[0], opcode, "opcode byte for {frame:?}");
        assert_eq!(
            Frame::decode(&encoded).expect("server frame must decode"),
            frame
        );
    }
}

#[test]
fn byte_payloads_pass_through_the_codec_verbatim() {
    // Payloads are arbitrary bytes — NULs, high bytes, split escape
    // sequences — and must survive the wire unchanged.
    let payload = vec![0x00, 0x1b, b'[', 0xff, b'x', 0x00];
    let encoded = Frame::Output(payload.clone()).encode();
    assert_eq!(&encoded[1..], payload.as_slice());
    assert_eq!(
        Frame::decode(&encoded).expect("decode"),
        Frame::Output(payload)
    );
}

#[test]
fn decoding_rejects_unknown_opcodes_and_empty_frames() {
    assert!(
        Frame::decode(b"").is_err(),
        "an empty frame has no opcode and must be rejected"
    );
    for opcode in [b'x', b'z', b'0', 0x00, 0xff] {
        let frame = [opcode, b'd', b'a', b't', b'a'];
        assert!(
            Frame::decode(&frame).is_err(),
            "opcode {opcode:#04x} is not in the protocol and must be rejected"
        );
    }
}

// ------------------------------------------------------------ ws plumbing

/// ASSUMED constructor — one place to fix when the real signature lands
/// (same choke point as `src/http/tests.rs`).
fn build_app(data_dir: &Path) -> axum::Router {
    crate::http::routes::build_router(data_dir.to_path_buf(), TOKEN.to_string())
}

async fn spawn_server(data_dir: &Path) -> String {
    let app = build_app(data_dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback ephemeral port");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .expect("serve");
    });
    base
}

async fn ws_connect(base: &str, session_id: &str) -> WsStream {
    let url = format!(
        "{}/api/sessions/{session_id}/ws?token={TOKEN}",
        base.replacen("http://", "ws://", 1),
    );
    let (socket, resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect must complete the upgrade");
    assert_eq!(
        resp.status().as_u16(),
        101,
        "attach upgrade must switch protocols"
    );
    socket
}

/// Next decoded protocol frame, skipping ping/pong; `None` on close.
async fn next_frame(ws: &mut WsStream) -> Option<Frame> {
    loop {
        match ws.next().await {
            None => return None,
            Some(Err(err)) => panic!("websocket read failed: {err}"),
            Some(Ok(Message::Binary(data))) => {
                return Some(
                    Frame::decode(&data).expect("every server frame must decode through the codec"),
                );
            }
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
            Some(Ok(Message::Close(_))) => return None,
            Some(Ok(msg)) => panic!("attach protocol is binary-only, got {msg:?}"),
        }
    }
}

/// Collect frames until `stop` matches (that frame included). Panics on
/// timeout or an early close.
async fn collect_until(ws: &mut WsStream, stop: impl Fn(&Frame) -> bool) -> Vec<Frame> {
    tokio::time::timeout(WAIT, async {
        let mut frames = Vec::new();
        loop {
            match next_frame(ws).await {
                Some(frame) => {
                    let done = stop(&frame);
                    frames.push(frame);
                    if done {
                        return frames;
                    }
                }
                None => panic!("socket closed before the expected frame arrived"),
            }
        }
    })
    .await
    .expect("timed out waiting for the expected frame")
}

/// Read live output until `needle` shows up in the concatenated 'o'
/// payloads (chunk boundaries are free to split it).
async fn wait_for_output(ws: &mut WsStream, needle: &[u8]) {
    tokio::time::timeout(WAIT, async {
        let mut acc = Vec::new();
        loop {
            match next_frame(ws).await {
                Some(Frame::Output(bytes)) => {
                    acc.extend_from_slice(&bytes);
                    if acc.windows(needle.len()).any(|window| window == needle) {
                        return;
                    }
                }
                Some(Frame::End(reason)) => panic!(
                    "session ended before {needle:?} arrived: {}",
                    String::from_utf8_lossy(&reason)
                ),
                Some(_) => {}
                None => panic!("socket closed before {needle:?} arrived"),
            }
        }
    })
    .await
    .expect("timed out waiting for live output");
}

async fn send_input(ws: &mut WsStream, bytes: &[u8]) {
    ws.send(Message::binary(Frame::Input(bytes.to_vec()).encode()))
        .await
        .expect("send input frame");
}

fn payload_bytes(frame: &Frame) -> &[u8] {
    match frame {
        Frame::Output(bytes) | Frame::Snapshot(bytes) | Frame::End(bytes) => bytes,
        _ => b"",
    }
}

fn concat_payloads(frames: &[Frame]) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in frames {
        out.extend_from_slice(payload_bytes(frame));
    }
    out
}

/// Attach prefix assertion shared by the live-session tests: every frame
/// before the snapshot is replayed scrollback, and the snapshot payload is
/// a complete DEC-2026-wrapped dump. Returns the snapshot bytes.
fn assert_attach_prefix(prefix: &[Frame]) -> Vec<u8> {
    let (last, scrollback) = prefix.split_last().expect("attach prefix is non-empty");
    let Frame::Snapshot(bytes) = last else {
        panic!("collect_until(Snapshot) must end on a snapshot frame")
    };
    assert!(
        scrollback.iter().all(|f| matches!(f, Frame::Output(_))),
        "frames before the snapshot must all be scrollback 'o' frames, got {scrollback:?}"
    );
    assert!(
        snapshot::verify(bytes),
        "snapshot frame payload must verify as a reset + DEC-2026-wrapped dump"
    );
    bytes.clone()
}

// --------------------------------------------------- attach / reattach

#[tokio::test]
async fn attaching_streams_scrollback_then_a_snapshot_then_live_output() {
    let data = TempDir::new().expect("tempdir");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();
    let id = create_session(&client, &base, data.path(), "sh", None).await;

    let mut ws = ws_connect(&base, &id).await;

    let prefix = collect_until(&mut ws, |f| matches!(f, Frame::Snapshot(_))).await;
    assert_attach_prefix(&prefix);

    // `echo $((6*7))` proves the input reached a shell: the pty echo of the
    // typed line never contains the result.
    send_input(&mut ws, b"echo $((6*7))\n").await;
    wait_for_output(&mut ws, b"42").await;
}

#[tokio::test]
async fn reattaching_replays_scrollback_and_restores_the_screen_before_live_output() {
    let data = TempDir::new().expect("tempdir");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();
    let id = create_session(&client, &base, data.path(), "sh", None).await;

    // First attach: produce screen content, then detach.
    let mut ws = ws_connect(&base, &id).await;
    let prefix = collect_until(&mut ws, |f| matches!(f, Frame::Snapshot(_))).await;
    assert_attach_prefix(&prefix);
    send_input(&mut ws, b"echo $((6*7))\n").await;
    wait_for_output(&mut ws, b"42").await;
    drop(ws);
    // Let the pump settle so the marker is committed to the replay log
    // before the reattach reads the retained tail.
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Reattach: retained scrollback first, then a snapshot of the screen as
    // it was left, then the live stream of the same shell.
    let mut ws = ws_connect(&base, &id).await;
    let prefix = collect_until(&mut ws, |f| matches!(f, Frame::Snapshot(_))).await;
    let scrollback = concat_payloads(&prefix[..prefix.len() - 1]);
    assert!(
        scrollback.windows(2).any(|w| w == b"42"),
        "reattach must replay retained scrollback containing the pre-detach output"
    );
    let snapshot_bytes = assert_attach_prefix(&prefix);
    assert!(
        snapshot_bytes.windows(2).any(|w| w == b"42"),
        "the snapshot must restore the screen the client detached from"
    );

    send_input(&mut ws, b"echo $((7*8))\n").await;
    wait_for_output(&mut ws, b"56").await;
}

// --------------------------------------------------------- multi-client

#[tokio::test]
async fn two_attached_clients_receive_identical_output_and_both_can_send_input() {
    let data = TempDir::new().expect("tempdir");
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();
    let id = create_session(&client, &base, data.path(), "sh", None).await;

    let mut a = ws_connect(&base, &id).await;
    let prefix = collect_until(&mut a, |f| matches!(f, Frame::Snapshot(_))).await;
    assert_attach_prefix(&prefix);

    let mut b = ws_connect(&base, &id).await;
    let prefix = collect_until(&mut b, |f| matches!(f, Frame::Snapshot(_))).await;
    assert_attach_prefix(&prefix);

    // Client A types; both clients must see the same live output.
    send_input(&mut a, b"echo $((6*7))\n").await;
    wait_for_output(&mut a, b"42").await;
    wait_for_output(&mut b, b"42").await;

    // Client B types into the same shell; again both must see it.
    send_input(&mut b, b"echo $((8*9))\n").await;
    wait_for_output(&mut a, b"72").await;
    wait_for_output(&mut b, b"72").await;
}

// --------------------------------------------------------- dead session

/// Seed a session whose shell is already gone: registry record marked Dead
/// plus a replay log holding its last output and exit event.
fn seed_dead_session(data_dir: &Path) -> Session {
    let mut registry = Registry::load(data_dir).expect("load seed registry");
    let session = registry
        .create(
            "dead-shell",
            "sh",
            data_dir,
            Some("export RSPN=revived-77".to_string()),
        )
        .expect("seed session");
    {
        let mut log = ReplayLog::create(&session.log_path, 80, 24).expect("seed log");
        log.append_output(b"last words: FAREWELL-7x9q\r\n")
            .expect("append output");
        log.append_exit(137).expect("append exit");
    }
    registry.mark_dead(&session.id).expect("mark dead");
    session
}

#[tokio::test]
async fn attaching_to_a_dead_session_replays_the_log_then_signals_the_end_with_a_respawn_offer() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_dead_session(data.path());
    let base = spawn_server(data.path()).await;

    let mut ws = ws_connect(&base, &session.id).await;
    let frames = collect_until(&mut ws, |f| matches!(f, Frame::End(_))).await;

    let (last, replayed) = frames.split_last().expect("frames are non-empty");
    let replay = concat_payloads(replayed);
    assert!(
        replay.windows(13).any(|w| w == b"FAREWELL-7x9q"),
        "a dead session must still replay its retained log, got {replayed:?}"
    );

    let Frame::End(notice) = last else {
        panic!("collect_until(End) must end on an end frame")
    };
    let notice = String::from_utf8_lossy(notice).to_lowercase();
    assert!(
        notice.contains("dead") || notice.contains("ended") || notice.contains("exit"),
        "the end frame must say the session is over, got {notice:?}"
    );
    assert!(
        notice.contains("respawn"),
        "the end frame must offer respawn, got {notice:?}"
    );
}

#[tokio::test]
async fn respawning_a_dead_session_then_attaching_runs_a_fresh_shell() {
    let data = TempDir::new().expect("tempdir");
    let session = seed_dead_session(data.path());
    let base = spawn_server(data.path()).await;
    let client = reqwest::Client::new();

    // The offered respawn action is the committed REST endpoint.
    let resp = client
        .post(format!("{base}/api/sessions/{}/respawn", session.id))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("respawn request");
    assert!(
        resp.status().is_success(),
        "respawn must succeed, got {}",
        resp.status()
    );

    // Attaching the respawned session delivers a live shell again, and the
    // setup script has re-run (VAL= is only produced by `echo`, never by
    // the echoed prelude itself).
    let mut ws = ws_connect(&base, &session.id).await;
    let prefix = collect_until(&mut ws, |f| matches!(f, Frame::Snapshot(_))).await;
    assert_attach_prefix(&prefix);
    send_input(&mut ws, b"echo VAL=$RSPN\n").await;
    wait_for_output(&mut ws, b"VAL=revived-77").await;
}

// --------------------------------------------------------------- helpers

/// Create a live session through the committed REST route; attaching is
/// what makes its `cmd` run under this task's contract.
async fn create_session(
    client: &reqwest::Client,
    base: &str,
    data_dir: &Path,
    cmd: &str,
    setup_script: Option<&str>,
) -> String {
    let resp = client
        .post(format!("{base}/api/sessions"))
        .bearer_auth(TOKEN)
        .json(&json!({
            "name": "test-session",
            "cmd": cmd,
            "cwd": data_dir,
            "setup_script": setup_script,
        }))
        .send()
        .await
        .expect("create request");
    assert!(
        resp.status().is_success(),
        "create must succeed, got {}",
        resp.status()
    );
    let body: Value = resp.json().await.expect("json body");
    body["id"]
        .as_str()
        .expect("created session carries an id")
        .to_string()
}
