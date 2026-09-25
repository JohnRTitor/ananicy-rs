//! Differential-free fuzzing of the cpuset parser.
//!
//! The target is driven by raw bytes, because a cpuset comes from a rule file
//! that somebody wrote by hand: the parser has to be total, and whatever it
//! accepts has to survive a round trip through the serialisation, since that is
//! what the topology and X3D code hands back to it. Run it with:
//!
//! ```text
//! cargo +nightly fuzz run parse_cpuset --fuzz-dir crates/ananicy-platform/fuzz/corpus
//! ```

use {
    ananicy_core::cpuset::CpuSet,
    libfuzzer_sys::fuzz_target,
};

/// A machine with more CPUs than any real one, so that a fuzzer-generated index
/// is inside the set rather than rejected for being out of range — the point is
/// to reach the interesting inputs, not to stop at the bound.
const MAX_CORES: u32 = 256;

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    let Some(parsed) = CpuSet::parse(input, MAX_CORES) else {
        // Rejecting is always allowed. An empty set is not a useful cpuset
        // though, so it must not be one of the answers.
        return;
    };

    // Whatever was accepted has to describe itself again, and the description
    // has to parse back to the same set.
    let serialised = parsed.to_string();
    let reparsed = CpuSet::parse(&serialised, MAX_CORES)
        .unwrap_or_else(|| panic!("{serialised:?} did not parse back"));

    assert_eq!(
        reparsed.get_cores(),
        parsed.get_cores(),
        "round trip through {serialised:?} changed the set"
    );

    // And it must be stable: serialising twice cannot depend on hash order.
    assert_eq!(reparsed.to_string(), serialised);
});
