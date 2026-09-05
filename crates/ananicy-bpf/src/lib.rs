pub mod bpf_monitor;

// Include the generated BPF skeleton
mod ananicy_cpp {
    include!(concat!(env!("OUT_DIR"), "/ananicy_cpp.skel.rs"));
}

pub use bpf_monitor::BpfMonitor;
