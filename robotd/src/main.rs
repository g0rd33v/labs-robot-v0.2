//! robotd: the one signed binary (arch sec 2). Boots the Robot, prints the
//! Tier-3 slug URL, serves the built-in Chat.

mod boot;
mod config;
mod robot;

use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let config_path = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("robot.toml"));

    let cfg = config::load(&config_path)?;
    let booted = boot::bootstrap(&cfg)?;

    tracing::info!(
        "robot '{}' is up; data in {}",
        cfg.robot.name,
        cfg.robot.data_dir
    );
    println!("\n  your robot is live. open this on this machine:\n\n  {}\n", booted.slug_url);

    surfaces::serve(booted.state, booted.addr, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}
