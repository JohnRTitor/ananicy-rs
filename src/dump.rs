use {crate::cli::DumpTarget, ananicy_core::rules::Rules, ananicy_platform::procfs::ProcfsScanner};

pub(crate) fn run(target: &DumpTarget, rules: &Rules) {
    match target {
        DumpTarget::Rules => {
            let sorted: std::collections::BTreeMap<_, _> = rules.get_rules().iter().map(|(k, v)| (k.as_ref(), v)).collect();
            println!("{}", serde_json::to_string_pretty(&sorted).unwrap_or_default());
        }
        DumpTarget::Types => {
            let sorted: std::collections::BTreeMap<_, _> = rules.get_types().iter().map(|(k, v)| (k.as_ref(), v)).collect();
            println!("{}", serde_json::to_string_pretty(&sorted).unwrap_or_default());
        }
        DumpTarget::Cgroups => {
            let sorted: std::collections::BTreeMap<_, _> = rules.get_cgroups().iter().map(|(k, v)| (k.as_ref(), v)).collect();
            println!("{}", serde_json::to_string_pretty(&sorted).unwrap_or_default());
        }
        DumpTarget::Proc => dump_processes(rules),
        DumpTarget::Autogroup => dump_autogroup(rules),
    }
}



fn get_process_info_map(rules: &Rules) -> serde_json::Map<String, serde_json::Value> {
    let (tx_dump, rx_dump) = std::sync::mpsc::channel();
    ananicy_core::spawn_named_thread!("ananicy-dump", move || {
        ProcfsScanner::full_scan(tx_dump);
    });

    let mut process_map = serde_json::Map::new();

    while let Ok(p) = rx_dump.recv() {
        let pid = p.identity.pid.0;
        let rule = rules.get_rule(&p.name);
        let rule_name = rule
            .as_ref()
            .and_then(|r| r.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

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

        if let Ok(entries) = std::fs::read_dir(format!("/proc/{}/task", pid)) {
            for entry in entries.flatten() {
                if let Ok(file_name) = entry.file_name().into_string() {
                    if let Ok(tid) = file_name.parse::<i32>() {
                        let rule_opt = if rule_name.is_empty() { None } else { Some(rule_name.clone()) };
                        let info = ananicy_platform::process_info::ProcessInfo::from_parts(
                            pid,
                            tid,
                            exe.clone(),
                            cmd.clone(),
                            cmdline.clone(),
                            oom_score_adj,
                            rule_opt,
                        );

                        if let Ok(val) = serde_json::to_value(info) {
                            process_map.insert(tid.to_string(), val);
                        }
                    }
                }
            }
        }
    }

    process_map
}

fn dump_processes(rules: &Rules) {
    let process_map = get_process_info_map(rules);
    println!("{}", serde_json::to_string_pretty(&process_map).unwrap_or_default());
}

fn dump_autogroup(rules: &Rules) {
    let mut autogroup_map = serde_json::Map::new();
    let process_info_map = get_process_info_map(rules);

    for (tpid, mut process_info) in process_info_map {
        let process_info_obj = if let serde_json::Value::Object(obj) = &mut process_info {
            obj
        } else {
            continue;
        };

        if let Some(autogroup) = process_info_obj.get("autogroup") {
            if !autogroup.is_null() {
                let autogroup_obj = if let serde_json::Value::Object(obj) = autogroup {
                    obj
                } else {
                    continue;
                };

                let group_num = autogroup_obj.get("group").and_then(|v| v.as_i64()).unwrap_or(0).to_string();
                let nice = autogroup_obj.get("nice").cloned().unwrap_or(serde_json::Value::Null);

                if !autogroup_map.contains_key(&group_num) {
                    let mut entry = serde_json::Map::new();
                    entry.insert("nice".into(), nice);
                    entry.insert("proc".into(), serde_json::Value::Object(serde_json::Map::new()));
                    autogroup_map.insert(group_num.clone(), serde_json::Value::Object(entry));
                }

                process_info_obj.remove("autogroup");

                if let Some(serde_json::Value::Object(entry)) = autogroup_map.get_mut(&group_num) {
                    if let Some(serde_json::Value::Object(proc_map)) = entry.get_mut("proc") {
                        proc_map.insert(tpid, process_info);
                    }
                }
            }
        }
    }

    println!("{}", serde_json::to_string_pretty(&autogroup_map).unwrap_or_default());
}
