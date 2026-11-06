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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use anyhow::Context;
use portable_pty::{CommandBuilder, ExitStatus};
use tokio::sync::{broadcast, mpsc, watch, Mutex as AsyncMutex};
use tracing::warn;

use crate::model::Session;
use crate::pty::PtyHandle;
use crate::replay::enforcer;
use crate::replay::log::{Event, ReplayLog};
use crate::replay::reader;
use crate::vt::emulator::VtEmu;

/// Terminal geometry assumed until a client sends its first `r` frame.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Broadcast queue per session; a client lagging past this many chunks is
/// skipped ahead rather than stalling the session's other consumers.
const BROADCAST_CAPACITY: usize = 256;

/// How much of the retained replay log an attach replays as scrollback —
/// also the dead-attach replay budget.
pub(crate) const SCROLLBACK_TAIL_BYTES: u64 = 256 * 1024;

/// Replay-log size cap applied on append when `--log-cap` is not given,
/// so a live log stays bounded without waiting for the periodic enforcer.
pub const DEFAULT_LOG_CAP: u64 = 8 * 1024 * 1024;

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
    // Output payloads captured from the on-disk log just before this
    // spawn truncated it — `scrollback` prepends them to the live log's
    // own tail so a respawn/reattach still replays the prior shell's
    // retained output. Fixed at construction; never mutated.
    retained_scrollback: Vec<Vec<u8>>,
    tx: broadcast::Sender<LiveEvent>,
    alive: AtomicBool,
    // False once the runtime has been evicted in favour of a fresh
    // spawn: the still-draining pump must not keep appending into the
    // log path its replacement just truncated — this runtime's
    // `ReplayLog` writes through a different mutex than the fresh log,
    // so drained output would land torn inside the winner's file.
    // Broadcast to still-attached clients is unaffected.
    appends_enabled: AtomicBool,
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
        if self.appends_enabled.load(Ordering::SeqCst) {
            if let Err(err) = lock(&self.log).append_resize(cols, rows) {
                warn!(session_pty = %self.pty_id(), error = %err, "replay log resize append failed");
            }
        }
    }

    /// Newest retained output within `n_bytes`, oldest first — the
    /// pre-spawn tail captured before this runtime's fresh log truncated
    /// the file, then the live log's own tail on top.
    pub fn scrollback(&self, n_bytes: u64) -> io::Result<Vec<Vec<u8>>> {
        let mut payloads = self.retained_scrollback.clone();
        payloads.extend(output_payloads(lock(&self.log).tail(n_bytes)?));
        Ok(tail_within(payloads, n_bytes))
    }

    /// DEC-2026-wrapped attach snapshot of the current screen.
    pub fn snapshot(&self) -> Vec<u8> {
        lock(&self.vt).snapshot()
    }

    /// Reset the replay log to a fresh header under this runtime's log
    /// mutex — the same lock the pump appends under, so a clean can never
    /// tear an in-flight append the way a path-based rewrite would.
    pub fn clean(&self) -> io::Result<()> {
        lock(&self.log).truncate_fresh()
    }

    /// Terminate the child shell; the pump still drains output and
    /// broadcasts the end event. Lifecycle-layer use: kill, and retiring
    /// a runtime replaced by respawn/resurrect.
    pub(crate) fn kill(&self) -> anyhow::Result<()> {
        lock(&self.pty).kill()
    }

    /// Stop this runtime's replay-log writes. Called when the runtime is
    /// evicted in favour of a fresh spawn: its pump keeps draining up to
    /// `EXIT_DRAIN_TIMEOUT` past the kill, and every file write it still
    /// makes races the winner's `ReplayLog::create` under a different
    /// mutex. Live broadcast to attached clients continues; only the
    /// file appends stop.
    fn disable_log_appends(&self) {
        self.appends_enabled.store(false, Ordering::SeqCst);
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
    let rt = {
        let mut guard = runtimes().lock().await;
        match guard.get(&session.id) {
            Some(rt) if rt.is_alive() => rt.clone(),
            // Registry may still say Live while the shell is gone: the
            // stale runtime is dropped; its replacement is the lifecycle
            // layer's call, not this attach's. Gating its appends first
            // keeps an in-flight `append_exit` from landing inside the
            // fresh log that replacement is about to create.
            Some(stale) => {
                stale.disable_log_appends();
                guard.remove(&session.id);
                return Ok(None);
            }
            None => return Ok(None),
        }
    };
    // `tail` is blocking file io — it must run neither on an executor
    // worker nor under the global map lock, where a slow read would stall
    // every other session's spawn, attach and monitor.
    let rt2 = rt.clone();
    let scrollback = tokio::task::spawn_blocking(move || rt2.scrollback(SCROLLBACK_TAIL_BYTES))
        .await
        .context("scrollback task failed to join")?
        .context("read retained log tail")?;
    Ok(Some((rt, scrollback)))
}

/// Spawn a fresh shell for `session` and swap it into the shared map,
/// killing any prior runtime first — a respawn or daemon-restart
/// resurrect must never leave two shells on one session. The only path
/// that inserts into the shared map; every caller in the lifecycle layer
/// registers the returned runtime so it is monitored.
///
/// The global map lock is only held for the cheap map operations: the
/// whole-file scrollback read, the pty fork and the log create run
/// outside it so one slow spawn cannot stall every other session's
/// attach and monitor. The TOCTOU serialization is preserved — the swap
/// still lands under the same lock `retire_dead` checks, and a runtime
/// another spawn slipped into the map during our work wins (ours is
/// retired before it ever ran a monitor).
pub(crate) async fn spawn_fresh(
    session: &Session,
    log_cap: u64,
) -> anyhow::Result<Arc<SessionRuntime>> {
    // Evict the replaced runtime first so its shell is killed before the
    // fresh log truncates the file its pump may still append to.
    let old = runtimes().lock().await.remove(&session.id);
    if let Some(old) = old {
        // The evicted pump keeps draining up to EXIT_DRAIN_TIMEOUT past
        // the kill; gate its file writes before the fresh log is created
        // so drained output and the 'x' exit cannot land inside it
        // through a foreign mutex.
        old.disable_log_appends();
        if let Err(err) = old.kill() {
            warn!(session_pty = %old.pty_id(), error = %err, "kill of replaced runtime failed");
        }
    }
    // `spawn` truncates the replay log for the new shell; capture the
    // retained tail first so the next attach still replays it as
    // scrollback. Blocking file io — off the executor worker, same as
    // the `tail` read in `attach_runtime`.
    let scrollback_session = session.clone();
    let retained =
        tokio::task::spawn_blocking(move || pre_spawn_scrollback(&scrollback_session))
            .await
            .context("pre-spawn scrollback task failed to join")?;
    let (rt, read_rx, exit_rx) = spawn(session, retained, log_cap).await?;
    let mut guard = runtimes().lock().await;
    if let Some(newer) = guard.get(&session.id) {
        // A concurrent spawn for this session won the swap while our
        // shell was being set up; retire ours and hand back theirs so
        // every caller lands on the same live shell. Its pump never
        // started, so nothing of ours can touch the winner's log.
        let newer = newer.clone();
        drop(guard);
        if let Err(err) = rt.kill() {
            warn!(session_pty = %rt.pty_id(), error = %err, "kill of superseded spawn failed");
        }
        return Ok(newer);
    }
    guard.insert(session.id.clone(), rt.clone());
    drop(guard);
    // The pump starts only once the map names this runtime, so
    // `runtime_entry` (clean_log) and every other map consumer see a
    // runtime before its pump can append — never after.
    tokio::spawn(pump(rt.clone(), read_rx, exit_rx));
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
    let rt = runtimes().lock().await.remove(session_id);
    // Detached runtimes lose the right to write the file — a later
    // respawn/clean creates or resets the log and an orphan pump's
    // seek+write would tear it (review finding: kill path never gated).
    if let Some(rt) = &rt {
        rt.disable_log_appends();
    }
    rt
}

/// The session's runtime while the shared map still holds one — alive or
/// still draining after exit — so writers serialize against the pump
/// through its log mutex instead of rewriting the file by path.
pub(crate) async fn runtime_entry(session_id: &str) -> Option<Arc<SessionRuntime>> {
    runtimes().lock().await.get(session_id).cloned()
}

/// The session's replay log path, but only when it resolves inside the
/// session's own dir — `<…>/<session-id>/replay.cast` after
/// canonicalization. A `session.json` pointing the log elsewhere (via
/// `..` or an absolute path) is refused with a warn rather than followed
/// to an arbitrary file; a missing log is `None` silently — nothing to
/// read either way.
pub(crate) fn validated_log_path(session: &Session) -> Option<PathBuf> {
    let canonical = session.log_path.canonicalize().ok()?;
    let is_own_log = canonical
        .file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(enforcer::LOG_FILE_NAME))
        && canonical
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == std::ffi::OsStr::new(&session.id));
    if is_own_log {
        Some(canonical)
    } else {
        warn!(
            session = %session.id,
            log_path = %session.log_path.display(),
            "log_path escapes the session dir; refusing to read it"
        );
        None
    }
}

/// Output payloads of the newest whole events in the session's log within
/// the scrollback budget — read just before `spawn` truncates the file,
/// so an attach following the respawn still replays the prior shell's
/// retained tail. A missing or unreadable log means none.
fn pre_spawn_scrollback(session: &Session) -> Vec<Vec<u8>> {
    let Some(log_path) = validated_log_path(session) else {
        return Vec::new();
    };
    let events = match reader::read_events(&log_path) {
        Ok(events) => events,
        Err(_) => return Vec::new(),
    };
    tail_within(output_payloads(events), SCROLLBACK_TAIL_BYTES)
}

/// Newest whole payloads within `n_bytes` — always at least the newest,
/// the same suffix-window semantics `ReplayLog::tail` applies to events.
pub(crate) fn tail_within(payloads: Vec<Vec<u8>>, n_bytes: u64) -> Vec<Vec<u8>> {
    let mut used = 0u64;
    let mut from = payloads.len();
    for (i, bytes) in payloads.iter().enumerate().rev() {
        if from != payloads.len() && used + bytes.len() as u64 > n_bytes {
            break;
        }
        used += bytes.len() as u64;
        from = i;
    }
    payloads[from..].to_vec()
}

/// Spawn `session.cmd` on a pty and open its fresh replay log, returning
/// the runtime plus the pump's channels — the caller starts the pump
/// task only after the runtime has entered the shared map, so a runtime
/// whose pump can append is always visible to `runtime_entry` and the
/// other map consumers. `retained_scrollback` is the log tail captured
/// before the fresh `ReplayLog::create` truncated it; the runtime
/// replays it as scrollback prefix.
///
/// `session.cmd` is a shell command line, not an argv: it runs through
/// `sh -c` so quoting, pipes and compound commands behave the way the
/// same line typed into a terminal would.
async fn spawn(
    session: &Session,
    retained_scrollback: Vec<Vec<u8>>,
    log_cap: u64,
) -> anyhow::Result<(
    Arc<SessionRuntime>,
    mpsc::Receiver<Vec<u8>>,
    watch::Receiver<Option<ExitStatus>>,
)> {
    anyhow::ensure!(
        !session.cmd.trim().is_empty(),
        "session cmd is empty"
    );
    let mut cmd = CommandBuilder::new("sh");
    cmd.arg("-c");
    cmd.arg(session.cmd.as_str());
    // The builder snapshots the daemon's whole environment; the auth
    // token must not leak into the shell's env or, via `env`/`export`
    // output, into the replay transcript.
    cmd.env_remove("DREAM_AUTH_TOKEN");
    // Pre-rename deployments may still export the old name.
    cmd.env_remove("WEBTERM_AUTH_TOKEN");
    // The header advertises xterm-256color; without these a pty child
    // launched under a minimal environment (launchd, systemd) has no
    // TERM at all and full-screen apps degrade or refuse to run.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    let mut pty = PtyHandle::spawn_async(session.cwd.clone(), session.setup_script.clone(), cmd)
        .await
        .with_context(|| format!("spawn {:?} for session {}", session.cmd, session.id))?;
    let read_rx = pty.read();
    let exit_rx = pty.wait_exit();

    // Blocking file create — off the executor worker, same as the other
    // replay-log reads in this module.
    let log_path = session.log_path.clone();
    let mut log = tokio::task::spawn_blocking(move || {
        ReplayLog::create(&log_path, DEFAULT_COLS, DEFAULT_ROWS)
    })
    .await
    .context("replay log create task failed to join")?
    .with_context(|| format!("create replay log {}", session.log_path.display()))?;
    // On-append trim keeps the live log bounded between enforcer ticks.
    log.set_cap(log_cap);
    let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
    let rt = Arc::new(SessionRuntime {
        session_id: session.id.clone(),
        pty: Mutex::new(pty),
        vt: Mutex::new(VtEmu::new(DEFAULT_COLS, DEFAULT_ROWS)),
        log: Mutex::new(log),
        retained_scrollback,
        tx,
        alive: AtomicBool::new(true),
        appends_enabled: AtomicBool::new(true),
    });

    Ok((rt, read_rx, exit_rx))
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
                // Still finalize: a runtime left `alive` wedges attaches
                // (no end frame ever comes) and keeps its monitor polling
                // a dead shell forever.
                Err(_) => {
                    let status = exit_rx
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| ExitStatus::with_exit_code(1));
                    finalize_exit(&rt, status);
                    return;
                }
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
    // An evicted runtime keeps draining to its attached clients but must
    // not write into the log file its replacement already owns.
    if rt.appends_enabled.load(Ordering::SeqCst) {
        if let Err(err) = lock(&rt.log).append_output(&bytes) {
            warn!(session_pty = %rt.pty_id(), error = %err, "replay log append failed");
        }
    }
    // No receivers yet is expected between detaches.
    let _ = rt.tx.send(LiveEvent::Output(bytes));
}

fn finalize_exit(rt: &Arc<SessionRuntime>, status: ExitStatus) {
    let code = status.exit_code() as i32;
    rt.alive.store(false, Ordering::SeqCst);
    if rt.appends_enabled.load(Ordering::SeqCst) {
        if let Err(err) = lock(&rt.log).append_exit(code) {
            warn!(session_pty = %rt.pty_id(), error = %err, "replay log exit append failed");
        }
    }
    let notice = format!(
        "session ended (exit {code}); respawn: POST /api/sessions/{}/respawn",
        rt.session_id
    );
    let _ = rt.tx.send(LiveEvent::Ended(notice));
}
