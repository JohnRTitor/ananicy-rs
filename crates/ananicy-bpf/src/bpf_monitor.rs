use {
    libbpf_rs::{
        PerfBufferBuilder,
        skel::{OpenSkel, Skel, SkelBuilder},
    },
    std::{io, sync::mpsc::Sender, time::Duration},
    tracing::{error, info},
};

use ananicy_core::process::Process;

use {crate::ananicy_cpp::*, ananicy_platform::procfs::get_command_from_pid};

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct Event {
    pid: i32,
    prev_pid: i32,
    delta_us: u64,
    task: [u8; 16],
}

pub struct BpfMonitor {
    // We must keep the skeleton alive so the BPF program stays attached.
    #[allow(dead_code)]
    skel: AnanicyCppSkel<'static>,
}

impl BpfMonitor {
    pub fn new(min_us: Option<u32>) -> Result<Self, io::Error> {
        let skel_builder = AnanicyCppSkelBuilder::default();
        let open_object = Box::leak(Box::new(std::mem::MaybeUninit::uninit()));
        let mut open_skel = skel_builder
            .open(open_object)
            .map_err(|e| io::Error::other(format!("Failed to open BPF skeleton: {}", e)))?;

        if let Some(_min) = min_us {
            // Set the rate limit in BPF to prevent context switch event storms
            if let Some(rodata) = open_skel.maps.rodata_data.as_mut() {
                rodata.min_us = _min as u64;
            }
        }

        let mut skel = open_skel
            .load()
            .map_err(|e| io::Error::other(format!("Failed to load BPF skeleton: {}", e)))?;

        skel.attach()
            .map_err(|e| io::Error::other(format!("Failed to attach BPF skeleton: {}", e)))?;

        info!("BPF Monitor initialized successfully.");

        Ok(Self { skel })
    }

    pub fn listen(
        &mut self,
        tx: Sender<Process>,
        shutdown_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let tx_clone = tx.clone();

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

                // Ignore PID 0 (swapper/idle thread) to prevent warning floods,
                // and ignore duplicate events for the same pid
                if event.pid == 0 || event.pid == event.prev_pid {
                    return;
                }

                // We do NOT call get_command_from_pid here because it performs slow file I/O
                // which blocks the BPF polling loop, causing thousands of lost events under load.
                // We just extract the 16-byte task name from the event. The worker thread will
                // resolve the full command line name using procfs asynchronously.
                let name = match std::ffi::CStr::from_bytes_until_nul(&event.task) {
                    Ok(cstr) => cstr.to_string_lossy().into_owned(),
                    Err(_) => String::from_utf8_lossy(&event.task).into_owned(),
                };

                let mut p = Process::new(ananicy_core::types::Pid(event.pid), name);
                p.delta_us = Some(event.delta_us);
                tx_clone.send(p).expect("Worker thread died");
            })
            .lost_cb(|cpu: i32, count: u64| {
                error!("Lost {} BPF events on CPU {}", count, cpu);
            })
            .build()
            .unwrap();

        // Polling loop
        loop {
            if shutdown_flag.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            if let Err(e) = perf_buffer.poll(Duration::from_millis(100)) {
                error!("Error polling BPF perf buffer: {}", e);
            }
        }
    }
}
