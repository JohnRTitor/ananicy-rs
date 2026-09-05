use {ananicy_core::cpuset::CpuSet, ananicy_platform::abi::affinity::set_affinity, std::process};

#[test]
fn test_set_affinity_on_current_process() {
    let mut cs = CpuSet::new(1024);
    // Setting all CPUs to avoid restricting the test environment too much,
    // though realistically we just want to ensure it succeeds.
    // For a safe test, we get the current affinity and just set it back.
    // However, rustix provides sched_getaffinity which we can use to populate the CpuSet.
    if let Ok(mask) = rustix::thread::sched_getaffinity(Some(
        rustix::process::Pid::from_raw(process::id() as i32).unwrap(),
    )) {
        for i in 0..1024 {
            if mask.is_set(i) {
                cs.set_cpu(i as u32);
            }
        }
    } else {
        cs.set_cpu(0); // Fallback
    }

    let result = set_affinity(process::id() as i32, &cs);
    assert!(
        result.is_ok(),
        "set_affinity on current process should succeed"
    );
}

#[test]
fn test_set_affinity_on_nonexistent_process() {
    let mut cs = CpuSet::new(1024);
    cs.set_cpu(0);

    // PID 999999999 almost certainly doesn't exist
    let result = set_affinity(999999999, &cs);
    assert!(
        result.is_err(),
        "set_affinity on nonexistent process should fail"
    );
}

#[test]
fn test_set_affinity_with_zero_ncpus_cpuset() {
    let cs = CpuSet::new(0); // zero max_cores
    // Should gracefully fail or be a no-op without crashing
    let _ = set_affinity(process::id() as i32, &cs);
}
