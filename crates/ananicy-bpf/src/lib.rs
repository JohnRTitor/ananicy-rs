pub mod bpf_monitor;

// Include the generated BPF skeleton
mod ananicy_cpp {
    include!(concat!(env!("OUT_DIR"), "/ananicy_cpp.skel.rs"));
}

pub use bpf_monitor::BpfMonitor;

#[cfg(test)]
mod tests {
    /// The program source, as the build reads it.
    ///
    /// `include_str!` rather than a `read_to_string` at runtime, so the test does
    /// not pass on a stale path or fail on a missing one.
    const SOURCE: &str = include_str!("../bpf/ananicy_cpp.bpf.c");

    /// `--bpf-min-us` writes into the program's read-only data, and the program
    /// decides what to do with it. For a long time the only line that read it was
    /// commented out, so the flag was accepted, documented, threaded all the way
    /// into `rodata.min_us`, and then consumed by nothing — a documented flag
    /// that silently does nothing, which is the worst kind, because it looks like
    /// it is working.
    ///
    /// Asserting on the source is not a substitute for exercising the program,
    /// which needs root and a BPF-capable kernel, and this does not pretend to
    /// be one. It is here because the failure mode is invisible to every other
    /// check available: the build passes, the tests pass, the flag parses, and
    /// nothing reports the value going nowhere. A two-line change to the program
    /// breaks this test rather than the daemon's behaviour, and that is the trade
    /// worth making for a setting nobody would otherwise notice going.
    #[test]
    fn the_minimum_interval_check_is_not_commented_out() {
        for line in SOURCE.lines() {
            let line = line.trim();
            if line.starts_with("//") && line.contains("min_us && delta_us") {
                panic!(
                    "the BPF program's minimum-interval check is commented out \
                     again ({line:?}), which leaves --bpf-min-us writing a value \
                     nothing reads. If the check is going away on purpose, the \
                     flag and its documentation go with it."
                );
            }
        }
        assert!(
            SOURCE
                .lines()
                .any(|line| !line.trim_start().starts_with("//")
                    && line.contains("min_us && delta_us")),
            "no minimum-interval check in the BPF program at all, so \
             --bpf-min-us is inert"
        );
    }

    /// The same argument for the `start` map: it was declared, never referenced
    /// by anything, and cost 624 kB of kernel memory — a declaration that is only
    /// ever a declaration has no way to fail visibly.
    #[test]
    fn there_is_no_start_map() {
        for (number, line) in SOURCE.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
                continue;
            }
            assert_ne!(
                line,
                "} start SEC(\".maps\");",
                "line {}: the unused `start` map is back, costing 624 kB of \
                 kernel memory that nothing in the program touches",
                number + 1
            );
        }
    }
}
