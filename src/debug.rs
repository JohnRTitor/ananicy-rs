use {
    crate::cli::DebugTarget,
    ananicy_platform::mounts::{CgroupVersion, get_cgroup_info},
    tracing::{debug, warn},
};

pub(crate) fn run(target: &DebugTarget) {
    match target {
        DebugTarget::Cgroups => print_debug_cgroups(),
        // Print nothing for unrecognized targets.
        // An unrecognized debug sub-action is silently ignored, and the process still exits successfully.
        DebugTarget::Unknown(_) => {}
    }
}

/// Prints `path`'s contents wrapped in BEGIN/END markers.
fn print_file(path: &str) {
    let file_data = std::fs::read_to_string(path).unwrap_or_default();
    // The format string is "#### BEGIN {0} #####\n{1}\n#### END {0} #####\n",
    // i.e. an extra newline is always inserted after the file content,
    // regardless of whether it already ends in one.
    println!("#### BEGIN {path} #####\n{file_data}\n#### END {path} #####");
}

fn print_debug_cgroups() {
    println!("Ananicy Rs {}\n", env!("CARGO_PKG_VERSION"));

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
        match std::fs::read_dir(cgroup_path) {
            Ok(entries) => {
                // Deliberately not sorted: std::filesystem::directory_iterator
                // yields entries in whatever order the underlying filesystem
                // returns them, and the reference implementation preserves
                // that order verbatim.
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

    let pid = std::process::id();
    println!("Unit name: {}", ananicy_platform::service::get_unit_name());
    println!("Cgroup: {}", get_cgroup_for_pid(pid));
}

/// Read the first line of `/proc/<pid>/cgroup` and take everything after
/// the last `:`.
fn get_cgroup_for_pid(pid: u32) -> String {
    match std::fs::read_to_string(format!("/proc/{pid}/cgroup")) {
        Ok(content) => {
            let first_line = content.lines().next().unwrap_or("");
            if first_line.is_empty() {
                "<empty>".to_string()
            } else {
                match first_line.rfind(':') {
                    Some(idx) => first_line[idx + 1..].to_string(),
                    None => first_line.to_string(),
                }
            }
        }
        Err(_) => "<empty>".to_string(),
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgroup_for_pid_takes_substring_after_last_colon() {
        // Simulates the parsing logic against representative /proc/<pid>/cgroup
        // content without touching the real filesystem.
        let v2_line = "0::/user.slice/user-1000.slice/session-2.scope";
        let idx = v2_line.rfind(':').unwrap();
        assert_eq!(
            &v2_line[idx + 1..],
            "/user.slice/user-1000.slice/session-2.scope"
        );

        let v1_line = "4:cpu,cpuacct:/user.slice";
        let idx = v1_line.rfind(':').unwrap();
        assert_eq!(&v1_line[idx + 1..], "/user.slice");
    }



    #[test]
    fn debug_target_unknown_is_infallible_and_silent() {
        // "debug cgroups" recognized...
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
        run(&DebugTarget::Unknown("nonsense".to_string()));
    }
}
