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

/// Apply scheduling attributes with the raw `sched_setattr` syscall.
///
/// The syscall takes four arguments: `pid`, a pointer to the attribute struct,
/// the number of bytes the kernel may read from it, and flags. `attr.size`
/// carries the byte count, so callers must initialize it first. `flags` is the
/// syscall's own flags and must be 0 on kernels that define none. An unknown
/// `pid` fails with `ESRCH` rather than causing undefined behaviour.
pub fn sched_setattr(pid: i32, attr: &sched_attr, flags: u32) -> io::Result<()> {
    // SAFETY: `attr` is a live reference and `attr.size` is the byte count the
    // kernel is told to read; every caller initializes it to the struct's size.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_sched_setattr,
            pid,
            attr as *const sched_attr,
            attr.size,
            flags,
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Read scheduling attributes with the raw `sched_getattr` syscall.
///
/// The syscall takes `pid`, a pointer, the size of the caller's struct and
/// flags. All callers pass the struct's size. The kernel writes only the fields
/// its own struct defines, so on a kernel older than this struct the extra
/// fields are left untouched; that is not an error.
pub fn sched_getattr(pid: i32, attr: &mut sched_attr, size: u32, flags: u32) -> io::Result<()> {
    // SAFETY: `attr` is a live, writable reference and `size` is the byte count
    // the kernel is told to write; every caller passes the struct's size.
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

#[cfg(test)]
mod tests {
    /// The raw syscall is `sched_setattr(pid, attr, size, flags)` — four
    /// arguments. The glibc wrapper of the same name takes three and supplies
    /// `attr->size` itself, but `libc::syscall` is the raw entry point and
    /// supplies nothing. With only three arguments the kernel reads `size` from
    /// the flags slot (always 0 here) and rejects every call with EINVAL, which
    /// is invisible except that latency nice silently never works.
    ///
    /// A source assertion is the only cheap guard: exercising the syscall needs
    /// a kernel with the out-of-tree extension and suitable privileges.
    #[test]
    fn the_raw_setattr_call_passes_all_four_arguments() {
        let source = include_str!("sched_attr.rs");
        let call = source
            .split("libc::SYS_sched_setattr")
            .nth(1)
            .expect("the sched_setattr syscall call site")
            .split("};")
            .next()
            .expect("the end of the call");
        assert!(
            call.contains("attr.size"),
            "the third syscall argument must be the byte count the kernel reads"
        );
        assert!(
            call.contains("flags"),
            "the fourth syscall argument must be the syscall flags"
        );
    }
}
