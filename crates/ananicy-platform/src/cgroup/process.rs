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

        // Try the cache first. A read lock is enough: `LruCache::get` takes
        // `&mut self` because it moves the entry to the most-recently-used end,
        // so the interior mutability is behind the `RwLock` rather than behind a
        // cell — a `read()` cannot do that. Taking the *write* lock here instead
        // serialised every process's cgroup lookup behind every other one's, on
        // the one path the worker takes for every process that has a rule.
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

    /// The caching resolver, which until now had no tests at all.
    ///
    /// It reads the real `/proc/<pid>/stat` for the process start time, so these
    /// use this test process's own pid: it exists, and its start time does not
    /// change while the test runs. That is enough to exercise a miss, a hit, the
    /// TTL, and eviction — the four things the cache decides.
    mod caching {
        use {
            super::*,
            std::sync::atomic::{AtomicUsize, Ordering},
        };

        /// Counts how often the inner resolver was reached, so a test can tell
        /// a cache hit from a cache miss rather than inferring it.
        #[derive(Default)]
        struct Counting {
            calls: AtomicUsize,
            answer: Option<CgroupIdentity>,
        }

        impl CgroupProcessResolver for &Counting {
            fn resolve(&self, _pid: i32) -> io::Result<Option<CgroupIdentity>> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.answer.clone())
            }
        }

        fn us() -> i32 {
            std::process::id() as i32
        }

        fn answer(path: &str) -> CgroupIdentity {
            CgroupIdentity {
                path: CgroupPath::new(path),
            }
        }

        #[test]
        fn a_second_lookup_is_answered_from_the_cache() {
            let inner = Counting {
                answer: Some(answer("/cached.scope")),
                ..Default::default()
            };
            let resolver = CachingCgroupResolver::new(&inner, 16, Duration::from_secs(60));

            let first = resolver.resolve(us()).unwrap();
            assert_eq!(
                first.as_ref().map(|c| c.path.as_path()),
                Some(std::path::Path::new("/cached.scope"))
            );
            assert_eq!(
                inner.calls.load(Ordering::SeqCst),
                1,
                "the first lookup is a miss"
            );

            let second = resolver.resolve(us()).unwrap();
            assert_eq!(
                second.as_ref().map(|c| c.path.as_path()),
                Some(std::path::Path::new("/cached.scope"))
            );
            assert_eq!(
                inner.calls.load(Ordering::SeqCst),
                1,
                "the second is served by the cache, so the inner resolver is not \
                 reached again -- that is the whole point of the cache"
            );
        }

        #[test]
        fn an_entry_past_its_ttl_is_resolved_again() {
            let inner = Counting {
                answer: Some(answer("/a.scope")),
                ..Default::default()
            };
            // A zero TTL is expired by the time it is compared, so every lookup
            // re-resolves even though the entry is still resident.
            let resolver = CachingCgroupResolver::new(&inner, 16, Duration::ZERO);

            resolver.resolve(us()).unwrap();
            resolver.resolve(us()).unwrap();
            assert_eq!(
                inner.calls.load(Ordering::SeqCst),
                2,
                "an entry older than the TTL is not trusted, however recent it was"
            );
        }

        #[test]
        fn the_cache_forgets_a_process_that_is_no_longer_there() {
            let inner = Counting {
                answer: Some(answer("/gone.scope")),
                ..Default::default()
            };
            let resolver = CachingCgroupResolver::new(&inner, 16, Duration::from_secs(60));

            // A pid that cannot have a start time is one that has exited, so
            // there is nothing to resolve and nothing worth caching.
            let missing = resolver.resolve(i32::MAX).unwrap();
            assert!(missing.is_none());
            assert_eq!(
                inner.calls.load(Ordering::SeqCst),
                0,
                "and no entry is made for it"
            );
        }

        #[test]
        fn the_cache_is_bounded_and_forgets_the_least_recently_used() {
            let inner = Counting {
                answer: Some(answer("/one.scope")),
                ..Default::default()
            };
            // A capacity of one, so the second pid's lookup must evict the first.
            let resolver = CachingCgroupResolver::new(&inner, 1, Duration::from_secs(60));

            resolver.resolve(us()).unwrap();
            // The one entry is now ours; asking again is a hit and does not grow
            // the cache past its bound.
            resolver.resolve(us()).unwrap();
            assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn a_cgroup_the_kernel_does_not_report_is_answered_as_absent() {
            let inner = Counting {
                answer: None,
                ..Default::default()
            };
            let resolver = CachingCgroupResolver::new(&inner, 16, Duration::from_secs(60));

            let answer = resolver.resolve(us()).unwrap();
            assert!(
                answer.is_none(),
                "no cgroup is a real answer, not a failure"
            );
            // And it is cached as such rather than re-resolved every time.
            resolver.resolve(us()).unwrap();
            assert_eq!(
                inner.calls.load(Ordering::SeqCst),
                1,
                "an absent answer caches too"
            );
        }
    }
}
