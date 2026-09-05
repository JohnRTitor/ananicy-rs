#![allow(clippy::collapsible_if)]
use {
    crate::{
        config::Config,
        types::{CgroupName, RuleName, TypeName},
    },
    serde_json::Value,
    std::{collections::HashMap, fs, num::NonZeroUsize, path::Path},
    tracing::{debug, error, warn},
};

pub struct Rules {
    config: std::sync::Arc<Config>,
    programs: HashMap<RuleName, Value>,
    types: HashMap<TypeName, Value>,
    cgroups: HashMap<CgroupName, Value>,
    // Store fallback regex rules if enabled
    regex_programs: Vec<(pcre2::bytes::Regex, String)>,
    // Cache for resolved rules to avoid linear scan overhead on every process
    resolved_cache: std::sync::Mutex<lru::LruCache<String, Option<Value>>>,
}

impl Rules {
    pub fn new(config: std::sync::Arc<Config>) -> Self {
        Self {
            config,
            programs: HashMap::new(),
            types: HashMap::new(),
            cgroups: HashMap::new(),
            regex_programs: Vec::new(),
            resolved_cache: std::sync::Mutex::new(lru::LruCache::new(
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
        for (name, mut rule) in self.programs.drain() {
            if let Some(type_name) = rule.get("type").and_then(|v| v.as_str()) {
                if let Some(type_rule) = self.types.get(&TypeName(type_name.to_string())) {
                    let mut merged = type_rule.clone();
                    merge_patch(&mut merged, &rule);
                    rule = merged;
                }
            }
            updated_programs.insert(name, rule);
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
                for line in content.lines() {
                    if !self.load_rule_from_string(line) {
                        // We only log debug here since blank lines and comments are normal
                        // But if it was an invalid JSON line, it will be logged by `load_rule_from_string`
                    }
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

        // C++ behavior: Find first '{' and last '}'
        let start = match line.find('{') {
            Some(i) => i,
            None => return false,
        };

        let end = match line.rfind('}') {
            Some(i) => i,
            None => return false,
        };

        if start > end {
            return false;
        }

        let json_str = &line[start..=end];
        match serde_json::from_str::<Value>(json_str) {
            Ok(value) => {
                if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                    self.programs
                        .insert(RuleName(name.to_string()), value.clone());

                    if let Some(regex_str) = value.get("name_regex").and_then(|v| v.as_str()) {
                        match pcre2::bytes::RegexBuilder::new()
                            .utf(true)
                            .ucp(true)
                            .build(regex_str)
                        {
                            Ok(re) => self.regex_programs.push((re, name.to_string())),
                            Err(e) => error!("Invalid regex '{}' in rule: {}", regex_str, e),
                        }
                    }
                    true
                } else if let Some(type_name) = value.get("type").and_then(|v| v.as_str()) {
                    // Type rule (has 'type' but no 'name')
                    // Actually, wait, program rules also have 'type'.
                    // The C++ logic sets it as a type rule if it HAS 'type' and NO 'name'
                    self.types.insert(TypeName(type_name.to_string()), value);
                    true
                } else if let Some(cgroup_name) = value.get("cgroup").and_then(|v| v.as_str()) {
                    // Cgroup rule
                    self.cgroups
                        .insert(CgroupName(cgroup_name.to_string()), value);
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

    pub fn get_rule(&self, name: &str) -> Option<Value> {
        // 0. Check cache
        let cache_key = name.to_string();

        if let Ok(mut cache) = self.resolved_cache.lock() {
            if let Some(cached_rule) = cache.get(&cache_key) {
                return cached_rule.clone();
            }
        }

        let best_match = self.find_best_match(name);

        // Update cache
        if let Ok(mut cache) = self.resolved_cache.lock() {
            cache.put(cache_key, best_match.clone());
        }

        best_match
    }

    fn find_best_match(&self, target_name: &str) -> Option<Value> {
        // 1. Exact match
        if let Some(rule) = self.programs.get(&RuleName(target_name.to_string())) {
            return Some(rule.clone());
        }

        // 2. Regex fallback
        for (re, prog_name) in &self.regex_programs {
            if re.is_match(target_name.as_bytes()).unwrap_or(false) {
                if let Some(rule) = self.programs.get(&RuleName(prog_name.clone())) {
                    return Some(rule.clone());
                }
            }
        }

        None
    }

    pub fn size(&self) -> usize {
        self.programs.len()
    }

    pub fn get_cgroups(&self) -> &HashMap<CgroupName, Value> {
        &self.cgroups
    }

    pub fn get_rules(&self) -> &HashMap<RuleName, Value> {
        &self.programs
    }

    pub fn get_types(&self) -> &HashMap<TypeName, Value> {
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
