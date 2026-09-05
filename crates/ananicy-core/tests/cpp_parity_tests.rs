use {
    ananicy_core::{
        config::{Config, ConfigSnapshot},
        cpuset::CpuSet,
        rules::Rules,
    },
    std::sync::Arc,
};

#[test]
fn test_cpuset_validity() {
    let mut cs = CpuSet::new(16);
    assert!(!cs.has_cpu(5));
    cs.set_cpu(5);
    assert!(cs.has_cpu(5));
    cs.clear_cpu(5);
    assert!(!cs.has_cpu(5));

    // Bounds checking
    cs.set_cpu(100);
    assert!(!cs.has_cpu(100));
}

#[test]
fn test_cpuset_zero_method() {
    let mut cs = CpuSet::new(16);
    cs.set_cpu(0);
    cs.set_cpu(5);
    cs.set_cpu(15);

    // Rust parity: Instead of .zero(), we can recreate or just loop
    for i in 0..16 {
        cs.clear_cpu(i);
    }
    assert!(cs.is_empty());
}

#[test]
fn test_parse_cpuset_string() {
    let cs = CpuSet::parse("0", 16).unwrap();
    assert!(cs.has_cpu(0));
    assert!(!cs.has_cpu(1));

    let cs2 = CpuSet::parse("0-3", 16).unwrap();
    assert!(cs2.has_cpu(0));
    assert!(cs2.has_cpu(3));
    assert!(!cs2.has_cpu(4));

    let cs3 = CpuSet::parse("0,2-4,7,10-12", 16).unwrap();
    assert!(cs3.has_cpu(0));
    assert!(!cs3.has_cpu(1));
    assert!(cs3.has_cpu(4));
    assert!(cs3.has_cpu(11));

    // trailing comma fails in Rust parser (if split doesn't handle empty).
    // Let's test if it handles trailing cleanly. It should fail or return None.
    assert!(CpuSet::parse("0,,2", 16).is_none());
    assert!(CpuSet::parse("0-a", 16).is_none());
}

#[test]
fn test_cpuset_to_string() {
    let mut cs = CpuSet::new(16);
    cs.set_cpu(5);
    assert_eq!(cs.to_string(), "5");

    for i in 0..=7 {
        cs.set_cpu(i);
    }
    assert_eq!(cs.to_string(), "0-7");

    let mut cs2 = CpuSet::new(16);
    cs2.set_cpu(0);
    cs2.set_cpu(2);
    cs2.set_cpu(4);
    assert_eq!(cs2.to_string(), "0,2,4");
}

#[test]
fn test_rules_loading_crlf_and_comments() {
    let config = Arc::new(Config::new(ConfigSnapshot::default()));
    let mut rules = Rules::new(config);

    // Type loading
    assert!(rules.load_rule_from_string(r#"{ "type": "Doc-View", "nice": -4 }"#));

    // Rule loading
    assert!(rules.load_rule_from_string(r#"{ "name": "icecat", "type": "Doc-View" }"#));
    assert!(rules.get_rule("icecat").is_some());

    // With comment
    assert!(rules.load_rule_from_string(r#"{ "name": "mpd", "type": "Doc-View" } # comment"#));
    assert!(rules.get_rule("mpd").is_some());

    // CRLF
    assert!(rules.load_rule_from_string("{ \"name\": \"crlf\", \"type\": \"Doc-View\" }\r"));
    assert!(rules.get_rule("crlf").is_some());

    // Failures
    assert!(!rules.load_rule_from_string(r#"{ "nm": "icct" }"#));
    assert!(!rules.load_rule_from_string(r#""name": "icct" }"#));
}

#[test]
fn test_pcre2_lookaround() {
    let config = Arc::new(Config::new(ConfigSnapshot::default()));
    let mut rules = Rules::new(config);

    // Add a rule with a regex that uses negative lookahead (PCRE2 specific, unsupported in standard 'regex')
    assert!(rules.load_rule_from_string(
        r#"{ "name": "lookaround_rule", "name_regex": "^bash(?!_script)", "type": "Doc-View" }"#
    ));

    // Test match
    assert!(rules.get_rule("bash").is_some());
    // Test negative lookahead prevents match
    assert!(rules.get_rule("bash_script").is_none());
}
