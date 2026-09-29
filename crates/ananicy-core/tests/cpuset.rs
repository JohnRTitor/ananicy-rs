//! `CpuSet` behaviour.
//!
//! A CPU set is the bridge between the textual `cpuset` field of a rule and the
//! `sched_setaffinity` call, so it is tested through the public API that
//! `ananicy-platform` consumes as well.

use ananicy_core::cpuset::CpuSet;

// ---------------------------------------------------------
// CpuSet
// ---------------------------------------------------------
#[test]
fn test_cpuset_construction_and_validity() {
    let cs = CpuSet::new(16);
    assert!(cs.valid());
    assert_eq!(cs.get_cores().len(), 0);
}

#[test]
fn test_cpuset_zero_initialized() {
    let cs = CpuSet::new(16);
    assert!(cs.valid());
    for i in 0..16 {
        assert!(!cs.has_cpu(i));
    }
}

#[test]
fn test_cpuset_set_and_clear() {
    let mut cs = CpuSet::new(16);
    assert!(cs.valid());

    cs.set_cpu(5);
    assert!(cs.has_cpu(5));
    assert!(!cs.has_cpu(4));
    assert!(!cs.has_cpu(6));

    cs.clear_cpu(5);
    assert!(!cs.has_cpu(5));
}

#[test]
fn test_cpuset_set_multiple() {
    let mut cs = CpuSet::new(32);
    assert!(cs.valid());

    cs.set_cpu(0);
    cs.set_cpu(7);
    cs.set_cpu(15);
    cs.set_cpu(31);

    assert!(cs.has_cpu(0));
    assert!(cs.has_cpu(7));
    assert!(cs.has_cpu(15));
    assert!(cs.has_cpu(31));
    assert!(!cs.has_cpu(1));
    assert!(!cs.has_cpu(16));
}

#[test]
fn test_cpuset_bounds_checking() {
    let mut cs = CpuSet::new(8);
    assert!(cs.valid());

    // Out-of-bounds set should be silently ignored
    cs.set_cpu(8);
    cs.set_cpu(100);
    // Note: Rust version uses u32, so negative is impossible statically, but out of bounds is.

    // Out-of-bounds is_set should return false
    assert!(!cs.has_cpu(8));
    assert!(!cs.has_cpu(100));
}

#[test]
fn test_cpuset_zero_method() {
    let mut cs = CpuSet::new(16);
    assert!(cs.valid());

    cs.set_cpu(0);
    cs.set_cpu(5);
    cs.set_cpu(15);
    assert!(cs.has_cpu(0));
    assert!(cs.has_cpu(5));

    cs.zero();
    for i in 0..16 {
        assert!(!cs.has_cpu(i));
    }
}

#[test]
fn test_cpuset_move_constructor() {
    let mut cs1 = CpuSet::new(16);
    assert!(cs1.valid());
    cs1.set_cpu(3);
    cs1.set_cpu(7);

    let cs2 = cs1; // Move assignment in Rust
    assert!(cs2.valid());
    assert!(cs2.has_cpu(3));
    assert!(cs2.has_cpu(7));
    assert!(!cs2.has_cpu(0));
}

#[test]
fn test_cpuset_move_assignment() {
    let mut cs1 = CpuSet::new(16);
    assert!(cs1.valid());
    cs1.set_cpu(10);

    let mut cs2 = CpuSet::new(8);
    assert!(cs2.valid());

    cs2 = cs1;
    assert!(cs2.valid());
    assert!(cs2.has_cpu(10));
}

// ---------------------------------------------------------
// cpuset_parsing
// ---------------------------------------------------------
#[test]
fn test_parse_single_cpu() {
    let result = CpuSet::parse("0", 16);
    assert!(result.is_some());
    let cs = result.unwrap();
    assert!(cs.has_cpu(0));
    assert!(!cs.has_cpu(1));
}

#[test]
fn test_parse_another_single_cpu() {
    let result = CpuSet::parse("5", 16);
    assert!(result.is_some());
    let cs = result.unwrap();
    assert!(cs.has_cpu(5));
    assert!(!cs.has_cpu(0));
    assert!(!cs.has_cpu(4));
    assert!(!cs.has_cpu(6));
}

#[test]
fn test_parse_simple_range() {
    let result = CpuSet::parse("0-3", 16);
    assert!(result.is_some());
    let cs = result.unwrap();
    assert!(cs.has_cpu(0));
    assert!(cs.has_cpu(1));
    assert!(cs.has_cpu(2));
    assert!(cs.has_cpu(3));
    assert!(!cs.has_cpu(4));
}

#[test]
fn test_parse_comma_separated() {
    let result = CpuSet::parse("0,2,4", 16);
    assert!(result.is_some());
    let cs = result.unwrap();
    assert!(cs.has_cpu(0));
    assert!(!cs.has_cpu(1));
    assert!(cs.has_cpu(2));
    assert!(!cs.has_cpu(3));
    assert!(cs.has_cpu(4));
}

#[test]
fn test_parse_mixed_notation() {
    let result = CpuSet::parse("0-3,8-11", 16);
    assert!(result.is_some());
    let cs = result.unwrap();
    for i in 0..=3 {
        assert!(cs.has_cpu(i));
    }
    for i in 4..=7 {
        assert!(!cs.has_cpu(i));
    }
    for i in 8..=11 {
        assert!(cs.has_cpu(i));
    }
}

#[test]
fn test_parse_complex_mixed_notation() {
    let result = CpuSet::parse("0,2-4,7,10-12", 16);
    assert!(result.is_some());
    let cs = result.unwrap();
    assert!(cs.has_cpu(0));
    assert!(!cs.has_cpu(1));
    assert!(cs.has_cpu(2));
    assert!(cs.has_cpu(3));
    assert!(cs.has_cpu(4));
    assert!(!cs.has_cpu(5));
    assert!(!cs.has_cpu(6));
    assert!(cs.has_cpu(7));
    assert!(!cs.has_cpu(8));
    assert!(!cs.has_cpu(9));
    assert!(cs.has_cpu(10));
    assert!(cs.has_cpu(11));
    assert!(cs.has_cpu(12));
}

#[test]
fn test_parse_single_element_range() {
    // "5-5" is a valid range with a single element
    let result = CpuSet::parse("5-5", 16);
    assert!(result.is_some());
    let cs = result.unwrap();
    assert!(cs.has_cpu(5));
    assert!(!cs.has_cpu(4));
    assert!(!cs.has_cpu(6));
}

#[test]
fn test_parse_invalid_empty_string() {
    let result = CpuSet::parse("", 16);
    assert!(result.is_none());
}

#[test]
fn test_parse_invalid_non_numeric() {
    let result = CpuSet::parse("abc", 16);
    assert!(result.is_none());
}

#[test]
fn test_parse_invalid_inverted_range() {
    let result = CpuSet::parse("5-3", 16);
    assert!(result.is_none());
}

#[test]
fn test_parse_invalid_negative() {
    let result = CpuSet::parse("-1", 16);
    assert!(result.is_none());
}

#[test]
fn test_parse_invalid_out_of_range() {
    let result = CpuSet::parse("99999", 16);
    assert!(result.is_none());
}

#[test]
fn test_parse_trailing_comma_accepts_prefix() {
    // Trailing comma: parser processes "0" and "1", then loop ends
    // since pos == size. This is accepted (not rejected).
    let result = CpuSet::parse("0,1,", 16);
    assert!(result.is_some(), "Trailing comma should be accepted");
    let cs = result.unwrap();
    assert!(cs.has_cpu(0));
    assert!(cs.has_cpu(1));
}

#[test]
fn test_parse_invalid_double_comma() {
    let result = CpuSet::parse("0,,2", 16);
    assert!(result.is_none());
}

#[test]
fn test_parse_invalid_range_with_letters() {
    let result = CpuSet::parse("0-a", 16);
    // "0-a" is clearly malformed. The parser rejects it instead of silently
    // treating the range as "0-0" the way a lenient integer reader would.
    assert!(result.is_none());
}

#[test]
fn test_parse_is_whitespace_tolerant_around_tokens() {
    let cs = CpuSet::parse("  0-1 , 4 ,  8-9  ", 16).unwrap();
    assert_eq!(cs.get_cores(), vec![0, 1, 4, 8, 9]);

    // Whitespace *inside* a range is not part of the accepted syntax.
    assert!(CpuSet::parse("8 - 9", 16).is_none());
}

#[test]
fn test_parse_accepts_leading_zeros_and_the_upper_bound() {
    assert!(CpuSet::parse("007", 16).unwrap().has_cpu(7));
    assert!(CpuSet::parse("15", 16).unwrap().has_cpu(15));
    assert!(CpuSet::parse("0-15", 16).unwrap().get_cores().len() == 16);
}

#[test]
fn test_parse_rejects_out_of_range_range_endpoints() {
    // The whole range must fit, not just its first CPU.
    assert!(CpuSet::parse("8-16", 16).is_none());
    assert!(CpuSet::parse("0-16", 16).is_none());
    assert!(CpuSet::parse("16", 16).is_none());
}

#[test]
fn test_parse_rejects_values_beyond_u32() {
    assert!(CpuSet::parse("4294967296", 16).is_none());
    assert!(CpuSet::parse("0-99999999999999999999", 16).is_none());
}

#[test]
fn test_parse_rejects_whitespace_only_and_comma_only_input() {
    assert!(CpuSet::parse("   ", 16).is_none());
    assert!(CpuSet::parse(",", 16).is_none());
    assert!(CpuSet::parse(",0", 16).is_none());
    assert!(
        CpuSet::parse("0,", 16).is_some(),
        "a trailing comma is tolerated"
    );
}

#[test]
fn test_parse_with_zero_cores_never_succeeds() {
    assert!(CpuSet::parse("0", 0).is_none());
    assert!(CpuSet::parse("0-3", 0).is_none());
    assert!(CpuSet::parse("", 0).is_none());
}

// ---------------------------------------------------------
// cpuset_to_string
// ---------------------------------------------------------
#[test]
fn test_serialize_single_cpu() {
    let mut cs = CpuSet::new(16);
    assert!(cs.valid());
    cs.set_cpu(5);
    assert_eq!(cs.to_string(), "5");
}

#[test]
fn test_serialize_contiguous_range() {
    let mut cs = CpuSet::new(16);
    assert!(cs.valid());
    for i in 0..=7 {
        cs.set_cpu(i);
    }
    assert_eq!(cs.to_string(), "0-7");
}

#[test]
fn test_serialize_discontiguous_cpus() {
    let mut cs = CpuSet::new(16);
    assert!(cs.valid());
    cs.set_cpu(0);
    cs.set_cpu(2);
    cs.set_cpu(4);
    assert_eq!(cs.to_string(), "0,2,4");
}

#[test]
fn test_serialize_mixed_ranges_and_singles() {
    let mut cs = CpuSet::new(16);
    assert!(cs.valid());
    for i in 0..=3 {
        cs.set_cpu(i);
    }
    for i in 8..=11 {
        cs.set_cpu(i);
    }
    assert_eq!(cs.to_string(), "0-3,8-11");
}

#[test]
fn test_serialize_empty_set() {
    let cs = CpuSet::new(16);
    assert!(cs.valid());
    assert_eq!(cs.to_string(), "");
}

#[test]
fn test_serialize_invalid_set() {
    let cs = CpuSet::new(0);
    assert_eq!(cs.to_string(), "");
}

#[test]
fn test_roundtrip_parse_then_serialize() {
    let parsed = CpuSet::parse("0-3,8-11", 16);
    assert!(parsed.is_some());
    assert_eq!(parsed.unwrap().to_string(), "0-3,8-11");
}

#[test]
fn test_roundtrip_single_values() {
    let parsed = CpuSet::parse("1,3,5", 16);
    assert!(parsed.is_some());
    assert_eq!(parsed.unwrap().to_string(), "1,3,5");
}

/// The kernel mask has to agree with the set, bit for bit.
///
/// This is the layout `sched_setaffinity(2)` reads, and it is now built by
/// `CpuSet` rather than derived from the set at the call site — so a
/// disagreement here would move every CPU in the set to a different bit and
/// pin processes to the wrong cores, with no error from the syscall to say so.
/// The width matters for the same reason: the kernel requires `len` to be at
/// least the size of its own `cpumask`, so a short mask is `EINVAL` and a
/// correct set is silently not applied.
///
/// Width is `ceil(max_cores / 8)` whole bytes, so every CPU the set can hold
/// has a byte to live in. The first width here is therefore 1, not 8: a set
/// narrower than a byte used to produce a zero-length mask that could not
/// express its own highest CPU, and the mask used to be `max_cores / 8` bytes
/// with a `cpu < max_cores` guard, which indexed past the end of that buffer
/// for any machine whose CPU count was not a multiple of eight. That was
/// unreachable only because `get_max_number_of_cpus` floors at 1024, which is
/// a multiple of eight; these widths are chosen to pin the rounding.
#[test]
fn the_kernel_mask_agrees_with_the_set() {
    for max_cores in [1u32, 7, 8, 9, 63, 64, 65, 1024, 1026, 8192] {
        let mut set = CpuSet::new(max_cores);
        // A scattered selection rather than a contiguous run, so every bit
        // position within a word is exercised — including 0 and 63, which are
        // the ones a `u64` shift gets wrong when it is written as `1 << n`.
        for cpu in [0u32, 1, 7, 8, 31, 32, 63, 64, 65, 127, 128] {
            if cpu < max_cores {
                set.set_cpu(cpu);
            }
        }

        let mask = set.kernel_mask();
        assert_eq!(
            mask.len(),
            (max_cores as usize).div_ceil(8),
            "the mask must be ceil(max_cores/8) bytes wide for {max_cores} CPUs"
        );

        for cpu in 0..max_cores {
            let bit_is_set = mask[cpu as usize / 8] & (1 << (cpu % 8)) != 0;
            assert_eq!(
                bit_is_set,
                set.has_cpu(cpu),
                "CPU {cpu} disagrees between the set and the kernel mask \
                 (max_cores = {max_cores})"
            );
        }
    }
}

/// Every set CPU, and only a set CPU, is in the mask.
#[test]
fn the_kernel_mask_holds_exactly_the_sets_cpus() {
    let parsed = CpuSet::parse("0-3,8-11", 1024).expect("a valid cpuset");
    let mask = parsed.kernel_mask();
    let from_mask: Vec<u32> = (0..1024u32)
        .filter(|&cpu| mask[cpu as usize / 8] & (1 << (cpu % 8)) != 0)
        .collect();
    assert_eq!(
        from_mask,
        parsed.get_cores(),
        "reading the mask back must give the set the parser was given"
    );
}

/// A machine with no CPU slots is not a panic.
#[test]
fn a_set_with_no_cpu_slots_has_an_empty_mask() {
    let set = CpuSet::new(0);
    assert!(set.is_empty(), "nothing is in a set with no slots");
    assert!(set.kernel_mask().is_empty());
    assert!(!set.has_cpu(0), "there is no CPU 0 to have");
}

/// A set narrower than a byte can still express its CPUs, which it could not
/// before the mask width was rounded up.
#[test]
fn a_set_narrower_than_a_byte_still_expresses_its_cpus() {
    let mut set = CpuSet::new(1);
    set.set_cpu(0);
    assert!(set.has_cpu(0), "CPU 0 is in the set");
    let mask = set.kernel_mask();
    assert_eq!(
        mask.len(),
        1,
        "one CPU still needs the one byte it lives in"
    );
    assert_eq!(mask[0] & 1, 1, "and CPU 0 is in it");
}
