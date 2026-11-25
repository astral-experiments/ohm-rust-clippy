//! Release drivers must participate in Cargo's ordinary file freshness checks.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs;
use std::path::Path;
use std::process::Command;

/// Run Cargo against an isolated target directory with the compiler used to build the driver.
fn configure_cargo(command: &mut Command, root: &Path) {
    command
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_INCREMENTAL", "1")
        .env("CARGO_BUILD_WARNINGS", "warn")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_BUILD_DIR");
    if let Some(rustc) = option_env!("TEST_RUSTC") {
        command.env("RUSTC", rustc);
    }
    if let Some(sysroot) = option_env!("TEST_SYSROOT") {
        command.env("SYSROOT", sysroot);
    }
}

fn cargo_check(root: &Path, driver: &Path) -> String {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    configure_cargo(&mut command, root);
    command
        .args(["check", "--offline", "--verbose"])
        .env("RUSTC_WORKSPACE_WRAPPER", driver)
        .env("CLIPPY_ARGS", "-Wclippy::zero_sized_map_values__CLIPPY_HACKERY__");
    let output = command.output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    stderr
}

/// Create an isolated library for exercising Cargo's automatic fixes.
fn fix_project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"fix-order\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )
    .unwrap();
    root
}

/// Run the test driver through Cargo, returning its diagnostic stream.
fn run_clippy(root: &Path, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-clippy"));
    configure_cargo(&mut command, root);
    let output = command
        .args(["clippy", "--lib", "--offline"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// Add a local `keys` dependency to the test project.
fn keys_project() -> tempfile::TempDir {
    let root = fix_project();
    let dir = root.path();
    let manifest = dir.join("Cargo.toml");
    let mut contents = fs::read_to_string(&manifest).unwrap();
    contents.push_str("\n[dependencies]\nkeys = { path = \"keys\" }\n");
    fs::write(manifest, contents).unwrap();

    fs::create_dir_all(dir.join("keys/src")).unwrap();
    fs::write(
        dir.join("keys/Cargo.toml"),
        "[package]\nname = \"keys\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    root
}

#[test]
fn configured_paths_match_public_reexports_from_private_modules() {
    let root = keys_project();
    let dir = root.path();
    fs::write(
        dir.join("keys/src/lib.rs"),
        "mod raw { pub struct Token; impl Token { pub fn use_it(&self) {} } }\n\
         pub use raw::Token as Handle;\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "#![warn(clippy::missing_enforced_import_renames)]\n\
         use keys::Handle;\n\
         pub fn take(key: Handle) { key.use_it(); }\n",
    )
    .unwrap();

    for path in ["keys::raw::Token", "keys::Handle"] {
        fs::write(
            dir.join("clippy.toml"),
            format!(
                "disallowed-types = [\"{path}\"]\n\
                 disallowed-methods = [\"{path}::use_it\"]\n\
                 enforced-import-renames = [{{ path = \"{path}\", rename = \"Key\" }}]\n"
            ),
        )
        .unwrap();
        let output = run_clippy(dir, &["--no-deps", "--message-format=json"]);
        assert!(output.contains("clippy::disallowed_types"), "{output}");
        assert!(output.contains("clippy::disallowed_methods"), "{output}");
        assert!(output.contains("clippy::missing_enforced_import_renames"), "{output}");
    }
}

#[test]
fn configured_paths_match_private_library_items_in_the_binary() {
    let root = fix_project();
    let dir = root.path();
    fs::write(dir.join("src/main.rs"), "fn main() { fix_order::run(); }\n").unwrap();
    fs::write(
        dir.join("clippy.toml"),
        "disallowed-methods = [\"fix_order::legacy\"]\n",
    )
    .unwrap();

    for (item, warning) in [
        ("fn legacy() {}", None),
        ("", Some("does not refer to a reachable function")),
        ("struct legacy;", Some("expected a function, found a struct")),
        ("struct legacy(u32);", None),
        ("fn legacy() {}", None),
    ] {
        let called = item.starts_with("fn");
        let call = if called { "legacy();" } else { "" };
        let source = format!("#![allow(non_camel_case_types, unused)]\n{item}\npub fn run() {{ {call} }}\n");
        fs::write(dir.join("src/lib.rs"), source).unwrap();
        let output = run_clippy(dir, &["--bins", "--message-format=json"]);
        let warnings: Vec<_> = output
            .lines()
            .filter(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                event["target"]["kind"][0] == "bin" && event["message"]["level"] == "warning"
            })
            .collect();
        assert_eq!(output.contains("use of a disallowed method"), called, "{output}");
        assert_eq!(warnings.len(), usize::from(warning.is_some()), "{output}");
        if let Some(warning) = warning {
            assert!(warnings[0].contains(warning), "{output}");
        }
    }
}

#[test]
fn configured_paths_follow_changes_to_hidden_imports() {
    let root = keys_project();
    let dir = root.path();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn take(key: keys::Alpha) { key.use_it(); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("clippy.toml"),
        "disallowed-types = [\"keys::policy::Restricted\"]\n\
         disallowed-methods = [\"keys::policy::Restricted::use_it\"]\n",
    )
    .unwrap();

    // Only the contents of the unexported module change between checks.
    for (binding, banned) in [
        ("", false),
        ("pub use super::Alpha as Restricted;", true),
        ("pub use super::Bravo as Restricted;", false),
        ("pub use super::Alpha as Restricted;", true),
        ("pub(crate) use super::Alpha as Restricted;", true),
        ("use super::Bravo as Restricted;", false),
        ("struct Hidden; use Hidden as Restricted;", false),
        ("trait Hidden { fn use_it(&self); } use Hidden as Restricted;", false),
        ("use super::Alpha as Restricted;", true),
    ] {
        fs::write(
            dir.join("keys/src/lib.rs"),
            format!(
                "pub struct Alpha; impl Alpha {{ pub fn use_it(&self) {{}} }}\n\
                 pub struct Bravo; impl Bravo {{ pub fn use_it(&self) {{}} }}\n\
                 #[allow(unused_imports)] mod policy {{ {binding} }}\n"
            ),
        )
        .unwrap();
        let output = run_clippy(dir, &["--no-deps", "--message-format=json"]);
        assert_eq!(output.contains("clippy::disallowed_types"), banned, "{output}");
        assert_eq!(output.contains("clippy::disallowed_methods"), banned, "{output}");
    }
}

#[test]
fn configured_type_follows_a_module_replaced_by_an_import() {
    let root = keys_project();
    let dir = root.path();
    fs::write(dir.join("src/lib.rs"), "pub fn take(_: keys::Key) {}\n").unwrap();
    fs::write(
        dir.join("clippy.toml"),
        "disallowed-types = [{ path = \"keys::banned\", allow-invalid = true }]\n",
    )
    .unwrap();

    for (binding, banned) in [
        ("mod banned {}", false),
        ("#[allow(unused_imports)] use Key as banned;", true),
        ("mod banned {}", false),
    ] {
        fs::write(dir.join("keys/src/lib.rs"), format!("pub struct Key;\n{binding}\n")).unwrap();
        let output = run_clippy(dir, &["--no-deps", "--message-format=json"]);
        assert_eq!(output.contains("clippy::disallowed_types"), banned, "{output}");
    }
}

#[test]
fn ignored_interior_mutability_follows_private_field_types() {
    let root = keys_project();
    let dir = root.path();
    fs::write(
        dir.join("clippy.toml"),
        "ignore-interior-mutability = [\"keys::Counter\"]\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "#![warn(clippy::mutable_key_type)]\npub fn take(_: std::collections::HashMap<keys::Key, u32>) {}\n",
    )
    .unwrap();

    for field in ["Counter", "Other", "Counter"] {
        fs::write(
            dir.join("keys/src/lib.rs"),
            format!("#![allow(dead_code)]\nstruct {field}(std::cell::Cell<u32>);\npub struct Key({field});\n"),
        )
        .unwrap();
        let output = run_clippy(dir, &["--message-format=json", "--no-deps"]);
        assert_eq!(
            output.contains("clippy::mutable_key_type"),
            field == "Other",
            "{output}"
        );
    }
}

#[test]
fn unwrap_setting_follows_private_type_aliases() {
    let root = fix_project();
    let dir = root.path();
    fs::write(dir.join("clippy.toml"), "allow-unwrap-types = [\"fix_order::Reply\"]\n").unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "#![warn(clippy::unwrap_used)]\nfn main() { let _ = fix_order::input().unwrap(); }\n",
    )
    .unwrap();

    for (declaration, allowed) in [
        ("type Reply = Option<u32>;", true),
        ("type Reply = Option<u64>;", false),
        ("type Reply = Option<u32>;", true),
        ("type Inner = Option<u32>; use Inner as Reply;", true),
        ("type Inner = Option<u64>; use Inner as Reply;", false),
    ] {
        fs::write(
            dir.join("src/lib.rs"),
            format!("{declaration}\npub fn input() -> Option<u32> {{ let _: Reply = None; Some(42) }}\n"),
        )
        .unwrap();
        let output = run_clippy(dir, &["--bins", "--message-format=json"]);
        assert_eq!(output.contains("clippy::unwrap_used"), !allowed, "{output}");
    }
}

#[test]
fn automatic_fixes_preserve_suggestion_order_after_a_regular_check() {
    let root = fix_project();
    let source = root.path().join("src/lib.rs");
    fs::write(
        &source,
        "pub fn f(a: bool, b: bool) -> bool { Some(a & b).unwrap() == false }\n",
    )
    .unwrap();

    let clippy = |args: &[&str]| run_clippy(root.path(), args);

    let lint_codes = |args: &[&str]| {
        clippy(args)
            .lines()
            .filter_map(|line| {
                let m: serde_json::Value = serde_json::from_str(line).unwrap();
                let code = m["message"]["code"]["code"].as_str()?;
                code.starts_with("clippy::").then(|| code.to_owned())
            })
            .collect::<Vec<_>>()
    };

    // Cargo can replay the previous check's diagnostics even when only the message format changes.
    clippy(&[]);
    for args in [
        &["--message-format", "json-diagnostic-rendered-ansi"][..],
        &["--message-format=json"][..],
    ] {
        assert_eq!(
            lint_codes(args),
            ["clippy::bool_comparison", "clippy::unnecessary_literal_unwrap"]
        );
    }

    clippy(&[]);
    clippy(&["--fix", "--allow-no-vcs"]);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "pub fn f(a: bool, b: bool) -> bool { !(a & b) }\n"
    );

    // Fixers can group diagnostics across targets before choosing which edit to apply.
    fs::write(
        &source,
        "#![warn(clippy::equatable_if_let)]\n\
         pub fn f(a: bool, b: bool) -> bool { if let false = Some(a | b).unwrap() { true } else { false } }\n",
    )
    .unwrap();
    assert_eq!(
        lint_codes(&["--message-format=json"]),
        [
            "clippy::match_like_matches_macro",
            "clippy::redundant_pattern_matching",
            "clippy::equatable_if_let",
            "clippy::unnecessary_literal_unwrap",
        ]
    );

    // A different mix of targets can apply the negation before removing the unwrap.
    fs::write(
        &source,
        "pub fn f(a: bool, b: bool) -> bool { !Some(a | b).unwrap() }\n",
    )
    .unwrap();
    clippy(&["--fix", "--allow-no-vcs"]);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "pub fn f(a: bool, b: bool) -> bool { !(a | b) }\n"
    );

    // The call's outer parentheses belong to the replaced span on both sides.
    fs::write(
        &source,
        "pub fn f(a: bool, b: bool) -> bool { (Some(a | b).unwrap()) & !a }\n",
    )
    .unwrap();
    clippy(&["--fix", "--allow-no-vcs"]);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "pub fn f(a: bool, b: bool) -> bool { (a | b) & !a }\n"
    );

    // Swapping the branches after the unwrap must preserve the grouping on the left of &&.
    fs::write(
        &source,
        "#![warn(clippy::if_not_else)]\n\
         pub fn f(a: bool, b: bool, c: bool) -> bool { (if !(a ^ b) { b } else { a }) && c }\n",
    )
    .unwrap();
    clippy(&["--fix", "--allow-no-vcs"]);
    let fixed = fs::read_to_string(&source).unwrap();
    assert!(fixed.contains("{ a } else { b }) && c"), "{fixed}");
}

#[test]
fn literal_unwrap_fixes_keep_only_required_grouping() {
    let root = fix_project();
    let source = root.path().join("src/lib.rs");
    fs::write(
        &source,
        r"#![deny(unused_parens)]
#![allow(clippy::all)]
#![warn(clippy::unnecessary_literal_unwrap)]
pub fn f(a: u8, b: u8) -> u8 {
    let x = std::hint::black_box(Some(a | b).unwrap());
    Some(a ^ b).unwrap().wrapping_add(Some(x & b).unwrap())
}
#[allow(irrefutable_let_patterns)]
pub fn check(a: bool, b: bool) {
    struct Mask { bit: bool }
    if Some(Mask { bit: a }.bit | b).unwrap() {}
    if Some(Mask { bit: a }.bit).unwrap() {}
    match Some(Mask { bit: a }).unwrap() { _ => () }
    if let true = Some(a || b).unwrap() {}
    if let read = Some(|| Mask { bit: a }).unwrap() { let _ = read().bit; }
    while let read = Some(|| a).unwrap() { let _ = read(); break; }
    if let read = Some(|| a).unwrap() && read() {}
    if let read = Some(async || { a as u8 } + 1).unwrap() { let _ = read(); }
    if let range = Some(|| { a as u8 }..3).unwrap() { let _ = range().end; }
    match Some(a | b).unwrap() { _ => () }
    for _ in Some(&[a, b]).unwrap() {}
}
pub fn closures() -> impl Fn() -> u8 {
    let first;
    first = Some(|| 1u8).unwrap();
    let second = || Some(|| 2u8).unwrap();
    let third = loop { break Some(|| 3u8).unwrap(); };
    let last = Some(|| 4u8).unwrap()();
    return Some(move || first() + second()() + third() + last).unwrap();
}
pub async fn read(a: bool, b: bool) -> Result<(), ()> {
    struct Mask { bit: bool }
    impl Mask {
        async fn read(self) -> Result<bool, ()> { Ok(self.bit) }
    }
    if let true = Some(Mask { bit: a }.read().await? == b).unwrap() && a {}
    if Some(Mask { bit: a }.read().await?).unwrap() {}
    Ok(())
}
",
    )
    .unwrap();
    run_clippy(root.path(), &["--fix", "--allow-no-vcs"]);
    let fixed = fs::read_to_string(&source).unwrap();
    assert!(!fixed.contains(".unwrap()"), "{fixed}");
    assert!(fixed.contains("black_box(a | b)"), "{fixed}");
    assert!(fixed.contains("(a ^ b).wrapping_add(x & b)"), "{fixed}");
    run_clippy(root.path(), &[]);
}

fn compiled_fixture(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|line| line.contains("Running `") && line.contains("--crate-name driver_freshness_fixture"))
}

#[test]
fn replacing_a_same_version_driver_reruns_cargo() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"driver-freshness-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "pub type Map = std::collections::HashMap<u8, ()>;\n",
    )
    .unwrap();
    let driver = root.path().join("clippy-driver");
    fs::copy(env!("CARGO_BIN_EXE_clippy-driver"), &driver).unwrap();
    fs::File::open(&driver)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    let version = Command::new(&driver).arg("--version").output().unwrap();
    assert!(version.status.success());

    assert!(compiled_fixture(&cargo_check(root.path(), &driver)));
    assert!(!compiled_fixture(&cargo_check(root.path(), &driver)));

    let replacement = root.path().join("replacement");
    fs::copy(env!("CARGO_BIN_EXE_clippy-driver"), &replacement).unwrap();
    #[cfg(target_os = "linux")]
    {
        use std::io::Write as _;
        // Non-loadable ELF bytes preserve behavior/version but change the driver hash.
        fs::OpenOptions::new()
            .append(true)
            .open(&replacement)
            .unwrap()
            .write_all(b"clippy-freshness-test")
            .unwrap();
    }
    // macOS's fs::copy preserves the original driver's modification time. Model an
    // actual newly built replacement, whose timestamp is later than the last check.
    fs::File::open(&replacement)
        .unwrap()
        .set_modified(std::time::SystemTime::now())
        .unwrap();
    fs::rename(replacement, &driver).unwrap();
    let replaced_version = Command::new(&driver).arg("--version").output().unwrap();
    assert!(replaced_version.status.success());
    assert_eq!(version.stdout, replaced_version.stdout);
    let replaced_check = cargo_check(root.path(), &driver);
    assert!(compiled_fixture(&replaced_check), "{replaced_check}");
    assert!(!compiled_fixture(&cargo_check(root.path(), &driver)));
}
