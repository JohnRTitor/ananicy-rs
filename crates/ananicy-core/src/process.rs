use crate::types::Pid;

/// ProcessIdentity wraps a Pid.
/// (pidfd was removed as it caused FD leaks via unbounded channel queues,
/// and PID recycling is already prevented via start_time checks).
#[derive(Debug)]
pub struct ProcessIdentity {
    pub pid: Pid,
}

impl ProcessIdentity {
    pub fn new(pid: Pid) -> Self {
        Self { pid }
    }
}

/// Represents a process as processed by the ananicy rules engine
#[derive(Debug)]
pub struct Process {
    pub identity: ProcessIdentity,
    pub name: String,
    // delta_us is optional and only used for BPF
    pub delta_us: Option<u64>,

    /// Indicates if `name` is fully resolved and authoritative.
    /// If false, the worker may need to read procfs (e.g., `/proc/<pid>/comm` or `/proc/<pid>/cmdline`)
    /// to get the full name, which is necessary for BPF events that only provide a 16-byte truncated name.
    /// If true, the worker skips this redundant I/O, optimizing performance for `netlink` and `procfs` events.
    pub name_is_authoritative: bool,
}

impl Process {
    pub fn new(pid: Pid, name: String) -> Self {
        Self {
            identity: ProcessIdentity::new(pid),
            name,
            delta_us: None,
            name_is_authoritative: false,
        }
    }

    /// Marks the process name as authoritative (fully resolved by the event source).
    /// Calling this prevents the core worker from doing redundant procfs I/O to resolve the name again.
    pub fn with_authoritative_name(mut self) -> Self {
        self.name_is_authoritative = true;
        self
    }
}

/// Represents an event from the process monitor (Netlink or BPF)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    Created { pid: Pid, parent_pid: Pid },
    Exec { pid: Pid },
    Comm { pid: Pid },
    Exit { pid: Pid },
}
