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

#[test]
fn automatic_fixes_preserve_suggestion_order_after_a_regular_check() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"fix-order\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )
    .unwrap();
    let source = root.path().join("src/lib.rs");
    fs::write(
        &source,
        "pub fn f(a: bool, b: bool) -> bool { Some(a & b).unwrap() == false }\n",
    )
    .unwrap();

    let clippy = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-clippy"));
        configure_cargo(&mut command, root.path());
        let output = command
            .args(["clippy", "--lib", "--offline"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };

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
