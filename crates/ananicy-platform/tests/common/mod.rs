//! Hermetic cgroup test harness.
//!
//! [`SimHierarchy`] builds a temporary cgroup-v2 layout with a delegated root
//! and a foreign systemd subtree. [`FakeController`] implements
//! `CgroupController` against that layout, records every control-line write and
//! can inject typed failures (including raw errnos) so tests can assert that
//! errors reach callers with their kind intact.

#![allow(dead_code)]

use {
    ananicy_platform::cgroup::{
        CgroupError, CgroupInfo, CgroupResult, CgroupVersion,
        manager::{CgroupController, CgroupManager},
    },
    std::{
        cell::RefCell,
        collections::HashMap,
        fs, io,
        path::{Path, PathBuf},
    },
    tempfile::TempDir,
};

/// A temporary `mount -> system.slice -> ananicy.service` hierarchy, plus a
/// `user.slice` sibling that is foreign to the daemon.
pub struct SimHierarchy {
    _temp: TempDir,
    pub mount_point: PathBuf,
    pub delegated_root: PathBuf,
    pub foreign_root: PathBuf,
}

impl SimHierarchy {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("a temporary cgroup hierarchy");
        let mount_point = temp.path().to_path_buf();
        let delegated_root = mount_point.join("system.slice/ananicy.service");
        let foreign_root = mount_point.join("user.slice");
        fs::create_dir_all(&delegated_root).expect("a delegated cgroup");
        fs::create_dir_all(&foreign_root).expect("a foreign cgroup");
        fs::write(delegated_root.join("cgroup.procs"), "").expect("our cgroup.procs");
        fs::write(foreign_root.join("cgroup.procs"), "1\n").expect("foreign cgroup.procs");
        Self {
            _temp: temp,
            mount_point,
            delegated_root,
            foreign_root,
        }
    }

    /// Creates a populated cgroup below the delegated root and seeds the named
    /// control files, as the kernel would.
    pub fn owned_cgroup(&self, name: &str, control_files: &[&str]) -> PathBuf {
        let path = self.delegated_root.join(name);
        fs::create_dir_all(&path).expect("an owned cgroup");
        fs::write(path.join("cgroup.procs"), "").expect("owned cgroup.procs");
        for file in control_files {
            fs::write(path.join(file), "").expect("a control file");
        }
        path
    }

    /// Creates a cgroup outside the delegated root.
    pub fn foreign_cgroup(&self, name: &str, control_files: &[&str]) -> PathBuf {
        let path = self.foreign_root.join(name);
        fs::create_dir_all(&path).expect("a foreign cgroup");
        for file in control_files {
            fs::write(path.join(file), "").expect("a control file");
        }
        path
    }

    pub fn manager(&self) -> CgroupManager {
        CgroupManager::new_with_root(
            CgroupInfo {
                version: CgroupVersion::V2,
                mount_point: self.mount_point.clone(),
            },
            Some(self.delegated_root.clone()),
        )
    }
}

impl Default for SimHierarchy {
    fn default() -> Self {
        Self::new()
    }
}

/// A failure the fake can inject. Errnos go through the same
/// `From<io::Error>` conversion the real manager uses, so `EBUSY` really does
/// become `CgroupError::Busy` rather than being asserted by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Foreign,
    NotFound,
    NoController,
    Busy,
    Gone,
    Errno(i32),
}

impl Failure {
    fn into_error(self) -> CgroupError {
        match self {
            Self::Foreign => CgroupError::Foreign,
            Self::NotFound => CgroupError::NotFound,
            Self::NoController => CgroupError::NoController,
            Self::Busy => CgroupError::Busy,
            Self::Gone => CgroupError::Gone,
            Self::Errno(errno) => CgroupError::from(io::Error::from_raw_os_error(errno)),
        }
    }
}

/// One recorded control-file write. The line includes its trailing newline,
/// matching what the kernel receives in a single `write()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedWrite {
    pub path: PathBuf,
    pub line: String,
}

/// A recording `CgroupController` over [`SimHierarchy`].
pub struct FakeController {
    hierarchy: SimHierarchy,
    failures: RefCell<HashMap<&'static str, Failure>>,
    writes: RefCell<Vec<RecordedWrite>>,
}

impl FakeController {
    pub fn new() -> Self {
        Self {
            hierarchy: SimHierarchy::new(),
            failures: RefCell::new(HashMap::new()),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// Makes `operation` fail on every call. The names match the trait methods.
    pub fn failing(self, operation: &'static str, failure: Failure) -> Self {
        self.failures.borrow_mut().insert(operation, failure);
        self
    }

    pub fn hierarchy(&self) -> &SimHierarchy {
        &self.hierarchy
    }

    pub fn writes(&self) -> Vec<RecordedWrite> {
        self.writes.borrow().clone()
    }

    /// All lines written to files named `file`, in order.
    pub fn lines_for(&self, file: &str) -> Vec<String> {
        self.writes()
            .into_iter()
            .filter(|write| write.path.file_name().is_some_and(|name| name == file))
            .map(|write| write.line)
            .collect()
    }

    fn check(&self, operation: &'static str) -> CgroupResult<()> {
        match self.failures.borrow().get(operation) {
            Some(failure) => Err((*failure).into_error()),
            None => Ok(()),
        }
    }

    fn owned(&self, target: &Path) -> bool {
        target.starts_with(&self.hierarchy.delegated_root)
    }

    fn record(&self, target: &Path, file: &str, line: String) {
        self.writes.borrow_mut().push(RecordedWrite {
            path: target.join(file),
            line,
        });
    }
}

impl Default for FakeController {
    fn default() -> Self {
        Self::new()
    }
}

impl CgroupController for FakeController {
    fn ensure_child(&self, name: &str) -> CgroupResult<PathBuf> {
        self.check("ensure_child")?;
        let target = self
            .hierarchy
            .manager()
            .resolve_target_dir(name)
            .ok_or_else(|| CgroupError::InvalidName(name.to_string()))?;
        if !self.owned(&target) {
            return Err(CgroupError::Foreign);
        }
        if !target.exists() {
            fs::create_dir_all(&target).map_err(CgroupError::from)?;
        }
        Ok(target)
    }

    fn move_pid(&self, pid: i32, target: &Path) -> CgroupResult<()> {
        self.check("move_pid")?;
        if !self.owned(target) {
            return Err(CgroupError::Foreign);
        }
        if !target.exists() {
            return Err(CgroupError::NotFound);
        }
        self.record(target, "cgroup.procs", format!("{pid}\n"));
        Ok(())
    }

    fn set_cpu_max(&self, target: &Path, quota: u32) -> CgroupResult<()> {
        self.check("set_cpu_max")?;
        if !self.owned(target) {
            return Err(CgroupError::Foreign);
        }
        if !target.join("cpu.max").exists() {
            return Err(CgroupError::NotFound);
        }
        self.record(target, "cpu.max", format!("{quota} 100000\n"));
        Ok(())
    }

    fn set_cpu_weight(&self, target: &Path, weight: u32) -> CgroupResult<()> {
        self.check("set_cpu_weight")?;
        if !target.join("cpu.weight").exists() {
            return Err(CgroupError::NotFound);
        }
        // Unlike cpu.max, tuning an existing foreign cpu.weight is allowed.
        self.record(target, "cpu.weight", format!("{weight}\n"));
        Ok(())
    }
}
