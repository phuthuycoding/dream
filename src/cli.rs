use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "dream", version, about = "Single-binary web terminal — sessions survive the browser")]
pub struct Cli {
    /// Lifecycle command; when omitted, runs the daemon in the foreground
    #[command(subcommand)]
    pub action: Option<Action>,


    /// Port to listen on
    #[arg(long, global = true, default_value_t = 4020)]
    pub port: u16,

    /// Address to bind (default 127.0.0.1; non-loopback requires --public)
    #[arg(long, global = true)]
    pub bind: Option<String>,

    /// Directory holding session state and logs
    #[arg(long, global = true)]
    pub data_dir: Option<String>,

    /// Auth token; prefer the DREAM_AUTH_TOKEN env var — a flag value
    /// is visible in the process list. When absent, one is generated and
    /// stored in the data dir
    #[arg(long, global = true)]
    pub auth_token: Option<String>,

    /// Allow binding a non-loopback interface (prints a security warning;
    /// traffic is cleartext)
    #[arg(long, global = true)]
    pub public: bool,

    /// Per-session replay log size cap in bytes; oldest events are trimmed
    /// once a log grows past this
    #[arg(long, global = true, value_name = "BYTES")]
    pub log_cap: Option<u64>,

    /// Max replay log age in seconds; older logs are reset to a fresh header
    #[arg(long, global = true, value_name = "SECONDS")]
    pub log_max_age: Option<u64>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Action {
    /// Install and start the daemon as a macOS launchd agent (survives
    /// logout and restarts; the same port/data-dir flags apply)
    Up,
    /// Stop the launchd agent and remove its plist
    Down,
    /// Show whether the launchd agent is running and print the URL
    Status,
    /// Restart the launchd agent
    Restart,
    /// Show the daemon's launchd logs
    Logs {
        /// Follow the log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
    },
    /// Print the authenticated URL from the token file
    Url,
    /// List sessions via the local daemon API
    Ps,
    /// Diagnose the service: agent state, port, token perms, tailscale
    Doctor,
}

impl Cli {
    pub fn parse() -> Self {
        let mut cli = <Self as Parser>::parse();
        // A `--auth-token` value leaks via `ps`; the env var is the
        // preferred channel and only fills in when the flag is absent.
        if cli.auth_token.is_none() {
            if let Ok(token) = std::env::var("DREAM_AUTH_TOKEN") {
                if !token.is_empty() {
                    cli.auth_token = Some(token);
                }
            }
        }
        cli
    }
}
