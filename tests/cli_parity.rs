use assert_cmd::Command;

// C++ tests:
// Argument Parser -> Default Value
// Argument Parser -> Parse unknown optional argument
// Argument Parser -> Parse a string argument with value
// Argument Parser -> Parse a string argument without default value

#[test]
fn test_cli_default_value() {
    // In ananicy-rs, the default behavior (no args) is to start the daemon.
    // It should parse correctly and not throw a clap error (code 2).
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    let exit_code = cmd.assert().get_output().status.code().unwrap_or(255);
    assert_ne!(exit_code, 2, "Default execution should not be a clap argument error");
}

#[test]
fn test_cli_unknown_argument() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    cmd.arg("--unknown-arg-12345");
    cmd.assert().failure().code(2);
}

#[test]
fn test_cli_string_argument_with_value() {
    // ananicy-rs takes --config
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    // Using --help and providing a config string
    cmd.arg("--config").arg("fake_config.toml").arg("--help");
    let exit_code = cmd.assert().get_output().status.code().unwrap_or(255);
    assert_eq!(exit_code, 0, "Should parse string argument successfully and exit 0 with help");
}

#[test]
fn test_cli_string_argument_without_default() {
    let mut cmd = Command::cargo_bin("ananicy-rs").unwrap();
    // Providing --config without a value should be a clap parse error
    cmd.arg("--config");
    cmd.assert().failure().code(2);
}
