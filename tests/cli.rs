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
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}
