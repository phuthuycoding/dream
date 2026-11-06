use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use dream::{
    auth::{oauth::GithubOauth, AuthToken},
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
                .unwrap_or_else(|_| "dream=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match &cli.action {
        Some(dream::cli::Action::Up) => return dream::daemon::up(&cli),
        Some(dream::cli::Action::Down) => return dream::daemon::down(),
        Some(dream::cli::Action::Status) => return dream::daemon::status(&cli),
        Some(dream::cli::Action::Restart) => return dream::daemon::restart(&cli),
        Some(dream::cli::Action::Logs { follow }) => return dream::daemon::logs(*follow),
        Some(dream::cli::Action::Url) => return dream::daemon::url(&cli),
        Some(dream::cli::Action::Ps) => return dream::daemon::ps(&cli),
        Some(dream::cli::Action::Doctor) => return dream::daemon::doctor(&cli),
        None => {}
    }
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
        println!("{}", dream::http::tailscale::tailscale_guidance(cfg.port));
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
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            println!("dream ready — open this URL once to authenticate:");
            println!("  http://{addr}/?token={}", auth.token);
        } else {
            // Under launchd/systemd stdout lands in a log other users can
            // read — point at the 0600 token file instead of printing it.
            println!("dream ready — token file at {}", cfg.data_dir.join("token").display());
        }
    } else {
        println!("dream ready — token file at {}", cfg.data_dir.display());
    }

    // Operator-configured GitHub OAuth (flags/env) becomes the
    // endpoint-complete client the routes run the login flow through.
    let oauth = cfg
        .github_oauth
        .clone()
        .map(|config| Arc::new(GithubOauth::from(config)));
    let app = build_router_with(
        manager,
        cfg.data_dir.clone(),
        auth.token,
        oauth,
        addr.clone(),
    )?;
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("cannot bind {addr} — port already in use?"))?;
    tracing::info!("dream listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
