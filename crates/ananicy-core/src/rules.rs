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
/// For a number it costs nothing: `Option<Option<i32>>` is the same size as
/// `Option<i32>`, because the inner `Option`'s tag leaves a niche the outer one
/// uses. For a name it did cost something — `Option<Option<Arc<str>>>` is 24
/// bytes, not the 16 that `Option<Arc<str>>` is, because the inner `Option`
/// spends the null-pointer niche and the outer one needs a discriminant of its
/// own. Four of those was 96 bytes of a rule's 168. [`NameAttribute`] is the same
/// three states in two.
pub type Attribute<T> = Option<Option<T>>;

/// The first index that names a value; below it are the two non-value states.
const NAME_FIRST_VALUE: u16 = 2;

/// A name a rule can carry, as an index into a process-wide table.
///
/// `ioclass`, `sched`, `cgroup` and `cpuset` are drawn from a few dozen distinct
/// values across a rule set of any size, and each was a 24-byte
/// `Option<Option<Arc<str>>>`. A default rule set spends 96 of its 168 bytes per
/// rule naming a handful of things. This is the same three states — absent,
/// declared with nothing usable in it, and a value — in two bytes.
///
/// Copy, and the table is never pruned, so a rule can hold one without a
/// lifetime and the accessors can keep handing out `&'static str`. See
/// [`NAME_TABLE`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct NameAttribute(u16);

impl NameAttribute {
    /// The key was not in the rule file at all.
    pub const fn absent() -> Self {
        Self(0)
    }

    /// The key was in the rule file with nothing usable in it, which is a
    /// decision: it suppresses whatever a type would have supplied.
    pub const fn unusable() -> Self {
        Self(1)
    }

    /// Interns `name` and returns the attribute that names it.
    fn intern(name: &str) -> Self {
        Self(NAME_TABLE.intern(name))
    }

    /// Whether the rule file said nothing about this key.
    ///
    /// The distinction this preserves is between "absent" and "declared
    /// unusable", and only inheritance needs it: a rule that wrote `"ioclass":
    /// null` must keep that rather than take its type's value.
    pub const fn is_absent(self) -> bool {
        self.0 == 0
    }

    /// The name, or `None` for either of the two states that are not a value.
    ///
    /// Absent and declared-unusable both answer `None` here, exactly as
    /// `Attribute::flatten` answered `None` for both. The difference between
    /// them is only visible to [`Rule::inherit`] and [`Rule::to_json`].
    pub fn as_str(self) -> Option<&'static str> {
        match self.0 {
            0 | 1 => None,
            n => NAME_TABLE.get((n - NAME_FIRST_VALUE) as usize),
        }
    }
}

/// A type name, as an index into the process-wide name table.
///
/// The two-state counterpart to [`NameAttribute`]: a `type` is either a string
/// the rule file wrote, which names a type, or it is absent, or it is something
/// else entirely — and something else was already recorded as "no type" rather
/// than as a declared-but-unusable one, because a type is looked up rather than
/// applied. Zero means "this rule names no type".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TypeNameId(u16);

impl TypeNameId {
    fn intern(name: &str) -> Self {
        // The table numbers from 2 (0 and 1 are the two non-value states a
        // `NameAttribute` needs), and this shifts down by one so that 0 can mean
        // "names no type" without colliding with a real entry.
        Self(NAME_TABLE.intern(name) - 1)
    }

    pub fn as_str(self) -> Option<&'static str> {
        match self.0 {
            0 => None,
            // Back up into the table's own numbering before indexing it.
            n => NAME_TABLE.get((n - 1) as usize),
        }
    }

    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// Every distinct name any rule in this process has carried.
///
/// Process-wide, and never pruned, which is what lets a [`NameAttribute`] be a
/// `Copy` index that stays valid after the rule set that made it is gone — the
/// accessors then return `&'static str` and no signature changes.
///
/// The cost is that a name interned by a rule set that has since been reloaded
/// away is not freed. That is bounded by the number of *distinct* names ever
/// seen rather than by the number of rules or reloads, so an operator editing a
/// cgroup name on every reload accumulates one entry each time: a few dozen
/// bytes, against a rule set measured in megabytes. Pruning it would mean
/// reference counting every field of every rule to know when the last one went,
/// which is not worth the complexity for that.
///
/// Loading is single-threaded, so the writes below do not contend in practice;
/// they are locked because `load_rule_from_string` is public.
static NAME_TABLE: std::sync::LazyLock<NameTable> = std::sync::LazyLock::new(NameTable::default);

#[derive(Default)]
struct NameTable {
    values: RwLock<Vec<&'static str>>,
    index: RwLock<HashMap<&'static str, u16>>,
}

impl NameTable {
    fn intern(&self, name: &str) -> u16 {
        if let Some(&found) = self
            .index
            .read()
            .expect("the name index is not poisoned")
            .get(name)
        {
            return found;
        }

        let mut values = self.values.write().expect("the name table is not poisoned");
        // Re-checked under the write lock: two threads loading at once would
        // otherwise both see a miss and both append.
        if let Some(&found) = self
            .index
            .read()
            .expect("the name index is not poisoned")
            .get(name)
        {
            return found;
        }

        // The two lowest indices are the states that are not values, so a table
        // entry starts at two.
        let id = NAME_FIRST_VALUE as usize + values.len();
        if id > u16::MAX as usize {
            // 65,533 distinct names in one process. A rule set holding that
            // many distinct ioclasses, scheds, cgroups and cpusets is far larger
            // than the memory this saves, and it cannot be reached by accident.
            // Said once, and the name is recorded as declared-but-unusable
            // rather than silently dropped or silently becoming the value.
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                error!(
                    "The rule name table is full; names past this point are recorded as unusable"
                )
            });
            return 1;
        }

        // Leaked rather than owned by the table's `values`, so that the
        // `HashMap` key and the stored `&'static str` are the same pointer and
        // no second copy of the name is kept. See the type's documentation for
        // why nothing is ever freed.
        let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
        values.push(leaked);
        self.index
            .write()
            .expect("the name index is not poisoned")
            .insert(leaked, id as u16);
        id as u16
    }

    fn get(&self, id: usize) -> Option<&'static str> {
        self.values
            .read()
            .expect("the name table is not poisoned")
            .get(id)
            .copied()
    }
}

/// A program rule, resolved against its type.
///
/// A rule used to be held as a `serde_json::Value`, which is a `BTreeMap` whose
/// node is roughly 700 bytes however few keys it holds -- about 1.2 KB for a
/// `{"name":..., "type":...}` rule, times fifteen thousand. The typed fields
/// below make that 72 bytes. See `docs/CONFIGURATION.md` § Memory.
///
/// The five names it carries — its `type`, `ioclass`, `sched`, `cgroup` and
/// `cpuset` — were 96 of that, as `Option<Option<Arc<str>>>` at 24 bytes each,
/// holding a few dozen distinct values across fifteen thousand rules. They are
/// [`NameAttribute`]s and a [`TypeNameId`] now, two bytes each, addressed
/// through [`NAME_TABLE`]. The accessors still hand out `&str`, and `to_json`
/// still produces the same document, so nothing a caller can observe moved.
///
/// Two properties the representation has to keep, both of which have a test
/// here. A name the rules share must resolve to the same index, or the sharing
/// is gone; and a name a rule declared with nothing usable in it must stay
/// distinct from one it said nothing about, or `"ioclass": null` stops
/// suppressing the value its type would have supplied.
///
/// What these bytes cost depends on where they are put, which is why they are in
/// a `Vec` and not in the map that finds them. A hash map's bucket count is the
/// next power of two above `count * 8/7`, so a per-rule cost inside the map is a
/// staircase: at 200 bytes a bucket, rule 28,673 cost 10.7 MB more than rule
/// 28,670. At 28 bytes a bucket it costs 1.2 MB.
///
/// An attribute this daemon does not implement is kept in `extras` rather than
/// dropped, so `dump rules` still reports what a rule file says. `extras` is
/// empty for every rule in the shipped set, so this costs nothing there.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rule {
    /// The type this rule inherited from, kept for `dump rules` to report.
    ///
    /// Interned like the four names, and for the same reason: a default rule set
    /// draws on a couple of dozen distinct type names across fifteen thousand
    /// rules, and this was the last owned string in the struct at sixteen bytes.
    ///
    /// Two states rather than three, because a `type` that is not a string was
    /// already `None` here and never had the middle one — a rule either names a
    /// type or does not. Zero means absent; anything else indexes [`NAME_TABLE`].
    pub type_name: TypeNameId,
    /// `nice`. `-20..=19` on any real system, stored wider so that a rule
    /// carrying a value outside that is clamped by the kernel rather than
    /// wrapping here into a value the author did not write.
    pub nice: Attribute<i32>,
    /// `latency_nice`, which falls back to `nice` when the rule does not say.
    pub latency_nice: Attribute<i32>,
    /// `ioclass`, one of the class names `ionice_get` reports.
    pub ioclass: NameAttribute,
    /// `ionice`, the priority within that class.
    pub ionice: Attribute<i32>,
    /// `sched`, a policy name.
    pub sched: NameAttribute,
    /// `rtprio`, the priority for a realtime policy.
    pub rtprio: Attribute<u32>,
    /// `oom_score_adj`.
    pub oom_score_adj: Attribute<i32>,
    /// `cgroup`, the name of a cgroup a `.cgroups` rule creates.
    pub cgroup: NameAttribute,
    /// `cpuset`, either a CPU list or an alias the topology resolves.
    pub cpuset: NameAttribute,
    /// Keys this daemon does not implement, kept verbatim so `dump rules` stays
    /// a faithful report of what is on disk.
    ///
    /// A boxed slice rather than a `Vec`: nothing appends to this after the rule
    /// is built, so the capacity a `Vec` carries is eight bytes per rule that
    /// never gets used — and it is empty for every rule in the shipped set, so
    /// an empty `Box<[T]>` costs no allocation at all.
    pub extras: Box<[(Box<str>, Value)]>,
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
/// Returns a borrow rather than a clone: the name lives once in
/// [`NAME_TABLE`] however many rules name it, and every caller wants to compare
/// it or read it, not to own it.
macro_rules! name_accessor {
    ($($(#[$meta:meta])* $name:ident : $field:ident),* $(,)?) => {
        impl Rule {
            $(
                $(#[$meta])*
                pub fn $name(&self) -> Option<&str> {
                    self.$field.as_str()
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
        let string = |key: &str| -> NameAttribute {
            match value.get(key) {
                None => NameAttribute::absent(),
                Some(Value::String(text)) => NameAttribute::intern(text),
                Some(_) => NameAttribute::unusable(),
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

        let extras: Box<[(Box<str>, Value)]> = value
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
            type_name: value
                .get("type")
                .and_then(Value::as_str)
                .map_or_else(TypeNameId::default, TypeNameId::intern),
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
        // A `NameAttribute` is only overlaid when the rule said nothing at all.
        // A key the rule declared with nothing usable in it keeps that, which is
        // what lets `"ioclass": null` suppress the type's value. The numbers use
        // `is_none()`, which for `Attribute<T>` means the outer `Option` is
        // `None` — the same "said nothing" test.
        macro_rules! overlay_name {
            ($($field:ident),* $(,)?) => {
                $(
                    if self.$field.is_absent() {
                        self.$field = base.$field;
                    }
                )*
            };
        }
        macro_rules! overlay_value {
            ($($field:ident),* $(,)?) => {
                $(
                    if self.$field.is_none() {
                        self.$field = base.$field.clone();
                    }
                )*
            };
        }
        overlay_value!(nice, latency_nice, ionice, rtprio, oom_score_adj);
        overlay_name!(ioclass, sched, cgroup, cpuset);
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

        if let Some(type_name) = self.type_name.as_str() {
            object.insert("type".into(), Value::String(type_name.to_string()));
        }

        // A name attribute carries its own "the key was declared" state now that
        // it is no longer an `Option` wrapped around the value. Absent
        // contributes nothing to the loop, which is what `Option::map` arranged
        // before.
        for (key, attribute) in [
            ("nice", self.nice.as_ref().map(|v| json_of_i32(*v))),
            (
                "latency_nice",
                self.latency_nice.as_ref().map(|v| json_of_i32(*v)),
            ),
            ("ioclass", if_declared(&self.ioclass).map(json_of_name)),
            ("ionice", self.ionice.as_ref().map(|v| json_of_i32(*v))),
            ("sched", if_declared(&self.sched).map(json_of_name)),
            ("rtprio", self.rtprio.as_ref().map(|v| json_of_u32(*v))),
            (
                "oom_score_adj",
                self.oom_score_adj.as_ref().map(|v| json_of_i32(*v)),
            ),
            ("cgroup", if_declared(&self.cgroup).map(json_of_name)),
            ("cpuset", if_declared(&self.cpuset).map(json_of_name)),
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

/// A declared name as the dump reports it: the value, or `null` for a key the
/// rule wrote with nothing usable in it.
///
/// The caller only reaches this for a key that was declared, so "absent" is not
/// a case here — it never got this far.
fn json_of_name(attribute: &NameAttribute) -> Value {
    attribute
        .as_str()
        .map_or(Value::Null, |name| Value::String(name.to_string()))
}

/// The attribute, if the rule file said anything at all about that key.
///
/// A name is a `NameAttribute` rather than an `Option`, and this is the
/// `Option` the old field type gave for free: `None` for a key that was absent,
/// which the loop below skips exactly as it skipped the un-wrapped case.
fn if_declared(attribute: &NameAttribute) -> Option<&NameAttribute> {
    if attribute.is_absent() {
        None
    } else {
        Some(attribute)
    }
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
/// The bytes a name has to begin with for a pattern to match it at all.
///
/// 256 bits, so a membership test is one load and a shift, and the whole set
/// is two cache lines. This is the *prefilter's* type and it exists for one
/// reason: a scan that runs every `name_regex` for every uncached process name
/// is linear in the number of regex rules, and at 100 of them the scan is the
/// entire cost of an uncached lookup.
///
/// The invariant this type has to keep is one-sided, and it is the whole
/// safety argument: **a prefilter may only ever reject a name that provably
/// cannot match.** The failure mode of getting it wrong is not a wrong answer,
/// which a test would catch — it is a rule that silently stops applying to
/// some process names, which looks identical to a rule that has stopped
/// working at all. So the set is only ever built from the one pattern shape
/// whose required prefix is unambiguous, and everything else gets no prefilter
/// and is always run. See [`anchored_first_byte`].
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct FirstByteSet([u64; 4]);

impl FirstByteSet {
    fn insert(&mut self, byte: u8) {
        self.0[usize::from(byte) / 64] |= 1 << (byte % 64);
    }

    fn union_with(&mut self, other: &Self) {
        for (mine, theirs) in self.0.iter_mut().zip(other.0.iter()) {
            *mine |= *theirs;
        }
    }

    fn contains(&self, byte: u8) -> bool {
        self.0[usize::from(byte) / 64] & (1 << (byte % 64)) != 0
    }

    /// Whether a name beginning with `byte` is still worth trying.
    fn admits(&self, byte: u8) -> bool {
        self.contains(byte)
    }
}

/// The first byte every match of `pattern` must begin with, or `None` if the
/// pattern does not say so unambiguously.
///
/// `None` is the answer for every pattern except a literal directly after a
/// leading `^`, and it is the answer that makes this safe. For `^` followed by
/// a plain character, a match can only begin at the start of the name — `regexr`
/// compiles `^` to a start-of-text anchor, not a start-of-line one, so the
/// pattern's own leading `^` cannot match after a newline in the name — and
/// that character's first UTF-8 byte is the name's first byte. That is a
/// *superset* of the possible first bytes, so testing it can never reject a
/// name that would have matched.
///
/// What is refused, and why each is a case where the answer is not a single
/// known byte:
///
/// * an unanchored pattern (`gcc-.*`) matches anywhere in the name, so its
///   first byte says nothing about the name's;
/// * a metacharacter after the `^` — a class (`^[a-z]`), a group
///   (`^(?:a|b)`), `.`, an escape, a quantifier — describes a *set* of first
///   bytes, or none at all, and extracting that set soundly is where this kind
///   of optimisation usually goes wrong. `.` in particular can match any byte
///   but one, so the set would be "everything";
/// * a quantified literal, which is the case that got this written wrong the
///   first time. `^m?` does not require an `m` — `?` allows zero occurrences —
///   so `^m?\w+ode$` matches `node`, and a prefilter built on "a match starts
///   with `m`" rejects it. Only an *unquantified* literal is a required
///   prefix, and `+` is refused alongside `?` and `*` not because it is
///   unsound but because a pattern like `^m+` is vanishingly rare next to the
///   ones this does catch, and one fewer thing to reason about is worth more
///   than it costs;
/// * a leading `(?...)` group, which is what turns multi-line mode on. With
///   `(?m)`, `^` *does* match after a newline, and a name like `"x\napp"`
///   would match `^app` — so a prefilter built on the assumption above would
///   reject a name that matches. Refusing the group is what keeps that sound;
/// * a `^` with nothing after it, which matches the empty string and so
///   constrains no byte.
///
/// Only a *leading* `^` is trusted, and a `(?m)` later in the pattern cannot
/// change it: flags take effect for the atoms parsed after them, and this
/// parser has already stopped at the second character.
fn anchored_first_byte(pattern: &str) -> Option<u8> {
    let mut chars = pattern.chars();
    if chars.next()? != '^' {
        return None;
    }
    let literal = chars.next()?;
    if matches!(
        literal,
        // Every metacharacter a bare pattern position can hold, plus `(` and
        // `)` for an inline flag group, plus the two anchors.
        '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
    ) {
        return None;
    }
    // The literal is only a required prefix if it cannot be quantified away.
    if chars
        .next()
        .is_some_and(|c| matches!(c, '?' | '*' | '+' | '{'))
    {
        return None;
    }
    // The first *byte* of the character's UTF-8 encoding, so a multi-byte
    // literal is compared the way the name's bytes actually are.
    let mut encoded = [0u8; 4];
    literal
        .encode_utf8(&mut encoded)
        .as_bytes()
        .first()
        .copied()
}

struct Matcher {
    compiled: regexr::Regex,
    /// The `name` of the program rule this pattern belongs to. Held so a failure
    /// can name the rule it came from; the pattern's own source is also kept by
    /// the engine, for the same purpose.
    ///
    /// Kept as the name rather than as the rule's index in `rules`: the failure
    /// report below needs the name, and the index would have to be looked back up
    /// to produce it. A rule set carries a handful of `name_regex` rules, so
    /// there are a handful of these.
    name: String,
    /// The bytes this pattern's matches must begin with, or `None` for no
    /// prefilter.
    ///
    /// `None` is the common case and is not a pessimisation to be fixed: a
    /// prefilter that guesses is worse than none at all, for the reason
    /// [`anchored_first_byte`] spends most of its comment on.
    first_bytes: Option<FirstByteSet>,
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
            first_bytes: anchored_first_byte(pattern).map(|byte| {
                let mut set = FirstByteSet::default();
                set.insert(byte);
                set
            }),
        })
    }

    /// Whether `name` is worth handing to the engine at all.
    ///
    /// A name with no bytes cannot be shown to be impossible by a required
    /// first byte, and neither can anything else this knows nothing about, so
    /// both are passed through: the engine is the thing that decides, and a
    /// prefilter's only power is to skip it when the answer is already known.
    fn may_match(&self, name: &str) -> bool {
        match (self.first_bytes, name.as_bytes().first()) {
            (Some(bytes), Some(first)) => bytes.admits(*first),
            (Some(_), None) | (None, _) => true,
        }
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
    /// Program rule name to its index in `rules`.
    ///
    /// The rules themselves are in a `Vec`, not in here, and that is the whole
    /// point. A `Rule` is 176 bytes, so storing them by value in this map made
    /// every bucket 200 bytes — and a hash map's bucket count is the next power
    /// of two above `count * 8/7`, which turns a per-rule cost into a staircase.
    /// At 15,831 rules the table is 6.5 MB; at 28,673 it becomes 13.1 MB, so
    /// *one extra rule* cost 10.7 MB. A `u32` index makes the bucket 28 bytes, the
    /// same steps cost 0.9 MB, and the rules grow continuously in the `Vec`.
    programs: HashMap<RuleName, u32>,
    /// The program rules, in the order they were read.
    ///
    /// Contiguous so that adding a rule does not have to grow a table, and read
    /// by index because `programs` already answered with one.
    rules: Vec<Rule>,
    /// Type definitions, as the JSON they were written as. There are a couple of
    /// dozen of these, so what they cost does not matter, and keeping them as
    /// written means `dump types` is a faithful report of the files.
    types: HashMap<TypeName, Arc<Value>>,
    cgroups: HashMap<CgroupName, Arc<Value>>,
    // Store fallback regex rules if enabled, in rule-file load order
    regex_programs: Vec<Matcher>,
    /// Every first byte any prefiltered `name_regex` can match at, or `None` if
    /// any one of them has no prefilter.
    ///
    /// This is the union, so one test rejects the whole scan for the names that
    /// match nothing at all — which, on a rule set with a community's worth of
    /// `name_regex` rules and a process population that mostly matches none of
    /// them, is the case that decides the cost. `None` once any pattern cannot
    /// be reasoned about, and it never recovers for the life of the list,
    /// because adding a pattern can only ever make the union less selective.
    regex_first_bytes: Option<FirstByteSet>,
    // Which rule a process name resolved to, so the regex scan below runs once
    // per distinct name rather than once per process.
    //
    // The value is the rule's index and nothing else. It used to be an
    // `Option<RuleName>`, and that cost an allocation per miss to clone a name
    // out of `programs` which `resolve` then had to look up again to get back
    // the `&RuleName` it started from. An index is four bytes, is `Copy`, and
    // turns `resolve` into a bounds-checked `Vec` index.
    //
    // The name itself is deliberately *not* here. It cannot be: a borrow of
    // `programs` cannot be stored in a field of the same `Rules`, and
    // interning every rule name is the change `NameTable` would have to be
    // widened for — a `u16` table already at its cap. `get_rule_with_name`,
    // which is the only caller that needs it, does the lookup instead; it is
    // the `dump` path and is not hot.
    //
    // The key is a `Box<str>` rather than a `String`. A cache entry's contents
    // never change once written, and a `String` spends eight bytes per entry on
    // a capacity only `push` would use, plus whatever the allocator's size
    // class rounded the copy up to. Over 5 000 entries that is a table of
    // slack, in a map whose entries are never grown.
    resolved_cache: Mutex<lru::LruCache<Box<str>, Option<u32>>>,
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
            rules: Vec::new(),
            types: HashMap::new(),
            cgroups: HashMap::new(),
            regex_programs: Vec::new(),
            regex_first_bytes: Some(FirstByteSet::default()),
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
        self.rules.clear();
        self.types.clear();
        self.cgroups.clear();
        self.regex_programs.clear();
        // The union goes back to empty rather than to "no prefilter": the list
        // it summarises is empty, so it constrains nothing, and a later load
        // that only adds prefilterable patterns should get one.
        self.regex_first_bytes = Some(FirstByteSet::default());
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
    /// Each type is resolved once, not once per rule that names it. `from_json`
    /// allocates a fresh `Arc<str>` for every name it reads, so re-parsing the
    /// type inside the loop gave every rule a private copy of a string the whole
    /// set shares: measured, that was two extra 32-byte chunks per rule —
    /// 31,660 allocations and 760 kB across a 15,831-rule set, for the two
    /// distinct values its types declare. Resolving the types up front makes the
    /// `Arc` do the job it is there for — one allocation per distinct value,
    /// shared by every rule that inherits it.
    /// `rules_inheriting_one_type_share_its_names` and
    /// `inheritance_does_not_mutate_the_shared_type` are what hold both halves
    /// of that: that it shares, and that sharing does not become aliasing.
    ///
    /// Types are read rather than taken: a program rule must not be able to
    /// rewrite the type it inherits from, and two rules sharing a type must not
    /// be able to see each other.
    fn precompute_inheritance(&mut self) {
        // Destructured rather than reaching through `self` for both: `rules` is
        // borrowed mutably and `types` immutably, and there is no way to ask `self`
        // for both at once.
        let Rules { rules, types, .. } = self;

        // One resolved rule per type, keyed by the name the rules refer to it
        // by. Borrowed from `types` for the length of this function and dropped
        // with it, which leaves each surviving `Arc` referenced only by the
        // rules that inherited it.
        let bases: HashMap<&str, Rule> = types
            .iter()
            .map(|(name, value)| {
                let mut base = Rule::from_json(value);
                // The type's own `type` key is not inherited: the rule reports
                // the type it named, and a type cannot itself inherit from a
                // type.
                base.type_name = TypeNameId::default();
                (name.as_ref(), base)
            })
            .collect();

        for rule in rules.iter_mut() {
            // Borrowed, not cloned: the key is the rule's own `type_name`, and
            // building a `TypeName` to look it up would allocate once per rule
            // for a lookup that answers with the type it already names.
            let Some(type_name) = rule.type_name.as_str() else {
                continue;
            };
            let Some(base) = bases.get(type_name) else {
                continue;
            };
            rule.inherit(base);
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
                    // One lookup, not two. `contains_key` asked the map the
                    // question the redefinition note needs and then `get` asked
                    // it again for the index; hashing the same name twice per
                    // definition was a load-time cost paid N times for an answer
                    // the first lookup already had.
                    let previous = self.programs.get(&key).copied();
                    Self::note_redefinition(previous.is_some(), "rule", name, source);

                    // Overwrite in place when the name is already there, rather
                    // than pushing. Appending would leave the superseded rule in
                    // `rules` with nothing pointing at it: it would still cost
                    // memory, and `dump rules` — which walks `rules` — would
                    // print every definition a rule file ever made of that name
                    // rather than the one that applies. A rule file overriding an
                    // earlier one is the documented way to change a shipped rule,
                    // so this path is not hypothetical.
                    let index = match previous {
                        Some(index) => index,
                        None => {
                            // Reserved and filled in below rather than pushed
                            // with its value, so the rule is built once.
                            self.rules.push(Rule::default());
                            (self.rules.len() - 1) as u32
                        }
                    };
                    if let Some(slot) = self.rules.get_mut(index as usize) {
                        *slot = Rule::from_json(&value);
                    }
                    self.programs.insert(key, index);

                    if let Some(regex_str) = value.get("name_regex").and_then(|v| v.as_str()) {
                        match Matcher::compile(regex_str, name) {
                            Ok(matcher) => {
                                // Widening the union is four `u64` ors, or —
                                // once any pattern has refused a prefilter —
                                // nothing for the rest of this list's life. A
                                // pattern that compiles but cannot be reasoned
                                // about does not cost itself its prefilter, it
                                // costs the whole union, deliberately.
                                self.regex_first_bytes =
                                    match (self.regex_first_bytes, matcher.first_bytes) {
                                        (Some(union), Some(set)) => {
                                            let mut next = union;
                                            next.union_with(&set);
                                            Some(next)
                                        }
                                        (Some(_), None) | (None, _) => None,
                                    };
                                self.regex_programs.push(matcher);
                            }
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

    /// The rule a process of this name gets.
    ///
    /// Just the rule. The name it was *declared* under is a second thing a
    /// caller might want, and only one caller does — see
    /// [`Rules::get_rule_with_name`]. Carrying it here cost a `String` on every
    /// cache miss, and a second hash in `resolve` to turn that name back into
    /// the `&RuleName` the lookup had started from.
    ///
    /// A borrow rather than a copy — nothing here is mutated while a worker is
    /// reading it, and a reference costs no allocation.
    pub fn get_rule(&self, name: &str) -> Option<&Rule> {
        // 0. Check cache
        if let Ok(mut cache) = self.resolved_cache.lock()
            && let Some(cached) = cache.get(name)
        {
            return self.resolve(*cached);
        }

        let best_match = self.find_best_match(name);

        // The cache is keyed on the process name, and a process name is the
        // basename of `argv[0]` — which the kernel puts no upper bound on. A
        // single `execve` with a 128 KiB `argv[0]` therefore produces a 128 KiB
        // key, and 5000 of them is half a gigabyte, against a `MemoryMax=96M` in
        // the shipped unit. A name that long is not a program name and no rule
        // can match it usefully, so it is answered without being remembered. The
        // lookup itself still happens, so the answer is the same either way.
        //
        // `Box::from` is the one allocation left on this path, and it is the key:
        // it has to be owned, because the cache outlives the caller's `&str`. It
        // is exact — no capacity, and no slack for a `String` to round up.
        if name.len() <= MAX_CACHEABLE_NAME
            && let Ok(mut cache) = self.resolved_cache.lock()
        {
            cache.put(Box::from(name), best_match.map(|(_, index)| index));
        }

        self.resolve(best_match.map(|(_, index)| index))
    }

    /// The rule a process of this name gets, *and* the name it was declared
    /// under.
    ///
    /// Both, because they can differ: a `name_regex` rule is declared under one
    /// name and matches others, and `dump proc` reports the rule a process
    /// matched, not the name the process was called.
    ///
    /// Deliberately not memoised. The cache stores an index because that is all
    /// [`Rules::get_rule`] needs, and this path is the one caller that needs the
    /// name back — so it takes the match directly rather than paying a lookup
    /// to reconstruct something the cache no longer holds. It is a diagnostic
    /// path: `dump` runs it once per process in a full `/proc` walk, and the
    /// worker never calls it.
    pub fn get_rule_with_name(&self, name: &str) -> Option<(&RuleName, &Rule)> {
        let (rule_name, index) = self.find_best_match(name)?;
        Some((rule_name, self.resolve(Some(index))?))
    }

    /// The rule at a cached index.
    ///
    /// The index is bounds-checked rather than assumed: `programs` and `rules` are
    /// two structures, and the only thing keeping them in step is that they are
    /// written in one place. A rule that is never indexed by anything is harmless,
    /// so a stale index is not worth a panic in the middle of a `/proc` walk.
    fn resolve(&self, index: Option<u32>) -> Option<&Rule> {
        self.rules.get(index? as usize)
    }

    /// The rule a name matches, as the name it is declared under and the index
    /// of the rule. Both are borrowed from `programs`, where the exact match
    /// matched.
    ///
    /// It used to return an owned `RuleName`, which meant cloning the name out
    /// of the map on every miss — and then `resolve` looked the same name up
    /// again to get back the `&RuleName` it had started from. Three hashes and
    /// two allocations where one hash and none are enough.
    fn find_best_match(&self, target_name: &str) -> Option<(&RuleName, u32)> {
        // 1. Exact match
        if let Some((name, &index)) = self.programs.get_key_value(target_name) {
            return Some((name, index));
        }

        // 2. Regex fallback, in rule-file load order, so the first pattern that
        // matches wins exactly as it does in the reference.
        //
        // Two prefilters, both of which can only ever skip the engine for a name
        // that provably does not match. The first is one test for the whole scan
        // and answers "could any pattern match this name at all", which is the
        // question for a name that matches nothing. The second is per pattern,
        // and answers "could this one", for the name that got past the first.
        if let Some(union) = &self.regex_first_bytes
            && target_name
                .as_bytes()
                .first()
                .is_some_and(|first| !union.admits(*first))
        {
            return None;
        }

        for matcher in &self.regex_programs {
            if matcher.may_match(target_name)
                && matcher.is_match(target_name)
                && let Some((name, &index)) = self.programs.get_key_value(matcher.name.as_str())
            {
                return Some((name, index));
            }
        }

        None
    }

    pub fn size(&self) -> usize {
        self.rules.len()
    }

    pub fn get_cgroups(&self) -> &HashMap<CgroupName, Arc<Value>> {
        &self.cgroups
    }

    /// Every program rule, with the name it is declared under.
    ///
    /// An iterator rather than the map, because the map holds indices now. It
    /// walks `programs` and follows each index into `rules`, so it is one pass
    /// and not one pass per rule; the order is the map's, which is unordered —
    /// `dump rules` sorts what it gets, so nothing depends on it.
    pub fn iter_rules(&self) -> impl Iterator<Item = (&RuleName, &Rule)> {
        self.programs
            .iter()
            .filter_map(|(name, index)| self.rules.get(*index as usize).map(|rule| (name, rule)))
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
    /// and the kernel reclaimed the daemon's own pages continuously.
    ///
    /// This is the size that has to stay under a few hundred bytes. It does not
    /// pin the total, because where these bytes live matters as much as how many
    /// there are — see the type's documentation, and `docs/CONFIGURATION.md`
    /// § Memory.
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

    /// The exact size `docs/MEMORY.md` reasons about, pinned so a change to the
    /// layout is a deliberate one rather than something the budget notices.
    ///
    /// The ceiling above is a guard rail; this is the number. It was 176 while
    /// the four names were `Option<Option<Arc<str>>>` at 24 bytes each, which
    /// is 96 of the struct for a few dozen distinct values — the inner `Option`
    /// spends the null-pointer niche, so the outer one needs a discriminant of
    /// its own. They are [`NameAttribute`]s now, two bytes each, packed into
    /// eight alongside the numbers, and `type_name` is a [`TypeNameId`] rather
    /// than an owned `Box<str>`.
    ///
    /// 72 is five `Option<Option<i32>>` (40) + four `NameAttribute` (8) +
    /// `TypeNameId` (2) + `Box<[(Box<str>, Value)]>` (16), plus padding.
    #[test]
    fn a_rule_is_the_size_the_memory_budget_assumes() {
        assert_eq!(
            std::mem::size_of::<Rule>(),
            72,
            "docs/MEMORY.md budgets this per rule; if it changed, that \
             document's numbers and any plan built on them need re-deriving"
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

            let rule = rules.get_rule("x").expect("the rule is there");
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

        let rule = rules.get_rule("x").expect("the rule is there");
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

    /// Two rules that happen to carry the same value can share one allocation.
    ///
    /// This is about the type's ability, not the loader's behaviour: the two
    /// rules here are built by hand, because what the loader does is a separate
    /// question with its own tests below — `rules_inheriting_one_type_share_its_names`
    /// and `a_cgroup_name_inherited_from_a_type_is_shared_too`, which go through
    /// `precompute_inheritance` and are what would fail if a rule set stopped
    /// sharing.
    #[test]
    fn a_shared_name_is_one_allocation() {
        let first = parse(r#"{"name":"x","sched":"batch"}"#);
        let second = parse(r#"{"name":"y","sched":"batch"}"#);
        let different = parse(r#"{"name":"z","sched":"idle"}"#);

        assert_eq!(first.sched, second.sched, "one name, one index");
        assert_ne!(first.sched, different.sched, "and different names differ");
        assert_eq!(first.sched(), Some("batch"));
    }

    /// The sharing, reached the way a rule set actually reaches it.
    ///
    /// What a rule set does is inherit every name from a `.types` file, so this
    /// is the path that has to share: `precompute_inheritance` resolves each
    /// rule's type and the rule ends up holding the same index into
    /// [`NAME_TABLE`] as every other rule that inherited the same value.
    ///
    /// Three hundred rules naming one type is a fifth of a default rule set, and
    /// a type that declares an `ioclass` and a `sched` means two shared names
    /// per rule. Before the names were interned this was two 32-byte chunks per
    /// rule of pure duplication — 760 kB across a 15,831-rule set, and
    /// invisible in peak RSS. `docs/MEMORY.md` § What the names cost has the
    /// figures.
    #[test]
    fn rules_inheriting_one_type_share_its_names() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        rules.load_rule_from_string(
            r#"{"type":"BG","ioclass":"idle","sched":"idle","cgroup":"background"}"#,
        );
        for i in 0..300 {
            rules.load_rule_from_string(&format!(r#"{{"name":"p{i}","type":"BG"}}"#));
        }
        rules.precompute_inheritance();

        let first = rules.get_rule("p0").expect("p0 has a rule");
        let first_sched = first.sched;
        let first_ioclass = first.ioclass;
        let first_cgroup = first.cgroup;
        assert_eq!(first.sched(), Some("idle"), "and the value is still 'idle'");

        for i in 1..300 {
            let other = rules
                .get_rule(&format!("p{i}"))
                .expect("every rule resolves");
            assert_eq!(
                other.sched, first_sched,
                "rule p{i} inherited the same sched and must hold the same index"
            );
            assert_eq!(other.ioclass, first_ioclass, "rule p{i}, ioclass");
            assert_eq!(other.cgroup, first_cgroup, "rule p{i}, cgroup");
        }
    }

    /// The same for the cgroup name, which is a third field on the same type and
    /// would be missed by a test that only looked at `sched`.
    #[test]
    fn a_cgroup_name_inherited_from_a_type_is_shared_too() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        rules.load_rule_from_string(r#"{"type":"BG","cgroup":"background"}"#);
        for i in 0..50 {
            rules.load_rule_from_string(&format!(r#"{{"name":"q{i}","type":"BG"}}"#));
        }
        rules.precompute_inheritance();

        let first = rules.get_rule("q0").expect("q0 has a rule");
        let shared = first.cgroup;
        for i in 1..50 {
            let other = rules
                .get_rule(&format!("q{i}"))
                .expect("every rule resolves");
            assert_eq!(
                other.cgroup, shared,
                "rule q{i} holds the same cgroup index"
            );
        }
    }

    /// A rule that overrides the type's name keeps its own, and the rest still
    /// share the type's. Sharing must not become aliasing: two rules naming
    /// different values are two allocations, or a mutation of one would be seen
    /// by the other.
    #[test]
    fn an_overridden_name_is_not_aliased_to_the_types() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        rules.load_rule_from_string(r#"{"type":"BG","ioclass":"idle"}"#);
        rules.load_rule_from_string(r#"{"name":"a","type":"BG"}"#);
        rules.load_rule_from_string(r#"{"name":"b","type":"BG","ioclass":"best-effort"}"#);
        rules.precompute_inheritance();

        let a = rules.get_rule("a").expect("a has a rule");
        let b = rules.get_rule("b").expect("b has a rule");
        assert_eq!(a.ioclass(), Some("idle"));
        assert_eq!(b.ioclass(), Some("best-effort"));
        assert_ne!(
            a.ioclass, b.ioclass,
            "an override is its own name, not an alias of the type's"
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

    /// The rules live in a `Vec` and the map holds indices into it, so a name
    /// defined twice has to overwrite the rule already at its index rather than
    /// append. Appending would leave the superseded rule in the `Vec` with
    /// nothing pointing at it: still costing memory, and printed by `dump rules`
    /// as if it were a rule in force. A rule file overriding an earlier one is
    /// the documented way to change a shipped rule, and the default rule set does
    /// it three times.
    #[test]
    fn a_redefined_rule_replaces_rather_than_accumulates() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        rules.load_rule_from_string(r#"{"name":"x","nice":1}"#);
        rules.load_rule_from_string(r#"{"name":"y","nice":2}"#);
        assert!(rules.load_rule_from_string(r#"{"name":"x","nice":9}"#));

        assert_eq!(rules.size(), 2, "three definitions, two names");
        assert_eq!(
            rules.get_rule("x").expect("x is there").nice(),
            Some(9),
            "the last definition is the one in force"
        );
        assert_eq!(rules.get_rule("y").expect("y is there").nice(), Some(2));

        // And nothing superseded is left behind for a report to walk into.
        // Compared unordered, because the iterator walks a map.
        let mut reported: Vec<_> = rules
            .iter_rules()
            .map(|(name, rule)| (name.as_ref().to_string(), rule.nice()))
            .collect();
        reported.sort();
        assert_eq!(
            reported,
            vec![("x".to_string(), Some(9)), ("y".to_string(), Some(2))],
            "one entry per name, whatever the files said"
        );
    }

    /// The two structures have to agree, which is the one invariant this layout
    /// introduces. Every index the map holds has to name a rule that exists, and
    /// every rule has to be named by some index.
    #[test]
    fn every_rule_is_reachable_and_every_index_resolves() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        for i in 0..500 {
            assert!(rules.load_rule_from_string(&format!(r#"{{"name":"p{i}"}}"#)));
        }
        // Redefine a third of them, so some indices are reused.
        for i in 0..500 {
            if i % 3 == 0 {
                assert!(rules.load_rule_from_string(&format!(r#"{{"name":"p{i}","nice":5}}"#)));
            }
        }

        assert_eq!(rules.size(), 500, "redefinitions do not add rules");
        for i in 0..500 {
            // `get_rule_with_name` rather than `get_rule`: the assertion is
            // about the name a rule resolves to, which is the thing the hot
            // path no longer carries.
            let (name, rule) = rules
                .get_rule_with_name(&format!("p{i}"))
                .expect("every rule resolves");
            assert_eq!(name.as_ref(), format!("p{i}"), "and to its own name");
            assert_eq!(rule.nice(), (i % 3 == 0).then_some(5));
        }
        assert_eq!(
            rules.iter_rules().count(),
            500,
            "and nothing in the vector is unreachable"
        );
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
            rule.nice(),
            Some(9),
            "the reloaded value must be what is read, not the cached earlier one"
        );
    }
}

#[cfg(test)]
mod prefilter {
    use {super::*, crate::config::ConfigSnapshot};

    /// The whole safety argument in one test: for every pattern that gets a
    /// prefilter, and every name the corpus says it must match, the prefilter
    /// must not reject the name.
    ///
    /// `the_whole_ananicy_name_regex_corpus_still_matches` in
    /// `tests/rules.rs` already catches this end-to-end, and it caught the
    /// first version of this — `^m?\w+ode$` was prefilted on `m`, and `node`
    /// stopped matching. This states the invariant directly rather than relying
    /// on a distant test happening to cover a given shape.
    #[test]
    fn a_prefilter_never_rejects_a_name_the_pattern_matches() {
        let corpus: &[(&str, &[&str])] = &[
            (r"^java[0-9.]*$", &["java", "java17", "java.1.0"]),
            (r"^app.*$", &["app", "app-helper", "appother", "app\n"]),
            (r"^bash(?!_script)", &["bash", "bashx", "bash-", "bash_"]),
            (r"^Steam.*$", &["Steam", "Steam.exe"]),
            (
                r"^firefox(-bin|-esr)?$",
                &["firefox", "firefox-bin", "firefox-esr"],
            ),
            (
                r"^VirtualBox(Headless)?VM$",
                &["VirtualBoxVM", "VirtualBoxHeadlessVM"],
            ),
            (r"^sshd(-session)?$", &["sshd", "sshd-session"]),
            (
                r"^kworker/[0-9]+-[0-9]+$",
                &["kworker/0-1", "kworker/12-345"],
            ),
            // The two that a first attempt got wrong, kept here so the shape
            // that broke stays covered: a quantified leading literal, and a
            // pattern whose only match does not begin with its first atom.
            (r"^m?\w+ode$", &["node", "mode", "mnode"]),
            (r"^(java|javaw)[0-9.]*$", &["java", "javaw", "javaw17"]),
            (r"^.*-wrapped$", &[".foo-wrapped", "-wrapped"]),
            (r"^.*\.exe$", &["setup.exe", ".exe"]),
            (r"gcc-.*", &["gcc-aarch64", "/usr/bin/gcc-aarch64"]),
        ];

        for (pattern, matches) in corpus {
            let matcher = Matcher::compile(pattern, "probe")
                .unwrap_or_else(|e| panic!("{pattern:?} should compile: {e}"));
            for name in *matches {
                assert!(
                    matcher.may_match(name),
                    "{pattern:?} matches {name:?}, so its prefilter must admit \
                     it — a prefilter that rejects a matching name makes the \
                     rule silently stop applying"
                );
            }
        }
    }

    /// The shapes that must *not* get a prefilter, each with the reason.
    ///
    /// Asserting the absence is the other half of the safety argument: a
    /// future change that widens the extraction has to come here and justify
    /// each of these, rather than quietly making the union a guess.
    #[test]
    fn only_an_unquantified_literal_after_a_leading_anchor_is_prefilterable() {
        let prefilterable: &[(&str, Option<u8>)] = &[
            (r"^java[0-9.]*$", Some(b'j')),
            (r"^app.*$", Some(b'a')),
            (r"^Steam.*$", Some(b'S')),
            (r"^sshd(-session)?$", Some(b's')),
            // Unquantified, so the first byte really is required.
            (r"^m\w+ode$", Some(b'm')),
            // A multi-byte literal is compared as the first byte it encodes.
            (r"^école$", Some(0xc3)),
        ];
        for (pattern, expected) in prefilterable {
            assert_eq!(
                anchored_first_byte(pattern),
                *expected,
                "{pattern:?} should yield a required first byte"
            );
        }

        let refused: &[&str] = &[
            // Unanchored: matches anywhere, so the name's first byte is
            // unrelated to the pattern's.
            r"gcc-.*",
            // A set of first bytes, not one.
            r"^[a-z]+",
            r"^(?:gimp|inkscape)$",
            r"^.",
            r"^\\w+ode",
            // A quantified literal is not a required prefix. `^m?` allows zero
            // occurrences, so `node` matches `^m?\w+ode$` and a prefilter on
            // `m` rejects it. This is the case the corpus test caught.
            r"^m?\w+ode$",
            r"^a*",
            r"^a{2}",
            // Multi-line mode would make `^` match after a newline in the name,
            // so `^app` could match `"x\napp"`. Refusing the group is what
            // keeps the anchor meaning start-of-text.
            r"(?m)^app.*$",
            r"(?i)^app.*$",
            // Nothing to constrain: an empty pattern, or one that matches only
            // the empty string.
            r"",
            r"^",
        ];
        for pattern in refused {
            assert_eq!(
                anchored_first_byte(pattern),
                None,
                "{pattern:?} must not be prefiltered: the engine decides, not \
                 this"
            );
        }
    }

    /// The whole-scan prefilter is the union over every pattern, and it is
    /// given up entirely the moment one pattern cannot be reasoned about.
    ///
    /// The give-up is not a pessimisation to be fixed later: a union computed
    /// over an unknown pattern's first bytes is a set that can be too narrow,
    /// which is the one direction this is not allowed to be wrong in.
    #[test]
    fn the_union_is_given_up_when_any_pattern_cannot_be_prefiltered() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));

        // Every pattern is prefilterable, so the union exists.
        assert!(rules.load_rule_from_string(r#"{"name":"a","name_regex":"^apple.*"}"#));
        assert!(rules.load_rule_from_string(r#"{"name":"b","name_regex":"^banana"}"#));
        assert!(
            rules.regex_first_bytes.is_some(),
            "both patterns are anchored literals, so the union survives"
        );

        // A name starting with neither is rejected by the union alone.
        assert!(rules.get_rule("cherry").is_none());

        // One unanchorable pattern takes the whole union down with it.
        assert!(rules.load_rule_from_string(r#"{"name":"c","name_regex":"gcc-.*"}"#));
        assert!(
            rules.regex_first_bytes.is_none(),
            "an unanchored pattern makes every other pattern's prefilter \
             unusable, so the union is abandoned"
        );
        // And the scan still works, which is the point of giving it up rather
        // than computing a narrower one.
        assert!(rules.get_rule("apple-pie").is_some());
        assert!(rules.get_rule("gcc-aarch64").is_some());
    }

    /// A second load that only clears the list gets its union back.
    ///
    /// The union is the one piece of state derived from `regex_programs`, and
    /// `load_directory` clears that list in place. If the union were not reset
    /// with it, a reload would keep rejecting names that the new rule set is
    /// perfectly willing to match.
    #[test]
    fn a_reload_starts_the_union_over_again() {
        let mut rules = Rules::new(Arc::new(Config::new(ConfigSnapshot::default())));
        assert!(rules.load_rule_from_string(r#"{"name":"a","name_regex":"gcc-.*"}"#));
        assert!(rules.regex_first_bytes.is_none(), "gcc-* is unanchorable");

        let (dir, path) = {
            let d = tempfile::tempdir().expect("a rules directory");
            std::fs::write(
                d.path().join("a.rules"),
                "{\"name\": \"kworker\", \"name_regex\": \"^kworker/[0-9]+$\"}\n",
            )
            .expect("a rule file");
            let p = d.path().to_path_buf();
            (d, p)
        };
        rules.load_directory(&path);

        assert!(
            rules.regex_first_bytes.is_some(),
            "the unanchorable pattern is gone with the rest of the old set"
        );
        assert!(
            rules.get_rule("kworker/0").is_some(),
            "and a name the new set matches is not rejected by a stale union"
        );
        assert!(
            rules.get_rule("gcc-aarch64").is_none(),
            "the old rule is gone"
        );

        drop(dir);
    }
}
