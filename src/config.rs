use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::bail;

use crate::cli::Cli;

#[derive(Debug, Clone)]
#[allow(dead_code)] // fields are consumed by later tasks (auth, http, state)
pub struct Config {
    pub port: u16,
    pub bind: IpAddr,
    pub data_dir: PathBuf,
    pub auth_token: Option<String>,
    pub public: bool,
    pub log_cap: Option<u64>,
    pub log_max_age: Option<Duration>,
}

impl Config {
    pub fn load(cli: &Cli) -> anyhow::Result<Self> {
        let bind: IpAddr = cli
            .bind
            .as_deref()
            .unwrap_or("127.0.0.1")
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid --bind address: {}", cli.bind.as_deref().unwrap_or("")))?;
        let public = cli.public || !bind.is_loopback();
        if !bind.is_loopback() && !cli.public {
            bail!(
                "refusing to bind non-loopback address {bind} without --public; \
                 exposing a terminal on a public interface is dangerous"
            );
        }
        if public && !bind.is_loopback() {
            eprintln!(
                "WARNING: binding {bind} exposes webterm on a non-loopback interface. \
                 Traffic is cleartext HTTP — the token and all terminal I/O \
                 are sniffable. Anyone reaching this address with the token \
                 gets a shell."
            );
        }
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
            log_cap: cli.log_cap,
            log_max_age: cli.log_max_age.map(Duration::from_secs),
        })
    }

    pub fn listen_addr(&self) -> String {
        if self.bind.is_ipv6() {
            format!("[{}]:{}", self.bind, self.port)
        } else {
            format!("{}:{}", self.bind, self.port)
        }
    }
}

pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("webterm")
}
