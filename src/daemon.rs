//! `dream up|down|status` — macOS launchd agent lifecycle.
//!
//! `up` installs `~/Library/LaunchAgents/com.dream.daemon.plist` and
//! bootstraps it so the daemon starts now and on every login (KeepAlive
//! restarts it on crash). `down` boots it out and removes the plist.
//! `status` reports whether the agent is live and prints the token URL.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};

use crate::cli::Cli;
use crate::config::Config;

const LABEL: &str = "com.dream.daemon";

fn home() -> anyhow::Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow!("cannot resolve home directory"))
}

fn plist_path() -> anyhow::Result<PathBuf> {
    Ok(home()?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn log_dir() -> anyhow::Result<PathBuf> {
    Ok(home()?.join("Library/Logs/dream"))
}

fn uid() -> anyhow::Result<String> {
    let out = Command::new("id").arg("-u").output()?;
    if !out.status.success() {
        return Err(anyhow!("`id -u` failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn plist_xml(program_args: &[String], log_out: &str, log_err: &str) -> String {
    let args: String = program_args
        .iter()
        .map(|a| format!("    <string>{}</string>", xml_escape(a)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{out}</string>
  <key>StandardErrorPath</key>
  <string>{err}</string>
</dict>
</plist>
"#,
        out = xml_escape(log_out),
        err = xml_escape(log_err),
    )
}

fn launchctl(args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new("launchctl")
        .args(args)
        .output()
        .context("failed to run launchctl")?;
    if !out.status.success() {
        return Err(anyhow!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// `dream up` — install + start the agent with the CLI's config values.
pub fn up(cli: &Cli) -> anyhow::Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(anyhow!(
            "`dream up` uses launchd and only runs on macOS — start the daemon with plain `dream`"
        ));
    }
    let exe = std::env::current_exe().context("cannot resolve own binary path")?;
    let cfg = Config::load(cli)?;
    let logs = log_dir()?;
    fs::create_dir_all(&logs)?;
    fs::create_dir_all(plist_path()?.parent().unwrap())?;

    let mut args = vec![exe.display().to_string()];
    args.push("--port".into());
    args.push(cfg.port.to_string());
    args.push("--bind".into());
    args.push(cfg.bind.to_string());
    args.push("--data-dir".into());
    args.push(cfg.data_dir.display().to_string());
    if cfg.public {
        args.push("--public".into());
    }
    if let Some(cap) = cfg.log_cap {
        args.push("--log-cap".into());
        args.push(cap.to_string());
    }
    if let Some(age) = cfg.log_max_age {
        args.push("--log-max-age".into());
        args.push(age.as_secs().to_string());
    }

    let plist = plist_path()?;
    fs::write(
        &plist,
        plist_xml(
            &args,
            &logs.join("out.log").display().to_string(),
            &logs.join("err.log").display().to_string(),
        ),
    )
    .with_context(|| format!("cannot write {}", plist.display()))?;

    let uid = uid()?;
    let domain = format!("gui/{uid}");
    // Already loaded → bootout first so `up` is idempotent (flag changes apply).
    let _ = launchctl(&["bootout", &domain, LABEL]);
    launchctl(&["bootstrap", &domain, &plist.display().to_string()])?;

    // Wait for the daemon to bind + write its token (first run generates it).
    let token_file = cfg.data_dir.join("token");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(token) = fs::read_to_string(&token_file) {
            let token = token.trim();
            if !token.is_empty() {
                println!("dream is up (launchd agent {LABEL})");
                println!("  http://127.0.0.1:{}/?token={token}", cfg.port);
                println!("  logs: {}", logs.display());
                println!("  stop: dream down");
                if cfg.bind.is_loopback() {
                    println!("{}", crate::http::tailscale::tailscale_guidance(cfg.port));
                }
                return Ok(());
            }
        }
        if Instant::now() > deadline {
            println!("dream agent bootstrapped but no token appeared in {}s", 15);
            println!("check {} for the cause", logs.join("err.log").display());
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// `dream down` — stop the agent and remove the plist.
pub fn down() -> anyhow::Result<()> {
    let uid = uid()?;
    match launchctl(&["bootout", &format!("gui/{uid}"), LABEL]) {
        Ok(()) => {}
        Err(err) => eprintln!("launchctl bootout: {err} (agent may not be running)"),
    }
    let plist = plist_path()?;
    if plist.exists() {
        fs::remove_file(&plist)?;
    }
    println!("dream is down");
    Ok(())
}

/// `dream status` — report agent state + URL.
pub fn status(cli: &Cli) -> anyhow::Result<()> {
    let uid = uid()?;
    let out = Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{LABEL}")])
        .output()?;
    if !out.status.success() {
        println!("dream is not running (no launchd agent {LABEL})");
        println!("start it: dream up");
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let state = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("state ="))
        .map(str::trim)
        .unwrap_or("unknown");
    println!("dream agent {LABEL}: state = {state}");
    let cfg = Config::load(cli)?;
    if let Ok(token) = fs::read_to_string(cfg.data_dir.join("token")) {
        println!("  http://127.0.0.1:{}/?token={}", cfg.port, token.trim());
    }
    println!("  logs: {}", log_dir()?.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_contains_label_args_and_keepalive() {
        let xml = plist_xml(
            &["/opt/dream".into(), "--port".into(), "4020".into()],
            "/tmp/o",
            "/tmp/e",
        );
        assert!(xml.contains("com.dream.daemon"));
        assert!(xml.contains("<string>/opt/dream</string>"));
        assert!(xml.contains("<string>--port</string>"));
        assert!(xml.contains("<true/>"));
        assert!(xml.contains("/tmp/o"));
        assert!(xml.contains("/tmp/e"));
    }

    #[test]
    fn plist_escapes_xml_chars() {
        let xml = plist_xml(&["/a&b/<x>".into()], "/o", "/e");
        assert!(xml.contains("/a&amp;b/&lt;x&gt;"));
    }
}
