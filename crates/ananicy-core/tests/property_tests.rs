//! Property-based tests for the parsers that see untrusted input.
//!
//! These express `ananicy-rs`'s own contract for its CPU set parser and its
//! rule loader: they must never panic, and the state they produce must stay
//! self-consistent. They are driven by `proptest`; the `libFuzzer` targets in
//! `crates/ananicy-platform/fuzz` cover the same class of input for the mount
//! table parser.

use {
    ananicy_core::{
        config::{Config, ConfigSnapshot},
        cpuset::CpuSet,
        rules::Rules,
    },
    proptest::prelude::*,
    std::sync::Arc,
};

const MAX_CORES: u32 = 64;

proptest! {
    /// Arbitrary text must never panic the CPU set parser, and whatever it
    /// accepts must survive a serialize/parse round trip unchanged.
    #[test]
    fn cpuset_parse_never_panics_and_round_trips(s in "\\PC*") {
        let Some(parsed) = CpuSet::parse(&s, MAX_CORES) else {
            return Ok(());
        };

        let serialized = parsed.to_string();
        let reparsed = CpuSet::parse(&serialized, MAX_CORES)
            .expect("a serialized CPU set must parse back");
        prop_assert_eq!(reparsed, parsed);
    }

    /// A successful parse can never select a CPU outside the advertised range.
    #[test]
    fn cpuset_parse_respects_the_cpu_bound(
        s in "\\PC*",
        max_cores in 0u32..512,
    ) {
        let Some(parsed) = CpuSet::parse(&s, max_cores) else {
            return Ok(());
        };

        for cpu in parsed.get_cores() {
            prop_assert!(cpu < max_cores, "cpu {cpu} escaped the bound {max_cores}");
        }
    }

    /// Any selection that can be built through the public API serializes to a
    /// string that parses back to exactly the same set. The empty set is the one
    /// documented exception: it renders as an empty string, which is rejected.
    #[test]
    fn cpuset_display_round_trips_any_selection(cores in prop::collection::vec(0u32..MAX_CORES, 0..64)) {
        let mut built = CpuSet::new(MAX_CORES);
        for cpu in &cores {
            built.set_cpu(*cpu);
        }

        let serialized = built.to_string();
        if built.is_empty() {
            prop_assert!(serialized.is_empty());
            prop_assert!(CpuSet::parse(&serialized, MAX_CORES).is_none());
        } else {
            let reparsed = CpuSet::parse(&serialized, MAX_CORES)
                .expect("a non-empty set must parse back");
            prop_assert_eq!(reparsed.to_string(), serialized);
            prop_assert_eq!(reparsed, built);
        }
    }

    /// The kernel mask is a whole number of bytes wide, and every CPU the set
    /// can hold is expressible in one.
    ///
    /// This is the invariant behind a real panic that was in `mask_from` and
    /// came with it into `CpuSet`: the mask was `max_cores / 8` bytes with its
    /// bits set behind a `cpu < max_cores` guard, so a machine whose CPU count
    /// is not a multiple of eight had a mask one byte too short for its own
    /// highest CPU, and naming that CPU indexed past the end of the buffer. It
    /// was unreachable only because `get_max_number_of_cpus` floors at 1024,
    /// which is a multiple of eight.
    ///
    /// `the_kernel_mask_agrees_with_the_set` in `tests/cpuset.rs` pins that at
    /// ten widths I picked, which is a list rather than a property, and a list
    /// is what let the first version of this through. This covers every width
    /// in range instead, so the width formula cannot be wrong again for a
    /// width nobody thought of — which is the only width it was wrong for
    /// before.
    #[test]
    fn the_kernel_mask_is_whole_bytes_and_holds_every_cpu(
        max_cores in 0u32..600,
        // Indices deliberately range wider than `max_cores`: out-of-range
        // `set_cpu` is a documented no-op, and a mask that honoured it would
        // be a different defect.
        edits in prop::collection::vec((0u32..700, prop::bool::ANY), 0..48),
    ) {
        let mut set = CpuSet::new(max_cores);
        for (cpu, on) in edits {
            if on {
                set.set_cpu(cpu);
            } else {
                set.clear_cpu(cpu);
            }
        }

        let mask = set.kernel_mask();
        // `prop_assert*` expands its message through `concat!`, which is a
        // macro and so cannot capture variables implicitly — every message here
        // passes its arguments explicitly.
        prop_assert_eq!(
            mask.len(),
            (max_cores as usize).div_ceil(8),
            "a mask for {} CPUs must be ceil({}/8) bytes",
            max_cores,
            max_cores
        );

        for cpu in 0..max_cores {
            let in_mask = mask[cpu as usize / 8] & (1 << (cpu % 8)) != 0;
            prop_assert_eq!(
                in_mask,
                set.has_cpu(cpu),
                "CPU {} disagrees between the set and the mask, at max_cores = {}",
                cpu,
                max_cores
            );
        }

        // And nothing outside the machine is in the mask, which is what the
        // `max_cores` bound is for: a rule may name a CPU the machine does not
        // have, and the syscall has to report that rather than have the mask
        // quietly widen.
        for cpu in max_cores..(max_cores + 8).min(mask.len() as u32 * 8) {
            prop_assert!(
                !mask[cpu as usize / 8] & (1 << (cpu % 8)) != 0,
                "CPU {} is beyond max_cores = {} and must not be set",
                cpu,
                max_cores
            );
        }
    }

    /// Arbitrary text must never panic the rule loader, and a line is either
    /// fully accepted into exactly one of the three rule maps or rejected
    /// without changing any of them.
    #[test]
    fn rule_loading_is_all_or_nothing(s in "\\PC*") {
        let config = Arc::new(Config::new(ConfigSnapshot::default()));
        let mut rules = Rules::new(config);

        let accepted = rules.load_rule_from_string(&s);
        let stored = rules.size() + rules.get_types().len() + rules.get_cgroups().len();

        prop_assert_eq!(stored, usize::from(accepted));
    }

    /// A rule that is accepted can always be looked up by its own name, and
    /// looking an unknown name up never invents a rule.
    #[test]
    fn accepted_rules_are_retrievable(s in "\\\\PC{0,64}") {
        let config = Arc::new(Config::new(ConfigSnapshot::default()));
        let mut rules = Rules::new(config);
        let _ = rules.load_rule_from_string(&s);

        for (name, _) in rules.iter_rules() {
            prop_assert!(rules.get_rule(name.as_ref()).is_some());
        }
        prop_assert!(rules.get_rule("ananicy-definitely-not-a-loaded-rule").is_none());
    }
}
