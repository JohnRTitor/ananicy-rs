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
    pub cmdline: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}

impl ProcessInfo {
    pub fn new(pid: i32, tpid: i32, rule: Option<String>) -> Self {
        let exe = std::fs::read_link(format!("/proc/{}/exe", pid))
            .map(|p| p.to_string_lossy().into_owned())
            .ok();
        let cmd = std::fs::read_to_string(format!("/proc/{}/comm", pid))
            .unwrap_or_default()
            .trim()
            .to_string();
        let cmdline = std::fs::read_to_string(format!("/proc/{}/cmdline", pid))
            .unwrap_or_default()
            .replace('\0', " ")
            .trim()
            .to_string();
        let oom_score_adj = std::fs::read_to_string(format!("/proc/{}/oom_score_adj", pid))
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
        cmdline: String,
        oom_score_adj: i32,
        rule: Option<String>,
    ) -> Self {
        let comm = std::fs::read_to_string(format!("/proc/{}/task/{}/comm", pid, tpid))
            .unwrap_or_default()
            .trim()
            .to_string();

        let stat = std::fs::read_to_string(format!("/proc/{}/task/{}/stat", pid, tpid))
            .unwrap_or_default()
            .trim()
            .to_string();
        let stat_name = parse_stat(&stat).unwrap_or_default();

        let autogroup_val =
            std::fs::read_to_string(format!("/proc/{}/task/{}/autogroup", pid, tpid))
                .unwrap_or_default();
        let autogroup = get_autogroup_from_str(&autogroup_val);

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
