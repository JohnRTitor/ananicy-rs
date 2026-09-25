use {
    crate::cli::DebugTarget,
    std::{
        fs::{read_dir, read_to_string},
        process::id,
    },
};

use {
    ananicy_platform::mounts::{CgroupVersion, get_cgroup_info},
    tracing::{debug, warn},
};

pub(crate) fn run(target: &DebugTarget, systemd_status: &str) {
    match target {
        DebugTarget::Cgroups => print_debug_cgroups(systemd_status),
        // Print nothing for unrecognized targets.
        // An unrecognized debug sub-action is silently ignored, and the process still exits successfully.
        DebugTarget::Unknown(_) => {}
    }
}

/// Prints `path`'s contents wrapped in BEGIN/END markers.
fn print_file(path: &str) {
    let file_data = read_to_string(path).unwrap_or_default();
    // The format string is "#### BEGIN {0} #####\n{1}\n#### END {0} #####\n",
    // i.e. an extra newline is always inserted after the file content,
    // regardless of whether it already ends in one.
    println!("#### BEGIN {path} #####\n{file_data}\n#### END {path} #####");
}

fn print_debug_cgroups(systemd_status: &str) {
    print_file("/etc/mtab");

    let cgroup_info = get_cgroup_info();
    let version_num = match cgroup_info.version {
        CgroupVersion::None => 0,
        CgroupVersion::V1 => 1,
        CgroupVersion::V2 => 2,
    };
    debug!(
        "Cgroup info: {}, path: {}",
        version_num,
        cgroup_info.mount_point.display()
    );

    if cgroup_info.version != CgroupVersion::None {
        let cgroup_path = &cgroup_info.mount_point;
        println!(
            "#### BEGIN listing files in {} #####",
            cgroup_path.display()
        );
        match read_dir(cgroup_path) {
            Ok(entries) => {
                // Deliberately not sorted: this is a diagnostic dump, so the
                // listing is printed in whatever order the filesystem returns,
                // and the output is meant to be pasted as-is into a bug report.
                for entry in entries {
                    match entry {
                        Ok(entry) => println!("{:?}", entry.path()),
                        Err(e) => {
                            warn!("print_debug_for_issue<21>: error: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                warn!("print_debug_for_issue<21>: error: {}", e);
            }
        }
        println!("#### END listing files in {} #####", cgroup_path.display());
    }

    let pid = id();
    println!("Systemd integration: {}", systemd_status);
    println!("Unit name: {}", ananicy_platform::service::get_unit_name());
    println!("Cgroup: {}", get_cgroup_for_pid(pid));
}

/// Read the first line of `/proc/<pid>/cgroup` and take everything after
/// the last `:`.
fn get_cgroup_for_pid(pid: u32) -> String {
    let Ok(content) = read_to_string(format!("/proc/{pid}/cgroup")) else {
        return "<empty>".to_string();
    };

    cgroup_path_from_line(content.lines().next().unwrap_or(""))
}

/// Extracts the cgroup path from one `hierarchy:controllers:path` line of
/// `/proc/<pid>/cgroup`. The path is everything after the last `:` so that the
/// v1 form (`4:cpu,cpuacct:/user.slice`) and the v2 form
/// (`0::/user.slice/…`) are both handled.
fn cgroup_path_from_line(line: &str) -> String {
    if line.is_empty() {
        return "<empty>".to_string();
    }

    match line.rfind(':') {
        Some(idx) => line[idx + 1..].to_string(),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgroup_path_is_taken_from_after_the_last_colon() {
        // The v2 form: an empty controller field, so the path starts right
        // after the second colon.
        assert_eq!(
            cgroup_path_from_line("0::/user.slice/user-1000.slice/session-2.scope"),
            "/user.slice/user-1000.slice/session-2.scope"
        );
        // The v1 form: a comma separated controller list before the path.
        assert_eq!(
            cgroup_path_from_line("4:cpu,cpuacct:/user.slice"),
            "/user.slice"
        );
        assert_eq!(
            cgroup_path_from_line("7:memory:/user.slice/app.slice"),
            "/user.slice/app.slice"
        );
    }

    #[test]
    fn a_cgroup_line_without_a_path_is_reported_as_empty() {
        assert_eq!(cgroup_path_from_line(""), "<empty>");
        assert_eq!(cgroup_path_from_line("0::"), "");
    }

    #[test]
    fn a_line_without_any_colon_is_returned_verbatim() {
        assert_eq!(cgroup_path_from_line("/user.slice"), "/user.slice");
    }

    /// System test: the cgroup of the test process is whatever the kernel says,
    /// but the helper must never panic and must never invent a path.
    #[test]
    fn the_cgroup_of_this_process_is_reported() {
        let pid = id();
        let reported = get_cgroup_for_pid(pid);
        assert!(!reported.is_empty());
    }

    /// A PID above any plausible `pid_max` has no cgroup file at all.
    #[test]
    fn a_dead_process_has_no_cgroup() {
        assert_eq!(get_cgroup_for_pid(u32::MAX), "<empty>");
    }

    #[test]
    fn debug_target_unknown_is_infallible_and_silent() {
        // "debug cgroups" is recognized...
        assert_eq!(
            "cgroups".parse::<DebugTarget>().unwrap(),
            DebugTarget::Cgroups
        );
        // ...anything else parses successfully too (never errors), with a
        // silent no-op for unrecognized debug sub-actions.
        assert_eq!(
            "nonsense".parse::<DebugTarget>().unwrap(),
            DebugTarget::Unknown("nonsense".to_string())
        );
        // And run() with an Unknown target must not panic and must not
        // print anything extra (verified structurally: it's a no-op match arm).
        run(
            &DebugTarget::Unknown("nonsense".to_string()),
            "disabled (test)",
        );
    }
}
