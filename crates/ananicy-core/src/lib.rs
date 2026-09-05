pub mod cgroup;
pub mod cgroup_rules;
pub mod config;
pub mod cpuset;
pub mod process;
pub mod rules;
pub mod types;
pub mod worker;

/// Spawns a new thread with the given name.
/// Equivalent to `std::thread::spawn`, but allows setting a custom thread name.
#[macro_export]
macro_rules! spawn_named_thread {
    ($name:expr, $f:expr) => {
        std::thread::Builder::new()
            .name($name.into())
            .spawn($f)
            .expect("failed to spawn thread")
    };
}
