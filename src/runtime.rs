use {
    ananicy_core::spawn_named_thread,
    ananicy_platform::{
        LinuxPlatform,
        cgroups::{CgroupApplyStatus, CgroupSettings, CpuQuota},
        procfs::ProcfsScanner,
        x3d::X3DMode,
    },
    std::{
        sync::{atomic::Ordering::SeqCst, mpsc::Sender},
        thread::{sleep, spawn},
    },
    tracing::{debug, error, info, warn},
};

use {
    crate::monitor,
    ananicy_core::{
        config::Config,
        process::Process,
        rules::{Rules, SharedRules},
        worker::Worker,
    },
    std::{
        collections::HashMap,
        process::exit,
        sync::{Arc, atomic::AtomicBool, mpsc::Receiver},
        thread,
        time::{Duration, Instant},
    },
};

/// The channel the event sources push processes into and the worker takes them
/// out of, together with the flag that tells every part to stop.
pub(crate) struct ProcessEvents {
    pub rx: Receiver<Process>,
    pub tx: Sender<Process>,
    pub shutdown: Arc<AtomicBool>,
}

/// How the daemon was asked to run, as decided from the command line and the
/// configuration before it starts.
pub(crate) struct RunOptions {
    pub manual_scanning: bool,
    pub cgroup_realtime_workaround: bool,
    pub bpf_min_us: Option<u32>,
    pub is_systemd: bool,
    pub saved_x3d_mode: Option<X3DMode>,
    pub benchmark: bool,
    pub benchmark_count: Option<u32>,
    pub verbose: bool,
}

pub(crate) fn run(
    config: Arc<Config>,
    rules: SharedRules,
    platform: Arc<LinuxPlatform>,
    aliases: HashMap<String, String>,
    events: ProcessEvents,
    options: RunOptions,
) {
    let ProcessEvents {
        rx,
        tx,
        shutdown: shutdown_flag,
    } = events;
    let RunOptions {
        manual_scanning,
        cgroup_realtime_workaround,
        bpf_min_us,
        is_systemd,
        saved_x3d_mode,
        benchmark,
        benchmark_count,
        verbose,
    } = options;

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
    // A cgroup hierarchy is not required for the daemon to be useful. Without
    // one, a rule's `cgroup` attribute is the only thing that cannot be
    // honoured; `nice`, `ioclass`, `sched`, `oom_score_adj` and `cpuset` all
    // still apply, and `ananicy-cpp` makes the same choice — it reports that
    // cgroups are unavailable and carries on. Stopping here instead would mean a
    // daemon that exits 0 having applied nothing at all, which a `Type=simple`
    // systemd unit reports as success, and which leaves an `x3d_mode` change
    // made at start-up in place because the restore lives on the shutdown path.
    if wait_for_cgroup_hierarchy() {
        create_cgroups(&rules.get());
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
    let worker_handle = match worker.start() {
        Ok(handle) => handle,
        Err(e) => {
            error!("{e}");
            error!(
                "The worker thread is the daemon's only source of work. This is \
                 almost always a thread limit: check the process thread count \
                 against threads-max, and any pids.max on the daemon's cgroup."
            );
            exit(1);
        }
    };

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
        // Asked non-blocking: the bounded wait above has already had its turn,
        // and a host that had no hierarchy a moment ago is not going to grow one
        // while the daemon was sleeping.
        if ananicy_platform::cgroups::has_cgroup_hierarchy() {
            create_cgroups(&rules.get());
        }
    }

    if manual_scanning
        && let Err(e) = start_manual_scanner(config.clone(), tx.clone(), shutdown_flag.clone())
    {
        error!("{e}");
        error!("--manual-scanning has no event source without that thread.");
        exit(1);
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
///
/// The answer decides whether the rule cgroups are created. It deliberately does
/// not decide whether the daemon runs: see the call site.
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
            .and_then(|value| match u32::try_from(value) {
                Ok(value) => Some(value),
                Err(_) => {
                    warn!(
                        "Cgroup rule: {key} = {value} is above u32::MAX; ignoring it rather than \
                         truncating it modulo 2^32"
                    );
                    None
                }
            })
    };

    let cpu_quota = number("CPUQuota").and_then(|quota| match quota {
        // Zero is almost certainly a typo: on cgroup v2 it would starve the
        // cgroup, and ananicy-cpp inherited the footgun. Leave the field unset
        // so the kernel default (`max`/`-1`) stays in place and no control file
        // is written at all.
        0 => {
            warn!(
                "Cgroup rule: CPUQuota = 0 would give the cgroup no CPU time; ignoring it \
                 and leaving the kernel default (unlimited) in place"
            );
            None
        }
        // Above 100 the value is a percentage nobody can ask for; clamp rather
        // than silently giving the cgroup all the CPUs the rule did not ask for.
        quota if quota > 100 => {
            warn!("Cgroup rule: CPUQuota = {quota} is above 100%; clamping to 100%");
            Some(CpuQuota::Percentage(100))
        }
        quota => Some(CpuQuota::Percentage(quota)),
    });

    CgroupSettings {
        cpu_quota,
        cpu_weight: number("CPUWeight"),
    }
}

/// Creates every cgroup a `.cgroups` rule asks for.
///
/// Nothing here can fail the daemon. A rule whose cgroup cannot be created or
/// whose settings are refused is warned about once here and skipped; the point
/// of this function is that running without one of them is still better than
/// not running at all, so it deliberately has no failure path for a caller to
/// take.
pub(crate) fn create_cgroups(rules: &Arc<Rules>) {
    for (name, value) in rules.get_cgroups() {
        let settings = settings_from_rule(value);
        match ananicy_platform::cgroups::create_cgroup(&name.0, settings) {
            CgroupApplyStatus::Applied { .. } => {}
            status => warn!(
                "Cgroup rule for '{}' could not be fully applied: {status}",
                name.0
            ),
        }
    }
    info!("Finished creating cgroups");
}

/// Starts the periodic `/proc` walk that `--manual-scanning` asks for.
///
/// Without this thread there is no event source at all -- it is the whole point
/// of the mode -- so a spawn failure is reported rather than swallowed, and the
/// mode does not pretend to be running.
fn start_manual_scanner(
    config: Arc<Config>,
    tx: Sender<Process>,
    shutdown_flag: Arc<AtomicBool>,
) -> std::io::Result<()> {
    spawn_named_thread!("ananicy-scan", move || {
        // Zero is refused at parse time, so there is no sensible value to
        // substitute here: an interval of zero would mean a full `/proc` walk in
        // a tight loop.
        let check_freq = config.get().check_freq;
        let mut last_scan = Instant::now();

        while !shutdown_flag.load(SeqCst) {
            thread::sleep(Duration::from_secs(1));
            if last_scan.elapsed().as_secs() >= check_freq as u64 {
                debug!("Running periodic manual procfs scan");
                ProcfsScanner::full_scan(tx.clone());
                last_scan = Instant::now();
            }
        }
    })?;
    Ok(())
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
                cpu_quota: Some(CpuQuota::Percentage(80)),
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
                cpu_quota: Some(CpuQuota::Percentage(50)),
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

    /// P1/P2: zero and out-of-range values must not reach the kernel, and a
    /// value above 100% is clamped rather than truncated.
    #[test]
    fn a_quota_is_rejected_when_zero_or_too_large_and_clamped_above_100() {
        assert_eq!(
            settings_from_rule(&rule(r#"{"cgroup":"x","CPUQuota":0}"#)),
            CgroupSettings::default(),
            "a zero quota must leave the field unset (kernel default: max)"
        );
        assert_eq!(
            settings_from_rule(&rule(&format!(
                r#"{{"cgroup":"x","CPUQuota":{}}}"#,
                u64::from(u32::MAX) + 1
            ))),
            CgroupSettings::default(),
            "a value above u32::MAX must not be truncated to zero"
        );
        assert_eq!(
            settings_from_rule(&rule(&format!(
                r#"{{"cgroup":"x","CPUQuota":{}}}"#,
                u64::from(u32::MAX) + 50
            ))),
            CgroupSettings::default(),
            "a value above u32::MAX must not be truncated to a small percentage"
        );
        assert_eq!(
            settings_from_rule(&rule(r#"{"cgroup":"x","CPUQuota":100}"#)),
            CgroupSettings {
                cpu_quota: Some(CpuQuota::Percentage(100)),
                ..Default::default()
            },
            "100% must be preserved exactly"
        );
        assert_eq!(
            settings_from_rule(&rule(r#"{"cgroup":"x","CPUQuota":101}"#)),
            CgroupSettings {
                cpu_quota: Some(CpuQuota::Percentage(100)),
                ..Default::default()
            },
            "above 100% is clamped"
        );
        assert_eq!(
            settings_from_rule(&rule(&format!(
                r#"{{"cgroup":"x","CPUWeight":{}}}"#,
                u64::from(u32::MAX) + 1
            ))),
            CgroupSettings::default(),
            "an out-of-range weight must be ignored rather than truncated"
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
