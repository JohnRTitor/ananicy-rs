use {
    ananicy_core::{
        config::{Config, ConfigSnapshot},
        cpuset::CpuSet,
        rules::Rules,
    },
    proptest::prelude::*,
    std::sync::Arc,
};

proptest! {
    #[test]
    fn fuzz_parse_cpuset(s in "\\PC*") {
        // Must never panic or crash
        let result = CpuSet::parse(&s, 64);
        if let Some(cs) = result {
            // Must support round-trip serialization without crashing
            let serialized = cs.to_string();
            let roundtrip = CpuSet::parse(&serialized, 64);
            assert!(roundtrip.is_some());
        }
    }

    #[test]
    fn fuzz_parse_rule(s in "\\PC*") {
        let config = Arc::new(Config::new(ConfigSnapshot::default()));
        let mut rules = Rules::new(config);

        // Fuzzing the load_rule_from_string function. Must never panic.
        // It should either return false (invalid) or true (valid).
        let _ = rules.load_rule_from_string(&s);
    }
}
