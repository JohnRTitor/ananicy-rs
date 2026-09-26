use std::sync::{Arc, Mutex};

use {
    crate::{
        config::Config,
        types::{CgroupName, RuleName, TypeName},
    },
    serde_json::Value,
    std::{collections::HashMap, fs, num::NonZeroUsize, path::Path},
    tracing::{debug, error, warn},
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
                    self.load_rule_from_string(line);
                }
            }
            Err(e) => {
                error!("Failed to read rule file {:?}: {}", path, e);
            }
        }
    }

    pub fn load_rule_from_string(&mut self, line: &str) -> bool {
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
                    self.programs
                        .insert(RuleName(name.to_string()), Arc::new(value.clone()));

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
                    self.types
                        .insert(TypeName(type_name.to_string()), Arc::new(value));
                    true
                } else if let Some(cgroup_name) = value.get("cgroup").and_then(|v| v.as_str()) {
                    // Cgroup rule
                    self.cgroups
                        .insert(CgroupName(cgroup_name.to_string()), Arc::new(value));
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

    pub fn get_rule(&self, name: &str) -> Option<Arc<Value>> {
        // 0. Check cache
        if let Ok(mut cache) = self.resolved_cache.lock()
            && let Some(cached_rule) = cache.get(name)
        {
            return cached_rule.clone();
        }

        let best_match = self.find_best_match(name);

        // Update cache
        if let Ok(mut cache) = self.resolved_cache.lock() {
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
