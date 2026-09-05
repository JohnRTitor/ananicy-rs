use {
    libc::{cpu_set_t, sched_setaffinity},
    std::{fs, io},
};

pub fn get_max_number_of_cpus() -> u32 {
    let sys_cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) as u32 };
    // Ensure we allocate at least sizeof(cpu_set_t) which covers 1024 CPUs
    std::cmp::max(sys_cpus, 1024)
}

pub fn set_affinity(pid: i32, cpuset: &ananicy_core::cpuset::CpuSet) -> io::Result<()> {
    if cpuset.get_cores().is_empty() {
        return Ok(());
    }

    let num_cpus = get_max_number_of_cpus();
    let num_bytes = (num_cpus as usize) / 8;
    let mut mask = vec![0u8; num_bytes];

    for cpu in cpuset.get_cores() {
        if cpu < num_cpus {
            let byte_idx = (cpu / 8) as usize;
            let bit_idx = cpu % 8;
            mask[byte_idx] |= 1 << bit_idx;
        }
    }

    let task_path = format!("/proc/{}/task", pid);
    let mut last_err = None;

    if let Ok(entries) = fs::read_dir(&task_path) {
        for entry in entries.flatten() {
            if let Ok(file_name) = entry.file_name().into_string()
                && let Ok(tid) = file_name.parse::<i32>()
            {
                let ret =
                    unsafe { sched_setaffinity(tid, num_bytes, mask.as_ptr() as *const cpu_set_t) };
                if ret != 0 {
                    last_err = Some(io::Error::last_os_error());
                } else {
                    last_err = Some(io::Error::from_raw_os_error(0));
                }
            }
        }
    } else {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }

    match last_err {
        Some(err) if err.raw_os_error() == Some(0) => {
            tracing::debug!("set_affinity: Successfully applied to {}", pid);
            Ok(())
        }
        Some(err) => Err(err),
        None => {
            tracing::debug!("set_affinity: Successfully applied to {}", pid);
            Ok(())
        }
    }
}
