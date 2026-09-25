use {
    ananicy_core::config::{Config, ConfigDiagnostic, ConfigSnapshot, LogLevel},
    std::fs,
};

fn parse(contents: &str) -> ConfigSnapshot {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");
    fs::write(&path, contents).unwrap();
    ConfigSnapshot::parse_file(path).unwrap()
}

#[test]
fn log_applied_rule_defaults_to_disabled() {
    assert!(!ConfigSnapshot::default().log_applied_rule);
}

#[test]
fn log_applied_rule_is_parsed_from_both_boolean_values() {
    assert!(!parse("log_applied_rule = false\n").log_applied_rule);
    assert!(parse("log_applied_rule = true\n").log_applied_rule);
}

#[test]
fn all_documented_log_levels_are_parsed() {
    let levels = [
        ("trace", LogLevel::Trace),
        ("debug", LogLevel::Debug),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
        ("critical", LogLevel::Critical),
    ];

    for (value, expected) in levels {
        assert_eq!(parse(&format!("loglevel = {}\n", value)).loglevel, expected);
    }
    assert_eq!(parse("loglevel = DEBUG\n").loglevel, LogLevel::Debug);
    assert_eq!(parse("loglevel = fatal\n").loglevel, LogLevel::Critical);
}

#[test]
fn log_levels_map_to_ordered_tracing_levels() {
    use tracing::Level;

    let levels = [
        (LogLevel::Trace, Level::TRACE),
        (LogLevel::Debug, Level::DEBUG),
        (LogLevel::Info, Level::INFO),
        (LogLevel::Warn, Level::WARN),
        (LogLevel::Error, Level::ERROR),
        (LogLevel::Critical, Level::ERROR),
    ];

    for (configured, emitted) in levels {
        assert_eq!(Level::from(&configured), emitted);
    }
}

#[test]
fn invalid_log_level_falls_back_to_info() {
    assert_eq!(parse("loglevel = verbose\n").loglevel, LogLevel::Info);
}

#[test]
fn invalid_log_level_reports_a_diagnostic_and_falls_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");
    fs::write(&path, "loglevel = verbose\n").unwrap();

    let (snapshot, diagnostics) = ConfigSnapshot::parse_file_with_diagnostics(path).unwrap();

    assert_eq!(snapshot.loglevel, LogLevel::Info);
    assert!(
        matches!(diagnostics.as_slice(), [ConfigDiagnostic::Warn(message)] if message.contains("Unknown loglevel"))
    );
}

#[test]
fn generated_config_round_trips_logging_and_apply_flags() {
    let expected = ConfigSnapshot {
        log_applied_rule: true,
        apply_cgroups: false,
        loglevel: LogLevel::Warn,
        ..ConfigSnapshot::default()
    };
    let actual = parse(&expected.to_config_string());

    assert_eq!(actual, expected);
}

#[test]
fn failed_reload_preserves_the_previous_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");
    fs::write(&path, "log_applied_rule = true\nloglevel = debug\n").unwrap();
    let config = Config::load_file(&path, true).unwrap();

    fs::remove_file(&path).unwrap();
    assert!(config.reload_file(&path, true).is_err());
    assert!(config.get().log_applied_rule);
    assert_eq!(config.get().loglevel, LogLevel::Debug);
}

#[test]
fn reload_replaces_logging_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");

    fs::write(&path, "log_applied_rule = true\nloglevel = debug\n").unwrap();
    let config = Config::load_file(&path, true).unwrap();
    assert!(config.get().log_applied_rule);
    assert_eq!(config.get().loglevel, LogLevel::Debug);

    fs::write(&path, "log_applied_rule = false\nloglevel = error\n").unwrap();
    config.reload_file(&path, true).unwrap();

    assert!(!config.get().log_applied_rule);
    assert_eq!(config.get().loglevel, LogLevel::Error);
}
