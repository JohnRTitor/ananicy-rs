use {crate::cli::DumpTarget, ananicy_core::rules::Rules, ananicy_platform::procfs::ProcfsScanner};

pub(crate) fn run(target: &DumpTarget, rules: &Rules) {
    match target {
        DumpTarget::Rules => println!("Loaded Rules: {:?}", rules.get_rules()),
        DumpTarget::Types => println!("Loaded Types: {:?}", rules.get_types()),
        DumpTarget::Cgroups => println!("Loaded Cgroups: {:?}", rules.get_cgroups()),
        DumpTarget::Proc => dump_processes(rules),
        DumpTarget::Autogroup => dump_autogroup(),
    }
}

fn dump_processes(rules: &Rules) {
    let (tx_dump, rx_dump) = std::sync::mpsc::channel();
    ananicy_core::spawn_named_thread!("ananicy-dump", move || {
        ProcfsScanner::full_scan(tx_dump);
    });

    println!("{:<10} {:<10} {:<20} {:<20}", "PID", "TID", "NAME", "RULES");
    while let Ok(p) = rx_dump.recv() {
        let pid = p.identity.pid.0;
        let rule = rules.get_rule(&p.name);
        let rule_name = rule
            .as_ref()
            .and_then(|r| r.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if let Ok(entries) = std::fs::read_dir(format!("/proc/{}/task", pid)) {
            for entry in entries.flatten() {
                if let Ok(file_name) = entry.file_name().into_string()
                    && let Ok(tid) = file_name.parse::<i32>()
                {
                    println!("{:<10} {:<10} {:<20} {}", pid, tid, p.name, rule_name);
                }
            }
        } else {
            println!("{:<10} {:<10} {:<20} {}", pid, pid, p.name, rule_name);
        }
    }
}

fn dump_autogroup() {
    if let Ok(content) = std::fs::read_to_string("/proc/sys/kernel/sched_autogroup_enabled") {
        println!("Autogroup enabled: {}", content.trim());
    } else {
        println!(
            "Autogroup status unknown (failed to read /proc/sys/kernel/sched_autogroup_enabled)"
        );
    }
}
