use {ananicy_core::cpuset::CpuSet, tracing::debug};

use {ananicy_core::worker::PlatformError, libc::cpu_set_t, std::io};

use std::sync::OnceLock;

/// The number of CPUs a mask is built for.
///
/// `_SC_NPROCESSORS_CONF` is the number of *configured* CPUs, and the kernel
/// requires `sched_setaffinity(2)`'s `len` to be at least the size of its own
/// `cpumask`. Taking the larger of the two covers both: a kernel whose
/// `CONFIG_NR_CPUS` is 1024 needs 128 bytes even on an 8-CPU machine, and a
/// machine with more than 1024 CPUs needs its own.
pub fn get_max_number_of_cpus() -> u32 {
    static MAX_CPUS: OnceLock<u32> = OnceLock::new();
    *MAX_CPUS.get_or_init(|| {
        let sys_cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) as u32 };
        // Ensure we allocate at least sizeof(cpu_set_t) which covers 1024 CPUs
        std::cmp::max(sys_cpus, 1024)
    })
}

/// The mask `get_max_number_of_cpus()` bytes wide, built from a `CpuSet`.
fn mask_from(cpuset: &CpuSet, num_cpus: u32) -> Vec<u8> {
    let num_bytes = (num_cpus as usize) / 8;
    let mut mask = vec![0u8; num_bytes];
    for cpu in cpuset.get_cores() {
        if cpu < num_cpus {
            let byte_idx = (cpu / 8) as usize;
            let bit_idx = cpu % 8;
            mask[byte_idx] |= 1 << bit_idx;
        }
    }
    mask
}

/// A `sched_setaffinity(2)` on one thread.
fn set_affinity_for(tid: i32, mask: &[u8]) -> io::Result<()> {
    let ret =
        unsafe { libc::sched_setaffinity(tid, mask.len(), mask.as_ptr() as *const cpu_set_t) };
    if ret != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The mask a task is currently allowed to run on, as the kernel reports it.
///
/// `/proc/<tid>/status`'s `Cpus_allowed_list` and `sched_getaffinity(2)` are
/// both `p->cpus_ptr`, which the kernel already intersects with the task's cpuset
/// and with the online-CPU mask, so this is the closest a user-space reader gets
/// to "where this task is allowed to run".
fn current_affinity(tid: i32, num_bytes: usize) -> Option<Vec<u8>> {
    let status = std::fs::read_to_string(format!("/proc/{tid}/status")).ok()?;
    let line = status
        .lines()
        .find(|l| l.starts_with("Cpus_allowed_list:"))?;
    let list = line.split_once(':')?.1;

    let mut mask = vec![0u8; num_bytes];
    for part in list.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (start, end) = match part.split_once('-') {
            Some((s, e)) => (s.parse::<u32>().ok()?, e.parse::<u32>().ok()?),
            None => {
                let cpu = part.parse::<u32>().ok()?;
                (cpu, cpu)
            }
        };
        for cpu in start..=end {
            if cpu < (num_bytes * 8) as u32 {
                mask[(cpu / 8) as usize] |= 1 << (cpu % 8);
            }
        }
    }
    Some(mask)
}

/// Applies a rule's `cpuset` to every thread of a process.
///
/// Two kernel behaviours shape this, and both were mishandled:
///
/// * `sched_setaffinity(2)` is **all or nothing**. The kernel ANDs the
///   requested mask with the task's allowed set
///   (`cpumask_and(new_mask, ctx->new_mask, cpuset_cpus_allowed(p, ...))` in
///   `__sched_setaffinity()`), and an empty intersection is `EINVAL`. A rule
///   asking for `big-cores` applied to a process systemd has confined to
///   `little-cores`, or to one `taskset` has pinned elsewhere, therefore failed
///   for the whole attribute and every thread, and the failure was reported as
///   `Unsupported` — indistinguishable from "this kernel has no affinity call".
/// * A thread that exited between the `task/` snapshot and this call answers
///   `ESRCH`, which says nothing about the threads that were still there.
///
/// So a failed call is retried once with the intersection of what was asked for
pub fn set_affinity(pid: i32, tids: &[i32], cpuset: &CpuSet) -> Result<(), PlatformError> {
    if cpuset.get_cores().is_empty() {
        return Ok(());
    }

    let num_cpus = get_max_number_of_cpus();
    let num_bytes = (num_cpus as usize) / 8;
    let mask = mask_from(cpuset, num_cpus);

    if let Err(failure) = apply_to_tids(tids, &mask) {
        return match failure {
            AffinityFailure::RetryNarrower => narrow_and_retry(pid, tids, &mask, num_bytes),
            AffinityFailure::Settled(e) => Err(e),
        };
    }
    debug!("set_affinity: Successfully applied to {}", pid);
    Ok(())
}

/// Why one pass over the thread list did not finish the job, and what can be
/// done about it.
enum AffinityFailure {
    /// `EINVAL`. `sched_setaffinity(2)` is all or nothing: the kernel ANDs the
    /// request with the task's allowed set
    /// (`cpumask_and(new_mask, ctx->new_mask, cpuset_cpus_allowed(p, ...))` in
    /// `__sched_setaffinity()`) and an empty intersection is `EINVAL`. A rule
    /// asking for `big-cores` on a process systemd has confined to
    /// `little-cores`, or one `taskset` has pinned elsewhere, therefore failed
    /// for every thread and the whole attribute. Worth one narrower retry.
    RetryNarrower,
    /// Anything else, already classified.
    Settled(PlatformError),
}

fn apply_to_tids(tids: &[i32], mask: &[u8]) -> Result<(), AffinityFailure> {
    let mut first_err: Option<io::Error> = None;
    let mut applied = false;
    let mut not_found = false;

    for &tid in tids {
        match set_affinity_for(tid, mask) {
            Ok(()) => applied = true,
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
                // The thread is gone. The process may well be a zombie, and a
                // zombie has no thread left to move. That is an expected state,
                // not a failed attribute.
                not_found = true;
            }
            Err(e) if first_err.is_none() => first_err = Some(e),
            Err(_) => {}
        }
    }

    if applied {
        return Ok(());
    }

    if let Some(e) = first_err {
        return Err(if e.raw_os_error() == Some(libc::EINVAL) {
            AffinityFailure::RetryNarrower
        } else {
            AffinityFailure::Settled(classify(e))
        });
    }

    if not_found {
        return Err(AffinityFailure::Settled(PlatformError::NotFound));
    }

    Ok(())
}

/// The CPUs the kernel was built with, as its own `possible` mask says.
///
/// Used to tell the two things `sched_setaffinity(2)` answers `EINVAL` for apart:
/// a mask naming CPUs that do not exist, and a mask whose intersection with the
/// task's cpuset is empty. The first is a rule that cannot be satisfied on this
/// machine; the second is a rule that could have been satisfied somewhere else.
/// The kernel does not distinguish them, and an operator reading a log would not
/// either if the daemon did not.
fn kernel_possible_cpus(num_bytes: usize) -> Option<Vec<u8>> {
    let raw = std::fs::read_to_string("/sys/devices/system/cpu/possible").ok()?;
    let mut mask = vec![0u8; num_bytes];
    for part in raw.trim().split(',') {
        let (start, end) = match part.split_once('-') {
            Some((s, e)) => (s.parse::<u32>().ok()?, e.parse::<u32>().ok()?),
            None => {
                let cpu = part.trim().parse::<u32>().ok()?;
                (cpu, cpu)
            }
        };
        for cpu in start..=end {
            if (cpu as usize) < num_bytes * 8 {
                mask[(cpu / 8) as usize] |= 1 << (cpu % 8);
            }
        }
    }
    Some(mask)
}

/// One retry with `requested ∩ allowed`.
///
/// A mask the kernel considers impossible does not narrow to anything, so it is
/// separated out first: naming a CPU that does not exist is a different answer
/// from a rule whose CPUs this process is not allowed to reach.
fn narrow_and_retry(
    pid: i32,
    tids: &[i32],
    mask: &[u8],
    num_bytes: usize,
) -> Result<(), PlatformError> {
    let and = |a: &[u8], b: &[u8]| -> Vec<u8> { a.iter().zip(b).map(|(x, y)| x & y).collect() };
    let any_set = |m: &[u8]| m.iter().any(|&b| b != 0);

    let Some(allowed) = tids.first().and_then(|&t| current_affinity(t, num_bytes)) else {
        return Err(PlatformError::InvalidCpuset(format!(
            "the kernel refused the cpuset for {pid} and its allowed CPUs could not be read"
        )));
    };

    if let Some(possible) = kernel_possible_cpus(num_bytes)
        && !any_set(&and(mask, &possible))
    {
        return Err(PlatformError::InvalidCpuset(
            "no CPU a rule asked for exists on this machine".to_string(),
        ));
    }

    let narrowed = and(mask, &allowed);

    if !any_set(&narrowed) {
        return Err(PlatformError::Skipped(format!(
            "no CPU in the cpuset a rule asked for is available to {pid}"
        )));
    }

    if narrowed == mask {
        return Err(PlatformError::Io(io::Error::from_raw_os_error(
            libc::EINVAL,
        )));
    }

    debug!(
        "set_affinity: the whole cpuset was refused for {pid}; retrying with the \
         CPUs it is allowed to run on"
    );
    match apply_to_tids(tids, &narrowed) {
        Ok(()) => Ok(()),
        Err(AffinityFailure::Settled(e)) => Err(e),
        Err(AffinityFailure::RetryNarrower) => Err(PlatformError::Skipped(format!(
            "the narrowed cpuset was refused for {pid} as well"
        ))),
    }
}

fn classify(error: io::Error) -> PlatformError {
    match error.raw_os_error() {
        Some(libc::ESRCH) => PlatformError::NotFound,
        Some(libc::EACCES) | Some(libc::EPERM) => PlatformError::PermissionDenied,
        _ => PlatformError::Io(error),
    }
}
