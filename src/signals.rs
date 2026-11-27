use ananicy_core::{config::Config, process::Process, rules::SharedRules, spawn_named_thread};

use {
    std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    tracing::{debug, error, info},
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
        // Captured before the swap: the old rule set is what tells us which
        // names the operator removed. Reconciliation itself diffs the registry
        // against the names still active after the swap.
        let previous_names: Vec<String> = previous
            .get_cgroups()
            .keys()
            .map(|name| name.0.to_string())
            .collect();
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
        let installed_names: Vec<String> = installed
            .get_cgroups()
            .keys()
            .map(|name| name.0.to_string())
            .collect();
        crate::runtime::reconcile_cgroups(&installed_names);

        let removed_names: Vec<&String> = previous_names
            .iter()
            .filter(|name| !installed_names.contains(name))
            .collect();
        info!(
            "Rules reloaded from {}: {} rules, {} types, {} cgroups (was {} rules, {} types, \
             {} cgroups); {} cgroup rules removed",
            self.config_dir_path,
            count,
            types,
            cgroups,
            previous.size(),
            previous.get_types().len(),
            previous.get_cgroups().len(),
            removed_names.len(),
        );
        if !removed_names.is_empty() {
            debug!("Cgroup rules removed on reload: {removed_names:?}");
        }
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
                    // Clean up before the channel closes or the process exits,
                    // while the manager still knows which cgroups this run
                    // created. This is bounded and never exits the process:
                    // one cgroup failing to be removed must not skip the X3D
                    // restore or the STOPPING notification below.
                    crate::runtime::cleanup_cgroups_on_shutdown();
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

#[cfg(test)]
mod reload_wiring_tests {
    use {
        super::*,
        ananicy_core::{config::ConfigSnapshot, rules::Rules},
        std::{fmt, fs, sync::Mutex},
        tracing::{Event, Subscriber, field::Field, field::Visit},
        tracing_subscriber::{
            filter::LevelFilter,
            layer::{Context, Layer},
            prelude::*,
        },
    };

    /// The smallest event recorder that can answer "did this caller reach the
    /// platform?" without injecting into `runtime` or touching the machine's
    /// cgroups: the two platform callers each log a fixed line on the way out.
    #[derive(Default)]
    struct Messages(Arc<Mutex<Vec<String>>>);

    #[derive(Default)]
    struct MessageVisitor(String);

    impl Visit for MessageVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }

    impl<S: Subscriber> Layer<S> for Messages {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = MessageVisitor::default();
            event.record(&mut visitor);
            self.0
                .lock()
                .expect("the recorder is not poisoned")
                .push(visitor.0);
        }
    }

    /// `SIGUSR1` reload must reach both platform calls after swapping in the
    /// fresh rule set. `rule_load = false` is the documented exception: the
    /// config is still reloaded, but neither caller runs.
    // Other tests in this binary also swap `tracing` subscribers and reload
    // log levels. `with_default` is thread-local, but a config reload still
    // modifies the shared reload layer, so serialize these scenarios.
    static LOG_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_reload_reaches_both_platform_callers_and_rule_load_false_skips_them() {
        let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let reconcile_before = crate::runtime::reconcile_calls();
        let temp = tempfile::tempdir().expect("a temporary config and rule directory");
        let config_path = temp.path().join("ananicy.conf");
        let rules_dir = temp.path().join("rules");
        fs::create_dir(&rules_dir).expect("a rules directory");
        fs::write(
            &config_path,
            "rule_load=true\ncgroup_load=true\nloglevel=info\n",
        )
        .expect("a config file");
        fs::write(
            rules_dir.join("probe.rules"),
            "{\"name\":\"reload-wiring-probe\",\"nice\":1}\n",
        )
        .expect("a rule file");

        let config = Arc::new(Config::new(ConfigSnapshot::default()));
        let rules = SharedRules::new(Rules::new(config.clone()));
        // A permissive filter: `Reload::apply` rewrites the active level from
        // the config, and another test's config may leave it above `info`.
        // The recorder itself is thread-local to `with_default`, so widening
        // the filter cannot pick up events from other tests.
        let (filter, handle) = tracing_subscriber::reload::Layer::new(LevelFilter::TRACE);
        let reload = Reload::new(
            config,
            config_path.to_string_lossy().into_owned(),
            rules_dir.to_string_lossy().into_owned(),
            rules.clone(),
            handle,
            None,
        );

        let messages = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(Messages(messages.clone()));
        tracing::subscriber::with_default(subscriber, || {
            reload.apply();

            let seen = messages
                .lock()
                .expect("the recorder is not poisoned")
                .clone();
            assert!(
                seen.iter()
                    .any(|line| line.contains("Finished creating cgroups")),
                "the reload must call the cgroup creation path"
            );
            assert_eq!(
                crate::runtime::reconcile_calls(),
                reconcile_before + 1,
                "the reload must call the cgroup reconciliation path with the new rules"
            );
            assert!(
                rules.get().get_rule("reload-wiring-probe").is_some(),
                "the freshly loaded rule set must have been installed"
            );

            messages
                .lock()
                .expect("the recorder is not poisoned")
                .clear();
            fs::write(
                rules_dir.join("skipped.rules"),
                "{\"name\":\"reload-wiring-skipped\",\"nice\":2}\n",
            )
            .expect("a second rule file");
            fs::write(&config_path, "rule_load=false\nloglevel=info\n").expect("a new config file");
            reload.apply();

            assert!(
                !reload.config.get().rule_load,
                "the new config must be in force after the reload"
            );
            let seen = messages
                .lock()
                .expect("the recorder is not poisoned")
                .clone();
            assert!(
                !seen
                    .iter()
                    .any(|line| line.contains("Finished creating cgroups")),
                "rule_load=false must skip cgroup creation"
            );
            assert_eq!(
                crate::runtime::reconcile_calls(),
                reconcile_before + 1,
                "rule_load=false must skip cgroup reconciliation too"
            );
            assert!(
                rules.get().get_rule("reload-wiring-skipped").is_none(),
                "rule_load=false must leave the rules already installed alone"
            );
        });
    }
}
