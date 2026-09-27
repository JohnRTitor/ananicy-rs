use ananicy_core::{config::Config, process::Process, spawn_named_thread};

use {
    std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    tracing::{error, info},
};

/// Installs the signal handlers, or reports why it could not.
///
/// Failing to install them is not something to shrug at. These three signals
/// are the daemon's only control channel: `SIGUSR1` is what `--reload` sends,
/// and `SIGINT`/`SIGTERM` are how it is stopped. The default disposition of all
/// three is to terminate the process, so a daemon that started without these
/// handlers would answer `systemctl reload` by dying, and would skip the
/// graceful shutdown that saves the X3D mode and sends `STOPPING=1`.
///
/// `Signals::new` fails when the signal mask cannot be set up, which is a
/// process or resource limit rather than anything an operator can retry by
/// waiting, so the caller is expected to refuse to start.
pub(crate) fn install(
    config: Arc<Config>,
    config_path: String,
    is_systemd: bool,
    shutdown_flag: Arc<AtomicBool>,
    tx: Sender<Process>,
    log_reload_handle: crate::startup::LogReloadHandle,
    log_level_override: Option<tracing::Level>,
) -> std::io::Result<()> {
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGUSR1,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])?;

    spawn_named_thread!("ananicy-signal", move || {
        for sig in signals.forever() {
            match sig {
                signal_hook::consts::SIGUSR1 => {
                    info!("Received SIGUSR1, reloading config...");
                    let latnice_supported = ananicy_platform::test_latnice_support();
                    match config.reload_file_with_diagnostics(&config_path, latnice_supported) {
                        Ok(diagnostics) => {
                            let new_config_level = config.get().loglevel.clone();
                            let new_level = log_level_override
                                .unwrap_or_else(|| tracing::Level::from(&new_config_level));
                            let update_result =
                                crate::startup::set_log_level(&log_reload_handle, new_level);
                            for diagnostic in &diagnostics {
                                diagnostic.emit();
                            }
                            if let Err(e) = update_result {
                                error!("Failed to update log level after config reload: {}", e);
                            } else {
                                info!("Config and log level reloaded (now {})", new_config_level);
                            }
                        }
                        Err(e) => error!("Failed to reload config: {}", e),
                    }
                }
                signal_hook::consts::SIGINT | signal_hook::consts::SIGTERM => {
                    info!("Received termination signal. Shutting down...");
                    shutdown_flag.store(true, Ordering::SeqCst);
                    drop(tx);
                    #[cfg(feature = "systemd")]
                    if is_systemd {
                        let _ = sd_notify::notify(&[sd_notify::NotifyState::Stopping]);
                    }
                    break;
                }
                _ => {}
            }
        }
    });

    Ok(())
}
