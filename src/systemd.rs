//! Resolution of the "systemd integration" mode.
//!
//! The mode decides whether the daemon talks to the service manager
//! (`sd_notify` readiness/stop notifications) and whether it logs through
//! the journald native protocol instead of stderr. It does *not* influence
//! cgroup handling: delegation is discovered from the process' own cgroup
//! independently (see `ananicy_platform::cgroup::ownership`).
//!
//! The question the automatic mode answers is therefore narrow: *is this
//! process supervised by a systemd service manager as a service?* It is
//! deliberately not "is systemd running on this host" — see
//! `docs/SYSTEMD_AUTO_DETECTION_RESEARCH.md`.

/// What the user asked for on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemdRequest {
    /// Neither `--systemd` nor `--no-systemd` was given: detect the mode.
    Auto,
    /// `--systemd` was given: enable systemd integration unconditionally.
    Enabled,
    /// `--no-systemd` was given: keep systemd integration off.
    Disabled,
}

/// Environment evidence that a service manager supervises this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemdEvidence {
    /// `$INVOCATION_ID` is set: the manager assigned this process to a unit
    /// runtime cycle (systemd >= 232, all unit types and all `Type=` values).
    InvocationId,
    /// `$NOTIFY_SOCKET` is set: the manager handed this process a status
    /// notification socket (systemd >= 229, `NotifyAccess=` != none).
    NotifySocket,
    /// `$JOURNAL_STREAM` is set: stdout/stderr of this process are wired into
    /// journald (systemd >= 231).
    JournalStream,
}

/// Why systemd integration stayed off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemdSkipReason {
    /// `--no-systemd` was given.
    Requested,
    /// This process is a member of a transient scope (interactive session),
    /// not of a service.
    TransientScope,
    /// No service manager supervision was found in the environment.
    NotDetected,
}

/// Resolved systemd integration mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemdMode {
    /// `--systemd` was given, so the mode is on regardless of the environment.
    Forced,
    /// The mode is on because the process is supervised by a service manager.
    Enabled(SystemdEvidence),
    /// The mode is off.
    Disabled(SystemdSkipReason),
}

impl SystemdMode {
    pub(crate) const fn is_enabled(self) -> bool {
        match self {
            Self::Forced | Self::Enabled(_) => true,
            Self::Disabled(_) => false,
        }
    }

    /// One-line description of the decision, for logs and diagnostics.
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Forced => "enabled (forced by --systemd)",
            Self::Enabled(SystemdEvidence::InvocationId) => {
                "enabled (auto-detected from $INVOCATION_ID)"
            }
            Self::Enabled(SystemdEvidence::NotifySocket) => {
                "enabled (auto-detected from $NOTIFY_SOCKET)"
            }
            Self::Enabled(SystemdEvidence::JournalStream) => {
                "enabled (auto-detected from $JOURNAL_STREAM)"
            }
            Self::Disabled(SystemdSkipReason::Requested) => "disabled (forced by --no-systemd)",
            Self::Disabled(SystemdSkipReason::TransientScope) => {
                "disabled (member of a transient .scope, not a service)"
            }
            Self::Disabled(SystemdSkipReason::NotDetected) => {
                "disabled (no systemd service manager detected)"
            }
        }
    }
}

/// The environment facts consulted by [`detect`].
///
/// Deliberately limited to variables that a systemd service manager sets for
/// the processes it supervises. Host-level facts (PID 1 being systemd,
/// `/run/systemd/system` existing, parent process names) are intentionally
/// excluded: they say nothing about whether *this* process is supervised.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SystemdEnvironment {
    /// `$INVOCATION_ID` is set and non-empty.
    pub(crate) invocation_id: bool,
    /// `$NOTIFY_SOCKET` is set and non-empty.
    pub(crate) notify_socket: bool,
    /// `$JOURNAL_STREAM` is set and non-empty.
    pub(crate) journal_stream: bool,
    /// The process sits in a transient scope cgroup, i.e. it is a member of an
    /// interactive session (or of a scope created by someone else) rather than
    /// the service that systemd was asked to start.
    pub(crate) in_transient_scope: bool,
}

impl SystemdEnvironment {
    /// Reads the detection inputs from the current process environment.
    ///
    /// `var_os` keeps this allocation-free and tolerates non-UTF-8 values; an
    /// empty value counts as unset, matching `sd_notify()` and
    /// `sd_listen_fds()`, which ignore empty variables.
    pub(crate) fn from_process() -> Self {
        let env = Self {
            invocation_id: is_set("INVOCATION_ID"),
            notify_socket: is_set("NOTIFY_SOCKET"),
            journal_stream: is_set("JOURNAL_STREAM"),
            in_transient_scope: false,
        };
        Self {
            // The cgroup is only read when there is something to interpret, so
            // manual runs do not pay for a `/proc` read they cannot use.
            in_transient_scope: has_supervision_evidence(env)
                && read_own_cgroup_path().is_some_and(|path| path_is_transient_scope(&path)),
            ..env
        }
    }
}

/// Whether the environment shows that a service manager supervises this
/// process, regardless of whether that turns out to be a service.
fn has_supervision_evidence(env: SystemdEnvironment) -> bool {
    env.invocation_id || env.notify_socket || env.journal_stream
}

fn is_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Reads this process' cgroup path as the service manager sees it, or `None`
/// if the hierarchy cannot be identified.
fn read_own_cgroup_path() -> Option<String> {
    // `from_utf8_lossy` keeps the parse working for the (legal) case of a
    // delegated subtree with non-UTF-8 directory names.
    let raw = std::fs::read("/proc/self/cgroup").ok()?;
    own_cgroup_path(&String::from_utf8_lossy(&raw)).map(str::to_owned)
}

/// Picks the cgroup path that represents the process in the service manager's
/// hierarchy: the unified one if present, otherwise the manager's own
/// `name=systemd` controller, otherwise any controller reporting a non-root
/// path.
fn own_cgroup_path(content: &str) -> Option<&str> {
    let mut manager: Option<&str> = None;
    let mut any_controller: Option<&str> = None;

    for line in content.lines() {
        let mut fields = line.splitn(3, ':');
        let Some(hierarchy) = fields.next() else {
            continue;
        };
        let Some(controllers) = fields.next() else {
            continue;
        };
        let Some(path) = fields.next() else {
            continue;
        };
        if hierarchy.parse::<u32>().is_err() || path.is_empty() {
            continue;
        }
        if controllers.is_empty() {
            // The unified hierarchy is what the service manager itself uses.
            return Some(path);
        }
        if controllers.split(',').any(|name| name == "name=systemd") {
            manager = Some(path);
        } else if any_controller.is_none() {
            any_controller = Some(path);
        }
    }

    manager.or(any_controller)
}

/// Whether a cgroup path denotes a transient scope rather than a service.
///
/// Only the leaf is considered: a daemon that has been delegated a subtree
/// (`ananicy-rs.service/…`) is still a service, and systemd's `DelegateSubgroup=`
/// places the initial process one level below the unit cgroup.
fn path_is_transient_scope(cgroup_path: &str) -> bool {
    cgroup_path
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .is_some_and(|unit| unit.ends_with(".scope"))
}

/// Resolves the mode, giving explicit command line requests precedence over
/// automatic detection.
pub(crate) fn resolve(request: SystemdRequest, env: SystemdEnvironment) -> SystemdMode {
    match request {
        SystemdRequest::Enabled => SystemdMode::Forced,
        SystemdRequest::Disabled => SystemdMode::Disabled(SystemdSkipReason::Requested),
        SystemdRequest::Auto => match detect(env) {
            Detection::Service(evidence) => SystemdMode::Enabled(evidence),
            Detection::TransientScope => SystemdMode::Disabled(SystemdSkipReason::TransientScope),
            Detection::NoEvidence => SystemdMode::Disabled(SystemdSkipReason::NotDetected),
        },
    }
}

/// Outcome of automatic detection.
enum Detection {
    /// A service manager supervises this process as part of a service.
    Service(SystemdEvidence),
    /// A service manager set the variables, but this process is only a member
    /// of a transient scope.
    TransientScope,
    /// No sign of a service manager at all.
    NoEvidence,
}

/// Detects service manager supervision from the environment alone.
///
/// The order is by generality, not by strength: `$INVOCATION_ID` covers every
/// unit type and every `Type=`, so it is checked first. `$NOTIFY_SOCKET`
/// covers systemd 229-231 and processes that were handed a notification socket
/// out of band. `$JOURNAL_STREAM` is the weakest hint and only matters when
/// neither of the others is present.
///
/// Environment variables are inherited, so a process started *from* a systemd
/// unit carries the same evidence as the unit's main process. That is why
/// transient scopes are excluded: desktop terminals and login sessions are
/// systemd units too, and a manual run started from one must not switch to
/// journald logging.
fn detect(env: SystemdEnvironment) -> Detection {
    let evidence = if env.invocation_id {
        SystemdEvidence::InvocationId
    } else if env.notify_socket {
        SystemdEvidence::NotifySocket
    } else if env.journal_stream {
        SystemdEvidence::JournalStream
    } else {
        return Detection::NoEvidence;
    };

    if env.in_transient_scope {
        Detection::TransientScope
    } else {
        Detection::Service(evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTO: SystemdRequest = SystemdRequest::Auto;

    fn env(invocation_id: bool, notify_socket: bool, journal_stream: bool) -> SystemdEnvironment {
        SystemdEnvironment {
            invocation_id,
            notify_socket,
            journal_stream,
            in_transient_scope: false,
        }
    }

    fn enabled(evidence: SystemdEvidence) -> SystemdMode {
        SystemdMode::Enabled(evidence)
    }

    fn not_detected() -> SystemdMode {
        SystemdMode::Disabled(SystemdSkipReason::NotDetected)
    }

    fn in_scope(mut env: SystemdEnvironment) -> SystemdEnvironment {
        env.in_transient_scope = true;
        env
    }

    #[test]
    fn service_supervised_by_systemd_enables_mode() {
        // Scenario A/F: `systemd -> ananicy-rs` and `systemd -> sh -> ananicy-rs`.
        // Wrappers inherit the environment and stay in the service cgroup, so
        // both resolve identically.
        assert_eq!(
            resolve(AUTO, env(true, true, true)),
            enabled(SystemdEvidence::InvocationId)
        );
        assert_eq!(
            resolve(AUTO, env(true, false, false)),
            enabled(SystemdEvidence::InvocationId)
        );
    }

    #[test]
    fn user_service_enables_mode() {
        // Scenario C: user managers pass the same variables as the system manager.
        assert_eq!(
            resolve(AUTO, env(true, false, false)),
            enabled(SystemdEvidence::InvocationId)
        );
    }

    #[test]
    fn systemd_in_container_enables_mode() {
        // Scenario D: a container running its own systemd as PID 1 sets the
        // same variables; the identity of the manager is irrelevant here.
        assert_eq!(
            resolve(AUTO, env(true, false, false)),
            enabled(SystemdEvidence::InvocationId)
        );
    }

    #[test]
    fn manual_invocation_is_not_detected() {
        // Scenario B/E: nothing indicates supervision of this process, no
        // matter which init system the host uses.
        assert_eq!(resolve(AUTO, env(false, false, false)), not_detected());
    }

    #[test]
    fn container_without_systemd_is_not_detected() {
        // Scenario D: the host may run systemd, but nothing in this
        // environment says this process is supervised by it.
        assert_eq!(resolve(AUTO, SystemdEnvironment::default()), not_detected());
    }

    #[test]
    fn inherited_environment_inside_scope_is_not_a_service() {
        // A terminal started by the desktop inherits the scope's
        // $INVOCATION_ID, but it is a scope member, not a service.
        assert_eq!(
            resolve(AUTO, in_scope(env(true, true, true))),
            SystemdMode::Disabled(SystemdSkipReason::TransientScope)
        );
        assert_eq!(
            resolve(AUTO, in_scope(env(false, true, false))),
            SystemdMode::Disabled(SystemdSkipReason::TransientScope)
        );
    }

    #[test]
    fn scope_without_evidence_is_reported_as_not_detected() {
        // Nothing to interpret: the scope hint must not be reported when
        // there was no supervision evidence in the first place.
        assert_eq!(
            resolve(AUTO, in_scope(env(false, false, false))),
            not_detected()
        );
    }

    #[test]
    fn notify_socket_alone_enables_mode() {
        // `Type=notify`/watchdog units, and systemd 229-231 which predate
        // $INVOCATION_ID.
        assert_eq!(
            resolve(AUTO, env(false, true, false)),
            enabled(SystemdEvidence::NotifySocket)
        );
    }

    #[test]
    fn journal_stream_alone_enables_mode() {
        assert_eq!(
            resolve(AUTO, env(false, false, true)),
            enabled(SystemdEvidence::JournalStream)
        );
    }

    #[test]
    fn partial_environment_keeps_evidence_order() {
        assert_eq!(
            resolve(AUTO, env(true, false, true)),
            enabled(SystemdEvidence::InvocationId)
        );
        assert_eq!(
            resolve(AUTO, env(false, true, true)),
            enabled(SystemdEvidence::NotifySocket)
        );
    }

    #[test]
    fn explicit_enable_wins_over_environment() {
        // Backwards compatibility: `ananicy-rs --systemd` keeps enabling
        // systemd integration, also inside a scope and on hosts older than
        // systemd 232.
        assert_eq!(
            resolve(SystemdRequest::Enabled, SystemdEnvironment::default()),
            SystemdMode::Forced
        );
        assert_eq!(
            resolve(SystemdRequest::Enabled, in_scope(env(true, true, true))),
            SystemdMode::Forced
        );
    }

    #[test]
    fn explicit_disable_wins_over_environment() {
        assert_eq!(
            resolve(SystemdRequest::Disabled, env(true, true, true)),
            SystemdMode::Disabled(SystemdSkipReason::Requested)
        );
        assert_eq!(
            resolve(SystemdRequest::Disabled, in_scope(env(true, true, true))),
            SystemdMode::Disabled(SystemdSkipReason::Requested)
        );
    }

    #[test]
    fn mode_reports_its_state() {
        assert!(SystemdMode::Forced.is_enabled());
        assert!(enabled(SystemdEvidence::InvocationId).is_enabled());
        assert!(!SystemdMode::Disabled(SystemdSkipReason::NotDetected).is_enabled());
    }

    #[test]
    fn mode_descriptions_are_distinct_and_carry_the_cause() {
        let all = [
            SystemdMode::Forced,
            enabled(SystemdEvidence::InvocationId),
            enabled(SystemdEvidence::NotifySocket),
            enabled(SystemdEvidence::JournalStream),
            SystemdMode::Disabled(SystemdSkipReason::Requested),
            SystemdMode::Disabled(SystemdSkipReason::TransientScope),
            SystemdMode::Disabled(SystemdSkipReason::NotDetected),
        ];
        for (index, mode) in all.iter().enumerate() {
            for other in &all[index + 1..] {
                assert_ne!(mode.description(), other.description());
            }
        }
    }

    #[test]
    fn unified_hierarchy_path_is_preferred() {
        assert_eq!(
            own_cgroup_path("0::/system.slice/ananicy-rs.service\n"),
            Some("/system.slice/ananicy-rs.service")
        );
    }

    #[test]
    fn hybrid_hierarchy_prefers_the_unified_line() {
        let content = "1:name=systemd:/user.slice/user-0.slice/app.slice/x.scope\n\
                       0::/user.slice/user-0.slice/app.slice/x.scope\n";
        assert_eq!(
            own_cgroup_path(content),
            Some("/user.slice/user-0.slice/app.slice/x.scope")
        );
    }

    #[test]
    fn legacy_hierarchy_uses_the_manager_controller() {
        let content = "11:memory:/user.slice\n\
                       10:cpu,cpuacct:/\n\
                       1:name=systemd:/system.slice/ananicy-rs.service\n";
        assert_eq!(
            own_cgroup_path(content),
            Some("/system.slice/ananicy-rs.service")
        );
    }

    #[test]
    fn legacy_hierarchy_falls_back_to_any_controller() {
        // No `name=systemd` line (e.g. a foreign hierarchy): any controller
        // reporting a non-root path is better than nothing.
        let content = "11:memory:/some/unit.scope\n10:cpu,cpuacct:/\n";
        assert_eq!(own_cgroup_path(content), Some("/some/unit.scope"));
    }

    #[test]
    fn unreadable_cgroup_content_yields_no_path() {
        assert_eq!(own_cgroup_path(""), None);
        assert_eq!(own_cgroup_path("garbage\n"), None);
        assert_eq!(own_cgroup_path("0::\n"), None);
        // A process in the root cgroup has a real, but unscoped, path.
        assert_eq!(own_cgroup_path("0::/\n"), Some("/"));
    }

    #[test]
    fn transient_scopes_are_recognized_by_the_leaf() {
        assert!(path_is_transient_scope(
            "/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-133224-0.scope"
        ));
        assert!(path_is_transient_scope(
            "/user.slice/user-0.slice/session-3.scope"
        ));
        assert!(path_is_transient_scope("app-foo-123.scope"));
    }

    #[test]
    fn services_and_delegated_subtrees_are_not_scopes() {
        assert!(!path_is_transient_scope("/system.slice/ananicy-rs.service"));
        assert!(!path_is_transient_scope(
            "/user.slice/user-0.slice/user@0.service/app.slice/ananicy-rs.service"
        ));
        assert!(!path_is_transient_scope("/ananicy-rs.service"));
        // Delegated subtrees and `DelegateSubgroup=` place the process below
        // the unit cgroup; those are still services.
        assert!(!path_is_transient_scope(
            "/system.slice/ananicy-rs.service/ananicy"
        ));
        assert!(!path_is_transient_scope("/"));
        assert!(!path_is_transient_scope(""));
    }

    #[test]
    fn from_process_reads_real_environment() {
        // Not asserting a specific mode (that would depend on the developer's
        // init system); only that reading is consistent and that an absent
        // variable counts as unset.
        let observed = SystemdEnvironment::from_process();
        assert_eq!(
            observed.invocation_id,
            is_set("INVOCATION_ID"),
            "$INVOCATION_ID presence must be reported consistently"
        );
        assert_eq!(observed.notify_socket, is_set("NOTIFY_SOCKET"));
        assert_eq!(observed.journal_stream, is_set("JOURNAL_STREAM"));
        assert!(!is_set("ANANICY_RS_DEFINITELY_UNSET_VARIABLE"));
    }
}
