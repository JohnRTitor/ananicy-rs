use std::fmt;

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
