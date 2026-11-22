use std::io;

#[derive(Default)]
pub struct SchedParam {
    pub sched_priority: i32,
}

/// # Safety
///
/// `pid` and `policy` are ordinary syscall arguments: unknown values fail with
/// an errno rather than causing undefined behaviour. Only the priority is taken
/// from `param` and copied into an initialized `libc::sched_param`, so nothing
/// uninitialized reaches the kernel. The call affects one thread, not the whole
/// process.
pub fn sched_setscheduler(pid: i32, policy: i32, param: &SchedParam) -> io::Result<()> {
    // SAFETY: `libc::sched_param` contains only integer fields, so an all-zero
    // bit pattern is a valid value for it.
    let mut libc_param = unsafe { std::mem::zeroed::<libc::sched_param>() };
    libc_param.sched_priority = param.sched_priority;

    // SAFETY: `libc_param` was initialized above and lives until the call
    // returns; `pid` and `policy` are scalars whose bad values produce an errno.
    let ret = unsafe { libc::sched_setscheduler(pid, policy, &libc_param) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
