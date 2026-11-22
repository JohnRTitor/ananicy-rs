use std::io;

// Scheduling policies (from linux/sched.h)
pub const SCHED_NORMAL: u32 = 0;
pub const SCHED_FIFO: u32 = 1;
pub const SCHED_RR: u32 = 2;
pub const SCHED_BATCH: u32 = 3;
pub const SCHED_ISO: u32 = 4;
pub const SCHED_IDLE: u32 = 5;
pub const SCHED_DEADLINE: u32 = 6;

// Flags for sched_setattr. These come from linux/sched.h except the 0x80
// latency-nice flag, which is an out-of-tree extension (defined where it is used
// in `priority.rs`) along with the `sched_latency_nice` field below.
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
    /// Out-of-tree extension: absent from upstream `struct sched_attr`, so a
    /// kernel that does not support it simply leaves it at zero.
    pub sched_latency_nice: i32,
}

/// # Safety
///
/// `attr` must point to a live `sched_attr`, and its `size` field must not
/// exceed `size_of::<sched_attr>()`, because the kernel reads that many bytes
/// from it. Callers initialize `size` with the full struct size. `flags` is the
/// syscall's own flags and must be 0 on kernels that define none. `pid` is an
/// ordinary argument: an unknown pid fails with `ESRCH`, it is not undefined
/// behaviour.
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
/// `attr` must point to a live, writable `sched_attr`, and `size` must not
/// exceed `size_of::<sched_attr>()`: the kernel writes up to `size` bytes through
/// the pointer, so a larger value would be an out-of-bounds write. Every caller
/// passes the struct's size. `flags` must be 0 on kernels that define none, and
/// an unknown `pid` merely fails the syscall.
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
