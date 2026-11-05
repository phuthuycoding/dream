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
//! shell exited is replaced through the lifecycle layer's spawn path on
//! the next attach to a `Live` session, never by the attach itself.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use anyhow::Context;
use portable_pty::{CommandBuilder, ExitStatus};
use tokio::sync::{broadcast, mpsc, watch, Mutex as AsyncMutex};
use tracing::warn;

use crate::model::Session;
use crate::pty::PtyHandle;
use crate::replay::log::{Event, ReplayLog};
use crate::vt::emulator::VtEmu;

/// Terminal geometry assumed until a client sends its first `r` frame.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Broadcast queue per session; a client lagging past this many chunks is
/// skipped ahead rather than stalling the session's other consumers.
const BROADCAST_CAPACITY: usize = 256;

/// How much of the retained replay log an attach replays as scrollback.
const SCROLLBACK_TAIL_BYTES: u64 = 256 * 1024;

/// Bound on draining pty output after the child exits. A detached
/// descendant can inherit the pty slave fd and keep the reader thread —
/// and `read_rx` — alive past exit, so an unbounded drain would stall
/// `finalize_exit` forever.
const EXIT_DRAIN_TIMEOUT: Duration = Duration::from_millis(400);

/// One live-stream event delivered to every attached client.
#[derive(Debug, Clone)]
pub enum LiveEvent {
    /// A pty output chunk; sent as an `o` frame.
    Output(Vec<u8>),
    /// The shell ended; sent as an `e` frame carrying the notice text.
    Ended(String),
    /// Attention flag set (bell in output or output idle); sent as an
    /// `a` frame. Emitted by the lifecycle layer via [`SessionRuntime::notify`].
    Attention,
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
        // Bind first: an `if let` scrutinee guard temporary lives through
        // the arm body, where `pty_id()` would re-lock the same mutex and
        // deadlock.
        let res = lock(&self.pty).resize(cols, rows);
        if let Err(err) = res {
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

    /// Terminate the child shell; the pump still drains output and
    /// broadcasts the end event. Lifecycle-layer use: kill, and retiring
    /// a runtime replaced by respawn/resurrect.
    pub(crate) fn kill(&self) -> anyhow::Result<()> {
        lock(&self.pty).kill()
    }

    /// Push one live event to every attached client; the lifecycle layer
    /// uses it for the `a` attention frame, which no pump chunk produces.
    pub(crate) fn notify(&self, event: LiveEvent) {
        let _ = self.tx.send(event);
    }

    /// Subscribe and snapshot under one `vt` guard. The pump feeds the
    /// emulator and broadcasts inside the same critical section, so a
    /// chunk lands either in this snapshot or on the returned receiver —
    /// never both (client prints it twice) and never neither (a hole).
    pub fn attach_view(&self) -> (broadcast::Receiver<LiveEvent>, Vec<u8>) {
        let vt = lock(&self.vt);
        let rx = self.tx.subscribe();
        let snapshot = vt.snapshot();
        drop(vt);
        (rx, snapshot)
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

/// Resolve the live runtime to attach `session` to, returning it with the
/// newest retained tail of its replay log as scrollback — or `None` when
/// the session has none. This never spawns: a runtime may only enter the
/// shared map through [`spawn_fresh`], which the lifecycle layer always
/// follows with `register`, so every live shell carries a monitor. An
/// attach that finds none fails and retries through the route's
/// manager-backed spawn.
pub async fn attach_runtime(
    session: &Session,
) -> anyhow::Result<Option<(Arc<SessionRuntime>, Vec<Vec<u8>>)>> {
    let mut guard = runtimes().lock().await;
    if let Some(rt) = guard.get(&session.id) {
        if rt.is_alive() {
            // `tail` is blocking file io; holding the global map lock
            // across it must not stall an executor worker.
            let rt2 = rt.clone();
            let scrollback = tokio::task::spawn_blocking(move || {
                rt2.scrollback(SCROLLBACK_TAIL_BYTES)
            })
            .await
            .context("scrollback task failed to join")?
            .context("read retained log tail")?;
            return Ok(Some((rt.clone(), scrollback)));
        }
        // Registry may still say Live while the shell is gone: the stale
        // runtime is dropped; its replacement is the lifecycle layer's
        // call, not this attach's.
        guard.remove(&session.id);
    }
    Ok(None)
}

/// Spawn a fresh shell for `session` and swap it into the shared map,
/// killing any prior runtime first — a respawn or daemon-restart
/// resurrect must never leave two shells on one session. The only path
/// that inserts into the shared map; every caller in the lifecycle layer
/// registers the returned runtime so it is monitored.
pub(crate) async fn spawn_fresh(session: &Session) -> anyhow::Result<Arc<SessionRuntime>> {
    let mut guard = runtimes().lock().await;
    if let Some(old) = guard.remove(&session.id) {
        if let Err(err) = old.kill() {
            warn!(session_pty = %old.pty_id(), error = %err, "kill of replaced runtime failed");
        }
    }
    let rt = spawn(session).await?;
    guard.insert(session.id.clone(), rt.clone());
    Ok(rt)
}

/// The shared runtime-map lock. The lifecycle layer holds it across a
/// "still this session's shell" check and the registry write that follows,
/// so a `spawn_fresh` swap cannot land between the check and the write —
/// a respawn's persisted `Live` would then be overwritten by a stale
/// `Dead` from the retired shell's monitor.
pub(crate) async fn lock_runtimes(
) -> tokio::sync::MutexGuard<'static, HashMap<String, Arc<SessionRuntime>>> {
    runtimes().lock().await
}

/// Evict `session_id`'s runtime from the shared map so a later attach can
/// never land on it; retiring the returned runtime is the caller's job.
pub(crate) async fn detach_runtime(session_id: &str) -> Option<Arc<SessionRuntime>> {
    runtimes().lock().await.remove(session_id)
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

    // Blocking file create; callers hold the global runtime-map lock, so
    // it must not run on an executor worker.
    let log_path = session.log_path.clone();
    let log = tokio::task::spawn_blocking(move || {
        ReplayLog::create(&log_path, DEFAULT_COLS, DEFAULT_ROWS)
    })
    .await
    .context("replay log create task failed to join")?
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
                Some(bytes) => pump_chunk(&rt, bytes),
                // Master hit end-of-stream; the exit watch fires next.
                None => reading = false,
            },
            changed = exit_rx.changed() => match changed {
                Ok(()) => {
                    let status = exit_rx.borrow().clone();
                    if let Some(status) = status {
                        // The exit watch can win the select while read_rx
                        // still buffers output; drain so the final bytes
                        // reach clients and log. The drain is bounded —
                        // a descendant still holding the slave open keeps
                        // read_rx alive past exit, and finalize must not
                        // wait on it.
                        let _ = tokio::time::timeout(EXIT_DRAIN_TIMEOUT, async {
                            while let Some(bytes) = read_rx.recv().await {
                                pump_chunk(&rt, bytes);
                            }
                        })
                        .await;
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

/// One pty output chunk: feed the emulator, commit to the replay log and
/// broadcast inside a single `vt` critical section — an attach taking its
/// snapshot under the same lock sees each chunk either in the screen or
/// on its live receiver, never both (duplicate print) and never neither
/// (a hole).
fn pump_chunk(rt: &Arc<SessionRuntime>, bytes: Vec<u8>) {
    let mut vt = lock(&rt.vt);
    vt.feed(&bytes);
    if let Err(err) = lock(&rt.log).append_output(&bytes) {
        warn!(session_pty = %rt.pty_id(), error = %err, "replay log append failed");
    }
    // No receivers yet is expected between detaches.
    let _ = rt.tx.send(LiveEvent::Output(bytes));
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
