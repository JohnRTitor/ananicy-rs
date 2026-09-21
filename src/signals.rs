use ananicy_core::{config::Config, process::Process, spawn_named_thread};

use {
    std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    tracing::{error, info},
};

pub(crate) fn install(
    config: Arc<Config>,
    config_path: String,
    is_systemd: bool,
    shutdown_flag: Arc<AtomicBool>,
    tx: Sender<Process>,
    log_reload_handle: crate::startup::LogReloadHandle,
) {
    let config_clone = config;
    if let Ok(mut signals) = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGUSR1,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ]) {
        spawn_named_thread!("ananicy-signal", move || {
            for sig in signals.forever() {
                if sig == signal_hook::consts::SIGUSR1 {
                    info!("Received SIGUSR1, reloading config...");
                    let latnice_supported = ananicy_platform::test_latnice_support();
                    if let Err(e) = config_clone.reload_file(&config_path, latnice_supported) {
                        error!("Failed to reload config: {}", e);
                    } else {
                        // Config reloaded successfully. Now update the active logger verbosity.
                        let new_config_level = config_clone.get().loglevel.clone();
                        let new_level = match new_config_level {
                            ananicy_core::config::LogLevel::Trace => tracing::Level::TRACE,
                            ananicy_core::config::LogLevel::Debug => tracing::Level::DEBUG,
                            ananicy_core::config::LogLevel::Info => tracing::Level::INFO,
                            ananicy_core::config::LogLevel::Warn => tracing::Level::WARN,
                            ananicy_core::config::LogLevel::Error
                            | ananicy_core::config::LogLevel::Critical => tracing::Level::ERROR,
                        };
                        let _ = log_reload_handle.modify(|filter| {
                            *filter = tracing_subscriber::filter::LevelFilter::from_level(new_level);
                        });
                        info!("Config and log level reloaded (now {})", new_config_level);
                    }
                } else if sig == signal_hook::consts::SIGINT || sig == signal_hook::consts::SIGTERM
                {
                    info!("Received termination signal. Shutting down...");
                    shutdown_flag.store(true, Ordering::SeqCst);
                    drop(tx);
                    #[cfg(feature = "systemd")]
                    if is_systemd {
                        let _ = sd_notify::notify(&[sd_notify::NotifyState::Stopping]);
                    }
                    break;
                }
            }
        });
    }
}
