use {
    std::{thread::sleep, time::Duration},
    tracing::trace,
};

use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

pub use crate::cgroup::{CgroupInfo, CgroupVersion};

static CGROUP_INFO: RwLock<Option<CgroupInfo>> = RwLock::new(None);

/// How many times [`init_cgroups`] re-detects before giving up, and how long it
/// waits in between. Together they bound the startup delay at ten seconds.
pub const CGROUP_INIT_ATTEMPTS: usize = 20;
pub const CGROUP_INIT_INTERVAL: Duration = Duration::from_millis(500);

/// The total time [`init_cgroups`] can spend waiting, for reporting.
pub const CGROUP_INIT_TIMEOUT: Duration =
    Duration::from_millis(CGROUP_INIT_ATTEMPTS as u64 * CGROUP_INIT_INTERVAL.as_millis() as u64);

/// The hierarchy version, without the mount point.
///
/// [`get_cgroup_info`] returns `CgroupInfo` by value, which is right for a
/// caller that wants the mount point. A caller that only wants the version was
/// cloning a `PathBuf` — a heap allocation and a copy — to read a one-byte
/// enum, and `is_cgroup_v2` is asked once per process that matches a rule with
/// a `nice` on it, so that was one allocation per process for a discriminant.
///
/// Falls back to [`get_cgroup_info`] when nothing is cached, which keeps the
/// lazy detection and its single-publisher rule exactly as they were.
pub fn cgroup_version() -> CgroupVersion {
    if let Ok(info) = CGROUP_INFO.read()
        && let Some(i) = &*info
    {
        return i.version;
    }
    get_cgroup_info().version
}

pub fn reset_cgroup_info() {
    if let Ok(mut info) = CGROUP_INFO.write() {
        *info = None;
    }
}

/// Re-detects the hierarchy until one is found, or the attempts run out.
///
/// Returns whether a hierarchy is available.
pub fn init_cgroups() -> bool {
    let found = init_cgroups_with(
        get_cgroup_info,
        // A cached `None` would make every attempt re-read the same stale
        // answer. Dropping it before each detection lets a hierarchy that is
        // mounted during the wait be seen by the next attempt. The reset is
        // injected alongside the reader so a test can drive the same pairing
        // as production instead of a stand-in that omits it.
        reset_cgroup_info,
        CGROUP_INIT_ATTEMPTS,
        CGROUP_INIT_INTERVAL,
    );
    if !found {
        reset_cgroup_info();
    }
    found
}

/// The retry loop with its detector and cache reset injected, so the
/// late-mount behaviour can be tested without waiting for the real interval or
/// touching the real mount. Both halves are parameters because the production
/// closure pairs `reset_cgroup_info()` with `get_cgroup_info()`: leaving the
/// reset out of the seam was what let a cached `None` hide a hierarchy mounted
/// during the wait from a test.
fn init_cgroups_with<F: Fn() -> CgroupInfo, R: Fn()>(
    reader: F,
    reset: R,
    attempts: usize,
    interval: Duration,
) -> bool {
    for _ in 0..attempts {
        reset();
        if reader().version != CgroupVersion::None {
            return true;
        }
        sleep(interval);
    }
    false
}

/// Whether a `cgroup2` mount point is a hierarchy this daemon can actually apply
/// `cpu` limits in.
///
/// `cgroup.controllers` on the *root* of a v2 hierarchy lists the controllers
/// that may be enabled for its children, which is exactly the question a
/// `CPUQuota`/`CPUWeight` rule ends up asking. The reference daemon (and this one
/// until now) instead created a throwaway cgroup in the mount point and read
/// `cgroup.controllers` from *that*.
///
/// The two answers are the same, and the read is strictly better:
///
/// * a `mkdir` in the cgroup root needs write access to it, which a read-only
///   cgroupfs mount does not give anybody — inside most container runtimes'
///   default cgroup namespace, and on any `ro` sysfs, so the probe failed and
///   the whole hierarchy was reported as unavailable even though it was fully
///   functional;
/// * the probe created and removed a directory in the global cgroup root on
///   every re-detection, which is visible to `cgroup.events`, to inotify
///   watchers, and to systemd's own accounting;
/// * two threads detecting at once raced on the same fixed name
///   (`ananicy_test_cgroup2`), so one got `EEXIST`, decided the hierarchy was
///   unusable, and could overwrite the other's correct answer.
fn v2_has_cpu_controller(mount_point: &Path) -> bool {
    let Ok(controllers) = fs::read_to_string(mount_point.join("cgroup.controllers")) else {
        return false;
    };
    let has_cpu = controllers.split_whitespace().any(|c| c == "cpu");
    trace!(
        "get_cgroup_version: {} controllers = {:?}",
        mount_point.display(),
        controllers
    );
    // `cpu.max` is what a `.cgroups` rule writes. A root whose `cpu` controller
    // exists but whose `cpu.max` does not is a kernel or configuration this
    // daemon cannot do anything useful with, so it is not a match.
    has_cpu && mount_point.join("cgroup.procs").exists()
}

pub fn parse_cgroups_from_str(content: &str, info: &mut CgroupInfo) {
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let fs_type = parts[2];
        let mount_point = PathBuf::from(parts[1]);

        if fs_type == "cgroup2" {
            if !v2_has_cpu_controller(&mount_point) {
                continue;
            }
            trace!("Found cgroup v2 at {}", mount_point.display());
            info.version = CgroupVersion::V2;
            info.mount_point = mount_point;
            break;
        } else if fs_type == "cgroup"
            && info.version == CgroupVersion::None
            && let Some(parent) = mount_point.parent()
            && parent.join("cpu").exists()
        {
            trace!("Found cgroup v1 at {}", parent.display());
            info.version = CgroupVersion::V1;
            info.mount_point = parent.to_path_buf();
        }
    }
}

pub fn get_cgroup_info() -> CgroupInfo {
    if let Ok(info) = CGROUP_INFO.read()
        && let Some(i) = &*info
    {
        return i.clone();
    }

    // A cached answer is not published here: the detection below reads the
    // mount table, and publishing only when nothing is cached keeps the first
    // successful detection authoritative if two threads detect at once. A
    // repeat costs one read of `/proc/self/mounts`, which is a few kilobytes
    // and happens at most once per `reset_cgroup_info`.
    let mut info = CgroupInfo {
        version: CgroupVersion::None,
        mount_point: PathBuf::new(),
    };

    // `/proc/self/mounts` is a pseudo-file; do not rely on metadata/file size before reading it.
    if let Ok(content) = fs::read_to_string("/proc/self/mounts") {
        parse_cgroups_from_str(&content, &mut info);
    }

    // Publish only if nothing else has in the meantime.
    if let Ok(mut lock) = CGROUP_INFO.write()
        && lock.is_none()
    {
        *lock = Some(info.clone());
    }

    info
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        std::{
            cell::{Cell, RefCell},
            sync::Mutex,
        },
    };

    // These tests reset the process-global detection cache, and they count
    // reader calls, so they must not interleave with each other.
    static DETECTION_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn detected(version: CgroupVersion) -> CgroupInfo {
        CgroupInfo {
            version,
            mount_point: if version == CgroupVersion::None {
                PathBuf::new()
            } else {
                PathBuf::from("/sys/fs/cgroup")
            },
        }
    }

    /// A hierarchy mounted while the startup loop is waiting is picked up on
    /// the next attempt instead of being hidden by a cached `None`. The test
    /// mirrors the production pair: the reader returns a cached answer when
    /// there is one and only consults the "mount table" when the injected reset
    /// has cleared it. A seam without that reset would keep returning the
    /// first `None` and never see the hierarchy.
    #[test]
    fn a_hierarchy_that_appears_on_the_third_attempt_is_found() {
        let _guard = DETECTION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let calls = Cell::new(0);
        let cache = RefCell::new(Some(detected(CgroupVersion::None)));

        let found = init_cgroups_with(
            || {
                if let Some(cached) = cache.borrow().clone() {
                    return cached;
                }
                let attempt = calls.get();
                calls.set(attempt + 1);
                let fresh = if attempt < 2 {
                    detected(CgroupVersion::None)
                } else {
                    detected(CgroupVersion::V2)
                };
                *cache.borrow_mut() = Some(fresh.clone());
                fresh
            },
            || *cache.borrow_mut() = None,
            5,
            Duration::ZERO,
        );

        assert!(found, "the hierarchy installed on attempt 3 must be found");
        assert_eq!(
            calls.get(),
            3,
            "detection must stop as soon as the hierarchy appears"
        );
    }

    /// The reset is part of the contract, not an implementation detail of the
    /// production closure: it runs before every read, including the last one.
    #[test]
    fn the_reset_runs_before_every_read() {
        let _guard = DETECTION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let events = RefCell::new(Vec::new());

        let found = init_cgroups_with(
            || {
                events.borrow_mut().push("read");
                detected(CgroupVersion::None)
            },
            || events.borrow_mut().push("reset"),
            3,
            Duration::ZERO,
        );

        assert!(!found);
        assert_eq!(
            events.into_inner(),
            ["reset", "read", "reset", "read", "reset", "read"],
            "each attempt must drop the cached answer before detecting again"
        );
    }

    #[test]
    fn every_attempt_is_made_when_no_hierarchy_ever_appears() {
        let _guard = DETECTION_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let calls = Cell::new(0);

        let found = init_cgroups_with(
            || {
                calls.set(calls.get() + 1);
                detected(CgroupVersion::None)
            },
            || {},
            4,
            Duration::ZERO,
        );

        assert!(!found);
        assert_eq!(calls.get(), 4, "each attempt must make a fresh detection");
    }
}
