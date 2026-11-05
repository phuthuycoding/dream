use std::path::PathBuf;

use crate::cli::Cli;

#[derive(Debug, Clone)]
#[allow(dead_code)] // fields are consumed by later tasks (auth, http, state)
pub struct Config {
    pub port: u16,
    pub bind: String,
    pub data_dir: PathBuf,
    pub auth_token: Option<String>,
    pub public: bool,
}

impl Config {
    pub fn from_cli(cli: &Cli) -> anyhow::Result<Self> {
        let bind = cli.bind.clone().unwrap_or_else(|| "127.0.0.1".to_string());
        let public = cli.public || !is_loopback(&bind);
        let data_dir = cli
            .data_dir
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        std::fs::create_dir_all(&data_dir)?;
        Ok(Self {
            port: cli.port,
            bind,
            data_dir,
            auth_token: cli.auth_token.clone(),
            public,
        })
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.bind, self.port)
    }
}

fn is_loopback(bind: &str) -> bool {
    matches!(bind, "127.0.0.1" | "::1" | "localhost")
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("webterm")
}
