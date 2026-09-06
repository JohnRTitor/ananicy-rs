use {assert_cmd::Command, predicates::prelude::*};

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("ANother Auto NICe daemon rewrite"));
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
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("start")
        .assert()
        .failure()
        .stdout(predicate::str::contains("This program must be run as root"));
}

#[test]
fn test_cli_unknown_action_non_root() {
    if rustix::process::geteuid().as_raw() == 0 {
        return; // skip if running as root
    }
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("nonsense")
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "Unknown action requested: nonsense",
        ))
        .stdout(predicate::str::contains("This program must be run as root"));
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
