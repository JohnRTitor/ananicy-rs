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
    // In C++, to_int returns 0 for non-numeric, so it becomes "0-0" which is valid.
    // In Rust, parse() will fail, returning None. This is technically a deviation,
    // but a desirable one since "0-a" is clearly malformed.
    // We will ensure it doesn't crash and returns None.
    assert!(result.is_none());
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
