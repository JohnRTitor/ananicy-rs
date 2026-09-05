use std::path::PathBuf;

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
