//! Rule, type and cgroup loading and matching.
//!
//! A rule file is a stream of JSON objects, one per line, optionally followed by
//! a `#` comment. The classification of a line decides where it is stored:
//!
//! * a line with a `name` becomes a *program* rule,
//! * a line with a `type` but no `name` becomes a *type*,
//! * a line with a `cgroup` but neither of the above becomes a *cgroup* entry.
//!
//! The `name_regex` extension and the `#`-comment / CRLF tolerance exist for
//! compatibility with the rule files shipped by the `ananicy` and `ananicy-cpp`
//! communities; see `docs/COMPATIBILITY.md`.

use {
    ananicy_core::{
        config::{Config, ConfigSnapshot},
        rules::Rules,
        types::TypeName,
    },
    std::{fs, path::PathBuf, sync::Arc},
};

fn config() -> Arc<Config> {
    Arc::new(Config::new(ConfigSnapshot::default()))
}

fn rules() -> Rules {
    Rules::new(config())
}

/// Creates a rules directory containing `entries` and returns the owner plus the
/// path, so the directory stays alive for as long as the caller needs it.
fn rules_dir(entries: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    for (name, content) in entries {
        fs::write(directory.path().join(name), content).unwrap();
    }
    let path = directory.path().to_path_buf();
    (directory, path)
}

fn rule_of(rules: &Rules, name: &str) -> serde_json::Value {
    rules
        .get_rule(name)
        .unwrap_or_else(|| panic!("no rule for {name}"))
        .as_ref()
        .clone()
}

#[test]
fn types_and_programs_are_stored_separately() {
    let mut rules = rules();
    assert_eq!(rules.size(), 0);

    assert!(
        rules.load_rule_from_string(r#"{ "type": "Doc-View", "nice": -4, "latency_nice": 5 }"#)
    );
    assert_eq!(rules.get_types().len(), 1);
    assert_eq!(rules.size(), 0, "a type is not a program rule");

    assert!(rules.load_rule_from_string(r#"{ "name": "icecat", "type": "Doc-View" }"#));
    assert_eq!(rules.size(), 1);
    assert_eq!(rule_of(&rules, "icecat")["name"], "icecat");

    assert!(rules.load_rule_from_string(r#"{ "name": "mpd", "type": "Player-Audio" }"#));
    assert_eq!(rules.size(), 2);
    assert!(rules.get_rule("mpd").is_some());
    assert!(rules.get_rule("nonesuch").is_none());
}

#[test]
fn cgroup_entries_are_stored_as_cgroups() {
    let mut rules = rules();

    assert!(rules.load_rule_from_string(r#"{ "cgroup": "lowlatency", "nice": -5 }"#));
    assert_eq!(rules.get_cgroups().len(), 1);
    assert_eq!(rules.size(), 0, "a cgroup entry is not a program rule");
    assert_eq!(rules.get_cgroups().keys().next().unwrap().0, "lowlatency");
}

#[test]
fn a_name_wins_over_a_type_or_cgroup_on_the_same_line() {
    let mut rules = rules();

    assert!(
        rules.load_rule_from_string(r#"{ "name": "app", "type": "Doc-View", "cgroup": "low" }"#)
    );
    assert_eq!(rules.size(), 1);
    assert_eq!(rules.get_types().len(), 0);
    assert_eq!(rules.get_cgroups().len(), 0);
}

#[test]
fn a_trailing_comment_after_a_rule_is_ignored() {
    let mut rules = rules();

    assert!(rules.load_rule_from_string(r#"{ "name": "someprogram", "type":"Dc-Vw" } # hey"#));
    assert_eq!(rule_of(&rules, "someprogram")["name"], "someprogram");
}

#[test]
fn surrounding_whitespace_is_trimmed() {
    let mut rules = rules();

    assert!(rules.load_rule_from_string(
        "      \t\t   { \"name\": \"someprogram2\", \"type\":\"Dc-Vw\" }         \t\t  "
    ));
    assert_eq!(rules.size(), 1);
}

#[test]
fn blank_and_comment_lines_are_skipped() {
    let mut rules = rules();

    assert!(!rules.load_rule_from_string("      "));
    assert!(!rules.load_rule_from_string("\t\t\t"));
    assert!(!rules.load_rule_from_string("  \t  \t  "));
    assert!(!rules.load_rule_from_string(""));
    assert!(!rules.load_rule_from_string("\r"));
    assert!(!rules.load_rule_from_string("   \r"));
    assert!(!rules.load_rule_from_string("# this is a comment\r"));
    assert!(!rules.load_rule_from_string("   # indented comment"));
    assert_eq!(rules.size(), 0);
}

#[test]
fn lines_without_a_complete_json_object_are_rejected() {
    let mut rules = rules();
    rules.load_rule_from_string(r#"{ "name": "sentinel", "nice": 0 }"#);
    let before = rules.size();

    // No `name`, `type` or `cgroup` field at all.
    assert!(!rules.load_rule_from_string(r#"{ "nm": "icct", "tp":"Dc-Vw" }"#));
    // Comment before the rule.
    assert!(!rules.load_rule_from_string(r#"# { "nm": "icct", "tp":"Dc-Vw" }"#));
    // Missing closing brace.
    assert!(!rules.load_rule_from_string(r#"{ "name": "icct", "type":"Dc-Vw" "#));
    // Missing opening brace.
    assert!(!rules.load_rule_from_string(r#""name": "icct", "type":"Dc-Vw" }"#));
    // Missing both braces.
    assert!(!rules.load_rule_from_string(r#""name": "icct", "type":"Dc-Vw" "#));
    // Not JSON at all, and no braces.
    assert!(!rules.load_rule_from_string("not json"));
    assert!(!rules.load_rule_from_string("{ not json }"));
    // A field of the wrong type is not a name.
    assert!(!rules.load_rule_from_string(r#"{ "name": 42 }"#));

    assert_eq!(
        rules.size(),
        before,
        "a rejected line never changes the state"
    );
}

#[test]
fn a_carriage_return_after_the_rule_is_tolerated() {
    let mut rules = rules();

    // `\r` after `}` is what reading a CRLF file line by line used to produce.
    assert!(rules.load_rule_from_string("{ \"name\": \"crlftest\", \"type\": \"Game\" }\r"));
    assert!(rules.get_rule("crlftest").is_some());
    assert_eq!(rules.size(), 1);
}

#[test]
fn nested_json_objects_are_tolerated() {
    let mut rules = rules();

    assert!(rules.load_rule_from_string(
        r#"{ "name": "nested", "type": "Game", "extra": { "key": "val" } }"#
    ));
    assert_eq!(rule_of(&rules, "nested")["extra"]["key"], "val");
}

#[test]
fn a_crlf_rule_file_is_loaded_from_a_directory() {
    let mut rules = rules();
    let (_directory, path) = rules_dir(&[(
        "crlf.rules",
        "# comment line\r\n\
         \r\n\
         { \"type\": \"TestType\", \"nice\": -1 }\r\n\
            \r\n\
         { \"name\": \"crlfprog\", \"type\": \"TestType\" }\r\n",
    )]);

    rules.load_directory(&path);

    assert!(rules.get_rule("crlfprog").is_some());
    assert_eq!(rules.get_types().len(), 1);
    assert_eq!(rules.size(), 1);
    assert_eq!(
        rule_of(&rules, "crlfprog")["nice"],
        -1,
        "the CRLF type line was parsed as a type, not as garbage"
    );
}

#[test]
fn a_missing_rule_file_or_directory_is_a_no_op() {
    let mut rules = rules();
    let (directory, path) = rules_dir(&[]);

    rules.load_file(path.join("absent.rules"));
    rules.load_directory(path.join("absent"));

    assert_eq!(rules.size(), 0);
    assert_eq!(rules.get_types().len(), 0);
    assert_eq!(rules.get_cgroups().len(), 0);
    drop(directory);
}

#[test]
fn an_unreadable_rules_directory_is_a_no_op() {
    let mut rules = rules();
    let file = tempfile::NamedTempFile::new().unwrap();

    // A regular file where a directory is expected must not abort or panic.
    rules.load_directory(file.path());

    assert_eq!(rules.size(), 0);
}

#[test]
fn name_regex_rules_match_by_pattern() {
    let mut rules = rules();

    assert!(rules.load_rule_from_string(
        r#"{ "name": "java", "name_regex": "^(java|javaw)[0-9.]*$", "nice": 3 }"#
    ));

    assert!(
        rules.get_rule("java").is_some(),
        "the rule name itself matches"
    );
    assert!(rules.get_rule("java17").is_some());
    assert!(rules.get_rule("javaw").is_some());
    assert!(rules.get_rule("notjava").is_none());
}

#[test]
fn name_regex_supports_lookaround() {
    // The engine is `regexr` rather than the standard `regex` crate precisely
    // so that lookarounds in existing community rules keep working. A negative
    // lookahead is the one advanced construct any ananicy rule has ever used,
    // and it is the reason this daemon cannot use `regex`: that crate omits
    // lookaround on purpose, to guarantee linear-time matching, so
    // `^bash(?!_script)` is a compile error there rather than a rule. The same
    // applies to `regex-automata`, whose HIR has no lookaround at all, so its
    // meta, hybrid and DFA engines each reject it.
    let mut rules = rules();

    assert!(rules.load_rule_from_string(
        r#"{ "name": "lookaround_rule", "name_regex": "^bash(?!_script)", "type": "Doc-View" }"#
    ));

    assert!(rules.get_rule("bash").is_some());
    assert!(rules.get_rule("bash_script").is_none());
}

#[test]
fn a_name_regex_anchor_dollar_matches_before_a_trailing_newline() {
    // `$` means "end of the name, or just before a final newline", which is what
    // PCRE2 does and therefore what a rule written for `ananicy-cpp` means. The
    // standard `regex` crate and `regex-automata` read `$` as absolute
    // end-of-input instead, so under either of them a rule written as `^foo$`
    // would silently stop matching a process whose `argv[0]` is `foo\n…` — which
    // is reachable, and which `docs/COMPATIBILITY.md` §5.2 already has to
    // reason about. Mimicking PCRE2 here is the whole point.
    let mut rules = rules();

    assert!(rules.load_rule_from_string(r#"{ "name": "foo", "name_regex": "^foo$", "nice": 3 }"#));

    assert_eq!(rule_of(&rules, "foo")["nice"], 3);
    assert_eq!(
        rule_of(&rules, "foo\n")["nice"],
        3,
        "a newline at the end of the name does not defeat the anchor"
    );
    assert!(
        rules.get_rule("foo\nbar").is_none(),
        "the anchor is still an anchor, not a prefix"
    );
}

#[test]
fn an_exact_name_match_wins_over_a_regex_match() {
    let mut rules = rules();

    assert!(
        rules.load_rule_from_string(r#"{ "name": "app", "name_regex": "^app.*$", "nice": 1 }"#)
    );
    assert!(rules.load_rule_from_string(r#"{ "name": "app-helper", "nice": 2 }"#));

    assert_eq!(rule_of(&rules, "app")["nice"], 1, "the exact rule is used");
    assert_eq!(
        rule_of(&rules, "app-helper")["nice"],
        2,
        "an exact name is never shadowed by a regex rule"
    );
    assert_eq!(rule_of(&rules, "app-other")["nice"], 1);
}

#[test]
fn an_invalid_name_regex_keeps_the_rule_usable() {
    let mut rules = rules();

    // The regex is rejected, but the rule itself is still registered so that a
    // typo in a community rule cannot silently drop the whole entry.
    assert!(rules.load_rule_from_string(
        r#"{ "name": "broken-regex", "name_regex": "^(unclosed", "nice": 1 }"#
    ));
    assert_eq!(rule_of(&rules, "broken-regex")["nice"], 1);
}

/// Every `name_regex` that has ever appeared in an ananicy rule file, in this
/// repository's tests, or in the `ananicy-cpp` unit tests, with the answers the
/// C++ daemon gives. These are the answers a rule author may rely on, so the
/// expectations here are the contract rather than a description of whatever the
/// current engine happens to answer.
#[test]
fn the_whole_ananicy_name_regex_corpus_still_matches() {
    // (pattern, names that match, names that must not)
    let corpus: &[(&str, &[&str], &[&str])] = &[
        (
            r"^(java|javaw)[0-9.]*$",
            &["java", "java17", "javaw", "java.1.0", "javaw17"],
            &["notjava", "xjava", "java17x", "", "Java"],
        ),
        (
            r"^java[0-9.]*$",
            &["java", "java17", "java17.0.1"],
            &["jav", "javaw", "xjava17"],
        ),
        (
            r"^bash(?!_script)",
            &["bash", "bashx", "bashrc", "bash-script", "bash_"],
            &["bash_script", "ba.sh", ""],
        ),
        (
            r"^app.*$",
            &["app", "app-helper", "appother", "app\n"],
            &["xapp", "App", ""],
        ),
        // Unanchored, so this is a search for `gcc-` anywhere in the name —
        // which is what makes it usable at all against a full path or a
        // truncated `comm`.
        (
            r"gcc-.*",
            &["gcc-aarch64", "gcc-", "gcc-1", "/usr/bin/gcc-aarch64"],
            &["gfc-aarch64", "gcc"],
        ),
        (r"^Steam.*$", &["Steam", "Steam.exe"], &["steam", "xSteam"]),
        (
            r"^.*-wrapped$",
            &[".foo-wrapped", "foo-wrapped", "-wrapped"],
            &["wrapped", "foo-wrapped-"],
        ),
        (
            r"^kworker/[0-9]+-[0-9]+$",
            &["kworker/0-1", "kworker/12-345"],
            &["kworker/a-b", "kworker/0-", "kworker"],
        ),
        (
            r"^(?:gimp|inkscape|blender)$",
            &["gimp", "inkscape", "blender"],
            &["gimp2", "Gimp", "gimp "],
        ),
        (
            r"^firefox(-bin|-esr)?$",
            &["firefox", "firefox-bin", "firefox-esr"],
            &["firefox2", "firefox-esr2", "firefox-"],
        ),
        (
            r"^.*\.exe$",
            &["setup.exe", "game.exe", ".exe"],
            &["setup.ex", "setup.exe2", "exe"],
        ),
        (
            r"^m?\w+ode$",
            &["mode", "node", "mnode"],
            &["mod", "m ode", "node2"],
        ),
    ];

    for (pattern, matches, misses) in corpus {
        let mut rules = rules();
        // One rule carrying the pattern; the `name` is arbitrary because every
        // lookup here is expected to go through the regex path.
        assert!(
            rules.load_rule_from_string(&format!(
                r#"{{ "name": "carrier", "name_regex": {pattern:?}, "nice": 7 }}"#
            )),
            "rule for {pattern} was rejected"
        );

        for name in *matches {
            assert_eq!(
                rule_of(&rules, name)["nice"],
                7,
                "{pattern:?} should match {name:?}"
            );
        }
        for name in *misses {
            assert!(
                rules.get_rule(name).is_none(),
                "{pattern:?} should not match {name:?}"
            );
        }
    }
}

/// Constructs a rule file may legally contain and the engine declines. All of
/// these are valid PCRE2, which is the dialect the `name_regex` key was
/// introduced for and the one existing rule files are written against, and none
/// of them appears in any ananicy ruleset — the shipped set has no `name_regex`
/// at all. A community ruleset could still carry one, so the contract is that it
/// degrades to a log line plus an exact-name rule rather than to a rule that
/// quietly stopped matching.
#[test]
fn a_name_regex_the_engine_refuses_keeps_the_rule_usable() {
    // `regexr` declines most of these deliberately: its engines are linear-time
    // and never backtrack, so there is nothing for an atomic group or a
    // possessive quantifier to bound, and refusing is the honest answer instead
    // of quietly ignoring a quantifier that asks for a different match.
    for pattern in [
        r"(?>a+)b", // atomic group
        r"a++b",    // possessive quantifiers
        r"a*+b",
        r"a?+b",
        r"foo\Kbar",     // \K
        r"(?|(a)b)",     // branch reset
        r"(a)(?(1)b|c)", // conditional
        r"(*UTF)abc",    // PCRE verbs
        r"\V",           // vertical whitespace
    ] {
        let mut rules = rules();
        let name = format!("refused-{}", pattern.escape_default());
        assert!(
            rules.load_rule_from_string(&format!(
                r#"{{ "name": {name:?}, "name_regex": {pattern:?}, "nice": 4 }}"#
            )),
            "the rule for {pattern:?} was dropped entirely"
        );
        assert_eq!(
            rule_of(&rules, &name)["nice"],
            4,
            "{pattern:?} must still register as an exact-name rule"
        );
    }
}

/// The two limits that keep a third-party rule from taking the daemon down. The
/// release profile sets `panic = "abort"`, so an unbounded compile here is not
/// a failed rule load — it is a dead daemon.
#[test]
fn an_unreasonable_name_regex_is_refused_rather_than_exhausting_the_daemon() {
    // Past the nesting limit. Unbounded, this overflows the stack, which Rust
    // reports as an uncatchable abort rather than an error a caller can handle.
    let deep = format!("{}a{}", "(?:".repeat(300), ")".repeat(300));
    let mut nested = rules();
    assert!(nested.load_rule_from_string(&format!(
        r#"{{ "name": "deep", "name_regex": {deep:?}, "nice": 1 }}"#
    )));
    assert_eq!(
        rule_of(&nested, "deep")["nice"],
        1,
        "a pattern too deep to compile must not take the load with it"
    );

    // Past the expansion limit: eleven characters of pattern, unbounded work.
    // A backslash has to be doubled to survive the JSON string, which is the
    // only way a rule author can reach `\w` at all — see
    // `a_name_regex_backslash_must_be_escaped_for_the_json`.
    let mut wide = rules();
    assert!(
        wide.load_rule_from_string(
            r#"{ "name": "wide", "name_regex": "\\w{200000,}", "nice": 2 }"#
        )
    );
    assert_eq!(rule_of(&wide, "wide")["nice"], 2);

    // Both of those were refused, so neither reached the match path; an
    // ordinary rule is unaffected by a neighbour that was too large.
    let mut ordinary = rules();
    assert!(
        ordinary.load_rule_from_string(
            r#"{ "name": "java", "name_regex": "^java[0-9.]*$", "nice": 3 }"#
        )
    );
    assert_eq!(rule_of(&ordinary, "java17")["nice"], 3);
}

/// `ananicy-cpp` walks its regex list in load order and takes the first hit. A
/// set-based matcher would answer "did anything match" without saying which,
/// and picking a different rule would change what a process is reniced to.
#[test]
fn the_first_matching_regex_in_load_order_wins() {
    let mut rules = rules();
    assert!(
        rules.load_rule_from_string(r#"{ "name": "first", "name_regex": "^app.*$", "nice": 1 }"#)
    );
    assert!(
        rules.load_rule_from_string(
            r#"{ "name": "second", "name_regex": "^app-helper$", "nice": 2 }"#
        )
    );

    assert_eq!(rule_of(&rules, "app-helper")["nice"], 1);
    assert_eq!(rule_of(&rules, "app-other")["nice"], 1);
    // Both patterns are reachable; only the first is ever consulted for a name
    // both match.
    assert_eq!(rules.size(), 2);
}

/// `\d` and `\w` mean ASCII here. PCRE2 built with `PCRE2_UCP` — how
/// `ananicy-cpp` matches, and so what a rule written for it means — makes them
/// Unicode-aware, and this does not: `\d` matches `١٧` there and not here. That
/// is the one place the mimicry is incomplete. No rule in any ananicy ruleset
/// uses either escape, the shipped rule set has no `name_regex` at all, and a
/// Unicode property class is Unicode-aware under both dialects, so `\p{Nd}` is
/// the spelling to port such a rule to. The difference is pinned here and
/// written down in `docs/COMPATIBILITY.md` rather than left to be discovered by
/// a user whose process name happens to contain a non-ASCII digit.
#[test]
fn a_ucp_sensitive_class_is_ascii_and_the_alternative_spelling_is_ported() {
    let mut ascii = rules();
    assert!(
        ascii.load_rule_from_string(r#"{ "name": "digit", "name_regex": "^\\d+$", "nice": 1 }"#)
    );
    assert!(ascii.get_rule("17").is_some());
    assert!(
        ascii.get_rule("١٧").is_none(),
        r"\d is ASCII here; PCRE2 under PCRE2_UCP, which ananicy-cpp uses, would match it"
    );

    let mut unicode = rules();
    assert!(
        unicode.load_rule_from_string(
            r#"{ "name": "letter", "name_regex": "^\\p{Nd}+$", "nice": 1 }"#
        )
    );
    assert!(unicode.get_rule("17").is_some());
    assert!(
        unicode.get_rule("١٧").is_some(),
        "a Unicode property class is Unicode-aware, and is the spelling to port a UCP rule to"
    );
}

/// A `\d` in a rule file is two characters in the file and one in the pattern,
/// because the rule is a JSON string and JSON has no `\d`. A rule author who
/// forgets is not writing a rule that fails to match — they are writing a line
/// this parser rejects outright, which is a much quieter mistake.
#[test]
fn a_name_regex_backslash_must_be_escaped_for_the_json() {
    let mut rules = rules();
    assert!(
        !rules.load_rule_from_string(r#"{ "name": "x", "name_regex": "^\d+$" }"#),
        "an unescaped \\d is not valid JSON, so the line is not a rule at all"
    );
    assert_eq!(rules.size(), 0);

    assert!(rules.load_rule_from_string(r#"{ "name": "x", "name_regex": "^\\d+$", "nice": 1 }"#));
    assert_eq!(rule_of(&rules, "42")["nice"], 1);
    assert!(
        rules.get_rule("x42").is_none(),
        "the pattern is anchored, so this is about the escape and not the class"
    );
}

/// A backreference is the only construct that can make a search exceed its step
/// budget, and no ananicy rule uses one. If a rule did, the engine reports the
/// budget rather than hanging the worker thread, and a bounded "no match" is
/// what the process gets.
#[test]
fn a_pattern_with_a_backreference_is_usable() {
    let mut rules = rules();
    // Compiles and runs, on the backtracking engine, under a step budget.
    assert!(
        rules
            .load_rule_from_string(r#"{ "name": "repeat", "name_regex": "^(a+)\\1$", "nice": 5 }"#)
    );
    assert!(rules.get_rule("aaaa").is_some());
    assert!(rules.get_rule("aaa").is_none());
    assert_eq!(rule_of(&rules, "aaaa")["nice"], 5);
}

#[test]
fn reloading_rules_invalidates_the_lookup_cache() {
    let mut rules = rules();

    assert!(rules.get_rule("cached").is_none());
    let (_directory, path) = rules_dir(&[("a.rules", r#"{ "name": "cached", "nice": 1 }"#)]);
    rules.load_directory(&path);

    assert_eq!(
        rule_of(&rules, "cached")["nice"],
        1,
        "a rule loaded after a cache miss must be visible"
    );
}

#[test]
fn directory_loading_only_reads_the_enabled_file_kinds() {
    let entries = [
        ("a.rules", r#"{ "name": "prog", "nice": 1 }"#),
        ("b.types", r#"{ "type": "Game", "nice": 2 }"#),
        ("c.cgroups", r#"{ "cgroup": "games" }"#),
        ("ignored.txt", r#"{ "name": "prog", "nice": 99 }"#),
        ("ignored.json", r#"{ "name": "prog", "nice": 98 }"#),
    ];

    let (_enabled, enabled_path) = rules_dir(&entries);
    let mut enabled = rules();
    enabled.load_directory(&enabled_path);
    assert_eq!(enabled.size(), 1);
    assert_eq!(enabled.get_types().len(), 1);
    assert_eq!(enabled.get_cgroups().len(), 1);
    assert_eq!(
        rule_of(&enabled, "prog")["nice"],
        1,
        "only *.rules files contribute program rules"
    );

    let (_disabled, disabled_path) = rules_dir(&entries);
    let mut disabled = Rules::new(Arc::new(Config::new(ConfigSnapshot {
        rule_load: false,
        type_load: false,
        cgroup_load: false,
        ..ConfigSnapshot::default()
    })));
    disabled.load_directory(&disabled_path);
    assert_eq!(disabled.size(), 0);
    assert_eq!(disabled.get_types().len(), 0);
    assert_eq!(disabled.get_cgroups().len(), 0);
}

#[test]
fn directory_loading_is_deterministic_for_duplicate_rule_names() {
    // Rule files are sorted before loading, so a duplicate program name is
    // resolved alphabetically instead of by filesystem iteration order.
    let (_first, first_path) = rules_dir(&[
        ("b-second.rules", r#"{ "name": "dup", "nice": 2 }"#),
        ("a-first.rules", r#"{ "name": "dup", "nice": 1 }"#),
    ]);
    let mut first = rules();
    first.load_directory(&first_path);

    let (_second, second_path) = rules_dir(&[
        ("a-first.rules", r#"{ "name": "dup", "nice": 1 }"#),
        ("b-second.rules", r#"{ "name": "dup", "nice": 2 }"#),
    ]);
    let mut second = rules();
    second.load_directory(&second_path);

    assert_eq!(
        rule_of(&first, "dup")["nice"],
        2,
        "files are loaded in sorted order, so the last one alphabetically wins"
    );
    assert_eq!(
        rule_of(&first, "dup"),
        rule_of(&second, "dup"),
        "the load order must not depend on the order the files were created"
    );
}

#[test]
fn program_rules_inherit_their_type() {
    let mut rules = rules();
    let (_directory, path) = rules_dir(&[
        (
            "types.types",
            r#"{ "type": "Doc-View", "nice": -4, "latency_nice": 5, "sched": "batch" }"#,
        ),
        (
            "rules.rules",
            r#"{ "name": "icecat", "type": "Doc-View", "nice": -7 }"#,
        ),
    ]);

    rules.load_directory(&path);

    let inherited = rule_of(&rules, "icecat");
    assert_eq!(inherited["nice"], -7, "the program rule overrides the type");
    assert_eq!(
        inherited["latency_nice"], 5,
        "unset fields come from the type"
    );
    assert_eq!(inherited["sched"], "batch");
    assert_eq!(inherited["type"], "Doc-View");
}

#[test]
fn a_type_defined_after_a_program_rule_is_still_applied() {
    let mut rules = rules();
    let (_directory, path) = rules_dir(&[
        ("a.rules", r#"{ "name": "app", "type": "Later" }"#),
        ("z.types", r#"{ "type": "Later", "oom_score_adj": -100 }"#),
    ]);

    rules.load_directory(&path);

    assert_eq!(
        rule_of(&rules, "app")["oom_score_adj"],
        -100,
        "inheritance must not depend on the order the files are read in"
    );
}

#[test]
fn inheritance_does_not_mutate_the_shared_type() {
    let mut rules = rules();
    let (_directory, path) = rules_dir(&[
        (
            "types.types",
            r#"{ "type": "Shared", "nice": 1, "ionice": 5 }"#,
        ),
        (
            "rules.rules",
            r#"{ "name": "first", "type": "Shared", "nice": 2 }"#,
        ),
    ]);

    rules.load_directory(&path);

    assert_eq!(rule_of(&rules, "first")["nice"], 2);
    let shared = rules
        .get_types()
        .get(&TypeName("Shared".to_string()))
        .expect("the type is still registered");
    assert_eq!(
        shared["nice"], 1,
        "a program rule must not rewrite the type it inherits from"
    );
}
