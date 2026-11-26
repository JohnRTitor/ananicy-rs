use std::{error::Error, fmt, io, path::PathBuf};

pub mod manager;
pub mod ownership;
pub mod process;

pub use ananicy_core::cgroup::CgroupPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupVersion {
    None,
    V1,
    V2,
}

#[derive(Debug, Clone)]
pub struct CgroupInfo {
    pub version: CgroupVersion,
    pub mount_point: PathBuf,
}

/// A typed failure from a cgroup operation.
///
/// The variants carry the distinctions callers need to make a decision:
/// `Foreign` means a structural write was refused by the single-writer rule,
/// `NotFound` means the cgroup or its control file is absent, `NoController`
/// means there is no usable hierarchy/controller, `Busy` is `-EBUSY`, `Gone`
/// is a process that disappeared (`-ESRCH`), and `Io` is everything else.
#[derive(Debug)]
pub enum CgroupError {
    InvalidName(String),
    Foreign,
    NotFound,
    NoController,
    Busy,
    Gone,
    Io(io::Error),
}

/// The result type of every fallible cgroup operation.
pub type CgroupResult<T> = Result<T, CgroupError>;

impl fmt::Display for CgroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "invalid cgroup name: {name}"),
            Self::Foreign => write!(f, "target is outside the delegated cgroup"),
            Self::NotFound => write!(f, "cgroup or control file not found"),
            Self::NoController => write!(f, "no cgroup hierarchy or controller available"),
            Self::Busy => write!(f, "cgroup is busy (EBUSY)"),
            Self::Gone => write!(f, "process no longer exists (ESRCH)"),
            Self::Io(error) => write!(f, "cgroup I/O error: {error}"),
        }
    }
}

impl Error for CgroupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for CgroupError {
    fn from(error: io::Error) -> Self {
        match error.raw_os_error() {
            Some(libc::EBUSY) => Self::Busy,
            Some(libc::ESRCH) => Self::Gone,
            _ if error.kind() == io::ErrorKind::NotFound => Self::NotFound,
            _ => Self::Io(error),
        }
    }
}
