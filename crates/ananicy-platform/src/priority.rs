use {
    crate::abi::sched::SchedParam,
    ananicy_core::worker::{
        PlatformError,
        PlatformError::{Io, NotFound, PermissionDenied, Unsupported},
    },
    rustix::io::Errno,
};

use crate::abi::{ioprio::*, sched_attr::*};

use {
    std::{fs, io},
    tracing::{debug, warn},
};

// Note: In C++ original code, `test_errno` handles EPERM, ESRCH, etc.
// We will replicate similar logic.
fn test_errno(err: io::Error, func_name: &str, pid: i32) -> Result<(), PlatformError> {
    let Some(raw_os_error) = err.raw_os_error() else {
        return Err(Io(err));
    };

    if raw_os_error == 0 {
        debug!("{}: Successfully applied to {}", func_name, pid);
        return Ok(());
    }

    if err.kind() == io::ErrorKind::NotFound || raw_os_error == Errno::SRCH.raw_os_error() {
        return Err(NotFound);
    }
    if raw_os_error == Errno::ACCESS.raw_os_error() || raw_os_error == Errno::PERM.raw_os_error() {
        return Err(PermissionDenied);
    }
    Err(Io(err))
}

/// The first error from a per-thread loop, if the attribute did not take effect
/// anywhere.
///
/// `ESRCH` from a tid the daemon listed in `/proc/<pid>/task` a moment ago says
/// nothing about the threads that were still there, so it is remembered rather
/// than returned: a process that lost two of its sixteen worker threads in that
/// window used to have its whole `nice` reported as failed even though the other
/// fourteen took the new value. A refusal for the threads that *are* still there
/// is a different thing and is still reported.
///
/// Every thread is attempted, and success needs at least one of them to have
/// accepted: returning on the first one that did would answer for the whole
/// process from a single thread's point of view, which is the mistake this
/// function exists next to.
fn first_real_error(results: impl Iterator<Item = Result<(), io::Error>>) -> Option<io::Error> {
    let mut first_real = None;
    let mut not_found = false;
    let mut applied = false;

    for result in results {
        match result {
            Ok(()) => applied = true,
            Err(e) if e.raw_os_error() == Some(Errno::SRCH.raw_os_error()) => not_found = true,
            Err(e) => {
                first_real.get_or_insert(e);
            }
        }
    }

    if let Some(e) = first_real {
        return Some(e);
    }
    // `ESRCH` only becomes the answer when nothing was applied anywhere, which
    // is the state a zombie or a process that exited mid-rule is in.
    if !applied && not_found {
        return Some(io::Error::from_raw_os_error(Errno::SRCH.raw_os_error()));
    }
    None
}

pub fn set_priority(pid: i32, tids: &[i32], nice_value: i32) -> Result<(), PlatformError> {
    use rustix::process::{Pid, setpriority_process};
    let results = tids
        .iter()
        .map(|&tid| setpriority_process(Pid::from_raw(tid), nice_value).map_err(io::Error::from));

    match first_real_error(results) {
        None => {
            debug!("set_priority: Successfully applied to {}", pid);
            Ok(())
        }
        Some(e) => test_errno(e, "set_priority", pid),
    }
}

pub fn set_latency_nice(
    pid: i32,
    tids: &[i32],
    latency_nice_value: i32,
) -> Result<(), PlatformError> {
    // `SCHED_FLAG_KEEP_POLICY` is not optional here. `sched_setattr(2)` rejects
    // a negative `sched_policy` outright and only substitutes the kernel's
    // "leave the policy alone" sentinel when this flag is set; without it, a
    // zero-filled `sched_attr` asks for `SCHED_OTHER` and the task is *moved*
    // onto it. `SCHED_FLAG_KEEP_PARAMS` alone does not help: it skips
    // `__setscheduler_params()`, which is what would have written the policy,
    // but the class is still recomputed from `attr->sched_policy` and applied.
    // So writing a latency nice would have demoted every `SCHED_BATCH`,
    // `SCHED_IDLE` and real-time task it was asked to touch.
    const SCHED_FLAG_LATENCY_NICE: u64 = 0x80;

    let results = tids.iter().map(|&tid| {
        let attr = sched_attr {
            size: std::mem::size_of::<sched_attr>() as u32,
            sched_policy: SCHED_NORMAL,
            sched_flags: SCHED_FLAG_LATENCY_NICE
                | crate::abi::sched_attr::SCHED_FLAG_KEEP_PARAMS
                | crate::abi::sched_attr::SCHED_FLAG_KEEP_POLICY,
            sched_latency_nice: latency_nice_value,
            ..Default::default()
        };

        crate::abi::sched_attr::sched_setattr(tid, &attr, 0)
    });

    match first_real_error(results) {
        None => {
            debug!("set_latency_nice: Successfully applied to {}", pid);
            Ok(())
        }
        Some(e) => test_errno(e, "set_latency_nice", pid),
    }
}

pub fn get_latency_nice(pid: i32) -> Option<i32> {
    let mut attr = sched_attr {
        size: std::mem::size_of::<sched_attr>() as u32,
        ..Default::default()
    };

    crate::abi::sched_attr::sched_getattr(
        pid,
        &mut attr,
        std::mem::size_of::<sched_attr>() as u32,
        0,
    )
    .ok()?;
    Some(attr.sched_latency_nice)
}

pub fn set_io_priority(pid: i32, io_class: &str, value: i32) -> Result<(), PlatformError> {
    let io_class_value = match io_class {
        "best-effort" => IOPRIO_CLASS_BE,
        "realtime" => IOPRIO_CLASS_RT,
        "idle" => IOPRIO_CLASS_IDLE,
        "none" => IOPRIO_CLASS_NONE,
        other => {
            // An unrecognised class is a typo in a rule, not a condition of the
            // machine: the attribute is dropped and the rest of the rule still
            // applies, which is what the caller has to be told with `Skipped`.
            return Err(PlatformError::Skipped(format!("unknown io class {other}")));
        }
    };

    // Class `none` is not a priority a task can hold: it is what a task reads as
    // "nothing was ever set", and the kernel's default for a fresh task is
    // best-effort at the middle of the range. Writing it would not restore that
    // default, it would replace it with class `none`, so a rule asking for
    // `none` leaves the process' I/O priority exactly as it is.
    if !ioprio_valid(io_class_value) {
        debug!(
            "set_io_priority: '{}' is not an ioprio class, leaving the I/O priority of {} untouched",
            io_class, pid
        );
        return Ok(());
    }

    let io_prio = ioprio_prio_value(io_class_value, value);

    if let Err(e) = ioprio_set(IOPRIO_WHO_PROCESS, pid, io_prio) {
        test_errno(e, "set_io_priority", pid)
    } else {
        debug!("set_io_priority: Successfully applied to {}", pid);
        Ok(())
    }
}

/// Applies a scheduling policy to every thread of a process.
///
/// `sched_setscheduler(2)` is per-thread — unlike `nice`, which since Linux
/// 2.6.36 is per-thread too but is what the reference reaches through a
/// `/proc/<pid>/task` loop. Passing only the thread-group leader, as this used
/// to, changed the policy of one of a game's sixteen worker threads and left the
/// other fifteen on `SCHED_OTHER`, so a rule saying `"sched": "idle"` or
/// `"sched": "batch"` had a visible effect on the process as a whole only by
/// accident of which thread the scheduler happened to run.
pub fn set_sched(
    pid: i32,
    tids: &[i32],
    sched_name: &str,
    rt_prio: u32,
) -> Result<(), PlatformError> {
    let mut param = SchedParam::default();

    let (sched, report) = match sched_name {
        "idle" => (SCHED_IDLE, None),
        "normal" | "other" => (SCHED_NORMAL, None),
        "rr" => {
            param.sched_priority = rt_prio as i32;
            (SCHED_RR, None)
        }
        "fifo" => {
            param.sched_priority = rt_prio as i32;
            (SCHED_FIFO, None)
        }
        "deadline" => {
            // The deadline policy needs a runtime, a deadline and a period, so
            // it cannot be handed to `sched_setscheduler`. The task is put back
            // on the normal policy, which is what the reference does, and the
            // attribute is reported as skipped so the rule is not claimed to
            // have been applied in full. This is the operator's own rule asking
            // for something, so it is worth a warning rather than a debug line.
            warn!("deadline scheduler is not available yet, falling back to OTHER");
            (SCHED_NORMAL, Some("deadline scheduler is unavailable"))
        }
        "batch" => (SCHED_BATCH, None),
        _ => return Err(Unsupported),
    };

    let results = tids
        .iter()
        .map(|&tid| crate::abi::sched::sched_setscheduler(tid, sched as i32, &param));

    let outcome = match first_real_error(results) {
        None => Ok(()),
        Some(e) => test_errno(e, "set_sched", pid),
    };

    match (outcome, report) {
        (Ok(()), Some(reason)) => {
            debug!("set_sched: Successfully applied to {}", pid);
            Err(PlatformError::Skipped(reason.to_string()))
        }
        (Ok(()), None) => {
            debug!("set_sched: Successfully applied to {}", pid);
            Ok(())
        }
        (Err(e), _) => Err(e),
    }
}

pub fn set_oom_score_adjust(pid: i32, value: i32) -> Result<(), PlatformError> {
    let path = format!("/proc/{}/oom_score_adj", pid);
    match fs::write(&path, value.to_string()) {
        Ok(()) => {
            debug!("set_oom_score_adjust: Successfully applied to {}", pid);
            Ok(())
        }
        Err(e) => match e.kind() {
            io::ErrorKind::NotFound => Err(NotFound),
            io::ErrorKind::PermissionDenied => Err(PermissionDenied),
            _ => Err(Io(e)),
        },
    }
}

pub fn test_latnice_support() -> bool {
    let pid = 0; // current process/thread
    let latency_nice = -20;

    const SCHED_FLAG_LATENCY_NICE: u64 = 0x80;
    const SCHED_FLAG_KEEP_PARAMS: u64 = 0x10;

    // Get original latency_nice state
    let original_latnice = get_latency_nice(pid).unwrap_or(0);

    let mut attr = sched_attr {
        size: std::mem::size_of::<sched_attr>() as u32,
        sched_flags: SCHED_FLAG_LATENCY_NICE | SCHED_FLAG_KEEP_PARAMS,
        sched_latency_nice: latency_nice,
        ..Default::default()
    };

    if crate::abi::sched_attr::sched_setattr(pid, &attr, 0).is_ok() {
        // Restore to original state
        attr.sched_latency_nice = original_latnice;
        let _ = crate::abi::sched_attr::sched_setattr(pid, &attr, 0);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PID above any plausible `pid_max` can never refer to a live process, so
    /// anything the function does before calling the kernel cannot be confused
    /// with what the kernel did.
    const DEAD_PID: i32 = i32::MAX;

    #[test]
    fn an_unknown_io_class_is_skipped_rather_than_fatal() {
        // The worker treats a skippable error as "this attribute did not apply,
        // carry on with the rest of the rule" and everything else as "the rule
        // cannot be applied". A typo in `ioclass` is the former.
        match set_io_priority(DEAD_PID, "best-effortt", 3) {
            Err(PlatformError::Skipped(message)) => {
                assert!(
                    message.contains("best-effortt"),
                    "the reported reason names the class: {message}"
                );
            }
            other => panic!("expected a skipped attribute, got {other:?}"),
        }
    }

    #[test]
    fn an_unapplicable_class_is_not_an_error() {
        // `none` is recognised, so it is not a skipped attribute: the rule did
        // everything it asked to, there was simply nothing to write.
        assert!(set_io_priority(DEAD_PID, "none", 0).is_ok());
    }

    #[test]
    fn a_recognised_class_reaches_the_kernel() {
        // Nothing is pre-empted for a valid class, so a dead PID surfaces the
        // kernel's own ESRCH. That proves the guard did not swallow it.
        match set_io_priority(DEAD_PID, "idle", 0) {
            Err(PlatformError::NotFound) => {}
            other => panic!("expected the kernel to reject a dead pid, got {other:?}"),
        }
    }

    /// `deadline` is a name the daemon understands, and it is never a policy it
    /// manages to apply — the attribute is dropped either way.
    ///
    /// The test process is asked to switch to the normal policy, which a
    /// restricted environment may refuse: the write is a priority change for
    /// whoever runs the suite at a non-negative nice, so a build sandbox without
    /// `CAP_SYS_NICE` answers `EPERM` instead. What holds regardless is that the
    /// name is *recognised* — an unknown one comes back as `Unsupported`, which
    /// aborts the whole rule — and that the attribute is never reported as
    /// applied.
    #[test]
    fn the_deadline_policy_is_recognised_but_never_applied() {
        // The *calling thread*, not the process. `sched_setscheduler(2)` is
        // per-thread, so asking for `std::process::id()` from a test running on
        // one of the harness' worker threads reached across and changed the
        // main thread's policy, which is shared with every other test in this
        // binary.
        let tid = rustix::thread::gettid().as_raw_pid();

        match set_sched(tid, &[tid], "deadline", 1) {
            Err(PlatformError::Skipped(reason)) => {
                assert!(reason.contains("deadline"), "the reason names it: {reason}");
            }
            Err(other) => assert!(
                !matches!(other, PlatformError::Unsupported),
                "`deadline` is a known policy, not an unknown one: {other:?}"
            ),
            Ok(()) => panic!("`deadline` was reported as applied"),
        }
    }
}

#[cfg(test)]
mod per_thread_tests {
    use super::*;

    /// The scheduling policy the kernel reports for one thread, as a digit:
    /// 0 `SCHED_OTHER`, 1 `SCHED_FIFO`, 2 `SCHED_RR`, 3 `SCHED_BATCH`,
    /// 5 `SCHED_IDLE`.
    ///
    /// `policy` is field 41 of `/proc/<pid>/task/<tid>/stat`, and field 2 is the
    /// `comm` in parentheses, so it is 41 − 3 = 38 fields after the closing
    /// parenthesis. `comm` may itself contain spaces and parentheses, so the
    /// *last* `)` is the one that ends it.
    fn policy_of(pid: i32, tid: i32) -> Option<String> {
        // A thread of the harness can retire between the snapshot and the read,
        // so a vanished one is reported as "no answer" rather than a failure.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/stat")).ok()?;
        stat.rsplit(')')
            .next()?
            .split_whitespace()
            .nth(38)
            .map(str::to_string)
    }

    fn tids_of(pid: i32) -> Vec<i32> {
        let mut tids: Vec<i32> = std::fs::read_dir(format!("/proc/{pid}/task"))
            .expect("a live process has a task directory")
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok()?.parse().ok())
            .collect();
        tids.sort();
        tids
    }

    /// A policy has to reach every thread of a process, not only the one whose
    /// pid the event happened to carry.
    ///
    /// `sched_setscheduler(2)` is per-thread. Passing the thread-group leader
    /// alone changed the policy of one of a multi-threaded process' threads and
    /// left the rest of them where they were, so `"sched": "batch"` or
    /// `"sched": "idle"` in a rule had a visible effect on the process as a whole
    /// only by accident of which thread the scheduler happened to be running. The
    /// reference daemon has the same limitation; that the two share it is not a
    /// reason to keep it.
    ///
    /// The multi-threaded process used is this test binary: the harness runs its
    /// tests on a pool of threads, so the process under test is guaranteed to
    /// have more than one, and `SCHED_BATCH` needs no privilege to enter. Every
    /// thread is put back the way it was found before the assertions run, so a
    /// failure cannot leave the rest of the suite on `SCHED_BATCH`.
    #[test]
    fn a_policy_reaches_every_thread_of_the_process() {
        let pid = std::process::id() as i32;
        let tids = tids_of(pid);
        assert!(
            tids.len() > 1,
            "the test harness should have run this on a pool of threads, got {tids:?}"
        );

        let before: Vec<(i32, String)> = tids
            .iter()
            .map(|&t| (t, policy_of(pid, t)))
            .filter_map(|(t, policy)| policy.map(|policy| (t, policy)))
            .collect();
        assert!(
            before.len() > 1,
            "not enough threads answered to say anything: {before:?}"
        );

        let outcome = set_sched(pid, &tids, "batch", 1);

        // The harness creates and retires threads as tests start and finish, so
        // only the ones that answer both reads are asserted on. That is enough:
        // the calling thread is always among them, and the property under test
        // is that a thread other than the caller moved too.
        let after: Vec<(i32, String)> = before
            .iter()
            .filter_map(|(tid, _)| policy_of(pid, *tid).map(|policy| (*tid, policy)))
            .collect();

        for (tid, policy) in &before {
            let name = match policy.as_str() {
                "3" => "batch",
                "5" => "idle",
                _ => "normal",
            };
            let _ = set_sched(pid, &[*tid], name, 1);
        }

        if let Err(e) = &outcome {
            // A sandbox that will not let the suite change scheduling policy is
            // not a failure of the code under test.
            assert!(
                matches!(e, PlatformError::PermissionDenied | PlatformError::Io(_)),
                "an unexpected failure: {e:?}"
            );
            return;
        }

        assert!(
            after.len() > 1,
            "not enough of the harness' threads survived to say anything: {after:?}"
        );
        let stale: Vec<&(i32, String)> = after.iter().filter(|(_, p)| p != "3").collect();
        assert!(
            stale.is_empty(),
            "every thread that answered must be on SCHED_BATCH; these are not: \
             {stale:?} (all of them: {after:?})"
        );
    }

    /// A thread that is no longer there is not a failed attribute.
    ///
    /// `ESRCH` from a tid the daemon listed in `/proc/<pid>/task` a moment ago
    /// says nothing about the threads that were still there. What the *live*
    /// thread answers is what the attribute did — here, a refusal, because the
    /// test binary runs at a negative nice and may not go back to 19. Either
    /// way the answer must not be `NotFound`, which the worker would report as
    /// a vanished process and skip.
    #[test]
    fn a_thread_that_vanished_does_not_mask_the_threads_that_did_not() {
        let pid = std::process::id() as i32;
        let outcome = set_priority(pid, &[i32::MAX, pid], 19);
        assert!(
            !matches!(outcome, Err(PlatformError::NotFound)),
            "one dead thread must not turn the whole attribute into a vanished \
             process: {outcome:?}"
        );
    }

    /// A whole process that is gone is still `NotFound` — the distinction the
    /// worker logs at debug level rather than at warning.
    #[test]
    fn a_process_that_is_gone_is_still_reported_as_gone() {
        let outcome = set_priority(i32::MAX, &[i32::MAX], 0);
        assert!(
            matches!(outcome, Err(PlatformError::NotFound)),
            "expected NotFound, got {outcome:?}"
        );
    }
}
