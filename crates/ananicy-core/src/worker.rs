use lru::LruCache;
use {
    crate::{
        cgroup::CgroupIdentity, config::ConfigSnapshot, cpuset::CpuSet, rules::Rule,
        spawn_named_thread,
    },
    std::{
        num::NonZeroUsize,
        sync::{
            atomic::{AtomicBool, Ordering::SeqCst},
            mpsc::Receiver,
        },
        time::{Duration, Instant},
    },
};

use {
    crate::{config::Config, process::Process, rules::SharedRules},
    smallvec::{SmallVec, smallvec},
    std::{collections::HashMap, sync::Arc, thread::JoinHandle},
    tracing::{debug, error, info, warn},
};

/// Thread ids of one process. Almost every process has only a handful, so the
/// common case stays on the stack; only unusually many-threaded processes fall
/// back to the heap. Defined once here so implementors do not have to agree on
/// an inline capacity independently.
pub type Tids = SmallVec<[i32; 8]>;

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

/// PlatformActions abstracts the Linux-specific OS operations so the core worker
/// can be unit tested without requiring a Linux kernel or root privileges.
pub trait PlatformActions: Send + Sync {
    /// Returns true if the process is currently running with a realtime scheduling policy (SCHED_FIFO, SCHED_RR)
    fn is_realtime(&self, pid: i32) -> bool;

    /// Whether the process' current cgroup, or an ancestor up to the mount
    /// root, has a bandwidth limit (`cpu.max` other than `max`).
    ///
    /// Only used for a diagnostic on realtime tasks: the kernel restriction on
    /// moving a realtime task into a bandwidth-limited cgroup is conditional on
    /// `CONFIG_RT_GROUP_SCHED=y`, so the daemon warns rather than assuming.
    fn is_in_bandwidth_limited_cgroup(&self, pid: i32) -> bool;

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

    fn get_tids(&self, pid: i32) -> Result<Tids, PlatformError> {
        Ok(smallvec![pid])
    }

    fn set_priority(&self, pid: i32, tids: &[i32], nice: i32) -> Result<(), PlatformError>;
    fn set_latency_nice(&self, pid: i32, tids: &[i32], lat_nice: i32) -> Result<(), PlatformError>;
    fn set_sched(
        &self,
        pid: i32,
        tids: &[i32],
        sched: &str,
        rtprio: u32,
    ) -> Result<(), PlatformError>;
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
    rules: SharedRules,
    platform: Arc<dyn PlatformActions>,
    cpuset_aliases: HashMap<String, String>,
    receiver: Receiver<Process>,
    benchmark_count: Option<u32>,
    shutdown_flag: Arc<AtomicBool>,
    /// Realtime tasks already warned about in this burst. The same process is
    /// reported to the worker several times in quick succession (one event per
    /// exec, and a full `/proc` scan after a netlink overrun), and the condition
    /// is a property of the cgroup rather than of any one event.
    reported_realtime_cgroups: LruCache<i32, ()>,
}

/// How many pids the "already warned" cache remembers. One warning per pid is
/// all a burst needs; the bound keeps a long-running daemon from growing it
/// without limit.
const REPORTED_REALTIME_CAPACITY: usize = 256;

impl Worker {
    pub fn new(
        config: Arc<Config>,
        rules: SharedRules,
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
            reported_realtime_cgroups: LruCache::new(
                NonZeroUsize::new(REPORTED_REALTIME_CAPACITY).unwrap_or(NonZeroUsize::MIN),
            ),
        }
    }

    /// Spawns a dedicated thread for the worker loop.
    ///
    /// This thread is the daemon's only source of work, so a failure to spawn it
    /// is reported rather than tolerated: there is nothing left to run.
    pub fn start(self) -> std::io::Result<JoinHandle<(usize, Duration)>> {
        spawn_named_thread!("ananicy-worker", move || self.work_loop())
    }

    pub fn work_loop(mut self) -> (usize, Duration) {
        let is_affected_by_cgroup_bug = self.platform.is_cgroup_v2();
        let start_time = Instant::now();
        let mut processed_count = 0;

        // Parsed cpusets keyed by the resolved cpuset string. The worker owns a
        // single thread, so a plain map is enough: a rule with a cpuset is
        // parsed once rather than once per process it matches. `get_max_cores`
        // is constant for the process, so it is not part of the key.
        let mut parsed_cpusets: HashMap<Box<str>, CpuSet> = HashMap::new();

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

            let rules = self.rules.get();
            let rule = rules.get_rule(lookup_name);
            let mut is_realtime = self.platform.is_realtime(p.identity.pid.0);

            if let Some(rule) = rule {
                let cfg = self.config.get();
                let do_log_applied_rule = cfg.log_applied_rule;

                debug!(name = %p.name, pid = p.identity.pid.0, rule = ?rule, "Found rule");

                let (tids, tids_failed) = match self.platform.get_tids(p.identity.pid.0) {
                    Ok(tids) if tids.is_empty() => (smallvec![p.identity.pid.0], true),
                    Ok(tids) => (tids, false),
                    Err(_) => (smallvec![p.identity.pid.0], true),
                };

                match self.apply_rule(
                    &p,
                    &tids,
                    rule,
                    &cfg,
                    &mut is_realtime,
                    is_affected_by_cgroup_bug,
                    &mut parsed_cpusets,
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

            // Realtime cgroup workaround.
            //
            // The reference moves a realtime process up to the hierarchy root, where
            // no bandwidth limit applies. That move is not possible here, and this
            // used to attempt it anyway: the target was the literal path "/", and the
            // ownership check classifies "/" as Foreign in every configuration --
            // a discovered delegated root is the cgroup mount point or deeper, never
            // "/", and with no root discovered the classification falls through to
            // Foreign as well. So the write was refused on every realtime process on
            // every host, at debug level, and the comment above it claimed a case
            // where it could succeed.
            //
            // What is left, and what the option actually does, is the `cgroup`
            // attribute skip in `apply_rule` below: a realtime task is left where it
            // is rather than moved into a bandwidth-limited cgroup it could not be
            // served from. That part works, so the option keeps its name.
            //
            // `is_realtime` is the value after the rule was applied, so a rule
            // that itself sets a realtime policy is covered in this same event.
            let cfg = self.config.get();
            if is_realtime && cfg.cgroup_realtime_workaround && is_affected_by_cgroup_bug {
                if self
                    .platform
                    .is_in_bandwidth_limited_cgroup(p.identity.pid.0)
                {
                    // A limit only matters when the kernel has RT group
                    // scheduling (`CONFIG_RT_GROUP_SCHED=y`); on other kernels
                    // this is a harmless diagnostic and nothing is fatal.
                    // Once per pid per burst, because every event for the same
                    // process would otherwise repeat the same warning.
                    if !self.reported_realtime_cgroups.contains(&p.identity.pid.0) {
                        self.reported_realtime_cgroups.put(p.identity.pid.0, ());
                        warn!(
                            "Realtime process {}({}) is in a bandwidth-limited cgroup; leaving \
                             it in its current cgroup. Relevant on kernels built with \
                             CONFIG_RT_GROUP_SCHED=y.",
                            p.name, p.identity.pid.0
                        );
                    }
                } else {
                    debug!(
                        "Leaving realtime process {}({}) in its current cgroup; no bandwidth \
                         limit applies",
                        p.name, p.identity.pid.0
                    );
                }
            }
        }

        (processed_count, start_time.elapsed())
    }

    #[tracing::instrument(
        level = "debug",
        skip(self, p, tids, rule, cfg, is_realtime, is_affected_by_cgroup_bug, parsed_cpusets),
        fields(pid = p.identity.pid.0, name = %p.name)
    )]
    // Private, with one caller in `work_loop`. Bundling the two booleans and the
    // cache into a context struct would add indirection for that one call.
    // `is_realtime` is an out-parameter: `set_sched` can change the policy, so
    // the sample taken before the rule is refreshed here and used by the caller
    // for the realtime diagnostic too.
    #[allow(clippy::too_many_arguments)]
    fn apply_rule(
        &mut self,
        p: &Process,
        tids: &[i32],
        rule: &Rule,
        cfg: &ConfigSnapshot,
        is_realtime: &mut bool,
        is_affected_by_cgroup_bug: bool,
        parsed_cpusets: &mut HashMap<Box<str>, CpuSet>,
    ) -> Result<RuleApplication, PlatformError> {
        let mut applied_any = false;
        // A failure on one attribute must not cost the rule the rest of them: the
        // reference applies what it can and carries on, and a perfectly valid
        // `ionice` is not worth losing because `sched` named a policy this kernel
        // would not take. Every attribute below therefore records its failure here
        // and moves on, rather than returning, and the rule is reported as
        // partially applied if anything at all got through.
        let mut partial_failure = None;

        if cfg.apply_nice
            && let Some(nice) = rule.nice()
        {
            debug!(
                "Setting priority of {}({}) to {}",
                p.name, p.identity.pid.0, nice
            );
            match self.platform.set_priority(p.identity.pid.0, tids, nice) {
                Ok(()) => applied_any = true,
                Err(e) => {
                    partial_failure.get_or_insert(e);
                }
            }
        }
        if cfg.apply_latnice
            && let Some(latnice) = rule.effective_latency_nice()
        {
            debug!(
                "Setting latency nice of {}({}) to {}",
                p.name, p.identity.pid.0, latnice
            );
            match self
                .platform
                .set_latency_nice(p.identity.pid.0, tids, latnice)
            {
                Ok(()) => applied_any = true,
                Err(e) => {
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if cfg.apply_sched
            && let Some(sched) = rule.sched()
        {
            let rtprio = rule.rtprio().unwrap_or(1);
            debug!(
                "Setting scheduler of {}({}) to {}",
                p.name, p.identity.pid.0, sched
            );
            match self
                .platform
                .set_sched(p.identity.pid.0, tids, sched, rtprio)
            {
                Ok(()) => {
                    applied_any = true;
                    // The sample taken before the rule was applied is stale for
                    // a rule that changes the policy: `fifo`/`rr` with the
                    // default `rtprio` of 1 makes the task realtime immediately,
                    // while `normal`/`batch`/`idle` puts an RT task back into
                    // the fair class. Ask again now, before the `cpu.weight`
                    // mirror and the cgroup decision below. A failed policy
                    // change cannot change the class, so there is nothing to
                    // re-check on that path.
                    *is_realtime = self.platform.is_realtime(p.identity.pid.0);
                }
                Err(e) => {
                    partial_failure.get_or_insert(e);
                }
            }
        }

        // On cgroup v2 a nice value is also mirrored into `cpu.weight`. The
        // write lands on the cgroup the process already belongs to, so it
        // also reweights every other task in that cgroup;
        // `apply_cpu_weight_from_nice` turns it off for operators who do not
        // want that. Realtime tasks are skipped as well: `cpu.weight` only
        // affects fair-class tasks, so the write cannot help the RT task and
        // can only reweight unrelated CFS siblings in the same cgroup.
        //
        // This deliberately runs after `set_sched` refreshed `is_realtime`:
        // the mirror has to decide on the task's final class, not the one it
        // had before the rule. It is best-effort, so a refused write is only
        // logged and does not count against the rule.
        //
        // `powi` is computed from a `f64` exponent rather than a negated
        // `i32`: a rule carrying `"nice": -2147483648` overflows that negation
        // in a debug build, and wraps to `i32::MIN` in a release one, which
        // `powi` then answers with an infinity that saturates to `u32::MAX`.
        // Casting the value straight to `f64` saturates at the same place the
        // clamp does, so an out-of-range nice gives the maximum weight
        // instead of panicking or wrapping.
        if cfg.apply_nice
            && let Some(nice) = rule.nice()
            && cfg.apply_cpu_weight_from_nice
            && !*is_realtime
            && self.platform.is_cgroup_v2()
        {
            let weight = (100.0 * 1.25f64.powf(-(nice as f64))) as u32;
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

        if cfg.apply_ionice
            && let Some(ioclass) = rule.ioclass()
        {
            let ionice = rule.ionice().unwrap_or(0);
            debug!(
                "Setting ioclass of {}({}) to {}",
                p.name, p.identity.pid.0, ioclass
            );
            match self
                .platform
                .set_io_priority(p.identity.pid.0, ioclass, ionice)
            {
                Ok(()) => applied_any = true,
                Err(e) => {
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if cfg.apply_oom_score_adj
            && let Some(oom_adj) = rule.oom_score_adj()
        {
            debug!(
                "Setting OOM score adjustment of {}({}) to {}",
                p.name, p.identity.pid.0, oom_adj
            );
            match self.platform.set_oom_score_adj(p.identity.pid.0, oom_adj) {
                Ok(()) => applied_any = true,
                Err(e) => {
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if *is_realtime && cfg.cgroup_realtime_workaround && is_affected_by_cgroup_bug {
            // The rule's `cgroup` is deliberately not applied here, and that is the
            // option working rather than something going wrong. This is the part of
            // it that does something: a realtime task is left where it is instead of
            // being moved into a cgroup with a bandwidth limit it could never be
            // served from, which is the case the reference's root-cgroup move exists
            // to avoid. The reference logs the same line at debug and moves on;
            // recording a failure would warn on every realtime process on every
            // cgroup-v2 host, and would suppress the applied-rule line that the
            // reference still prints.
            if cfg.apply_cgroups && rule.cgroup().is_some() {
                debug!("Skipping cgroup for realtime process {}", p.name);
            }
        } else if cfg.apply_cgroups
            && let Some(cgroup) = rule.cgroup()
        {
            debug!(
                "Adding process {}({}) to cgroup {}",
                p.name, p.identity.pid.0, cgroup
            );
            match self.platform.add_pid_to_cgroup(p.identity.pid.0, cgroup) {
                Ok(()) => applied_any = true,
                Err(e) => {
                    partial_failure.get_or_insert(e);
                }
            }
        }

        if cfg.apply_cpuset
            && let Some(raw_cpuset) = rule.cpuset()
        {
            let mut cpuset_str = raw_cpuset;
            let mut skip_cpuset = false;

            if let Some(resolved) = self.cpuset_aliases.get(raw_cpuset) {
                if resolved.is_empty() {
                    // An alias that resolves to nothing is the documented way of
                    // saying "do not touch this process' affinity" on a machine
                    // where the tier does not exist — `little-cores` on a
                    // homogeneous host, say. It is a decision, not a failure, so
                    // it is not recorded as one; the reference treats it as a
                    // plain skip and still logs the applied rule.
                    debug!(
                        "cpuset alias '{}' resolved to empty set, skipping for {}",
                        raw_cpuset, p.name
                    );
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
                // Look up by the borrowed string first — `Box<str>` borrows as
                // `str`, so a hit costs neither parsing nor an allocation. Only
                // a miss copies the key (which may borrow from `cpuset_aliases`)
                // into the map. The map belongs to this single worker thread, so
                // a borrow of its value can be passed straight to `set_affinity`.
                let parsed_set = if parsed_cpusets.contains_key(cpuset_str) {
                    parsed_cpusets.get(cpuset_str)
                } else {
                    match CpuSet::parse(cpuset_str, self.platform.get_max_cores()) {
                        Some(parsed) => {
                            parsed_cpusets.insert(cpuset_str.into(), parsed);
                            parsed_cpusets.get(cpuset_str)
                        }
                        None => None,
                    }
                };
                match parsed_set {
                    Some(parsed_set) => {
                        match self
                            .platform
                            .set_affinity(p.identity.pid.0, tids, parsed_set)
                        {
                            Ok(()) => applied_any = true,
                            Err(e) => {
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
