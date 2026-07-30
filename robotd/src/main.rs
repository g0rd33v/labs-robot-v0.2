//! robotd: the one signed binary (arch sec 2). A thin dispatcher over the
//! library -- boot the Robot and serve, or run an operational subcommand.

use robotd::cli::{self, Cmd};
use robotd::{backup, boot, config, evals, maintenance, package, scheduler};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match cli::parse(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };

    match cmd {
        Cmd::Help => {
            print!("{}", cli::USAGE);
            Ok(())
        }
        Cmd::Version => {
            println!("robotd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }

        // `restore` deliberately does NOT load a config: it is run in a
        // fresh directory, and loading would write a default robot.toml
        // into the current one as a side effect.
        Cmd::Restore {
            pkg,
            code,
            into,
            port,
            force,
        } => package::restore(&pkg, &code, &into, port, force),

        Cmd::Backup { config } => {
            let cfg = config::load(&config)?;
            let path = backup::run(&cfg)?;
            println!("backup sealed: {}", path.display());
            Ok(())
        }
        Cmd::BackupRestore {
            config,
            sealed,
            dest,
        } => {
            let cfg = config::load(&config)?;
            backup::restore(&cfg, &sealed, &dest)?;
            println!("restored into {}", dest.display());
            Ok(())
        }
        Cmd::Package { config, dest } => {
            let cfg = config::load(&config)?;
            let (path, code) = package::export(&cfg, dest)?;
            println!("robot package: {}", path.display());
            println!("one-time code: {code}");
            println!("(the code is the seal -- carry it separately from the file)");
            Ok(())
        }
        Cmd::Eval { config, live } => {
            let cfg = config::load(&config)?;
            let gateway = if live {
                Some(evals::live_gateway(&cfg)?)
            } else {
                None
            };
            let code = evals::run(live, gateway)?;
            std::process::exit(code);
        }
        Cmd::Serve { config } => serve(&config).await,
    }
}

async fn serve(config_path: &std::path::Path) -> anyhow::Result<()> {
    let cfg = config::load(config_path)?;
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
            robotd::telegram::spawn(booted.robot.clone(), tg);
        }
    }

    tracing::info!(
        "robot '{}' is up; data in {}",
        cfg.robot.name,
        cfg.robot.data_dir
    );
    println!(
        "\n  your robot is live. open this on this machine:\n\n  {}\n",
        booted.slug_url
    );

    surfaces::serve(booted.state, booted.addr, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}
