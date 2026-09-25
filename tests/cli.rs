//! End-to-end tests of the `ananicy-rs` command line.
//!
//! These drive the real binary, so they assert the process' observable
//! behaviour: exit status and what is written to stdout/stderr. They never
//! require a configuration in `/etc/ananicy.d`, a running daemon or root, and
//! the systemd-mode cases neutralise the supervision variables first so they do
//! not depend on how the test runner itself was started.

use {assert_cmd::Command, predicates::prelude::*};

/// Builds a command with all systemd supervision variables removed, so that
/// assertions about the logger or the systemd mode do not depend on how the
/// test runner itself was started (a systemd service, a transient scope, or a
/// CI container without an init system).
fn ananicy() -> Command {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    for var in ["INVOCATION_ID", "NOTIFY_SOCKET", "JOURNAL_STREAM"] {
        cmd.env_remove(var);
    }
    cmd
}

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("ANother Auto NICe daemon rewrite"));
}

// ---------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------

#[test]
fn test_cli_string_argument_with_value() {
    // A global option before a sub-command: bpaf accepts the pair and then
    // prints the help text instead of starting the daemon.
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--config").arg("fake_config.toml").arg("--help");
    cmd.assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("ANother Auto NICe daemon rewrite"));
}

#[test]
fn test_cli_string_argument_without_default() {
    // `--config` takes a value, so using it without one is a usage error.
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--config");
    cmd.assert().failure().code(2);
}

#[test]
fn test_cli_unknown_argument() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--unknown-arg-12345");
    cmd.assert().failure().code(2);
}

#[test]
fn test_cli_unknown_argument_after_a_valid_one() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--config")
        .arg("fake_config.toml")
        .arg("--unknown-arg-12345");
    cmd.assert().failure().code(2);
}

#[test]
fn test_cli_help_documents_systemd_flags() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--systemd"))
        .stdout(predicate::str::contains("--no-systemd"));
}

#[test]
fn test_cli_bare_invocation() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("ANother Auto NICe daemon rewrite"));
}

#[test]
fn test_cli_start_non_root() {
    if rustix::process::geteuid().as_raw() == 0 {
        return; // skip if running as root
    }
    let mut cmd = ananicy();
    cmd.arg("start")
        .assert()
        .failure()
        .stdout(predicate::str::contains("This program must be run as root"));
}

#[test]
fn test_cli_unknown_action() {
    let mut cmd = ananicy();
    cmd.arg("nonsense")
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "Unknown action requested: nonsense",
        ));
}

#[test]
fn test_cli_invalid_dump_action() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("dump").arg("invalid_action").assert().failure();
}

#[test]
fn test_cli_debug_cgroups_accepted() {
    // Reads only world-readable files (/etc/mtab, /proc/self/mounts,
    // /proc/<pid>/cgroup), so unlike `start` this must succeed without root,
    // which exits before the root check.
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("debug")
        .arg("cgroups")
        .assert()
        .success()
        .stdout(predicate::str::contains("#### BEGIN /etc/mtab #####"))
        .stdout(predicate::str::contains("Unit name:"))
        .stdout(predicate::str::contains("Cgroup:"));
}

#[test]
fn test_cli_debug_missing_sub_action() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("debug")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "A sub-action must be specified for debug.",
        ));
}

#[test]
fn test_cli_debug_unknown_sub_action_is_silent_success() {
    // An unrecognized debug sub-action is *not* an error (unlike `dump`'s
    // unknown sub-action handling) — it just exits successfully having printed
    // nothing extra.
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("debug")
        .arg("nonsense")
        .assert()
        .success()
        .stdout(predicate::str::contains("#### BEGIN").not())
        .stdout(predicate::str::contains("Unit name:").not());
}

#[test]
fn test_cli_help_does_not_mention_debug() {
    // The `debug` action is intentionally undocumented (only "dump [sub-action]"
    // and "start" are listed), so it must stay out of --help here too.
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("debug").not());
}

#[test]
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn test_cli_completions_bash() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("completions")
        .arg("bash")
        .assert()
        .success()
        .stdout(predicate::str::contains("_bpaf_dynamic_completion"))
        .stdout(predicate::str::contains("ananicy-rs"));
}

#[test]
fn test_cli_completions_zsh() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("completions")
        .arg("zsh")
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef ananicy-rs"))
        .stdout(predicate::str::contains("--bpaf-complete-rev="));
}

#[test]
fn test_cli_completions_fish() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("completions")
        .arg("fish")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "function _bpaf_dynamic_completion",
        ))
        .stdout(predicate::str::contains("ananicy-rs"));
}

#[test]
fn test_cli_completions_elvish() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("completions")
        .arg("elvish")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "edit:completion:arg-completer[ananicy-rs]",
        ));
}

#[test]
fn test_cli_completions_invalid_shell() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("completions")
        .arg("cmd.exe")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "error: invalid shell 'cmd.exe'; expected one of: bash, zsh, fish, elvish",
        ));
}

/// The `dump` sub-action is a closed set, so completion must offer the valid
/// targets instead of the positional's metavar.
#[test]
fn test_cli_completion_offers_dump_targets() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    // bpaf reports completion requests on stdout with a non-zero status.
    cmd.arg("--bpaf-complete-rev=8")
        .arg("dump")
        .arg("")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("'rules'"))
        .stdout(predicate::str::contains("'autogroup'"))
        .stdout(predicate::str::contains("SUB_ACTION").not());
}

#[test]
fn test_cli_completion_filters_dump_targets_by_prefix() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--bpaf-complete-rev=8")
        .arg("dump")
        .arg("cg")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("'cgroups'"))
        .stdout(predicate::str::contains("'rules'").not());
}

#[test]
fn test_cli_loglevel_config_propagation() {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join(format!("ananicy_test_config_{}.conf", std::process::id()));
    std::fs::write(&config_path, "loglevel=debug").unwrap();

    let mut cmd = ananicy();
    cmd.arg("--config").arg(&config_path).arg("start");

    // The daemon might exit if it needs root or IPC fails, but it should print
    // "Config loglevel: debug" and other debug messages before failing.
    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    let _ = std::fs::remove_file(&config_path);

    // debug level output is typically on stderr or stdout depending on fmt
    let combined = format!("{}\n{}", stdout, stderr);
    assert!(
        combined.contains("DEBUG"),
        "Expected DEBUG level logs, got:\n{}",
        combined
    );
    assert!(
        combined.contains("loglevel: debug"),
        "Expected parsed config loglevel, got:\n{}",
        combined
    );
}

/// Extracts the `Systemd integration: ...` line printed by `debug cgroups`.
fn status_line(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .find_map(|line| line.strip_prefix("Systemd integration: "))
        .expect("debug cgroups must report the systemd mode")
        .to_string()
}

/// Runs `debug cgroups` with the given environment overrides and returns the
/// reported systemd mode.
fn resolved_systemd_mode(vars: &[(&str, &str)], args: &[&str]) -> String {
    let mut cmd = ananicy();
    for (name, value) in vars {
        cmd.env(name, value);
    }
    let output = cmd.args(args).args(["debug", "cgroups"]).output().unwrap();
    assert!(
        output.status.success(),
        "ananicy-rs {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    status_line(&output.stdout)
}

/// Builds without the `systemd` cargo feature cannot use the integration at
/// all, so they report that instead of the resolved mode.
const FEATURE_OFF: &str = "disabled (built without the `systemd` feature)";

fn assert_systemd_mode(reported: &str, with_systemd_feature: &str) {
    let expected = if cfg!(feature = "systemd") {
        with_systemd_feature
    } else {
        FEATURE_OFF
    };
    assert_eq!(reported, expected);
}

#[test]
fn test_cli_systemd_mode_reported_in_debug_output() {
    // The resolved mode is always reported, whatever the environment looks like.
    let mut cmd = ananicy();
    cmd.arg("debug")
        .arg("cgroups")
        .assert()
        .success()
        .stdout(predicate::str::contains("Systemd integration: disabled"));
}

#[test]
fn test_cli_systemd_not_enabled_because_the_host_uses_systemd() {
    // Being started on a systemd host is not evidence of anything: without
    // supervision variables the mode stays off. This holds on systemd hosts,
    // in containers and on non-systemd hosts alike.
    assert_systemd_mode(
        &resolved_systemd_mode(&[], &[]),
        "disabled (no systemd service manager detected)",
    );
}

#[test]
fn test_cli_systemd_auto_detection_follows_the_environment() {
    // `$INVOCATION_ID` is what a systemd service (system or user instance)
    // gets since systemd 232. Whether the result is "enabled" or whether the
    // transient-scope veto applies depends on the cgroup the *test runner*
    // lives in, which must not make the test depend on the developer's init
    // system: the invariants asserted here are that the variable drives the
    // decision and that the outcome is one of the two documented ones.
    if !cfg!(feature = "systemd") {
        return; // the mode is compiled out in this build
    }

    let without = resolved_systemd_mode(&[], &[]);
    let with = resolved_systemd_mode(
        &[("INVOCATION_ID", "0123456789abcdef0123456789abcdef")],
        &[],
    );

    assert_ne!(
        with, without,
        "a supervised process must resolve differently from an unsupervised one"
    );
    assert!(
        with.ends_with("enabled (auto-detected from $INVOCATION_ID)")
            || with.ends_with("disabled (member of a transient .scope, not a service)"),
        "unexpected decision: {with}"
    );
}

#[test]
fn test_cli_systemd_auto_detection_uses_notify_socket_and_journal_stream() {
    // The fallback evidence must be honoured as well, i.e. the decision is
    // never derived from the host, only from what a service manager handed to
    // this process.
    if !cfg!(feature = "systemd") {
        return; // the mode is compiled out in this build
    }

    for (name, value) in [
        ("NOTIFY_SOCKET", "/run/systemd/notify"),
        ("JOURNAL_STREAM", "12:3456"),
    ] {
        let without = resolved_systemd_mode(&[], &[]);
        let with = resolved_systemd_mode(&[(name, value)], &[]);
        assert_ne!(with, without, "${name} was ignored by the detection");
    }
}

#[test]
fn test_cli_systemd_ignores_empty_environment_values() {
    // Empty values must not be mistaken for evidence: an `Environment=`
    // assignment with an empty value cannot force the mode on.
    assert_systemd_mode(
        &resolved_systemd_mode(
            &[
                ("INVOCATION_ID", ""),
                ("NOTIFY_SOCKET", ""),
                ("JOURNAL_STREAM", ""),
            ],
            &[],
        ),
        "disabled (no systemd service manager detected)",
    );
}

#[test]
fn test_cli_systemd_flag_forces_systemd_mode() {
    // Backwards compatibility: `ananicy-rs --systemd` still works, and the
    // flag stays valueless so that `--systemd start` keeps parsing.
    assert_systemd_mode(
        &resolved_systemd_mode(&[], &["--systemd"]),
        "enabled (forced by --systemd)",
    );
}

#[test]
fn test_cli_no_systemd_overrides_detection() {
    assert_systemd_mode(
        &resolved_systemd_mode(
            &[("INVOCATION_ID", "0123456789abcdef0123456789abcdef")],
            &["--no-systemd"],
        ),
        "disabled (forced by --no-systemd)",
    );
}

#[test]
fn test_cli_systemd_flags_are_mutually_exclusive() {
    let mut cmd = ananicy();
    cmd.arg("--systemd")
        .arg("--no-systemd")
        .arg("debug")
        .arg("cgroups")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "error: --systemd and --no-systemd are mutually exclusive",
        ));
}
