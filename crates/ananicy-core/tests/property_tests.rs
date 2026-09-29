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

    /// The serialize/parse round trip, at widths that straddle the bitset's
    /// word boundary.
    ///
    /// `CpuSet` holds its CPUs in `u64` words, and the parser, the serializer
    /// and the kernel mask each translate between a bit index and a byte index
    /// at a different point. At a width of exactly 64 — which is what
    /// `MAX_CORES` is, and what the round trip above always uses — every CPU is
    /// in the first word and there is no second word to get wrong. Widths of
    /// 65, 66 and 128 are where a bit index that is computed rather than looked
    /// up stops agreeing with one that is.
    ///
    /// The CPUs are chosen rather than parsed from random text, and that is the
    /// part that took a second attempt. Feeding random strings at a width of
    /// 65 looks like it covers the second word and does not: the string has to
    /// happen to spell "64", and a proptest string almost never does, so the
    /// test passed against a deliberately broken `has_cpu` that read only the
    /// first word. `the_kernel_mask_is_whole_bytes_and_holds_every_cpu` caught
    /// that mutation on `max_cores = 65`; this one did not. Naming the boundary
    /// CPUs explicitly is what makes the name true.
    #[test]
    fn cpuset_round_trips_at_every_word_boundary(
        max_cores in prop::sample::select(vec![1u32, 7, 8, 9, 63, 64, 65, 66, 127, 128, 129, 1023, 1024, 1025]),
        extra in prop::collection::vec(0u32..1100, 0..24),
    ) {
        // Every position where a word boundary, a byte boundary, and a word
        // index computed rather than looked up disagree: 0 and 1 either side
        // of 8, 63 and 64, 64 and 65, 127 and 128, and the machine's own floor.
        let boundary = [
            0u32, 1, 7, 8, 9, 31, 32, 33, 63, 64, 65, 66, 127, 128, 129, 255, 256, 511, 512,
            1023, 1024, 1025,
        ];
        let mut built = CpuSet::new(max_cores);
        for cpu in boundary.into_iter().chain(extra) {
            built.set_cpu(cpu);
        }
        if built.is_empty() {
            return Ok(());
        }

        let serialized = built.to_string();
        let reparsed = CpuSet::parse(&serialized, max_cores)
            .expect("a serialized CPU set must parse back");
        // `Display` first, by reference: equality of the set implies equality
        // of its description, but the fuzz target this mirrors asserted it
        // separately and it is a distinct claim — that `Display` is a function
        // of the set rather than of anything about the order the two were
        // built in.
        prop_assert_eq!(&reparsed.to_string(), &serialized);
        prop_assert_eq!(
            &reparsed,
            &built,
            "round trip through {:?} changed the set, at max_cores = {}",
            serialized,
            max_cores
        );
    }

    /// A successful parse never yields an empty set.
    ///
    /// This is the one claim `fuzz/fuzz_targets/parse_cpuset.rs` makes that no
    /// test here made, and it is a contract rather than a tautology: a set
    /// with no CPUs in it means "do not touch this process' affinity", and
    /// `set_affinity` returns `Ok` for it without writing anything. So an empty
    /// set returned where a real one was written silently drops the rule
    /// instead of failing it, which is the failure mode the fuzzer was
    /// checking for and the reason a set that is not useful must not be an
    /// answer the parser can give.
    #[test]
    fn a_parsed_cpuset_is_never_empty(s in "\\PC*", max_cores in 1u32..300) {
        let Some(parsed) = CpuSet::parse(&s, max_cores) else {
            return Ok(());
        };
        prop_assert!(
            !parsed.is_empty(),
            "parse({:?}, {}) returned an empty set, which means \
             'leave affinity alone' rather than 'set it to these CPUs'",
            s,
            max_cores
        );
        prop_assert!(
            !parsed.get_cores().is_empty(),
            "the set and its own CPU list disagree about being empty"
        );
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
