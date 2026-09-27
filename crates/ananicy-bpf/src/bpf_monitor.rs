use {
    ananicy_core::types::Pid,
    std::{
        ffi::CStr,
        mem::MaybeUninit,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering::Relaxed},
        },
    },
};

use {
    libbpf_rs::{
        PerfBufferBuilder, PrintLevel,
        skel::{OpenSkel, Skel, SkelBuilder},
    },
    std::{io, sync::mpsc::Sender, time::Duration},
    tracing::{error, info},
};

use ananicy_core::process::Process;

use crate::ananicy_cpp::*;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct Event {
    pid: i32,
    prev_pid: i32,
    delta_us: u64,
    task: [u8; 16],
}

/// Whether an event names a process the daemon can do anything about.
///
/// PID 0 is the swapper, which the kernel names on every CPU, and it is not a
/// process: there is nothing to renice and a rule read out of it would be a
/// coincidence.
///
/// The `prev_pid` comparison that used to sit here is gone, and must not come
/// back. `prev_pid` is the pid of the previous event *on the same CPU*, so the
/// test discarded the second of any two events that named the same process —
/// and two events in a row is the shape of every Nix-wrapped program, whose
/// `bin/foo` script execs `.foo-wrapped` a moment later. The name that second
/// event carries is the only one that names the program rather than its
/// interpreter, so discarding it is how a `#!/bin/bash` wrapper came to be
/// matched as `bash` and given the `bash` rule. It turned actively wrong once
/// fork events named the child instead of the parent: a fork and the child's
/// exec then share a pid, and the filter ate the exec.
fn is_reportable(event: &Event) -> bool {
    event.pid != 0
}

pub struct BpfMonitor {
    // We must keep the skeleton alive so the BPF program stays attached.
    #[allow(dead_code)]
    skel: AnanicyCppSkel<'static>,
}

/// libbpf's own diagnostics, forwarded to stderr when `--verbose` is given.
///
/// These are the only place the reason for a refused load or attach is printed,
/// and libbpf says nothing above debug on its own, so the daemon's log level
/// does not reach them.
fn print_libbpf_message(_level: PrintLevel, message: String) {
    eprint!("{message}");
}

impl BpfMonitor {
    /// Loads and attaches the tracepoint programs.
    ///
    /// `verbose` mirrors `--verbose` in the reference: it turns on libbpf's own
    /// diagnostics, which are the only way to see why a load or an attach was
    /// refused (a missing BTF, a tracepoint the kernel does not have, a missing
    /// `CAP_BPF`).
    pub fn new(min_us: Option<u32>, verbose: bool) -> Result<Self, io::Error> {
        if verbose {
            libbpf_rs::set_print(Some((PrintLevel::Debug, print_libbpf_message)));
        }

        let skel_builder = AnanicyCppSkelBuilder::default();
        let open_object = Box::leak(Box::new(MaybeUninit::uninit()));
        let mut open_skel = skel_builder
            .open(open_object)
            .map_err(|e| io::Error::other(format!("Failed to open BPF skeleton: {}", e)))?;

        if let Some(min) = min_us
            && let Some(rodata) = open_skel.maps.rodata_data.as_mut()
        {
            // Set the rate limit in BPF to prevent context switch event storms
            rodata.min_us = min as u64;
        }

        let mut skel = open_skel
            .load()
            .map_err(|e| io::Error::other(format!("Failed to load BPF skeleton: {}", e)))?;

        skel.attach()
            .map_err(|e| io::Error::other(format!("Failed to attach BPF skeleton: {}", e)))?;

        info!("BPF Monitor initialized successfully.");

        Ok(Self { skel })
    }

    pub fn listen(&mut self, tx: Sender<Process>, shutdown_flag: Arc<AtomicBool>) {
        let tx_clone = tx.clone();
        let shutdown_on_channel_close = shutdown_flag.clone();

        // Setup the PerfBuffer
        let perf_buffer = PerfBufferBuilder::new(&self.skel.maps.events)
            .sample_cb(move |_cpu: i32, data: &[u8]| {
                if data.len() < std::mem::size_of::<Event>() {
                    return;
                }

                // SAFETY: We checked `data.len()` above to ensure we have enough bytes.
                // We use `read_unaligned` because the BPF perf buffer may not provide
                // the alignment required for `Event`, which would trigger UB on some archs.
                let event = unsafe { std::ptr::read_unaligned(data.as_ptr() as *const Event) };

                // Ignore PID 0 (swapper/idle thread) to prevent warning floods
                if !is_reportable(&event) {
                    return;
                }

                // We do NOT call get_command_from_pid here because it performs slow file I/O
                // which blocks the BPF polling loop, causing thousands of lost events under load.
                // We just extract the 16-byte task name from the event. The worker thread will
                // resolve the full command line name using procfs asynchronously.
                let name = match CStr::from_bytes_until_nul(&event.task) {
                    Ok(cstr) => cstr.to_string_lossy().into_owned(),
                    Err(_) => String::from_utf8_lossy(&event.task).into_owned(),
                };

                let mut p = Process::new(Pid(event.pid), name);
                p.delta_us = Some(event.delta_us);
                if tx_clone.send(p).is_err() {
                    shutdown_on_channel_close.store(true, Relaxed);
                }
            })
            .lost_cb(|cpu: i32, count: u64| {
                error!("Lost {} BPF events on CPU {}", count, cpu);
            })
            .build();

        let Ok(perf_buffer) =
            perf_buffer.inspect_err(|e| error!("Failed to build BPF perf buffer: {}", e))
        else {
            return;
        };

        // Polling loop
        loop {
            if shutdown_flag.load(Relaxed) {
                break;
            }
            if let Err(e) = perf_buffer.poll(Duration::from_millis(100)) {
                error!("Error polling BPF perf buffer: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(pid: i32, prev_pid: i32) -> Event {
        Event {
            pid,
            prev_pid,
            delta_us: 0,
            task: [0; 16],
        }
    }

    /// A process that execs twice in a row is the ordinary case on NixOS, where
    /// every program is a wrapper script that execs the real binary. Both events
    /// have to reach the worker: the second one is the only event whose name
    /// names the program rather than its interpreter.
    #[test]
    fn a_second_event_for_the_same_process_is_still_reported() {
        assert!(is_reportable(&event(4143, 4143)));
        assert!(is_reportable(&event(4143, 2898)));
    }

    /// A fork and the exec that follows it name the same pid now, which is the
    /// other half of why the same-pid test had to go.
    #[test]
    fn a_fork_and_the_exec_after_it_are_both_reported() {
        assert!(is_reportable(&event(4143, 2898)));
        assert!(is_reportable(&event(4143, 4143)));
    }

    #[test]
    fn the_swapper_is_not_reported() {
        assert!(!is_reportable(&event(0, 0)));
    }
}
