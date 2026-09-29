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

/// A kernel CPU list — `0-3,8`, the shape both
/// `/sys/devices/system/cpu/possible` and `/proc/<tid>/status`'s
/// `Cpus_allowed_list` use — as a mask of `num_bytes` bytes.
///
/// `None` for a range that does not parse, and for a list with no CPU in it at
/// all: a mask of zeroes would assert that this machine has no CPUs, which is
/// not something to conclude from a string that could not be read.
fn mask_from_cpu_list(list: &str, num_bytes: usize) -> Option<Vec<u8>> {
    let mut mask = vec![0u8; num_bytes];
    let mut any = false;

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
                any = true;
            }
        }
    }

    any.then_some(mask)
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
    mask_from_cpu_list(line.split_once(':')?.1, num_bytes)
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
    // `is_empty` rather than `get_cores().is_empty()`: the latter built a
    // `Vec<u32>` of every set CPU, on every process, to answer a yes/no
    // question and throw the vector away.
    if cpuset.is_empty() {
        return Ok(());
    }

    // The mask comes from the set rather than being derived from it here. It
    // is a pure function of the set and the CPU count, and this runs for every
    // process a `cpuset` rule touches, so deriving it per call meant building
    // the same bytes from the same set over and over.
    let mask = cpuset.kernel_mask();
    let num_bytes = mask.len();

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

/// The CPUs this machine can run a task on, and how precisely that is known.
///
/// Used to tell the two things `sched_setaffinity(2)` answers `EINVAL` for apart:
/// a mask naming CPUs that do not exist, and a mask whose intersection with the
/// task's cpuset is empty. The first is a rule that cannot be satisfied on this
/// machine; the second is a rule that could have been satisfied somewhere else.
/// The kernel does not distinguish them, and an operator reading a log would not
/// either if the daemon did not.
///
/// Neither source is always readable. `/sys` is absent from a Nix build sandbox
/// and from many containers, so `/proc/cpuinfo` stands in for it — but it lists
/// the *online* CPUs, which is a narrower claim than "exists", and the two must
/// not be reported as the same thing.
enum MachineCpus {
    /// `/sys/devices/system/cpu/possible`: every CPU the kernel was built for.
    Possible(Vec<u8>),
    /// `/proc/cpuinfo`: the CPUs that are up, which excludes those that exist
    /// but are off.
    Online(Vec<u8>),
}

impl MachineCpus {
    fn read(num_bytes: usize) -> Option<Self> {
        std::fs::read_to_string("/sys/devices/system/cpu/possible")
            .ok()
            .and_then(|raw| mask_from_cpu_list(raw.trim(), num_bytes))
            .map(Self::Possible)
            .or_else(|| online_cpus(num_bytes).map(Self::Online))
    }

    fn mask(&self) -> &[u8] {
        match self {
            Self::Possible(mask) | Self::Online(mask) => mask,
        }
    }

    /// What to say about a rule that named none of them. Wording follows the
    /// source, so a CPU that is merely offline is not called non-existent.
    fn absent(&self) -> &'static str {
        match self {
            Self::Possible(_) => "no CPU a rule asked for exists on this machine",
            Self::Online(_) => "no CPU a rule asked for is online on this machine",
        }
    }
}

fn online_cpus(num_bytes: usize) -> Option<Vec<u8>> {
    let raw = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let mut mask = vec![0u8; num_bytes];
    let mut any = false;

    for line in raw.lines() {
        let Some(rest) = line.strip_prefix("processor") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let Ok(cpu) = value.trim().parse::<u32>() else {
            continue;
        };
        if cpu < (num_bytes * 8) as u32 {
            mask[(cpu / 8) as usize] |= 1 << (cpu % 8);
            any = true;
        }
    }

    any.then_some(mask)
}

/// One retry with `requested ∩ allowed`.
///
/// A mask the kernel considers impossible does not narrow to anything, so it is
/// separated out first: naming a CPU that does not exist is a different answer
/// from a rule whose CPUs this process is not allowed to reach. Neither is
/// something to infer when the machine's CPU set could not be read — a mask
/// naming CPUs that do not exist is not "not available to this process", and
/// saying so would point whoever reads the log at the wrong process.
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

    let machine = MachineCpus::read(num_bytes);
    if let Some(known) = &machine
        && !any_set(&and(mask, known.mask()))
    {
        return Err(PlatformError::InvalidCpuset(known.absent().to_string()));
    }

    let narrowed = and(mask, &allowed);

    if !any_set(&narrowed) {
        return Err(match &machine {
            Some(_) => PlatformError::Skipped(format!(
                "no CPU in the cpuset a rule asked for is available to {pid}"
            )),
            // Every CPU named exists and none of them is this process's to use,
            // unless the machine's CPUs are unknown, in which case that is a
            // guess and the kernel's own refusal is what gets reported.
            None => PlatformError::Io(io::Error::from_raw_os_error(libc::EINVAL)),
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cores(mask: &[u8]) -> Vec<u32> {
        mask.iter()
            .enumerate()
            .flat_map(|(byte, &b)| {
                (0..8)
                    .filter_map(move |bit| (b & (1 << bit) != 0).then_some((byte * 8) as u32 + bit))
            })
            .collect()
    }

    /// A list of CPUs is read from three places — the kernel's `possible` mask,
    /// a thread's `Cpus_allowed_list` and a rule's own `cpuset` string — so a
    /// mistake in it is a mistake in what a rule is allowed to touch.
    #[test]
    fn a_cpu_list_becomes_the_mask_it_names() {
        assert_eq!(
            cores(&mask_from_cpu_list("0-3,8", 2).unwrap()),
            [0, 1, 2, 3, 8]
        );
        assert_eq!(cores(&mask_from_cpu_list(" 2 , 8 ", 2).unwrap()), [2, 8]);
        assert_eq!(cores(&mask_from_cpu_list("7", 1).unwrap()), [7]);
    }

    /// A list naming nothing the mask can address is an empty result, not a mask
    /// of zeroes: zeroes would say this machine has no CPUs at all, and the caller
    /// acts on that difference.
    #[test]
    fn a_list_the_mask_is_too_short_for_is_no_cpus_at_all() {
        assert!(mask_from_cpu_list("8", 1).is_none());
        assert!(mask_from_cpu_list("64-65", 1).is_none());
        assert!(mask_from_cpu_list("", 2).is_none());
        assert!(mask_from_cpu_list("garbage", 2).is_none());
    }

    /// A CPU id past the end of the mask is dropped rather than read as one: a
    /// `possible` file that does not parse must not be able to name CPU 4096, and
    /// a list that reaches past the end is still a list of the CPUs that fit.
    #[test]
    fn a_list_reaching_past_the_mask_keeps_the_part_that_fits() {
        assert_eq!(
            cores(&mask_from_cpu_list("0-1023", 2).unwrap()),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
    }

    /// The offline-CPU stand-in for `possible`, and the last source consulted
    /// when `/sys` is not mounted. It is read from the running machine, so all
    /// this can say is that it is there and does not claim CPUs out of range.
    #[test]
    fn the_online_cpus_are_readable_where_procfs_is() {
        let Some(mask) = online_cpus(128) else {
            return;
        };
        assert!(
            mask.iter().any(|&b| b != 0),
            "a mask of zeroes would claim this machine has no CPUs"
        );
        assert_eq!(mask.len(), 128, "as wide as the mask the caller asked for");
    }

    /// Whichever source answers, the machine's CPUs have to be narrower than the
    /// mask the rule was built for — a source that reported a CPU nobody can
    /// address would turn every `cpuset` into a `Skipped`.
    #[test]
    fn the_machine_cpus_fit_in_the_mask_they_are_read_for() {
        if let Some(machine) = MachineCpus::read(128) {
            assert_eq!(machine.mask().len(), 128);
            assert!(
                !machine.absent().is_empty(),
                "an unsatisfiable cpuset is reported with a reason"
            );
        }
    }
}
