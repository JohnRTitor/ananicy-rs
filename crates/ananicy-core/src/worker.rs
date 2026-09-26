use {
    crate::{cgroup::CgroupIdentity, config::ConfigSnapshot, cpuset::CpuSet, spawn_named_thread},
    std::{
        sync::{
            atomic::{AtomicBool, Ordering::SeqCst},
            mpsc::Receiver,
        },
        time::{Duration, Instant},
    },
};

use {
    crate::{config::Config, process::Process, rules::Rules},
    serde_json::Value,
    std::{collections::HashMap, sync::Arc, thread::JoinHandle},
    tracing::{debug, error, info, warn},
};

#[derive(thiserror::Error, Debug)]
pub enum PlatformError {
    #[error("Not found")]
    NotFound,
    #[error("Permission denied")]
    PermissionDenied,
    #[error("Skipped: {0}")]
    Skipped(String),
    #[error("Unsupported")]
    Unsupported,
    #[error("Invalid cpuset: {0}")]
    InvalidCpuset(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl PlatformError {
    /// Whether this error should skip the current attribute and allow the rest of the rule to proceed,
    /// or abort the entire rule application for this process.
    pub fn is_skippable(&self) -> bool {
        matches!(
            self,
            PlatformError::PermissionDenied | PlatformError::Skipped(_)
        )
    }
}

/// PlatformActions abstracts the Linux-specific OS operations so the core worker
/// can be unit tested without requiring a Linux kernel or root privileges.
pub trait PlatformActions: Send + Sync {
    /// Returns true if the process is currently running with a realtime scheduling policy (SCHED_FIFO, SCHED_RR)
    fn is_realtime(&self, pid: i32) -> bool;

    /// Fetches the process start time to protect against PID recycling races.
    fn get_start_time(&self, pid: i32) -> Option<u64>;

    /// Fetches the full command name of the process from the platform.
    fn get_process_name(&self, pid: i32) -> String;

    /// Returns true if the system is using cgroups v2
    fn is_cgroup_v2(&self) -> bool;

    /// Resolve a process's current cgroup for matching purposes. Read-only,
    /// always safe, no ownership implied.
    fn process_cgroup(&self, _pid: i32) -> Option<CgroupIdentity> {
        None
    }

    fn get_max_cores(&self) -> u32;

    fn get_tids(&self, pid: i32) -> Result<Vec<i32>, PlatformError> {
        Ok(vec![pid])
    }

    fn set_priority(&self, pid: i32, tids: &[i32], nice: i32) -> Result<(), PlatformError>;
    fn set_latency_nice(&self, pid: i32, tids: &[i32], lat_nice: i32) -> Result<(), PlatformError>;
    fn set_sched(&self, pid: i32, sched: &str, rtprio: u32) -> Result<(), PlatformError>;
    fn set_io_priority(&self, pid: i32, ioclass: &str, ionice: i32) -> Result<(), PlatformError>;
    fn set_oom_score_adj(&self, pid: i32, oom_score_adj: i32) -> Result<(), PlatformError>;
    fn add_pid_to_cgroup(&self, pid: i32, cgroup: &str) -> Result<(), PlatformError>;
    fn set_cpu_weight(&self, pid: i32, weight: u32) -> Result<(), PlatformError>;
    fn set_affinity(&self, pid: i32, tids: &[i32], cpuset: &CpuSet) -> Result<(), PlatformError>;
}

enum RuleApplication {
    Applied,
    NoApplicable,
    Partial(PlatformError),
}

pub struct Worker {
    config: Arc<Config>,
    rules: Arc<Rules>,
    platform: Arc<dyn PlatformActions>,
    cpuset_aliases: HashMap<String, String>,
    receiver: Receiver<Process>,
    benchmark_count: Option<u32>,
    shutdown_flag: Arc<AtomicBool>,
}

impl Worker {
    pub fn new(
        config: Arc<Config>,
        rules: Arc<Rules>,
        platform: Arc<dyn PlatformActions>,
        cpuset_aliases: HashMap<String, String>,
        receiver: Receiver<Process>,
        benchmark_count: Option<u32>,
        shutdown_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            rules,
            platform,
            cpuset_aliases,
            receiver,
            benchmark_count,
            shutdown_flag,
        }
    }

    /// Spawns a dedicated thread for the worker loop.
    pub fn start(self) -> JoinHandle<(usize, Duration)> {
        spawn_named_thread!("ananicy-worker", move || self.work_loop())
    }

    pub fn work_loop(self) -> (usize, Duration) {
        let is_affected_by_cgroup_bug = self.platform.is_cgroup_v2();
        let start_time = Instant::now();
        let mut processed_count = 0;

        while let Ok(p) = self.receiver.recv() {
            processed_count += 1;

            if let Some(limit) = self.benchmark_count
                && processed_count >= limit as usize
            {
                self.shutdown_flag.store(true, SeqCst);
            }

            if p.identity.pid.0 == 0 {
                // Ignore PID 0 (swapper/idle thread) to prevent warning floods
                continue;
            }

            let mut p = p;
            if !p.name_is_authoritative {
                let full_name = self.platform.get_process_name(p.identity.pid.0);
                if !full_name.is_empty() && full_name != "<unknown>" {
                    p.name = full_name;
                    p.name_is_authoritative = true;
                }
            }

            let mut lookup_name = p.name.as_str();

            // NixOS specific fix: Some executables are wrapped in a shell script
            // and the actual binary is renamed with a leading '.' and a trailing '-wrapped'.
            // This strips those so the correct Ananicy rules apply.
            if lookup_name.starts_with('.') {
                let is_wrapped = lookup_name.ends_with("-wrapped");
                let is_truncated_wrapped = lookup_name.len() == 15
                    && lookup_name
                        .rfind('-')
                        .is_some_and(|idx| "-wrapped".starts_with(&lookup_name[idx..]));

                if (is_wrapped || is_truncated_wrapped)
                    && let Some(end_idx) = lookup_name.rfind('-')
                    && end_idx > 1
                {
                    lookup_name = &lookup_name[1..end_idx];
                }
            }

            let rules = &self.rules;
            let rule = rules.get_rule(lookup_name);
            let is_realtime = self.platform.is_realtime(p.identity.pid.0);

            if let Some(rule) = rule {
                let cfg = self.config.get();
                let do_log_applied_rule = cfg.log_applied_rule;

                debug!(name = %p.name, pid = p.identity.pid.0, rule = ?rule, "Found rule");

                let (tids, tids_failed) = match self.platform.get_tids(p.identity.pid.0) {
                    Ok(tids) if tids.is_empty() => (vec![p.identity.pid.0], true),
                    Ok(tids) => (tids, false),
                    Err(_) => (vec![p.identity.pid.0], true),
                };

                match self.apply_rule(
                    &p,
                    &tids,
                    &rule,
                    &cfg,
                    is_realtime,
                    is_affected_by_cgroup_bug,
                ) {
                    Ok(RuleApplication::Applied) if tids_failed => {
                        warn!(
                            "Failed to enumerate threads for {}({}); rule application incomplete",
                            p.name, p.identity.pid.0
                        );
                    }
                    Ok(RuleApplication::Applied) => {
                        if do_log_applied_rule {
                            info!(
                                name = %p.name,
                                pid = p.identity.pid.0,
                                rule = ?rule,
                                "{}({})",
                                p.name,
                                p.identity.pid.0
                            );
                        }
                    }
                    Ok(RuleApplication::NoApplicable) => {
                        debug!(
                            name = %p.name,
                            pid = p.identity.pid.0,
                            "Matched rule has no enabled applicable attributes"
                        );
                    }
                    Ok(RuleApplication::Partial(e)) => {
                        if matches!(e, PlatformError::NotFound) {
                            debug!(
                                "Process {}({}) exited before rule could be applied",
                                p.name, p.identity.pid.0
                            );
                        } else {
                            warn!(
                                "Rule application partially failed for {}({}): {}",
                                p.name, p.identity.pid.0, e
                            );
                        }
                    }
                    Err(e) => {
                        if matches!(e, PlatformError::NotFound) {
                            debug!(
                                "Process {}({}) exited before rule could be applied",
                                p.name, p.identity.pid.0
                            );
                        } else {
                            error!(
                                "Failed to apply rule for {}({}): {}",
                                p.name, p.identity.pid.0, e
                            );
                        }
                    }
                }
            }

            // Realtime cgroup workaround
            let cfg = self.config.get();
            if is_realtime && cfg.cgroup_realtime_workaround && is_affected_by_cgroup_bug {
                debug!(
                    "Moving realtime process {}({}) to root cgroup",
                    p.name, p.identity.pid.0
                );
                // CRITICAL: We must use "/" instead of "" for the target cgroup here.
                // In Cgroup V2, "" is treated as a relative path and resolves to our delegated root
                // (e.g. /system.slice/ananicy-rs.service). If we used "", this workaround would
                // mistakenly hijack realtime processes (like Hyprland) into our own systemd service.
                // Using "/" explicitly targets the global root, which safely fails (due to Foreign ownership protection)
                // or succeeds if we genuinely have access, without polluting our own service cgroup.
                if let Err(e) = self.platform.add_pid_to_cgroup(p.identity.pid.0, "/") {
                    debug!(
                        ?e,
                        "Failed to add realtime process {}({}) to root cgroup",
                        p.name,
                        p.identity.pid.0
                    );
                }
            }
        }

        (processed_count, start_time.elapsed())
    }

    #[tracing::instrument(
        level = "debug",
        skip(self, p, tids, rule, cfg),
        fields(pid = p.identity.pid.0, name = %p.name)
    )]
    fn apply_rule(
        &self,
        p: &Process,
        tids: &[i32],
        rule: &Value,
        cfg: &ConfigSnapshot,
        is_realtime: bool,
        is_affected_by_cgroup_bug: bool,
    ) -> Result<RuleApplication, PlatformError> {
        let mut applied_any = false;
        let mut partial_failure = None;

        if cfg.apply_nice
            && let Some(nice) = rule.get("nice").and_then(|v| v.as_i64())
        {
            debug!(
                "Setting priority of {}({}) to {}",
                p.name, p.identity.pid.0, nice
            );
            match self
                .platform
                .set_priority(p.identity.pid.0, tids, nice as i32)
            {
                Ok(()) => applied_any = true,
                Err(e) if e.is_skippable() => {
                    partial_failure.get_or_insert(e);
                }
                Err(e) => {
                    // A failure on one attribute must not cost the rule the rest of
                    // them: the reference applies what it can and carries on, and a
                    // perfectly valid `ionice` is not worth losing because `sched`
                    // named a policy this kernel would not take.
                    partial_failure.get_or_insert(e);
                }
            }

            // On cgroup v2 a nice value is also mirrored into `cpu.weight`. The
            // write lands on the cgroup the process already belongs to, so it
            // also reweights every other task in that cgroup; `apply_cpu_weight`
            // turns it off for operators who do not want that.
            if cfg.apply_cpu_weight && self.platform.is_cgroup_v2() {
                let weight = (100.0 * 1.25f64.powi(-nice as i32)) as u32;
                let weight = weight.clamp(1, 10000);
                match self.platform.set_cpu_weight(p.identity.pid.0, weight) {
                    Ok(()) => {
                        applied_any = true;
                        debug!("Applied cpu.weight {} for {}", weight, p.name);
                    }
                    Err(e) => {
                        debug!(
                            "Skipping optional cgroup-v2 cpu.weight {} for {}: {:?}",
                            weight, p.name, e
                        );
                    }
                }
            }
        }

        if cfg.apply_latnice {
            let latnice_val = rule
                .get("latency_nice")
                .and_then(|v| v.as_i64())
                .or_else(|| rule.get("nice").and_then(|v| v.as_i64()))
                .map(|n| n as i32);

            if let Some(latnice) = latnice_val {
                debug!(
                    "Setting latency nice of {}({}) to {}",
                    p.name, p.identity.pid.0, latnice
                );
                match self
                    .platform
                    .set_latency_nice(p.identity.pid.0, tids, latnice)
                {
                    Ok(()) => applied_any = true,
                    Err(e) if e.is_skippable() => {
                        partial_failure.get_or_insert(e);
                    }
                    Err(e) => {
                        // A failure on one attribute must not cost the rule the rest of
                        // them: the reference applies what it can and carries on, and a
                        // perfectly valid `ionice` is not worth losing because `sched`
                        // named a policy this kernel would not take.
                        partial_failure.get_or_insert(e);
                    }
                }
            }
        }

        if cfg.apply_sched
            && let Some(sched) = rule.get("sched").and_then(|v| v.as_str())
        {
            let rtprio = rule.get("rtprio").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
            debug!(
                "Setting scheduler of {}({}) to {}",
                p.name, p.identity.pid.0, sched
            );
            match self.platform.set_sched(p.identity.pid.0, sched, rtprio) {
                Ok(()) => applied_any = true,
                Err(e) if e.is_skippable() => {
                    partial_failure.get_or_insert(e);
                }
                Err(e) => {
                    // A failure on one attribute must not cost the rule the rest of
                    // them: the reference applies what it can and carries on, and a
                    // perfectly valid `ionice` is not worth losing because `sched`
                    // named a policy this kernel would not take.
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if cfg.apply_ionice
            && let Some(ioclass) = rule.get("ioclass").and_then(|v| v.as_str())
        {
            let ionice = rule.get("ionice").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            debug!(
                "Setting ioclass of {}({}) to {}",
                p.name, p.identity.pid.0, ioclass
            );
            match self
                .platform
                .set_io_priority(p.identity.pid.0, ioclass, ionice)
            {
                Ok(()) => applied_any = true,
                Err(e) if e.is_skippable() => {
                    partial_failure.get_or_insert(e);
                }
                Err(e) => {
                    // A failure on one attribute must not cost the rule the rest of
                    // them: the reference applies what it can and carries on, and a
                    // perfectly valid `ionice` is not worth losing because `sched`
                    // named a policy this kernel would not take.
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if cfg.apply_oom_score_adj
            && let Some(oom_adj) = rule.get("oom_score_adj").and_then(|v| v.as_i64())
        {
            debug!(
                "Setting OOM score adjustment of {}({}) to {}",
                p.name, p.identity.pid.0, oom_adj
            );
            match self
                .platform
                .set_oom_score_adj(p.identity.pid.0, oom_adj as i32)
            {
                Ok(()) => applied_any = true,
                Err(e) if e.is_skippable() => {
                    partial_failure.get_or_insert(e);
                }
                Err(e) => {
                    // A failure on one attribute must not cost the rule the rest of
                    // them: the reference applies what it can and carries on, and a
                    // perfectly valid `ionice` is not worth losing because `sched`
                    // named a policy this kernel would not take.
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if is_realtime && cfg.cgroup_realtime_workaround && is_affected_by_cgroup_bug {
            debug!(
                "Cgroups are not compatible with realtime scheduling for now (linux limitation)"
            );
            if cfg.apply_cgroups && rule.get("cgroup").and_then(|v| v.as_str()).is_some() {
                partial_failure.get_or_insert(PlatformError::Unsupported);
            }
        } else if cfg.apply_cgroups
            && let Some(cgroup) = rule.get("cgroup").and_then(|v| v.as_str())
        {
            debug!(
                "Adding process {}({}) to cgroup {}",
                p.name, p.identity.pid.0, cgroup
            );
            match self.platform.add_pid_to_cgroup(p.identity.pid.0, cgroup) {
                Ok(()) => applied_any = true,
                Err(e) if e.is_skippable() => {
                    partial_failure.get_or_insert(e);
                }
                Err(e) => {
                    // A failure on one attribute must not cost the rule the rest of
                    // them: the reference applies what it can and carries on, and a
                    // perfectly valid `ionice` is not worth losing because `sched`
                    // named a policy this kernel would not take.
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if cfg.apply_cpuset
            && let Some(raw_cpuset) = rule.get("cpuset").and_then(|v| v.as_str())
        {
            let mut cpuset_str = raw_cpuset;
            let mut skip_cpuset = false;

            if let Some(resolved) = self.cpuset_aliases.get(raw_cpuset) {
                if resolved.is_empty() {
                    debug!(
                        "cpuset alias '{}' resolved to empty set, skipping for {}",
                        raw_cpuset, p.name
                    );
                    partial_failure.get_or_insert(PlatformError::Unsupported);
                    skip_cpuset = true;
                } else {
                    cpuset_str = resolved.as_str();
                }
            }

            if !skip_cpuset {
                debug!(
                    "Setting cpuset of {}({}) to {}",
                    p.name, p.identity.pid.0, cpuset_str
                );
                match CpuSet::parse(cpuset_str, self.platform.get_max_cores()) {
                    Some(parsed_set) => {
                        match self
                            .platform
                            .set_affinity(p.identity.pid.0, tids, &parsed_set)
                        {
                            Ok(()) => applied_any = true,
                            Err(e) if e.is_skippable() => {
                                partial_failure.get_or_insert(e);
                            }
                            Err(e) => {
                                // A failure on one attribute must not cost the rule the rest of
                                // them: the reference applies what it can and carries on, and a
                                // perfectly valid `ionice` is not worth losing because `sched`
                                // named a policy this kernel would not take.
                                partial_failure.get_or_insert(e);
                            }
                        }
                    }
                    None => {
                        partial_failure
                            .get_or_insert(PlatformError::InvalidCpuset(cpuset_str.to_string()));
                    }
                }
            }
        }

        match (applied_any, partial_failure) {
            // Nothing was applied and something failed: the rule did not take
            // effect at all, which is a failure rather than a partial one. A rule
            // whose only attribute was refused is the common case, and reporting
            // it as "partially failed" would understate it.
            (false, Some(e)) => Err(e),
            (_, Some(e)) => Ok(RuleApplication::Partial(e)),
            (true, None) => Ok(RuleApplication::Applied),
            (false, None) => Ok(RuleApplication::NoApplicable),
        }
    }
}
