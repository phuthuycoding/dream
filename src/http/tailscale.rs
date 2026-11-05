//! Tailscale detection + remote-access guidance (TASK-010 / FR-009).
//!
//! `tailscale_guidance` builds the startup hint telling the operator how to
//! reach the loopback-only server from their tailnet. It never suggests
//! `tailscale funnel` — dream must stay tailnet-only, never public.

use std::process::{Command, Stdio};

/// Guidance text for exposing dream on the tailnet.
///
/// When the `tailscale` binary is on PATH the text is the exact
/// `tailscale serve --bg <port>` command; otherwise it is an install hint.
/// The caller decides when to print it (loopback bind only — a `--public`
/// bind is already reachable without Tailscale).
pub fn tailscale_guidance(port: u16) -> String {
    if tailscale_on_path() {
        format!(
            "tailscale detected — to reach dream from your tailnet:\n  \
             tailscale serve --bg {port}\n  \
             this exposes dream on your tailnet only"
        )
    } else {
        "tailscale not found — install it for tailnet access:\n  \
         brew install tailscale        (macOS)\n  \
         https://tailscale.com/download"
            .to_string()
    }
}

/// True when a `tailscale` binary is runnable — on PATH, or at the
/// well-known locations for brew/pkg installs. launchd runs agents with a
/// minimal PATH, so the PATH probe alone reports "not found" on machines
/// where Tailscale is installed.
fn tailscale_on_path() -> bool {
    const CANDIDATES: &[&str] = &[
        "tailscale",
        "/opt/homebrew/bin/tailscale",
        "/usr/local/bin/tailscale",
        "/usr/bin/tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ];
    CANDIDATES.iter().any(|bin| {
        Command::new(bin)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::tailscale_guidance;

    #[test]
    fn guidance_mentions_serve_or_install_and_never_funnel() {
        let text = tailscale_guidance(4020);
        assert!(
            text.contains("tailscale serve --bg 4020") || text.contains("install"),
            "guidance must be either the serve command or the install hint, got: {text}"
        );
        assert!(
            !text.to_lowercase().contains("funnel"),
            "guidance must never suggest funnel, got: {text}"
        );
    }
}
