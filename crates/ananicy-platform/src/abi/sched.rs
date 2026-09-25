use std::io;

#[derive(Default)]
pub struct SchedParam {
    pub sched_priority: i32,
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
