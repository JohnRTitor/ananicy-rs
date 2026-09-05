use std::{
    fs, io,
    sync::RwLock,
    time::{Duration, Instant},
};
use lru::LruCache;
use std::num::NonZeroUsize;

use {
    crate::cgroup::CgroupVersion,
    ananicy_core::cgroup::{CgroupIdentity, CgroupPath},
};

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

        for line in content.lines() {
            // v2 looks like "0::/user.slice/..."
            if line.starts_with("0::") {
                if let Some(path_str) = line.strip_prefix("0::") {
                    let path = path_str.trim_end_matches(" (deleted)");
                    // Reject paths that start with /../ (escaping cgroup namespace)
                    if path.starts_with("/../") {
                        return Ok(None);
                    }
                    return Ok(Some(CgroupIdentity {
                        path: CgroupPath::new(path),
                    }));
                }
            }
        }

        Ok(None)
    }
}

pub struct CachingCgroupResolver<R: CgroupProcessResolver> {
    inner: R,
    // (start_time, cgroup_identity, timestamp)
    cache: RwLock<LruCache<i32, (u64, Option<CgroupIdentity>, Instant)>>,
    ttl: Duration,
}

impl<R: CgroupProcessResolver> CachingCgroupResolver<R> {
    pub fn new(inner: R, capacity: usize, ttl: Duration) -> Self {
        Self {
            inner,
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(capacity).unwrap())),
            ttl,
        }
    }
}

impl<R: CgroupProcessResolver> CgroupProcessResolver for CachingCgroupResolver<R> {
    fn resolve(&self, pid: i32) -> io::Result<Option<CgroupIdentity>> {
        // Try the cache first
        if let Some(start_time_current) = crate::procfs::get_start_time(pid) {
            let mut cache = self.cache.write().unwrap();
            if let Some(&(cached_start_time, ref cached_id, ref timestamp)) = cache.get(&pid) {
                if cached_start_time == start_time_current && timestamp.elapsed() < self.ttl {
                    return Ok(cached_id.clone());
                }
            }
            drop(cache); // Release lock before resolving

            // Cache miss, expired, or start_time mismatch
            let resolved = self.inner.resolve(pid)?;
            
            // Re-check start_time to prevent race condition during resolve
            if let Some(start_time_after) = crate::procfs::get_start_time(pid) {
                if start_time_current == start_time_after {
                    let mut cache = self.cache.write().unwrap();
                    cache.put(pid, (start_time_current, resolved.clone(), Instant::now()));
                    return Ok(resolved);
                }
            }
            
            // Start time changed during resolve, return None to skip
            return Ok(None);
        }
        
        // Failed to get start time (process probably died), just return None
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

            for line in self.content.lines() {
                if line.starts_with("0::") {
                    if let Some(path_str) = line.strip_prefix("0::") {
                        let path = path_str.trim_end_matches(" (deleted)");
                        if path.starts_with("/../") {
                            return Ok(None);
                        }
                        return Ok(Some(CgroupIdentity {
                            path: CgroupPath::new(path),
                        }));
                    }
                }
            }
            Ok(None)
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
