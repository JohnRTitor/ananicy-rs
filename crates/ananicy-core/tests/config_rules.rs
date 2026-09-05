use ananicy_core::{config::Config, rules::Rules};

use {
    ananicy_core::config::LogLevel,
    std::{fs, path::Path},
};

// ---------------------------------------------------------
// Config Tests
// ---------------------------------------------------------
#[test]
fn test_config_is_correctly_set_according_to_file() {
    let conf = Config::load_file(Path::new("tests/fixtures/test-sampleconfig.txt"), true).unwrap();

    // Check "apply" options.
    assert!(!conf.get().apply_nice);
    assert!(!conf.get().apply_latnice);
    assert!(!conf.get().apply_sched);
    assert!(!conf.get().apply_ioclass); // Note: in C++ it was apply_ionice, but apply_ioclass maps similarly or we check both
    assert!(!conf.get().apply_oom_score_adj);
    assert!(!conf.get().apply_cgroups);
    assert!(!conf.get().apply_cpuset);

    // Check "load" options.
    assert!(!conf.get().cgroup_load);
    assert!(!conf.get().type_load);
    assert!(!conf.get().rule_load);

    // Check logging.
    assert!(conf.get().log_applied_rule);
    assert_eq!(conf.get().loglevel, LogLevel::Error);

    // Check frequency.
    assert_eq!(conf.get().check_freq, 5); // wait, it's u32 not a Duration

    // Check x3d.
    assert_eq!(conf.get().x3d_mode, "frequency");

    // Check workaround.
    assert!(!conf.get().cgroup_realtime_workaround);
}

// ---------------------------------------------------------
// Rules Tests
// ---------------------------------------------------------
#[test]
fn test_load_rule_from_string() {
    let conf = std::sync::Arc::new(Config::new(ananicy_core::config::ConfigSnapshot::default()));
    let mut rules = Rules::new(conf.clone());

    assert_eq!(rules.size(), 0);

    // Check if type was loaded successfully.
    assert!(
        rules.load_rule_from_string(r#"{ "type": "Doc-View", "nice": -4, "latency_nice": 5 }"#)
    );
    assert!(rules.load_rule_from_string(
        r#"{ "type": "Player-Audio", "nice": 6, "ioclass": "realtime", "latency_nice": 8 }"#
    ));

    // Check if rule was loaded successfully.
    assert!(rules.load_rule_from_string(r#"{ "name": "icecat", "type":"Doc-View" }"#));
    assert!(rules.get_rule("icecat").is_some());
    assert_eq!(rules.size(), 1);

    // Check if more rules were loaded successfully.
    assert!(rules.load_rule_from_string(r#"{ "name": "mpd", "type": "Player-Audio" }"#));
    assert!(rules.get_rule("mpd").is_some());
    assert_eq!(rules.size(), 2);

    // comment(#) after the rule
    assert!(rules.load_rule_from_string(r#"{ "name": "someprogram", "type":"Dc-Vw" } # hey"#));
    assert!(rules.get_rule("someprogram").is_some());
    assert_eq!(rules.size(), 3);

    // should be trimmed
    assert!(rules.load_rule_from_string(
        "      \t\t   { \"name\": \"someprogram2\", \"type\":\"Dc-Vw\" }         \t\t  "
    ));
    assert!(rules.get_rule("someprogram2").is_some());
    assert_eq!(rules.size(), 4);

    // These rules must fail.

    // no field called "name"
    assert!(!rules.load_rule_from_string(r#"{ "nm": "icct", "tp":"Dc-Vw" }"#));
    // comment(#) before the rule
    assert!(!rules.load_rule_from_string(r#"# { "nm": "icct", "tp":"Dc-Vw" }"#));
    // missing closing bracket
    assert!(!rules.load_rule_from_string(r#"{ "name": "icct", "type":"Dc-Vw" "#));
    // missing open bracket
    assert!(!rules.load_rule_from_string(r#""name": "icct", "type":"Dc-Vw" }"#));
    // missing both brackets
    assert!(!rules.load_rule_from_string(r#""name": "icct", "type":"Dc-Vw" "#));

    assert_eq!(rules.size(), 4);
}

#[test]
fn test_whitespace_only_lines_are_skipped() {
    let conf = std::sync::Arc::new(Config::new(ananicy_core::config::ConfigSnapshot::default()));
    let mut rules = Rules::new(conf.clone());

    assert!(!rules.load_rule_from_string("      "));
    assert!(!rules.load_rule_from_string("\t\t\t"));
    assert!(!rules.load_rule_from_string("  \t  \t  "));
    assert!(!rules.load_rule_from_string(""));
    assert_eq!(rules.size(), 0);
}

#[test]
fn test_trailing_carriage_return_is_handled() {
    let conf = std::sync::Arc::new(Config::new(ananicy_core::config::ConfigSnapshot::default()));
    let mut rules = Rules::new(conf.clone());

    assert!(!rules.load_rule_from_string("\r"));
    assert!(!rules.load_rule_from_string("   \r"));
    assert!(!rules.load_rule_from_string("# this is a comment\r"));

    // \r after } simulates std::getline on a CRLF file
    assert!(rules.load_rule_from_string("{ \"name\": \"crlftest\", \"type\": \"Game\" }\r"));
    assert!(rules.get_rule("crlftest").is_some());
    assert_eq!(rules.size(), 1);
}

#[test]
fn test_nested_json_objects_parse_correctly() {
    let conf = std::sync::Arc::new(Config::new(ananicy_core::config::ConfigSnapshot::default()));
    let mut rules = Rules::new(conf.clone());

    // The rust parser parses the JSON into a value. It should ignore `extra`.
    assert!(rules.load_rule_from_string(
        r#"{ "name": "nested", "type": "Game", "extra": { "key": "val" } }"#
    ));
    assert!(rules.get_rule("nested").is_some());
    assert_eq!(rules.size(), 1);
}

#[test]
fn test_load_rules_from_crlf_file() {
    let conf = std::sync::Arc::new(Config::new(ananicy_core::config::ConfigSnapshot::default()));
    let mut rules = Rules::new(conf.clone());

    let tmp_path = Path::new("tests/fixtures/test-crlf.rules");
    let content = "# comment line\r\n\
                   \r\n\
                   { \"type\": \"TestType\", \"nice\": -1 }\r\n\
                      \r\n\
                   { \"name\": \"crlfprog\", \"type\": \"TestType\" }\r\n";

    fs::write(tmp_path, content).unwrap();

    rules.load_file(tmp_path);
    assert!(rules.get_rule("crlfprog").is_some());
    assert_eq!(rules.size(), 1);

    let _ = fs::remove_file(tmp_path);
}
