use ananicy_core::{config::Config, process::Process, rules::SharedRules, spawn_named_thread};

use {
    std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    tracing::{error, info},
};

/// Everything a `SIGUSR1` needs, and the reload itself.
///
/// Grouped because it is one thing: the configuration and the rules are reloaded
/// together, from paths the process was started with, into handles it already
/// holds. Passing them as a set rather than as six arguments also means the reload
/// is a named operation with somewhere to be tested from.
pub(crate) struct Reload {
    config: Arc<Config>,
    config_path: String,
    config_dir_path: String,
    rules: SharedRules,
    log_reload_handle: crate::startup::LogReloadHandle,
    log_level_override: Option<tracing::Level>,
}

impl Reload {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: Arc<Config>,
        config_path: String,
        config_dir_path: String,
        rules: SharedRules,
        log_reload_handle: crate::startup::LogReloadHandle,
        log_level_override: Option<tracing::Level>,
    ) -> Self {
        Self {
            config,
            config_path,
            config_dir_path,
            rules,
            log_reload_handle,
            log_level_override,
        }
    }

    /// Re-reads the configuration, the log level, and the rule files.
    ///
    /// The configuration is handled first and independently: a file that fails to
    /// parse keeps the values already in force rather than resetting them, which
    /// is the documented behaviour and is why the rules are not gated on it
    /// succeeding. `rule_load = false` *is* honoured as a deliberate choice, and
    /// leaves the loaded rules alone.
    fn apply(&self) {
        info!("Received SIGUSR1, reloading config and rules...");
        let latnice_supported = ananicy_platform::test_latnice_support();
        match self
            .config
            .reload_file_with_diagnostics(&self.config_path, latnice_supported)
        {
            Ok(diagnostics) => {
                let new_config_level = self.config.get().loglevel.clone();
                let new_level = self
                    .log_level_override
                    .unwrap_or_else(|| tracing::Level::from(&new_config_level));
                let update_result =
                    crate::startup::set_log_level(&self.log_reload_handle, new_level);
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

        if !self.config.get().rule_load {
            info!("rule_load is false; keeping the rules already loaded");
            return;
        }

        let previous = self.rules.get();
        let fresh = crate::startup::load_rules(self.config.clone(), &self.config_dir_path);
        let (count, types, cgroups) = (
            fresh.size(),
            fresh.get_types().len(),
            fresh.get_cgroups().len(),
        );
        self.rules.replace(fresh);

        // A cgroup named by a newly added rule has no directory until one is
        // made, and a restart would have made it. Doing the same here is what
        // makes a reload equivalent to the restart it stands in for, rather than
        // a partial one.
        let installed = self.rules.get();
        crate::runtime::create_cgroups(&installed);

        info!(
            "Rules reloaded from {}: {} rules, {} types, {} cgroups (was {} rules, {} types, \
             {} cgroups)",
            self.config_dir_path,
            count,
            types,
            cgroups,
            previous.size(),
            previous.get_types().len(),
            previous.get_cgroups().len(),
        );
    }
}

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
    reload: Reload,
    is_systemd: bool,
    shutdown_flag: Arc<AtomicBool>,
    tx: Sender<Process>,
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
                    reload.apply();
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
    })?;

    Ok(())
}
