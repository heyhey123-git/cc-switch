//! Linux service entry point using the existing proxy core without a GUI runtime.

use crate::{AppState, Database};
use std::sync::Arc;

struct StderrLogger;
static LOGGER: StderrLogger = StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "[{}][{}] {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

#[derive(Debug, PartialEq)]
enum Action {
    Serve,
    Init,
    Help,
    Version,
}

fn action(args: &[String]) -> Result<Action, String> {
    match args {
        [] => Ok(Action::Serve),
        [arg] if arg == "--init-only" => Ok(Action::Init),
        [arg] if arg == "--help" || arg == "-h" => Ok(Action::Help),
        [arg] if arg == "--version" => Ok(Action::Version),
        _ => Err("usage: cc-switch --headless [--init-only | --help | --version]".into()),
    }
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let action = action(&args)?;
    if action == Action::Help {
        println!(
            "cc-switch --headless [--init-only | --help | --version]\n\
            Runs the proxy without initializing GTK/WebKit or requiring a display session.\n\
            Uses this user's ~/.cc-switch configuration; listen address must be loopback.\n\
            --init-only creates/migrates the database and exits without starting a listener.\n\
            SIGINT/SIGTERM stop the listener gracefully. Client config files are not rewritten."
        );
        return Ok(());
    }
    if action == Action::Version {
        println!("CC Switch {} headless proxy", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("runtime initialization failed: {error}"))?;
    tauri::async_runtime::set(runtime.handle().clone());
    runtime.block_on(async {
        let db = Arc::new(Database::init().map_err(|error| error.to_string())?);
        if action == Action::Init {
            println!("CC Switch headless database initialized");
            return Ok(());
        }
        let config = db
            .get_listen_config()
            .await
            .map_err(|error| error.to_string())?;
        let address: std::net::IpAddr = config
            .listen_address
            .parse()
            .map_err(|_| "headless listen_address must be a loopback IP address".to_string())?;
        if !address.is_loopback() {
            return Err("headless listen_address must be loopback (127.0.0.1 or ::1)".into());
        }
        let proxy_url = db
            .get_global_proxy_url()
            .map_err(|error| error.to_string())?;
        crate::proxy::http_client::init(proxy_url.as_deref())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|error| format!("SIGTERM handler failed: {error}"))?;
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .map_err(|error| format!("SIGINT handler failed: {error}"))?;
        let state = AppState::new(db);
        let info = state.proxy_service.start().await?;
        println!("CC_SWITCH_HEADLESS_READY {}:{}", info.address, info.port);
        tokio::select! {
            _ = terminate.recv() => {},
            _ = interrupt.recv() => {},
        }
        state.proxy_service.stop().await?;
        log::info!("Headless proxy stopped");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_supported_service_actions() {
        assert_eq!(action(&[]).unwrap(), Action::Serve);
        for (arg, expected) in [
            ("--init-only", Action::Init),
            ("--help", Action::Help),
            ("--version", Action::Version),
        ] {
            assert_eq!(action(&[arg.into()]).unwrap(), expected);
        }
        assert!(action(&["--listen=0.0.0.0".into()]).is_err());
        assert!(action(&["--init-only".into(), "--help".into()]).is_err());
    }
}
