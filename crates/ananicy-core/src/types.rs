use std::{borrow::Borrow, fmt, hash::Hash};

/// Represents a process ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pid(pub i32);

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Represents a thread ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tid(pub i32);

impl fmt::Display for Tid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A strong type for rule names (e.g. program names)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleName(pub String);

impl AsRef<str> for RuleName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Lets a rule be looked up by the process name it is being matched against,
/// without building a `RuleName` to do it.
///
/// Every rule the worker resolves arrives as a `&str` — the basename of a
/// process' `argv[0]` — and the map is keyed by `RuleName`. Without this, each of
/// those lookups allocates a `String` and then throws it away, which on a
/// machine that forks a lot is a per-process allocation on the one path that is
/// supposed to be cheap. `Borrow` is what tells `HashMap` the two are the same
/// key, and the `Hash`/`Eq` impls come along with the type so they agree.
impl Borrow<str> for RuleName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A strong type for rule types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeName(pub String);

impl AsRef<str> for TypeName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A strong type for cgroup names
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CgroupName(pub String);

impl AsRef<str> for CgroupName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
