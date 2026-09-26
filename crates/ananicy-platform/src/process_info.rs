use std::fs::{read_link, read_to_string};

use {
    serde::{Deserialize, Serialize},
    serde_json::Value,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: i32,
    pub tpid: i32,
    pub exe: Option<String>,
    pub comm: String,
    pub cmd: String,
    pub stat: String,
    pub stat_name: String,
    pub autogroup: Option<Value>,
    pub sched: String,
    pub rtprio: i32,
    pub nice: i32,
    pub latency_nice: i32,
    pub ionice: Value,
    pub oom_score_adj: i32,
    pub cmdline: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}

impl ProcessInfo {
    pub fn new(pid: i32, tpid: i32, rule: Option<String>) -> Self {
        let exe = read_link(format!("/proc/{}/exe", pid))
            .map(|p| p.to_string_lossy().into_owned())
            .ok();
        let cmd = read_to_string(format!("/proc/{}/comm", pid))
            .unwrap_or_default()
            .trim()
            .to_string();
        // The arguments, as the NUL-separated `/proc/<pid>/cmdline` splits
        // them. The reference emits this as a JSON array
        // (`process_info.cpp:243`), and joining them into one string loses the
        // boundaries — an argument containing a space becomes
        // indistinguishable from two arguments.
        let cmdline = read_to_string(format!("/proc/{}/cmdline", pid))
            .unwrap_or_default()
            .split('\0')
            .map(str::trim)
            .filter(|arg| !arg.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let oom_score_adj = read_to_string(format!("/proc/{}/oom_score_adj", pid))
            .unwrap_or_default()
            .trim()
            .parse::<i32>()
            .unwrap_or(0);

        Self::from_parts(pid, tpid, exe, cmd, cmdline, oom_score_adj, rule)
    }

    pub fn from_parts(
        pid: i32,
        tpid: i32,
        exe: Option<String>,
        cmd: String,
        cmdline: Vec<String>,
        oom_score_adj: i32,
        rule: Option<String>,
    ) -> Self {
        let comm = read_to_string(format!("/proc/{}/task/{}/comm", pid, tpid))
            .unwrap_or_default()
            .trim()
            .to_string();

        let stat = read_to_string(format!("/proc/{}/task/{}/stat", pid, tpid))
            .unwrap_or_default()
            .trim()
            .to_string();
        let stat_name = parse_stat(&stat).unwrap_or_default();

        let autogroup =
            get_autogroup_from_str(&read_to_string(autogroup_path(pid)).unwrap_or_default());

        let size = std::mem::size_of::<crate::abi::sched_attr::sched_attr>() as u32;
        let mut attr = crate::abi::sched_attr::sched_attr {
            size,
            ..Default::default()
        };

        let (sched, rtprio, nice, latency_nice) =
            if crate::abi::sched_attr::sched_getattr(tpid, &mut attr, size, 0).is_ok() {
                (
                    get_sched_policy_name(attr.sched_policy).to_string(),
                    attr.sched_priority as i32,
                    attr.sched_nice,
                    attr.sched_latency_nice,
                )
            } else {
                ("unknown".to_string(), 0, 0, 0)
            };

        use crate::abi::ioprio::*;
        let io_prio_data = ioprio_get(IOPRIO_WHO_PROCESS, tpid).unwrap_or(0);
        let io_class = io_prio_data >> IOPRIO_CLASS_SHIFT;
        let io_nice = io_prio_data & IOPRIO_PRIO_MASK;

        let io_class_name = get_io_class_name(io_class);
        let ionice = if io_class == IOPRIO_CLASS_BE {
            serde_json::json!([io_class_name, io_nice])
        } else {
            serde_json::json!([io_class_name, Value::Null])
        };

        Self {
            pid,
            tpid,
            exe,
            comm,
            cmd,
            stat,
            stat_name,
            autogroup,
            sched,
            rtprio,
            nice,
            latency_nice,
            ionice,
            oom_score_adj,
            cmdline,
            rule,
        }
    }
}

fn parse_stat(stat: &str) -> Option<String> {
    let start = stat.find('(')?;
    let end = stat.rfind(')')?;
    Some(stat[start + 1..end].to_string())
}

/// Where the kernel publishes a process's autogroup.
///
/// The autogroup is a property of the *thread group*, not of a thread, and the
/// kernel publishes it in exactly one place: `/proc/<pid>/autogroup`, which is
/// the thread-group leader's entry. There is no `/proc/<pid>/task/<tid>/autogroup`
/// — unlike `comm` and `stat`, which are genuinely per-thread and are read
/// through the `task/<tid>` path above.
fn autogroup_path(pid: i32) -> String {
    format!("/proc/{pid}/autogroup")
}

fn get_autogroup_from_str(s: &str) -> Option<Value> {
    let s = s.trim();
    if !s.starts_with("/autogroup-") {
        return None;
    }
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 3 && parts[1] == "nice" {
        let group_str = &parts[0]["/autogroup-".len()..];
        let nice_str = parts[2];
        if let (Ok(group), Ok(nice)) = (group_str.parse::<i32>(), nice_str.parse::<i32>()) {
            return Some(serde_json::json!({
                "group": group,
                "nice": nice
            }));
        }
    }
    None
}

fn get_sched_policy_name(policy: u32) -> &'static str {
    use crate::abi::sched_attr::*;
    match policy {
        SCHED_NORMAL => "normal",
        SCHED_FIFO => "fifo",
        SCHED_RR => "rr",
        SCHED_BATCH => "batch",
        SCHED_ISO => "iso",
        SCHED_IDLE => "idle",
        SCHED_DEADLINE => "deadline",
        _ => "unknown",
    }
}

fn get_io_class_name(class: i32) -> &'static str {
    use crate::abi::ioprio::*;
    match class {
        IOPRIO_CLASS_NONE => "none",
        IOPRIO_CLASS_RT => "realtime",
        IOPRIO_CLASS_BE => "best-effort",
        IOPRIO_CLASS_IDLE => "idle",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic `/proc/<pid>/stat` line. The `comm` field is wrapped in
    /// parentheses and may itself contain spaces and parentheses, so everything
    /// after the *last* `)` is a field.
    fn stat_line(comm: &str, rest: &[&str]) -> String {
        format!(
            "42 ({comm}) S 1 42 42 0 -1 4194560 {} rest\n",
            rest.join(" ")
        )
    }

    #[test]
    fn parse_stat_extracts_the_comm_field() {
        assert_eq!(parse_stat("42 (bash) S 1 2 3").as_deref(), Some("bash"));
    }

    #[test]
    fn parse_stat_handles_a_comm_with_spaces_and_parentheses() {
        // A process called `Web Content (Renderer)` must not truncate the line.
        assert_eq!(
            parse_stat("42 (Web Content (Renderer)) S 1 2 3").as_deref(),
            Some("Web Content (Renderer)")
        );
        assert_eq!(parse_stat("42 ((a b)) S").as_deref(), Some("(a b)"));
    }

    #[test]
    fn parse_stat_rejects_a_line_without_parentheses() {
        assert_eq!(parse_stat("42 bash S 1 2 3"), None);
        assert_eq!(parse_stat(""), None);
        assert_eq!(parse_stat("42 (bash"), None);
    }

    #[test]
    fn parse_stat_is_never_affected_by_trailing_content() {
        let long = stat_line("bash", &["0"; 30]);
        assert_eq!(parse_stat(&long).as_deref(), Some("bash"));
    }

    #[test]
    fn autogroup_lines_are_parsed_into_group_and_nice() {
        // The shape written by the kernel to /proc/<pid>/autogroup.
        let parsed = get_autogroup_from_str("/autogroup-3 nice 7").unwrap();
        assert_eq!(parsed["group"], 3);
        assert_eq!(parsed["nice"], 7);
    }

    #[test]
    fn autogroup_is_read_from_a_path_the_kernel_actually_provides() {
        use std::path::Path;

        let pid = std::process::id() as i32;
        let leader = autogroup_path(pid);
        let per_thread = format!("/proc/{pid}/task/{pid}/autogroup");

        if !Path::new(&leader).exists() {
            // A kernel built without CONFIG_SCHED_AUTOGROUP, or a sandbox that
            // does not mount it. There is nothing to read and nothing to assert.
            return;
        }

        assert!(
            !Path::new(&per_thread).exists(),
            "the kernel now publishes a per-thread autogroup at {per_thread:?}; \
             it would be a different file from {leader:?} and the one to read \
             would need revisiting"
        );

        let info = ProcessInfo::new(pid, pid, None);
        assert!(
            info.autogroup.is_some(),
            "the kernel published {leader:?} but the daemon reported no autogroup \
             for its own process: {:?}",
            info.autogroup
        );
    }

    #[test]
    fn autogroup_lines_tolerate_surrounding_whitespace() {
        assert_eq!(
            get_autogroup_from_str("  /autogroup-12 nice -3 \n"),
            get_autogroup_from_str("/autogroup-12 nice -3")
        );
    }

    #[test]
    fn unrelated_or_incomplete_autogroup_content_is_ignored() {
        assert_eq!(get_autogroup_from_str(""), None);
        assert_eq!(get_autogroup_from_str("/"), None);
        assert_eq!(get_autogroup_from_str("not-an-autogroup"), None);
        // Missing the `nice` value.
        assert_eq!(get_autogroup_from_str("/autogroup-3:1"), None);
        // Non-numeric group or nice value.
        assert_eq!(get_autogroup_from_str("/autogroup-x:1 nice 7"), None);
        assert_eq!(get_autogroup_from_str("/autogroup-3:1 nice seven"), None);
    }

    #[test]
    fn scheduler_policy_names_cover_the_known_policies() {
        use crate::abi::sched_attr::*;
        assert_eq!(get_sched_policy_name(SCHED_NORMAL), "normal");
        assert_eq!(get_sched_policy_name(SCHED_FIFO), "fifo");
        assert_eq!(get_sched_policy_name(SCHED_RR), "rr");
        assert_eq!(get_sched_policy_name(SCHED_BATCH), "batch");
        assert_eq!(get_sched_policy_name(SCHED_ISO), "iso");
        assert_eq!(get_sched_policy_name(SCHED_IDLE), "idle");
        assert_eq!(get_sched_policy_name(SCHED_DEADLINE), "deadline");
        assert_eq!(get_sched_policy_name(0xdead_beef), "unknown");
    }

    #[test]
    fn io_class_names_cover_the_known_classes() {
        use crate::abi::ioprio::*;
        assert_eq!(get_io_class_name(IOPRIO_CLASS_NONE), "none");
        assert_eq!(get_io_class_name(IOPRIO_CLASS_RT), "realtime");
        assert_eq!(get_io_class_name(IOPRIO_CLASS_BE), "best-effort");
        assert_eq!(get_io_class_name(IOPRIO_CLASS_IDLE), "idle");
        assert_eq!(get_io_class_name(99), "unknown");
    }

    /// System test: the reported info for the test process itself must be
    /// self-consistent.
    #[test]
    fn process_info_of_the_test_process_is_readable() {
        let pid = std::process::id() as i32;
        let info = ProcessInfo::new(pid, pid, Some("a-rule".to_string()));

        assert_eq!(info.pid, pid);
        assert_eq!(info.tpid, pid);
        assert_eq!(info.rule.as_deref(), Some("a-rule"));
        assert!(
            !info.comm.is_empty(),
            "comm is always readable for a live process"
        );
        assert_eq!(
            info.stat_name, info.comm,
            "the name parsed out of stat must match /proc/<pid>/comm"
        );
        assert!(
            (-20..=19).contains(&info.nice),
            "nice is out of range: {}",
            info.nice
        );
        assert_ne!(
            info.sched, "unknown",
            "the scheduling policy of a live process is readable"
        );
        assert!(
            !info.cmdline.is_empty() || info.exe.is_some(),
            "at least one source for the command line must be readable"
        );
    }
}
