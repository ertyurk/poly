#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod types;
#[allow(dead_code)]
mod math;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("polymarket_bot=info".parse()?),
        )
        .init();

    let config = config::Config::load("config.toml")?;
    tracing::info!(mode = %config.general.mode, "loaded config");
    tracing::info!("polymarket-bot starting in {} mode", config.general.mode);

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    Ok(())
}
