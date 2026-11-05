//! `dream up|down|status|restart|logs|url|ps|doctor` — service lifecycle
//! (launchd on macOS, systemd --user on Linux) plus local admin.

use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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
    let out = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .context("cannot run /usr/bin/id")?;
    if !out.status.success() {
        return Err(anyhow!("`id -u` failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn service() -> anyhow::Result<String> {
    // launchctl verbs take one `domain/service` argument — passing the
    // domain and label separately is silently wrong (verified: bootout
    // fails with an I/O error and the agent keeps running).
    Ok(format!("gui/{}/{LABEL}", uid()?))
}

/// Which service manager this host uses.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Platform {
    Macos,
    Linux,
}

fn platform() -> anyhow::Result<Platform> {
    if cfg!(target_os = "macos") {
        Ok(Platform::Macos)
    } else if cfg!(target_os = "linux") {
        Ok(Platform::Linux)
    } else {
        Err(anyhow!(
            "service lifecycle supports launchd (macOS) and systemd --user (Linux) — \
             start the daemon with plain `dream` on this platform"
        ))
    }
}

fn systemd_unit_path() -> anyhow::Result<PathBuf> {
    Ok(home()?
        .join(".config/systemd/user")
        .join("dream.service"))
}

fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new("/usr/bin/systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("systemctl not available — is this a systemd host?")?;
    if !out.status.success() {
        return Err(anyhow!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn systemd_unit(program_args: &[String]) -> anyhow::Result<String> {
    for a in program_args {
        if a.chars().any(|c| c.is_control()) {
            return Err(anyhow!("argument contains a control character — refusing to write the unit"));
        }
    }
    let exec = program_args
        .iter()
        // systemd expands % specifiers and $ env vars regardless of quoting.
        .map(|a| a.replace('%', "%%").replace('$', "$$").replace('\\', "\\\\"))
        .map(|a| shell_quote(&a))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!(
        "[Unit]\nDescription=dream web terminal daemon\nAfter=network-online.target\n\n\
         [Service]\nExecStart={exec}\nRestart=always\nRestartSec=3\n\n\
         [Install]\nWantedBy=default.target\n"
    ))
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_alphanumeric() || "-_./:".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn systemd_out(args: &[&str]) -> Option<std::process::Output> {
    Command::new("/usr/bin/systemctl")
        .arg("--user")
        .args(args)
        .output()
        .ok()
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
    let out = Command::new("/bin/launchctl").args(args).output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// The service daemon's credential — only the token FILE counts; a CLI
/// `--auth-token` override is never stored in the unit, so the service
/// would reject it.
fn read_service_token(cfg: &Config) -> Option<String> {
    fs::read_to_string(cfg.data_dir.join("token"))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .filter(|t| t.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)))
}

fn read_token(cfg: &Config) -> Option<String> {
    let token = cfg
        .auth_token
        .clone()
        .or_else(|| read_service_token(cfg))?;
    // The token feeds an HTTP header verbatim — reject anything that could
    // inject lines.
    if token.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) { Some(token) } else { None }
}

/// Host the daemon is reachable on for printed URLs and admin connections —
/// loopback by default; for `0.0.0.0`/`::` binds loopback is still the
/// reachable local admin address.
fn admin_host(cfg: &Config) -> String {
    if cfg.bind.is_unspecified() { "127.0.0.1".to_string() } else { cfg.bind.to_string() }
}

/// admin_host formatted for URLs/Host headers — IPv6 needs brackets.
fn admin_host_display(cfg: &Config) -> String {
    let h = admin_host(cfg);
    if h.contains(':') { format!("[{h}]") } else { h }
}

fn print_url(cfg: &Config) {
    match read_service_token(cfg) {
        Some(token) => println!("  http://{}:{}/?token={token}", admin_host_display(cfg), cfg.port),
        None => println!("  (no token file at {} yet)", cfg.data_dir.display()),
    }
}

/// `dream up` — install + bootstrap the agent; idempotent.
pub fn up(cli: &Cli) -> anyhow::Result<()> {
    let platform = platform()?;
    let exe = std::env::current_exe().context("cannot resolve own binary path")?;
    let cfg = Config::load(cli)?;
    if cfg.auth_token.is_some() {
        eprintln!("note: --auth-token/DREAM_AUTH_TOKEN is not stored in the service unit; the service generates/reads the token file");
    }

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

    match platform {
        Platform::Macos => {
            let logs = log_dir()?;
            fs::create_dir_all(&logs)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o700))
                    .context("cannot lock down the launchd log dir")?;
            }
            fs::create_dir_all(plist_path()?.parent().unwrap())?;
            let plist = plist_path()?;
            fs::write(
                &plist,
                plist_xml(&args, &logs.join("out.log").display().to_string(), &logs.join("err.log").display().to_string()),
            )
            .with_context(|| format!("cannot write {}", plist.display()))?;
            let svc = service()?;
            let _ = launchctl(&["bootout", &svc]);
            launchctl(&["bootstrap", &format!("gui/{}", uid()?), &plist.display().to_string()])?;
        }
        Platform::Linux => {
            let unit = systemd_unit_path()?;
            fs::create_dir_all(unit.parent().unwrap())?;
            fs::write(&unit, systemd_unit(&args)?)
                .with_context(|| format!("cannot write {}", unit.display()))?;
            systemctl(&["daemon-reload"])?;
            let _ = systemctl(&["disable", "--now", "dream.service"]);
            systemctl(&["enable", "--now", "dream.service"])?;
        }
    }

    // The token file persists across runs and a bare port probe can hit a
    // foreign listener — require both the service manager reporting the
    // unit active AND the port answering.
    let deadline = Instant::now() + Duration::from_secs(15);
    let service_active = |p: Platform| -> bool {
        match p {
            Platform::Macos => Command::new("/bin/launchctl")
                .args(["print", &format!("gui/{}/{}", uid().unwrap_or_default(), LABEL)])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            Platform::Linux => systemd_out(&["is-active", "dream.service"])
                .map(|o| o.status.success())
                .unwrap_or(false),
        }
    };
    loop {
        if service_active(platform)
            && TcpStream::connect((admin_host(&cfg).as_str(), cfg.port)).is_ok()
        {
            let mgr = match platform { Platform::Macos => "launchd agent", Platform::Linux => "systemd --user service" };
            println!("dream is up ({mgr} {LABEL})");
            print_url(&cfg);
            match platform {
                Platform::Macos => println!("  logs: {}", log_dir()?.display()),
                Platform::Linux => println!("  logs: journalctl --user -u dream  (or: dream logs)"),
            }
            println!("  stop: dream down");
            if cfg.bind.is_loopback() {
                println!("{}", crate::http::tailscale::tailscale_guidance(cfg.port));
            }
            return Ok(());
        }
        if Instant::now() > deadline {
            println!("dream service installed but the port did not come up within 30s");
            match platform {
                Platform::Macos => println!("check {} for the cause", log_dir()?.join("err.log").display()),
                Platform::Linux => println!("check `journalctl --user -u dream` for the cause"),
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// `dream down` — stop the agent and remove the plist.
pub fn down() -> anyhow::Result<()> {
    match platform()? {
        Platform::Macos => {
            match launchctl(&["bootout", &service()?]) {
                Ok(()) => {}
                Err(err) => eprintln!("launchctl bootout: {err} (agent may not be running)"),
            }
            let plist = plist_path()?;
            if plist.exists() {
                fs::remove_file(&plist)?;
            }
        }
        Platform::Linux => {
            match systemctl(&["disable", "--now", "dream.service"]) {
                Ok(()) => {}
                Err(err) => eprintln!("systemctl disable: {err} (unit may not be running)"),
            }
            let unit = systemd_unit_path()?;
            if unit.exists() {
                fs::remove_file(&unit)?;
            }
            let _ = systemctl(&["daemon-reload"]);
        }
    }
    println!("dream is down");
    Ok(())
}

/// `dream status` — report agent state + URL.
pub fn status(cli: &Cli) -> anyhow::Result<()> {
    let state = match platform()? {
        Platform::Macos => {
            let out = Command::new("/bin/launchctl").args(["print", &service()?]).output()?;
            if !out.status.success() {
                println!("dream is not running (no launchd agent {LABEL})");
                println!("start it: dream up");
                return Ok(());
            }
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .find_map(|l| l.trim().strip_prefix("state ="))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".into())
        }
        Platform::Linux => match systemd_out(&["is-active", "dream.service"]) {
            Some(o) if o.status.success() => "running".into(),
            Some(o) => {
                let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
                if t.is_empty() { format!("not running{}", if e.is_empty() { "".into() } else { format!(" ({e})") }) } else { t }
            }
            None => "unknown (systemctl missing)".into(),
        },
    };
    if state == "inactive" || state == "not running" || state.starts_with("not running") || state == "dead" || state == "failed" || state.starts_with("unknown") {
        println!("dream is not running ({state})");
        println!("start it: dream up");
        return Ok(());
    }
    println!("dream agent {LABEL}: state = {state}");
    let cfg = Config::load(cli)?;
    print_url(&cfg);
    match platform()? {
        Platform::Macos => println!("  logs: {}", log_dir()?.display()),
        Platform::Linux => println!("  logs: journalctl --user -u dream  (or: dream logs)"),
    }
    Ok(())
}

/// `dream restart` — kickstart the agent.
pub fn restart() -> anyhow::Result<()> {
    match platform()? {
        Platform::Macos => launchctl(&["kickstart", "-k", &service()?])?,
        Platform::Linux => systemctl(&["restart", "dream.service"])?,
    }
    println!("dream restarted");
    Ok(())
}

/// `dream logs [-f]` — cat/tail the service logs (launchd files on
/// macOS, journalctl --user on Linux).
pub fn logs(follow: bool) -> anyhow::Result<()> {
    if platform()? == Platform::Linux {
        let mut args = vec!["--user", "-u", "dream.service", "-n", "50", "--no-pager"];
        if follow {
            args.push("-f");
        }
        let mut cmd = Command::new("/usr/bin/journalctl")
            .args(&args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("journalctl not available")?;
        cmd.wait()?;
        return Ok(());
    }
    let dir = log_dir()?;
    let out_log = dir.join("out.log");
    let err_log = dir.join("err.log");
    if !out_log.exists() && !err_log.exists() {
        println!("no logs yet at {}", dir.display());
        return Ok(());
    }
    let existing: Vec<&PathBuf> = [&out_log, &err_log].iter().filter(|p| p.exists()).copied().collect();
    for path in &existing {
        println!("== {} ==", path.display());
        print!("{}", fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?);
    }
    if follow && !existing.is_empty() {
        let mut args = vec!["-f".to_string()];
        args.extend(existing.iter().map(|p| p.display().to_string()));
        let mut tail = Command::new("/usr/bin/tail")
            .args(&args)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        tail.wait()?;
    }
    Ok(())
}

/// `dream url` — reprint the token URL.
pub fn url(cli: &Cli) -> anyhow::Result<()> {
    let cfg = Config::load(cli)?;
    match read_token(&cfg) {
        Some(token) => {
            println!("http://{}:{}/?token={token}", admin_host_display(&cfg), cfg.port);
            Ok(())
        }
        None => Err(anyhow!(
            "no token file at {} — is the daemon running?",
            cfg.data_dir.join("token").display()
        )),
    }
}

/// `dream ps` — list sessions via the local API using the token file.
pub fn ps(cli: &Cli) -> anyhow::Result<()> {
    let cfg = Config::load(cli)?;
    let token = read_token(&cfg)
        .ok_or_else(|| anyhow!("no token file — is the daemon running? (dream status)"))?;
    let body = http_get(&cfg, "/api/sessions", &token)?;
    let sessions: Vec<serde_json::Value> = serde_json::from_str(&body)
        .context("unexpected response from /api/sessions")?;
    if sessions.is_empty() {
        println!("no sessions");
        return Ok(());
    }
    println!("{:<38} {:<24} {:<10} cmd", "id", "name", "status");
    for s in &sessions {
        let id = s["id"].as_str().unwrap_or("?");
        let short = &id[..id.len().min(8)];
        let clean = |v: &str| -> String { v.chars().filter(|c| !c.is_control()).collect() };
        let name = clean(s["name"].as_str().unwrap_or("?"));
        let status = if s["resurrected"].as_bool() == Some(true) {
            "resurrected".to_string()
        } else {
            clean(s["status"].as_str().unwrap_or("?"))
        };
        let cmd = clean(s["cmd"].as_str().unwrap_or("?"));
        println!("{short:<38} {name:<24.24} {status:<10} {cmd:.40}");
    }
    Ok(())
}

/// Minimal HTTP/1.1 GET against the loopback daemon (no new dep).
fn http_get(cfg: &Config, path: &str, token: &str) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect((admin_host(cfg).as_str(), cfg.port))
        .context("cannot connect — is the daemon running? (dream status)")?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n", admin_host_display(cfg), cfg.port
    )?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some((h, b)) => (h, b.to_string()),
        None => (text.as_ref(), String::new()),
    };
    if head.to_lowercase().contains("transfer-encoding: chunked") {
        return Err(anyhow!("chunked response not supported by the CLI client"));
    }
    if status != 200 {
        return Err(anyhow!("GET {path} → HTTP {status}: {body:.200}"));
    }
    Ok(body)
}

/// `dream doctor` — diagnostic checklist.
pub fn doctor(cli: &Cli) -> anyhow::Result<()> {
    let cfg = Config::load(cli)?;
    let mut checks: Vec<(bool, String)> = Vec::new();

    let agent_state = match platform() {
        Err(err) => {
            checks.push((false, format!("service platform unsupported: {err}")));
            return finish_doctor(checks);
        }
        Ok(Platform::Macos) => {
            let plist = plist_path()?;
            checks.push((plist.exists(), format!("plist {}", plist.display())));
            Command::new("/bin/launchctl")
                .args(["print", &service()?])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .find_map(|l| l.trim().strip_prefix("state ="))
                        .map(|s| s.trim().to_string())
                })
        }
        Ok(Platform::Linux) => {
            let unit = systemd_unit_path()?;
            checks.push((unit.exists(), format!("unit {}", unit.display())));
            systemd_out(&["is-active", "dream.service"]).map(|o| {
                if o.status.success() { "active".to_string() }
                else { String::from_utf8_lossy(&o.stdout).trim().to_string() }
            })
        }
    };
    let running = matches!(agent_state.as_deref(), Some("running") | Some("active"));
    checks.push((running, format!("service state = {}", agent_state.unwrap_or_else(|| "not running".into()))));

    let port_ok = TcpStream::connect((admin_host(&cfg).as_str(), cfg.port)).is_ok();
    checks.push((port_ok, format!("port {} reachable", cfg.port)));

    let token_file = cfg.data_dir.join("token");
    let (tok_ok, tok_msg) = match fs::metadata(&token_file) {
        Ok(m) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = m.permissions().mode() & 0o777;
                (m.is_file() && mode == 0o600, format!("token file {} mode {:03o}", token_file.display(), mode))
            }
            #[cfg(not(unix))]
            {
                (m.is_file(), format!("token file {}", token_file.display()))
            }
        }
        Err(_) => (false, format!("token file {} missing", token_file.display())),
    };
    checks.push((tok_ok, tok_msg));

    // launchd-style minimal PATH — probe well-known install locations too.
    const TS_BINS: &[&str] = &[
        "tailscale",
        "/opt/homebrew/bin/tailscale",
        "/usr/local/bin/tailscale",
        "/usr/bin/tailscale",
    ];
    let ts = TS_BINS
        .iter()
        .find_map(|bin| Command::new(bin).arg("status").output().ok());
    match ts {
        Some(o) if o.status.success() => checks.push((true, "tailscale connected".into())),
        Some(_) => checks.push((false, "tailscale installed but not connected".into())),
        None => checks.push((false, "tailscale not installed".into())),
    }

    finish_doctor(checks)
}

fn finish_doctor(checks: Vec<(bool, String)>) -> anyhow::Result<()> {
    let mut fails = 0;
    for (ok, msg) in &checks {
        println!("{} {}", if *ok { "✓" } else { "✗" }, msg);
        if !ok {
            fails += 1;
        }
    }
    println!("{fails} problem(s)");
    if fails > 0 {
        return Err(anyhow!("{fails} check(s) failed"));
    }
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

    #[test]
    fn systemd_unit_escapes_percent_and_quotes() {
        let xml = systemd_unit(&["/opt/dream".into(), "--data-dir".into(), "/home/x%h/data".into()]).unwrap();
        assert!(xml.contains("ExecStart=/opt/dream --data-dir '/home/x%%h/data'"));
        assert!(xml.contains("Restart=always"));
        assert!(xml.contains("WantedBy=default.target"));
    }

    #[test]
    fn systemd_unit_rejects_control_chars() {
        assert!(systemd_unit(&["ok".into(), "evil\nExecStartPost=/bin/sh".into()]).is_err());
    }

    #[test]
    fn shell_quote_plain_vs_special() {
        assert_eq!(shell_quote("/a/b-c_d"), "/a/b-c_d");
        assert_eq!(shell_quote("a b"), "'a b'");
    }

    #[test]
    fn service_uses_domain_service_single_arg() {
        let s = service().unwrap();
        assert!(s.starts_with("gui/"));
        assert!(s.ends_with("/com.dream.daemon"));
        assert!(!s.contains(' '));
    }
}
