use std::path::PathBuf;
use std::process::Command;
use test_utils::{CARGO_CLIPPY_PATH, IS_RUSTC_TEST_SUITE};

mod test_utils;

#[test]
fn clippy_rechecks_changed_argument_files() {
    if IS_RUSTC_TEST_SUITE {
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"argfile\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { return 7; }").unwrap();
    // Cargo starts with deny, then the build script selects allow for this compilation.
    std::fs::write(
        root.join("build.rs"),
        r#"fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    std::fs::write("lint.flags", "-Aclippy::needless_return\n").unwrap();
}"#,
    )
    .unwrap();
    let run = || {
        Command::new(&*CARGO_CLIPPY_PATH)
            .current_dir(root)
            .env("CARGO_TARGET_DIR", root.join("target"))
            .args(["clippy", "--offline", "--lib", "--", "@lint.flags"])
            .output()
            .unwrap()
    };
    std::fs::write(root.join("lint.flags"), "-Dclippy::needless_return\n").unwrap();
    let original_mtime = std::fs::metadata(root.join("lint.flags")).unwrap().modified().unwrap();
    let initial = run();
    assert!(initial.status.success(), "{}", String::from_utf8_lossy(&initial.stderr));
    std::fs::write(root.join("lint.flags"), "-Dclippy::needless_return\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(root.join("lint.flags"))
        .unwrap()
        .set_modified(original_mtime)
        .unwrap();
    let changed = run();
    assert!(!changed.status.success());
    assert!(String::from_utf8_lossy(&changed.stderr).contains("needless_return"));
}

#[cfg(unix)]
#[test]
fn non_unicode_argument_is_reported() {
    use std::os::unix::ffi::OsStrExt as _;

    if IS_RUSTC_TEST_SUITE {
        return;
    }
    let output = Command::new(&*CARGO_CLIPPY_PATH)
        .args(["clippy", "--target-dir"])
        .arg(std::ffi::OsStr::from_bytes(b"out-\xff"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("invalid UTF-8"), "{stderr}");
}

#[test]
fn test_module_style_with_dep_in_subdir() {
    if IS_RUSTC_TEST_SUITE {
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = root.join("target").join("workspace_test");
    let cwd = root.join("tests/workspace_test");

    // Make sure we start with a clean state
    Command::new("cargo")
        .current_dir(&cwd)
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("clean")
        .args(["-p", "pass-no-mod-with-dep-in-subdir"])
        .args(["-p", "pass-mod-with-dep-in-subdir"])
        .output()
        .unwrap();

    // [#8887](https://github.com/rust-lang/rust-clippy/issues/8887)
    // `mod.rs` checks should not be applied to crate dependencies
    // located in the subdirectory of workspace
    let output = Command::new(&*CARGO_CLIPPY_PATH)
        .current_dir(&cwd)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("clippy")
        .args(["-p", "pass-no-mod-with-dep-in-subdir"])
        .args(["-p", "pass-mod-with-dep-in-subdir"])
        .arg("--")
        .arg("-Cdebuginfo=0") // disable debuginfo to generate less data in the target dir
        .output()
        .unwrap();

    println!("status: {}", output.status);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success());
}

#[test]
fn test_no_deps_ignores_path_deps_in_workspaces() {
    if IS_RUSTC_TEST_SUITE {
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = root.join("target").join("workspace_test");
    let cwd = root.join("tests/workspace_test");

    // Make sure we start with a clean state
    Command::new("cargo")
        .current_dir(&cwd)
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("clean")
        .args(["-p", "subcrate"])
        .args(["-p", "path_dep"])
        .output()
        .unwrap();

    // `path_dep` is a path dependency of `subcrate` that would trigger a denied lint.
    // Make sure that with the `--no-deps` argument Clippy does not run on `path_dep`.
    let output = Command::new(&*CARGO_CLIPPY_PATH)
        .current_dir(&cwd)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("clippy")
        .args(["-p", "subcrate"])
        .arg("--no-deps")
        .arg("--")
        .arg("-Cdebuginfo=0") // disable debuginfo to generate less data in the target dir
        .args(["--cfg", r#"feature="primary_package_test""#])
        .output()
        .unwrap();
    println!("status: {}", output.status);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success());

    let lint_path_dep = || {
        // Test that without the `--no-deps` argument, `path_dep` is linted.
        let output = Command::new(&*CARGO_CLIPPY_PATH)
            .current_dir(&cwd)
            .env("CARGO_INCREMENTAL", "0")
            .env("CARGO_TARGET_DIR", &target_dir)
            .arg("clippy")
            .args(["-p", "subcrate"])
            .arg("--")
            .arg("-Cdebuginfo=0") // disable debuginfo to generate less data in the target dir
            .args(["--cfg", r#"feature="primary_package_test""#])
            .output()
            .unwrap();
        println!("status: {}", output.status);
        println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("error: empty `loop {}` wastes CPU cycles")
        );
    };

    // Make sure Cargo is aware of the removal of `--no-deps`.
    lint_path_dep();

    let successful_build = || {
        let output = Command::new(&*CARGO_CLIPPY_PATH)
            .current_dir(&cwd)
            .env("CARGO_INCREMENTAL", "0")
            .env("CARGO_TARGET_DIR", &target_dir)
            .arg("clippy")
            .args(["-p", "subcrate"])
            .arg("--")
            .arg("-Cdebuginfo=0") // disable debuginfo to generate less data in the target dir
            .output()
            .unwrap();
        println!("status: {}", output.status);
        println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

        assert!(output.status.success());

        output
    };

    // Trigger a successful build, so Cargo would like to cache the build result.
    successful_build();

    // Make sure there's no spurious rebuild when nothing changes.
    let stderr = String::from_utf8(successful_build().stderr).unwrap();
    assert!(!stderr.contains("Compiling"));
    assert!(!stderr.contains("Checking"));
    assert!(stderr.contains("Finished"));

    // Make sure Cargo is aware of the new `--cfg` flag.
    lint_path_dep();
}

#[test]
fn clippy_preserves_compiler_arguments() {
    if IS_RUSTC_TEST_SUITE {
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'clippy-args'\nversion = '0.1.0'\nedition = '2024'\n[lib]\npath = 'lib.rs'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("lib.rs"),
        r#"#![allow(unexpected_cfgs)]
#[cfg(not(clippy))]
compile_error!("lost Clippy configuration");
#[cfg(not(payload = "left__CLIPPY_HACKERY__right"))]
compile_error!("lost compiler argument");
"#,
    )
    .unwrap();
    let run = |extra: &[&str]| {
        Command::new(&*CARGO_CLIPPY_PATH)
            .current_dir(root)
            .env("CARGO_TARGET_DIR", root.join("target"))
            .args([
                "clippy",
                "--offline",
                "--",
                "--cfg",
                r#"payload="left__CLIPPY_HACKERY__right""#,
            ])
            .args(extra)
            .output()
            .unwrap()
    };
    let control = run(&[]);
    assert!(control.status.success(), "{}", String::from_utf8_lossy(&control.stderr));
    let terminated = run(&["--"]);
    assert!(
        terminated.status.success(),
        "{}",
        String::from_utf8_lossy(&terminated.stderr)
    );
    let empty = run(&[""]);
    let stderr = String::from_utf8_lossy(&empty.stderr);
    assert!(
        !empty.status.success() && stderr.contains("multiple input filenames"),
        "{stderr}"
    );

    std::fs::write(root.join("lib.rs"), "const BAD: u8 = false;").unwrap();
    for path in ["--help", "--version", "--explain"] {
        std::fs::create_dir(root.join(path)).unwrap();
        let output = run(&["-L", path]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success() && stderr.contains("E0308"), "{path}: {stderr}");
    }

    for path in ["--help", "--version", "--rustc"] {
        let output = Command::new(env!("CARGO_BIN_EXE_clippy-driver"))
            .current_dir(root)
            .args(["--crate-type=lib", "--emit=metadata", "lib.rs", "-L", path])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success() && stderr.contains("E0308"), "{path}: {stderr}");
    }
}
