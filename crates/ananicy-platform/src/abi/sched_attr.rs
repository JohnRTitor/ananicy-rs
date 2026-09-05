use std::io;

// Scheduling policies (from linux/sched.h)
pub const SCHED_NORMAL: u32 = 0;
pub const SCHED_FIFO: u32 = 1;
pub const SCHED_RR: u32 = 2;
pub const SCHED_BATCH: u32 = 3;
pub const SCHED_IDLE: u32 = 5;
pub const SCHED_DEADLINE: u32 = 6;

// Flags for sched_setattr
pub const SCHED_FLAG_RESET_ON_FORK: u64 = 0x01;
pub const SCHED_FLAG_RECLAIM: u64 = 0x02;
pub const SCHED_FLAG_DL_OVERRUN: u64 = 0x04;
pub const SCHED_FLAG_KEEP_POLICY: u64 = 0x08;
pub const SCHED_FLAG_KEEP_PARAMS: u64 = 0x10;
pub const SCHED_FLAG_UTIL_CLAMP_MIN: u64 = 0x20;
pub const SCHED_FLAG_UTIL_CLAMP_MAX: u64 = 0x40;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct sched_attr {
    pub size: u32,
    pub sched_policy: u32,
    pub sched_flags: u64,
    pub sched_nice: i32,
    pub sched_priority: u32,
    pub sched_runtime: u64,
    pub sched_deadline: u64,
    pub sched_period: u64,
    pub sched_util_min: u32,
    pub sched_util_max: u32,
    pub sched_latency_nice: i32,
}

/// # Safety
///
/// Wraps the raw `sched_setattr` syscall. The `attr` reference must point to a
/// properly initialized `sched_attr` struct, and `size` must be correctly set.
pub fn sched_setattr(pid: i32, attr: &sched_attr, flags: u32) -> io::Result<()> {
    let ret = unsafe {
        libc::syscall(
            libc::SYS_sched_setattr,
            pid,
            attr as *const sched_attr,
            flags,
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// # Safety
///
/// Wraps the raw `sched_getattr` syscall. The `attr` reference must be a valid
/// allocated `sched_attr` where `size` indicates its capacity.
pub fn sched_getattr(pid: i32, attr: &mut sched_attr, size: u32, flags: u32) -> io::Result<()> {
    let ret = unsafe {
        libc::syscall(
            libc::SYS_sched_getattr,
            pid,
            attr as *mut sched_attr,
            size,
            flags,
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
