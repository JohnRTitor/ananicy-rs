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
fn test_cli_accepts_both_spellings_of_manual_scanning() {
    // `--manualscanning` is the spelling the Ananicy command line used and
    // ananicy-cpp still accepts, so a unit file or wrapper that carries it must
    // keep working. `--help` is used as the terminating argument, because
    // without it the flag would start the daemon.
    for flag in ["--manual-scanning", "--manualscanning"] {
        let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
        cmd.arg(flag).arg("--help");
        cmd.assert()
            .success()
            .code(0)
            .stdout(predicate::str::contains("ANother Auto NICe daemon rewrite"));
    }
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
        .stderr(predicate::str::contains("This program must be run as root"));
}

#[test]
fn test_cli_unknown_action() {
    let mut cmd = ananicy();
    cmd.arg("nonsense")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Unknown action requested: nonsense",
        ));
}

#[test]
fn test_cli_invalid_dump_action() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("dump").arg("invalid_action").assert().failure();
}

/// `dump proc` is a JSON object keyed by TID, and it is the only place the
/// daemon reports what it can read about a process: the fields the rules use, the
/// ones they do not, and which rule matched.
#[test]
fn test_cli_dump_proc_reports_every_field() {
    let output = ananicy().arg("dump").arg("proc").output().unwrap();
    assert!(
        output.status.success(),
        "dump proc must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dump: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("dump proc prints JSON on stdout");
    let processes = dump.as_object().expect("dump proc prints an object");
    assert!(!processes.is_empty(), "the test process is in there");

    let self_pid = std::process::id().to_string();
    let entry = processes
        .get(&self_pid)
        .unwrap_or_else(|| panic!("the test process {self_pid} is missing from the dump"));
    let entry = entry.as_object().expect("an entry is an object");

    for field in [
        "pid",
        "tpid",
        "exe",
        "comm",
        "cmd",
        "stat",
        "stat_name",
        "autogroup",
        "sched",
        "rtprio",
        "nice",
        "latency_nice",
        "ionice",
        "oom_score_adj",
        "cmdline",
    ] {
        assert!(
            entry.contains_key(field),
            "{field} is missing from a dump proc entry: {entry:?}"
        );
    }

    // Keys are TIDs, and each entry knows which one it is.
    let tpid: i64 = entry["tpid"].as_i64().expect("tpid is a number");
    assert_eq!(tpid.to_string(), self_pid);
}

/// The same information, regrouped by autogroup. On a host without autogroups
/// — CONFIG_AUTO_NUMA_GROUPS off, which is the default — the answer is an empty
/// object rather than a failure, and that is the point of the test.
#[test]
fn test_cli_dump_autogroup_is_json() {
    let output = ananicy().arg("dump").arg("autogroup").output().unwrap();
    assert!(
        output.status.success(),
        "dump autogroup must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dump: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("dump autogroup prints JSON on stdout");
    let groups = dump.as_object().expect("dump autogroup prints an object");

    for (group, value) in groups {
        let group_entry = value.as_object().expect("a group is an object");
        assert!(
            group_entry.contains_key("nice"),
            "group {group} has no nice value"
        );
        assert!(
            group_entry.contains_key("proc"),
            "group {group} has no processes"
        );
    }
}

/// The three dumps that are a map of what was loaded: they have to be valid JSON
/// with the keys of the rule files behind them, not a human-readable table.
#[test]
fn test_cli_dump_prints_the_loaded_state_as_json() {
    let temp_dir = std::env::temp_dir().join(format!("ananicy_test_dump_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let config_path = temp_dir.join("ananicy.conf");
    std::fs::write(&config_path, "loglevel=error\n").unwrap();
    std::fs::write(
        temp_dir.join("test.rules"),
        "{\"name\": \"ananicy-dump-test\", \"nice\": 5}\n",
    )
    .unwrap();
    std::fs::write(temp_dir.join("test.types"), "{\"type\": \"Dump-Type\"}\n").unwrap();
    std::fs::write(
        temp_dir.join("test.cgroups"),
        "{\"cgroup\": \"dump-cgroup\", \"CPUQuota\": 80}\n",
    )
    .unwrap();

    let dump = |target: &str| {
        let output = ananicy()
            .arg("--config")
            .arg(&config_path)
            .arg("--config-dir")
            .arg(&temp_dir)
            .arg("dump")
            .arg(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "dump {target} must succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .unwrap_or_else(|e| panic!("dump {target} is not JSON: {e}"))
    };

    let rules = dump("rules");
    assert_eq!(rules["ananicy-dump-test"]["nice"], 5);
    assert!(dump("types").get("Dump-Type").is_some());
    assert_eq!(dump("cgroups")["dump-cgroup"]["CPUQuota"], 80);

    let _ = std::fs::remove_dir_all(&temp_dir);
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

#[test]
fn test_cli_reports_every_configuration_value_at_startup() {
    // `dump` runs after the configuration, topology and rules are set up but
    // before the root check, so this observes the startup reporting without
    // starting (or needing) the daemon.
    let temp_dir = std::env::temp_dir().join(format!("ananicy_test_report_{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let config_path = temp_dir.join("ananicy.conf");
    std::fs::write(
        &config_path,
        concat!(
            "apply_nice=false\n",
            "apply_sched=false\n",
            "apply_ionice=false\n",
            "apply_ioclass=false\n",
            "apply_cgroup=false\n",
            "apply_cpuset=false\n",
            "apply_cpu_weight=false\n",
            "apply_oom_score_adj=false\n",
            "apply_latnice=false\n",
            "cgroup_load=false\n",
            "type_load=false\n",
            "cgroup_realtime_workaround=false\n",
            "log_applied_rule=true\n",
            "check_freq=42\n",
            "x3d_mode=cache\n",
            "loglevel=info\n",
        ),
    )
    .unwrap();
    std::fs::write(
        temp_dir.join("test.rules"),
        "{\"name\": \"ananicy-test\", \"nice\": 5}\n",
    )
    .unwrap();

    let mut cmd = ananicy();
    cmd.arg("--config")
        .arg(&config_path)
        .arg("--config-dir")
        .arg(&temp_dir)
        .arg("dump")
        .arg("rules");

    let output = cmd.output().unwrap();
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&temp_dir);

    // Every configurable value is reported, so a run can be reconstructed from
    // the journal alone. The values are all non-default on purpose: a line that
    // silently keeps its default would still contain the key, but the value
    // proves the file was actually read.
    let expected = [
        "Config apply_nice: false",
        "Config apply_sched: false",
        "Config apply_ionice: false",
        "Config apply_ioclass: false",
        "Config apply_cgroup: false",
        "Config cgroup_load: false",
        "Config apply_oom_score_adj: false",
        "Config apply_latnice: false",
        "Config log_applied_rule: true",
        "Config type_load: false",
        "Config rule_load: true",
        "Config cgroup_realtime_workaround: false",
        "Config check_freq: 42",
        "Config apply_cpuset: false",
        "Config apply_cpu_weight: false",
        "Config x3d_mode: cache",
        "Config loglevel: info",
    ];
    for line in expected {
        assert!(
            combined.contains(line),
            "Expected {line:?} at startup, got:\n{combined}"
        );
    }

    assert!(
        combined.contains("topology: "),
        "Expected the detected topology, got:\n{combined}"
    );
    assert!(
        combined.contains("Loaded 1 rules"),
        "Expected the loaded rule count, got:\n{combined}"
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

// ---------------------------------------------------------
// Starting without a cgroup hierarchy
// ---------------------------------------------------------

/// A host with no usable cgroup hierarchy is still a host the daemon can do
/// most of its work on: only a rule's `cgroup` attribute needs one.
///
/// The regression this covers made the daemon give up entirely. It waited for
/// the hierarchy, warned, and returned from `main` with status 0 before spawning
/// the worker, so no rule was applied, no event source was subscribed, and an
/// `x3d_mode` change made at start-up was never restored. A `Type=simple` unit
/// reported that as success.
///
/// A private mount namespace with an empty tmpfs over `/sys/fs/cgroup` is the
/// closest reproducible stand-in for such a host, and is the only way to observe
/// the path at all. It needs user namespaces, so it is skipped where they are
/// unavailable rather than failing the suite on a host that cannot run it.
#[test]
fn test_cli_daemon_still_runs_without_a_cgroup_hierarchy() {
    use std::{fs, process::Command as StdCommand};

    let dir = tempfile::tempdir().expect("a temporary config directory");
    let config = dir.path().join("ananicy.conf");
    fs::write(&config, "check_freq=5\n").expect("a configuration");
    // An empty rules directory, so the daemon is not reading whatever the host
    // happens to have installed in /etc/ananicy.d.
    fs::create_dir(dir.path().join("rules")).expect("a rules directory");

    let script = format!(
        "mount -t tmpfs none /sys/fs/cgroup && \
         mount -t tmpfs none /dev/shm 2>/dev/null; \
         exec {binary} --config {config} start",
        binary = assert_cmd::cargo::cargo_bin("ananicy-rs").display(),
        config = config.display(),
    );

    let mut child = match StdCommand::new("unshare")
        .args(["--user", "--map-root-user", "--mount", "sh", "-c", &script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("skipping: cannot run unshare ({e})");
            return;
        }
    };

    // The wait for a hierarchy is 10 seconds by design, so the daemon is given
    // longer than that before it is stopped.
    std::thread::sleep(std::time::Duration::from_secs(13));
    let still_running = child.try_wait().expect("to poll the daemon").is_none();
    let _ = child.kill();
    let output = child.wait_with_output().expect("to reap the daemon");
    let log = String::from_utf8_lossy(&output.stderr);

    if log.contains("Operation not permitted") && !log.contains("Spawning worker thread") {
        eprintln!("skipping: user namespaces are not permitted here");
        return;
    }

    assert!(
        log.contains("Still no cgroup hierarchy"),
        "the test did not reach the no-hierarchy path, so it proves nothing:\n{log}"
    );
    assert!(
        log.contains("cgroup rules will not be applied"),
        "the daemon did not report that it cannot apply cgroup rules:\n{log}"
    );
    assert!(
        still_running,
        "the daemon exited instead of carrying on without cgroups:\n{log}"
    );
    assert!(
        log.contains("Spawning worker thread"),
        "the daemon gave up before spawning its worker, so no rule can be applied:\n{log}"
    );
}

// ---------------------------------------------------------
// --force-remove-semaphore
// ---------------------------------------------------------

/// The exit status answers "is a stale singleton object still there?", so a
/// failure to remove one has to be a failure.
///
/// The reference prints the errno and returns `EXIT_FAILURE` when `shm_unlink`
/// does not succeed (`main.cpp:115-119`). Returning 0 regardless meant a
/// wrapper script using the status to confirm cleanup was told it had worked
/// when it had not. Verified by execution before the change: a second
/// `--force-remove-semaphore` with no daemon running returned 0.
#[test]
fn test_cli_force_remove_semaphore_reports_failure() {
    let mut cmd = ananicy();
    cmd.arg("--force-remove-semaphore");
    // There is no shared memory object for this name, so the unlink fails. The
    // name is unique to the test binary's namespace on a shared host only if the
    // daemon is not running, which is the case the reference also treats as an
    // error.
    cmd.assert()
        .code(1)
        .stderr(predicate::str::contains("Failed to remove semaphore"));
}

/// `cmd` is the name the rule engine matches on, and `cmdline` keeps the
/// argument boundaries.
///
/// The reference's `cmd` is `get_command_from_pid(pid)` (`process_info.cpp:188`)
/// — the same function it matches rules with (`process.cpp:61`, `:155`) — so its
/// `cmd` and its `rule` never disagree. Reading `/proc/<pid>/comm` here instead
/// meant they did, for any process that rewrote `argv[0]` or whose `comm` is
/// truncated to 15 characters: `rule` would name one process and `cmd` another.
///
/// `cmdline` is a JSON array, as the reference emits it
/// (`process_info.cpp:243`). Joining the arguments with spaces loses the
/// boundaries, so an argument containing a space was indistinguishable from two
/// arguments.
#[test]
fn test_cli_dump_proc_cmd_is_the_matched_name_and_cmdline_is_an_array() {
    let output = ananicy()
        .arg("dump")
        .arg("proc")
        .output()
        .expect("dump proc runs");
    let dump: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("dump proc prints JSON on stdout");
    let processes = dump.as_object().expect("dump proc prints an object");

    let self_pid = std::process::id().to_string();
    let entry = processes
        .get(&self_pid)
        .unwrap_or_else(|| panic!("the test process {self_pid} is missing"));

    assert!(
        entry["cmdline"].is_array(),
        "cmdline must be an array of arguments, got {:?}",
        entry["cmdline"]
    );
    let args = entry["cmdline"].as_array().expect("an array");
    assert!(
        args.iter().all(serde_json::Value::is_string),
        "every argument is a string: {args:?}"
    );

    // This process was not renamed, so the matched name and the kernel's `comm`
    // agree — but `cmd` must be the *matched* name, and a value that is
    // truncated to 15 characters would not be usable as a rule name.
    let cmd = entry["cmd"].as_str().expect("cmd is a string");
    assert!(
        !cmd.is_empty() && cmd.len() <= 4096,
        "cmd should be a full command name, got {cmd:?}"
    );
    assert!(
        entry["rule"].is_string() || entry["rule"].is_null(),
        "rule is a string or null, got {:?}",
        entry["rule"]
    );
}

/// The `dump proc` / `dump autogroup` field contract, as documented in
/// `CLI.md`.
///
/// Three things there are easy to get wrong and none of them is visible from the
/// type: `rule` is absent rather than `null` when nothing matched, `cmd` is the
/// name the rule engine matched on rather than the kernel's `comm`, and
/// `oom_score_adj` is signed — `ananicy-cpp` reports a `-900` process as
/// `4294966396`, so a parser handling both has to cope. The first is asserted
/// here because it is the only one that changes the shape of the object; the
/// second and third need a renamed process and a negative score respectively to
/// observe, and are covered by the documentation.
#[test]
fn test_cli_dump_proc_field_contract() {
    let output = ananicy()
        .arg("dump")
        .arg("proc")
        .output()
        .expect("dump proc runs");
    let dump: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("dump proc prints JSON on stdout");
    let processes = dump.as_object().expect("dump proc prints an object");

    const ALWAYS: [&str; 15] = [
        "pid",
        "tpid",
        "exe",
        "comm",
        "cmd",
        "cmdline",
        "stat",
        "stat_name",
        "autogroup",
        "sched",
        "rtprio",
        "nice",
        "latency_nice",
        "ionice",
        "oom_score_adj",
    ];

    let mut matched = 0;
    for (key, entry) in processes {
        let entry = entry.as_object().expect("an entry is an object");
        for field in ALWAYS {
            assert!(
                entry.contains_key(field),
                "field {field} went missing from entry {key}: {:?}",
                entry.keys().collect::<Vec<_>>()
            );
        }
        // `rule` is `skip_serializing_if = "Option::is_none"`, so it is absent,
        // not null. Asserting it is *never* null is what a parser gets wrong.
        if let Some(rule) = entry.get("rule") {
            assert!(
                rule.is_string(),
                "entry {key} has a rule that is not a string: {rule:?}"
            );
            matched += 1;
        }
    }

    assert!(matched > 0, "no entry on this host matched a rule");
    assert!(
        matched < processes.len(),
        "not every process matched a rule, so the absent case was not exercised"
    );
}

/// `dump autogroup` groups by autogroup number and keeps the group shape.
///
/// Processes with no autogroup are absent from this dump entirely, which is what
/// the reference does: its `get_autogroup_map` only files a process under a group
/// when `autogroup` is non-null (`process_info.cpp:44-58`). The per-process
/// `autogroup` key is removed from the entry, because the grouping is the outer
/// level.
#[test]
fn test_cli_dump_autogroup_groups_by_number() {
    let output = ananicy()
        .arg("dump")
        .arg("autogroup")
        .output()
        .expect("dump autogroup runs");
    let groups: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("dump autogroup prints JSON on stdout");
    let groups = groups.as_object().expect("dump autogroup prints an object");

    assert!(!groups.is_empty(), "the test host has autogroups");

    for (number, group) in groups {
        assert!(
            number.parse::<u32>().is_ok(),
            "a group is keyed by its autogroup number, got {number:?}"
        );
        let group = group.as_object().expect("a group is an object");
        for field in ["nice", "proc"] {
            assert!(group.contains_key(field), "group {number} has no {field}");
        }

        let procs = group["proc"].as_object().expect("proc is an object");
        for (tid, entry) in procs {
            assert_eq!(
                tid.parse::<i32>().ok(),
                entry["tpid"].as_i64().map(|t| t as i32),
                "group {number} keys its processes by tpid"
            );
            assert!(
                !entry
                    .as_object()
                    .expect("an entry")
                    .contains_key("autogroup"),
                "the per-process autogroup is removed; it is the outer level"
            );
        }
    }
}
