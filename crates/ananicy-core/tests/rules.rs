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
    // PCRE2 is used instead of the standard `regex` crate precisely so that
    // lookarounds in existing community rules keep working.
    let mut rules = rules();

    assert!(rules.load_rule_from_string(
        r#"{ "name": "lookaround_rule", "name_regex": "^bash(?!_script)", "type": "Doc-View" }"#
    ));

    assert!(rules.get_rule("bash").is_some());
    assert!(rules.get_rule("bash_script").is_none());
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
