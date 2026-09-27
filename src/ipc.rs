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
            let holder = file
                .read_to_string(&mut buf)
                .ok()
                .and_then(|_| buf.trim().parse::<i32>().ok());

            // The object existing is not the same as the daemon running: a
            // `SIGKILL`, or a `panic = "abort"` before the guard's `Drop`, leaves
            // the name behind with a pid in it that will eventually be somebody
            // else's. Reporting that as "another instance is running" blocks a
            // restart until an operator runs `--force-remove-semaphore`, so the
            // distinction is made here, once, and stated either way.
            match holder {
                Some(pid) if looks_like_this_daemon(pid) => {
                    Err(format!("Another instance is running (PID: {pid})"))
                }
                Some(pid) => Err(format!(
                    "A stale singleton object left behind by a dead instance \
                     (PID: {pid}) is in the way. Remove it with: \
                     ananicy-rs --force-remove-semaphore"
                )),
                None => Err("Another instance is running (PID: unknown)".to_string()),
            }
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

/// Sends `SIGUSR1` to the running daemon, which makes it reload its
/// configuration.
///
/// The pid comes out of the singleton object, so it is only as trustworthy as the
/// object's lifetime. A daemon killed with `SIGKILL` — or one whose `Drop` never
/// ran because `panic = "abort"` took it — leaves `/dev/shm/AnanicyRsMutex`
/// behind, and the pid in it eventually belongs to something else. `SIGUSR1`'s
/// default disposition is to terminate, so `--reload` run at the wrong moment
/// would kill an unrelated process.
///
/// The reference avoids the problem by *writing* `RELOAD` into the shared memory
/// and letting the running instance notice, which needs no pid at all. The pid is
/// kept here because the object is the only handshake available, so it is
/// verified first: the pid must still exist, and `/proc/<pid>/` must still look
/// like this daemon.
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
        if !looks_like_this_daemon(pid.as_raw_pid()) {
            eprintln!(
                "The singleton object names pid {old_pid}, which is not a running \
                 ananicy-rs (it is a leftover from a crash, and that pid now belongs \
                 to something else). Refusing to signal it."
            );
            eprintln!("Remove the stale object with: ananicy-rs --force-remove-semaphore");
            exit(1);
        }
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

/// Whether `pid` is a live process that is plausibly this daemon.
///
/// The singleton is a fixed name in `/dev/shm` with a pid in it and no ownership
/// or generation check, so "the object exists" and "the daemon is running" are
/// different claims. This is the cheapest thing that tells them apart: the pid
/// has to exist, and it has to be an `ananicy-rs`. A process that has exited but
/// not been reaped — a zombie — has an empty `cmdline`, so it is rejected too,
/// which is the right answer: signalling a zombie does nothing and the daemon is
/// not there to reload.
/// Whether `pid` is a running `ananicy-rs`, decided by its first `argv`
/// element.
///
/// This guards two destructive paths — refusing to start because "another
/// instance is running", and sending `SIGUSR1` to whatever the singleton object
/// names — so a false positive is not a cosmetic problem. It has to be exactly
/// `ananicy-rs` and nothing else:
///
/// * The whole of `/proc/pid/cmdline` is never used. A process is free to have
///   `ananicy-rs` anywhere in its arguments, and `sh -c 'ananicy-rs --reload'`
///   has it as the *second* element with `sh` first. Matching the full command
///   line would let an unrelated shell be signalled and would let it block a
///   real start-up.
/// * Only the first element is read, and only its `file_name`, so
///   `/usr/bin/ananicy-rs` matches while `ananicy-rs-debug` does not.
fn looks_like_this_daemon(pid: i32) -> bool {
    proc_entry_looks_like_daemon(std::path::Path::new("/proc"), pid)
}

/// The same predicate with the procfs root as a parameter, so each shape can be
/// tested without needing a real process of that shape to exist.
fn proc_entry_looks_like_daemon(proc_root: &std::path::Path, pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let cmdline = proc_root.join(pid.to_string()).join("cmdline");
    match std::fs::read(cmdline) {
        Ok(raw) => {
            let name: Vec<u8> = raw
                .split(|&b| b == 0)
                .find(|a| !a.is_empty())
                .unwrap_or(&[])
                .to_vec();
            let name = String::from_utf8_lossy(&name);
            std::path::Path::new(&*name)
                .file_name()
                .is_some_and(|f| f == "ananicy-rs")
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::proc_entry_looks_like_daemon;
    use std::io::Write;
    use std::path::Path;

    /// Writes a `/proc`-shaped tree with one process's `cmdline`, where the
    /// elements are NUL-separated exactly as the kernel reports them.
    fn cmdline(dir: &Path, pid: i32, argv: &[&str]) {
        let entry = dir.join(pid.to_string());
        std::fs::create_dir_all(&entry).expect("a process directory");
        let mut raw = Vec::new();
        for arg in argv {
            raw.extend_from_slice(arg.as_bytes());
            raw.push(0);
        }
        let mut file = std::fs::File::create(entry.join("cmdline")).expect("a cmdline file");
        file.write_all(&raw).expect("cmdline bytes");
    }

    #[test]
    fn a_bare_name_is_this_daemon() {
        let dir = tempfile::tempdir().expect("a proc root");
        cmdline(dir.path(), 100, &["ananicy-rs"]);
        assert!(proc_entry_looks_like_daemon(dir.path(), 100));
    }

    #[test]
    fn an_absolute_path_is_this_daemon() {
        let dir = tempfile::tempdir().expect("a proc root");
        cmdline(dir.path(), 100, &["/usr/bin/ananicy-rs", "--systemd"]);
        assert!(proc_entry_looks_like_daemon(dir.path(), 100));
    }

    /// The regression: a shell whose *second* argument mentions us is not us.
    /// Comparing the whole command line — as the pre-fix code did — reported
    /// this pid as a running daemon, which both blocked a legitimate start and
    /// pointed `SIGUSR1` at a shell.
    #[test]
    fn a_shell_that_merely_mentions_us_is_not_this_daemon() {
        let dir = tempfile::tempdir().expect("a proc root");
        cmdline(
            dir.path(),
            100,
            &["/bin/sh", "-c", "exec ananicy-rs --reload"],
        );
        assert!(!proc_entry_looks_like_daemon(dir.path(), 100));
    }

    #[test]
    fn a_similarly_named_binary_is_not_this_daemon() {
        let dir = tempfile::tempdir().expect("a proc root");
        cmdline(dir.path(), 100, &["/usr/bin/ananicy-rs-debug"]);
        assert!(!proc_entry_looks_like_daemon(dir.path(), 100));
    }

    #[test]
    fn a_process_with_no_arguments_is_not_this_daemon() {
        let dir = tempfile::tempdir().expect("a proc root");
        cmdline(dir.path(), 100, &[]);
        assert!(!proc_entry_looks_like_daemon(dir.path(), 100));
    }

    #[test]
    fn a_pid_that_does_not_exist_is_not_this_daemon() {
        let dir = tempfile::tempdir().expect("a proc root");
        assert!(!proc_entry_looks_like_daemon(dir.path(), 100));
    }

    /// `pid` is used as a path component, so a non-positive one must be refused
    /// before it can name anything.
    #[test]
    fn a_non_positive_pid_is_never_this_daemon() {
        let dir = tempfile::tempdir().expect("a proc root");
        cmdline(dir.path(), 1, &["ananicy-rs"]);
        for pid in [0, -1, i32::MIN] {
            assert!(!proc_entry_looks_like_daemon(dir.path(), pid));
        }
    }
}
