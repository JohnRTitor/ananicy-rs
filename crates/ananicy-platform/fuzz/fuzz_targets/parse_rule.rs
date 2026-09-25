//! Differential-free fuzzing of the rule-line parser.
//!
//! A rule file is a stream of hand-written JSON lines, and one of them is enough
//! to take the daemon down if the parser is not total. The target feeds
//! arbitrary bytes to `load_rule_from_string` and checks the two invariants the
//! engine depends on: a line is stored under exactly one of the three maps, and
//! a line that was accepted changed nothing about how it is looked up.
//!
//! The same invariants are asserted by the property tests in
//! `crates/ananicy-core/tests/property_tests.rs`, which run in CI; this target
//! exists to reach the inputs a generator will not. Run it with:
//!
//! ```text
//! cargo +nightly fuzz run parse_rule --fuzz-dir crates/ananicy-platform/fuzz/corpus
//! ```

use {
    ananicy_core::{
        config::{Config, ConfigSnapshot},
        rules::Rules,
    },
    libfuzzer_sys::fuzz_target,
    std::sync::Arc,
};

fuzz_target!(|data: &[u8]| {
    let Ok(line) = std::str::from_utf8(data) else {
        return;
    };

    let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
    let accepted = rules.load_rule_from_string(line);

    // A name, a type and a cgroup are mutually exclusive: the classifier takes
    // the first one it finds, so one line can never land in two maps.
    let stored = rules.size() + rules.get_types().len() + rules.get_cgroups().len();
    assert!(stored <= 1, "one line was stored more than once: {line:?}");

    if !accepted {
        assert_eq!(stored, 0, "a rejected line changed the state: {line:?}");
        return;
    }

    // Everything that was stored is still reachable under the name it declared.
    for (name, rule) in rules.get_rules() {
        assert_eq!(
            rules.get_rule(name.as_ref()).as_deref(),
            Some(rule.as_ref()),
            "{} is stored but not resolvable",
            name.as_ref()
        );
    }
});
