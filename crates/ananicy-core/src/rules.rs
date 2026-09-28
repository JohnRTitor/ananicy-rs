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

/// A rule attribute as a rule file states it.
///
/// Three states, not two, and the third is the one that matters. `None` is "the
/// key is absent", `Some(None)` is "the key is present with nothing usable in
/// it", and `Some(Some(v))` is a value. The middle state is what lets a rule
/// carry `"nice": null` — or `"nice": "5"`, which is no more a number than
/// `null` is — and *suppress* a `nice` its type would otherwise have supplied,
/// which is what an RFC 7396 merge patch does with it. Collapsing the middle
/// state into the first would silently re-apply the type's value to a rule that
/// asked for none.
///
/// It costs nothing: `Option<Option<T>>` is the same size as `Option<i32>` and
/// the same size as `Option<Arc<str>>`, because `None` is a niche in both.
pub type Attribute<T> = Option<Option<T>>;

/// A program rule, resolved against its type.
///
/// This is the change that made the daemon fit in memory. A rule used to be kept
/// as a `serde_json::Value` — a `BTreeMap` with a node of roughly 700 bytes
/// however few keys it held — plus an `Arc` header, plus a `String` key, for
/// every rule in the set. A 15,829-rule default rule set cost about 18.6 MB, and
/// against the `MemoryHigh` the shipped unit used to carry that was most of the
/// problem: the working set did not fit, so the kernel reclaimed the daemon's own
/// pages continuously and the daemon spent its life re-reading itself from disk.
///
/// Every attribute here is a fixed-size field, and the five that are names —
/// `type`, `ioclass`, `sched`, `cgroup` and `cpuset` — are `Box<str>` or
/// `Arc<str>`, which is one allocation shared by every rule that uses the same
/// name rather than one each. A default rule set draws on sixteen distinct type
/// names and a handful of `ioclass`, `sched`, `cpuset` and cgroup names, so that
/// is a couple of dozen allocations behind fifteen thousand rules rather than a
/// hundred and fifty thousand.
///
/// A rule is 176 bytes, and the map stores them inline, so the whole set is about
/// 10.5 MB. It is worth being exact about where that goes, because the number is
/// not what the per-rule arithmetic suggests: the rules themselves are 2.8 MB of
/// it, the `HashMap` that indexes them is 6.5 MB (it holds each rule's 176 bytes
/// in a table sized for the next power of two), and the rest is the `RuleName`
/// keys. Storing the rules in a `Vec` and indexing that instead would bring it to
/// about 4.5 MB, and is not done here: it trades a single obvious invariant — the
/// map *is* the rules — for three megabytes in a process whose limit is now
/// 128M and whose working set has a hundred megabytes of room in it.
///
/// What a rule file may say is not closed: an attribute this daemon does not
/// implement still has to survive into `dump rules`, or a rule set using one
/// would be reported as if the attribute had never been written. Those keys go
/// in `extras` — empty for every rule in the shipped set, so they cost nothing
/// there, and populated for a rule that uses one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rule {
    /// The type this rule inherited from, kept for `dump rules` to report.
    pub type_name: Option<Box<str>>,
    /// `nice`. `-20..=19` on any real system, stored wider so that a rule
    /// carrying a value outside that is clamped by the kernel rather than
    /// wrapping here into a value the author did not write.
    pub nice: Attribute<i32>,
    /// `latency_nice`, which falls back to `nice` when the rule does not say.
    pub latency_nice: Attribute<i32>,
    /// `ioclass`, one of the class names `ionice_get` reports.
    pub ioclass: Attribute<Arc<str>>,
    /// `ionice`, the priority within that class.
    pub ionice: Attribute<i32>,
    /// `sched`, a policy name.
    pub sched: Attribute<Arc<str>>,
    /// `rtprio`, the priority for a realtime policy.
    pub rtprio: Attribute<u32>,
    /// `oom_score_adj`.
    pub oom_score_adj: Attribute<i32>,
    /// `cgroup`, the name of a cgroup a `.cgroups` rule creates.
    pub cgroup: Attribute<Arc<str>>,
    /// `cpuset`, either a CPU list or an alias the topology resolves.
    pub cpuset: Attribute<Arc<str>>,
    /// Keys this daemon does not implement, kept verbatim so `dump rules` stays
    /// a faithful report of what is on disk.
    pub extras: Vec<(Box<str>, Value)>,
}

/// Declares a resolved-value accessor for a numeric attribute.
///
/// `Attribute<T>` is three states and the caller wants two, so every one of
/// these answers "absent" and "declared with nothing usable" the same way — the
/// distinction is only needed while a type is being applied on top.
macro_rules! numeric_accessor {
    ($($(#[$meta:meta])* $name:ident : $field:ident),* $(,)?) => {
        impl Rule {
            $(
                $(#[$meta])*
                pub fn $name(&self) -> Option<i32> {
                    self.$field.flatten()
                }
            )*
        }
    };
}

/// Declares a resolved-value accessor for an attribute that is a name.
///
/// Returns a borrow rather than a clone: the `Rc` is shared with the rule, and
/// every caller wants to compare it or read it, not to own it.
macro_rules! name_accessor {
    ($($(#[$meta:meta])* $name:ident : $field:ident),* $(,)?) => {
        impl Rule {
            $(
                $(#[$meta])*
                pub fn $name(&self) -> Option<&str> {
                    self.$field.as_ref().and_then(|name| name.as_deref())
                }
            )*
        }
    };
}

impl Rule {
    /// Reads the attributes out of one parsed JSON object.
    ///
    /// Every read keeps the "present but unusable" state, so a rule that writes
    /// a number as a string suppresses the type's value instead of inheriting
    /// it. A key this daemon does not implement goes to `extras`; a key that is
    /// also an implemented one is recorded in both, because the implemented one
    /// is what applies and the other is what `dump rules` has to show.
    fn from_json(value: &Value) -> Self {
        let string = |key: &str| -> Attribute<Arc<str>> {
            match value.get(key) {
                None => None,
                Some(Value::String(text)) => Some(Some(Arc::from(text.as_str()))),
                Some(_) => Some(None),
            }
        };
        let number = |key: &str| -> Attribute<i32> {
            match value.get(key) {
                None => None,
                Some(Value::Number(number)) => Some(number.as_i64().map(|n| n as i32)),
                Some(_) => Some(None),
            }
        };

        // `name` is not an extra: it is the key this rule is stored under, and
        // `dump rules` prints it as the object's key. `type` is not one either —
        // it is in `type_name`, and is what inheritance looks up. Everything else
        // the daemon does not implement is an extra, `name_regex` included: a
        // pattern is part of how the rule matches, so a report of the rule that
        // left it out would be describing a different rule.
        const IMPLEMENTED: [&str; 11] = [
            "name",
            "type",
            "nice",
            "latency_nice",
            "ioclass",
            "ionice",
            "sched",
            "rtprio",
            "oom_score_adj",
            "cgroup",
            "cpuset",
        ];

        let extras = value
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .filter(|(key, _)| !IMPLEMENTED.contains(&key.as_str()))
                    .map(|(key, value)| (Box::from(key.as_str()), value.clone()))
                    .collect()
            })
            .unwrap_or_default();

        Rule {
            type_name: value.get("type").and_then(Value::as_str).map(Box::from),
            nice: number("nice"),
            latency_nice: number("latency_nice"),
            ioclass: string("ioclass"),
            ionice: number("ionice"),
            sched: string("sched"),
            rtprio: match value.get("rtprio") {
                None => None,
                Some(Value::Number(number)) => Some(number.as_u64().map(|n| n as u32)),
                Some(_) => Some(None),
            },
            oom_score_adj: number("oom_score_adj"),
            cgroup: string("cgroup"),
            cpuset: string("cpuset"),
            extras,
        }
    }

    /// Overlays this rule on `base`, which is the rule it inherits from.
    ///
    /// Done on the resolved attributes rather than on the JSON they came from, so
    /// inheritance costs one pass over nine fields instead of a clone of two
    /// `BTreeMap`s per rule — which is what made loading a large rule set briefly
    /// need twice the memory of the set it was building.
    ///
    /// `type_name` is the one field that is not overlaid: the rule's own is the
    /// one that is reported, and the type's is not a thing anybody asks about.
    fn inherit(&mut self, base: &Rule) {
        macro_rules! overlay {
            ($($field:ident),* $(,)?) => {
                $(
                    if self.$field.is_none() {
                        self.$field = base.$field.clone();
                    }
                )*
            };
        }
        overlay!(
            nice,
            latency_nice,
            ioclass,
            ionice,
            sched,
            rtprio,
            oom_score_adj,
            cgroup,
            cpuset,
        );
    }

    /// The JSON this rule reports as, which is what `dump rules` prints.
    ///
    /// `name` is the rule's key in the map rather than a field of the rule, so it
    /// is passed in and written out again. That is redundant, and it is what the
    /// dump has always printed — every value under `dump rules` carries a `name`
    /// equal to the key it sits under — so it stays. A report format that is
    /// rewritten as a side effect of how rules are stored is a change nobody
    /// asked for, and scripts read this output.
    ///
    /// Apart from `name`, the shape is the rule as written: a key the rule file
    /// declared is present, a key it did not is absent, and one declared with
    /// nothing usable in it is present and `null`. That is the same answer an RFC
    /// 7396 merge of the rule onto its type gives, which is what this replaced.
    pub fn to_json(&self, name: &str) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("name".into(), Value::String(name.to_string()));

        if let Some(type_name) = &self.type_name {
            object.insert("type".into(), Value::String(type_name.to_string()));
        }
        for (key, attribute) in [
            ("nice", self.nice.as_ref().map(|v| json_of_i32(*v))),
            (
                "latency_nice",
                self.latency_nice.as_ref().map(|v| json_of_i32(*v)),
            ),
            ("ioclass", self.ioclass.as_ref().map(json_of_str)),
            ("ionice", self.ionice.as_ref().map(|v| json_of_i32(*v))),
            ("sched", self.sched.as_ref().map(json_of_str)),
            ("rtprio", self.rtprio.as_ref().map(|v| json_of_u32(*v))),
            (
                "oom_score_adj",
                self.oom_score_adj.as_ref().map(|v| json_of_i32(*v)),
            ),
            ("cgroup", self.cgroup.as_ref().map(json_of_str)),
            ("cpuset", self.cpuset.as_ref().map(json_of_str)),
        ] {
            if let Some(value) = attribute {
                object.insert(key.into(), value.clone());
            }
        }
        for (key, value) in &self.extras {
            object.insert(key.to_string(), value.clone());
        }

        Value::Object(object)
    }
}

fn json_of_i32(value: Option<i32>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn json_of_u32(value: Option<u32>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn json_of_str(value: &Option<Arc<str>>) -> Value {
    value
        .as_ref()
        .map_or(Value::Null, |value| Value::String(value.to_string()))
}

numeric_accessor! {
    /// The `nice` this rule asks for, or `None` for "do not touch it".
    nice: nice,
    /// The `ionice` this rule asks for.
    ionice: ionice,
    /// The `oom_score_adj` this rule asks for.
    oom_score_adj: oom_score_adj,
}

name_accessor! {
    /// The `ioclass` this rule asks for.
    ioclass: ioclass,
    /// The `sched` policy this rule asks for.
    sched: sched,
    /// The cgroup this rule asks the process to be moved into.
    cgroup: cgroup,
    /// The cpuset this rule asks for, or an alias for one.
    cpuset: cpuset,
}

impl Rule {
    /// The `rtprio` this rule asks for.
    ///
    /// Separate from the numeric accessors because the kernel's field is
    /// unsigned and a rule's is a JSON number that need not be.
    pub fn rtprio(&self) -> Option<u32> {
        self.rtprio.flatten()
    }

    /// The `latency_nice` to apply, which is the rule's own or else its `nice`.
    ///
    /// A rule that sets `nice` and no `latency_nice` means the same value for
    /// both, which is what the reference does, and it is worth doing here: the
    /// two knobs are the same idea at different timescales, and a rule author
    /// who set one almost always means the other.
    pub fn effective_latency_nice(&self) -> Option<i32> {
        self.latency_nice.flatten().or_else(|| self.nice.flatten())
    }
}

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
    /// Program rules, resolved against their types, stored by value.
    ///
    /// By value and not behind an `Arc`: the map is read once per process the
    /// worker sees and never mutated while it is read, so a borrow is enough
    /// and there is no reason to pay for a reference count on fifteen thousand
    /// allocations. A rule used to be an `Arc<Value>`, which is what made the
    /// rule set cost twenty megabytes instead of two.
    programs: HashMap<RuleName, Rule>,
    /// Type definitions, as the JSON they were written as. There are a couple of
    /// dozen of these, so what they cost does not matter, and keeping them as
    /// written means `dump types` is a faithful report of the files.
    types: HashMap<TypeName, Arc<Value>>,
    cgroups: HashMap<CgroupName, Arc<Value>>,
    // Store fallback regex rules if enabled, in rule-file load order
    regex_programs: Vec<Matcher>,
    // Which rule a process name resolved to, so the regex scan below runs once
    // per distinct name rather than once per process. It is the *name* that is
    // remembered rather than the rule, because the rule lives in `programs` and
    // borrowing it costs nothing while copying it would cost an allocation.
    resolved_cache: Mutex<lru::LruCache<String, Option<RuleName>>>,
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

    /// Applies every program rule's type to it, in place.
    ///
    /// In place, and that is the point. This used to drain the whole rule map
    /// into a second one, merging as it went, so loading a rule set briefly
    /// needed room for both the unmerged and the merged copies — against a
    /// `MemoryHigh` the working set already exceeded, which is how a large rule
    /// set turned into a start-up that took minutes. Overlaying nine fixed-size
    /// fields in place needs room for one copy and no more.
    ///
    /// Types are read rather than taken: a program rule must not be able to
    /// rewrite the type it inherits from, and two rules sharing a type must not
    /// be able to see each other.
    fn precompute_inheritance(&mut self) {
        for rule in self.programs.values_mut() {
            let Some(type_name) = rule.type_name.clone() else {
                continue;
            };
            let Some(type_rule) = self.types.get(&TypeName(type_name.to_string())) else {
                continue;
            };
            let mut base = Rule::from_json(type_rule);
            // The type's own `type` key is not inherited: the rule reports the
            // type it named, and a type cannot itself inherit from a type.
            base.type_name = None;
            rule.inherit(&base);
        }

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
                    self.programs.insert(key, Rule::from_json(&value));

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

    /// The rule a process of this name gets, and the name it was declared under.
    ///
    /// Both, because they can differ: a `name_regex` rule is declared under one
    /// name and matches others, and `dump proc` reports the rule a process
    /// matched, not the name the process was called. A borrow rather than a copy
    /// — nothing here is mutated while a worker is reading it, and a reference
    /// costs no allocation.
    pub fn get_rule(&self, name: &str) -> Option<(&RuleName, &Rule)> {
        // 0. Check cache
        if let Ok(mut cache) = self.resolved_cache.lock()
            && let Some(cached) = cache.get(name)
        {
            return self.resolve(cached.as_ref());
        }

        let best_match = self.find_best_match(name);

        // The cache is keyed on the process name, and a process name is the
        // basename of `argv[0]` — which the kernel puts no upper bound on. A
        // single `execve` with a 128 KiB `argv[0]` therefore produces a 128 KiB
        // key, and 5000 of them is half a gigabyte, against a `MemoryMax=128M` in
        // the shipped unit. A name that long is not a program name and no rule
        // can match it usefully, so it is answered without being remembered. The
        // lookup itself still happens, so the answer is the same either way.
        let cacheable = name.len() <= MAX_CACHEABLE_NAME;
        if cacheable && let Ok(mut cache) = self.resolved_cache.lock() {
            cache.put(name.to_string(), best_match.clone());
        }

        self.resolve(best_match.as_ref())
    }

    /// Looks a rule up by the name it is declared under.
    fn resolve(&self, name: Option<&RuleName>) -> Option<(&RuleName, &Rule)> {
        name.and_then(|name| self.programs.get_key_value(name))
    }

    fn find_best_match(&self, target_name: &str) -> Option<RuleName> {
        // 1. Exact match
        if let Some(rule) = self.programs.get_key_value(target_name) {
            return Some(rule.0.clone());
        }

        // 2. Regex fallback, in rule-file load order, so the first pattern that
        // matches wins exactly as it does in the reference.
        for matcher in &self.regex_programs {
            if matcher.is_match(target_name)
                && let Some(rule) = self.programs.get_key_value(matcher.name.as_str())
            {
                return Some(rule.0.clone());
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

    pub fn get_rules(&self) -> &HashMap<RuleName, Rule> {
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

#[cfg(test)]
mod rule {
    use {super::*, crate::config::ConfigSnapshot};

    fn parse(json: &str) -> Rule {
        Rule::from_json(&serde_json::from_str(json).expect("a JSON rule"))
    }

    /// The whole reason the type exists. A 15,829-rule default rule set cost
    /// about 18.6 MB when each rule was a parsed JSON document, which against the
    /// `MemoryHigh` the shipped unit carried meant the working set did not fit
    /// and the kernel reclaimed the daemon's own pages continuously. It costs
    /// about 10.5 MB now, of which the rules themselves are 2.8 MB and the rest
    /// is the map that indexes them — see the type's documentation.
    ///
    /// This is the size that has to stay under a few hundred bytes. It does not
    /// pin the total, which is mostly the map, and it is a floor rather than a
    /// ceiling: a rule has nine attributes and this is what nine of them cost.
    #[test]
    fn a_rule_is_small() {
        let size = std::mem::size_of::<Rule>();
        assert!(
            size <= 192,
            "a rule is {size} bytes; 15,829 of them is {} MB, and it is held \
             inline in the map that indexes them",
            size * 15_829 / (1024 * 1024)
        );
    }

    /// A rule file is third-party content, and the set of keys this daemon
    /// implements is not the set a rule file may use. Dropping the rest would
    /// make `dump rules` report a rule as if the attribute had never been
    /// written, so they are kept and reported.
    #[test]
    fn an_attribute_this_daemon_does_not_implement_is_kept_and_reported() {
        let rule = parse(r#"{"name":"x","nice":5,"an_unknown_knob":{"a":[1,2]}}"#);
        assert_eq!(rule.nice(), Some(5));
        assert_eq!(rule.extras.len(), 1);
        assert_eq!(&*rule.extras[0].0, "an_unknown_knob");
        assert_eq!(rule.to_json("x")["an_unknown_knob"]["a"][1], 2);
    }

    /// A rule set that only uses implemented attributes — which is every
    /// shipped one — pays nothing for the preservation above, because the vector
    /// is empty rather than allocated-and-empty.
    #[test]
    fn a_rule_with_only_implemented_attributes_carries_no_extras() {
        let rule = parse(r#"{"name":"x","type":"T","nice":5,"ioclass":"idle","cpuset":"0-3"}"#);
        assert!(rule.extras.is_empty());
    }

    /// The report shape, which is what `dump rules` prints and what every
    /// assertion in the integration suite is written against. A key the rule
    /// file wrote is present; a key it did not is absent; one it wrote with
    /// nothing usable in it is present and `null`. That is exactly what an RFC
    /// 7396 merge of the rule onto its type produced.
    #[test]
    fn a_rule_reports_the_keys_it_declared() {
        let rule = parse(r#"{"name":"x","type":"T","nice":-7,"sched":"batch"}"#);
        let json = rule.to_json("x");
        assert_eq!(json["type"], "T");
        assert_eq!(json["nice"], -7);
        assert_eq!(json["sched"], "batch");
        assert!(
            json.get("cpuset").is_none(),
            "a key the rule did not declare must not be invented: {json}"
        );
    }

    /// A number written as a string is no more a number than `null` is, and both
    /// have to suppress the value a type would otherwise supply. Collapsing this
    /// into "absent" would silently re-apply the type's `nice` to a rule that
    /// asked for none.
    #[test]
    fn an_attribute_written_as_nothing_suppresses_the_types_value() {
        for written in [
            r#"{"name":"x","type":"T","nice":null}"#,
            r#"{"name":"x","type":"T","nice":"5"}"#,
        ] {
            let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
            rules.load_rule_from_string(r#"{"type":"T","nice":5}"#);
            assert!(rules.load_rule_from_string(written), "{written} is a rule");
            rules.precompute_inheritance();

            let (_, rule) = rules.get_rule("x").expect("the rule is there");
            assert_eq!(
                rule.nice(),
                None,
                "an unusable {written} must not fall back to the type's 5"
            );
            assert_eq!(
                rule.to_json("x")["nice"],
                serde_json::Value::Null,
                "and it is still reported as a declared key: {}",
                rule.to_json("x")
            );
        }
    }

    /// A rule that says nothing about an attribute inherits it, and one that
    /// says something overrides it. The whole of type inheritance is this.
    #[test]
    fn a_rule_overrides_its_type_and_inherits_the_rest() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        rules.load_rule_from_string(
            r#"{"type":"T","nice":-4,"latency_nice":5,"sched":"batch","ioclass":"idle","ionice":7}"#,
        );
        rules.load_rule_from_string(r#"{"name":"x","type":"T","nice":-7,"ionice":0}"#);
        rules.precompute_inheritance();

        let (_, rule) = rules.get_rule("x").expect("the rule is there");
        assert_eq!(rule.nice(), Some(-7), "the rule overrides the type");
        assert_eq!(rule.ionice(), Some(0), "including a zero, which is a value");
        assert_eq!(
            rule.sched(),
            Some("batch"),
            "and inherits what it is silent on"
        );
        assert_eq!(rule.ioclass(), Some("idle"));
        assert_eq!(rule.effective_latency_nice(), Some(5));
    }

    /// `latency_nice` falls back to `nice`, which is what the reference does and
    /// what a rule author means by setting one and not the other.
    #[test]
    fn a_latency_nice_falls_back_to_the_nice() {
        assert_eq!(
            parse(r#"{"name":"x","nice":7}"#).effective_latency_nice(),
            Some(7)
        );
        assert_eq!(
            parse(r#"{"name":"x","nice":7,"latency_nice":-3}"#).effective_latency_nice(),
            Some(-3),
            "an explicit latency_nice wins over the fallback"
        );
        assert_eq!(parse(r#"{"name":"x"}"#).effective_latency_nice(), None);
    }

    /// The names two rules share are one allocation, not two. A default rule set
    /// has fifteen thousand rules drawing on a couple of dozen distinct type
    /// names, ioclasses and cpusets, so this is the difference between one
    /// hundred thousand allocations at load and a couple of dozen.
    #[test]
    fn a_shared_name_is_one_allocation() {
        let value: Arc<str> = Arc::from("Doc-View");
        let mut rule = parse(r#"{"name":"x"}"#);
        rule.type_name = Some("Doc-View".into());
        rule.sched = Some(Some(value.clone()));
        assert_eq!(
            rule.sched.as_ref().and_then(|s| s.as_deref()),
            Some("Doc-View")
        );
        assert_eq!(
            std::sync::Arc::strong_count(&value),
            2,
            "the rule holds the one allocation both names came from"
        );
    }

    /// A rule that declares a name in an unexpected type is treated as declaring
    /// nothing usable, which is the same answer the `Value`-based reader gave:
    /// `get("ioclass").and_then(as_str)` was `None` for a number, and so is this.
    #[test]
    fn an_attribute_of_the_wrong_type_is_declared_but_not_usable() {
        let rule = parse(r#"{"name":"x","ioclass":7,"sched":null,"nice":true}"#);
        assert_eq!(rule.ioclass(), None);
        assert_eq!(rule.sched(), None);
        assert_eq!(rule.nice(), None);
        let json = rule.to_json("x");
        assert_eq!(json["ioclass"], serde_json::Value::Null);
        assert_eq!(json["sched"], serde_json::Value::Null);
        assert_eq!(json["nice"], serde_json::Value::Null);
    }

    /// `rtprio` is the one attribute the kernel reads as unsigned, so a negative
    /// value is declared but not usable rather than wrapping into a large
    /// positive priority.
    #[test]
    fn a_negative_realtime_priority_is_not_usable() {
        assert_eq!(parse(r#"{"name":"x","rtprio":99}"#).rtprio(), Some(99));
        assert_eq!(parse(r#"{"name":"x","rtprio":-1}"#).rtprio(), None);
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

        let current = shared.get();
        let rule = current.get_rule("cached").expect("the rule is still there");
        assert_eq!(
            rule.1.nice(),
            Some(9),
            "the reloaded value must be what is read, not the cached earlier one"
        );
    }
}
