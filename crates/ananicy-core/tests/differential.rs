//! Differential tests verifying that Rust output matches C++ expected output.
use ananicy_core::config::{ConfigSnapshot, LogLevel};

#[test]
fn test_config_parsing_matches_cpp() {
    let test_dir = std::env::current_dir().unwrap().join("tests/fixtures");
    let config_path = test_dir.join("test-sampleconfig.txt");

    // We expect the file to parse exactly as the C++ unit test describes.
    let config = ConfigSnapshot::parse_file(&config_path).unwrap_or_default();

    // Check "apply" options.
    assert!(!config.apply_nice);
    assert!(!config.apply_latnice);
    assert!(!config.apply_sched);
    assert!(!config.apply_ionice);
    assert!(!config.apply_oom_score_adj);
    assert!(!config.apply_cgroups);
    assert!(!config.apply_cpuset);

    // Check "load" options.
    assert!(!config.cgroup_load);
    assert!(!config.type_load);
    assert!(!config.rule_load);

    // Check logging.
    assert!(config.log_applied_rule);
    assert_eq!(config.loglevel, LogLevel::Error);

    // Check frequency.
    assert_eq!(config.check_freq, 5);

    // Check x3d.
    assert_eq!(config.x3d_mode, "frequency");

    // Check workaround.
    assert!(!config.cgroup_realtime_workaround);
}

#[test]
fn test_rules_parsing_matches_cpp() {
    let test_dir = std::env::current_dir().unwrap().join("tests/fixtures");
    let config_path = test_dir.join("test-rulesconfig.txt");

    let config =
        std::sync::Arc::new(ananicy_core::config::Config::load_file(&config_path, true).unwrap());
    let mut rules = ananicy_core::rules::Rules::new(config.clone());

    // Load rule from string
    assert!(
        rules.load_rule_from_string(r#"{ "type": "Doc-View", "nice": -4, "latency_nice": 5 }"#)
    );
    assert!(rules.load_rule_from_string(
        r#"{ "type": "Player-Audio", "nice": 6, "ioclass": "realtime", "latency_nice": 8 }"#
    ));

    assert!(rules.load_rule_from_string(r#"{ "name": "icecat", "type":"Doc-View" }"#));
    assert!(rules.get_rule("icecat").is_some());
    assert_eq!(rules.size(), 1);

    assert!(rules.load_rule_from_string(r#"{ "name": "mpd", "type": "Player-Audio" }"#));
    assert!(rules.get_rule("mpd").is_some());
    assert_eq!(rules.size(), 2);

    assert!(rules.load_rule_from_string(r#"{ "name": "someprogram", "type":"Dc-Vw" } # hey"#));
    assert!(rules.get_rule("someprogram").is_some());
    assert_eq!(rules.size(), 3);

    assert!(rules.load_rule_from_string(
        r#"      		   { "name": "someprogram2", "type":"Dc-Vw" }         		  "#
    ));
    assert!(rules.get_rule("someprogram2").is_some());
    assert_eq!(rules.size(), 4);

    // Invalid rules
    assert!(!rules.load_rule_from_string(r#"{ "nm": "icct", "tp":"Dc-Vw" }"#));
    assert!(!rules.load_rule_from_string(r#"# { "nm": "icct", "tp":"Dc-Vw" }"#));
    assert!(!rules.load_rule_from_string(r#"{ "name": "icct", "type":"Dc-Vw" "#));
    assert!(!rules.load_rule_from_string(r#""name": "icct", "type":"Dc-Vw" }"#));
    assert!(!rules.load_rule_from_string(r#""name": "icct", "type":"Dc-Vw" "#));
    assert_eq!(rules.size(), 4);

    // Whitespace only
    assert!(!rules.load_rule_from_string("      "));
    assert!(!rules.load_rule_from_string("\t\t\t"));
    assert!(!rules.load_rule_from_string(""));

    // Trailing \r
    assert!(!rules.load_rule_from_string("\r"));
    assert!(rules.load_rule_from_string("{ \"name\": \"crlftest\", \"type\": \"Game\" }\r"));
    assert!(rules.get_rule("crlftest").is_some());
    assert_eq!(rules.size(), 5);

    // Nested JSON
    assert!(rules.load_rule_from_string(
        r#"{ "name": "nested", "type": "Game", "extra": { "key": "val" } }"#
    ));
    assert!(rules.get_rule("nested").is_some());
    assert_eq!(rules.size(), 6);
}
