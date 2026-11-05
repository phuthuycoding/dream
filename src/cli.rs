use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "webterm", version, about = "Single-binary web terminal — sessions survive the browser")]
pub struct Cli {
    /// Port to listen on
    #[arg(long, default_value_t = 4020)]
    pub port: u16,

    /// Address to bind (default 127.0.0.1; non-loopback requires --public)
    #[arg(long)]
    pub bind: Option<String>,

    /// Directory holding session state and logs
    #[arg(long)]
    pub data_dir: Option<String>,

    /// Auth token; prefer the WEBTERM_AUTH_TOKEN env var — a flag value
    /// is visible in the process list. When absent, one is generated and
    /// stored in the data dir
    #[arg(long)]
    pub auth_token: Option<String>,

    /// Allow binding a non-loopback interface (prints a security warning;
    /// traffic is cleartext)
    #[arg(long)]
    pub public: bool,

    /// Per-session replay log size cap in bytes; oldest events are trimmed
    /// once a log grows past this
    #[arg(long, value_name = "BYTES")]
    pub log_cap: Option<u64>,

    /// Max replay log age in seconds; older logs are reset to a fresh header
    #[arg(long, value_name = "SECONDS")]
    pub log_max_age: Option<u64>,
}

impl Cli {
    pub fn parse() -> Self {
        let mut cli = <Self as Parser>::parse();
        // A `--auth-token` value leaks via `ps`; the env var is the
        // preferred channel and only fills in when the flag is absent.
        if cli.auth_token.is_none() {
            if let Ok(token) = std::env::var("WEBTERM_AUTH_TOKEN") {
                if !token.is_empty() {
                    cli.auth_token = Some(token);
                }
            }
        }
        cli
    }
}
