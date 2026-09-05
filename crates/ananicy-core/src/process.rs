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
}

impl Process {
    pub fn new(pid: Pid, name: String) -> Self {
        Self {
            identity: ProcessIdentity::new(pid),
            name,
            delta_us: None,
        }
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
