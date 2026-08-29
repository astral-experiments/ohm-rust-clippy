//! Compare cached module diagnostics with an uncached whole-crate run after each change.
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
}

impl Fixture {
    fn new(threads: usize) -> Self {
        let fixture = Self {
            directory: tempfile::tempdir().unwrap(),
            threads,
            docs_links: true,
            msrv: None,
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
                "-Wclippy::zero_sized_map_values",
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
            if let Some(state) = cache_state {
                command.arg(format!("-Zassert-incr-state={state}"));
            }
        }
        let output = command.output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{command:?}\n{stderr}");
        let diagnostics = stderr
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value["$message_type"] == "diagnostic")
            .collect();
        (diagnostics, fs::read(self.path("out.rmeta")).unwrap())
    }

    fn check(&self, warnings: usize, expectations: usize, cache_state: Option<&str>) {
        let (reference, _) = self.run(None, None);
        let fresh_cache = tempfile::tempdir_in(self.directory.path()).unwrap();
        let (cold, cold_metadata) = self.run(Some(fresh_cache.path()), Some("not-loaded"));
        let (cached, cached_metadata) = self.run(Some(&self.path("incremental")), cache_state);
        for diagnostics in [&reference, &cold, &cached] {
            assert_eq!(
                diagnostics.iter().filter(|value| value["code"]["code"] == LINT).count(),
                warnings
            );
            assert_eq!(
                diagnostics
                    .iter()
                    .filter(|value| value["code"]["code"] == "unfulfilled_lint_expectations")
                    .count(),
                expectations
            );
        }
        let reference = normalize(reference);
        assert_eq!(reference, normalize(cold));
        assert_eq!(reference, normalize(cached));
        // rustc includes owner spans in the crate hash only when incremental is enabled.
        // Compare identical compiler inputs with a cold cache and the retained cache.
        assert!(
            cold_metadata == cached_metadata,
            "incremental lint inputs changed emitted metadata"
        );
    }
}

/// A spanless command-line note may attach to a different warning with parallel linting.
/// Preserve its complete content/count while comparing all other diagnostics and spans.
fn normalize(diagnostics: Vec<Value>) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut diagnostics = diagnostics
        .into_iter()
        .map(|mut value| {
            value.as_object_mut().unwrap().remove("rendered");
            if let Some(children) = value["children"].as_array_mut() {
                children.retain(|child| {
                    if child["level"] == "note"
                        && child["spans"] == serde_json::json!([])
                        && child["message"] == "requested on the command line with `-W clippy::zero-sized-map-values`"
                    {
                        notes.push(child.to_string());
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
