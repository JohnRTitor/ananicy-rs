use {
    rustix::{
        io::Errno,
        process::{Pid, Signal},
    },
    std::{
        fs::File,
        process::{exit, id},
    },
    tracing::{error, info},
};

use std::io::{Read, Write};

const IPC_NAME: &str = "/AnanicyRsMutex";

pub(crate) struct IpcSingletonGuard;

impl Drop for IpcSingletonGuard {
    fn drop(&mut self) {
        let _ = rustix::shm::unlink(IPC_NAME);
    }
}

/// Removes a stale singleton object and exits.
///
/// The exit status is the answer to "is there still a stale object?", so a
/// failure to remove one is a failure, not a shrug: the reference prints the
/// errno and returns `EXIT_FAILURE` (`main.cpp:115-119`), and a wrapper script
/// that uses the status to confirm cleanup would otherwise be told the cleanup
/// worked.
pub(crate) fn force_remove_semaphore() -> ! {
    if let Err(errno) = rustix::shm::unlink(IPC_NAME) {
        error!("Failed to remove semaphore: {errno}");
        exit(1);
    }
    info!("Semaphore was successfully removed!");
    exit(0);
}

pub(crate) fn check_singleton() -> Result<IpcSingletonGuard, String> {
    use rustix::{
        fs::Mode,
        shm::{self, OFlags as ShmOFlags},
    };

    match shm::open(IPC_NAME, ShmOFlags::RDWR, Mode::empty()) {
        Ok(fd) => {
            let mut file = File::from(fd);
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok()
                && let Ok(old_pid) = buf.trim().parse::<i32>()
            {
                return Err(format!("Another instance is running (PID: {})", old_pid));
            }
            Err("Another instance is running (PID: unknown)".to_string())
        }
        Err(Errno::NOENT) => {
            let fd = shm::open(
                IPC_NAME,
                ShmOFlags::CREATE | ShmOFlags::EXCL | ShmOFlags::RDWR,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|e| e.to_string())?;
            let mut file = File::from(fd);
            let _ = write!(file, "{}", id());
            Ok(IpcSingletonGuard)
        }
        Err(e) => Err(format!("Failed to open shm: {}", e)),
    }
}

pub(crate) fn request_reload() -> ! {
    use rustix::{
        fs::Mode,
        shm::{self, OFlags as ShmOFlags},
    };

    let Ok(fd) = shm::open(IPC_NAME, ShmOFlags::RDONLY, Mode::empty()) else {
        eprintln!("Unable to reload. Ananicy is not running!");
        exit(1);
    };

    let mut file = File::from(fd);
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_ok()
        && let Ok(old_pid) = buf.trim().parse::<i32>()
        && let Some(pid) = Pid::from_raw(old_pid)
    {
        if let Err(e) = rustix::process::kill_process(pid, Signal::USR1) {
            eprintln!("Failed to send reload signal: {}", e);
            exit(1);
        }
        println!("Reload signal sent to PID {}", old_pid);
        exit(0);
    }
    eprintln!("Unable to read PID from IPC singleton");
    exit(1);
}
