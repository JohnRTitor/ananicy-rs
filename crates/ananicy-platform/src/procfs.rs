#![allow(clippy::collapsible_if)]
use {ananicy_core::process::Process, std::fs};

use std::sync::atomic::{AtomicU32, Ordering};

static EXE_FAIL_COUNT: AtomicU32 = AtomicU32::new(0);
const COMMAND_NAME_HEURISTIC_SKIP_EXE_FAILURES: u32 = 5;

/// Tries to determine the effective process name exactly as C++ ananicy did:
/// 1. `/proc/<pid>/cmdline` (argv[0] basename)
/// 2. `/proc/<pid>/exe` (readlink basename, trimming ` (deleted)`)
/// 3. `/proc/<pid>/comm` (fallback)
pub fn get_command_from_pid(pid: i32) -> String {
    let proc_dir = format!("/proc/{}", pid);

    // 1. Try cmdline
    if let Ok(cmdline_bytes) = fs::read(format!("{}/cmdline", proc_dir)) {
        if !cmdline_bytes.is_empty() {
            // Find the first non-empty argument (C++ parity: find_first_not_of('\0'))
            let argv0_bytes = cmdline_bytes
                .split(|&b| b == 0)
                .find(|arg| !arg.is_empty())
                .unwrap_or(&[]);
            if !argv0_bytes.is_empty() {
                let argv0_str = String::from_utf8_lossy(argv0_bytes);
                let mut name = argv0_str.to_string();

                // If the name ends with .exe, it might be a Wine/Proton game with backslashes
                if name.ends_with(".exe") {
                    name = name.replace('\\', "/");
                }

                // Get the basename
                if let Some(slash_idx) = name.rfind('/') {
                    return name[slash_idx + 1..].to_string();
                } else {
                    return name;
                }
            }
        }
    }

    // 2. Try exe (if we haven't failed too many times)
    if EXE_FAIL_COUNT.load(Ordering::Relaxed) < COMMAND_NAME_HEURISTIC_SKIP_EXE_FAILURES {
        match fs::read_link(format!("{}/exe", proc_dir)) {
            Ok(exe_target) => {
                EXE_FAIL_COUNT.store(0, Ordering::Relaxed);
                if let Some(file_name) = exe_target.file_name() {
                    let mut name = file_name.to_string_lossy().to_string();
                    if let Some(deleted_idx) = name.find(" (deleted)") {
                        name.truncate(deleted_idx);
                    }
                    return name;
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    EXE_FAIL_COUNT.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    // 3. Try comm
    if let Ok(comm) = fs::read_to_string(format!("{}/comm", proc_dir)) {
        let comm_trimmed = comm.trim().to_string();
        if !comm_trimmed.is_empty() {
            return comm_trimmed;
        }
    }

    "<unknown>".to_string()
}

pub struct ProcfsScanner;

impl ProcfsScanner {
    pub fn full_scan(tx: std::sync::mpsc::Sender<Process>) {
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let pid_str = file_name.to_string_lossy();

                // If it's a numeric directory, it's a PID
                if pid_str.chars().all(|c| c.is_ascii_digit())
                    && let Ok(pid) = pid_str.parse::<i32>()
                {
                    let name = get_command_from_pid(pid);
                    if tx
                        .send(Process::new(ananicy_core::types::Pid(pid), name))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    }
}

pub fn get_start_time(pid: i32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // start time is the 22nd field (0-indexed 21)
    let parts: Vec<&str> = stat.split_whitespace().collect();
    if parts.len() > 21 {
        parts[21].parse::<u64>().ok()
    } else {
        None
    }
}

pub fn get_tgid(pid: i32) -> Option<i32> {
    let status = fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    for line in status.lines() {
        if line.starts_with("Tgid:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2 {
                return parts[1].parse::<i32>().ok();
            }
        }
    }
    None
}
