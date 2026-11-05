//! `RuntimeManager`: the session lifecycle layer over the shared
//! `ws::runtime` map.
//!
//! Runtimes live in `ws::runtime`'s process-wide map so a ws attach and a
//! manager spawn always land on the same shell — this layer owns the
//! lifecycle decisions around that map: spawn on create/attach/respawn,
//! dead-mark + eviction when the pty exits, daemon-restart resurrection,
//! bell/idle attention tracking, and a periodic retention enforcer.
//!
//! Per session the manager keeps a `SessionEntry` (runtime + attention
//! flag + last-output instant) and runs a monitor task on the runtime's
//! broadcast. The monitor
//! is what observes a natural exit: on `Ended` it persists `Dead` — but
//! only when the shared map still names *this* runtime for the session,
//! so a monitor retired by respawn/resurrect/kill cannot stamp `Dead`
//! over a newer shell's record.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::model::{Session, Status};
use crate::replay::enforcer::{self, RetentionPolicy};
use crate::state::registry::Registry;
use crate::ws::runtime::{self as ws_runtime, LiveEvent, SessionRuntime};

/// Production idle-attention threshold (FR-014): 30 s without pty output.
/// Tests inject a shorter one via [`RuntimeManager::new`].
pub const DEFAULT_IDLE_ATTENTION: Duration = Duration::from_secs(30);

/// Monitor wakeup bounds: short enough that a sub-second injected
/// threshold still resolves promptly, long enough that an idle session
/// under the 30 s production threshold is not polled hot.
const MIN_TICK: Duration = Duration::from_millis(10);
const MAX_TICK: Duration = Duration::from_secs(5);

/// One managed session: its live runtime and its attention state — the
/// flag the monitor task sets and `spawn_on_attach` clears, plus the
/// last-output instant the idle check measures from, under one lock so
/// the monitor's "should idle-flag?" decision and the flag set are
/// atomic against an attach's clear+reset.
struct SessionEntry {
    rt: Arc<SessionRuntime>,
    attention: Mutex<Attention>,
}

/// Attention flag + the instant of the last output chunk. A single
/// mutex covers both: `spawn_on_attach` clears the flag and restarts
/// the idle clock in one hold, so a monitor that observed
/// `elapsed >= idle_after` before the attach either commits its flag
/// first (the attach then clears it) or sees the post-attach timestamp
/// and does not flag — a pre-attach observation can no longer re-arm a
/// flag the attach just cleared.
struct Attention {
    flagged: bool,
    last_output: Instant,
}

/// Lifecycle manager for one data dir. `entries` is shared with the
/// monitor tasks so an entry can outlive the manager that created it
/// without losing its attention state.
pub struct RuntimeManager {
    data_dir: PathBuf,
    idle_attention_after: Duration,
    entries: Arc<Mutex<HashMap<String, Arc<SessionEntry>>>>,
}

impl RuntimeManager {
    /// `idle_attention_after` is injectable: production passes
    /// [`DEFAULT_IDLE_ATTENTION`], tests a few hundred milliseconds.
    pub fn new(data_dir: PathBuf, idle_attention_after: Duration) -> Self {
        Self {
            data_dir,
            idle_attention_after,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create-route call: a fresh shell for a freshly created session.
    pub async fn spawn(&self, session: &Session) -> anyhow::Result<Arc<SessionRuntime>> {
        let rt = ws_runtime::spawn_fresh(session).await?;
        self.register(&session.id, rt.clone());
        Ok(rt)
    }

    /// Attach-route call: the session's live runtime, spawning only when
    /// none exists. Never spawns for a `Dead` session. Clears the
    /// attention flag and restarts the idle clock — attaching is the
    /// acknowledgement.
    pub async fn spawn_on_attach(&self, session: &Session) -> anyhow::Result<Arc<SessionRuntime>> {
        anyhow::ensure!(
            session.status != Status::Dead,
            "session {} is dead; respawn it first",
            session.id
        );
        let rt = match ws_runtime::attach_runtime(session).await? {
            Some((rt, _scrollback)) => rt,
            // No live runtime in the shared map — a stale shell died or
            // the create-time spawn failed. The fresh spawn registers its
            // monitor below like every other manager spawn.
            None => ws_runtime::spawn_fresh(session).await?,
        };
        let entry = self.register(&session.id, rt.clone());
        let mut attention = lock(&entry.attention);
        attention.flagged = false;
        attention.last_output = Instant::now();
        Ok(rt)
    }

    /// Respawn-route call: a fresh shell (setup script re-runs), then the
    /// persisted `Live` mark — spawning first so a failed spawn leaves the
    /// record `Dead` rather than claiming life for a shell that never ran.
    pub async fn respawn(&self, session: &Session) -> anyhow::Result<Arc<SessionRuntime>> {
        let rt = ws_runtime::spawn_fresh(session).await?;
        self.register(&session.id, rt.clone());
        let mut registry =
            Registry::load(&self.data_dir).context("load registry for respawn")?;
        registry
            .respawn(&session.id)
            .context("persist Live status")?;
        Ok(rt)
    }

    /// Kill-route call: evict the runtime, terminate its shell, persist
    /// `Dead`. Tolerates a session with no runtime — the dead mark is
    /// owed to the registry either way.
    pub async fn kill(&self, session_id: &str) -> anyhow::Result<()> {
        lock(&self.entries).remove(session_id);
        if let Some(rt) = ws_runtime::detach_runtime(session_id).await {
            if let Err(err) = rt.kill() {
                // The supervisor may already be gone (dead pty); the dead
                // mark is still owed.
                warn!(session_id, error = %err, "pty kill failed; marking dead anyway");
            }
        }
        let mut registry = Registry::load(&self.data_dir).context("load registry for kill")?;
        registry
            .mark_dead(session_id)
            .context("persist dead mark")
    }

    /// Daemon-startup pass: every session persisted non-`Dead` gets a
    /// fresh shell in its saved cwd (setup script re-runs, `resurrected`
    /// persisted). `Dead` sessions are skipped. Per-session failures are
    /// reported together after all sessions were attempted; the count
    /// covers only fully resurrected sessions.
    pub async fn resurrect_all(&self) -> anyhow::Result<usize> {
        let registry =
            Registry::load(&self.data_dir).context("load registry for resurrection")?;
        let mut resurrected = 0usize;
        let mut failures = Vec::new();
        for session in registry.list() {
            if session.status == Status::Dead {
                continue;
            }
            match self.resurrect_one(session).await {
                Ok(()) => resurrected += 1,
                Err(err) => failures.push(format!("session {}: {err:#}", session.id)),
            }
        }
        if failures.is_empty() {
            Ok(resurrected)
        } else {
            Err(anyhow!(
                "{} session(s) failed resurrection: {}",
                failures.len(),
                failures.join("; ")
            ))
        }
    }

    async fn resurrect_one(&self, session: &Session) -> anyhow::Result<()> {
        let rt = ws_runtime::spawn_fresh(session).await?;
        self.register(&session.id, rt);
        let mut registry =
            Registry::load(&self.data_dir).context("load registry for resurrected mark")?;
        registry
            .mark_resurrected(&session.id)
            .context("persist resurrected flag")
    }

    /// The session's live runtime, if this manager has one and its shell
    /// is still running.
    pub fn runtime(&self, session_id: &str) -> Option<Arc<SessionRuntime>> {
        let entry = lock(&self.entries).get(session_id)?.clone();
        entry.rt.is_alive().then(|| entry.rt.clone())
    }

    /// True while the session's attention flag is set (bell in output or
    /// `idle_attention_after` of output silence); cleared by
    /// `spawn_on_attach`.
    pub fn attention(&self, session_id: &str) -> bool {
        lock(&self.entries)
            .get(session_id)
            .is_some_and(|entry| lock(&entry.attention).flagged)
    }

    /// Periodic retention pass over `<data_dir>/sessions/` using
    /// `enforcer::enforce`. The caller owns policy and cadence — the
    /// daemon wires `Config::log_cap`/`log_max_age` in.
    pub fn spawn_enforcer(
        &self,
        policy: RetentionPolicy,
        period: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let sessions = self.data_dir.join("sessions");
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let dir = sessions.clone();
                let policy = policy.clone();
                match tokio::task::spawn_blocking(move || enforcer::enforce(&dir, &policy)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => warn!(error = %err, "replay retention pass had failures"),
                    Err(err) => warn!(error = %err, "retention task failed to join"),
                }
            }
        })
    }

    /// Track `rt` under `session_id`, starting its monitor unless this
    /// exact runtime is already tracked (a re-attach to the same shell
    /// must not spawn a second monitor).
    fn register(&self, session_id: &str, rt: Arc<SessionRuntime>) -> Arc<SessionEntry> {
        let mut map = lock(&self.entries);
        if let Some(entry) = map.get(session_id) {
            if Arc::ptr_eq(&entry.rt, &rt) {
                return entry.clone();
            }
        }
        let entry = Arc::new(SessionEntry {
            rt: rt.clone(),
            attention: Mutex::new(Attention {
                flagged: false,
                last_output: Instant::now(),
            }),
        });
        // Subscribe before the spawn returns so output produced between
        // now and the monitor task's first poll is not missed — the bell
        // check reads chunks, and a missed \x07 is a missed flag.
        let rx = rt.subscribe();
        map.insert(session_id.to_string(), entry.clone());
        drop(map);
        self.monitor(session_id.to_string(), entry.clone(), rx);
        entry
    }

    /// Per-session watcher: bell detection, idle timing, dead-mark on the
    /// `Ended` broadcast (or a liveness poll when the broadcast was
    /// missed). Exits when the shell ends or the broadcast closes.
    fn monitor(
        &self,
        session_id: String,
        entry: Arc<SessionEntry>,
        mut rx: broadcast::Receiver<LiveEvent>,
    ) {
        let data_dir = self.data_dir.clone();
        let idle_after = self.idle_attention_after;
        let entries = self.entries.clone();
        tokio::spawn(async move {
            loop {
                // Once flagged, the flag holds until attach clears it —
                // keep waking occasionally only to notice a missed exit.
                let wait = {
                    let attention = lock(&entry.attention);
                    if attention.flagged {
                        MAX_TICK
                    } else {
                        idle_after
                            .saturating_sub(attention.last_output.elapsed())
                            .clamp(MIN_TICK, MAX_TICK)
                    }
                };
                tokio::select! {
                    event = rx.recv() => match event {
                        Ok(LiveEvent::Output(bytes)) => {
                            let mut attention = lock(&entry.attention);
                            attention.last_output = Instant::now();
                            if bytes.contains(&0x07) {
                                flag_attention(&mut attention, &entry.rt);
                            }
                        }
                        Ok(LiveEvent::Ended(_)) => {
                            retire_dead(&entries, &data_dir, &session_id, &entry).await;
                            return;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                    _ = tokio::time::sleep(wait) => {
                        if !entry.rt.is_alive() {
                            retire_dead(&entries, &data_dir, &session_id, &entry).await;
                            return;
                        }
                        let mut attention = lock(&entry.attention);
                        if attention.last_output.elapsed() >= idle_after {
                            flag_attention(&mut attention, &entry.rt);
                        }
                    }
                }
            }
        });
    }
}

/// Lock ignoring poisoning: a panicking touch of the map must not take
/// monitors or route handlers down with it (same rule as ws::runtime).
fn lock<'a, T>(mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Transition the flag false→true and notify attached clients once; an
/// already-set flag does not re-broadcast. The caller already holds the
/// attention lock, so the set is atomic with the check that triggered
/// it — an attach's clear+reset cannot land between the two.
fn flag_attention(attention: &mut Attention, rt: &SessionRuntime) {
    if !attention.flagged {
        attention.flagged = true;
        rt.notify(LiveEvent::Attention);
    }
}

/// Persist `Dead` for a runtime that just ended — but only while the
/// shared map still names this runtime for the session. `spawn_fresh`
/// swaps the map entry before the old shell finishes dying, and `kill`'s
/// `detach_runtime` removes it first; in both cases this monitor is stale
/// and must not stamp `Dead` over a newer or already-handled record.
///
/// The check and the mark run inside one hold of the shared-map lock:
/// a respawn/resurrect swaps the map under that same lock and only then
/// persists `Live`, so a `Dead` taken while the map still names this
/// runtime cannot be overtaken by a newer shell's `Live` — either the
/// swap happened first (the check fails and no mark is written) or the
/// mark lands first and the respawn's `Live` correctly lands last.
async fn retire_dead(
    entries: &Mutex<HashMap<String, Arc<SessionEntry>>>,
    data_dir: &Path,
    session_id: &str,
    entry: &Arc<SessionEntry>,
) {
    let guard = ws_runtime::lock_runtimes().await;
    match guard.get(session_id) {
        Some(current) if Arc::ptr_eq(current, &entry.rt) => {}
        _ => return,
    }
    {
        let mut map = lock(entries);
        if map
            .get(session_id)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            map.remove(session_id);
        }
    }
    let dir = data_dir.to_path_buf();
    let id = session_id.to_string();
    let marked = tokio::task::spawn_blocking(move || mark_dead(&dir, &id)).await;
    drop(guard);
    match marked {
        Ok(Ok(())) => debug!(session_id, "session marked dead after pty exit"),
        Ok(Err(err)) => warn!(session_id, error = %err, "cannot persist dead mark"),
        Err(err) => warn!(session_id, error = %err, "dead-mark task failed to join"),
    }
}

/// Registry dead-mark; a session deleted between exit and mark is gone
/// already, so a missing record is not an error.
fn mark_dead(data_dir: &Path, session_id: &str) -> anyhow::Result<()> {
    let mut registry = Registry::load(data_dir)?;
    match registry.get(session_id) {
        Some(_) => registry.mark_dead(session_id),
        None => Ok(()),
    }
}
