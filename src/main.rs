use anyhow::Context;
use axum::{routing::get, Router};
use webterm::{cli::Cli, config::Config};

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

    let app = Router::new().route("/", get(|| async { "webterm" }));

    let addr = cfg.listen_addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("cannot bind {addr} — port already in use?"))?;
    tracing::info!("webterm listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
