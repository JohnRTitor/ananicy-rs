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
/// The raw syscall takes three arguments: `pid`, a pointer to the attribute
/// struct, and flags. Unlike `sched_getattr` it has no separate size argument —
/// the byte count is carried in `attr.size`, which every caller initializes to
/// the struct's size. `flags` must be 0 on every kernel. An unknown `pid` fails
/// with `ESRCH` rather than causing undefined behaviour.
pub fn sched_setattr(pid: i32, attr: &sched_attr, flags: u32) -> io::Result<()> {
    // SAFETY: `attr` is a live reference and the kernel reads `attr.size` bytes
    // from it; every caller initializes that field to the struct's size.
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
    /// The two raw syscalls have different arities: `sched_setattr` is
    /// `(pid, attr, flags)` with the byte count inside `attr.size`, while
    /// `sched_getattr` is `(pid, attr, size, flags)`. Neither mistake is a
    /// compile error because `libc::syscall` is variadic: adding a size argument
    /// to the setter makes the kernel read it as flags, and omitting it from the
    /// getter truncates the call. Both fail quietly or misapply a setting.
    ///
    /// A source assertion is the only cheap guard: exercising the setter needs
    /// a kernel with the out-of-tree latency-nice extension and privileges.
    #[test]
    fn the_raw_syscall_argument_lists_match_the_kernel_abi() {
        let source = include_str!("sched_attr.rs");

        fn args_after<'a>(source: &'a str, marker: &str) -> Vec<&'a str> {
            source
                .split(marker)
                .nth(1)
                .expect("the syscall call site")
                .split("};")
                .next()
                .expect("the end of the call")
                .split(')')
                .next()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|arg| !arg.is_empty())
                .collect()
        }

        assert_eq!(
            args_after(source, "libc::SYS_sched_setattr,"),
            vec!["pid", "attr as *const sched_attr", "flags"]
        );
        assert_eq!(
            args_after(source, "libc::SYS_sched_getattr,"),
            vec!["pid", "attr as *mut sched_attr", "size", "flags"]
        );
    }
}
