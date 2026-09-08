use {
    crate::abi::sched::SchedParam,
    ananicy_core::worker::{
        PlatformError,
        PlatformError::{Io, NotFound, PermissionDenied, Unsupported},
    },
    rustix::io::Errno,
    tracing::error,
};

use crate::abi::{ioprio::*, sched_attr::*};

use {
    std::{fs, io},
    tracing::{debug, warn},
};

// Note: In C++ original code, `test_errno` handles EPERM, ESRCH, etc.
// We will replicate similar logic.
fn test_errno(err: io::Error, func_name: &str, pid: i32) -> Result<(), PlatformError> {
    if let Some(raw_os_error) = err.raw_os_error() {
        if raw_os_error == 0 {
            debug!("{}: Successfully applied to {}", func_name, pid);
            return Ok(());
        }

        if err.kind() == io::ErrorKind::NotFound
            || raw_os_error == Errno::SRCH.raw_os_error()
            || raw_os_error == Errno::ACCESS.raw_os_error()
            || raw_os_error == Errno::PERM.raw_os_error()
        {
            return Err(PermissionDenied);
        }
        return Err(Io(err));
    }

    Err(Io(err))
}

pub fn set_priority(pid: i32, tids: &[i32], nice_value: i32) -> Result<(), PlatformError> {
    use rustix::process::{Pid, setpriority_process};
    let mut last_err = None;

    for &tid in tids {
        let who = Pid::from_raw(tid);
        if let Err(e) = setpriority_process(who, nice_value) {
            last_err = Some(e.into());
        } else {
            last_err = Some(io::Error::from_raw_os_error(0));
        }
    }

    match last_err {
        Some(err) => test_errno(err, "set_priority", pid),
        None => test_errno(io::Error::from_raw_os_error(0), "set_priority", pid),
    }
}

pub fn set_latency_nice(
    pid: i32,
    tids: &[i32],
    latency_nice_value: i32,
) -> Result<(), PlatformError> {
    // LATENCY_NICE is applied via sched_setattr
    let mut last_err = None;

    // SCHED_FLAG_LATENCY_NICE (matching C++ exactly)
    const SCHED_FLAG_LATENCY_NICE: u64 = 0x80;
    const SCHED_FLAG_KEEP_PARAMS: u64 = 0x10;

    // ananicy_sched_attr in C++ had sched_latency_nice as the 11th field

    for &tid in tids {
        let attr = sched_attr {
            size: std::mem::size_of::<sched_attr>() as u32,
            sched_flags: SCHED_FLAG_LATENCY_NICE | SCHED_FLAG_KEEP_PARAMS,
            sched_latency_nice: latency_nice_value,
            ..Default::default()
        };

        if let Err(e) = crate::abi::sched_attr::sched_setattr(tid, &attr, 0) {
            last_err = Some(e);
        } else {
            last_err = Some(io::Error::from_raw_os_error(0));
        }
    }

    match last_err {
        Some(err) => test_errno(err, "set_latency_nice", pid),
        None => test_errno(io::Error::from_raw_os_error(0), "set_latency_nice", pid),
    }
}

pub fn get_latency_nice(pid: i32) -> Option<i32> {
    let mut attr = sched_attr {
        size: std::mem::size_of::<sched_attr>() as u32,
        ..Default::default()
    };

    if crate::abi::sched_attr::sched_getattr(
        pid,
        &mut attr,
        std::mem::size_of::<sched_attr>() as u32,
        0,
    )
    .is_err()
    {
        None
    } else {
        Some(attr.sched_latency_nice)
    }
}

pub fn set_io_priority(pid: i32, io_class: &str, value: i32) -> Result<(), PlatformError> {
    let io_class_value = match io_class {
        "best-effort" => IOPRIO_CLASS_BE,
        "realtime" => IOPRIO_CLASS_RT,
        "idle" => IOPRIO_CLASS_IDLE,
        "none" => IOPRIO_CLASS_NONE,
        _ => {
            return Err(Unsupported);
        }
    };

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
            warn!("deadline scheduler is not available yet, falling back to OTHER");
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
            error!("set_sched: Unknown error {} applying to {}", raw, pid);
        }
        test_errno(e, "set_sched", pid)
    } else {
        debug!("set_sched: Successfully applied to {}", pid);
        Ok(())
    }
}

pub fn set_oom_score_adjust(pid: i32, value: i32) -> Result<(), PlatformError> {
    let path = format!("/proc/{}/oom_score_adj", pid);
    if let Err(e) = fs::write(&path, value.to_string()) {
        if e.kind() == io::ErrorKind::NotFound {
            return Err(NotFound);
        }
        if e.kind() == io::ErrorKind::PermissionDenied {
            return Err(PermissionDenied);
        }
        Err(Io(e))
    } else {
        debug!("set_oom_score_adjust: Successfully applied to {}", pid);
        Ok(())
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
