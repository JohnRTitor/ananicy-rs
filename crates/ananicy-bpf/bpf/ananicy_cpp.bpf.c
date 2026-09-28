// SPDX-License-Identifier: GPL-2.0
// Copyright (c) 2019 Facebook
#include <vmlinux.h>
#include <bpf/bpf_helpers.h>       /* most used helpers: SEC, __always_inline, etc */
#include <bpf/bpf_core_read.h>     /* for BPF CO-RE helpers */
#include <bpf/bpf_tracing.h>       /* for getting kprobe arguments */
#include "core_fixes.bpf.h"

#define TASK_COMM_LEN 16
#define INVALID_UID ((uid_t)-1)

struct event {
    pid_t pid;
    pid_t prev_pid;
    __u64 delta_us;
    char task[TASK_COMM_LEN];
};

const volatile __u64 min_us = 0;
const volatile pid_t targ_pid = 0;
const volatile pid_t targ_tgid = 0;
const volatile uid_t targ_uid = INVALID_UID;

/* No `start` map.
 *
 * The reference declares one — a 10,240-entry hash of pid -> timestamp — and
 * never reads it back. BPF_MAP_TYPE_HASH allocates its element pool up front, so
 * an unused map of that size is still a few hundred kilobytes of kernel memory
 * charged to this daemon's cgroup for the life of the process, and unlike the
 * rule set it is memory reclaim cannot give back: it raises the floor under
 * everything else. Every event gets a timestamp from `bpf_ktime_get_ns()` and
 * carries it in the event itself, which is why nothing ever needed the map.
 */

struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, int);
    __type(value, struct event);
} heap SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_PERF_EVENT_ARRAY);
    __uint(key_size, sizeof(u32));
    __uint(value_size, sizeof(u32));
} events SEC(".maps");

static __always_inline bool valid_uid(uid_t uid) {
    return uid != INVALID_UID;
}

static __always_inline struct event* handle_event(pid_t pid, pid_t* prev_pid, u64* prev_ts) {
    struct event *e;
    int zero = 0;
    e = bpf_map_lookup_elem(&heap, &zero);
    if (!e) /* can't happen */
        return NULL;

    u64 ts = bpf_ktime_get_ns();

    // Compute delta_us
    u64 delta_us = (ts - *prev_ts) / 1000;
    //if (min_us && delta_us <= min_us)
    //    return 0;

    // Assign variables to event struct
    e->pid = pid;
    e->prev_pid = *prev_pid;
    e->delta_us = delta_us;
    bpf_get_current_comm(&e->task, sizeof(e->task));

    // Update previous process's timestamp
    *prev_pid = e->pid;
    *prev_ts = ts;

    return e;
}

SEC("tp/sched/sched_process_exec")
int handle_exec(struct trace_event_raw_sched_process_exec* ctx)
{
    static pid_t prev_pid;
    static u64 prev_ts;
    struct event *e;

    u32 pid = bpf_get_current_pid_tgid();
    e = handle_event(pid, &prev_pid, &prev_ts);
    if (!e) /* can't happen */
        return 0;

    /* output */
    bpf_perf_event_output(ctx, &events, BPF_F_CURRENT_CPU,
                  e, sizeof(*e));
    return 0;
}

SEC("tp/sched/sched_process_fork")
int handle_fork(struct trace_event_raw_sched_process_fork* ctx)
{
    static pid_t prev_pid;
    static u64 prev_ts;
    struct event *e;

    /* The child, not the forking task.
     *
     * `bpf_get_current_pid_tgid()` in this tracepoint is the process doing the
     * fork, so naming the event after it attributes every fork to the parent:
     * the daemon then re-applies the *parent's* rule to the parent, once per
     * fork, for the life of the daemon. A shell that runs a command a minute
     * has its rule re-applied sixty times, and each re-application repeats the
     * cgroup and cpu.weight writes too. `child_pid` is the pid the event is
     * about, and it is the field the tracepoint carries for exactly this.
     *
     * `task` below is still the parent's `comm`, because the tracepoint does
     * not carry the child's `task_struct` and so its name cannot be read here.
     * That is harmless: the daemon treats a BPF event's name as unresolved and
     * reads the name back out of procfs for the pid the event names, which is
     * the child.
     */
    u32 pid = ctx->child_pid;
    e = handle_event(pid, &prev_pid, &prev_ts);
    if (!e) /* can't happen */
        return 0;

    /* output */
    bpf_perf_event_output(ctx, &events, BPF_F_CURRENT_CPU,
                  e, sizeof(*e));
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
