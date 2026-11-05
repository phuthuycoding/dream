//! Per-session live runtime: `PtyHandle` + `VtEmu` + `ReplayLog` + fan-out.
//!
//! One `SessionRuntime` per live session, kept in a process-global map
//! keyed by session id so every attach to the same session lands on the
//! same shell. A pump task per runtime consumes pty output, feeds the
//! emulator and the replay log, and broadcasts each chunk to every
//! attached client. On child exit the runtime goes `!alive` and notifies
//! attached clients with an end notice.
//!
//! Lifecycle note: this map is the attach layer's view of "which sessions
//! have a running shell". Marking the registry Dead, evicting runtimes on
//! session delete, and API-driven kill are TASK-009's job; a runtime whose
//! shell exited is simply replaced on the next attach to a `Live` session.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use anyhow::Context;
use portable_pty::{CommandBuilder, ExitStatus};
use tokio::sync::{broadcast, mpsc, watch, Mutex as AsyncMutex};
use tracing::warn;

use crate::model::Session;
use crate::pty::PtyHandle;
use crate::replay::log::{Event, ReplayLog};
use crate::replay::reader;
use crate::vt::emulator::VtEmu;

/// Terminal geometry assumed until a client sends its first `r` frame.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Broadcast queue per session; a client lagging past this many chunks is
/// skipped ahead rather than stalling the session's other consumers.
const BROADCAST_CAPACITY: usize = 256;

/// How much of the retained replay log an attach replays as scrollback.
const SCROLLBACK_TAIL_BYTES: u64 = 256 * 1024;

/// One live-stream event delivered to every attached client.
#[derive(Debug, Clone)]
pub enum LiveEvent {
    /// A pty output chunk; sent as an `o` frame.
    Output(Vec<u8>),
    /// The shell ended; sent as an `e` frame carrying the notice text.
    Ended(String),
}

/// The running state of one spawned session shell.
pub struct SessionRuntime {
    session_id: String,
    // `PtyHandle` is Send but not Sync (`Box<dyn MasterPty>`); the mutex
    // makes the runtime shareable across attaches and the pump.
    pty: Mutex<PtyHandle>,
    vt: Mutex<VtEmu>,
    log: Mutex<ReplayLog>,
    tx: broadcast::Sender<LiveEvent>,
    alive: AtomicBool,
}

impl SessionRuntime {
    /// Receive live events broadcast from this point onward.
    pub fn subscribe(&self) -> broadcast::Receiver<LiveEvent> {
        self.tx.subscribe()
    }

    /// False once the child has exited (or the pump has shut down).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Queue input for the child's stdin.
    pub fn write_input(&self, bytes: &[u8]) -> anyhow::Result<()> {
        lock(&self.pty).write(bytes)
    }

    /// Pty id, for correlating log lines.
    pub fn pty_id(&self) -> uuid::Uuid {
        lock(&self.pty).id
    }

    /// Resize the pty, the shadow emulator and the replay log in step —
    /// a later snapshot must reflect the same geometry the pty now has.
    pub fn resize(&self, cols: u16, rows: u16) {
        if let Err(err) = lock(&self.pty).resize(cols, rows) {
            warn!(session_pty = %self.pty_id(), error = %err, "pty resize failed");
        }
        lock(&self.vt).resize(cols, rows);
        if let Err(err) = lock(&self.log).append_resize(cols, rows) {
            warn!(session_pty = %self.pty_id(), error = %err, "replay log resize append failed");
        }
    }

    /// Newest retained output events within `n_bytes`, oldest first.
    pub fn scrollback(&self, n_bytes: u64) -> io::Result<Vec<Vec<u8>>> {
        let events = lock(&self.log).tail(n_bytes)?;
        Ok(output_payloads(events))
    }

    /// DEC-2026-wrapped attach snapshot of the current screen.
    pub fn snapshot(&self) -> Vec<u8> {
        lock(&self.vt).snapshot()
    }
}

/// Lock ignoring poisoning: a panicking writer must not take the pump or
/// other attaches down with it.
fn lock<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn output_payloads(events: Vec<Event>) -> Vec<Vec<u8>> {
    events
        .into_iter()
        .filter_map(|event| match event {
            Event::Output(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

fn runtimes() -> &'static AsyncMutex<HashMap<String, Arc<SessionRuntime>>> {
    static RUNTIMES: OnceLock<AsyncMutex<HashMap<String, Arc<SessionRuntime>>>> = OnceLock::new();
    RUNTIMES.get_or_init(|| AsyncMutex::new(HashMap::new()))
}

/// Resolve the runtime to attach `session` to, spawning its shell when no
/// live runtime exists. Returns the runtime plus the scrollback payloads
/// the attach must replay before the snapshot:
///
/// - live runtime → the newest retained tail of its replay log;
/// - fresh spawn → the tail of the log as it existed just before spawn
///   (spawn truncates the log for the new shell, so it is read first).
pub async fn attach_runtime(
    session: &Session,
) -> anyhow::Result<(Arc<SessionRuntime>, Vec<Vec<u8>>)> {
    let mut guard = runtimes().lock().await;
    if let Some(rt) = guard.get(&session.id) {
        if rt.is_alive() {
            let scrollback = rt
                .scrollback(SCROLLBACK_TAIL_BYTES)
                .context("read retained log tail")?;
            return Ok((rt.clone(), scrollback));
        }
        // Registry says Live but the previous shell is gone: the runtime
        // is stale, so the attach spawns a replacement shell.
        guard.remove(&session.id);
    }
    let scrollback = pre_spawn_scrollback(session);
    let rt = spawn(session).await?;
    guard.insert(session.id.clone(), rt.clone());
    Ok((rt, scrollback))
}

/// Tail of the on-disk log before `spawn` truncates it — the scrollback a
/// first attach sees for a session whose earlier output is still retained
/// (e.g. a respawned one). A missing or unreadable log means none.
fn pre_spawn_scrollback(session: &Session) -> Vec<Vec<u8>> {
    let events = match reader::read_events(&session.log_path) {
        Ok(events) => events,
        Err(_) => return Vec::new(),
    };
    let outputs = output_payloads(events);
    // Keep the newest whole payloads within the scrollback budget.
    let mut used = 0usize;
    let mut from = outputs.len();
    for (i, bytes) in outputs.iter().enumerate().rev() {
        if from != outputs.len() && used + bytes.len() > SCROLLBACK_TAIL_BYTES as usize {
            break;
        }
        used += bytes.len();
        from = i;
    }
    outputs[from..].to_vec()
}

/// Spawn `session.cmd` on a pty, open its fresh replay log, and start the
/// pump task that drives emulator/log/broadcast.
async fn spawn(session: &Session) -> anyhow::Result<Arc<SessionRuntime>> {
    let mut words = session.cmd.split_whitespace();
    let program = words.next().context("session cmd is empty")?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(words);

    let mut pty = PtyHandle::spawn_async(session.cwd.clone(), session.setup_script.clone(), cmd)
        .await
        .with_context(|| format!("spawn {:?} for session {}", session.cmd, session.id))?;
    let read_rx = pty.read();
    let exit_rx = pty.wait_exit();

    let log = ReplayLog::create(&session.log_path, DEFAULT_COLS, DEFAULT_ROWS)
        .with_context(|| format!("create replay log {}", session.log_path.display()))?;
    let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
    let rt = Arc::new(SessionRuntime {
        session_id: session.id.clone(),
        pty: Mutex::new(pty),
        vt: Mutex::new(VtEmu::new(DEFAULT_COLS, DEFAULT_ROWS)),
        log: Mutex::new(log),
        tx,
        alive: AtomicBool::new(true),
    });

    tokio::spawn(pump(rt.clone(), read_rx, exit_rx));
    Ok(rt)
}

/// Drain pty output into emulator + replay log + broadcast until the child
/// exits, then mark the runtime dead and notify every attached client.
async fn pump(
    rt: Arc<SessionRuntime>,
    mut read_rx: mpsc::Receiver<Vec<u8>>,
    mut exit_rx: watch::Receiver<Option<ExitStatus>>,
) {
    let mut reading = true;
    loop {
        tokio::select! {
            chunk = read_rx.recv(), if reading => match chunk {
                Some(bytes) => {
                    // Order matters: feed the emulator and commit to the
                    // log before broadcasting, so an attach snapshotting
                    // mid-chunk either sees it in the screen or in the
                    // retained tail.
                    lock(&rt.vt).feed(&bytes);
                    if let Err(err) = lock(&rt.log).append_output(&bytes) {
                        warn!(session_pty = %rt.pty_id(), error = %err, "replay log append failed");
                    }
                    // No receivers yet is expected between detaches.
                    let _ = rt.tx.send(LiveEvent::Output(bytes));
                }
                // Master hit end-of-stream; the exit watch fires next.
                None => reading = false,
            },
            changed = exit_rx.changed() => match changed {
                Ok(()) => {
                    let status = exit_rx.borrow().clone();
                    if let Some(status) = status {
                        finalize_exit(&rt, status);
                        return;
                    }
                }
                // Sender dropped with the handle — nothing left to pump.
                Err(_) => return,
            },
        }
    }
}

fn finalize_exit(rt: &Arc<SessionRuntime>, status: ExitStatus) {
    let code = status.exit_code() as i32;
    rt.alive.store(false, Ordering::SeqCst);
    if let Err(err) = lock(&rt.log).append_exit(code) {
        warn!(session_pty = %rt.pty_id(), error = %err, "replay log exit append failed");
    }
    let notice = format!(
        "session ended (exit {code}); respawn: POST /api/sessions/{}/respawn",
        rt.session_id
    );
    let _ = rt.tx.send(LiveEvent::Ended(notice));
}
