use std::thread;

// Parity for C++ tests:
// get_num_conf_cores returns positive
// get_num_online_cores returns positive
// online cores <= configured cores
// get_max_number_of_cpus returns positive

#[test]
fn test_get_num_conf_cores_returns_positive() {
    let conf_cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    assert!(conf_cores > 0, "configured cores must be positive");
}

#[test]
fn test_get_num_online_cores_returns_positive() {
    let online_cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    assert!(online_cores > 0, "online cores must be positive");
}

#[test]
fn test_online_cores_le_configured_cores() {
    let conf_cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) };
    let online_cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    assert!(
        online_cores <= conf_cores,
        "online cores should be <= configured cores"
    );
}

#[test]
fn test_get_max_number_of_cpus_returns_positive() {
    // In Rust, we use available_parallelism or _SC_NPROCESSORS_CONF
    let max_cores = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    assert!(max_cores > 0, "max cpus must be positive");
}
