use std::io::{Read, Write};

const IPC_NAME: &str = "/AnanicyRsMutex";

pub(crate) struct IpcSingletonGuard;

impl Drop for IpcSingletonGuard {
    fn drop(&mut self) {
        let _ = rustix::shm::unlink(IPC_NAME);
    }
}

pub(crate) fn force_remove_semaphore() -> ! {
    let _ = rustix::shm::unlink(IPC_NAME);
    tracing::info!("Force removed IPC semaphore. Exiting.");
    std::process::exit(0);
}

pub(crate) fn check_singleton() -> Result<IpcSingletonGuard, String> {
    use rustix::{
        fs::Mode,
        shm::{self, OFlags as ShmOFlags},
    };

    match shm::open(IPC_NAME, ShmOFlags::RDWR, Mode::empty()) {
        Ok(fd) => {
            let mut file = std::fs::File::from(fd);
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok()
                && let Ok(old_pid) = buf.trim().parse::<i32>()
            {
                return Err(format!("Another instance is running (PID: {})", old_pid));
            }
            Err("Another instance is running (PID: unknown)".to_string())
        }
        Err(rustix::io::Errno::NOENT) => {
            let fd = shm::open(
                IPC_NAME,
                ShmOFlags::CREATE | ShmOFlags::EXCL | ShmOFlags::RDWR,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|e| e.to_string())?;
            let mut file = std::fs::File::from(fd);
            let _ = write!(file, "{}", std::process::id());
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

    match shm::open(IPC_NAME, ShmOFlags::RDONLY, Mode::empty()) {
        Ok(fd) => {
            let mut file = std::fs::File::from(fd);
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok()
                && let Ok(old_pid) = buf.trim().parse::<i32>()
                && let Some(pid) = rustix::process::Pid::from_raw(old_pid)
            {
                if let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::USR1) {
                    eprintln!("Failed to send reload signal: {}", e);
                    std::process::exit(1);
                }
                println!("Reload signal sent to PID {}", old_pid);
                std::process::exit(0);
            }
            eprintln!("Unable to read PID from IPC singleton");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("Unable to reload. Ananicy is not running!");
            std::process::exit(1);
        }
    }
}
