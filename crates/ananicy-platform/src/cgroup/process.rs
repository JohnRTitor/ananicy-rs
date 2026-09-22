use crate::cgroup::CgroupVersion;

use {
    lru::LruCache,
    std::{
        fs, io,
        num::NonZeroUsize,
        sync::RwLock,
        time::{Duration, Instant},
    },
};

use ananicy_core::cgroup::{CgroupIdentity, CgroupPath};

pub trait CgroupProcessResolver: Send + Sync {
    /// Resolve the *current* cgroup of a process. Returns Ok(None) if the
    /// process has no resolvable cgroup (kernel thread, already exited).
    fn resolve(&self, pid: i32) -> io::Result<Option<CgroupIdentity>>;
}

pub struct LinuxCgroupResolver {
    version: CgroupVersion,
}

impl LinuxCgroupResolver {
    pub fn new(version: CgroupVersion) -> Self {
        Self { version }
    }
}

impl CgroupProcessResolver for LinuxCgroupResolver {
    fn resolve(&self, pid: i32) -> io::Result<Option<CgroupIdentity>> {
        if self.version == CgroupVersion::None || self.version == CgroupVersion::V1 {
            return Ok(None);
        }

        let cgroup_file = format!("/proc/{}/cgroup", pid);
        let content = match fs::read_to_string(&cgroup_file) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };

        // v2 looks like "0::/user.slice/..."
        let Some(path_str) = content.lines().find_map(|line| line.strip_prefix("0::")) else {
            return Ok(None);
        };

        let path = path_str.trim_end_matches(" (deleted)");
        // Reject paths that start with /../ (escaping cgroup namespace)
        if path.starts_with("/../") {
            return Ok(None);
        }
        Ok(Some(CgroupIdentity {
            path: CgroupPath::new(path),
        }))
    }
}

/// (start_time, cgroup_identity, timestamp)
type CacheEntry = (u64, Option<CgroupIdentity>, Instant);

pub struct CachingCgroupResolver<R: CgroupProcessResolver> {
    inner: R,
    cache: RwLock<LruCache<i32, CacheEntry>>,
    ttl: Duration,
}

impl<R: CgroupProcessResolver> CachingCgroupResolver<R> {
    pub fn new(inner: R, capacity: usize, ttl: Duration) -> Self {
        let capacity = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN);

        Self {
            inner,
            cache: RwLock::new(LruCache::new(capacity)),
            ttl,
        }
    }
}

impl<R: CgroupProcessResolver> CgroupProcessResolver for CachingCgroupResolver<R> {
    fn resolve(&self, pid: i32) -> io::Result<Option<CgroupIdentity>> {
        let Some(start_time_current) = crate::procfs::get_start_time(pid) else {
            // Failed to get start time (process probably died), just return None
            return Ok(None);
        };

        // Try the cache first
        let Ok(mut cache) = self.cache.write() else {
            return Err(io::Error::other("cgroup resolver cache lock is poisoned"));
        };
        if let Some(&(cached_start_time, ref cached_id, ref timestamp)) = cache.get(&pid)
            && cached_start_time == start_time_current
            && timestamp.elapsed() < self.ttl
        {
            return Ok(cached_id.clone());
        }
        drop(cache); // Release lock before resolving

        // Cache miss, expired, or start_time mismatch
        let resolved = self.inner.resolve(pid)?;

        // Re-check start_time to prevent race condition during resolve
        if let Some(start_time_after) = crate::procfs::get_start_time(pid)
            && start_time_current == start_time_after
        {
            let Ok(mut cache) = self.cache.write() else {
                return Err(io::Error::other("cgroup resolver cache lock is poisoned"));
            };
            cache.put(pid, (start_time_current, resolved.clone(), Instant::now()));
            return Ok(resolved);
        }

        // Start time changed during resolve, return None to skip
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeCgroupResolver {
        version: CgroupVersion,
        content: String,
    }

    impl FakeCgroupResolver {
        fn resolve_from_content(&self) -> io::Result<Option<CgroupIdentity>> {
            if self.version == CgroupVersion::None || self.version == CgroupVersion::V1 {
                return Ok(None);
            }

            let Some(path_str) = self
                .content
                .lines()
                .find_map(|line| line.strip_prefix("0::"))
            else {
                return Ok(None);
            };

            let path = path_str.trim_end_matches(" (deleted)");
            if path.starts_with("/../") {
                return Ok(None);
            }
            Ok(Some(CgroupIdentity {
                path: CgroupPath::new(path),
            }))
        }
    }

    #[test]
    fn test_resolve_v2() {
        let resolver = FakeCgroupResolver {
            version: CgroupVersion::V2,
            content: "0::/user.slice/user-1000.slice/session-2.scope".to_string(),
        };
        let id = resolver.resolve_from_content().unwrap().unwrap();
        assert_eq!(id.path.basename(), Some("session-2.scope"));
    }

    #[test]
    fn test_resolve_v2_deleted() {
        let resolver = FakeCgroupResolver {
            version: CgroupVersion::V2,
            content: "0::/user.slice/user-1000.slice/session-2.scope (deleted)".to_string(),
        };
        let id = resolver.resolve_from_content().unwrap().unwrap();
        assert_eq!(id.path.basename(), Some("session-2.scope"));
    }

    #[test]
    fn test_resolve_namespace_escape() {
        let resolver = FakeCgroupResolver {
            version: CgroupVersion::V2,
            content: "0::/../user.slice".to_string(),
        };
        let id = resolver.resolve_from_content().unwrap();
        assert!(id.is_none());
    }
}
