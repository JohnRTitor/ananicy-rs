use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CgroupPath(PathBuf);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupIdentity {
    pub path: CgroupPath,
    // Note: version might not be needed for matching, but if it is,
    // it could be represented as an enum or string. For now, since ananicy-core
    // doesn't have CgroupVersion from ananicy-platform, we can just omit it
    // or redefine a minimal enum here. For rule matching, only path is needed.
}

impl CgroupPath {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self(path.into())
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0
            .iter()
            .filter_map(|s| s.to_str())
            .filter(|s| *s != "/")
    }

    pub fn is_ancestor_of(&self, other: &CgroupPath) -> bool {
        other.0.starts_with(&self.0)
    }

    pub fn basename(&self) -> Option<&str> {
        self.0.file_name().and_then(|s| s.to_str())
    }

    /// Extracts the systemd unit name from the cgroup path, if it has a standard extension.
    /// E.g. `/user.slice/.../app-gnome-terminal-1234.scope` -> `app-gnome-terminal-1234.scope`
    pub fn derive_unit_name(&self) -> Option<&str> {
        self.basename().filter(|name| {
            name.ends_with(".service") || name.ends_with(".scope") || name.ends_with(".slice")
        })
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
    fn test_segments() {
        let p = CgroupPath::new("/user.slice/user-1000.slice/app.slice/app-foo.scope");
        let segments: Vec<&str> = p.segments().collect();
        assert_eq!(
            segments,
            vec![
                "user.slice",
                "user-1000.slice",
                "app.slice",
                "app-foo.scope"
            ]
        );
    }

    #[test]
    fn test_is_ancestor_of() {
        let p1 = CgroupPath::new("/user.slice/user-1000.slice");
        let p2 = CgroupPath::new("/user.slice/user-1000.slice/app.slice/app-foo.scope");
        let p3 = CgroupPath::new("/system.slice");

        assert!(p1.is_ancestor_of(&p2));
        assert!(!p2.is_ancestor_of(&p1));
        assert!(!p3.is_ancestor_of(&p2));
        assert!(p1.is_ancestor_of(&p1));
    }

    #[test]
    fn test_basename() {
        let p = CgroupPath::new("/user.slice/user-1000.slice/app.slice/app-foo.scope");
        assert_eq!(p.basename(), Some("app-foo.scope"));
    }

    #[test]
    fn test_derive_unit_name() {
        let p = CgroupPath::new("/user.slice/user-1000.slice/app.slice/app-foo.scope");
        assert_eq!(p.derive_unit_name(), Some("app-foo.scope"));

        let p_svc = CgroupPath::new("/system.slice/systemd-journald.service");
        assert_eq!(p_svc.derive_unit_name(), Some("systemd-journald.service"));

        let p_invalid = CgroupPath::new("/docker/12345abcde");
        assert_eq!(p_invalid.derive_unit_name(), None);
    }
}
