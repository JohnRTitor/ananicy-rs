#![allow(clippy::collapsible_if)]
use {
    crate::{config::Config, cpuset::CpuSet, process::Process, rules::Rules},
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
    #[error("Unsupported")]
    Unsupported,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl PlatformError {
    /// Whether this error should skip the current attribute and allow the rest of the rule to proceed,
    /// or abort the entire rule application for this process.
    pub fn is_skippable(&self) -> bool {
        matches!(self, PlatformError::PermissionDenied)
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
    fn process_cgroup(&self, _pid: i32) -> Option<crate::cgroup::CgroupIdentity> {
        None
    }

    /// Return the maximum number of configured CPU cores
    fn get_max_cores(&self) -> u32;

    fn set_priority(&self, pid: i32, nice: i32) -> Result<(), PlatformError>;
    fn set_latency_nice(&self, pid: i32, lat_nice: i32) -> Result<(), PlatformError>;
    fn set_sched(&self, pid: i32, sched: &str, rtprio: u32) -> Result<(), PlatformError>;
    fn set_io_priority(&self, pid: i32, ioclass: &str, ionice: i32) -> Result<(), PlatformError>;
    fn set_oom_score_adj(&self, pid: i32, oom_score_adj: i32) -> Result<(), PlatformError>;
    fn add_pid_to_cgroup(&self, pid: i32, cgroup: &str) -> Result<(), PlatformError>;
    fn set_affinity(&self, pid: i32, cpuset: &CpuSet) -> Result<(), PlatformError>;
}

pub struct Worker {
    config: Arc<Config>,
    rules: Arc<std::sync::RwLock<Rules>>,
    platform: Arc<dyn PlatformActions>,
    cpuset_aliases: HashMap<String, String>,
    receiver: std::sync::mpsc::Receiver<Process>,
    benchmark_count: Option<u32>,
    shutdown_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl Worker {
    pub fn new(
        config: Arc<Config>,
        rules: Arc<std::sync::RwLock<Rules>>,
        platform: Arc<dyn PlatformActions>,
        cpuset_aliases: HashMap<String, String>,
        receiver: std::sync::mpsc::Receiver<Process>,
        benchmark_count: Option<u32>,
        shutdown_flag: Arc<std::sync::atomic::AtomicBool>,
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
    pub fn start(self) -> JoinHandle<(usize, std::time::Duration)> {
        crate::spawn_named_thread!("ananicy-worker", move || self.work_loop())
    }

    pub fn work_loop(self) -> (usize, std::time::Duration) {
        let is_affected_by_cgroup_bug = self.platform.is_cgroup_v2();
        let start_time = std::time::Instant::now();
        let mut processed_count = 0;

        while let Ok(p) = self.receiver.recv() {
            processed_count += 1;

            if let Some(limit) = self.benchmark_count {
                if processed_count >= limit as usize {
                    self.shutdown_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }

            if p.identity.pid.0 == 0 {
                // Ignore PID 0 (swapper/idle thread) to prevent warning floods
                continue;
            }

            let mut p = p;
            if p.name.is_empty() || p.name == "<unknown>" || p.name.len() >= 15 {
                // If it came from BPF, it might be truncated (15 chars) or empty.
                // We resolve the full command name in the worker thread to avoid
                // blocking the high-throughput BPF polling loop with slow file I/O.
                let full_name = self.platform.get_process_name(p.identity.pid.0);
                if !full_name.is_empty() && full_name != "<unknown>" {
                    p.name = full_name;
                }
            }

            let rules = match self.rules.read() {
                Ok(rules) => rules,
                Err(_) => {
                    error!("Rules lock is poisoned; stopping worker loop");
                    break;
                }
            };
            let rule = rules.get_rule(&p.name);
            let is_realtime = self.platform.is_realtime(p.identity.pid.0);

            if let Some(rule) = rule {
                let cfg = self.config.get();
                let do_log_applied_rule = cfg.log_applied_rule; // In Debug builds we would override this

                if tracing::enabled!(tracing::Level::DEBUG) {
                    debug!("Found rule for {}: {}", p.name, rule.to_string());
                } else if do_log_applied_rule {
                    info!("{}({})", p.name, p.identity.pid.0);
                }

                if let Err(e) =
                    self.apply_rule(&p, &rule, &cfg, is_realtime, is_affected_by_cgroup_bug)
                {
                    error!(
                        "Failed to apply rule for {}({}): {}",
                        p.name, p.identity.pid.0, e
                    );
                    continue;
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

    #[tracing::instrument(skip(self, p, rule, cfg), fields(pid = p.identity.pid.0, name = %p.name))]
    fn apply_rule(
        &self,
        p: &Process,
        rule: &Value,
        cfg: &crate::config::ConfigSnapshot,
        is_realtime: bool,
        is_affected_by_cgroup_bug: bool,
    ) -> Result<(), PlatformError> {
        if cfg.apply_nice
            && let Some(nice) = rule.get("nice").and_then(|v| v.as_i64())
        {
            debug!(
                "Setting priority of {}({}) to {}",
                p.name, p.identity.pid.0, nice
            );
            if let Err(e) = self.platform.set_priority(p.identity.pid.0, nice as i32) {
                if !e.is_skippable() {
                    return Err(e);
                }
            }
        }

        if cfg.apply_latnice {
            let mut latnice_val = None;
            if let Some(l) = rule.get("latency_nice").and_then(|v| v.as_i64()) {
                latnice_val = Some(l as i32);
            } else if let Some(n) = rule.get("nice").and_then(|v| v.as_i64()) {
                latnice_val = Some(n as i32);
            }

            if let Some(latnice) = latnice_val {
                debug!(
                    "Setting latency nice of {}({}) to {}",
                    p.name, p.identity.pid.0, latnice
                );
                if let Err(e) = self.platform.set_latency_nice(p.identity.pid.0, latnice) {
                    if !e.is_skippable() {
                        return Err(e);
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
            if let Err(e) = self.platform.set_sched(p.identity.pid.0, sched, rtprio) {
                if !e.is_skippable() {
                    return Err(e);
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
            if let Err(e) = self.platform.set_io_priority(p.identity.pid.0, ioclass, ionice) {
                if !e.is_skippable() {
                    return Err(e);
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
            if let Err(e) = self.platform.set_oom_score_adj(p.identity.pid.0, oom_adj as i32) {
                if !e.is_skippable() {
                    return Err(e);
                }
            }
        }

        if is_realtime && cfg.cgroup_realtime_workaround && is_affected_by_cgroup_bug {
            debug!(
                "Cgroups are not compatible with realtime scheduling for now (linux limitation)"
            );
        } else if cfg.apply_cgroups
            && let Some(cgroup) = rule.get("cgroup").and_then(|v| v.as_str())
        {
            debug!(
                "Adding process {}({}) to cgroup {}",
                p.name, p.identity.pid.0, cgroup
            );
            if let Err(e) = self.platform.add_pid_to_cgroup(p.identity.pid.0, cgroup) {
                if !e.is_skippable() {
                    return Err(e);
                }
            }
        }

        if cfg.apply_cpuset {
            if let Some(raw_cpuset) = rule.get("cpuset").and_then(|v| v.as_str()) {
                let mut cpuset_str = raw_cpuset;

                if let Some(resolved) = self.cpuset_aliases.get(raw_cpuset) {
                    if resolved.is_empty() {
                        debug!(
                            "cpuset alias '{}' resolved to empty set, skipping for {}",
                            raw_cpuset, p.name
                        );
                        return Ok(());
                    }
                    cpuset_str = resolved.as_str();
                }

                debug!(
                    "Setting cpuset of {}({}) to {}",
                    p.name, p.identity.pid.0, cpuset_str
                );
                match crate::cpuset::CpuSet::parse(cpuset_str, self.platform.get_max_cores()) {
                    Some(parsed_set) => {
                        if let Err(e) = self.platform.set_affinity(p.identity.pid.0, &parsed_set) {
                            if !e.is_skippable() {
                                return Err(e);
                            }
                        }
                    }
                    None => {
                        warn!("Invalid cpuset string '{}' for {}", cpuset_str, p.name);
                    }
                }
            }
        }

        Ok(())
    }
}
