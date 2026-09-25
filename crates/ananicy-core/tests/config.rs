//! Configuration file parsing behaviour of `ananicy-core`.
//!
//! These tests pin the `ananicy.conf` contract that the daemon itself relies on:
//! a file is parsed into a [`ConfigSnapshot`], unknown or malformed entries are
//! reported as diagnostics instead of aborting startup, and a missing file is
//! replaced by a generated default file.
//!
//! Two checked-in fixtures are used as documentation of the accepted syntax:
//!
//! * `fixtures/test-sampleconfig.txt` — every `apply_*`/`*_load` flag turned off.
//! * `fixtures/test-rulesconfig.txt`  — the default-enabled configuration, but
//!   with `cgroup_realtime_workaround` disabled.
//!
//! The exact key spelling is part of the on-disk format that is shared with the
//! historical `ananicy-cpp` configuration, so it is a compatibility requirement
//! rather than an implementation detail. See `docs/ANANICY_CPP_DIFFERENCES.md`.

use {
    ananicy_core::config::{Config, ConfigDiagnostic, ConfigSnapshot, LogLevel},
    std::{fs, path::Path},
};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn parse(contents: &str) -> ConfigSnapshot {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");
    fs::write(&path, contents).unwrap();
    ConfigSnapshot::parse_file(path).expect("valid configuration must parse")
}

fn parse_with_diagnostics(contents: &str) -> (ConfigSnapshot, Vec<ConfigDiagnostic>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ananicy.conf");
    fs::write(&path, contents).unwrap();
    ConfigSnapshot::parse_file_with_diagnostics(path).expect("valid file must be readable")
}

#[test]
fn sample_config_fixture_is_applied_in_full() {
    let config = Config::load_file(fixture("test-sampleconfig.txt"), true).unwrap();
    let config = config.get();

    // "apply" options.
    assert!(!config.apply_nice);
    assert!(!config.apply_latnice);
    assert!(!config.apply_sched);
    assert!(!config.apply_ioclass);
    assert!(!config.apply_ionice);
    assert!(!config.apply_oom_score_adj);
    assert!(!config.apply_cgroups);
    assert!(!config.apply_cpuset);

    // "load" options.
    assert!(!config.cgroup_load);
    assert!(!config.type_load);
    assert!(!config.rule_load);

    // Logging, frequency, x3d and workaround.
    assert!(config.log_applied_rule);
    assert_eq!(config.loglevel, LogLevel::Error);
    assert_eq!(config.check_freq, 5);
    assert_eq!(config.x3d_mode, "frequency");
    assert!(!config.cgroup_realtime_workaround);
}

#[test]
fn default_enabled_config_fixture_is_applied_in_full() {
    let config = Config::load_file(fixture("test-rulesconfig.txt"), true).unwrap();
    let config = config.get();

    assert!(config.rule_load);
    assert!(config.type_load);
    assert!(config.cgroup_load);

    assert!(config.apply_nice);
    assert!(config.apply_latnice);
    assert!(config.apply_sched);
    assert!(config.apply_ioclass);
    assert!(config.apply_ionice);
    assert!(config.apply_oom_score_adj);
    assert!(config.apply_cgroups);

    // Keys the fixture does not mention keep their default value.
    assert!(
        config.apply_cpuset,
        "an omitted apply_cpuset keeps the default"
    );
    assert_eq!(
        config.x3d_mode, "auto",
        "an omitted x3d_mode keeps the default"
    );

    assert!(!config.log_applied_rule);
    assert_eq!(config.loglevel, LogLevel::Info);
    assert_eq!(config.check_freq, 60);
    assert!(!config.cgroup_realtime_workaround);
}

#[test]
fn empty_configuration_keeps_every_default() {
    assert_eq!(parse(""), ConfigSnapshot::default());
    assert_eq!(parse("\n\n   \n\t\n"), ConfigSnapshot::default());
    assert_eq!(
        parse("# only a comment\n   # and an indented one\n"),
        ConfigSnapshot::default()
    );
}

#[test]
fn apply_cgroup_is_the_documented_key_for_cgroup_application() {
    // The configuration key is `apply_cgroup` (singular) even though the
    // snapshot field is `apply_cgroups`. Both spellings have shipped in
    // `ananicy.conf` files in the wild, so the accepted one is pinned here.
    assert!(!parse("apply_cgroup=false\n").apply_cgroups);
    assert!(parse("apply_cgroup=true\n").apply_cgroups);

    let (config, diagnostics) = parse_with_diagnostics("apply_cgroups=true\n");
    assert!(diagnostics.iter().any(|d| matches!(
        d,
        ConfigDiagnostic::Warn(message) if message.contains("Unknown config key: apply_cgroups")
    )));
    assert!(
        config.apply_cgroups,
        "an unknown key must not change the default"
    );
}

#[test]
fn only_the_literal_true_enables_a_flag() {
    // Flags are compared against the literal string `true`; anything else is
    // false. This keeps `apply_nice=0` from silently enabling the attribute and
    // matches the shipped configuration format.
    assert!(!parse("apply_nice=0\n").apply_nice);
    assert!(!parse("apply_nice=1\n").apply_nice);
    assert!(!parse("apply_nice=yes\n").apply_nice);
    assert!(!parse("apply_nice=True\n").apply_nice);
    assert!(parse("apply_nice=true\n").apply_nice);
    assert!(parse("apply_nice = true \n").apply_nice);
}

#[test]
fn lines_without_a_key_value_pair_are_ignored() {
    let (config, diagnostics) = parse_with_diagnostics("this is not a setting\napply_nice=false\n");
    assert!(
        !config.apply_nice,
        "the malformed line is skipped, the valid one is still applied"
    );
    assert!(
        diagnostics.is_empty(),
        "a line without '=' is not a configuration error: {diagnostics:?}"
    );
}

#[test]
fn unknown_keys_are_reported_and_do_not_abort_parsing() {
    let (config, diagnostics) =
        parse_with_diagnostics("definitely_not_a_key=1\napply_nice=true\nanother_unknown=2\n");
    let unknown: Vec<&String> = diagnostics
        .iter()
        .filter_map(|d| match d {
            ConfigDiagnostic::Warn(message) if message.starts_with("Unknown config key") => {
                Some(message)
            }
            _ => None,
        })
        .collect();

    assert_eq!(unknown.len(), 2, "both unknown keys must be reported");
    assert!(config.apply_nice, "parsing continues after an unknown key");
}

#[test]
fn invalid_check_freq_is_rejected_and_keeps_the_default() {
    let (config, diagnostics) = parse_with_diagnostics("check_freq=fast\n");
    assert_eq!(config.check_freq, 60, "the default is kept");
    assert!(
        diagnostics
            .iter()
            .any(|d| matches!(d, ConfigDiagnostic::Error(message) if message.contains("Invalid check_freq"))),
        "an invalid check_freq is an error, not a warning: {diagnostics:?}"
    );
}

#[test]
fn check_freq_accepts_the_full_unsigned_range() {
    assert_eq!(parse("check_freq=0\n").check_freq, 0);
    assert_eq!(parse("check_freq=4294967295\n").check_freq, u32::MAX);
    assert_eq!(
        parse("check_freq=-1\n").check_freq,
        60,
        "negative is invalid"
    );
    assert_eq!(parse("check_freq=1.5\n").check_freq, 60, "not an integer");
}

#[test]
fn last_assignment_of_a_key_wins() {
    let config = parse("apply_nice=true\napply_nice=false\nx3d_mode=one\nx3d_mode=two\n");
    assert!(!config.apply_nice);
    assert_eq!(config.x3d_mode, "two");
}

#[test]
fn x3d_mode_is_kept_verbatim() {
    assert_eq!(parse("x3d_mode=auto\n").x3d_mode, "auto");
    assert_eq!(parse("x3d_mode=frequency\n").x3d_mode, "frequency");
    assert_eq!(
        parse("x3d_mode=Disabled\n").x3d_mode,
        "Disabled",
        "the mode is a free-form string; validation belongs to the topology layer"
    );
}

#[test]
fn parsing_a_missing_file_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("not-there.conf");

    let error = ConfigSnapshot::parse_file(&path).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(!path.exists(), "parsing must not create the file");

    let (config, diagnostics) = Config::load_file_with_diagnostics(&path, true).unwrap();
    assert!(
        config.get().apply_nice,
        "defaults are used when the file is absent"
    );
    assert!(
        diagnostics.iter().any(
            |d| matches!(d, ConfigDiagnostic::Info(message) if message.contains("does not exist"))
        ),
        "the fallback to defaults is reported: {diagnostics:?}"
    );
    assert!(path.exists(), "the default configuration is written back");
    assert_eq!(
        ConfigSnapshot::parse_file(&path).unwrap(),
        ConfigSnapshot::default(),
        "the written default configuration parses back to the defaults"
    );
}

#[test]
fn latnice_is_disabled_when_the_kernel_does_not_support_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("latnice.conf");
    fs::write(
        &path,
        "apply_latnice=true\napply_nice=true\nlog_applied_rule=true\n",
    )
    .unwrap();

    let config = Config::load_file(&path, false).unwrap();
    assert!(
        !config.get().apply_latnice,
        "an unsupported latency_nice must never be requested"
    );
    assert!(config.get().apply_nice, "other flags are unaffected");

    let supported = Config::load_file(&path, true).unwrap();
    assert!(supported.get().apply_latnice);
}

#[test]
fn apply_cpu_weight_is_read_from_the_configuration() {
    // The mirror of `nice` into `cpu.weight` is on by default, and the one
    // switch that turns it off is a plain `apply_*` key like the others.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ananicy.conf");
    std::fs::write(&path, "apply_cpu_weight=false\n").unwrap();

    let (config, diagnostics) = Config::load_file_with_diagnostics(&path, true).unwrap();
    assert!(!config.get().apply_cpu_weight);
    assert!(
        diagnostics.is_empty(),
        "the key is a documented one, not an unknown key: {diagnostics:?}"
    );
    assert!(
        ConfigSnapshot::default().apply_cpu_weight,
        "the mirror stays on unless it is switched off"
    );
}

#[test]
fn check_disks_schedulers_is_read_from_the_configuration() {
    assert!(parse("").check_disks_schedulers, "the default stays on");
    assert!(
        !parse("check_disks_schedulers=false\n").check_disks_schedulers,
        "and it can be switched off"
    );
    assert!(
        parse("check_disks_schedulers=true\n").check_disks_schedulers,
        "and switched back on"
    );
}

#[test]
fn the_default_configuration_mentions_the_disk_check() {
    // A key the daemon acts on belongs in the file it writes, or a
    // configuration read back from it will not round-trip.
    assert!(
        ConfigSnapshot::default()
            .to_config_string()
            .contains("check_disks_schedulers=true")
    );
}
