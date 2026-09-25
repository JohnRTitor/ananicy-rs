use {
    ananicy_core::spawn_named_thread,
    ananicy_platform::{
        LinuxPlatform, cgroups::CgroupSettings, procfs::ProcfsScanner, x3d::X3DMode,
    },
    std::{
        sync::{atomic::Ordering::SeqCst, mpsc::Sender},
        thread::{sleep, spawn},
    },
    tracing::{debug, info, warn},
};

use {
    crate::monitor,
    ananicy_core::{config::Config, process::Process, rules::Rules, worker::Worker},
    std::{
        collections::HashMap,
        sync::{Arc, atomic::AtomicBool, mpsc::Receiver},
        thread,
        time::{Duration, Instant},
    },
};

pub(crate) fn run(
    config: Arc<Config>,
    rules: Arc<Rules>,
    platform: Arc<LinuxPlatform>,
    aliases: HashMap<String, String>,
    rx: Receiver<Process>,
    tx: Sender<Process>,
    shutdown_flag: Arc<AtomicBool>,
    manual_scanning: bool,
    cgroup_realtime_workaround: bool,
    bpf_min_us: Option<u32>,
    is_systemd: bool,
    saved_x3d_mode: Option<X3DMode>,
    benchmark: bool,
    benchmark_count: Option<u32>,
    verbose: bool,
) {
    if benchmark {
        info!("Benchmark enabled!");
        let shutdown = shutdown_flag.clone();
        spawn(move || {
            sleep(Duration::from_secs(30));
            shutdown.store(true, SeqCst);
        });
    }
    if let Some(count) = benchmark_count {
        info!("Benchmark count: {}", count);
    }

    info!("Initializing cgroups based on rules");
    if !wait_for_cgroup_hierarchy() || !create_cgroups(&rules) {
        return;
    }

    info!("Spawning worker thread");
    let worker = Worker::new(
        config.clone(),
        rules.clone(),
        platform,
        aliases,
        rx,
        benchmark_count,
        shutdown_flag.clone(),
    );
    let worker_handle = worker.start();

    #[cfg(feature = "systemd")]
    if is_systemd {
        info!("Notifying systemd of readiness...");
        let _ = sd_notify::notify(&[sd_notify::NotifyState::Ready]);
    }

    if cgroup_realtime_workaround {
        thread::sleep(Duration::from_millis(100));
        // The whole detection is dropped, not just the mount-table cache: the
        // cgroup manager caches the hierarchy it was built from, so re-creating
        // the cgroups would keep resolving names against the old one.
        ananicy_platform::cgroups::reset_cgroup_detection();
        if !create_cgroups(&rules) {
            return;
        }
    }

    if manual_scanning {
        start_manual_scanner(config.clone(), tx.clone(), shutdown_flag.clone());
    }

    monitor::run(
        tx,
        shutdown_flag,
        worker_handle,
        saved_x3d_mode,
        bpf_min_us,
        verbose,
    );
}

/// Waits for a usable cgroup hierarchy before the first cgroup creation.
///
/// On a host that mounts its cgroup filesystems slightly after the daemon — a
/// container image without cgroupfs, or a very early boot — the first detection
/// can legitimately come back empty. Retrying for a bounded while turns that
/// into a short delay instead of a daemon that silently never applies its
/// cgroup rules. A host with no cgroup support at all is reported and given up
/// on, because waiting cannot help there.
fn wait_for_cgroup_hierarchy() -> bool {
    if ananicy_platform::cgroups::has_cgroup_hierarchy() {
        return true;
    }

    warn!("No cgroup hierarchy detected yet, waiting for it to appear...");
    if ananicy_platform::mounts::init_cgroups() {
        info!("cgroup hierarchy became available");
        true
    } else {
        warn!(
            "Still no cgroup hierarchy after {:?}; cgroup rules will not be applied",
            ananicy_platform::mounts::CGROUP_INIT_TIMEOUT
        );
        false
    }
}

/// Reads the cgroup settings a `.cgroups` rule asks for.
///
/// An attribute that is absent, or present with a value that is not a number, is
/// left unset rather than guessed at: a malformed rule must not configure a
/// cgroup with a value nobody asked for.
fn settings_from_rule(rule: &serde_json::Value) -> CgroupSettings {
    let number = |key: &str| {
        rule.get(key)
            .and_then(|value| value.as_u64())
            .map(|value| value as u32)
    };

    CgroupSettings {
        cpu_quota: number("CPUQuota"),
        cpu_weight: number("CPUWeight"),
    }
}

fn create_cgroups(rules: &Arc<Rules>) -> bool {
    for (name, value) in rules.get_cgroups() {
        ananicy_platform::cgroups::create_cgroup(&name.0, settings_from_rule(value));
    }
    info!("Finished creating cgroups");
    true
}

fn start_manual_scanner(config: Arc<Config>, tx: Sender<Process>, shutdown_flag: Arc<AtomicBool>) {
    spawn_named_thread!("ananicy-scan", move || {
        let freq = config.get().check_freq;
        let check_freq = if freq > 0 { freq } else { 60 };
        let mut last_scan = Instant::now();

        while !shutdown_flag.load(SeqCst) {
            thread::sleep(Duration::from_secs(1));
            if last_scan.elapsed().as_secs() >= check_freq as u64 {
                debug!("Running periodic manual procfs scan");
                ProcfsScanner::full_scan(tx.clone());
                last_scan = Instant::now();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("a JSON rule")
    }

    #[test]
    fn a_cgroup_rule_yields_the_settings_it_declares() {
        assert_eq!(
            settings_from_rule(&rule(r#"{"cgroup":"cpu80","CPUQuota":80}"#)),
            CgroupSettings {
                cpu_quota: Some(80),
                cpu_weight: None,
            }
        );
        assert_eq!(
            settings_from_rule(&rule(r#"{"cgroup":"light","CPUWeight":200}"#)),
            CgroupSettings {
                cpu_quota: None,
                cpu_weight: Some(200),
            }
        );
        assert_eq!(
            settings_from_rule(&rule(r#"{"cgroup":"both","CPUQuota":50,"CPUWeight":10}"#)),
            CgroupSettings {
                cpu_quota: Some(50),
                cpu_weight: Some(10),
            }
        );
    }

    #[test]
    fn a_cgroup_rule_that_declares_nothing_configures_nothing() {
        assert_eq!(
            settings_from_rule(&rule(r#"{"cgroup":"empty"}"#)),
            CgroupSettings::default()
        );
    }

    #[test]
    fn a_malformed_setting_is_ignored_rather_than_guessed() {
        // A quoted number, a negative one and a boolean are all not settings the
        // daemon can use. Configuring a cgroup with something nobody wrote down
        // would be worse than leaving it alone.
        for json in [
            r#"{"cgroup":"x","CPUQuota":"80"}"#,
            r#"{"cgroup":"x","CPUQuota":-5}"#,
            r#"{"cgroup":"x","CPUWeight":true}"#,
            r#"{"cgroup":"x","CPUWeight":null}"#,
        ] {
            assert_eq!(
                settings_from_rule(&rule(json)),
                CgroupSettings::default(),
                "{json} must not configure anything"
            );
        }
    }
}
