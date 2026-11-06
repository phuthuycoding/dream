//! `dream up|down|status|restart|logs|url|ps|doctor` — service lifecycle
//! (launchd on macOS, systemd --user on Linux) plus local admin.

use std::fs;
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
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

#[cfg(test)]
fn systemd_unit(program_args: &[String], exe_sha: &str) -> anyhow::Result<String> {
    systemd_unit_with_env(program_args, &[], exe_sha)
}

/// Same, plus one `Environment=` line per (name, value) pair — how the
/// oauth config reaches the service. A `--github-client-secret` ExecStart
/// arg would sit in `ps` output instead.
fn systemd_unit_with_env(
    program_args: &[String],
    env: &[(String, String)],
    exe_sha: &str,
) -> anyhow::Result<String> {
    for a in program_args
        .iter()
        .map(String::as_str)
        .chain(env.iter().flat_map(|(k, v)| [k.as_str(), v.as_str()]))
    {
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
    let svc_path = format!("{SERVICE_PATH}:%h/.local/bin");
    let mut env_lines = format!("Environment=PATH={svc_path}\n");
    for (name, value) in env {
        // Unlike Exec lines, Environment= expands % specifiers (%% escapes)
        // and C-style escapes inside quotes, but leaves $ literal.
        let v = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        env_lines.push_str(&format!("Environment=\"{name}={v}\"\n"));
    }
    Ok(format!(
        "# dream-exe-sha256: {exe_sha}\n\
         [Unit]\nDescription=dream daemon\nAfter=network-online.target\n\n\
         [Service]\n{env_lines}ExecStart={exec}\nRestart=always\nRestartSec=3\n\n\
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

/// PATH the service's PTY children inherit — launchd/systemd start with a
/// minimal PATH that misses /opt/homebrew/bin where the AI CLIs live.
const SERVICE_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// SERVICE_PATH plus the user's own ~/.local/bin — launchd does not expand
/// `~`, so the entry is expanded at plist-write time.
fn service_path_expanded() -> String {
    match home() {
        Ok(h) => format!("{SERVICE_PATH}:{}/.local/bin", h.display()),
        Err(_) => SERVICE_PATH.to_string(),
    }
}

#[cfg(test)]
fn plist_xml(program_args: &[String], log_out: &str, log_err: &str, exe_sha: &str) -> String {
    plist_xml_with_env(program_args, log_out, log_err, exe_sha, &[])
}

/// Same, plus EnvironmentVariables entries for each (name, value) pair —
/// the launchd twin of the systemd env lines, keeping the oauth secret
/// out of `ps`.
fn plist_xml_with_env(
    program_args: &[String],
    log_out: &str,
    log_err: &str,
    exe_sha: &str,
    env: &[(String, String)],
) -> String {
    let args: String = program_args
        .iter()
        .map(|a| format!("    <string>{}</string>", xml_escape(a)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut env_entries = String::new();
    for (name, value) in env {
        env_entries.push_str(&format!(
            "    <key>{}</key>\n    <string>{}</string>\n",
            xml_escape(name),
            xml_escape(value)
        ));
    }
    format!(
        r#"<!-- dream-exe-sha256: {exe_sha} -->
<?xml version="1.0" encoding="UTF-8"?>
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
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{svc_path}</string>
{env_entries}  </dict>
</dict>
</plist>
"#,
        out = xml_escape(log_out),
        err = xml_escape(log_err),
        svc_path = xml_escape(&service_path_expanded()),
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

/// Host the daemon is reachable on for printed URLs and admin connections —
/// loopback by default; for `0.0.0.0`/`::` binds loopback is still the
/// reachable local admin address.
fn admin_host(cfg: &Config) -> String {
    if cfg.bind.is_unspecified() { "127.0.0.1".to_string() } else { cfg.bind.to_string() }
}

/// admin_host formatted for URLs/Host headers — IPv6 needs brackets.
fn try_connect(cfg: &Config) -> bool {
    use std::net::ToSocketAddrs;
    let addr = match (admin_host(cfg).as_str(), cfg.port).to_socket_addrs().ok().and_then(|mut i| i.next()) {
        Some(a) => a,
        None => return false,
    };
    TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok()
}

fn admin_host_display(cfg: &Config) -> String {
    let h = admin_host(cfg);
    if h.contains(':') { format!("[{h}]") } else { h }
}

fn print_url(cfg: &Config) {
    match read_service_token(cfg) {
        Some(token) => println!("  http://{}:{}/?token={token}", admin_host_display(cfg), cfg.port),
        None => println!("  (no token file at {} yet)", cfg.data_dir.join("token").display()),
    }
}

/// True when the service manager still has the unit loaded — in ANY state
/// (running, spawn-scheduled, crashed pending respawn). Used by `down` to
/// decide whether the unit file is safe to delete.
fn service_loaded(p: Platform) -> bool {
    match p {
        Platform::Macos => Command::new("/bin/launchctl")
            .args(["print", &format!("gui/{}/{}", uid().unwrap_or_default(), LABEL)])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        // `systemctl status` exits 4 only when the unit is not loaded at
        // all — any other result (running, inactive, failed) counts loaded.
        Platform::Linux => systemd_out(&["status", "dream.service"])
            .map(|o| o.status.code() != Some(4))
            .unwrap_or(false),
    }
}

fn service_active(p: Platform) -> bool {
    match p {
        Platform::Macos => Command::new("/bin/launchctl")
            .args(["print", &format!("gui/{}/{}", uid().unwrap_or_default(), LABEL)])
            .output()
            .map(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout).contains("state = running")
            })
            .unwrap_or(false),
        Platform::Linux => systemd_out(&["is-active", "dream.service"])
            .map(|o| o.status.success())
            .unwrap_or(false),
    }
}

/// sha256 of a file via the platform tool — used to pin which binary the
/// installed unit actually runs, so `up` can tell "same flags" from
/// "same flags + rebuilt binary" without trusting mtimes.
fn file_sha256(path: &str) -> Option<String> {
    for bin in ["/usr/bin/shasum", "/usr/bin/sha256sum", "shasum", "sha256sum"] {
        let args: &[&str] = if bin.contains("shasum") { &["-a", "256"] } else { &[] };
        if let Ok(out) = Command::new(bin).args(args).arg(path).output() {
            if out.status.success() {
                return Some(
                    String::from_utf8_lossy(&out.stdout)
                        .split_whitespace()
                        .next()?
                        .to_string(),
                );
            }
        }
    }
    None
}

/// The unit records `# dream-exe-sha256: <hex>` — skipping a reinstall is
/// only safe when the recorded hash equals the running binary's.
fn unit_matches_binary(unit_text: &str, exe: &str) -> bool {
    let want = file_sha256(exe);
    unit_text
        .lines()
        .find_map(|l| l.strip_prefix("# dream-exe-sha256: "))
        .map(|recorded| Some(recorded.trim().to_string()) == want)
        .unwrap_or(false)
}

/// Write a service unit file owner-only — the plist/unit can carry the
/// oauth client secret in its env lines, so `0600` must apply at create
/// (a `fs::write` + `chmod` pair leaves a world-readable window), and is
/// forced again afterwards because `mode()` only applies on creation.
/// Mirrors `write_key_file` in auth/session.rs.
fn write_private_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("cannot fsync {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot restrict {}", path.display()))?;
    }
    Ok(())
}

/// `dream up` — install + bootstrap the agent; idempotent.
pub fn up(cli: &Cli) -> anyhow::Result<()> {
    let platform = platform()?;
    let exe = std::env::current_exe().context("cannot resolve own binary path")?;
    let mut cfg = Config::load(cli)?;
    // The service starts with cwd=/ — a relative data dir would resolve to a
    // different location under the agent than the CLI just created.
    cfg.data_dir = fs::canonicalize(&cfg.data_dir)
        .with_context(|| format!("cannot resolve {}", cfg.data_dir.display()))?;
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

    // The resolved oauth config (flags or env — Config already merged
    // them) travels to the service as unit environment; putting the
    // client secret in the arg list would expose it via `ps`.
    let mut service_env: Vec<(String, String)> = Vec::new();
    if let Some(oauth) = &cfg.github_oauth {
        for (name, value) in [
            ("DREAM_GITHUB_CLIENT_ID", &oauth.client_id),
            ("DREAM_GITHUB_CLIENT_SECRET", &oauth.client_secret),
            ("DREAM_GITHUB_ALLOWLIST", &oauth.allowlist.join(",")),
        ] {
            if value.chars().any(|c| c.is_control()) {
                return Err(anyhow!(
                    "{name} contains a control character — refusing to write the unit"
                ));
            }
            service_env.push((name.to_string(), value.clone()));
        }
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
            let exe_sha = file_sha256(&args[0]).unwrap_or_default();
            let new_xml = plist_xml_with_env(&args, &logs.join("out.log").display().to_string(), &logs.join("err.log").display().to_string(), &exe_sha, &service_env);
            // `up` is a daily command — when the unit text did not change,
            // leave a healthy running agent alone instead of killing its ptys.
            let already = fs::read_to_string(&plist).map(|c| c == new_xml).unwrap_or(false)
                && unit_matches_binary(&new_xml, &args[0]);
            if already && service_active(Platform::Macos) {
                println!("dream is already up — nothing to do");
                print_url(&cfg);
                return Ok(());
            }
            // The plist can carry the oauth client secret in
            // EnvironmentVariables — it must be owner-only from creation.
            write_private_file(&plist, &new_xml)?;
            let svc = service()?;
            let _ = launchctl(&["bootout", &svc]);
            launchctl(&["bootstrap", &format!("gui/{}", uid()?), &plist.display().to_string()])?;
        }
        Platform::Linux => {
            let unit = systemd_unit_path()?;
            let exe_sha = file_sha256(&args[0]).unwrap_or_default();
            let new_unit = systemd_unit_with_env(&args, &service_env, &exe_sha)?;
            let already = fs::read_to_string(&unit).map(|c| c == new_unit).unwrap_or(false)
                && unit_matches_binary(&new_unit, &args[0]);
            if already && service_active(Platform::Linux) {
                println!("dream is already up — nothing to do");
                print_url(&cfg);
                return Ok(());
            }
            fs::create_dir_all(unit.parent().unwrap())?;
            // Same lockdown as the plist: the unit's Environment= lines
            // can hold the oauth client secret.
            write_private_file(&unit, &new_unit)?;
            systemctl(&["daemon-reload"])?;
            let _ = systemctl(&["disable", "--now", "dream.service"]);
            systemctl(&["enable", "--now", "dream.service"])?;
        }
    }

    // The token file persists across runs and a bare port probe can hit a
    // foreign listener — require both the service manager reporting the
    // unit active AND the port answering.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if service_active(platform) && try_connect(&cfg) {
            let mgr = match platform { Platform::Macos => format!("launchd agent {LABEL}"), Platform::Linux => "systemd --user service dream.service".into() };
            println!("dream is up ({mgr})");
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
            let hint = match platform {
                Platform::Macos => format!("check {} for the cause", log_dir()?.join("err.log").display()),
                Platform::Linux => "check `journalctl --user -u dream` for the cause".into(),
            };
            return Err(anyhow!("service installed but the port did not come up within 30s — {hint}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Unit file may be deleted only when the stop succeeded or the manager
/// says nothing is loaded — any other state keeps the file.
fn unit_removable(still_loaded: bool) -> bool {
    !still_loaded
}

/// `dream down` — stop the agent and remove the plist.
pub fn down() -> anyhow::Result<()> {
    let p = platform()?;
    let stop = match p {
        Platform::Macos => launchctl(&["bootout", &service()?]),
        Platform::Linux => systemctl(&["disable", "--now", "dream.service"]),
    };
    match stop {
        Ok(()) => {}
        Err(err) => {
            // Stop failed — only delete the unit when the manager confirms
            // it is not loaded at all; any loaded state (running, scheduled,
            // crashed-pending) means the file must stay.
            if !unit_removable(service_loaded(p)) {
                return Err(err.context("service is still loaded — not removing its unit"));
            }
            eprintln!("{err} (service not loaded)");
        }
    }
    match p {
        Platform::Macos => {
            let plist = plist_path()?;
            if plist.exists() {
                fs::remove_file(&plist)?;
            }
        }
        Platform::Linux => {
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
    if state.starts_with("not running") || state == "inactive" || state == "dead" || state == "failed" || state.starts_with("unknown") {
        println!("dream is not running ({state})");
        println!("start it: dream up");
        return Ok(());
    }
    let unit_name = match platform()? {
        Platform::Macos => LABEL,
        Platform::Linux => "dream.service",
    };
    println!("dream {unit_name}: state = {state}");
    let cfg = Config::load_admin(cli)?;
    print_url(&cfg);
    match platform()? {
        Platform::Macos => println!("  logs: {}", log_dir()?.display()),
        Platform::Linux => println!("  logs: journalctl --user -u dream  (or: dream logs)"),
    }
    Ok(())
}

/// `dream restart` — kickstart the agent.
pub fn restart(cli: &Cli) -> anyhow::Result<()> {
    match platform()? {
        Platform::Macos => launchctl(&["kickstart", "-k", &service()?])?,
        Platform::Linux => systemctl(&["restart", "dream.service"])?,
    }
    // Give the rebound port a moment to answer before reporting success.
    let cfg = Config::load_admin(cli)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let p = platform()?;
    while Instant::now() < deadline {
        if service_active(p) && try_connect(&cfg) {
            println!("dream restarted");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(anyhow!("restart issued but the port did not come back within 30s"))
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
    let existing: Vec<&PathBuf> = [&out_log, &err_log].iter().filter(|p| p.exists()).copied().collect();
    if existing.is_empty() {
        println!("no logs yet at {}", dir.display());
        return Ok(());
    }
    // Stream via tail — the launchd logs are unbounded append-only files.
    let mut args = vec!["-n".to_string(), "50".to_string()];
    if follow {
        args.push("-f".to_string());
    }
    args.extend(existing.iter().map(|p| p.display().to_string()));
    let status = Command::new("/usr/bin/tail")
        .args(&args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(anyhow!("tail exited with {status}"));
    }
    Ok(())
}

/// `dream url` — reprint the token URL.
pub fn url(cli: &Cli) -> anyhow::Result<()> {
    let cfg = Config::load_admin(cli)?;
    match read_service_token(&cfg) {
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
    let cfg = Config::load_admin(cli)?;
    let token = read_service_token(&cfg)
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
        let short: String = id.chars().take(8).collect();
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
    let addr = (admin_host(cfg).as_str(), cfg.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("cannot resolve admin host"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
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
    let cfg = Config::load_admin(cli)?;
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

    let port_ok = try_connect(&cfg);
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

    // A wedged tailscaled must not hang the checklist: one shared deadline
    // bounds every step, a failing binary moves on to the next candidate,
    // and a probe that outlives the deadline has its process group killed
    // so children of `tailscale status` cannot survive it.
    enum Probe { Started(u32), Result(bool), Done }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for bin in ["/opt/homebrew/bin/tailscale", "/usr/local/bin/tailscale", "/usr/bin/tailscale", "tailscale"] {
            let mut cmd = Command::new(bin);
            cmd.arg("status");
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            if let Ok(mut child) = cmd.spawn() {
                let _ = tx.send(Probe::Started(child.id()));
                let ok = child.wait().map(|s| s.success()).unwrap_or(false);
                let _ = tx.send(Probe::Result(ok));
                if ok {
                    return;
                }
            }
        }
        let _ = tx.send(Probe::Done);
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut probe_pid = None;
    let ts_connected = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break false;
        }
        match rx.recv_timeout(left) {
            Ok(Probe::Started(pid)) => probe_pid = Some(pid),
            Ok(Probe::Result(ok)) => {
                if ok {
                    break true;
                }
                probe_pid = None;
            }
            Ok(Probe::Done) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break false,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Some(pid) = probe_pid.take() {
                    let grp = Command::new("/bin/kill")
                        .args(["-TERM", "--", &format!("-{pid}")])
                        .output();
                    if grp.map(|o| !o.status.success()).unwrap_or(true) {
                        let _ = Command::new("/bin/kill").args(["-TERM", &pid.to_string()]).output();
                    }
                }
                break false;
            }
        }
    };
    match ts_connected {
        true => checks.push((true, "tailscale connected".into())),
        false => checks.push((true, "tailscale not connected/installed (optional — loopback-only is fine)".into())),
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
            "sha",
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
        let xml = plist_xml(&["/a&b/<x>".into()], "/o", "/e", "sha");
        assert!(xml.contains("/a&amp;b/&lt;x&gt;"));
    }

    #[test]
    fn systemd_unit_escapes_percent_and_quotes() {
        let xml = systemd_unit(&["/opt/dream".into(), "--data-dir".into(), "/home/x%h/data".into()], "sha").unwrap();
        assert!(xml.contains("ExecStart=/opt/dream --data-dir '/home/x%%h/data'"));
        assert!(xml.contains("Restart=always"));
        assert!(xml.contains("WantedBy=default.target"));
    }

    #[test]
    fn systemd_unit_rejects_control_chars() {
        assert!(systemd_unit(&["ok".into(), "evil\nExecStartPost=/bin/sh".into()], "sha").is_err());
    }

    #[test]
    fn shell_quote_plain_vs_special() {
        assert_eq!(shell_quote("/a/b-c_d"), "/a/b-c_d");
        assert_eq!(shell_quote("a b"), "'a b'");
    }

    #[test]
    fn unit_removal_decision() {
        assert!(unit_removable(false));
        assert!(!unit_removable(true));
    }

    #[test]
    fn systemd_unit_pins_env_path_and_escapes() {
        let u = systemd_unit(&["/b".into(), "/p$HOME/x".into()], "sha").unwrap();
        assert!(u.contains("Environment=PATH="));
        // `%h` is a systemd specifier — it must stay a single %, so pin the
        // full segment rather than a substring that %%h would also satisfy.
        assert!(u.contains(":%h/.local/bin"));
        assert!(!u.contains("%%h"));
        assert!(u.contains("/p$$HOME/x"));
        // A literal backslash survives as an escaped pair — separate call so
        // a validation change on it cannot mask the PATH asserts above.
        let v = systemd_unit(&["/b".into(), "a\\nb".to_string()], "sha").unwrap();
        assert!(v.contains("a\\\\nb"));
        // Real control characters are rejected outright.
        assert!(systemd_unit(&["/b".into(), "a\u{0007}b".into()], "sha").is_err());
        assert!(systemd_unit(&["/b".into(), "a\u{000a}b".into()], "sha").is_err());
    }

    #[test]
    fn systemd_unit_forwards_oauth_env_not_args() {
        let env = vec![
            ("DREAM_GITHUB_CLIENT_ID".to_string(), "id-1".to_string()),
            ("DREAM_GITHUB_CLIENT_SECRET".to_string(), "s3cret".to_string()),
            ("DREAM_GITHUB_ALLOWLIST".to_string(), "alice,org:acme".to_string()),
        ];
        let u = systemd_unit_with_env(&["/b".into()], &env, "sha").unwrap();
        assert!(u.contains("Environment=\"DREAM_GITHUB_CLIENT_ID=id-1\""));
        assert!(u.contains("Environment=\"DREAM_GITHUB_CLIENT_SECRET=s3cret\""));
        assert!(u.contains("Environment=\"DREAM_GITHUB_ALLOWLIST=alice,org:acme\""));
        // The secret rides the environment — an ExecStart arg would be
        // visible in `ps`.
        assert!(!u.lines().any(|l| l.starts_with("ExecStart=") && l.contains("s3cret")));
        // % and " inside env values are escaped for the unit parser.
        let env2 = vec![("DREAM_GITHUB_CLIENT_SECRET".to_string(), "a%b\"c".to_string())];
        let u2 = systemd_unit_with_env(&["/b".into()], &env2, "sha").unwrap();
        assert!(u2.contains("Environment=\"DREAM_GITHUB_CLIENT_SECRET=a%%b\\\"c\""));
        // Control chars in env values are rejected like args are.
        let env3 = vec![("DREAM_GITHUB_CLIENT_SECRET".to_string(), "a\nb".to_string())];
        assert!(systemd_unit_with_env(&["/b".into()], &env3, "sha").is_err());
    }

    #[test]
    fn plist_forwards_oauth_env_vars() {
        let env = vec![
            ("DREAM_GITHUB_CLIENT_SECRET".to_string(), "s3&cret".to_string()),
            ("DREAM_GITHUB_ALLOWLIST".to_string(), "alice,org:acme".to_string()),
        ];
        let xml = plist_xml_with_env(&["/b".into()], "/o", "/e", "sha", &env);
        assert!(xml.contains("<key>DREAM_GITHUB_CLIENT_SECRET</key>"));
        assert!(xml.contains("<string>s3&amp;cret</string>"));
        assert!(xml.contains("<key>DREAM_GITHUB_ALLOWLIST</key>"));
        assert!(xml.contains("<string>alice,org:acme</string>"));
        assert!(!xml.contains("--github-client-secret"));
    }

    #[test]
    fn service_uses_domain_service_single_arg() {
        let s = service().unwrap();
        assert!(s.starts_with("gui/"));
        assert!(s.ends_with("/com.dream.daemon"));
        assert!(!s.contains(' '));
    }
}
