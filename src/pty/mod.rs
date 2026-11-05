//! PTY session layer: a `portable-pty` child wrapped in `PtyHandle`.
//!
//! portable-pty does blocking `std::io` on the master side, so each handle
//! owns dedicated threads rather than `.await`ing on the pty: a reader
//! pumping bytes into a tokio mpsc channel, a writer fed by a bounded
//! std mpsc channel, and a supervisor that owns the `Child` and
//! publishes its exit status on a watch channel.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context};
use portable_pty::{native_pty_system, Child, CommandBuilder, ExitStatus, MasterPty, PtySize};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

const READ_BUF_SIZE: usize = 8 * 1024;
const READ_CHAN_CAPACITY: usize = 64;
const WRITE_CHAN_CAPACITY: usize = 64;
const SUPERVISOR_POLL: Duration = Duration::from_millis(25);

/// read(2) on a pty master fails with EIO once the last slave fd is gone;
/// on unix that is the normal end-of-stream signal, not an error. libc is
/// not a direct dependency of this crate; EIO is 5 on every unix target.
#[cfg(unix)]
const ERRNO_EIO: i32 = 5;

/// A running pty session: write input, receive output, resize, observe the
/// child's exit status, and kill it.
pub struct PtyHandle {
    pub id: Uuid,
    master: Box<dyn MasterPty>,
    writer_tx: std_mpsc::SyncSender<Vec<u8>>,
    kill_tx: std_mpsc::Sender<()>,
    exit_rx: watch::Receiver<Option<ExitStatus>>,
    read_rx: Option<mpsc::Receiver<Vec<u8>>>,
}

impl PtyHandle {
    /// Spawn `cmd` on a fresh pty rooted at `cwd`.
    ///
    /// `setup_script`, when given, is queued as the child's first input line —
    /// a shell prelude that runs before anything written later (so exported
    /// variables are visible to subsequent commands).
    ///
    /// openpty + `spawn_command` are blocking `std::io`, so `spawn` blocks
    /// the calling thread until the child is up. `spawn_command` forks via
    /// `pre_exec`; forking inside a busy multi-threaded runtime is a
    /// documented crash source on macOS, and that risk is process-wide —
    /// no thread choice at this call site changes it. What the caller does
    /// control is which thread is stalled: async callers must use
    /// [`spawn_async`](Self::spawn_async), which runs this whole function
    /// on `tokio::task::spawn_blocking` instead of blocking a runtime
    /// worker.
    pub fn spawn(
        cwd: &Path,
        setup_script: Option<&str>,
        mut cmd: CommandBuilder,
    ) -> anyhow::Result<Self> {
        cmd.cwd(cwd.as_os_str());

        let pair = native_pty_system()
            .openpty(PtySize::default())
            .map_err(|e| e.context("openpty"))?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| e.context("spawn command into pty"))?;
        // The child inherited its own slave fd during fork; ours must
        // be closed or the master never sees end-of-stream. Queued
        // output is still delivered to the master before the
        // terminating EIO, so the reader drains everything.
        drop(pair.slave);
        let master = pair.master;

        let id = Uuid::new_v4();

        let (read_tx, read_rx) = mpsc::channel(READ_CHAN_CAPACITY);
        let reader = master
            .try_clone_reader()
            .map_err(|e| e.context("clone pty reader"))?;
        thread::Builder::new()
            .name(format!("pty-read-{id}"))
            .spawn(move || read_loop(reader, read_tx, id))
            .context("spawn pty reader thread")?;

        let (writer_tx, writer_rx) = std_mpsc::sync_channel::<Vec<u8>>(WRITE_CHAN_CAPACITY);
        let writer = master
            .take_writer()
            .map_err(|e| e.context("take pty writer"))?;
        thread::Builder::new()
            .name(format!("pty-write-{id}"))
            .spawn(move || write_loop(writer, writer_rx, id))
            .context("spawn pty writer thread")?;

        let (exit_tx, exit_rx) = watch::channel(None);
        let (kill_tx, kill_rx) = std_mpsc::channel::<()>();
        thread::Builder::new()
            .name(format!("pty-wait-{id}"))
            .spawn(move || supervise(child, kill_rx, exit_tx, id))
            .context("spawn pty supervisor thread")?;

        let handle = Self {
            id,
            master,
            writer_tx,
            kill_tx,
            exit_rx,
            read_rx: Some(read_rx),
        };

        if let Some(script) = setup_script {
            let mut prelude = script.as_bytes().to_vec();
            prelude.push(b'\n');
            handle.write(&prelude).context("inject setup script")?;
        }

        Ok(handle)
    }

    /// Async variant of [`spawn`](Self::spawn): the blocking openpty +
    /// `spawn_command` work runs on `tokio::task::spawn_blocking`, so no
    /// tokio worker stalls while the child comes up. Arguments are owned
    /// because the blocking closure must be `'static`.
    pub async fn spawn_async(
        cwd: PathBuf,
        setup_script: Option<String>,
        cmd: CommandBuilder,
    ) -> anyhow::Result<Self> {
        tokio::task::spawn_blocking(move || Self::spawn(&cwd, setup_script.as_deref(), cmd))
            .await
            .context("pty spawn task failed to join")?
    }

    /// Take the stream of pty output chunks. May only be called once.
    pub fn read(&mut self) -> mpsc::Receiver<Vec<u8>> {
        self.read_rx
            .take()
            .expect("PtyHandle::read may only be called once")
    }

    /// Queue bytes for the child's stdin. Fails once the writer thread is
    /// gone (session ended or a previous write errored), or when the
    /// bounded queue is full — the child is not draining stdin fast
    /// enough; retrying later is the caller's decision.
    pub fn write(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer_tx.try_send(bytes.to_vec()).map_err(|e| match e {
            std_mpsc::TrySendError::Full(_) => anyhow!("pty writer queue is full"),
            std_mpsc::TrySendError::Disconnected(_) => anyhow!("pty writer thread is gone"),
        })
    }

    /// Resize the pty; the kernel reports SIGWINCH to the child.
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }

    /// Receiver whose value flips to `Some(status)` when the child exits.
    pub fn wait_exit(&self) -> watch::Receiver<Option<ExitStatus>> {
        self.exit_rx.clone()
    }

    /// Terminate the child: SIGHUP first, SIGKILL if it keeps running.
    pub fn kill(&self) -> anyhow::Result<()> {
        self.kill_tx
            .send(())
            .map_err(|_| anyhow!("pty supervisor is gone"))
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        // A running child is this handle's resource; don't leave a shell
        // behind on a dead pty.
        if self.exit_rx.borrow().is_none() {
            if let Err(e) = self.kill() {
                tracing::warn!(pty_id = %self.id, error = %e, "failed to kill pty child on drop");
            }
        }
    }
}

fn read_loop(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>, id: Uuid) {
    let mut buf = vec![0u8; READ_BUF_SIZE];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break; // consumer dropped the receiver
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                #[cfg(unix)]
                if e.raw_os_error() == Some(ERRNO_EIO) {
                    break; // last slave fd closed; queued output was delivered first
                }
                tracing::error!(pty_id = %id, error = %e, "pty read failed");
                break;
            }
        }
    }
}

fn write_loop(mut writer: Box<dyn Write + Send>, rx: std_mpsc::Receiver<Vec<u8>>, id: Uuid) {
    while let Ok(bytes) = rx.recv() {
        if let Err(e) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
            tracing::error!(pty_id = %id, error = %e, "pty write failed; stopping writer");
            return;
        }
    }
}

fn supervise(
    mut child: Box<dyn Child + Send + Sync>,
    kill_rx: std_mpsc::Receiver<()>,
    exit_tx: watch::Sender<Option<ExitStatus>>,
    id: Uuid,
) {
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                tracing::error!(pty_id = %id, error = %e, "try_wait on pty child failed");
                break ExitStatus::with_signal("wait error");
            }
        }
        if kill_rx.try_recv().is_ok() {
            // Child::kill sends SIGHUP, then escalates to SIGKILL after a
            // grace period; the loop's next try_wait reaps the status.
            if let Err(e) = child.kill() {
                tracing::warn!(pty_id = %id, error = %e, "pty kill request failed");
            }
        }
        thread::sleep(SUPERVISOR_POLL);
    };
    // send_replace never fails; receivers observe the final status if any.
    exit_tx.send_replace(Some(status));
}

#[cfg(test)]
mod tests;
