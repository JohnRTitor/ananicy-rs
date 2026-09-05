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

pub struct NetlinkMonitor {
    sock: NlSocketHandle,
}

impl NetlinkMonitor {
    pub fn new() -> Result<Self, io::Error> {
        let pid = std::process::id();
        let sock = NlSocketHandle::connect(
            NlFamily::Connector,
            Some(pid),
            Groups::new_bitmask(CnMsgIdx::Proc.into()),
        )
        .map_err(|e| io::Error::other(format!("Netlink connect error: {}", e)))?;

        use std::os::unix::io::AsRawFd;
        let fd = sock.as_raw_fd();
        let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        if let Err(e) = rustix::net::sockopt::set_socket_recv_buffer_size(borrowed, 8 * 1024 * 1024) {
            tracing::warn!("Failed to set Netlink SO_RCVBUF to 8MB: {}. (ENOBUFS may be more frequent)", e);
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
        shutdown_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), io::Error> {
        use rustix::event::epoll;
        use std::os::unix::io::AsRawFd;
        
        let fd = self.sock.as_raw_fd();
        let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };

        // Ensure non-blocking so recv doesn't hang if epoll wakes spuriously
        let flags = rustix::fs::fcntl_getfl(borrowed)?;
        rustix::fs::fcntl_setfl(borrowed, flags | rustix::fs::OFlags::NONBLOCK)?;

        let epoll_fd = epoll::create(epoll::CreateFlags::CLOEXEC)?;
        epoll::add(
            &epoll_fd,
            borrowed,
            epoll::EventData::new_u64(1),
            epoll::EventFlags::IN,
        )?;

        let mut event_list: Vec<epoll::Event> = Vec::with_capacity(1);
        let mut prev_pid = 0;
        
        info!("Starting epoll-based Netlink event loop");
        
        loop {
            if shutdown_flag.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(());
            }

            let timeout = rustix::time::Timespec { tv_sec: 0, tv_nsec: 100_000_000 };
            match epoll::wait(&epoll_fd, &mut event_list, Some(&timeout)) {
                Ok(_) => {
                    if event_list.is_empty() {
                        continue; // Timeout
                    }
                }
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => {
                    error!("epoll_wait error: {}", e);
                    continue;
                }
            }

            // Drain all available messages
            loop {
                let iter = match self.sock.recv::<Nlmsg, CnMsg<ProcEventHeader>>() {
                    Ok(msgs) => msgs.0,
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("No buffer space available") || err_str.contains("ENOBUFS")
                        {
                            error!(
                                "Netlink recv error (ENOBUFS): buffer overrun. Stopping listener for recovery."
                            );
                            return Err(io::Error::other("ENOBUFS"));
                        }
                        // Non-blocking mode returns EAGAIN / WouldBlock when empty
                        if err_str.contains("Resource temporarily unavailable")
                            || err_str.contains("EAGAIN")
                            || err_str.contains("WouldBlock")
                        {
                            break; // Done draining
                        }
                        error!("Netlink recv error: {}", e);
                        return Err(io::Error::other(err_str));
                    }
                };

                for event in iter {
                    let event = match event {
                        Ok(e) => e,
                        Err(e) => {
                            error!("Netlink event error: {}", e);
                            continue;
                        }
                    };

                    let payload = match event.get_payload() {
                        Some(p) => p,
                        None => continue,
                    };

                    match payload.payload().event {
                        ProcEvent::Exec { process_tgid, .. } => {
                            if process_tgid != prev_pid {
                                prev_pid = process_tgid;
                                let name = get_command_from_pid(process_tgid);
                                tx.send(Process::new(ananicy_core::types::Pid(process_tgid), name))
                                    .expect("Worker thread died");
                            }
                        }
                        ProcEvent::Fork { child_tgid, .. } => {
                            if child_tgid != prev_pid {
                                prev_pid = child_tgid;
                                let name = get_command_from_pid(child_tgid);
                                tx.send(Process::new(ananicy_core::types::Pid(child_tgid), name))
                                    .expect("Worker thread died");
                            }
                        }
                        ProcEvent::Comm { process_tgid, .. } => {
                            if process_tgid != prev_pid {
                                prev_pid = process_tgid;
                                let name = get_command_from_pid(process_tgid);
                                tx.send(Process::new(ananicy_core::types::Pid(process_tgid), name))
                                    .expect("Worker thread died");
                            }
                        }
                        ProcEvent::Exit { .. } => {
                            // We can send Exit if needed in the future
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

impl Drop for NetlinkMonitor {
    fn drop(&mut self) {
        let pid = std::process::id();
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
