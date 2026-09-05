use std::io;

pub const IOPRIO_CLASS_SHIFT: i32 = 13;
pub const IOPRIO_PRIO_MASK: i32 = (1 << IOPRIO_CLASS_SHIFT) - 1;

pub const IOPRIO_CLASS_NONE: i32 = 0;
pub const IOPRIO_CLASS_RT: i32 = 1;
pub const IOPRIO_CLASS_BE: i32 = 2;
pub const IOPRIO_CLASS_IDLE: i32 = 3;

pub const IOPRIO_WHO_PROCESS: i32 = 1;

#[inline]
pub const fn ioprio_prio_value(class: i32, data: i32) -> i32 {
    (class << IOPRIO_CLASS_SHIFT) | data
}

/// # Safety
///
/// Makes a raw `ioprio_set` syscall. The caller must ensure `which` and `who`
/// are valid, and `ioprio` is correctly formatted using `ioprio_prio_value`.
pub fn ioprio_set(which: i32, who: i32, ioprio: i32) -> io::Result<()> {
    let ret = unsafe { libc::syscall(libc::SYS_ioprio_set, which, who, ioprio) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// # Safety
///
/// Makes a raw `ioprio_get` syscall. The caller must ensure `which` and `who`
/// refer to valid targets.
pub fn ioprio_get(which: i32, who: i32) -> io::Result<i32> {
    let ret = unsafe { libc::syscall(libc::SYS_ioprio_get, which, who) };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret as i32)
    }
}
