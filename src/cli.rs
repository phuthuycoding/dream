use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "webterm", version, about = "Single-binary web terminal — sessions survive the browser")]
pub struct Cli {
    /// Port to listen on
    #[arg(long, default_value_t = 4020)]
    pub port: u16,

    /// Address to bind (default 127.0.0.1; use --public for non-loopback)
    #[arg(long)]
    pub bind: Option<String>,

    /// Directory holding session state and logs
    #[arg(long)]
    pub data_dir: Option<String>,

    /// Auth token; when absent, one is generated at first run
    #[arg(long)]
    pub auth_token: Option<String>,

    /// Bind a non-loopback interface (prints a security warning)
    #[arg(long)]
    pub public: bool,
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
