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

    // Cargo can replay the previous check's diagnostics even when only the message format changes.
    clippy(&[]);
    for args in [
        &["--message-format", "json-diagnostic-rendered-ansi"][..],
        &["--message-format=json"][..],
    ] {
        let output = clippy(args);
        let messages: Vec<serde_json::Value> = output.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let lints: Vec<_> = messages
            .iter()
            .filter_map(|m| m["message"]["code"]["code"].as_str())
            .filter(|lint| lint.starts_with("clippy::"))
            .collect();
        assert_eq!(lints, ["clippy::bool_comparison", "clippy::unnecessary_literal_unwrap"]);
    }

    clippy(&[]);
    clippy(&["--fix", "--allow-no-vcs"]);
    assert_eq!(
        fs::read_to_string(source).unwrap(),
        "pub fn f(a: bool, b: bool) -> bool { !(a & b) }\n"
    );
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
