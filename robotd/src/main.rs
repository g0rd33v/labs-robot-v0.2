//! robotd: the one signed binary (arch sec 2). Boots the Robot, prints the
//! Tier-3 slug URL, serves the built-in Chat.

mod backup;
mod boot;
mod config;
mod evals;
mod maintenance;
mod robot;
mod scheduler;
mod telegram;

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

    // subcommands: backup / backup-restore / eval
    match args.get(1).map(String::as_str) {
        Some("backup") => {
            let path = backup::run(&cfg)?;
            println!("backup sealed: {}", path.display());
            return Ok(());
        }
        Some("backup-restore") => {
            let sealed = args.get(2).map(PathBuf::from).ok_or_else(|| {
                anyhow::anyhow!("usage: robotd backup-restore <sealed> <dest-dir>")
            })?;
            let dest = args.get(3).map(PathBuf::from).ok_or_else(|| {
                anyhow::anyhow!("usage: robotd backup-restore <sealed> <dest-dir>")
            })?;
            backup::restore(&cfg, &sealed, &dest)?;
            println!("restored into {}", dest.display());
            return Ok(());
        }
        Some("eval") => {
            let live = args.iter().any(|a| a == "--live");
            let gateway = if live {
                match std::env::var("OPENROUTER_API_KEY") {
                    Ok(key) if !key.trim().is_empty() => Some(std::sync::Arc::new(
                        hub::ModelGateway::new(
                            std::sync::Arc::new(hub::UreqApi::new(
                                key.trim().to_string(),
                                cfg.hub.base_url.clone(),
                            )),
                            cfg.hub.cast.clone(),
                            hub::GatewayConfig::default(),
                            None, // eval calls are not this instance's traffic
                        ),
                    )),
                    _ => None,
                }
            } else {
                None
            };
            let code = evals::run(live, gateway)?;
            std::process::exit(code);
        }
        _ => {}
    }

    let booted = boot::bootstrap(&cfg)?;

    // the commitment ledger's background lane: due reminders fire
    scheduler::spawn(booted.robot.clone());
    // watchdog (60s in-without-out) + zombie sweeper (Q12)
    maintenance::spawn(booted.robot.clone());

    // the telegram surface, behind its flag: token present = on, absent = fine
    if let Ok(token) = std::env::var("TELEGRAM_BOT_TOKEN") {
        if !token.trim().is_empty() {
            let tg = std::sync::Arc::new(hub::Telegram::new(
                token.trim().to_string(),
                Some(booted.robot.core.clone()),
            ));
            telegram::spawn(booted.robot.clone(), tg);
        }
    }

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
