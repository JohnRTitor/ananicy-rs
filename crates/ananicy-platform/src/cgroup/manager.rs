use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::available_parallelism,
};

use {
    lru::LruCache,
    std::{
        fs::{self, OpenOptions},
        io::Write,
    },
    tracing::{debug, error, warn},
};

use crate::cgroup::{
    CgroupInfo, CgroupVersion,
    ownership::{CgroupOwnership, discover_delegated_root},
};

pub trait CgroupController {
    fn ensure_child(&self, name: &str) -> Option<PathBuf>;
    fn move_pid(&self, pid: i32, target: &Path) -> bool;
    fn set_cpu_max(&self, target: &Path, quota: u32) -> bool;
    fn set_cpu_weight(&self, target: &Path, weight: u32) -> bool;
}

/// `base` with each of `segments` appended, in a single allocation.
///
/// `PathBuf::join` allocates a fresh buffer for every call, so a v1 target — a
/// controller directory and then a name — was two allocations, and the same
/// shape appeared in three of the four version arms. The worker resolves a
/// target twice per process, once to move the pid and once to weight its
/// cgroup, so this is two allocations per process for a path nothing keeps.
///
/// Empty segments are skipped rather than appended, which is how an empty
/// cgroup name means "the base itself" rather than a trailing separator.
fn joined(base: &Path, segments: &[&str]) -> PathBuf {
    let appended: usize = segments
        .iter()
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.len() + 1)
        .sum();
    let mut path = PathBuf::with_capacity(base.as_os_str().len() + appended);
    path.push(base);
    for segment in segments.iter().filter(|segment| !segment.is_empty()) {
        path.push(segment);
    }
    path
}

/// The CFS period a quota is expressed against: 100 ms on cgroup v2 (the kernel's
/// own default for `cpu.max`) and 1 s on cgroup v1 (the maximum the v1 files
/// accept), matching the reference daemon.
const V2_PERIOD_US: u64 = 100_000;
const V1_PERIOD_US: u64 = 1_000_000;

/// `period * cores * quota / 100`, the bandwidth a `.cgroups` rule's `CPUQuota`
/// percentage asks for, in microseconds per period.
///
/// `CPUQuota` is a percentage *of the machine*, so it has to be multiplied out
/// against the CPU count before it means anything. That product does not fit in
/// 32 bits once the machine has a few hundred CPUs, and the arithmetic this
/// replaced was done in `u32`: at 430 cores and a 100% quota the multiplication
/// wrapped and the cgroup was silently limited to a two-thousandth of one CPU
/// instead of 430. The kernel reads the field as a `u64`, so it is computed as
/// one here and only formatted at the point of writing.
fn bandwidth_us(cores: u64, quota: u32, period: u64) -> u64 {
    period
        .saturating_mul(cores)
        .saturating_mul(u64::from(quota.min(100)))
        / 100
}

#[derive(Debug, Clone)]
pub struct CgroupManager {
    info: CgroupInfo,
    delegated_root: Option<PathBuf>,
    /// Cgroups already reported as having no bandwidth control file.
    ///
    /// Behind an `Arc` because the manager lives in a process-global and is
    /// cloned to reach it, so the memory has to be shared rather than per-clone.
    ///
    /// Bounded, because it was a `HashSet` that only ever grew: entries are
    /// inserted on a failure path and nothing removes them, so its size was
    /// whatever the daemon happened to see before it was killed. The number of
    /// distinct cgroups a host has is small — one per `.cgroups` rule, really —
    /// so a bound of this size is never reached in practice, and being wrong
    /// about that costs a repeated warning and nothing else.
    reported_weightless: Arc<Mutex<LruCache<PathBuf, ()>>>,
}

/// How many cgroups the "already reported" set remembers. Comfortably above the
/// number of cgroups a `.cgroups` rule set creates, and small enough that
/// forgetting one is a log line rather than a problem.
const REPORTED_WEIGHTLESS_CAPACITY: usize = 256;

fn reported_cache() -> LruCache<PathBuf, ()> {
    LruCache::new(NonZeroUsize::new(REPORTED_WEIGHTLESS_CAPACITY).unwrap_or(NonZeroUsize::MIN))
}

impl CgroupManager {
    pub fn new(info: CgroupInfo) -> Self {
        let delegated_root = if info.version == CgroupVersion::V2 {
            discover_delegated_root(&info.mount_point)
        } else {
            None
        };

        if info.version == CgroupVersion::V2 {
            match &delegated_root {
                Some(root) => debug!("Cgroup v2: Discovered delegated root at {:?}", root),
                None => warn!(
                    "Cgroup v2: No writable delegated root discovered. Cgroup modifications will be disabled. Please run ananicy-rs as a systemd service with `Delegate=yes`."
                ),
            }
        }

        Self {
            info,
            delegated_root,
            reported_weightless: Arc::new(Mutex::new(reported_cache())),
        }
    }

    /// Creates a CgroupManager with a specific delegated root, useful for testing.
    #[doc(hidden)]
    pub fn new_with_root(info: CgroupInfo, delegated_root: Option<PathBuf>) -> Self {
        Self {
            info,
            delegated_root,
            reported_weightless: Arc::new(Mutex::new(reported_cache())),
        }
    }

    pub fn info(&self) -> &CgroupInfo {
        &self.info
    }

    /// Reports a cgroup that has no bandwidth control file, once per cgroup.
    ///
    /// A missing `cpu.weight` is a property of the cgroup, not of the process: it
    /// means `cpu` is not in the parent cgroup's `cgroup.subtree_control`. The
    /// process's `nice` was applied either way, so nothing is wrong, but the
    /// bandwidth that `nice` implies on cgroup v2 was not. This was a `debug`
    /// line, which on a machine where no leaf has the file meant one line per
    /// process per rule and nothing at all at the default log level.
    ///
    /// Once per path rather than once per process, because the condition repeats
    /// for every process in that cgroup and saying it again adds nothing.
    fn report_weightless(&self, target: &Path, file: &Path) {
        let Ok(mut seen) = self.reported_weightless.lock() else {
            return;
        };
        // Borrowed check before the owned insert. `to_path_buf` copies the whole
        // path, and this is called for every process in a cgroup that has no
        // weight file, of which all but the first are the case where the copy is
        // thrown away again.
        if seen.contains(target) {
            return;
        }
        seen.put(target.to_path_buf(), ());
        warn!(
            "Cgroup {} has no {}: nice applied, implied cpu weight skipped. \
             Needs +cpu in the parent's cgroup.subtree_control.",
            target.display(),
            file.file_name().unwrap_or_default().to_string_lossy(),
        );
    }

    /// Helper to resolve the target directory based on the cgroup name and version.
    pub fn resolve_target_dir(&self, name: &str) -> Option<PathBuf> {
        // Prevent basic path traversal attacks
        if name.contains("..") {
            warn!(
                "Security: Rejecting cgroup name with traversal sequence '..': {}",
                name
            );
            return None;
        }

        let is_absolute = name.starts_with('/');
        let relative_name = name.strip_prefix('/').unwrap_or(name);

        match self.info.version {
            CgroupVersion::None => None,
            CgroupVersion::V1 => Some(joined(&self.info.mount_point, &["cpu", relative_name])),
            CgroupVersion::V2 => {
                // Absolute names resolve from the global mount, relative ones
                // from the delegated root, and a relative name on a host with no
                // delegated root falls back to the mount point.
                let base: &Path = if is_absolute {
                    &self.info.mount_point
                } else if let Some(root) = self.delegated_root.as_deref() {
                    root
                } else {
                    &self.info.mount_point
                };
                Some(joined(base, &[relative_name]))
            }
        }
    }

    pub fn cgroup_exists(&self, name: &str) -> bool {
        self.resolve_target_dir(name)
            .is_some_and(|target| target.exists())
    }
}

impl CgroupController for CgroupManager {
    fn ensure_child(&self, name: &str) -> Option<PathBuf> {
        let target_dir = self.resolve_target_dir(name)?;

        let ownership = CgroupOwnership::classify(
            &target_dir,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        match ownership {
            CgroupOwnership::Legacy => {
                // Legacy v1 behavior: just create the directory
                if !target_dir.exists()
                    && let Err(e) = fs::create_dir_all(&target_dir)
                {
                    error!("ensure_child(V1): Failed to create cgroup {}: {}", name, e);
                    return None;
                }
                Some(target_dir)
            }
            CgroupOwnership::Owned => {
                // Owned v2 behavior: ensure subtree control on parent before creating child
                if let Some(parent) = target_dir.parent() {
                    // Try to enable cpu controller on parent
                    let subtree_control = parent.join("cgroup.subtree_control");
                    if subtree_control.exists() {
                        match OpenOptions::new()
                            .write(true)
                            .open(&subtree_control)
                            .and_then(|mut f| f.write_all(b"+cpu\n"))
                        {
                            Ok(()) => {}
                            Err(e) => {
                                // The kernel refuses `+cpu` on a cgroup that has
                                // processes in it ("no internal process" rule,
                                // cgroup_vet_subtree_control_enable()). A delegated
                                // unit's own cgroup always has one: the service
                                // itself. Without `cpu` in the parent's
                                // subtree_control the cgroup about to be created
                                // has no `cpu.max` and no `cpu.weight`, so every
                                // `CPUQuota`/`CPUWeight` in a `.cgroups` rule and
                                // every nice-mirroring weight below it silently
                                // does nothing. That is worth one line, once, at
                                // the level an operator is reading.
                                warn!(
                                    "Could not enable the cpu controller on {}: {}. \
                                     Cgroups created under it will have no cpu.max \
                                     and no cpu.weight, so CPUQuota and CPUWeight \
                                     rules cannot be applied to them.",
                                    parent.display(),
                                    e
                                );
                            }
                        }
                    }
                }

                if !target_dir.exists()
                    && let Err(e) = fs::create_dir_all(&target_dir)
                {
                    error!("ensure_child(V2): Failed to create cgroup {}: {}", name, e);
                    return None;
                }
                Some(target_dir)
            }
            CgroupOwnership::Foreign => {
                debug!(
                    "ensure_child(V2): Path {:?} is Foreign (not in delegated root). Refusing to create.",
                    target_dir
                );
                None
            }
        }
    }

    fn move_pid(&self, pid: i32, target: &Path) -> bool {
        let ownership = CgroupOwnership::classify(
            target,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        if ownership == CgroupOwnership::Foreign {
            debug!(
                "move_pid: Target {:?} is Foreign. Refusing to write.",
                target
            );
            return false;
        }

        if !target.exists() {
            debug!("move_pid: Cgroup {:?} does not exist.", target);
            return false;
        }

        let procs_file = if self.info.version == CgroupVersion::V2 {
            "cgroup.procs"
        } else {
            "tasks"
        };
        let procs_path = target.join(procs_file);

        // Open first. The open depends on nothing but the target, so doing it
        // before the pid reads below keeps the PID-reuse window — the span
        // between the last time we look at the process and the moment the kernel
        // acts on what we read — down to a single read immediately followed by
        // the write.
        let mut file = match OpenOptions::new().write(true).open(&procs_path) {
            Ok(file) => file,
            Err(e) => {
                debug!("move_pid: Failed to open {:?}: {}", procs_path, e);
                return false;
            }
        };

        // If in CgroupV2, we must write the TGID (process ID), not the TID, to cgroup.procs
        // Writing a TID that is not a thread group leader to cgroup.procs fails with EINVAL (os error 22)
        let pid_to_write = if self.info.version == CgroupVersion::V2 {
            match crate::procfs::get_tgid(pid) {
                Some(tgid) => tgid,
                None => return false, // Process is dead, writing raw TID causes EINVAL (os error 22)
            }
        } else {
            pid
        };

        // Read last, immediately before the write, and for the narrowest window
        // the sequence allows. Note what this does and does not do:
        //
        // The write below is irreversible, and the kernel resolves the pid in it
        // with its own `find_task_by_pid`. There is no way to make it reuse-safe
        // from here — `cgroup.procs` takes a pid, not a pidfd, and no pidfd API
        // exists for cgroup membership. So the comparison *after* the write is a
        // diagnostic that reports a mis-move that has already happened, not a
        // guard that prevents one. All that is available here is to make the
        // window as small as possible, and to be honest in the message about what
        // has occurred.
        let Some(start_time_before) = crate::procfs::get_start_time(pid) else {
            return false;
        };

        // CRITICAL: Do NOT use writeln!() here. On an unbuffered File,
        // writeln!(f, "{}", pid) performs two separate write() syscalls:
        //   write(fd, "1234", 4)  → succeeds (kernel moves the PID)
        //   write(fd, "\n", 1)    → EINVAL (kernel tries to parse "\n" as a PID)
        // Instead, format the PID + newline into a single buffer so that
        // write_all() issues one atomic write() syscall.
        let pid_buf = format!("{}\n", pid_to_write);
        if let Err(e) = file.write_all(pid_buf.as_bytes()) {
            debug!("move_pid: Failed to write to {:?}: {}", procs_path, e);
            return false;
        }

        match crate::procfs::get_start_time(pid) {
            // Unchanged identity: the pid we checked is the pid the kernel moved.
            Some(after) if after == start_time_before => {}
            // The pid is gone. Either it died in the window, in which case the
            // write found nothing to move and there is nothing to undo, or it
            // died after being moved, which leaves no live process to affect.
            None => {
                debug!(
                    "move_pid: PID {} is gone after the move; nothing to attribute",
                    pid
                );
                return false;
            }
            // A different process holds the pid, so the write moved that process
            // and not the one this was checked against. The move already happened
            // and cannot be undone from here; what can be done is not pretend
            // otherwise, and report which cgroup now holds a process that did not
            // ask to be there.
            Some(after) => {
                warn!(
                    "move_pid: PID {} was reused between the check (start time {}) and the \
                     write (start time {}), so {} — not the process that was inspected — is \
                     now in {:?}. This cannot be undone; expect the new process to be \
                     mis-tuned until it exits.",
                    pid, start_time_before, after, pid_to_write, target,
                );
                return false;
            }
        }

        debug!("move_pid: Successfully added {} to {:?}", pid, target);
        true
    }

    fn set_cpu_max(&self, target: &Path, quota: u32) -> bool {
        let ownership = CgroupOwnership::classify(
            target,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        if ownership == CgroupOwnership::Foreign {
            warn!(
                "set_cpu_max: Target {:?} is Foreign. Refusing to write.",
                target
            );
            return false;
        }

        // The number of CPUs the quota is a percentage of. `available_parallelism`
        // is what the reference uses too, and it is what `sched_getaffinity`
        // reports, so it is the machine's CPU count unless the unit pins the
        // daemon — which a `.cgroups` rule cannot see and cannot correct.
        let logical_cores = available_parallelism().map(|n| n.get()).unwrap_or(1) as u64;
        let clamped_quota = quota.clamp(0, 100);

        // `period * cores * quota / 100` in `u32` wraps on any machine with more
        // than ~430 CPUs at a 100% quota: 100_000 * 430 * 100 is 4.3e9, one more
        // than `u32::MAX`. See `bandwidth_us`.
        let period: u64 = if self.info.version == CgroupVersion::V2 {
            V2_PERIOD_US
        } else {
            V1_PERIOD_US
        };
        let quota_val = bandwidth_us(logical_cores, clamped_quota, period);

        if self.info.version == CgroupVersion::V2 {
            let max_file = target.join("cpu.max");
            let Ok(mut f) = OpenOptions::new().write(true).open(&max_file) else {
                error!("set_cpu_max: Failed to open {:?}", max_file);
                return false;
            };
            let buf = format!("{quota_val} {period}\n");
            if let Err(e) = f.write_all(buf.as_bytes()) {
                error!("set_cpu_max: Failed to write {:?}: {}", max_file, e);
                return false;
            }
        } else {
            let period_file = target.join("cpu.cfs_period_us");
            let Ok(mut f) = OpenOptions::new().write(true).open(&period_file) else {
                error!("set_cpu_max: Failed to open {:?}", period_file);
                return false;
            };
            let buf = format!("{period}\n");
            if let Err(e) = f.write_all(buf.as_bytes()) {
                error!("set_cpu_max: Failed to write {:?}: {}", period_file, e);
                return false;
            }
            let quota_file = target.join("cpu.cfs_quota_us");
            let Ok(mut f) = OpenOptions::new().write(true).open(&quota_file) else {
                error!("set_cpu_max: Failed to open {:?}", quota_file);
                return false;
            };
            let buf = format!("{quota_val}\n");
            if let Err(e) = f.write_all(buf.as_bytes()) {
                error!("set_cpu_max: Failed to write {:?}: {}", quota_file, e);
                return false;
            }
        }
        true
    }

    fn set_cpu_weight(&self, target: &Path, weight: u32) -> bool {
        let ownership = CgroupOwnership::classify(
            target,
            self.delegated_root.as_deref(),
            self.info.version == CgroupVersion::V2,
        );

        if ownership == CgroupOwnership::Foreign {
            // Resource tuning is allowed outside the delegated root only when
            // the kernel has already exposed the controller file.
            debug!(
                "set_cpu_weight: Target {:?} is outside the delegated cgroup; attempting optional resource tuning if available",
                target
            );
        }

        if self.info.version == CgroupVersion::V2 {
            let weight_val = weight.clamp(1, 10000);
            let weight_file = target.join("cpu.weight");
            let Ok(mut f) = OpenOptions::new().write(true).open(&weight_file) else {
                self.report_weightless(target, &weight_file);
                debug!("set_cpu_weight: Failed to open {:?}", weight_file);
                return false;
            };
            let buf = format!("{}\n", weight_val);
            if let Err(e) = f.write_all(buf.as_bytes()) {
                debug!("set_cpu_weight: Failed to write {:?}: {}", weight_file, e);
                return false;
            }
        } else {
            let shares = (weight as f32 / 100.0 * 1024.0) as u32;
            let shares = shares.clamp(2, 262144);
            let shares_file = target.join("cpu.shares");
            let Ok(mut f) = OpenOptions::new().write(true).open(&shares_file) else {
                self.report_weightless(target, &shares_file);
                debug!("set_cpu_weight: Failed to open {:?}", shares_file);
                return false;
            };
            let buf = format!("{}\n", shares);
            if let Err(e) = f.write_all(buf.as_bytes()) {
                debug!("set_cpu_weight: Failed to write {:?}: {}", shares_file, e);
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::mounts::{CgroupInfo, CgroupVersion},
        PathBuf,
    };

    /// Integration tests in `tests/cgroup_manager.rs` drive the manager through
    /// `new_with_root` against a temporary hierarchy, so this module only keeps
    /// the checks that need the private fields.
    ///
    /// The product `period * cores * quota` first exceeds `u32::MAX`, which is
    /// where the old `u32` arithmetic started writing nonsense. The v2 period is
    /// 100 ms, so it takes 430 cores at a full quota; the v1 period is ten times
    /// that, so 430 cores too. Below the threshold the two agree, which is why
    /// this went unnoticed on every ordinary machine.
    #[test]
    fn a_quota_beyond_the_32_bit_product_is_not_wrapped() {
        for period in [V2_PERIOD_US, V1_PERIOD_US] {
            for cores in [1u64, 8, 64, 128, 256, 430, 512, 1024, 4096, 65536] {
                for quota in [0u32, 1, 25, 50, 99, 100, 255] {
                    let expected = period * cores * u64::from(quota.min(100)) / 100;
                    assert_eq!(
                        bandwidth_us(cores, quota, period),
                        expected,
                        "period={period} cores={cores} quota={quota}"
                    );
                    assert!(
                        bandwidth_us(cores, quota, period) <= u64::from(u32::MAX) * 100,
                        "the value has to be representable in the u64 the kernel reads"
                    );
                }
            }
        }
    }

    /// The concrete case the overflow produced: a 512-core machine asking for a
    /// full quota, on cgroup v2. `u32` arithmetic gave 8_250_327 µs — 0.08 of a
    /// CPU — where 51_200_000 µs is 512 CPUs.
    #[test]
    fn the_512_core_full_quota_case() {
        assert_eq!(bandwidth_us(512, 100, V2_PERIOD_US), 51_200_000);
        assert_eq!(bandwidth_us(430, 100, V2_PERIOD_US), 43_000_000);
        assert_eq!(bandwidth_us(8, 100, V2_PERIOD_US), 800_000);
        assert_eq!(bandwidth_us(8, 50, V2_PERIOD_US), 400_000);
        assert_eq!(bandwidth_us(8, 100, V1_PERIOD_US), 8_000_000);
        // A quota above 100 is still clamped, as it always was.
        assert_eq!(bandwidth_us(8, 1000, V2_PERIOD_US), 800_000);
        assert_eq!(bandwidth_us(8, 0, V2_PERIOD_US), 0);
    }

    /// A `Manager` without a hierarchy resolves nothing.
    #[test]
    fn a_manager_without_a_hierarchy_resolves_nothing() {
        let manager = CgroupManager::new_with_root(
            CgroupInfo {
                mount_point: PathBuf::new(),
                version: CgroupVersion::None,
            },
            None,
        );

        assert_eq!(manager.resolve_target_dir("anything"), None);
        assert!(!manager.cgroup_exists("anything"));
    }

    #[test]
    fn info_is_reported_back() {
        let manager = CgroupManager::new_with_root(
            CgroupInfo {
                mount_point: PathBuf::from("/sys/fs/cgroup"),
                version: CgroupVersion::V2,
            },
            None,
        );

        assert_eq!(manager.info().version, CgroupVersion::V2);
        assert_eq!(manager.info().mount_point, PathBuf::from("/sys/fs/cgroup"));
    }
}

#[cfg(test)]
mod weightless_reporting {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts what the logger was actually handed, so "warns once" is measured
    /// rather than assumed. `tracing` has no capture harness here, so the set the
    /// helper maintains is inspected directly instead -- it is the thing that
    /// decides whether the second call logs.
    fn manager() -> CgroupManager {
        CgroupManager::new_with_root(
            CgroupInfo {
                mount_point: PathBuf::from("/sys/fs/cgroup"),
                version: CgroupVersion::V2,
            },
            Some(PathBuf::from("/sys/fs/cgroup/delegated")),
        )
    }

    #[test]
    fn a_cgroup_is_reported_once_and_a_second_one_still_is_reported() {
        let m = manager();
        let a = Path::new("/sys/fs/cgroup/delegated/a");
        let b = Path::new("/sys/fs/cgroup/delegated/b");
        let f = Path::new("/sys/fs/cgroup/delegated/a/cpu.weight");

        m.report_weightless(a, f);
        m.report_weightless(a, f);
        m.report_weightless(b, f);

        let seen = m
            .reported_weightless
            .lock()
            .expect("the set is not poisoned");
        assert_eq!(seen.len(), 2, "one entry per cgroup, not per call");
        assert!(seen.contains(a));
        assert!(seen.contains(b));
    }

    /// The manager is cloned to reach the process-global, so the memory has to be
    /// shared. If it were per-clone, a clone would re-report everything.
    #[test]
    fn a_clone_does_not_re_report_what_the_original_reported() {
        let m = manager();
        let a = Path::new("/sys/fs/cgroup/delegated/a");
        let f = Path::new("/sys/fs/cgroup/delegated/a/cpu.weight");
        m.report_weightless(a, f);

        let clone = m.clone();
        clone.report_weightless(a, f);
        let seen = clone.reported_weightless.lock().expect("not poisoned");
        assert_eq!(seen.len(), 1, "the set must be shared across clones");
    }

    /// The bound is the point: this set only ever grows, so without one its size
    /// is whatever the daemon happened to meet before it was killed. Reported
    /// paths are made up here, which no real run would produce — a real one has
    /// a handful of cgroups — because the assertion is about the bound holding
    /// and not about it being reached in practice.
    #[test]
    fn the_reported_set_does_not_grow_past_its_bound() {
        let m = manager();
        let f = Path::new("/sys/fs/cgroup/delegated/a/cpu.weight");
        for i in 0..(REPORTED_WEIGHTLESS_CAPACITY * 4) {
            m.report_weightless(Path::new(&format!("/sys/fs/cgroup/delegated/c{i}")), f);
        }
        let seen = m.reported_weightless.lock().expect("not poisoned");
        assert_eq!(
            seen.len(),
            REPORTED_WEIGHTLESS_CAPACITY,
            "four times the bound went in, so the set is bounded rather than growing"
        );
    }

    /// A cgroup still in the set is not reported a second time, which is the
    /// whole reason the set exists.
    #[test]
    fn a_bounded_set_still_deduplicates_what_it_keeps() {
        let m = manager();
        let a = Path::new("/sys/fs/cgroup/delegated/a");
        let f = Path::new("/sys/fs/cgroup/delegated/a/cpu.weight");
        m.report_weightless(a, f);
        m.report_weightless(a, f);
        m.report_weightless(a, f);
        let seen = m.reported_weightless.lock().expect("not poisoned");
        assert_eq!(seen.len(), 1, "three calls, one cgroup, one entry");
        assert!(seen.contains(a));
    }

    /// A poisoned lock must not take the daemon down: the worst outcome of losing
    /// the set is a repeated warning, which is what it was before.
    #[test]
    fn a_poisoned_lock_is_survivable() {
        static CALLED: AtomicUsize = AtomicUsize::new(0);
        let m = manager();
        m.reported_weightless
            .lock()
            .expect("lock it once")
            .put(PathBuf::from("/poison"), ());
        // Poison it deliberately.
        let _ = std::panic::catch_unwind(|| {
            let _guard = m.reported_weightless.lock().unwrap();
            panic!("poison");
        });
        CALLED.fetch_add(1, Ordering::SeqCst);
        // Must not panic even though the lock is now poisoned.
        m.report_weightless(Path::new("/sys/fs/cgroup/delegated/x"), Path::new("x"));
        assert_eq!(CALLED.load(Ordering::SeqCst), 1);
    }
}
