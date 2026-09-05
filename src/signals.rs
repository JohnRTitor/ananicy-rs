use {
    ananicy_core::config::Config,
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
    tx: Sender<ananicy_core::process::Process>,
) {
    let config_clone = config;
    if let Ok(mut signals) = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGUSR1,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ]) {
        ananicy_core::spawn_named_thread!("ananicy-signal", move || {
            for sig in signals.forever() {
                if sig == signal_hook::consts::SIGUSR1 {
                    info!("Received SIGUSR1, reloading config...");
                    let latnice_supported = ananicy_platform::test_latnice_support();
                    if let Err(e) = config_clone.reload_file(&config_path, latnice_supported) {
                        error!("Failed to reload config: {}", e);
                    }
                    info!("Config reloaded");
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
