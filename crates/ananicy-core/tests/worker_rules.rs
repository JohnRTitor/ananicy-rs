//! What the worker does with a matched rule.
//!
//! `worker_logging.rs` covers how the outcome is reported; this file covers the
//! mapping itself: which platform operations a rule's attributes trigger, and
//! which `apply_*` configuration flags suppress them. The platform is a
//! recorder, so the assertions are about the intended daemon behaviour and no
//! process is ever touched.

mod common;

use {
    ananicy_core::{config::ConfigSnapshot, worker::PlatformError},
    common::{Call, FakePlatform, run_worker, run_worker_with, snapshot},
    std::collections::HashMap,
};

/// The default configuration: every `apply_*` flag is on, so a rule attribute is
/// only suppressed when the test turns its flag off explicitly. Logging is off,
/// which `worker_logging.rs` covers.
fn all_attributes_enabled() -> ConfigSnapshot {
    ConfigSnapshot::default()
}

fn aliases(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(alias, cpuset)| (alias.to_string(), cpuset.to_string()))
        .collect()
}

#[test]
fn every_attribute_of_a_rule_is_applied() {
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":5,"latency_nice":-3,"sched":"idle",
            "ioclass":"best-effort","ionice":3,"oom_score_adj":100,"cgroup":"lowlatency",
            "cpuset":"0-3"}"#,
        FakePlatform::new().with_max_cores(8),
    );

    let calls = run.platform.calls();
    assert!(calls.contains(&Call::SetPriority { nice: 5 }));
    assert!(calls.contains(&Call::SetLatencyNice { value: -3 }));
    assert!(calls.contains(&Call::SetSched {
        sched: "idle".to_string(),
        rtprio: 1
    }));
    assert!(calls.contains(&Call::SetIoPriority {
        ioclass: "best-effort".to_string(),
        ionice: 3
    }));
    assert!(calls.contains(&Call::SetOomScoreAdj { value: 100 }));
    assert!(calls.contains(&Call::AddPidToCgroup {
        cgroup: "lowlatency".to_string()
    }));
    assert!(calls.contains(&Call::SetAffinity {
        cpuset: "0-3".to_string()
    }));
}

#[test]
fn a_missing_rtprio_defaults_to_one() {
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","sched":"fifo"}"#,
        FakePlatform::new(),
    );

    assert!(run.platform.calls().contains(&Call::SetSched {
        sched: "fifo".to_string(),
        rtprio: 1
    }));
}

#[test]
fn a_missing_ionice_defaults_to_zero() {
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","ioclass":"idle"}"#,
        FakePlatform::new(),
    );

    assert!(run.platform.calls().contains(&Call::SetIoPriority {
        ioclass: "idle".to_string(),
        ionice: 0
    }));
}

#[test]
fn latency_nice_falls_back_to_nice() {
    // A rule that only sets `nice` still moves the process's latency class,
    // which is the behaviour the `latency_nice` fallback exists for.
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":-4}"#,
        FakePlatform::new(),
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetLatencyNice { value: -4 })
    );
}

#[test]
fn an_explicit_latency_nice_wins_over_nice() {
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":-4,"latency_nice":5}"#,
        FakePlatform::new(),
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetLatencyNice { value: 5 })
    );
    assert!(
        !run.platform
            .calls()
            .contains(&Call::SetLatencyNice { value: -4 })
    );
}

#[test]
fn each_apply_flag_suppresses_only_its_own_attribute() {
    let cases: [(ConfigSnapshot, &str, &str); 7] = [
        (
            ConfigSnapshot {
                apply_nice: false,
                ..all_attributes_enabled()
            },
            "set_priority",
            "set_latency_nice",
        ),
        (
            ConfigSnapshot {
                apply_latnice: false,
                ..all_attributes_enabled()
            },
            "set_latency_nice",
            "set_priority",
        ),
        (
            ConfigSnapshot {
                apply_sched: false,
                ..all_attributes_enabled()
            },
            "set_sched",
            "set_priority",
        ),
        (
            ConfigSnapshot {
                apply_ionice: false,
                ..all_attributes_enabled()
            },
            "set_io_priority",
            "set_priority",
        ),
        (
            ConfigSnapshot {
                apply_oom_score_adj: false,
                ..all_attributes_enabled()
            },
            "set_oom_score_adj",
            "set_priority",
        ),
        (
            ConfigSnapshot {
                apply_cgroups: false,
                ..all_attributes_enabled()
            },
            "add_pid_to_cgroup",
            "set_priority",
        ),
        (
            ConfigSnapshot {
                apply_cpuset: false,
                ..all_attributes_enabled()
            },
            "set_affinity",
            "set_priority",
        ),
    ];

    let rule = r#"{"name":"worker-test","nice":1,"sched":"idle","ioclass":"idle",
        "oom_score_adj":5,"cgroup":"low","cpuset":"0-1"}"#;

    for (config, suppressed, still_applied) in cases {
        let run = run_worker(config, rule, FakePlatform::new().with_max_cores(8));
        let calls = run.platform.calls();

        assert!(
            run.platform.calls_of(suppressed).is_empty(),
            "{suppressed} must not be called when it is disabled, got {calls:?}"
        );
        assert!(
            !run.platform.calls_of(still_applied).is_empty(),
            "{still_applied} must still be applied, got {calls:?}"
        );
    }
}

#[test]
fn a_cpuset_alias_is_resolved_before_the_affinity_call() {
    let run = run_worker_with(
        all_attributes_enabled(),
        r#"{"name":"worker-test","cpuset":"big-cores"}"#,
        FakePlatform::new().with_max_cores(8),
        aliases(&[("big-cores", "4-7")]),
        ananicy_core::types::Pid(1),
        "worker-test",
    );

    assert!(run.platform.calls().contains(&Call::SetAffinity {
        cpuset: "4-7".to_string()
    }));
}

#[test]
fn an_alias_resolving_to_no_cpus_skips_the_affinity_call() {
    // On a machine with no little cores the alias is empty; the daemon must not
    // build an empty mask, which would mean "do not touch affinity" anyway.
    let run = run_worker_with(
        all_attributes_enabled(),
        r#"{"name":"worker-test","cpuset":"little-cores"}"#,
        FakePlatform::new().with_max_cores(8),
        aliases(&[("little-cores", "")]),
        ananicy_core::types::Pid(1),
        "worker-test",
    );

    assert!(
        !run.platform
            .calls()
            .iter()
            .any(|call| matches!(call, Call::SetAffinity { .. }))
    );
}

#[test]
fn an_unparseable_cpuset_is_reported_as_a_partial_failure() {
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","cpuset":"not-a-cpuset"}"#,
        FakePlatform::new().with_max_cores(8),
    );

    assert!(
        !run.platform
            .calls()
            .iter()
            .any(|call| matches!(call, Call::SetAffinity { .. }))
    );
    assert!(
        run.events
            .contains(tracing::Level::WARN, "partially failed")
    );
}

#[test]
fn a_cpuset_naming_a_cpu_outside_the_machine_is_rejected() {
    // The mask is built with `get_max_cores` CPUs, so a rule mentioning CPU 63
    // on an 8-CPU machine cannot be honoured.
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","cpuset":"60-63"}"#,
        FakePlatform::new().with_max_cores(8),
    );

    assert!(
        !run.platform
            .calls()
            .iter()
            .any(|call| matches!(call, Call::SetAffinity { .. }))
    );
}

#[test]
fn nice_is_mirrored_into_a_cgroup_v2_cpu_weight() {
    // On cgroup v2 the kernel ignores `nice` for bandwidth control, so the
    // daemon mirrors it into `cpu.weight` on top of setting the nice value.
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":0}"#,
        FakePlatform::cgroup_v2(),
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetCpuWeight { weight: 100 })
    );
}

#[test]
fn a_failing_cpu_weight_does_not_prevent_the_nice_value() {
    let run = run_worker(
        snapshot(true),
        r#"{"name":"worker-test","nice":0}"#,
        FakePlatform::cgroup_v2().failing("set_cpu_weight", PlatformError::Unsupported),
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetPriority { nice: 0 })
    );
    assert!(run.events.contains(tracing::Level::INFO, "worker-test(42)"));
}

#[test]
fn no_cpu_weight_is_mirrored_on_a_cgroup_v1_host() {
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":0}"#,
        FakePlatform::new(),
    );

    assert!(
        !run.platform
            .calls()
            .iter()
            .any(|call| matches!(call, Call::SetCpuWeight { .. }))
    );
}

#[test]
fn a_realtime_process_is_not_moved_into_a_rule_cgroup() {
    // The cgroup v2 kernel limitation: a realtime task's cgroup must not be
    // changed, so the rule's cgroup is reported as unusable instead. The
    // separate workaround still moves the process to the hierarchy root.
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":1,"cgroup":"lowlatency"}"#,
        FakePlatform::realtime_on_cgroup_v2(),
    );

    assert!(
        !run.platform
            .calls()
            .iter()
            .any(|call| matches!(call, Call::AddPidToCgroup { cgroup } if cgroup == "lowlatency")),
        "the rule's cgroup must not be applied to a realtime process"
    );
    assert!(run.platform.calls().contains(&Call::AddPidToCgroup {
        cgroup: "/".to_string()
    }));
    assert!(
        run.events
            .contains(tracing::Level::WARN, "partially failed")
    );
}

#[test]
fn a_realtime_process_on_cgroup_v1_still_gets_its_cgroup() {
    // The limitation is specific to the unified hierarchy.
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":1,"cgroup":"lowlatency"}"#,
        FakePlatform::realtime_on_cgroup_v1(),
    );

    assert!(run.platform.calls().contains(&Call::AddPidToCgroup {
        cgroup: "lowlatency".to_string()
    }));
}

#[test]
fn the_realtime_workaround_targets_the_hierarchy_root() {
    // A realtime process on cgroup v2 is moved to "/" — not to "", which in cgroup
    // v2 would resolve to our own delegated subtree and hijack the process.
    let run = run_worker(
        snapshot(false),
        r#"{"name":"worker-test","nice":1}"#,
        FakePlatform::realtime_on_cgroup_v2(),
    );

    assert!(run.platform.calls().contains(&Call::AddPidToCgroup {
        cgroup: "/".to_string()
    }));
}

#[test]
fn a_non_realtime_process_is_left_in_its_cgroup() {
    let run = run_worker(
        snapshot(false),
        r#"{"name":"worker-test","nice":1}"#,
        FakePlatform::cgroup_v2(),
    );

    assert!(
        !run.platform
            .calls()
            .iter()
            .any(|call| matches!(call, Call::AddPidToCgroup { .. }))
    );
}

#[test]
fn the_realtime_workaround_can_be_switched_off() {
    let mut config = snapshot(false);
    config.cgroup_realtime_workaround = false;

    let run = run_worker(
        config,
        r#"{"name":"worker-test","nice":1}"#,
        FakePlatform::realtime_on_cgroup_v2(),
    );

    assert!(
        !run.platform
            .calls()
            .iter()
            .any(|call| matches!(call, Call::AddPidToCgroup { .. }))
    );
}

#[test]
fn a_process_without_a_rule_is_never_touched() {
    let config = all_attributes_enabled();
    let run = run_worker(
        config,
        r#"{"name":"somebody-else","nice":1}"#,
        FakePlatform::new(),
    );

    assert!(
        run.platform.calls().is_empty(),
        "a process without a matching rule must not be modified: {:?}",
        run.platform.calls()
    );
}

#[test]
fn an_attribute_of_the_wrong_type_is_ignored() {
    let run = run_worker(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":"5","sched":7,"cpuset":["0"]}"#,
        FakePlatform::new().with_max_cores(8),
    );

    assert!(
        run.platform.calls().is_empty(),
        "an attribute of the wrong type must be skipped, got {:?}",
        run.platform.calls()
    );
}

#[test]
fn the_swapper_process_is_ignored() {
    // PID 0 is the idle thread; applying rules to it would be meaningless and
    // would flood the log.
    let run = run_worker_with(
        all_attributes_enabled(),
        r#"{"name":"worker-test","nice":1}"#,
        FakePlatform::new(),
        HashMap::new(),
        ananicy_core::types::Pid(0),
        "worker-test",
    );

    assert!(run.platform.calls().is_empty());
}

#[test]
fn a_name_regex_rule_is_matched_by_the_worker() {
    let run = run_worker_with(
        all_attributes_enabled(),
        r#"{"name":"java","name_regex":"^java[0-9.]*$","nice":3}"#,
        FakePlatform::new(),
        HashMap::new(),
        ananicy_core::types::Pid(10),
        "java17",
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetPriority { nice: 3 })
    );
}

#[test]
fn a_nix_wrapped_executable_is_unwrapped_before_matching() {
    // NixOS wraps executables as `.foo-wrapped`, and the rule is written for
    // `foo`.
    let run = run_worker_with(
        all_attributes_enabled(),
        r#"{"name":"foo","nice":7}"#,
        FakePlatform::new(),
        HashMap::new(),
        ananicy_core::types::Pid(7),
        ".foo-wrapped",
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetPriority { nice: 7 })
    );
}

#[test]
fn a_truncated_nix_wrapped_executable_is_unwrapped_before_matching() {
    // procfs truncates `comm` to 15 characters, so `.abcdefghij-wrapped`
    // arrives as `.abcdefghij-wra` — cut off inside the `-wrapped` suffix.
    let run = run_worker_with(
        all_attributes_enabled(),
        r#"{"name":"abcdefghij","nice":9}"#,
        FakePlatform::new(),
        HashMap::new(),
        ananicy_core::types::Pid(8),
        ".abcdefghij-wra",
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetPriority { nice: 9 })
    );
}

#[test]
fn a_dotted_name_that_is_not_a_wrapper_is_matched_as_is() {
    let run = run_worker_with(
        all_attributes_enabled(),
        r#"{"name":".hidden","nice":4}"#,
        FakePlatform::new(),
        HashMap::new(),
        ananicy_core::types::Pid(9),
        ".hidden",
    );

    assert!(
        run.platform
            .calls()
            .contains(&Call::SetPriority { nice: 4 })
    );
}
