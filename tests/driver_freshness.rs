//! Release drivers must participate in Cargo's ordinary file freshness checks.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs;
use std::path::Path;
use std::process::Command;

fn cargo_check(root: &Path, driver: &Path) -> String {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .current_dir(root)
        .args(["check", "--offline", "--verbose"])
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_INCREMENTAL", "1")
        .env("CARGO_BUILD_WARNINGS", "warn")
        .env("RUSTC_WORKSPACE_WRAPPER", driver)
        .env("CLIPPY_ARGS", "-Wclippy::zero_sized_map_values__CLIPPY_HACKERY__")
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
    let output = command.output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    stderr
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
    filetime::set_file_mtime(&driver, filetime::FileTime::from_unix_time(1, 0)).unwrap();
    let version = Command::new(&driver).arg("--version").output().unwrap();
    assert!(version.status.success());

    assert!(compiled_fixture(&cargo_check(root.path(), &driver)));
    assert!(!compiled_fixture(&cargo_check(root.path(), &driver)));

    let replacement = root.path().join("replacement");
    fs::copy(env!("CARGO_BIN_EXE_clippy-driver"), &replacement).unwrap();
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        // Non-loadable ELF bytes preserve behavior/version but change the driver hash.
        fs::OpenOptions::new()
            .append(true)
            .open(&replacement)
            .unwrap()
            .write_all(b"clippy-freshness-test")
            .unwrap();
    }
    fs::rename(replacement, &driver).unwrap();
    let replaced_version = Command::new(&driver).arg("--version").output().unwrap();
    assert!(replaced_version.status.success());
    assert_eq!(version.stdout, replaced_version.stdout);
    assert!(compiled_fixture(&cargo_check(root.path(), &driver)));
    assert!(!compiled_fixture(&cargo_check(root.path(), &driver)));
}
