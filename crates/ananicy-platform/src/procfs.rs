use {
    ananicy_core::{process::Process, types::Pid},
    std::{io::ErrorKind::PermissionDenied, sync::mpsc::Sender},
};

use {
    lru::LruCache,
    std::{
        fs,
        num::NonZeroUsize,
        sync::{Mutex, OnceLock},
    },
};

static EXE_FAIL_CACHE: OnceLock<Mutex<LruCache<i32, u8>>> = OnceLock::new();
const COMMAND_NAME_HEURISTIC_SKIP_EXE_FAILURES: u8 = 5;
const MAX_EXE_FAIL_CACHE_SIZE: usize = 256;

fn get_exe_fail_cache() -> &'static Mutex<LruCache<i32, u8>> {
    EXE_FAIL_CACHE.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(MAX_EXE_FAIL_CACHE_SIZE).unwrap(),
        ))
    })
}

/// Strips the kernel's ` (deleted)` marker from an `exe` readlink target.
///
/// The kernel appends a space and `(deleted)` to `/proc/<pid>/exe` when the
/// binary has been unlinked — which an ordinary package upgrade does, since the
/// new file is created underneath the running process. The marker includes its
/// leading space, so the name is truncated *at* the index of that space.
///
/// `ananicy-cpp` truncates one byte later, keeping the space, so `/usr/bin/foo
/// (deleted)` resolves to `"foo "` there and to `"foo"` here — and a rule written
/// as `{"name": "foo"}` therefore matches here and not there. This daemon's
/// answer is the correct one; see `docs/ANANICY_CPP_DIFFERENCES.md` §5.1.
fn strip_deleted_suffix(name: &str) -> String {
    match name.find(" (deleted)") {
        Some(index) => name[..index].to_string(),
        None => name.to_string(),
    }
}

/// Tries to determine the effective process name exactly as C++ ananicy did:
/// 1. `/proc/<pid>/cmdline` (argv[0] basename)
/// 2. `/proc/<pid>/exe` (readlink basename, trimming ` (deleted)`)
/// 3. `/proc/<pid>/comm` (fallback)
pub fn get_command_from_pid(pid: i32) -> String {
    let proc_dir = format!("/proc/{}", pid);

    // 1. Try cmdline
    if let Ok(cmdline_bytes) = fs::read(format!("{}/cmdline", proc_dir))
        && !cmdline_bytes.is_empty()
    {
        // Find the first non-empty argument. A process that rewrote its argv[0]
        // to "" still has a name in `exe` or `comm`, so an empty first argument
        // must not win over them.
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
            }
            return name;
        }
    }

    // Repeated EACCES usually means `/proc/<pid>/exe` is not readable to us; stop retrying
    // it to avoid repeated procfs I/O on every event, tracked per-PID via LRU.
    // 2. Try exe (if we haven't failed too many times)
    let exe_failures = if let Ok(mut cache) = get_exe_fail_cache().lock()
        && let Some(&fails) = cache.get(&pid)
    {
        fails
    } else {
        0
    };

    if exe_failures < COMMAND_NAME_HEURISTIC_SKIP_EXE_FAILURES {
        match fs::read_link(format!("{}/exe", proc_dir)) {
            Ok(exe_target) => {
                // Success, clear any stored failure count
                if exe_failures > 0
                    && let Ok(mut cache) = get_exe_fail_cache().lock()
                {
                    cache.pop(&pid);
                }
                if let Some(file_name) = exe_target.file_name() {
                    return strip_deleted_suffix(&file_name.to_string_lossy());
                }
            }
            Err(e) => {
                if e.kind() == PermissionDenied
                    && let Ok(mut cache) = get_exe_fail_cache().lock()
                {
                    cache.put(pid, exe_failures + 1);
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
    pub fn full_scan(tx: Sender<Process>) {
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
                        .send(Process::new(Pid(pid), name).with_authoritative_name())
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
    // The comm field (2nd field) is enclosed in parentheses and can contain spaces.
    // We must find the last ')' to skip over it to prevent field shifting.
    let rparen = stat.rfind(')')?;

    // The fields after the comm field start with a space, then the state.
    // Field 3 (state) is index 0 in the new split array.
    // start time is field 22. 22 - 3 = 19. So it's index 19.
    let fields_after_comm: Vec<&str> = stat[rparen + 1..].split_whitespace().collect();
    if fields_after_comm.len() > 19 {
        fields_after_comm[19].parse::<u64>().ok()
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

pub fn get_tids(pid: i32) -> Result<Vec<i32>, ananicy_core::worker::PlatformError> {
    let task_path = format!("/proc/{}/task", pid);
    let mut tids = Vec::new();

    match fs::read_dir(&task_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(file_name) = entry.file_name().into_string()
                    && let Ok(tid) = file_name.parse::<i32>()
                {
                    tids.push(tid);
                }
            }
            Ok(tids)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ananicy_core::worker::PlatformError::NotFound)
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(ananicy_core::worker::PlatformError::PermissionDenied)
        }
        Err(e) => Err(ananicy_core::worker::PlatformError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's ` (deleted)` marker includes its leading space, so the name
    /// is truncated at the index of that space.
    ///
    /// `ananicy-cpp` truncates one byte later and keeps the space, so
    /// `/usr/bin/foo (deleted)` resolves to `"foo "` there. A rule written as
    /// `{"name": "foo"}` then matches here and not there, for any process whose
    /// binary was replaced — an ordinary package upgrade does exactly that. This
    /// daemon's answer is the correct one, and the reference's is the bug, so
    /// this test exists to stop the space coming back: it is a one-character
    /// change away, and nothing else in the suite would notice.
    #[test]
    fn the_deleted_marker_is_stripped_without_its_leading_space() {
        assert_eq!(strip_deleted_suffix("foo (deleted)"), "foo");
        assert_eq!(
            strip_deleted_suffix(" (deleted)"),
            "",
            "a binary whose whole name is the marker leaves nothing, not a space"
        );
    }

    #[test]
    fn a_name_without_the_marker_is_untouched() {
        for name in ["foo", "foo.bar", "libsystemd.so.1", "deleted", "(deleted)"] {
            assert_eq!(
                strip_deleted_suffix(name),
                name,
                "{name:?} should pass through unchanged"
            );
        }
    }

    /// The marker is located by its leading space, so a name that merely contains
    /// the word — without the space in front of it — is left alone.
    #[test]
    fn the_space_is_what_makes_it_a_marker() {
        assert_eq!(strip_deleted_suffix("foo(deleted)"), "foo(deleted)");
        assert_eq!(strip_deleted_suffix("foo x(deleted)"), "foo x(deleted)");
        assert_eq!(strip_deleted_suffix("foo (deleted)"), "foo");
    }
}
