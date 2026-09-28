use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// The cgroup a process belongs to, as the kernel reports it in
/// `/proc/<pid>/cgroup`.
///
/// The path is kept as the kernel spelled it — relative, without the leading
/// controller id — so it can be joined onto the cgroup mount point without a
/// second interpretation step.
///
/// Shared rather than owned, because the answer is handed out far more often
/// than it is built: the cgroup resolver caches one per pid and returns a copy
/// on every lookup, which on a cgroup-v2 host is once per process carrying a
/// `nice`. An owned `PathBuf` made each of those a fresh copy of a path that
/// was already in memory; this makes it a reference count. A `PathBuf` and not
/// an `Arc<str>` or an `Arc<OsStr>` because a cgroup path is bytes the kernel
/// chose and reinterpreting it as text would either lose the invalid ones or
/// refuse the valid ones.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CgroupPath(Arc<PathBuf>);

/// A process' current cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupIdentity {
    pub path: CgroupPath,
}

impl CgroupPath {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self(Arc::new(path.into()))
    }

    pub fn basename(&self) -> Option<&str> {
        self.0.file_name().and_then(|s| s.to_str())
    }

    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    /// How many copies of this path exist.
    ///
    /// A test seam, so that a caller holding a path the resolver cached can ask
    /// whether a lookup handed back a reference to it or a fresh copy. It is
    /// `#[doc(hidden)]` because nothing outside a test has a reason to.
    #[doc(hidden)]
    pub fn holders(&self) -> usize {
        Arc::strong_count(&self.0)
    }
}

impl AsRef<Path> for CgroupPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basename() {
        let p = CgroupPath::new("/user.slice/user-1000.slice/app.slice/app-foo.scope");
        assert_eq!(p.basename(), Some("app-foo.scope"));
    }

    #[test]
    fn test_as_path() {
        let p = CgroupPath::new("/user.slice/app-foo.scope");
        assert_eq!(p.as_path(), Path::new("/user.slice/app-foo.scope"));
    }

    /// The reason the path is shared. Every lookup the resolver serves from its
    /// cache hands a copy back, and an owned `PathBuf` made that an allocation
    /// and a copy of a path the cache already held.
    #[test]
    fn a_copy_shares_the_path_rather_than_copying_it() {
        let original = CgroupPath::new("/user.slice/app-foo.scope");
        let copy = original.clone();
        assert_eq!(original, copy);
        assert_eq!(
            Arc::strong_count(&original.0),
            2,
            "one path behind both, so a cached lookup allocates nothing"
        );
    }
}
