//! Compare cached module diagnostics with an uncached whole-crate run after each change.
#![feature(rustc_private)]
#![cfg(all(ohm_incremental_driver_hash, any(target_os = "linux", target_os = "macos")))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const LINT: &str = "clippy::zero_sized_map_values";

struct Fixture {
    directory: tempfile::TempDir,
    threads: usize,
    docs_links: bool,
    msrv: Option<&'static str>,
    command_line_lint: bool,
    lints: Vec<String>,
    whole_crate_reference: bool,
    expect_source_input: bool,
}

impl Fixture {
    fn new(threads: usize) -> Self {
        let fixture = Self {
            directory: tempfile::tempdir().unwrap(),
            threads,
            docs_links: true,
            msrv: None,
            command_line_lint: true,
            lints: vec![LINT.into()],
            whole_crate_reference: false,
            expect_source_input: false,
        };
        fixture.write("lib.rs", "pub mod value;\npub mod first;\npub mod second;\n");
        fixture.write("value.rs", "pub struct Value;\n");
        for module in ["first", "second"] {
            fixture.write(&format!("{module}.rs"), "use std::collections::{HashMap, BTreeMap};\nuse crate::value::Value;\npub type Hash = HashMap<u8, Value>;\npub type Tree = BTreeMap<u8, Value>;\npub fn edited() -> u8 { 1 + 1 }\n");
        }
        fixture.write("clippy.toml", "avoid-breaking-exported-api = false\n");
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.path(name), text).unwrap();
    }

    fn replace(&self, name: &str, from: &str, to: &str) {
        let source = fs::read_to_string(self.path(name)).unwrap();
        assert!(source.contains(from));
        self.write(name, &source.replace(from, to));
    }

    fn run(&self, incremental: Option<&Path>, cache_state: Option<&str>) -> (Vec<Value>, Vec<u8>) {
        self.run_with_lint_flags(incremental, cache_state, self.command_line_lint)
    }

    fn run_with_lint_flags(
        &self,
        incremental: Option<&Path>,
        cache_state: Option<&str>,
        command_line_lint: bool,
    ) -> (Vec<Value>, Vec<u8>) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clippy-driver"));
        command
            .current_dir(self.directory.path())
            .args([
                "--crate-name",
                "incremental_lint_fixture",
                "--crate-type",
                "lib",
                "--edition=2024",
                "--emit=metadata",
                "--error-format=json",
                "-Aclippy::all",
                "-Aclippy::pedantic",
                "-Aunused",
                "-Wunfulfilled_lint_expectations",
                "-Zincremental-verify-ich=yes",
                "-Cmetadata=fixture",
                "lib.rs",
                "-o",
                "out.rmeta",
            ])
            .arg(format!("-Zthreads={}", self.threads))
            .env("CLIPPY_CONF_DIR", self.directory.path())
            .env_remove("CLIPPY_ARGS")
            .env_remove("CLIPPY_DISABLE_DOCS_LINKS")
            .env_remove("CARGO_PKG_RUST_VERSION");
        if command_line_lint {
            for lint in &self.lints {
                command.arg(format!("-W{lint}"));
            }
        }
        if let Some(sysroot) = option_env!("TEST_SYSROOT") {
            command.arg("--sysroot").arg(sysroot);
        }
        if !self.docs_links {
            command.env("CLIPPY_DISABLE_DOCS_LINKS", "1");
        }
        if let Some(msrv) = self.msrv {
            command.env("CARGO_PKG_RUST_VERSION", msrv);
        }
        if let Some(incremental) = incremental {
            command.arg(format!("-Cincremental={}", incremental.display()));
            if self.expect_source_input {
                command.arg("-Zincremental-info");
            }
            if let Some(state) = cache_state {
                command.arg(format!("-Zassert-incr-state={state}"));
            }
        }
        let output = command.output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{command:?}\n{stderr}");
        if self.expect_source_input && command_line_lint && cache_state == Some("not-loaded") {
            assert!(
                stderr.contains("source_file_hash"),
                "source-sensitive lints fell back instead of recording source inputs: {stderr}"
            );
        }
        let diagnostics = stderr
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value["$message_type"] == "diagnostic")
            .collect();
        (diagnostics, fs::read(self.path("out.rmeta")).unwrap())
    }

    fn compare(&self, cache_state: Option<&str>) -> Vec<Value> {
        let (reference, _) = self.run(None, None);
        let fresh_cache = tempfile::tempdir_in(self.directory.path()).unwrap();
        let (cold, cold_metadata) = self.run(Some(fresh_cache.path()), Some("not-loaded"));
        let (cached, cached_metadata) = self.run(Some(&self.path("incremental")), cache_state);
        assert_eq!(normalize(reference.clone()), normalize(cold));
        assert_eq!(normalize(reference.clone()), normalize(cached));
        if self.whole_crate_reference {
            // Attribute-only activation retains the whole-crate pass. Compare identical
            // edit histories in both traversals: even the released compiler can encode
            // different bytes for cold versus warm whitespace-only edits. Neither cold
            // nor warm metadata is exempted from byte-for-byte reference comparison.
            let whole_cold_cache = tempfile::tempdir_in(self.directory.path()).unwrap();
            let (whole_cold, whole_cold_metadata) =
                self.run_with_lint_flags(Some(whole_cold_cache.path()), Some("not-loaded"), false);
            let (whole_cached, whole_cached_metadata) =
                self.run_with_lint_flags(Some(&self.path("whole-incremental")), cache_state, false);
            assert_eq!(normalize(reference.clone()), normalize(whole_cold));
            assert_eq!(normalize(reference.clone()), normalize(whole_cached));
            assert!(
                cold_metadata == whole_cold_metadata,
                "cold module metadata differs from whole-crate metadata"
            );
            assert!(
                cached_metadata == whole_cached_metadata,
                "warm module metadata differs from whole-crate metadata"
            );
        } else {
            // rustc includes owner spans in the crate hash only when incremental is enabled.
            assert!(
                cold_metadata == cached_metadata,
                "incremental lint inputs changed emitted metadata"
            );
        }
        reference
    }

    fn check(&self, warnings: usize, expectations: usize, cache_state: Option<&str>) {
        let diagnostics = self.compare(cache_state);
        assert_eq!(count(&diagnostics, LINT), warnings);
        assert_eq!(count(&diagnostics, "unfulfilled_lint_expectations"), expectations);
    }

    fn check_counts(&self, expected: &[(&str, usize)], cache_state: &str) {
        let diagnostics = self.compare(Some(cache_state));
        for (lint, expected) in expected {
            assert_eq!(count(&diagnostics, lint), *expected, "{lint}: {diagnostics:?}");
        }
    }
}

/// A lint-level note may attach to a different warning for the same lint with parallel
/// linting. Preserve its lint identity, complete content, spans, and count separately.
fn normalize(diagnostics: Vec<Value>) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut diagnostics = diagnostics
        .into_iter()
        .map(|mut value| {
            value.as_object_mut().unwrap().remove("rendered");
            let code = value["code"].clone();
            if let Some(children) = value["children"].as_array_mut() {
                children.retain(|child| {
                    if child["level"] == "note"
                        && (child["message"] == "the lint level is defined here"
                            || (child["spans"] == serde_json::json!([])
                                && child["message"].as_str().is_some_and(|message| {
                                    message.starts_with("requested on the command line with `-W clippy::")
                                })))
                    {
                        notes.push(serde_json::json!({"lint": code, "note": child}).to_string());
                        false
                    } else {
                        true
                    }
                });
            }
            value.to_string()
        })
        .collect::<Vec<_>>();
    diagnostics.sort();
    notes.sort();
    (diagnostics, notes)
}

#[test]
fn module_lints_replay_and_invalidate_observed_inputs() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.check(4, 0, Some("not-loaded"));
        fixture.check(4, 0, Some("loaded"));
        fixture.replace("first.rs", "1 + 1", "9 + 1");
        fixture.check(4, 0, Some("loaded"));
        fixture.write("value.rs", "pub struct Value(pub u8);\n");
        fixture.check(0, 0, Some("loaded"));
        fixture.write("value.rs", "pub struct Value;\n");
        fixture.check(4, 0, Some("loaded"));

        fixture.write("clippy.toml", "avoid-breaking-exported-api = true\n");
        fixture.check(4, 0, Some("not-loaded"));
        fs::remove_file(fixture.path("clippy.toml")).unwrap();
        fixture.check(4, 0, Some("not-loaded"));
        fixture.write("clippy.toml", "avoid-breaking-exported-api = false\n");
        fixture.check(4, 0, Some("not-loaded"));
        fixture.msrv = Some("1.70");
        fixture.check(4, 0, Some("not-loaded"));
        fixture.docs_links = false;
        fixture.check(4, 0, Some("not-loaded"));
        fixture.check(4, 0, Some("loaded"));
        fixture.docs_links = true;
        fixture.check(4, 0, Some("not-loaded"));

        fixture.replace(
            "lib.rs",
            "pub mod value;",
            "#![allow(clippy::zero_sized_map_values)]\npub mod value;",
        );
        fixture.check(0, 0, Some("loaded"));
        fixture.replace(
            "first.rs",
            "use std",
            "#![warn(clippy::zero_sized_map_values)]\nuse std",
        );
        fixture.check(2, 0, Some("loaded"));
        fixture.replace("lib.rs", "#![allow(clippy::zero_sized_map_values)]\n", "");
        fixture.replace(
            "first.rs",
            "pub type Hash",
            "#[expect(clippy::zero_sized_map_values)]\npub type Hash",
        );
        fixture.check(3, 0, Some("loaded"));
        fixture.replace("first.rs", "HashMap<u8, Value>", "HashMap<u8, u8>");
        fixture.check(3, 1, Some("loaded"));
        fixture.check(3, 1, Some("loaded"));
        fixture.replace("first.rs", "#![warn", "// move diagnostic spans\n#![warn");
        fixture.check(3, 1, Some("loaded"));
        fixture.replace("first.rs", "9 + 1", "9+  1");
        fixture.check(3, 1, Some("loaded"));

        let finalized_caches = walkdir::WalkDir::new(fixture.path("incremental"))
            .into_iter()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name() == "dep-graph.bin")
            .count();
        assert!(
            finalized_caches <= 2,
            "configuration changes retained {finalized_caches} query graphs"
        );
    }
}

#[test]
fn existing_ui_cases_replay_in_an_unchanged_module() {
    for threads in [1, 4] {
        let fixture = Fixture::new(threads);
        fixture.write("lib.rs", "pub mod examples;\npub fn unrelated() -> u8 { 1 + 1 }\n");
        fixture.write("examples.rs", include_str!("ui/zero_sized_hashmap_values.rs"));
        fixture.check(14, 0, Some("not-loaded"));
        fixture.replace("lib.rs", "1 + 1", "2 + 1");
        fixture.check(14, 0, Some("loaded"));
        fixture.check(14, 0, Some("loaded"));
        fixture.replace("examples.rs", "const CONST_OK", "// move spans\nconst CONST_OK");
        fixture.check(14, 0, Some("loaded"));
    }
}

#[test]
fn attribute_only_activation_retains_the_whole_crate_pass() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.command_line_lint = false;
        fixture.replace(
            "lib.rs",
            "pub mod value;",
            "#![warn(clippy::zero_sized_map_values)]\npub mod value;",
        );
        fixture.check(4, 0, Some("not-loaded"));
        fixture.write("clippy.toml", "avoid-breaking-exported-api = true\n");
        // This mode does not cache Clippy queries or add a driver compatibility key.
        fixture.check(4, 0, Some("loaded"));
        fixture.write("value.rs", "pub struct Value(pub u8);\n");
        fixture.check(0, 0, Some("loaded"));
    }
}

fn count(diagnostics: &[Value], code: &str) -> usize {
    diagnostics.iter().filter(|value| value["code"]["code"] == code).count()
}

/// Every registered lint must emit a diagnostic in an unchanged module. Compare the
/// complete diagnostic payload, including suggestions, to an uncached compilation.
#[test]
fn audited_lints_replay_after_edits() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("incremental_lint_cases.json")).unwrap();
    let covered: std::collections::BTreeSet<_> = cases
        .iter()
        .flat_map(|case| {
            case["lints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|lint| format!("clippy::{}", lint.as_str().unwrap()))
        })
        .collect();
    let registered: std::collections::BTreeSet<_> = clippy_lints::incremental_lints::lints()
        .map(|info| info.lint.name_lower())
        .collect();
    assert_eq!(covered, registered, "every cached lint needs a positive fixture");
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.lints = covered.iter().cloned().collect();
        fixture.whole_crate_reference = true;
        fixture.write(
            "clippy.toml",
            "avoid-breaking-exported-api = false\narray-size-threshold = 16\n",
        );
        // The source-level lint settings are identical in both traversals. Omitting
        // positive CLI flags selects the existing attribute-only whole-crate fallback.
        let mut root = format!(
            "#![warn({})]\npub fn unrelated() -> u8 {{ 1 + 1 }}\n",
            fixture.lints.join(",")
        );
        for (index, case) in cases.iter().enumerate() {
            root.push_str(&format!("pub mod case_{index};\n"));
            fixture.write(&format!("case_{index}.rs"), case["source"].as_str().unwrap());
        }
        // Nested modules, including those inside bodies, are also separate lint queries.
        root.push_str("pub fn nested() { mod inner { pub struct S; impl S { pub fn f(&self) {} } } }\n");
        fixture.write("lib.rs", &root);
        for state in ["not-loaded", "loaded"] {
            let diagnostics = fixture.compare(Some(state));
            for lint in &covered {
                assert!(count(&diagnostics, lint) > 0, "missing {lint}: {diagnostics:?}");
            }
        }
        fixture.replace("lib.rs", "1 + 1", "2 + 1");
        fixture.compare(Some("loaded"));
        fixture.replace("lib.rs", "2 + 1", "2+  1");
        fixture.compare(Some("loaded"));
        let question_mark_module = format!(
            "case_{}.rs",
            cases
                .iter()
                .position(|case| case["pass"] == "NeedlessQuestionMark")
                .unwrap()
        );
        fixture.replace(&question_mark_module, "Some(x?)", "Some(x? )");
        fixture.compare(Some("loaded"));
        // Move whitespace within a suggested replacement without changing file length.
        // Cached multipart edits must still remove exactly the intended tokens.
        fixture.replace(&question_mark_module, "Some(x? )", "Some( x?)");
        fixture.compare(Some("loaded"));
        fixture.write(
            "case_0.rs",
            "// remove this module's diagnostic\npub struct Replaced;\n",
        );
        fixture.compare(Some("loaded"));
        for (index, case) in cases.iter().enumerate().skip(1) {
            fixture.write(
                &format!("case_{index}.rs"),
                &format!("\n// moved spans\n{}", case["source"].as_str().unwrap()),
            );
        }
        fixture.compare(Some("loaded"));
    }
}

#[test]
fn inherited_msrv_and_expectations_invalidate_unchanged_children() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.lints = vec![
            "clippy::manual_main_separator_str".into(),
            "clippy::manual_noop_waker".into(),
        ];
        fixture.write("lib.rs", "pub mod first;\n");
        fixture.write("first.rs", "pub mod child;\n");
        fs::create_dir(fixture.path("first")).unwrap();
        fixture.write("first/child.rs", "pub fn separator() { let _: &str = &std::path::MAIN_SEPARATOR.to_string(); }\npub struct W; impl std::task::Wake for W { fn wake(self: std::sync::Arc<Self>) {} }\n");
        let both = [
            ("clippy::manual_main_separator_str", 1),
            ("clippy::manual_noop_waker", 1),
        ];
        fixture.check_counts(&both, "not-loaded");
        fixture.check_counts(&both, "loaded");
        fixture.write("first.rs", "#[clippy::msrv = \"1.60\"]\npub mod child;\n");
        fixture.check_counts(
            &[
                ("clippy::manual_main_separator_str", 0),
                ("clippy::manual_noop_waker", 0),
            ],
            "loaded",
        );
        fixture.replace("first.rs", "1.60", "1.70");
        fixture.check_counts(
            &[
                ("clippy::manual_main_separator_str", 1),
                ("clippy::manual_noop_waker", 0),
            ],
            "loaded",
        );
        fixture.write("first.rs", "pub mod child;\n");
        fixture.check_counts(&both, "loaded");
        fixture.write(
            "first.rs",
            "#[expect(clippy::manual_main_separator_str, clippy::manual_noop_waker)]\npub mod child;\n",
        );
        fixture.check_counts(
            &[
                ("clippy::manual_main_separator_str", 0),
                ("clippy::manual_noop_waker", 0),
                ("unfulfilled_lint_expectations", 0),
            ],
            "loaded",
        );
        fixture.check_counts(&[("unfulfilled_lint_expectations", 0)], "loaded");
        fixture.replace(
            "first/child.rs",
            "&std::path::MAIN_SEPARATOR.to_string()",
            "std::path::MAIN_SEPARATOR_STR",
        );
        fixture.check_counts(&[("unfulfilled_lint_expectations", 1)], "loaded");
        fixture.check_counts(&[("unfulfilled_lint_expectations", 1)], "loaded");
    }
}

#[test]
fn macro_definitions_and_trait_impls_invalidate_consumers() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.lints = vec![
            "clippy::inherent_to_string".into(),
            "clippy::inherent_to_string_shadow_display".into(),
            "clippy::debug_assert_with_mut_call".into(),
        ];
        fixture.write(
            "lib.rs",
            "macro_rules! mutation { () => { crate::first::mutation(&mut 0) }; }\npub mod first; pub mod second;\n",
        );
        fixture.write("first.rs", "pub struct S; impl S { pub fn to_string(&self) -> String { String::new() } }\npub fn mutation(x: &mut u8) -> bool { *x += 1; true }\npub fn f() { debug_assert!(mutation!()); }\n");
        fixture.write("second.rs", "");
        fixture.check_counts(
            &[
                ("clippy::inherent_to_string", 1),
                ("clippy::inherent_to_string_shadow_display", 0),
                ("clippy::debug_assert_with_mut_call", 1),
            ],
            "not-loaded",
        );
        fixture.compare(Some("loaded"));
        fixture.write("second.rs", r#"impl std::fmt::Display for crate::first::S { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("S") } }"#);
        fixture.check_counts(
            &[
                ("clippy::inherent_to_string", 0),
                ("clippy::inherent_to_string_shadow_display", 1),
            ],
            "loaded",
        );
        fixture.replace("lib.rs", "crate::first::mutation(&mut 0)", "true");
        fixture.check_counts(&[("clippy::debug_assert_with_mut_call", 0)], "loaded");
        fixture.replace("lib.rs", "=> { true }", "=> { crate::first::mutation(&mut 0) }");
        fixture.check_counts(&[("clippy::debug_assert_with_mut_call", 1)], "loaded");
    }
}

#[test]
fn configuration_changes_affect_cached_lints() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.lints = vec!["clippy::unused_self".into(), "clippy::large_const_arrays".into()];
        fixture.write("lib.rs", "pub mod first;\n");
        fixture.write(
            "first.rs",
            "pub struct S; impl S { pub fn unused(&self) {} }\npub const LARGE: [u8; 32] = [0; 32];\n",
        );
        fixture.write(
            "clippy.toml",
            "avoid-breaking-exported-api = false\narray-size-threshold = 16\n",
        );
        fixture.check_counts(
            &[("clippy::unused_self", 1), ("clippy::large_const_arrays", 1)],
            "not-loaded",
        );
        fixture.check_counts(
            &[("clippy::unused_self", 1), ("clippy::large_const_arrays", 1)],
            "loaded",
        );
        fixture.write(
            "clippy.toml",
            "avoid-breaking-exported-api = true\narray-size-threshold = 64\n",
        );
        fixture.check_counts(
            &[("clippy::unused_self", 0), ("clippy::large_const_arrays", 0)],
            "not-loaded",
        );
        fixture.check_counts(
            &[("clippy::unused_self", 0), ("clippy::large_const_arrays", 0)],
            "loaded",
        );
    }
}

#[test]
#[cfg(ohm_source_file_hash)]
fn source_sensitive_ui_cases_replay_in_an_unchanged_module() {
    for (lint, source) in [
        (
            "clippy::needless_pass_by_value",
            include_str!("ui/needless_pass_by_value.rs"),
        ),
        ("clippy::redundant_clone", include_str!("ui/redundant_clone.rs")),
        ("clippy::unnecessary_wraps", include_str!("ui/unnecessary_wraps.rs")),
        (
            "clippy::needless_borrows_for_generic_args",
            include_str!("ui/needless_borrows_for_generic_args/needless_borrows_for_generic_args.rs"),
        ),
    ] {
        for threads in [1, 4] {
            let mut fixture = Fixture::new(threads);
            fixture.lints = vec![lint.into()];
            fixture.whole_crate_reference = true;
            fixture.expect_source_input = true;
            fixture.write("lib.rs", "pub mod examples;\npub fn unrelated() -> u8 { 1 + 1 }\n");
            fixture.write("examples.rs", source);
            assert!(count(&fixture.compare(Some("not-loaded")), lint) > 0);
            fixture.replace("lib.rs", "1 + 1", "2 + 1");
            fixture.compare(Some("loaded"));
            fixture.compare(Some("loaded"));
            fixture.write("examples.rs", &format!("// move spans\n{source}"));
            fixture.compare(Some("loaded"));
        }
    }
}

#[test]
#[cfg(ohm_source_file_hash)]
fn source_sensitive_lints_observe_trait_changes() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.lints = vec![
            "clippy::needless_pass_by_value".into(),
            "clippy::redundant_clone".into(),
        ];
        fixture.whole_crate_reference = true;
        fixture.write(
            "lib.rs",
            "#![warn(clippy::needless_pass_by_value, clippy::redundant_clone)]\npub mod first; pub mod second;\n",
        );
        fixture.write("first.rs", "use crate::second::S;\n#[allow(clippy::needless_pass_by_value)]\npub fn redundant(value: S) -> S { value.clone() }\npub fn borrowed(value: S) -> u8 { value.0 }\n");
        fixture.write("second.rs", "#[derive(Clone)] pub struct S(pub u8);\n");
        let counts = [("clippy::needless_pass_by_value", 1), ("clippy::redundant_clone", 1)];
        fixture.check_counts(&counts, "not-loaded");
        fixture.check_counts(&counts, "loaded");
        fixture.write(
            "second.rs",
            "#[derive(Clone)] pub struct S(pub u8);\nimpl Copy for S {}\n",
        );
        fixture.check_counts(
            &[("clippy::needless_pass_by_value", 0), ("clippy::redundant_clone", 0)],
            "loaded",
        );
        fixture.check_counts(
            &[("clippy::needless_pass_by_value", 0), ("clippy::redundant_clone", 0)],
            "loaded",
        );
        fixture.write("second.rs", "#[derive(Clone)] pub struct S(pub u8);\n");
        fixture.check_counts(&counts, "loaded");
    }
}

#[test]
#[cfg(ohm_source_file_hash)]
fn source_text_changes_invalidate_suggestions() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        fixture.lints = vec![
            "clippy::needless_pass_by_value".into(),
            "clippy::redundant_clone".into(),
        ];
        fixture.whole_crate_reference = true;
        fixture.expect_source_input = true;
        fixture.write(
            "lib.rs",
            "#![warn(clippy::needless_pass_by_value, clippy::redundant_clone)]\npub mod first; pub mod second;\n",
        );
        let source = "#[allow(clippy::needless_pass_by_value)]\npub fn redundant(value: String) -> String { value.clone    () }\n#[allow(clippy::redundant_clone)]\npub fn borrowed(value: Vec<u8>) -> Vec<u8> { value.clone    () }\n";
        for module in ["first.rs", "second.rs"] {
            fixture.write(module, source);
        }
        let counts = [("clippy::needless_pass_by_value", 2), ("clippy::redundant_clone", 2)];
        fixture.check_counts(&counts, "not-loaded");
        fixture.check_counts(&counts, "loaded");

        // Identical token positions and HIR, but different suggestion text/applicability.
        fixture.replace("first.rs", "clone    ()", "clone/**/()");
        fixture.check_counts(&counts, "loaded");
        let changed = fixture.compare(Some("loaded"));
        assert!(changed.iter().any(|diagnostic| {
            diagnostic["code"]["code"] == "clippy::needless_pass_by_value"
                && diagnostic.to_string().contains("value.clone/**/()")
        }));
        assert!(changed.iter().any(|diagnostic| {
            diagnostic["code"]["code"] == "clippy::redundant_clone"
                && diagnostic["children"].as_array().unwrap().iter().any(|child| {
                    child["spans"].as_array().unwrap().iter().any(|span| {
                        span["file_name"] == "first.rs" && span["suggestion_applicability"] == "MaybeIncorrect"
                    })
                })
        }));
        fixture.replace("first.rs", "clone/**/()", "clone    ()");
        fixture.check_counts(&counts, "loaded");

        // Source-file identities must not rely on their order in the source map.
        fixture.replace(
            "lib.rs",
            "pub mod first; pub mod second;",
            "pub mod second; pub mod first;",
        );
        fixture.check_counts(&counts, "loaded");
        fixture.replace("second.rs", "value.clone    ()", "value");
        fixture.check_counts(
            &[("clippy::needless_pass_by_value", 1), ("clippy::redundant_clone", 1)],
            "loaded",
        );
    }
}

#[test]
#[cfg(ohm_source_file_hash)]
fn borrowed_arguments_observe_traits_and_nested_bodies() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        let lint = "clippy::needless_borrows_for_generic_args";
        fixture.lints = vec![lint.into()];
        fixture.whole_crate_reference = true;
        fixture.expect_source_input = true;
        fixture.write(
            "lib.rs",
            "#![warn(clippy::needless_borrows_for_generic_args)]\npub mod first; pub mod second;\n",
        );
        fixture.write(
            "first.rs",
            r#"
use crate::second::{S, take};
pub fn outer(value: &mut S, extra: &mut S) {
    take(&extra);
    fn nested(value: &mut S) { take(&value); }
    let closure = |value: &mut S| { take(&value); };
    take(&value);
    closure(&mut S);
    nested(&mut S);
    mod inner { pub fn f(value: &mut crate::second::S) { crate::second::take(&value); } }
}
"#,
        );
        let traits = "pub struct S; pub trait Marker {} impl Marker for &&mut S {} impl Marker for &mut S {} pub fn take(_: impl Marker) {}\n";
        fixture.write("second.rs", traits);
        fixture.check_counts(&[(lint, 5)], "not-loaded");
        fixture.check_counts(&[(lint, 5)], "loaded");
        // No snippet is read once the trait change removes every diagnostic.
        fixture.expect_source_input = false;
        fixture.replace("second.rs", "impl Marker for &mut S {}", "");
        fixture.check_counts(&[(lint, 0)], "loaded");
        fixture.check_counts(&[(lint, 0)], "loaded");
        fixture.write("second.rs", traits);
        fixture.check_counts(&[(lint, 5)], "loaded");
    }
}

#[test]
#[cfg(ohm_source_file_hash)]
fn wrapped_returns_and_macro_arguments_track_suggestion_text() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        let wraps = "clippy::unnecessary_wraps";
        let borrows = "clippy::needless_borrows_for_generic_args";
        fixture.lints = vec![wraps.into(), borrows.into()];
        fixture.whole_crate_reference = true;
        fixture.expect_source_input = true;
        fixture.write("lib.rs", "#![warn(clippy::unnecessary_wraps, clippy::needless_borrows_for_generic_args)]\npub mod first; pub mod second;\n");
        fixture.write(
            "second.rs",
            "#[macro_export] macro_rules! text { () => { \"value\" }; }\n",
        );
        fixture.write(
            "first.rs",
            r#"
fn take(_: impl AsRef<str>) {}
pub fn wrapped() -> Option<u8> { Some(1    + 2) }
pub fn borrowed() { take(&crate::text!(    )); }
"#,
        );
        let counts = [(wraps, 1), (borrows, 1)];
        fixture.check_counts(&counts, "not-loaded");
        fixture.check_counts(&counts, "loaded");
        // Keep token positions unchanged; only the requested replacement text changes.
        fixture.replace("first.rs", "1    + 2", "1/**/+ 2");
        fixture.replace("first.rs", "text!(    )", "text!(/**/)");
        let changed = fixture.compare(Some("loaded"));
        for (lint, text) in [(wraps, "1/**/+ 2"), (borrows, "crate::text!(/**/)")] {
            assert_eq!(count(&changed, lint), 1);
            assert!(changed.iter().any(|diagnostic| {
                diagnostic["code"]["code"] == lint
                    && diagnostic["children"].as_array().unwrap().iter().any(|child| {
                        child["spans"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|span| span["suggested_replacement"] == text)
                    })
            }));
        }
        fixture.compare(Some("loaded"));
        fixture.replace("first.rs", "1/**/+ 2", "1    + 2");
        fixture.replace("first.rs", "text!(/**/)", "text!(    )");
        fixture.check_counts(&counts, "loaded");
        // Attribute-only fallback does not fingerprint configuration, so its old
        // session can load even though the module traversal must reject its session.
        fixture.whole_crate_reference = false;
        fixture.write("clippy.toml", "avoid-breaking-exported-api = true\n");
        fixture.check_counts(&[(wraps, 0), (borrows, 1)], "not-loaded");
        fixture.check_counts(&[(wraps, 0), (borrows, 1)], "loaded");
    }
}

#[test]
#[cfg(ohm_source_file_hash)]
fn borrowed_arguments_observe_inherited_msrv_and_expectations() {
    for threads in [1, 4] {
        let mut fixture = Fixture::new(threads);
        let lint = "clippy::needless_borrows_for_generic_args";
        fixture.lints = vec![lint.into()];
        fixture.whole_crate_reference = true;
        fixture.expect_source_input = true;
        fixture.write(
            "lib.rs",
            "#![warn(clippy::needless_borrows_for_generic_args)]\n#[clippy::msrv = \"1.53\"] pub mod first;\n",
        );
        fixture.write(
            "first.rs",
            "fn take(_: impl IntoIterator) {} pub fn f() { take(&[0u8]); }\n",
        );
        fixture.check_counts(&[(lint, 1)], "not-loaded");
        fixture.check_counts(&[(lint, 1)], "loaded");
        // Disabled diagnostics do not need to read source snippets.
        fixture.expect_source_input = false;
        fixture.replace("lib.rs", "1.53", "1.52");
        fixture.check_counts(&[(lint, 0)], "loaded");
        fixture.replace("lib.rs", "1.52", "1.53");
        fixture.check_counts(&[(lint, 1)], "loaded");
        fixture.replace(
            "lib.rs",
            "pub mod first;",
            "#[expect(clippy::needless_borrows_for_generic_args)] pub mod first;",
        );
        fixture.check_counts(&[(lint, 0), ("unfulfilled_lint_expectations", 0)], "loaded");
        fixture.check_counts(&[(lint, 0), ("unfulfilled_lint_expectations", 0)], "loaded");
        fixture.replace("first.rs", "&[0u8]", "[0u8]");
        fixture.check_counts(&[("unfulfilled_lint_expectations", 1)], "loaded");
        fixture.check_counts(&[("unfulfilled_lint_expectations", 1)], "loaded");
    }
}
