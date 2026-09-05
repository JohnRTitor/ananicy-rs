#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut info = ananicy_platform::mounts::CgroupInfo {
            version: ananicy_platform::mounts::CgroupVersion::None,
            mount_point: std::path::PathBuf::new(),
        };
        ananicy_platform::mounts::parse_cgroups_from_str(s, &mut info);
    }
});
