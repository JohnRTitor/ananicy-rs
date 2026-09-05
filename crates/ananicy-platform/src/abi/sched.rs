use std::io;

pub const SCHED_FIFO: i32 = libc::SCHED_FIFO;
pub const SCHED_RR: i32 = libc::SCHED_RR;

pub struct SchedParam {
    pub sched_priority: i32,
}

impl Default for SchedParam {
    fn default() -> Self {
        Self { sched_priority: 0 }
    }
}

/// # Safety
///
/// Wraps `libc::sched_setscheduler`.
pub fn sched_setscheduler(pid: i32, policy: i32, param: &SchedParam) -> io::Result<()> {
    let mut libc_param = unsafe { std::mem::zeroed::<libc::sched_param>() };
    libc_param.sched_priority = param.sched_priority;
    
    let ret = unsafe { libc::sched_setscheduler(pid, policy, &libc_param) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// # Safety
///
/// Wraps `libc::sched_getscheduler`. It returns the current scheduling policy or
/// an error if the process does not exist or permissions are insufficient.
pub fn sched_getscheduler(pid: i32) -> io::Result<i32> {
    let ret = unsafe { libc::sched_getscheduler(pid) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}
