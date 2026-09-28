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

    /// The maps the program declares, and which of them are live.
    ///
    /// Read from the source rather than from the generated skeleton, which looks
    /// like the stronger choice and is not. `bpftool gen skeleton` emits an entry
    /// only for maps some program references, so a live-but-unused map leaves no
    /// trace in it at all: with `start` uncommented, the generated `pub struct
    /// maps` still listed only `events` and `heap`. The compiled object would
    /// settle it, but libbpf-cargo does not keep it — `OUT_DIR` holds the
    /// skeleton and nothing else.
    ///
    /// That combination is why this is a source check and why the declaration is
    /// worth commenting rather than deleting: an unused map costs 624 kB, is
    /// invisible in the generated Rust, and is gone from the daemon's view of its
    /// own maps, so the only place the fact is recorded is here.
    fn declared_maps() -> (Vec<String>, Vec<String>) {
        let mut live = Vec::new();
        let mut commented = Vec::new();
        for line in SOURCE.lines() {
            let Some((_, name)) = line.split_once("} ") else {
                continue;
            };
            // Every map declaration ends `} <name> SEC(".maps");`, so this is the
            // one shape to look for rather than a parse of the whole struct.
            let Some((name, _)) = name.split_once(" SEC(\".maps\")") else {
                continue;
            };
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                commented.push(name.to_string());
            } else {
                live.push(name.to_string());
            }
        }
        assert!(
            !live.is_empty(),
            "no live maps found in the BPF program at all, so this check would \
             pass on a program that declares none"
        );
        (live, commented)
    }

    /// The `start` map is declared upstream and never touched, and a BPF hash map
    /// allocates its element pool at creation rather than on first insert, so the
    /// reference's 10,240 entries cost 624 kB of kernel memory charged to this
    /// cgroup for the life of the process and buy nothing.
    ///
    /// The declaration stays in the source, commented, as a record for anyone
    /// reading this program against ananicy-cpp and for whoever eventually wants
    /// per-process state. What must not happen is it becoming live, which is what
    /// this asks. See [`declared_maps`] for why the generated skeleton cannot be
    /// used to ask it.
    #[test]
    fn the_start_map_is_declared_but_not_live() {
        let (live, commented) = declared_maps();
        assert!(
            commented.contains(&"start".to_string()),
            "the commented record of the `start` map is gone from the source. \
             That is a reasonable thing to do, but then drop this test too: \
             there is nothing left for it to protect."
        );
        assert!(
            !live.contains(&"start".to_string()),
            "the `start` map is live again ({live:?}), which costs 624 kB of \
             kernel memory for a map nothing in the program reads or writes"
        );
    }

    /// Both ways. A test that only checked `start` was absent would also pass on
    /// a program that declared no maps at all.
    #[test]
    fn the_maps_the_program_uses_are_live() {
        let (live, _) = declared_maps();
        for used in ["events", "heap"] {
            assert!(
                live.contains(&used.to_string()),
                "`{used}` should be a live map ({live:?}); the program writes \
                 every event through `events` and takes its scratch buffer from \
                 `heap`"
            );
        }
    }
}
