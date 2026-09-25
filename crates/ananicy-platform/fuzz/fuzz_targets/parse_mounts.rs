#![no_main]
//! Differential-free fuzzing of the mount-table parser.
//!
//! The target is driven by raw bytes from `/proc/self/mounts`, so the parser has
//! to be total: it must never panic, and the `CgroupInfo` it fills in must stay
//! self-consistent. Run it with:
//!
//! ```text
//! cargo +nightly fuzz run parse_mounts --fuzz-dir crates/ananicy-platform/fuzz/corpus
//! ```

use {
    ananicy_platform::mounts::{CgroupInfo, CgroupVersion, parse_cgroups_from_str},
    libfuzzer_sys::fuzz_target,
    std::path::PathBuf,
};

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    let mut info = CgroupInfo {
        version: CgroupVersion::None,
        mount_point: PathBuf::new(),
    };
    parse_cgroups_from_str(input, &mut info);

    // Invariants of the resulting state.
    match info.version {
        // Nothing was recognised: no mount point may be reported.
        CgroupVersion::None => {
            assert!(
                info.mount_point.as_os_str().is_empty(),
                "a rejected mount table left a mount point behind: {info:?}"
            );
        }
        // A hierarchy was recognised: its mount point comes from the input and
        // is therefore never relative.
        CgroupVersion::V1 | CgroupVersion::V2 => {
            assert!(
                info.mount_point.is_absolute(),
                "a detected mount point is not absolute: {info:?}"
            );
        }
    }
});
