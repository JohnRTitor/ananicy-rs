use std::path::{Path, PathBuf};

/// The cgroup a process belongs to, as the kernel reports it in
/// `/proc/<pid>/cgroup`.
///
/// The path is kept as the kernel spelled it — relative, without the leading
/// controller id — so it can be joined onto the cgroup mount point without a
/// second interpretation step.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CgroupPath(PathBuf);

/// A process' current cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupIdentity {
    pub path: CgroupPath,
}

impl CgroupPath {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self(path.into())
    }

    pub fn basename(&self) -> Option<&str> {
        self.0.file_name().and_then(|s| s.to_str())
    }

    pub fn as_path(&self) -> &Path {
        self.0.as_path()
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
}
