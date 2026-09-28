use {
    ananicy_core::types::Pid,
    rustix::{fs::OFlags, io::Errno, time::Timespec},
    std::{
        os::fd::BorrowedFd,
        process::id,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    },
    tracing::warn,
};

use {
    neli::{
        connector::{CnMsg, ProcEvent, ProcEventHeader},
        consts::{
            connector::{CnMsgIdx, CnMsgVal, ProcCnMcastOp},
            nl::{NlmF, Nlmsg},
            socket::NlFamily,
        },
        nl::{NlPayload, NlmsghdrBuilder},
        socket::synchronous::NlSocketHandle,
        utils::Groups,
    },
    std::{io, sync::mpsc::Sender},
    tracing::{debug, error, info},
};

use {crate::procfs::get_command_from_pid, ananicy_core::process::Process};

/// What a failed receive on the netlink socket means for the drain loop.
#[derive(Debug, PartialEq, Eq)]
enum RecvFailure {
    /// The kernel dropped messages because the receive buffer overflowed. The
    /// stream has a gap in it, so the listener has to stop and be rebuilt.
    BufferOverrun,
    /// The non-blocking socket has nothing queued. This is the normal end of a
    /// drain, not a failure.
    Drained,
    /// Anything else. The socket is not usable and the caller reconnects.
    Fatal,
}

/// The errno behind a socket error, if it carries one.
///
/// The netlink library wraps an I/O failure in its own enum, so the errno has to
/// be dug out of the variant rather than read off the error directly.
fn errno_of(e: &neli::err::SocketError) -> Option<Errno> {
    match e {
        // An `io::Error` built from a custom `ErrorKind` carries no errno, and
        // that is reported as "no errno" rather than guessed at. It lands in the
        // fatal arm, which is where an unidentifiable error belongs.
        neli::err::SocketError::Io(io) => io.raw_os_error().map(Errno::from_raw_os_error),
        _ => None,
    }
}

/// Classifies a receive error by its errno.
///
/// This used to render the error to a string and look for "No buffer space
/// available", "ENOBUFS", "Resource temporarily unavailable", "EAGAIN" and
/// "WouldBlock" in the result. That couples the control flow to how one library
/// happens to phrase its errors: a wording change, a different rendering, or any
/// unrelated error whose text happens to contain one of those tokens would
/// silently change whether the loop drains or tears the listener down. The errno
/// is the thing being classified, so it is what is compared.
///
/// `EWOULDBLOCK` needs no separate case because on Linux it is `EAGAIN`; the
/// kernel returns one value under both names.
fn classify_recv_error(e: &neli::err::SocketError) -> RecvFailure {
    match errno_of(e) {
        Some(Errno::NOBUFS) => RecvFailure::BufferOverrun,
        Some(Errno::AGAIN) => RecvFailure::Drained,
        _ => RecvFailure::Fatal,
    }
}

/// Carries a socket error back as an `io::Error` with its errno intact.
///
/// The old code returned `io::Error::other(err_str)`, which keeps the message and
/// discards the code -- so a caller inspecting `raw_os_error` to decide whether to
/// retry saw nothing, and could only re-parse the same string.
fn to_io_error(e: &neli::err::SocketError) -> io::Error {
    match errno_of(e) {
        Some(errno) => io::Error::from_raw_os_error(errno.raw_os_error()),
        None => io::Error::other(e.to_string()),
    }
}

pub struct NetlinkMonitor {
    sock: NlSocketHandle,
}

/// A process' identity as the filter remembers it: a length and a digest, never
/// the name.
///
/// The bound that matters here is the *size* of an entry, not the count of them.
/// The process name is the basename of `argv[0]`, which the kernel does not
/// upper-bound — `MAX_ARG_STRLEN` is 128 KiB — and storing it verbatim at
/// `REPORTED_NAMES_CAPACITY` entries would let any unprivileged local process
/// make this cache cost a gigabyte against a `MemoryMax=96M`. Storing a digest
/// fixes the size at 16 bytes per entry, so the whole cache is bounded at about
/// 128 KiB no matter what names pass through it.
///
/// This is the same reasoning as `Rules::MAX_CACHEABLE_NAME`, arrived at the
/// second time; the first time it was applied to the rule cache and missed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    len: u32,
    digest: u64,
}

impl Identity {
    /// FNV-1a. Not chosen for cryptographic strength: it only has to tell one
    /// process' name from another's, and the cost of a collision is one process
    /// not being re-reported, not a security property.
    fn of(name: &str) -> Self {
        let mut digest = 0xcbf2_9ce4_8422_2325u64;
        for byte in name.as_bytes() {
            digest ^= u64::from(*byte);
            digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self {
            len: name.len() as u32,
            digest,
        }
    }
}

/// Decides which proc-connector events turn into work for the rule engine.
///
/// The proc connector delivers at least two, and often three, events for the
/// ordinary way a program starts:
///
/// ```text
/// FORK(p)   the child exists, but has not exec'd: /proc/p/* is the parent's
/// EXEC(p)   p has exec'd: this is the first event that carries p's real name
/// COMM(p)   p's `comm` was set, which exec also does
/// ```
///
/// A `FORK(p)` immediately followed by an `EXEC(p)` is the norm, not the
/// exception: it is what `posix_spawn`, `system(3)`, `sh -c` and the Go/Java
/// runtimes all do. Reporting only the first of them classifies every
/// `fork`+`exec`'d process under *its parent*, and the one event that would
/// have corrected that is the one a "same pid as last time" filter throws away.
///
/// So the filter is per process and name-based rather than positional: a pid is
/// reported when it is seen for the first time, or when the name resolved for it
/// now differs from the name it was last reported under. That keeps the
/// `COMM`-after-`EXEC` pair from being processed twice — the two resolve to the
/// same name — while letting a process that really did change its identity
/// through `execve` be reclassified.
///
/// Bounded so a long-running daemon under process churn cannot grow it without
/// limit; eviction only costs a redundant report of a still-live pid.
struct ReportedNames {
    seen: lru::LruCache<i32, Identity>,
}

/// How many pids' last-reported identities are remembered. Comfortably above the
/// number of concurrently live processes on a large machine, and small enough
/// that the bookkeeping is free. At 16 bytes of identity per entry the whole
/// cache is ~128 KiB at this size.
const REPORTED_NAMES_CAPACITY: usize = 8192;

impl ReportedNames {
    fn new() -> Self {
        Self {
            seen: lru::LruCache::new(
                std::num::NonZeroUsize::new(REPORTED_NAMES_CAPACITY)
                    .unwrap_or(std::num::NonZeroUsize::MIN),
            ),
        }
    }

    /// Records `name` as `pid`'s reported identity and answers whether the
    /// caller should hand the process to the worker.
    fn should_report(&mut self, pid: i32, name: &str) -> bool {
        let identity = Identity::of(name);
        match self.seen.get(&pid) {
            Some(previous) if *previous == identity => false,
            _ => {
                self.seen.put(pid, identity);
                true
            }
        }
    }
}

impl NetlinkMonitor {
    pub fn new() -> Result<Self, io::Error> {
        let pid = id();
        let sock = NlSocketHandle::connect(
            NlFamily::Connector,
            Some(pid),
            Groups::new_bitmask(CnMsgIdx::Proc.into()),
        )
        .map_err(|e| io::Error::other(format!("Netlink connect error: {}", e)))?;

        use std::os::unix::io::AsRawFd;
        let fd = sock.as_raw_fd();
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        if let Err(e) = rustix::net::sockopt::set_socket_recv_buffer_size(borrowed, 8 * 1024 * 1024)
        {
            warn!(
                "Failed to set Netlink SO_RCVBUF to 8MB: {}. (ENOBUFS may be more frequent)",
                e
            );
        }

        let subscribe = NlmsghdrBuilder::default()
            .nl_type(Nlmsg::Done)
            .nl_flags(NlmF::empty())
            .nl_pid(pid)
            .nl_payload(NlPayload::Payload(
                neli::connector::CnMsgBuilder::default()
                    .idx(CnMsgIdx::Proc)
                    .val(CnMsgVal::Proc)
                    .payload(ProcCnMcastOp::Listen)
                    .build()
                    .map_err(|e| io::Error::other(format!("CnMsg error: {}", e)))?,
            ))
            .build()
            .map_err(|e| io::Error::other(format!("Nlmsghdr error: {}", e)))?;

        sock.send(&subscribe)
            .map_err(|e| io::Error::other(format!("Netlink send error: {}", e)))?;

        Ok(Self { sock })
    }

    pub fn listen(
        &mut self,
        tx: Sender<Process>,
        shutdown_flag: Arc<AtomicBool>,
    ) -> Result<(), io::Error> {
        use {rustix::event::epoll, std::os::unix::io::AsRawFd};

        let fd = self.sock.as_raw_fd();
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };

        // Ensure non-blocking so recv doesn't hang if epoll wakes spuriously
        let flags = rustix::fs::fcntl_getfl(borrowed)?;
        rustix::fs::fcntl_setfl(borrowed, flags | OFlags::NONBLOCK)?;

        let epoll_fd = epoll::create(epoll::CreateFlags::CLOEXEC)?;
        epoll::add(
            &epoll_fd,
            borrowed,
            epoll::EventData::new_u64(1),
            epoll::EventFlags::IN,
        )?;

        let mut event_list: Vec<epoll::Event> = Vec::with_capacity(1);
        let mut reported = ReportedNames::new();

        info!("Starting epoll-based Netlink event loop");

        loop {
            if shutdown_flag.load(Ordering::Relaxed) {
                return Ok(());
            }

            let timeout = Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            };
            match epoll::wait(&epoll_fd, &mut event_list, Some(&timeout)) {
                Ok(_) => {
                    if event_list.is_empty() {
                        continue; // Timeout
                    }
                }
                Err(Errno::INTR) => continue,
                Err(e) => {
                    error!("epoll_wait error: {}", e);
                    continue;
                }
            }

            // Drain all available messages
            loop {
                let iter = match self.sock.recv::<Nlmsg, CnMsg<ProcEventHeader>>() {
                    Ok(msgs) => msgs.0,
                    Err(e) => match classify_recv_error(&e) {
                        RecvFailure::BufferOverrun => {
                            error!(
                                "Netlink recv error (ENOBUFS): buffer overrun. Stopping listener for recovery."
                            );
                            return Err(to_io_error(&e));
                        }
                        // Non-blocking mode returns EAGAIN once the queue is drained.
                        RecvFailure::Drained => break,
                        RecvFailure::Fatal => {
                            error!("Netlink recv error: {}", e);
                            return Err(to_io_error(&e));
                        }
                    },
                };

                for event in iter {
                    let event = match event {
                        Ok(e) => e,
                        Err(e) => {
                            error!("Netlink event error: {}", e);
                            continue;
                        }
                    };

                    let Some(payload) = event.get_payload() else {
                        continue;
                    };

                    // A `Fork` event and the `Exec` that follows it name the *same*
                    // pid, and they do not carry the same thing. At `Fork` time the
                    // child has not run `execve` yet, so its `mm` — and therefore
                    // its `/proc/<pid>/cmdline`, `/proc/<pid>/exe` and
                    // `/proc/<pid>/comm` — is still a copy of the parent's, and
                    // `get_command_from_pid` answers with the *parent's* name. The
                    // `Exec` event is the first and only notification that carries
                    // the name the rule engine is meant to match on.
                    //
                    // De-duplicating on "the pid differs from the previous one"
                    // therefore threw the `Exec` away in exactly the case it
                    // mattered: a `fork()` immediately followed by an `execve()`
                    // produces `FORK(p)` then `EXEC(p)`, and the second is dropped
                    // because the first has just set the marker. A shell that runs
                    // a single command gets the *shell's* rule applied and the
                    // command's own rule never runs. `ReportedNames` is the filter
                    // that replaces it.
                    let pid = match payload.payload().event {
                        ProcEvent::Exec { process_pid, .. }
                        | ProcEvent::Comm { process_pid, .. } => process_pid,
                        ProcEvent::Fork { child_pid, .. } => child_pid,
                        // Exit carries nothing to classify. It can be handled
                        // here if a use for it appears.
                        ProcEvent::Exit { .. } => continue,
                        _ => continue,
                    };

                    // The swapper/idle thread is PID 0. It is not a process, and
                    // matching a rule against it is never what anyone meant.
                    if pid == 0 {
                        continue;
                    }

                    let name = get_command_from_pid(pid);
                    if !reported.should_report(pid, &name) {
                        continue;
                    }

                    if tx
                        .send(Process::new(Pid(pid), name).with_authoritative_name())
                        .is_err()
                    {
                        // The receiver is gone, which means the worker has
                        // stopped and nothing is left to apply rules to.
                        // Tearing the monitor down lets the caller join the
                        // worker and shut down; panicking here would take the
                        // whole daemon down on the main thread instead.
                        warn!("Worker thread is gone, stopping the netlink listener");
                        shutdown_flag.store(true, Ordering::SeqCst);
                        return Ok(());
                    }
                }
            }
        }
    }
}

impl Drop for NetlinkMonitor {
    fn drop(&mut self) {
        let pid = id();
        if let Ok(unsubscribe) = NlmsghdrBuilder::default()
            .nl_type(Nlmsg::Done)
            .nl_flags(NlmF::empty())
            .nl_pid(pid)
            .nl_payload(NlPayload::Payload(
                neli::connector::CnMsgBuilder::default()
                    .idx(CnMsgIdx::Proc)
                    .val(CnMsgVal::Proc)
                    .payload(ProcCnMcastOp::Ignore)
                    .build()
                    .unwrap_or_else(|_| unreachable!()),
            ))
            .build()
        {
            let _ = self.sock.send(&unsubscribe);
            debug!("Netlink monitor unsubscribed successfully");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this filter exists to prevent, reproduced from a real event
    /// stream: a child is forked, sleeps, and only then execs.
    ///
    /// `/proc/<child>/cmdline` at the `Fork` event is the parent's copy, because
    /// `fork(2)` gives the child the parent's `mm` and the `execve(2)` has not
    /// run yet. `get_command_from_pid` therefore answers `slowexec`. The `Exec`
    /// that follows is the only event that ever says `sleep`.
    #[test]
    fn the_exec_that_follows_a_fork_is_not_thrown_away() {
        let mut reported = ReportedNames::new();

        assert!(
            reported.should_report(4242, "slowexec"),
            "the fork is reported, under the name visible at that moment"
        );
        assert!(
            reported.should_report(4242, "sleep"),
            "the exec that renames the process must be reported too, or the \
             process is tuned with its parent's rule for the rest of its life"
        );
    }

    /// The ordinary `fork` → `exec` → `comm` sequence resolves to the same name
    /// twice, so the third event is a repeat and costs nothing to skip. The
    /// `comm` the kernel emits after every `execve` would otherwise double the
    /// `/proc` reads for every single process the system starts.
    #[test]
    fn the_comm_after_an_exec_is_a_repeat_and_is_skipped() {
        let mut reported = ReportedNames::new();

        assert!(
            reported.should_report(1, "sh"),
            "fork, under the parent's name"
        );
        assert!(
            reported.should_report(1, "vim"),
            "exec, under the process' own name"
        );
        assert!(
            !reported.should_report(1, "vim"),
            "the comm the kernel emits after an execve resolves to the same \
             name and is a repeat"
        );
    }

    /// A process that renames itself with `prctl(PR_SET_NAME)` while keeping its
    /// `argv[0]`, and therefore while `get_command_from_pid` keeps answering
    /// with the same string, is not re-reported: the rule engine matched on
    /// `argv[0]` both times and there is nothing new to apply.
    #[test]
    fn an_unchanged_name_is_never_reported_twice() {
        let mut reported = ReportedNames::new();
        assert!(reported.should_report(7, "kworker"));
        for _ in 0..100 {
            assert!(!reported.should_report(7, "kworker"));
        }
    }

    /// The filter is per pid: interleaved processes do not suppress each other.
    #[test]
    fn different_pids_do_not_suppress_each_other() {
        let mut reported = ReportedNames::new();
        assert!(reported.should_report(100, "sh"));
        assert!(reported.should_report(200, "sh"));
        assert!(reported.should_report(100, "sleep"));
        assert!(reported.should_report(200, "sleep"));
        assert!(!reported.should_report(100, "sleep"));
        assert!(!reported.should_report(200, "sleep"));
    }

    /// A pid is reused by a new process. If the entry has already been evicted
    /// the new process is reported, as a first sighting. If the pid is *still*
    /// cached and the new process resolves to the same name, it is not — the
    /// filter cannot see the difference, because all it knows is a pid and a
    /// string.
    ///
    /// That gap is bounded and worth stating rather than hiding: the entry is
    /// only reachable while the pid is among the last 8192 processes seen, so
    /// reuse has to happen inside a burst that recent to be missed, and the
    /// initial `/proc` walk at start-up reports every process that existed
    /// before the listener was attached. The alternative — reporting on every
    /// event — costs one `/proc` read per thread creation for the lifetime of
    /// the daemon, which is the larger and more certain waste.
    #[test]
    fn eviction_lets_a_recycled_pid_be_reported_again() {
        let mut reported = ReportedNames::new();
        assert!(reported.should_report(1, "sh"));

        for pid in 2..=(REPORTED_NAMES_CAPACITY as i32 + 8) {
            assert!(reported.should_report(pid, "sh"));
        }

        assert!(
            reported.should_report(1, "sh"),
            "pid 1 fell out of the cache, so its next sighting is a first sighting"
        );
    }
}

/// The identity is fixed-size, so the cache cannot be made expensive by a long
/// `argv[0]`. This is the same bound as `Rules::MAX_CACHEABLE_NAME`, and it is
/// asserted here because the first version of this filter stored the name
/// verbatim and had exactly the hole BUG-014 describes.
#[test]
fn an_absurdly_long_name_does_not_grow_the_cache() {
    let mut reported = ReportedNames::new();

    // 128 KiB is `MAX_ARG_STRLEN`, the most a process can put in `argv[0]`.
    let huge = "z".repeat(128 * 1024);
    assert!(reported.should_report(1, &huge));

    // The point is not the name, it is that two absurd names are still told apart
    // and that a short name is not confused with a long one — a filter that
    // stored a truncated prefix would fail the second half of this.
    assert!(
        !reported.should_report(1, &huge),
        "the same absurd name is a repeat"
    );
    assert!(
        reported.should_report(1, &("z".repeat(128 * 1024 - 1) + "y")),
        "a different name of nearly the same length is not a repeat"
    );
    assert!(
        reported.should_report(2, "sh"),
        "a short name is not confused with a long one"
    );
    assert!(
        !reported.should_report(2, "sh"),
        "and is still de-duplicated normally"
    );
}

#[test]
fn the_identity_is_sixteen_bytes_regardless_of_the_name() {
    assert_eq!(
        std::mem::size_of::<Identity>(),
        16,
        "a fixed-size identity is what bounds the cache"
    );
}

#[cfg(test)]
mod recv_classification {
    use super::{RecvFailure, classify_recv_error, errno_of, to_io_error};
    use rustix::io::Errno;

    fn socket_error(errno: Errno) -> neli::err::SocketError {
        neli::err::SocketError::Io(std::sync::Arc::new(std::io::Error::from_raw_os_error(
            errno.raw_os_error(),
        )))
    }

    /// The two the loop branches on. Everything else tears the listener down, so
    /// a value landing in the wrong arm means either a dropped message or a
    /// reconnect on every idle pass.
    #[test]
    fn an_overrun_and_an_empty_queue_are_told_apart() {
        assert_eq!(
            classify_recv_error(&socket_error(Errno::NOBUFS)),
            RecvFailure::BufferOverrun
        );
        assert_eq!(
            classify_recv_error(&socket_error(Errno::AGAIN)),
            RecvFailure::Drained
        );
    }

    #[test]
    fn everything_else_is_fatal() {
        for e in [
            Errno::INTR,
            Errno::BADF,
            Errno::CONNREFUSED,
            Errno::PERM,
            Errno::PIPE,
        ] {
            assert_eq!(
                classify_recv_error(&socket_error(e)),
                RecvFailure::Fatal,
                "{e:?}"
            );
        }
    }

    /// The old test rendered the error and looked for five substrings. None of
    /// them is an errno, so each of them now has to classify as fatal: that is
    /// the whole point, since a message that happens to contain "ENOBUFS" must
    /// not read as a buffer overrun.
    #[test]
    fn the_classification_does_not_depend_on_the_error_text() {
        for text in [
            "No buffer space available",
            "ENOBUFS",
            "Resource temporarily unavailable",
            "EAGAIN",
            "WouldBlock",
        ] {
            let e = neli::err::SocketError::Msg(neli::err::MsgError::new(text));
            assert_eq!(
                classify_recv_error(&e),
                RecvFailure::Fatal,
                "{text:?} must not classify by its wording"
            );
        }
    }

    /// An error carrying no errno at all -- a parse or serialisation failure
    /// inside the library -- is not a drained queue and must not be read as one.
    #[test]
    fn an_error_carrying_no_errno_is_fatal() {
        let e = neli::err::SocketError::Msg(neli::err::MsgError::new("nope"));
        assert_eq!(errno_of(&e), None);
        assert_eq!(classify_recv_error(&e), RecvFailure::Fatal);
    }

    /// EWOULDBLOCK is the same value as EAGAIN on Linux, so the drained arm
    /// catches both spellings with one case. That is why the old code listed
    /// "WouldBlock" as a separate token to look for.
    #[test]
    fn ewould_block_is_the_same_value_as_eagain() {
        assert_eq!(Errno::WOULDBLOCK, Errno::AGAIN);
        assert_eq!(
            classify_recv_error(&socket_error(Errno::WOULDBLOCK)),
            RecvFailure::Drained
        );
    }

    /// The errno has to survive the trip back out, so a caller can decide whether
    /// to retry without re-parsing a message.
    #[test]
    fn the_errno_survives_conversion_back_to_an_io_error() {
        for e in [Errno::NOBUFS, Errno::AGAIN, Errno::CONNREFUSED] {
            let back = to_io_error(&socket_error(e));
            assert_eq!(back.raw_os_error(), Some(e.raw_os_error()), "{e:?}");
        }
    }
}
