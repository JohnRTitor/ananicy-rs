use {
    crate::cli::Args,
    ananicy_core::{
        config::{Config, ConfigDiagnostic, ConfigSnapshot, LogLevel},
        rules::Rules,
    },
    std::{collections::HashMap, env::var, sync::Arc},
    tracing::{Level, error, info, warn},
    tracing_subscriber::{filter::LevelFilter, util::SubscriberInitExt},
};

pub(crate) type LogReloadHandle = tracing_subscriber::reload::Handle<
    tracing_subscriber::filter::LevelFilter,
    tracing_subscriber::Registry,
>;

pub(crate) fn log_level_override(verbose: bool, force_trace: bool) -> Option<Level> {
    if force_trace {
        Some(Level::TRACE)
    } else if verbose {
        Some(Level::DEBUG)
    } else {
        None
    }
}

pub(crate) fn effective_log_level(
    config_level: &LogLevel,
    verbose: bool,
    force_trace: bool,
) -> Level {
    log_level_override(verbose, force_trace).unwrap_or_else(|| tracing::Level::from(config_level))
}

pub(crate) fn init_logging(
    config_level: LogLevel,
    verbose: bool,
    force_trace: bool,
    is_systemd: bool,
) -> Result<LogReloadHandle, String> {
    let log_level = effective_log_level(&config_level, verbose, force_trace);

    let (filter, reload_handle) =
        tracing_subscriber::reload::Layer::new(LevelFilter::from_level(log_level));

    #[cfg(feature = "systemd")]
    if is_systemd && let Ok(layer) = tracing_journald::layer() {
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber = tracing_subscriber::Registry::default()
            .with(filter)
            .with(layer);
        subscriber.try_init().map_err(|error| error.to_string())?;
        return Ok(reload_handle);
    }

    use tracing_subscriber::layer::SubscriberExt;
    let fmt_layer = tracing_subscriber::fmt::layer();
    let subscriber = tracing_subscriber::Registry::default()
        .with(filter)
        .with(fmt_layer);
    subscriber.try_init().map_err(|error| error.to_string())?;

    Ok(reload_handle)
}

pub(crate) fn set_log_level(handle: &LogReloadHandle, level: Level) -> Result<(), String> {
    handle
        .modify(|filter| *filter = LevelFilter::from_level(level))
        .map_err(|error| error.to_string())
}

pub(crate) fn resolve_config_paths(args: &Args) -> (String, String) {
    let config_path = var("ANANICY_RS_CONF").unwrap_or_else(|_| {
        args.config
            .clone()
            .unwrap_or_else(|| "/etc/ananicy.d/ananicy.conf".to_string())
    });
    let config_dir_path = var("ANANICY_RS_CONFDIR").unwrap_or_else(|_| {
        args.config_dir
            .clone()
            .unwrap_or_else(|| "/etc/ananicy.d".to_string())
    });
    (config_path, config_dir_path)
}

pub(crate) fn load_config(
    config_path: &str,
) -> (Arc<Config>, Option<String>, Vec<ConfigDiagnostic>, bool) {
    let latnice_supported = ananicy_platform::test_latnice_support();
    match Config::load_file_with_diagnostics(config_path, latnice_supported) {
        Ok((config, diagnostics)) => (Arc::new(config), None, diagnostics, latnice_supported),
        Err(e) => {
            let mut snapshot = ConfigSnapshot::default();
            if !latnice_supported {
                snapshot.apply_latnice = false;
            }
            (
                Arc::new(Config::new(snapshot)),
                Some(format!(
                    "Failed to load config from {}: {}. Using default.",
                    config_path, e
                )),
                Vec::new(),
                latnice_supported,
            )
        }
    }
}

pub(crate) fn log_config(
    config: &Arc<Config>,
    err: Option<String>,
    diagnostics: &[ConfigDiagnostic],
    latnice_supported: bool,
) {
    for diagnostic in diagnostics {
        diagnostic.emit();
    }
    if let Some(e) = err {
        error!("{}", e);
        if !latnice_supported {
            warn!("latency_nice is not supported by the kernel, disabling it");
        }
    } else {
        let snap = config.get();
        info!("Config apply_nice: {}", snap.apply_nice);
        info!("Config apply_sched: {}", snap.apply_sched);
        info!("Config apply_ionice: {}", snap.apply_ionice);
        info!("Config apply_ioclass: {}", snap.apply_ioclass);
        info!("Config apply_cgroup: {}", snap.apply_cgroups);
        info!("Config cgroup_load: {}", snap.cgroup_load);
        info!("Config apply_oom_score_adj: {}", snap.apply_oom_score_adj);
        info!("Config apply_latnice: {}", snap.apply_latnice);
        info!("Config log_applied_rule: {}", snap.log_applied_rule);
        info!("Config type_load: {}", snap.type_load);
        info!("Config rule_load: {}", snap.rule_load);
        info!(
            "Config cgroup_realtime_workaround: {}",
            snap.cgroup_realtime_workaround
        );
        info!("Config check_freq: {}", snap.check_freq);
        info!("Config apply_cpuset: {}", snap.apply_cpuset);
        info!("Config x3d_mode: {}", snap.x3d_mode);
        info!("Config loglevel: {}", snap.loglevel);
    }
}

pub(crate) fn load_topology_aliases(
    config: &Arc<Config>,
) -> (
    HashMap<String, String>,
    Option<ananicy_platform::x3d::X3DMode>,
) {
    let top = ananicy_platform::topology::detect_topology();
    info!("topology: {}", top.summary());
    if top.has_big_little {
        info!("Performance cores: {}", top.big_cores_str);
        info!("Efficiency cores: {}", top.little_cores_str);
        if !top.turbo_cores_str.is_empty() && top.turbo_cores_str != top.all_cores_str {
            info!("Turbo cores: {}", top.turbo_cores_str);
        }
    }
    let mut aliases = top.generate_cpuset_aliases();
    let mut saved_x3d_mode = None;

    if let Some(x3d_top) = ananicy_platform::x3d::detect_x3d_topology() {
        info!(
            "AMD X3D detected: cache cores={}, frequency cores={}",
            x3d_top.cache_cores_str, x3d_top.frequency_cores_str
        );
        // X3D topology overrides the generic largest-LLC alias because the V-Cache CCD
        // is identified from X3D-specific die topology.
        aliases.insert("x3d-cache".to_string(), x3d_top.cache_cores_str);
        aliases.insert("x3d-frequency".to_string(), x3d_top.frequency_cores_str);

        let x3d_mode_str = config.get().x3d_mode.clone();
        if x3d_mode_str != "auto" {
            saved_x3d_mode = ananicy_platform::x3d::get_driver_mode();
            if saved_x3d_mode.is_some() {
                use ananicy_platform::x3d::X3DMode;
                let target = if x3d_mode_str == "cache" {
                    X3DMode::Cache
                } else {
                    X3DMode::Frequency
                };
                if ananicy_platform::x3d::set_driver_mode(target) {
                    info!("Set X3D mode to '{}'", x3d_mode_str);
                } else {
                    warn!("Failed to set X3D mode to '{}'", x3d_mode_str);
                    saved_x3d_mode = None;
                }
            } else {
                info!("X3D driver not present, x3d_mode config ignored");
            }
        }
    }

    (aliases, saved_x3d_mode)
}

pub(crate) fn load_rules(config: Arc<Config>, config_dir_path: &str) -> Rules {
    let mut rules = Rules::new(config);
    rules.load_directory(config_dir_path);
    info!(
        "Loaded {} rules, {} types and {} cgroups from {}",
        rules.size(),
        rules.get_types().len(),
        rules.get_cgroups().len(),
        config_dir_path
    );
    rules
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        tracing::{Event, Subscriber, info},
        tracing_subscriber::{
            layer::{Context, Layer},
            prelude::*,
        },
    };

    struct EventCount(Arc<AtomicUsize>);

    impl<S> Layer<S> for EventCount
    where
        S: Subscriber,
    {
        fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, S>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn effective_level_prefers_cli_overrides() {
        assert_eq!(
            effective_log_level(&LogLevel::Error, false, false),
            Level::ERROR
        );
        assert_eq!(
            effective_log_level(&LogLevel::Error, true, false),
            Level::DEBUG
        );
        assert_eq!(
            effective_log_level(&LogLevel::Error, false, true),
            Level::TRACE
        );
    }

    #[test]
    fn log_level_reload_changes_the_active_filter() {
        let (filter, handle) = tracing_subscriber::reload::Layer::new(LevelFilter::ERROR);
        let count = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(EventCount(count.clone()));

        tracing::subscriber::with_default(subscriber, || {
            info!("before reload");
            assert_eq!(count.load(Ordering::SeqCst), 0);
            set_log_level(&handle, Level::INFO).unwrap();
            info!("after reload");
        });

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn critical_uses_error_filter_level() {
        assert_eq!(Level::from(&LogLevel::Critical), Level::ERROR);
    }
}
