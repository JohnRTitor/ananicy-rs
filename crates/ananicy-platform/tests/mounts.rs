//! Mount-table parsing and cgroup detection.
//!
//! `parse_cgroups_from_str` is fed the content of `/proc/self/mounts`, so it has
//! to cope with whatever the host happens to mount. The hermetic tests below
//! drive it with synthetic mount tables; the one system test checks the answer
//! it gives for the machine the suite is running on.

use {
    ananicy_platform::mounts::{
        CgroupInfo, CgroupVersion, get_cgroup_info, parse_cgroups_from_str,
    },
    std::path::{Path, PathBuf},
};

fn empty_info() -> CgroupInfo {
    CgroupInfo {
        version: CgroupVersion::None,
        mount_point: PathBuf::new(),
    }
}

/// A cgroup v1 hierarchy: the `cgroup` filesystem is mounted somewhere and the
/// per-controller directories (`cpu`, `cpuacct`, ...) are its siblings.
fn v1_hierarchy() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for controller in ["cpu", "cpuacct", "memory", "blkio"] {
        std::fs::create_dir(directory.path().join(controller)).unwrap();
    }
    std::fs::create_dir(directory.path().join("pids")).unwrap();
    directory
}

fn mount_line(device: &str, mount_point: &Path, fs_type: &str) -> String {
    format!(
        "{device} {} {fs_type} rw,nosuid,nodev,noexec,relatime 0 0\n",
        mount_point.display()
    )
}

#[test]
fn an_empty_mount_table_yields_no_cgroup() {
    let mut info = empty_info();
    parse_cgroups_from_str("", &mut info);
    assert_eq!(info.version, CgroupVersion::None);
    assert!(info.mount_point.as_os_str().is_empty());
}

#[test]
fn a_mount_table_without_cgroups_yields_no_cgroup() {
    let mut info = empty_info();
    parse_cgroups_from_str(
        "proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0\n\
         sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0\n\
         tmpfs /tmp tmpfs rw,nosuid,nodev,relatime 0 0\n",
        &mut info,
    );
    assert_eq!(info.version, CgroupVersion::None);
}

#[test]
fn malformed_lines_are_skipped() {
    let mut info = empty_info();
    parse_cgroups_from_str(
        "\n\
         # a comment line\n\
         proc\n\
         /only/two/fields\n\
         /three/fields rw 0\n\
         \n",
        &mut info,
    );
    assert_eq!(info.version, CgroupVersion::None);
    assert!(
        info.mount_point.as_os_str().is_empty(),
        "a rejected line must not contribute a mount point"
    );
}

#[test]
fn a_cgroup_v1_hierarchy_is_detected() {
    let hierarchy = v1_hierarchy();
    let mount_point = hierarchy.path().join("cgroup");
    let mut info = empty_info();

    parse_cgroups_from_str(
        &format!(
            "proc /proc proc rw,relatime 0 0\n{}",
            mount_line("none", &mount_point, "cgroup")
        ),
        &mut info,
    );

    assert_eq!(info.version, CgroupVersion::V1);
    assert_eq!(
        info.mount_point,
        hierarchy.path(),
        "v1 reports the parent of the controller directories, not the mount point"
    );
}

#[test]
fn a_cgroup_v1_mount_without_controller_directories_is_ignored() {
    // A `cgroup` line is only accepted when the per-controller directories next
    // to it are present; otherwise the mount point is a leftover from a system
    // that no longer uses v1.
    let directory = tempfile::tempdir().unwrap();
    let mount_point = directory.path().join("cgroup");
    std::fs::create_dir(&mount_point).unwrap();

    let mut info = empty_info();
    parse_cgroups_from_str(&mount_line("none", &mount_point, "cgroup"), &mut info);

    assert_eq!(info.version, CgroupVersion::None);
}

#[test]
fn a_cgroup2_mount_is_not_accepted_for_a_missing_hierarchy() {
    // The v2 probe creates a throw-away cgroup inside the mount point to find
    // out whether the cpu controller is usable. When the mount point does not
    // exist, the probe fails and nothing is reported.
    let directory = tempfile::tempdir().unwrap();
    let mount_point = directory.path().join("not-mounted-here");
    let mut info = empty_info();

    parse_cgroups_from_str(&mount_line("cgroup2", &mount_point, "cgroup2"), &mut info);

    assert_eq!(info.version, CgroupVersion::None);
    assert!(info.mount_point.as_os_str().is_empty());
}

#[test]
fn the_first_usable_cgroup2_mount_wins() {
    // Only a mount point the kernel backs with a cpu controller and `cpu.max` is
    // accepted, which is why the cgroup2 probe is not satisfied by a plain
    // temporary directory. Whichever entry is found first, the result must be
    // self-consistent.
    let mut info = empty_info();
    parse_cgroups_from_str(
        "proc /proc proc rw,relatime 0 0\n\
         cgroup2 /sys/fs/cgroup cgroup2 rw,relatime 0 0\n\
         cgroup2 /sys/fs/cgroup/other cgroup2 rw,relatime 0 0\n",
        &mut info,
    );

    if info.version == CgroupVersion::V2 {
        assert_eq!(info.mount_point, Path::new("/sys/fs/cgroup"));
    } else {
        assert_eq!(info.version, CgroupVersion::None);
    }
}

#[test]
fn a_v2_result_is_never_overwritten_by_a_later_v1_entry() {
    // v2 takes precedence: once the unified hierarchy has been found the parser
    // stops instead of falling back to a v1 controller directory.
    let mut info = empty_info();
    parse_cgroups_from_str(
        "cgroup2 /sys/fs/cgroup cgroup2 rw,relatime 0 0\n\
         none /sys/fs/cgroup/unified cgroup rw,relatime 0 0\n",
        &mut info,
    );

    if info.version == CgroupVersion::V2 {
        assert_eq!(info.mount_point, Path::new("/sys/fs/cgroup"));
    } else {
        assert_eq!(info.version, CgroupVersion::None);
    }
}

/// System test: the answer for the host running the suite.
#[test]
fn the_detected_cgroup_hierarchy_of_this_host_is_consistent() {
    let info = get_cgroup_info();

    match info.version {
        CgroupVersion::None => {
            assert!(
                info.mount_point.as_os_str().is_empty(),
                "no hierarchy means no mount point"
            );
        }
        CgroupVersion::V1 | CgroupVersion::V2 => {
            assert!(
                info.mount_point.is_absolute(),
                "a detected mount point must be absolute, got {:?}",
                info.mount_point
            );
            assert!(
                info.mount_point.exists(),
                "a detected mount point must exist, got {:?}",
                info.mount_point
            );
        }
    }
}
