//! Combined lint/codegen mode retains ordinary compiler semantics and standalone lint coverage.

#[cfg(ohm_clippy_codegen)]
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

#[cfg(ohm_clippy_codegen)]
const LINTS: &[&str] = &[
    "redundant_clone",
    "missing_const_for_fn",
    "missing_const_for_thread_local",
    "large_stack_frames",
    "useless_format",
    "useless_borrows_in_formatting",
    "unused_format_specs",
    "uninlined_format_args",
    "format_push_string",
    "recursive_format_impl",
];

fn driver(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_clippy-driver"));
    command
        .current_dir(root)
        .env("CLIPPY_CONF_DIR", root)
        .env_remove("CLIPPY_ARGS")
        .env_remove("CLIPPY_CODEGEN");
    if let Some(sysroot) = option_env!("TEST_SYSROOT") {
        command.arg("--sysroot").arg(sysroot);
    }
    command
}

fn succeeded(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    output
}

#[cfg(ohm_clippy_codegen)]
fn diagnostics(output: &Output) -> Vec<String> {
    let mut diagnostics = String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| {
            message["code"]["code"]
                .as_str()
                .is_some_and(|code| code.starts_with("clippy::"))
        })
        .map(|mut message| {
            message.as_object_mut().unwrap().remove("rendered");
            serde_json::to_string(&message).unwrap()
        })
        .collect::<Vec<_>>();
    diagnostics.sort();
    diagnostics
}

#[test]
fn capability_matches_compiler_support() {
    let output = Command::new(env!("CARGO_BIN_EXE_clippy-driver"))
        .arg("--ohm-clippy-codegen-version")
        .output()
        .unwrap();
    assert_eq!(output.status.success(), cfg!(ohm_clippy_codegen));
    if cfg!(ohm_clippy_codegen) {
        assert_eq!(output.stdout, b"ohm-clippy-codegen-v1\n");
    }
    let ordinary = succeeded(driver(Path::new(".")).args(["--rustc", "-vV"]));
    let bypass = succeeded(
        driver(Path::new("."))
            .env("CLIPPY_CODEGEN", "1")
            .args(["--rustc", "-vV"]),
    );
    assert_eq!(ordinary.stdout, bypass.stdout);
}

#[test]
#[cfg(ohm_clippy_codegen)]
fn lint_diagnostics_and_ordinary_codegen_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.rs"), include_str!("codegen/fixture.rs")).unwrap();
    fs::write(root.path().join("clippy.toml"), "stack-size-threshold = 2048\n").unwrap();
    let common = [
        "main.rs",
        "--crate-name",
        "clippy_codegen_fixture",
        "--edition=2024",
        "-Copt-level=2",
        "-Cdebuginfo=0",
        "--error-format=json",
        "-Adead_code",
    ];
    let lint_args = LINTS.iter().map(|lint| format!("-Wclippy::{lint}")).collect::<Vec<_>>();
    let standalone = succeeded(
        driver(root.path())
            .args(common)
            .arg("-Aclippy::all")
            .args(&lint_args)
            .args(["--crate-type=lib", "--emit=metadata", "-o", "standalone.rmeta"]),
    );
    let combined = succeeded(
        driver(root.path())
            .env("CLIPPY_CODEGEN", "1")
            .args(common)
            .arg("-Aclippy::all")
            .args(&lint_args)
            .args(["--crate-type=lib", "--emit=llvm-ir", "-o", "combined.ll"]),
    );
    let reference = diagnostics(&standalone);
    for lint in LINTS {
        assert!(
            reference
                .iter()
                .any(|message| message.contains(&format!("clippy::{lint}"))),
            "missing fixture coverage for {lint}: {reference:#?}"
        );
    }
    // Standalone Clippy detects direct recursion, but currently skips the nested format_args!
    // call in NestedRecursive. Combined mode must retain both behaviors.
    assert_eq!(
        reference
            .iter()
            .filter(|message| message.contains("clippy::recursive_format_impl"))
            .count(),
        1
    );
    assert_eq!(reference, diagnostics(&combined));
    succeeded(
        driver(root.path())
            .arg("--rustc")
            .env("CLIPPY_CODEGEN", "1")
            .args(common)
            .args(["--crate-type=lib", "--emit=llvm-ir", "-o", "ordinary.ll"]),
    );
    // Export the complete exercise as a library to exclude std's binary entry shim, whose
    // inline-asm source-location metadata depends on the order dependency source files are read.
    let ordinary = fs::read_to_string(root.path().join("ordinary.ll")).unwrap();
    let combined = fs::read_to_string(root.path().join("combined.ll")).unwrap();
    if ordinary != combined {
        let line = ordinary
            .lines()
            .zip(combined.lines())
            .position(|(ordinary, combined)| ordinary != combined)
            .unwrap_or_else(|| ordinary.lines().count().min(combined.lines().count()));
        let root = root.keep();
        panic!(
            "LLVM IR differs at line {}:\nordinary: {}\ncombined: {}\nIR files preserved in {}",
            line + 1,
            ordinary.lines().nth(line).unwrap_or("<end>"),
            combined.lines().nth(line).unwrap_or("<end>"),
            root.display()
        );
    }
    succeeded(
        driver(root.path())
            .env("CLIPPY_CODEGEN", "1")
            .args(common)
            .args(["-Aclippy::all", "-o", "combined"]),
    );
    succeeded(Command::new(root.path().join("combined")).current_dir(root.path()));

    // Normal format lowering removes temporaries before MIR is built. The stack estimate
    // therefore describes the normally lowered program, not standalone Clippy's unflattened one.
    fs::write(root.path().join("clippy.toml"), "stack-size-threshold = 0\n").unwrap();
    let stack_args = ["-Aclippy::all", "-Wclippy::large_stack_frames", "--emit=metadata"];
    let standalone = succeeded(
        driver(root.path())
            .args(common)
            .args(stack_args)
            .args(["-o", "standalone.rmeta"]),
    );
    let combined = succeeded(
        driver(root.path())
            .env("CLIPPY_CODEGEN", "1")
            .args(common)
            .args(stack_args)
            .args(["-o", "combined.rmeta"]),
    );
    assert!(formatting_stack_size(&combined) < formatting_stack_size(&standalone));
}

#[cfg(ohm_clippy_codegen)]
fn formatting_stack_size(output: &Output) -> u64 {
    diagnostics(output)
        .into_iter()
        .map(|message| serde_json::from_str::<serde_json::Value>(&message).unwrap())
        .find(|message| {
            message["code"]["code"] == "clippy::large_stack_frames"
                && message["spans"].as_array().unwrap().iter().any(|span| {
                    span["is_primary"] == true
                        && span["text"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|line| line["text"].as_str().unwrap().contains("fn formatting_frame("))
                })
        })
        .unwrap()["message"]
        .as_str()
        .unwrap()
        .strip_prefix("this function may allocate ")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}
