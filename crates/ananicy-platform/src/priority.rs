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
    tracing::debug,
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

pub fn set_priority(pid: i32, tids: &[i32], nice_value: i32) -> Result<(), PlatformError> {
    use rustix::process::{Pid, setpriority_process};
    let mut first_err = None;

    for &tid in tids {
        let who = Pid::from_raw(tid);
        if let Err(e) = setpriority_process(who, nice_value)
            && first_err.is_none()
        {
            first_err = Some(e.into());
        }
    }

    test_errno(
        first_err.unwrap_or_else(|| io::Error::from_raw_os_error(0)),
        "set_priority",
        pid,
    )
}

pub fn set_latency_nice(
    pid: i32,
    tids: &[i32],
    latency_nice_value: i32,
) -> Result<(), PlatformError> {
    // LATENCY_NICE is applied via sched_setattr
    const SCHED_FLAG_LATENCY_NICE: u64 = 0x80;
    const SCHED_FLAG_KEEP_PARAMS: u64 = 0x10;
    let mut first_err = None;

    for &tid in tids {
        let attr = sched_attr {
            size: std::mem::size_of::<sched_attr>() as u32,
            sched_flags: SCHED_FLAG_LATENCY_NICE | SCHED_FLAG_KEEP_PARAMS,
            sched_latency_nice: latency_nice_value,
            ..Default::default()
        };

        if let Err(e) = crate::abi::sched_attr::sched_setattr(tid, &attr, 0)
            && first_err.is_none()
        {
            first_err = Some(e);
        }
    }

    test_errno(
        first_err.unwrap_or_else(|| io::Error::from_raw_os_error(0)),
        "set_latency_nice",
        pid,
    )
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

pub fn set_sched(pid: i32, sched_name: &str, rt_prio: u32) -> Result<(), PlatformError> {
    let mut param = SchedParam::default();
    let mut skipped = false;

    let sched = match sched_name {
        "idle" => SCHED_IDLE,
        "normal" | "other" => SCHED_NORMAL,
        "rr" => {
            param.sched_priority = rt_prio as i32;
            SCHED_RR
        }
        "fifo" => {
            param.sched_priority = rt_prio as i32;
            SCHED_FIFO
        }
        "deadline" => {
            debug!("deadline scheduler is not available yet, falling back to OTHER");
            skipped = true;
            SCHED_NORMAL
        }
        "batch" => SCHED_BATCH,
        _ => {
            return Err(Unsupported);
        }
    };

    if let Err(e) = crate::abi::sched::sched_setscheduler(pid, sched as i32, &param) {
        if let Some(raw) = e.raw_os_error()
            && raw != 0
            && raw != Errno::SRCH.raw_os_error()
            && raw != Errno::PERM.raw_os_error()
            && raw != Errno::ACCESS.raw_os_error()
        {
            debug!("set_sched: Unknown error {} applying to {}", raw, pid);
        }
        test_errno(e, "set_sched", pid)
    } else {
        debug!("set_sched: Successfully applied to {}", pid);
        if skipped {
            Err(PlatformError::Skipped(
                "deadline scheduler is unavailable".to_string(),
            ))
        } else {
            Ok(())
        }
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
}
