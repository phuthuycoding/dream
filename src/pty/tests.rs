use std::time::Duration;

use portable_pty::{CommandBuilder, ExitStatus};
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

use super::PtyHandle;

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

fn spawn_in(temp: &TempDir, setup_script: Option<&str>, cmd: CommandBuilder) -> PtyHandle {
    PtyHandle::spawn(temp.path(), setup_script, cmd).expect("pty spawn failed")
}

async fn collect_until(
    rx: &mut mpsc::Receiver<Vec<u8>>,
    done: impl Fn(&[u8]) -> bool,
    dur: Duration,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let deadline = tokio::time::sleep(dur);
    tokio::pin!(deadline);
    loop {
        if done(&buf) {
            break;
        }
        tokio::select! {
            _ = &mut deadline => break,
            chunk = rx.recv() => match chunk {
                Some(chunk) => buf.extend_from_slice(&chunk),
                None => break,
            },
        }
    }
    buf
}

fn output_has_line(buf: &[u8], line: &str) -> bool {
    String::from_utf8_lossy(buf)
        .lines()
        .any(|l| l.trim() == line)
}

async fn wait_until_exited(rx: &mut watch::Receiver<Option<ExitStatus>>) {
    loop {
        if rx.borrow().is_some() {
            return;
        }
        // Sender dropped: whatever state remains is final, let the caller assert it.
        if rx.changed().await.is_err() {
            return;
        }
    }
}

#[tokio::test]
async fn spawned_command_runs_in_the_given_working_directory() {
    let temp = TempDir::new().expect("create temp dir");

    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", "pwd"]);
    let mut handle = spawn_in(&temp, None, cmd);
    let mut rx = handle.read();

    // `sh -c pwd` reports the logical cwd, but the implementation may canonicalize
    // it first, so accept either form (macOS: /var/... vs /private/var/...).
    let literal = temp.path().to_string_lossy().into_owned();
    let canonical = std::fs::canonicalize(temp.path())
        .expect("canonicalize temp dir")
        .to_string_lossy()
        .into_owned();

    let output = collect_until(
        &mut rx,
        |buf| {
            let text = String::from_utf8_lossy(buf);
            text.contains(literal.as_str()) || text.contains(canonical.as_str())
        },
        READ_TIMEOUT,
    )
    .await;

    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains(&literal) || text.contains(&canonical),
        "expected pwd to report {literal} (canonical {canonical}), got: {text:?}"
    );
}

#[tokio::test]
async fn written_input_produces_command_output() {
    let temp = TempDir::new().expect("create temp dir");
    let mut handle = spawn_in(&temp, None, CommandBuilder::new("sh"));
    let mut rx = handle.read();

    handle.write(b"echo hi\n").expect("write to pty");

    // Match a whole `hi` line so the terminal echo of the typed `echo hi`
    // input does not satisfy the assertion by itself.
    let output = collect_until(
        &mut rx,
        |buf| output_has_line(buf, "hi"),
        READ_TIMEOUT,
    )
    .await;

    assert!(
        output_has_line(&output, "hi"),
        "expected a `hi` output line after `echo hi`, got: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[tokio::test]
async fn resize_updates_the_terminal_size_seen_by_the_shell() {
    let temp = TempDir::new().expect("create temp dir");
    let mut handle = spawn_in(&temp, None, CommandBuilder::new("sh"));
    let mut rx = handle.read();

    handle.resize(120, 40).expect("resize pty");
    handle.write(b"stty size\n").expect("write stty");

    // `stty size` prints `rows cols`, so 120 cols x 40 rows reads `40 120`.
    let output = collect_until(
        &mut rx,
        |buf| output_has_line(buf, "40 120"),
        READ_TIMEOUT,
    )
    .await;

    assert!(
        output_has_line(&output, "40 120"),
        "expected `stty size` to report `40 120` after resize, got: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[tokio::test]
async fn shell_exit_is_observable_with_its_exit_status() {
    let temp = TempDir::new().expect("create temp dir");

    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", "exit 7"]);
    let handle = spawn_in(&temp, None, cmd);
    let mut exit_rx = handle.wait_exit();

    timeout(EXIT_TIMEOUT, wait_until_exited(&mut exit_rx))
        .await
        .expect("timed out waiting for shell exit");

    let code = exit_rx.borrow().as_ref().map(|status| status.exit_code());
    assert_eq!(code, Some(7), "expected exit status 7");
}

#[tokio::test]
async fn kill_terminates_a_shell_that_is_still_running() {
    let temp = TempDir::new().expect("create temp dir");
    let handle = spawn_in(&temp, None, CommandBuilder::new("sh"));
    let mut exit_rx = handle.wait_exit();

    handle.kill().expect("kill pty shell");

    timeout(EXIT_TIMEOUT, wait_until_exited(&mut exit_rx))
        .await
        .expect("shell did not terminate after kill");

    let succeeded = exit_rx.borrow().as_ref().map(|status| status.success());
    assert_eq!(
        succeeded,
        Some(false),
        "killed shell should not report success"
    );
}

#[tokio::test]
async fn final_output_is_delivered_before_the_stream_ends() {
    let temp = TempDir::new().expect("create temp dir");

    // The process exits immediately after writing; on macOS the tail of the
    // output is lost to EIO unless the slave stays open until the master
    // drains. This is the observable contract for that caveat.
    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", "echo tail_marker; exit"]);
    let mut handle = spawn_in(&temp, None, cmd);
    let mut rx = handle.read();

    let output = collect_until(
        &mut rx,
        |buf| output_has_line(buf, "tail_marker"),
        READ_TIMEOUT,
    )
    .await;

    assert!(
        output_has_line(&output, "tail_marker"),
        "expected `tail_marker` in drained output, got: {:?}",
        String::from_utf8_lossy(&output)
    );
}

#[tokio::test]
async fn setup_script_runs_before_the_shell_accepts_input() {
    let temp = TempDir::new().expect("create temp dir");

    // An exported variable survives both plausible injection models: prelude
    // bytes on the shell's stdin and `sh -c 'script; exec $SHELL'` wrapping.
    let mut handle = spawn_in(
        &temp,
        Some("export KF_SETUP_MARKER=ran"),
        CommandBuilder::new("sh"),
    );
    let mut rx = handle.read();

    handle
        .write(b"echo $KF_SETUP_MARKER\n")
        .expect("write echo");

    let output = collect_until(
        &mut rx,
        |buf| output_has_line(buf, "ran"),
        READ_TIMEOUT,
    )
    .await;

    assert!(
        output_has_line(&output, "ran"),
        "expected setup_script export to be visible in the shell, got: {:?}",
        String::from_utf8_lossy(&output)
    );
}
