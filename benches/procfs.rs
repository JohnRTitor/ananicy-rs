//! What resolving a process name costs.
//!
//! `get_command_from_pid` runs on the hottest path in the daemon: once per
//! process-creation event on the netlink listener, once per process in a full
//! `/proc` scan, and once more in the worker when an event carried a
//! non-authoritative name. It reads `/proc/<pid>/cmdline` and, when a second
//! argument is present, `/proc/<pid>/comm` — so the syscall cost belongs to the
//! kernel and is not what this measures.
//!
//! What it measures is the part this crate owns: building the `/proc/<pid>`
//! path, and the string handling around the two files. That is a real cost and
//! it is per-event, which is why it is worth a bench rather than an assertion.
//!
//! The subject is this process. `argv[0]` for `cargo bench` is `cargo`, and
//! `cargo bench --bench procfs -- <name>` re-execs under a different name if
//! one is given, so the same binary can be measured under the shapes that
//! matter: a short name, a name with a `.exe` suffix (the Wine rewrite), and a
//! long one. What is being compared across runs is the crate's own overhead,
//! not which program is being measured.

use {
    ananicy_core::worker::PlatformActions,
    ananicy_platform::{
        LinuxPlatform,
        procfs::{get_command_from_pid, get_start_time, get_tgid, get_tids},
    },
    criterion::{Criterion, black_box, criterion_group, criterion_main},
};

fn bench_command_from_pid(c: &mut Criterion) {
    let pid = std::process::id() as i32;

    c.bench_function("get_command_from_pid", |b| {
        b.iter(|| get_command_from_pid(black_box(pid)))
    });
}

/// The same call for a pid that does not exist.
///
/// This is the failure path, and it is not a rare one: by the time a
/// process-creation event is read, the process it names has sometimes already
/// exited. It is also the path with the most `format!` calls in it — the
/// `/proc/<pid>` prefix is built once and then extended for `cmdline`, `exe`
/// and `comm` before every one of them fails — so it is the clearest
/// measurement of what the path building costs on its own.
fn bench_command_from_dead_pid(c: &mut Criterion) {
    // A pid that cannot be alive: `i32::MAX` is above every `pid_max` any
    // kernel permits, and `/proc/<pid>` for it cannot exist.
    let dead = i32::MAX;

    c.bench_function("get_command_from_dead_pid", |b| {
        b.iter(|| get_command_from_pid(black_box(dead)))
    });
}

/// The two other procfs reads on the per-process path, measured because they
/// were the two nobody had measured.
///
/// `get_start_time` reads `/proc/<pid>/stat` and is the validity check behind
/// the cgroup resolver's cache, the pid-reuse guard behind `move_pid`, and
/// the attribution check behind the `/proc/<pid>/exe` failure budget — so a
/// single process with a rule can pay for it several times over, and each
/// payment is a file read and a parse.
///
/// `get_tids` is a `readdir` of `/proc/<pid>/task` and runs once for every
/// process that matched a rule, before a single attribute is applied. It is
/// the same shape of cost as `get_command_from_pid` and was never in anyone's
/// profile.
fn bench_other_procfs_reads(c: &mut Criterion) {
    let pid = std::process::id() as i32;

    c.bench_function("get_start_time", |b| {
        b.iter(|| get_start_time(black_box(pid)))
    });

    c.bench_function("get_tids", |b| b.iter(|| get_tids(black_box(pid)).ok()));
}

/// The cgroup-resolution path, which is the one that decides whether a
/// process' `nice` is mirrored into `cpu.weight`.
///
/// `apply_cpu_weight` is on by default and fires for any rule carrying a
/// `nice`, so this is on the path of nearly every matched process — and it
/// reads `/proc/<pid>/stat` to validate the cache before consulting it. The
/// cache exists to avoid a `/proc/<pid>/cgroup` read, so the interesting
/// question is which of the two costs more.
fn bench_cgroup_resolution(c: &mut Criterion) {
    let pid = std::process::id() as i32;
    let platform = LinuxPlatform::new();

    c.bench_function("get_tgid", |b| b.iter(|| get_tgid(black_box(pid))));

    c.bench_function("process_cgroup", |b| {
        b.iter(|| platform.process_cgroup(black_box(pid)))
    });
}

criterion_group!(
    benches,
    bench_command_from_pid,
    bench_command_from_dead_pid,
    bench_other_procfs_reads,
    bench_cgroup_resolution
);
criterion_main!(benches);
