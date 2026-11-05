//! Tests for the session lifecycle layer (`RuntimeManager`): spawn on
//! create/attach/respawn, dead-mark + end broadcast on pty exit,
//! daemon-restart resurrection, and bell/idle attention tracking.
//!
//! Everything is driven through the manager API with real ptys against a
//! tempdir data dir — no REST server, since the route wiring (create →
//! spawn, respawn → re-spawn, kill → kill+dead-mark) is thin delegation
//! onto the same calls.
//!
//! Assumed surface (the dev pass reconciles — task-009-tests-report.md):
//!   - `crate::runtime::RuntimeManager` — per-instance manager over one
//!     data dir; runtimes it returns are `Arc<crate::ws::runtime::
//!     SessionRuntime>` (the committed TASK-007 type, wrapped or reused)
//!   - `RuntimeManager::new(data_dir: PathBuf, idle_attention_after:
//!     Duration) -> RuntimeManager` — the idle threshold is injectable so
//!     tests do not wait 30 s of wall clock
//!   - `spawn(&self, &Session) -> Result<Arc<SessionRuntime>>` — the
//!     create route's call: fresh pty + log + pump, `setup_script` first
//!   - `spawn_on_attach(&self, &Session) -> Result<Arc<SessionRuntime>>` —
//!     returns the live runtime, spawning only when none exists; a second
//!     call on the same live session reuses the same pty; `Err` on Dead;
//!     clears the attention flag
//!   - `respawn(&self, &Session) -> Result<Arc<SessionRuntime>>` — dead →
//!     live again with a fresh pty whose `setup_script` re-runs
//!   - `kill(&self, session_id: &str) -> Result<()>` — terminates the pty
//!     and persists `Status::Dead`
//!   - `resurrect_all(&self) -> Result<usize>` — daemon-startup pass: every
//!     session persisted `Live` gets a fresh shell in its saved `cwd` with
//!     `setup_script` re-run and `resurrected = true` persisted; `Dead`
//!     sessions are skipped; returns the resurrected count
//!   - `runtime(&self, session_id: &str) -> Option<Arc<SessionRuntime>>`
//!   - `attention(&self, session_id: &str) -> bool` — set on a bell byte
//!     (\x07) in pty output or on `idle_attention_after` of inactivity,
//!     cleared by `spawn_on_attach`
//!   - `crate::ws::runtime::LiveEvent::Attention` — a third broadcast
//!     variant alongside the committed `Output`/`Ended`, mapped to the
//!     'a' wire frame downstream

#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::broadcast;

use crate::model::{Session, Status};
use crate::replay::log::{Event, ReplayLog};
use crate::replay::reader;
use crate::state::registry::Registry;
use crate::ws::runtime::LiveEvent;

use crate::runtime::RuntimeManager;

/// Long enough that slow CI still settles; every wait is event-driven, so
/// the suite does not sleep this long in practice.
const WAIT: Duration = Duration::from_secs(15);
/// Injectable idle threshold — the production default is 30 s per the
/// spec; tests shrink it so the idle path is exercised in milliseconds.
const IDLE_ATTENTION_AFTER: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(25);

// ------------------------------------------------------------ plumbing

/// ASSUMED constructor — the only place to fix when the real signature
/// lands (same choke-point role as `build_app` in the http/ws suites).
fn test_manager(data_dir: &Path) -> RuntimeManager {
    RuntimeManager::new(data_dir.to_path_buf(), IDLE_ATTENTION_AFTER)
}

fn create_session(
    data_dir: &Path,
    name: &str,
    cmd: &str,
    cwd: &Path,
    setup_script: Option<&str>,
) -> Session {
    let mut registry = Registry::load(data_dir).expect("load registry");
    registry
        .create(name, cmd, cwd, setup_script.map(str::to_string))
        .expect("create session")
}

/// The session record as persisted on disk right now — the manager's
/// writes are asserted through a fresh registry read, not its internals.
fn session_on_disk(data_dir: &Path, id: &str) -> Session {
    Registry::load(data_dir)
        .expect("load registry")
        .get(id)
        .unwrap_or_else(|| panic!("session {id} must exist on disk"))
        .clone()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Concatenated output payloads of the session's replay log, in order.
fn log_output_bytes(session: &Session) -> Vec<u8> {
    let events = reader::read_events(&session.log_path).expect("read replay log");
    let mut bytes = Vec::new();
    for event in events {
        if let Event::Output(chunk) = event {
            bytes.extend_from_slice(&chunk);
        }
    }
    bytes
}

/// Read live output until `needle` appears in the concatenated 'o'
/// payloads; chunk boundaries are free to split it.
async fn wait_for_output(rx: &mut broadcast::Receiver<LiveEvent>, needle: &[u8]) {
    tokio::time::timeout(WAIT, async {
        let mut acc = Vec::new();
        loop {
            match rx.recv().await {
                Ok(LiveEvent::Output(bytes)) => {
                    acc.extend_from_slice(&bytes);
                    if acc.windows(needle.len()).any(|w| w == needle) {
                        return;
                    }
                }
                Ok(LiveEvent::Ended(notice)) => {
                    panic!("session ended before {needle:?} arrived: {notice}")
                }
                // Attention (or any future variant) does not stop the wait.
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    panic!("broadcast closed before {needle:?} arrived")
                }
            }
        }
    })
    .await
    .expect("timed out waiting for live output");
}

/// The next end-of-session broadcast; returns the notice text.
async fn wait_for_end(rx: &mut broadcast::Receiver<LiveEvent>) -> String {
    tokio::time::timeout(WAIT, async {
        loop {
            match rx.recv().await {
                Ok(LiveEvent::Ended(notice)) => return notice,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    panic!("broadcast closed without an end event")
                }
            }
        }
    })
    .await
    .expect("timed out waiting for the end event")
}

/// Poll `check` until it holds or the wait budget runs out. Used for
/// state the manager persists asynchronously (dead marks, attention).
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !check() {
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

// ------------------------------------------------------------- TC-013
// setup script on spawn / respawn, resurrected flag

#[tokio::test]
async fn setup_script_output_is_logged_before_any_later_input() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    // `echo SETUP-$((6*7))` prints SETUP-42 only when the script actually
    // executes — the pty echo of the typed prelude keeps the literal `$((`.
    let session = create_session(
        data.path(),
        "setup-first",
        "sh",
        data.path(),
        Some("echo SETUP-$((6*7))"),
    );

    let rt = manager.spawn(&session).await.expect("spawn session");
    assert!(rt.is_alive(), "a freshly spawned shell must be running");

    let mut rx = rt.subscribe();
    rt.write_input(b"echo AFTER-$((7*8))\n")
        .expect("write later input");
    wait_for_output(&mut rx, b"AFTER-56").await;

    // The pump commits output to the log before broadcasting, so by the
    // time AFTER-56 arrived both markers are on disk.
    let logged = log_output_bytes(&session);
    let setup_at =
        find(&logged, b"SETUP-42").expect("the setup script output must be in the replay log");
    let after_at =
        find(&logged, b"AFTER-56").expect("the later input's output must be in the replay log");
    assert!(
        setup_at < after_at,
        "setup script output must land in the log before later input"
    );
}

#[tokio::test]
async fn respawning_a_dead_session_starts_a_fresh_shell_that_reruns_setup() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    let session = create_session(
        data.path(),
        "respawnable",
        "sh",
        data.path(),
        Some("echo SETUP-$((6*7))"),
    );

    // Run the session to a natural death first so the dead state is
    // produced by the manager's own exit watcher, not seeded around it.
    let rt = manager.spawn(&session).await.expect("spawn");
    let mut rx = rt.subscribe();
    rt.write_input(b"exit\n").expect("exit the shell");
    wait_for_end(&mut rx).await;
    eventually("session is marked dead after the shell exits", || {
        session_on_disk(data.path(), &session.id).status == Status::Dead
    })
    .await;

    let dead = session_on_disk(data.path(), &session.id);
    let rt2 = manager.respawn(&dead).await.expect("respawn dead session");
    assert!(
        rt2.is_alive(),
        "a respawned session must run a fresh live shell"
    );
    assert_ne!(
        rt2.pty_id(),
        rt.pty_id(),
        "respawn must spawn a new pty, not resurrect the dead one"
    );

    let mut rx2 = rt2.subscribe();
    rt2.write_input(b"echo ALIVE-$((5*5))\n")
        .expect("write to the new shell");
    wait_for_output(&mut rx2, b"ALIVE-25").await;

    let logged = log_output_bytes(&session);
    assert!(
        find(&logged, b"SETUP-42").is_some(),
        "the setup script must run again in the respawned shell"
    );
    eventually("registry records the session live again", || {
        session_on_disk(data.path(), &session.id).status != Status::Dead
    })
    .await;
}

// ------------------------------------------------------------- TC-015
// exit → dead + 'e' broadcast; bell/idle attention; attach clears

#[tokio::test]
async fn pty_exit_marks_the_session_dead_and_broadcasts_an_end_event() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    let session = create_session(data.path(), "short-lived", "sh", data.path(), None);

    let rt = manager.spawn(&session).await.expect("spawn");
    let mut rx = rt.subscribe();
    rt.write_input(b"exit\n").expect("exit the shell");

    let notice = wait_for_end(&mut rx).await;
    let lower = notice.to_lowercase();
    assert!(
        lower.contains("exit") || lower.contains("end") || lower.contains("dead"),
        "the end event must say the session is over, got {notice:?}"
    );
    assert!(
        !rt.is_alive(),
        "the runtime must report dead once the child exits"
    );
    eventually("registry persists the dead mark", || {
        session_on_disk(data.path(), &session.id).status == Status::Dead
    })
    .await;
}

#[tokio::test]
async fn a_bell_in_pty_output_sets_attention_and_attaching_clears_it() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    let session = create_session(data.path(), "ringer", "sh", data.path(), None);

    let rt = manager.spawn(&session).await.expect("spawn");
    let mut rx = rt.subscribe();
    assert!(
        !manager.attention(&session.id),
        "a session starts without attention"
    );

    // `printf '\a'` makes the shell emit a raw BEL on its stdout — the
    // manager watches pty output for \x07.
    rt.write_input(b"printf '\\a'\n").expect("ring the bell");
    wait_for_output(&mut rx, b"\x07").await;

    eventually("the bell sets attention", || manager.attention(&session.id)).await;

    manager
        .spawn_on_attach(&session)
        .await
        .expect("attach to the live session");
    eventually("attaching clears attention", || {
        !manager.attention(&session.id)
    })
    .await;
}

#[tokio::test]
async fn an_idle_session_sets_attention_and_attaching_clears_it() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    let session = create_session(data.path(), "quiet", "sh", data.path(), None);

    let _rt = manager.spawn(&session).await.expect("spawn");
    assert!(
        !manager.attention(&session.id),
        "no attention before the idle threshold passes"
    );

    // Nothing is written and nobody is attached; after the initial
    // prompt the session is silent for longer than the injected
    // IDLE_ATTENTION_AFTER.
    eventually("an idle session sets attention", || {
        manager.attention(&session.id)
    })
    .await;

    manager
        .spawn_on_attach(&session)
        .await
        .expect("attach to the live session");
    eventually("attaching clears the idle attention", || {
        !manager.attention(&session.id)
    })
    .await;
}

#[tokio::test]
async fn attached_clients_are_notified_when_attention_is_set() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    let session = create_session(data.path(), "ringer", "sh", data.path(), None);

    let rt = manager.spawn(&session).await.expect("spawn");
    let mut rx = rt.subscribe();
    rt.write_input(b"printf '\\a'\n").expect("ring the bell");

    // ASSUMED third LiveEvent variant — the 'a' wire frame's source,
    // broadcast on the same channel as output and end.
    tokio::time::timeout(WAIT, async {
        loop {
            match rx.recv().await {
                Ok(LiveEvent::Attention) => return,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    panic!("broadcast closed without an attention event")
                }
            }
        }
    })
    .await
    .expect("timed out waiting for the attention notification");
}

#[tokio::test]
async fn attaching_twice_to_the_same_live_session_reuses_one_shell() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    let session = create_session(data.path(), "shared", "sh", data.path(), None);

    let first = manager
        .spawn_on_attach(&session)
        .await
        .expect("first attach spawns");
    let second = manager
        .spawn_on_attach(&session)
        .await
        .expect("second attach");

    assert_eq!(
        first.pty_id(),
        second.pty_id(),
        "a second attach to the same live session must land on the same shell"
    );
}

#[tokio::test]
async fn killing_a_session_terminates_its_shell_and_marks_it_dead() {
    let data = TempDir::new().expect("tempdir");
    let manager = test_manager(data.path());
    let session = create_session(data.path(), "doomed", "sh", data.path(), None);

    let rt = manager.spawn(&session).await.expect("spawn");
    manager.kill(&session.id).await.expect("kill session");

    eventually("kill terminates the shell", || !rt.is_alive()).await;
    eventually("kill persists the dead mark", || {
        session_on_disk(data.path(), &session.id).status == Status::Dead
    })
    .await;
}

// ------------------------------------------------------------- TC-019
// daemon-restart resurrection

#[tokio::test]
async fn daemon_restart_resurrects_a_live_session_with_a_fresh_shell_and_reruns_setup() {
    let data = TempDir::new().expect("tempdir");
    // A dedicated cwd proves the resurrected shell starts in the saved
    // cwd rather than wherever the daemon happens to run.
    let work = data.path().join("workdir");
    std::fs::create_dir_all(&work).expect("create session cwd");
    let work_canon = work.canonicalize().expect("canonicalize cwd");
    let session = create_session(
        data.path(),
        "long-runner",
        "sh",
        &work,
        Some("export RESFLAG=up-25"),
    );

    {
        // First daemon lifetime: spawn and prove the shell works.
        let first = test_manager(data.path());
        let rt = first.spawn(&session).await.expect("spawn on first daemon");
        let mut rx = rt.subscribe();
        rt.write_input(b"echo T1-$((4*4))\n")
            .expect("write to first shell");
        wait_for_output(&mut rx, b"T1-16").await;
        // Drop kills the pty child — the "daemon" is gone, the registry
        // still says Live.
    }

    // Daemon restart: a new manager over the same data dir.
    let second = test_manager(data.path());
    let resurrected = second.resurrect_all().await.expect("resurrect_all");
    assert!(
        resurrected >= 1,
        "a daemon restart must resurrect the persisted live session"
    );

    let persisted = session_on_disk(data.path(), &session.id);
    assert!(
        persisted.resurrected,
        "the resurrected flag must be persisted on the session"
    );
    assert_ne!(
        persisted.status,
        Status::Dead,
        "a resurrected session must not be dead"
    );

    let rt = second
        .runtime(&session.id)
        .expect("resurrected session must have a live runtime");
    assert!(
        rt.is_alive(),
        "resurrection spawns a fresh live shell, not a preserved process"
    );

    let mut rx = rt.subscribe();
    rt.write_input(b"pwd\n")
        .expect("ask the fresh shell where it is");
    wait_for_output(&mut rx, work_canon.to_string_lossy().as_bytes()).await;

    // RESFLAG only exists if the setup script re-ran in the new shell —
    // a resurrected session is a fresh process, not a preserved one.
    rt.write_input(b"echo GOT=$RESFLAG\n")
        .expect("read the setup-exported variable");
    wait_for_output(&mut rx, b"GOT=up-25").await;
}

#[tokio::test]
async fn daemon_restart_leaves_dead_sessions_dead_and_unspawned() {
    let data = TempDir::new().expect("tempdir");
    let dead = {
        let mut registry = Registry::load(data.path()).expect("load seed registry");
        let dead = registry
            .create(
                "already-dead",
                "sh",
                data.path(),
                Some("echo RIP".to_string()),
            )
            .expect("seed session");
        {
            let mut log = ReplayLog::create(&dead.log_path, 80, 24).expect("seed log");
            log.append_output(b"last words\r\n").expect("append output");
            log.append_exit(137).expect("append exit");
        }
        registry.mark_dead(&dead.id).expect("mark dead");
        dead
    };

    let manager = test_manager(data.path());
    let resurrected = manager.resurrect_all().await.expect("resurrect_all");
    assert_eq!(resurrected, 0, "a dead session must not be resurrected");
    assert!(
        manager.runtime(&dead.id).is_none(),
        "a dead session must have no runtime after restart"
    );

    let persisted = session_on_disk(data.path(), &dead.id);
    assert_eq!(
        persisted.status,
        Status::Dead,
        "resurrection must leave dead sessions dead"
    );
    assert!(
        !persisted.resurrected,
        "a dead session must not gain the resurrected flag"
    );
    assert!(
        manager.spawn_on_attach(&persisted).await.is_err(),
        "attaching a dead session must not spawn a shell"
    );
}
