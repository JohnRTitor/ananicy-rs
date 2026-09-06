use {
    crate::cli::Args,
    ananicy_core::{
        config::{Config, ConfigSnapshot},
        rules::Rules,
    },
    std::{collections::HashMap, sync::Arc},
    tracing::{error, info, warn},
};

pub(crate) fn init_logging(verbose: bool, force_trace: bool, is_systemd: bool) {
    let log_level = if force_trace {
        tracing::Level::TRACE
    } else if verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    #[cfg(feature = "systemd")]
    if is_systemd {
        if let Ok(layer) = tracing_journald::layer() {
            use tracing_subscriber::layer::SubscriberExt;
            let subscriber = tracing_subscriber::Registry::default()
                .with(tracing_subscriber::filter::LevelFilter::from_level(
                    log_level,
                ))
                .with(layer);
            let _ = tracing::subscriber::set_global_default(subscriber);
            return;
        }
    }

    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(log_level)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

pub(crate) fn resolve_config_paths(args: &Args) -> (String, String) {
    let config_path = std::env::var("ANANICY_RS_CONF").unwrap_or_else(|_| {
        args.config
            .clone()
            .unwrap_or_else(|| "/etc/ananicy.d/ananicy.conf".to_string())
    });
    let config_dir_path = std::env::var("ANANICY_RS_CONFDIR").unwrap_or_else(|_| {
        args.config_dir
            .clone()
            .unwrap_or_else(|| "/etc/ananicy.d".to_string())
    });
    (config_path, config_dir_path)
}

pub(crate) fn load_config(config_path: &str) -> Arc<Config> {
    let latnice_supported = ananicy_platform::test_latnice_support();
    match Config::load_file(config_path, latnice_supported) {
        Ok(cfg) => {
            let snap = cfg.get();
            info!("Config apply_nice: {}", snap.apply_nice);
            info!("Config apply_sched: {}", snap.apply_sched);
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
            info!("Config apply_ionice: {}", snap.apply_ionice);
            info!("Config x3d_mode: {}", snap.x3d_mode);
            info!("Config loglevel: {}", snap.loglevel);
            Arc::new(cfg)
        }
        Err(e) => {
            error!(
                "Failed to load config from {}: {}. Using default.",
                config_path, e
            );
            let mut snapshot = ConfigSnapshot::default();
            if !latnice_supported {
                snapshot.apply_latnice = false;
                warn!("latency_nice is not supported by the kernel, disabling it");
            }
            Arc::new(Config::new(snapshot))
        }
    }
}

pub(crate) fn load_topology_aliases(
    config: &Arc<Config>,
) -> (
    HashMap<String, String>,
    Option<ananicy_platform::x3d::X3DMode>,
) {
    let top = ananicy_platform::topology::detect_topology();
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
                let target = if x3d_mode_str == "cache" {
                    ananicy_platform::x3d::X3DMode::Cache
                } else {
                    ananicy_platform::x3d::X3DMode::Frequency
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
    rules
}
