use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use webterm::{
    auth::AuthToken,
    cli::Cli,
    config::Config,
    http::routes::build_router_with,
    replay::enforcer::RetentionPolicy,
    runtime::{RuntimeManager, DEFAULT_IDLE_ATTENTION},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "webterm=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = Config::load(&cli)?;
    let addr = cfg.listen_addr();

    let auth = match &cfg.auth_token {
        Some(token) => AuthToken {
            token: token.clone(),
            first_run: false,
        },
        None => AuthToken::load_or_generate(&cfg.data_dir)?,
    };

    if cfg.bind.is_loopback() {
        println!("{}", webterm::http::tailscale::tailscale_guidance(cfg.port));
    }

    // Resurrect sessions that were live when the daemon last stopped, and
    // start the retention enforcer when a policy is configured. The router
    // shares this same manager — a second one would duplicate monitors
    // and diverge attention state.
    let manager = Arc::new(
        RuntimeManager::new(cfg.data_dir.clone(), DEFAULT_IDLE_ATTENTION)
            .with_log_cap(cfg.log_cap),
    );
    match manager.resurrect_all().await {
        Ok(0) => {}
        Ok(n) => tracing::info!("resurrected {n} session(s)"),
        Err(err) => tracing::warn!(error = %err, "session resurrection failed"),
    }
    if cfg.log_cap.is_some() || cfg.log_max_age.is_some() {
        let policy = RetentionPolicy {
            max_size_bytes: cfg.log_cap,
            max_age: cfg.log_max_age,
        };
        manager.spawn_enforcer(policy, Duration::from_secs(60));
    }

    if auth.first_run {
        println!("webterm ready — open this URL once to authenticate:");
        println!("  http://{addr}/?token={}", auth.token);
    } else {
        println!("webterm ready — token file at {}", cfg.data_dir.display());
    }

    let app = build_router_with(manager, cfg.data_dir.clone(), auth.token);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("cannot bind {addr} — port already in use?"))?;
    tracing::info!("webterm listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
