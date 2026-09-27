pub mod cgroup;
pub mod config;
pub mod cpuset;
pub mod process;
pub mod rules;
pub mod types;
pub mod worker;

/// Attaches the thread's name to a spawn failure.
///
/// A thread-spawn failure is `EAGAIN` — the process or cgroup thread limit, or
/// `threads-max` — which is a reachable state on a busy machine, not a bug. The
/// errno on its own does not say which of the daemon's threads ran out of room,
/// and there are six of them across four files, so the name is the useful part.
pub fn spawn_error(name: &str, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("failed to spawn the {name} thread: {e}"))
}

/// Spawns a named thread, returning the `io::Result` rather than unwinding on
/// failure.
///
/// Whether a given call site may continue without its thread is a decision for
/// that site: the signal and worker threads are the daemon's control channel and
/// its only source of work, so those propagate; an initial full scan does not,
/// because the periodic scan will cover the same ground.
#[macro_export]
macro_rules! spawn_named_thread {
    ($name:expr, $f:expr) => {
        std::thread::Builder::new()
            .name($name.into())
            .spawn($f)
            .map_err(|e| $crate::spawn_error($name, e))
    };
}

#[cfg(test)]
mod tests {
    use super::spawn_error;
    use rustix::io::Errno;

    /// The errno alone does not say which of the daemon'"'"'s six threads ran out
    /// of room, so the name has to survive into the message.
    #[test]
    fn a_spawn_failure_names_the_thread_and_keeps_the_errno_kind() {
        let err = spawn_error(
            "ananicy-worker",
            std::io::Error::from_raw_os_error(Errno::AGAIN.raw_os_error()),
        );
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        let text = err.to_string();
        assert!(
            text.contains("ananicy-worker"),
            "the message must name the thread, got: {text}"
        );
        assert!(
            text.contains(&Errno::AGAIN.raw_os_error().to_string()),
            "the message must keep the errno, got: {text}"
        );
    }
}
