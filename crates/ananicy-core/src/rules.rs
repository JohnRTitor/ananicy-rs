use std::sync::{Arc, Mutex, RwLock};

use {
    crate::{
        config::Config,
        types::{CgroupName, RuleName, TypeName},
    },
    serde_json::Value,
    std::{collections::HashMap, fs, num::NonZeroUsize, path::Path},
    tracing::{debug, error, info, warn},
};

/// One `name_regex` rule, compiled once when the rule file is read.
///
/// A rule file is third-party content, so the pattern is not this daemon's to
/// trust, and the ways one can hurt are bounded by the engine's own defaults
/// rather than by anything written here:
///
/// * expansion is capped at roughly a tenth of a second of compilation, so a
///   rule like `\w{200000,}` — eleven characters — is a log line instead of a
///   start-up stall;
/// * nesting is capped at 250 levels, which is a stack overflow otherwise. The
///   release profile sets `panic = "abort"`, so an overflow would take the whole
///   daemon down rather than just this thread.
///
/// Both are `regexr`'s defaults and are deliberately not raised; the previous
/// engine set no equivalent limit, so a rule like either of the above could take
/// the process out. This type exists rather than a bare `Vec<(Regex, String)>` so
/// that the engine stays behind `compile` and `is_match`, which makes the limits
/// and the budget arm below the only places a pattern can reach.
struct Matcher {
    compiled: regexr::Regex,
    /// The `name` of the program rule this pattern belongs to. Held so a failure
    /// can name the rule it came from; the pattern's own source is also kept by
    /// the engine, for the same purpose.
    name: String,
}

impl Matcher {
    fn compile(pattern: &str, name: &str) -> Result<Self, regexr::Error> {
        // No `jit(true)`, and no `default-features = false` either: the `simd`
        // feature is what drives the literal prefilter, and the prefilter is what
        // makes a match cheap on a haystack of 3-15 bytes — the kernel truncates
        // `/proc/<pid>/comm` to 15, and an `argv[0]` basename is short in
        // practice. A JIT cannot amortise its own compilation over a match that
        // size, and most rule patterns would not take a JIT anyway: one with an
        // alternation goes to an ordered-NFA engine so that PCRE's leftmost-first
        // branch priority is reproduced, and one with a lookahead goes to the
        // tagged-NFA engine, and neither of those has a JIT on this target.
        Ok(Self {
            compiled: regexr::RegexBuilder::new(pattern).build()?,
            name: name.to_string(),
        })
    }

    /// Whether `name` matches. Every engine `regexr` can pick is linear in the
    /// length of `name`, so this is bounded work; the one exception is a pattern
    /// containing a backreference, because matching one is NP-hard in general.
    /// That runs on a backtracking engine under a step budget, and the budget
    /// being spent is reported rather than hung on.
    fn is_match(&self, name: &str) -> bool {
        match self.compiled.try_is_match(name) {
            Ok(matched) => matched,
            Err(e) => {
                // A bounded "no match" with a log behind it is the right answer
                // for a rule this daemon did not write: leaving a process alone is
                // better than applying settings decided by a truncated search.
                warn!(
                    "name_regex '{}' (rule '{}') exhausted its match budget: {}",
                    self.compiled.as_str(),
                    self.name,
                    e
                );
                false
            }
        }
    }
}

pub struct Rules {
    config: Arc<Config>,
    programs: HashMap<RuleName, Arc<Value>>,
    types: HashMap<TypeName, Arc<Value>>,
    cgroups: HashMap<CgroupName, Arc<Value>>,
    // Store fallback regex rules if enabled, in rule-file load order
    regex_programs: Vec<Matcher>,
    // Cache for resolved rules to avoid linear scan overhead on every process
    resolved_cache: Mutex<lru::LruCache<String, Option<Arc<Value>>>>,
}

/// The longest process name the resolved-rule cache will remember.
///
/// The kernel caps `/proc/<pid>/comm` at 15 bytes and `Task comm` at 64, but not
/// `argv[0]`, whose first argument may be up to `MAX_ARG_STRLEN` (128 KiB) — and
/// `get_command_from_pid` prefers `argv[0]`. A name longer than this is not a
/// program name: it matches no rule that anybody would write, and caching it
/// would let a handful of local processes push the daemon past its `MemoryMax`.
const MAX_CACHEABLE_NAME: usize = 256;

impl Rules {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            programs: HashMap::new(),
            types: HashMap::new(),
            cgroups: HashMap::new(),
            regex_programs: Vec::new(),
            resolved_cache: Mutex::new(lru::LruCache::new(
                NonZeroUsize::new(5000).unwrap_or(NonZeroUsize::MIN),
            )),
        }
    }

    /// Recursively loads all `.rules`, `.types`, and `.cgroups` files from a directory
    pub fn load_directory<P: AsRef<Path>>(&mut self, dir: P) {
        let dir = dir.as_ref();
        if !dir.exists() || !dir.is_dir() {
            warn!(
                "Rules directory {:?} does not exist or is not a directory",
                dir
            );
            return;
        }

        // Start from nothing, so a second load is a replacement and not an
        // accumulation. Appending would keep every `name_regex` matcher the
        // first load compiled — a rule deleted from disk stays in force, and a
        // duplicated one is matched against twice for every process — and would
        // keep types and cgroups that no longer exist.
        self.programs.clear();
        self.types.clear();
        self.cgroups.clear();
        self.regex_programs.clear();
        if let Ok(mut cache) = self.resolved_cache.lock() {
            cache.clear();
        }

        let mut paths: Vec<_> = walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect();

        // Sort to ensure deterministic loading order
        paths.sort();

        let cfg = self.config.get();

        for path in paths {
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if (ext == "rules" && cfg.rule_load)
                || (ext == "types" && cfg.type_load)
                || (ext == "cgroups" && cfg.cgroup_load)
            {
                self.load_file(&path);
            }
        }

        self.precompute_inheritance();
    }

    fn precompute_inheritance(&mut self) {
        // Pre-merge all programs with their respective types so we don't have to
        // do expensive JSON merge-patches at runtime.
        let mut updated_programs = HashMap::new();
        for (name, rule_arc) in self.programs.drain() {
            let mut rule = Arc::unwrap_or_clone(rule_arc);
            // Merge into a clone so a program rule cannot mutate a shared type definition.
            if let Some(type_name) = rule.get("type").and_then(|v| v.as_str())
                && let Some(type_rule) = self.types.get(&TypeName(type_name.to_string()))
            {
                let mut merged = (**type_rule).clone();
                merge_patch(&mut merged, &rule);
                rule = merged;
            }
            updated_programs.insert(name, Arc::new(rule));
        }
        self.programs = updated_programs;

        // Also clear the cache since rules have been reloaded
        if let Ok(mut cache) = self.resolved_cache.lock() {
            cache.clear();
        }
    }

    pub fn load_file<P: AsRef<Path>>(&mut self, file: P) {
        let path = file.as_ref();
        match fs::read_to_string(path) {
            Ok(content) => {
                debug!("Loading rules from {:?}", path);
                // Rule files may use CRLF line endings; `lines()` treats `\r\n` as one line ending.
                for line in content.lines() {
                    // The result is intentionally ignored: blank lines and comments are normal,
                    // and an invalid JSON line is already logged by `load_rule_from_string`.
                    self.load_rule_from_named(line, Some(path));
                }
            }
            Err(e) => {
                error!("Failed to read rule file {:?}: {}", path, e);
            }
        }
    }

    /// Says so when a definition replaces one of the same name read earlier.
    ///
    /// A rule directory is a pile of third-party files plus whatever the operator
    /// adds, and the last entry read for a name wins outright. That is the
    /// documented way to override a shipped rule, and it is completely silent:
    /// `dump rules` prints the winner, the loser is gone from the map, and
    /// nothing in the output distinguishes an override from a rule that was
    /// always there. So a rule that is quietly not doing anything — or quietly
    /// doing something else — is indistinguishable from a working one until
    /// somebody greps the rule directory by hand. This is the one line that
    /// makes it visible, and it is `info` because an operator who overrides a
    /// rule on purpose should see it in the ordinary log.
    fn note_redefinition(replaced: bool, what: &str, name: &str, source: Option<&Path>) {
        if !replaced {
            return;
        }
        match source {
            Some(path) => info!(
                "{} '{}' from {} replaces an earlier definition of the same name",
                what,
                name,
                path.display()
            ),
            None => info!(
                "{} '{}' replaces an earlier definition of the same name",
                what, name
            ),
        }
    }

    /// Parses one line, attributing anything it defines to `source` for the
    /// redefinition diagnostic.
    fn load_rule_from_named(&mut self, line: &str, source: Option<&Path>) -> bool {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return false;
        }

        // The rule is the text between the first '{' and the last '}'. Taking
        // the widest span is what makes a trailing `# comment` and leading
        // whitespace tolerable, and it is the behaviour of the rule files that
        // shipped with the C++ daemon.
        let Some(start) = line.find('{') else {
            return false;
        };

        let Some(end) = line.rfind('}') else {
            return false;
        };

        if start > end {
            return false;
        }

        let json_str = &line[start..=end];
        match serde_json::from_str::<Value>(json_str) {
            Ok(value) => {
                if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                    let key = RuleName(name.to_string());
                    Self::note_redefinition(self.programs.contains_key(&key), "rule", name, source);
                    self.programs.insert(key, Arc::new(value.clone()));

                    if let Some(regex_str) = value.get("name_regex").and_then(|v| v.as_str()) {
                        match Matcher::compile(regex_str, name) {
                            Ok(matcher) => self.regex_programs.push(matcher),
                            // The rule is already in `programs`, so a pattern
                            // this daemon cannot compile costs the process its
                            // regex matching and nothing else: it still matches
                            // by exact name.
                            Err(e) => error!("Invalid regex '{}' in rule: {}", regex_str, e),
                        }
                    }
                    true
                } else if let Some(type_name) = value.get("type").and_then(|v| v.as_str()) {
                    // Type rule (has 'type' but no 'name')
                    // Actually, wait, program rules also have 'type'.
                    // The C++ logic sets it as a type rule if it HAS 'type' and NO 'name'
                    let key = TypeName(type_name.to_string());
                    Self::note_redefinition(
                        self.types.contains_key(&key),
                        "type",
                        type_name,
                        source,
                    );
                    self.types.insert(key, Arc::new(value));
                    true
                } else if let Some(cgroup_name) = value.get("cgroup").and_then(|v| v.as_str()) {
                    // Cgroup rule
                    let key = CgroupName(cgroup_name.to_string());
                    Self::note_redefinition(
                        self.cgroups.contains_key(&key),
                        "cgroup",
                        cgroup_name,
                        source,
                    );
                    self.cgroups.insert(key, Arc::new(value));
                    true
                } else {
                    error!(
                        "Rule must have 'name', 'type', or 'cgroup' field: {}",
                        json_str
                    );
                    false
                }
            }
            Err(e) => {
                error!("Failed to parse rule JSON: {} - Error: {}", json_str, e);
                false
            }
        }
    }

    /// Parses one rule, type or cgroup definition from a line of JSON.
    ///
    /// A line with no file behind it still gets the redefinition diagnostic; it
    /// just cannot name where the earlier definition came from.
    pub fn load_rule_from_string(&mut self, line: &str) -> bool {
        self.load_rule_from_named(line, None)
    }

    pub fn get_rule(&self, name: &str) -> Option<Arc<Value>> {
        // 0. Check cache
        if let Ok(mut cache) = self.resolved_cache.lock()
            && let Some(cached_rule) = cache.get(name)
        {
            return cached_rule.clone();
        }

        let best_match = self.find_best_match(name);

        // The cache is keyed on the process name, and a process name is the
        // basename of `argv[0]` — which the kernel puts no upper bound on. A
        // single `execve` with a 128 KiB `argv[0]` therefore produces a 128 KiB
        // key, and 5000 of them is half a gigabyte, against a `MemoryMax=64M` in
        // the shipped unit. A name that long is not a program name and no rule
        // can match it usefully, so it is answered without being remembered. The
        // lookup itself still happens, so the answer is the same either way.
        let cacheable = name.len() <= MAX_CACHEABLE_NAME;
        if cacheable && let Ok(mut cache) = self.resolved_cache.lock() {
            cache.put(name.to_string(), best_match.clone());
        }

        best_match
    }

    fn find_best_match(&self, target_name: &str) -> Option<Arc<Value>> {
        // 1. Exact match
        if let Some(rule) = self.programs.get(&RuleName(target_name.to_string())) {
            return Some(rule.clone());
        }

        // 2. Regex fallback, in rule-file load order, so the first pattern that
        // matches wins exactly as it does in the reference.
        for matcher in &self.regex_programs {
            if matcher.is_match(target_name)
                && let Some(rule) = self.programs.get(&RuleName(matcher.name.clone()))
            {
                return Some(rule.clone());
            }
        }

        None
    }

    pub fn size(&self) -> usize {
        self.programs.len()
    }

    pub fn get_cgroups(&self) -> &HashMap<CgroupName, Arc<Value>> {
        &self.cgroups
    }

    pub fn get_rules(&self) -> &HashMap<RuleName, Arc<Value>> {
        &self.programs
    }

    pub fn get_types(&self) -> &HashMap<TypeName, Arc<Value>> {
        &self.types
    }

    /// How many process names the resolved-rule cache is currently holding.
    ///
    /// A test seam and nothing else: the cache is private, and the property that
    /// matters about it — that a name too long to be a program name does not end
    /// up in it — is not observable from outside without asking. `doc(hidden)`
    /// because it is not part of the crate's interface.
    #[doc(hidden)]
    pub fn cached_resolution_count(&self) -> usize {
        self.resolved_cache.lock().map(|c| c.len()).unwrap_or(0)
    }
}

/// Simple JSON merge patch (RFC 7396) implementation
fn merge_patch(target: &mut Value, patch: &Value) {
    if let Value::Object(patch_obj) = patch {
        if !target.is_object() {
            *target = Value::Object(serde_json::Map::new());
        }
        let Some(target_obj) = target.as_object_mut() else {
            return;
        };

        for (k, v) in patch_obj {
            if v.is_null() {
                target_obj.remove(k);
            } else {
                let target_val = target_obj.entry(k.clone()).or_insert(Value::Null);
                merge_patch(target_val, v);
            }
        }
    } else {
        *target = patch.clone();
    }
}

/// A rule set that can be replaced while the daemon is running.
///
/// `--reload` promises to pick up rule changes, and a rule set built once at
/// start-up cannot deliver that: the worker holds it for the life of the process
/// and reads it on every process it sees. This wraps it so a reload can build a
/// new one and swap it in, which is only safe because the reader takes a snapshot
/// rather than borrowing across the swap.
///
/// An `RwLock` rather than an `ArcSwap` because it is in `std` and the read side
/// is a shared lock plus an `Arc` clone — once per process received, which is
/// nothing next to the procfs reads the same iteration does.
#[derive(Clone)]
pub struct SharedRules(Arc<RwLock<Arc<Rules>>>);

impl SharedRules {
    pub fn new(rules: Rules) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(rules))))
    }

    /// The current rule set.
    ///
    /// The returned `Arc` is a snapshot: a reload that lands after this call does
    /// not affect the set being read, so one process is always matched against one
    /// consistent set of rules rather than a mixture of the old and the new.
    ///
    /// A poisoned lock yields the last set that was successfully installed rather
    /// than propagating the panic. The only way to poison it is a panic while
    /// holding it, and refusing to tune any process from then on would be a far
    /// worse outcome than using a slightly stale rule set.
    pub fn get(&self) -> Arc<Rules> {
        match self.0.read() {
            Ok(rules) => rules.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Installs a freshly loaded rule set, returning how many rules it holds.
    ///
    /// Takes the lock for the swap only, so a process being read at the same moment
    /// is unaffected and simply sees one set or the other.
    pub fn replace(&self, rules: Rules) -> usize {
        let size = rules.size();
        let installed = Arc::new(rules);
        match self.0.write() {
            Ok(mut current) => *current = installed,
            Err(poisoned) => *poisoned.into_inner() = installed,
        }
        size
    }
}

#[cfg(test)]
mod shared_rules {
    use {super::*, crate::config::ConfigSnapshot};

    fn rules_named(name: &str) -> Rules {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        rules.load_rule_from_string(&format!(r#"{{"name":"{name}","nice":5}}"#));
        rules
    }

    /// The point of the snapshot: a reload that lands while a process is being
    /// matched must not change the rules that process is matched against. If the
    /// reader borrowed instead, this could not be said.
    #[test]
    fn a_reader_keeps_the_set_it_started_with() {
        let shared = SharedRules::new(rules_named("before"));
        let in_flight = shared.get();
        shared.replace(rules_named("after"));

        assert!(
            in_flight.get_rule("before").is_some(),
            "the set taken before the swap must still answer for itself"
        );
        assert!(
            in_flight.get_rule("after").is_none(),
            "and must not see the set installed after it"
        );
        assert!(
            shared.get().get_rule("after").is_some(),
            "a reader arriving after the swap sees the new set"
        );
        assert!(shared.get().get_rule("before").is_none());
    }

    /// Every clone names the same set, which is what lets the worker and the
    /// signal handler each hold one.
    #[test]
    fn a_clone_sees_the_swap() {
        let shared = SharedRules::new(rules_named("before"));
        let other = shared.clone();
        shared.replace(rules_named("after"));

        assert!(other.get().get_rule("after").is_some());
    }

    /// A rule that is removed from disk must stop applying after a reload. This
    /// is the user-visible half of the feature: before it, only a restart dropped
    /// a rule.
    #[test]
    fn a_removed_rule_stops_applying_after_a_reload() {
        let shared = SharedRules::new(rules_named("doomed"));
        assert!(shared.get().get_rule("doomed").is_some());

        shared.replace(rules_named("survivor"));
        assert!(shared.get().get_rule("doomed").is_none());
        assert!(shared.get().get_rule("survivor").is_some());
    }

    /// The count is reported so a reload can say what changed.
    #[test]
    fn replace_reports_the_installed_rule_count() {
        let shared = SharedRules::new(rules_named("one"));
        assert_eq!(shared.replace(rules_named("one")), 1);
    }

    /// A fresh set has an empty resolution cache, so a rule edited on disk is not
    /// answered out of the previous set's cache.
    #[test]
    fn a_reloaded_set_does_not_answer_from_the_old_cache() {
        let first = rules_named("cached");
        let shared = SharedRules::new(first);
        // Warm the cache in the installed set.
        assert!(shared.get().get_rule("cached").is_some());

        // Replace it with a set where that rule has a different value.
        let mut second = rules_named("cached");
        second.load_rule_from_string(r#"{"name":"cached","nice":9}"#);
        shared.replace(second);

        let rule = shared
            .get()
            .get_rule("cached")
            .expect("the rule is still there");
        assert_eq!(
            rule.get("nice").and_then(|v| v.as_i64()),
            Some(9),
            "the reloaded value must be what is read, not the cached earlier one"
        );
    }
}
