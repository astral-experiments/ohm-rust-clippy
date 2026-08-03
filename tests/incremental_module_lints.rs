use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

#[test]
fn module_lints_replay_without_suppressing_whole_crate_side_effects() -> Result<(), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("ohm-clippy-module-lints-{}", std::process::id()));
    fs::create_dir_all(&root)?;

    fs::write(root.join("lib.rs"), "#![allow(dead_code)]\nmod edited;\nmod stable;\n")?;
    fs::write(root.join("edited.rs"), "pub fn edited() -> usize { 1 }\n")?;
    fs::write(
        root.join("stable.rs"),
        "pub fn stable() {\n    #[clippy::author]\n    let value = 0.0_f32 / 0.0;\n    let _ = value;\n}\n",
    )?;

    let sysroot = Command::new("rustc").arg("--print=sysroot").output()?;
    assert!(sysroot.status.success());
    let sysroot = String::from_utf8(sysroot.stdout)?;

    let first = run_clippy(&root, sysroot.trim())?;
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert!(String::from_utf8_lossy(&first.stderr).contains("constant division of `0.0` with `0.0`"));
    assert!(String::from_utf8_lossy(&first.stdout).contains("// report your lint here"));

    fs::write(root.join("edited.rs"), "pub fn edited() -> usize { 2 }\n")?;
    let replayed = run_clippy(&root, sysroot.trim())?;
    assert!(
        replayed.status.success(),
        "{}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert!(String::from_utf8_lossy(&replayed.stderr).contains("constant division of `0.0` with `0.0`"));
    assert!(String::from_utf8_lossy(&replayed.stdout).contains("// report your lint here"));

    fs::write(
        root.join("stable.rs"),
        "pub fn stable() {\n    #[clippy::author]\n    let value = 1.0_f32 / 1.0;\n    let _ = value;\n}\n",
    )?;
    let invalidated = run_clippy(&root, sysroot.trim())?;
    assert!(
        invalidated.status.success(),
        "{}",
        String::from_utf8_lossy(&invalidated.stderr)
    );
    assert!(!String::from_utf8_lossy(&invalidated.stderr).contains("constant division of `0.0` with `0.0`"));
    assert!(String::from_utf8_lossy(&invalidated.stdout).contains("// report your lint here"));

    fs::remove_dir_all(root)?;
    Ok(())
}

fn run_clippy(root: &Path, sysroot: &str) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_clippy-driver"))
        .current_dir(root)
        .args([
            "--crate-name",
            "ohm_module_lints",
            "--crate-type",
            "lib",
            "--edition",
            "2024",
            "--sysroot",
            sysroot,
            "--emit=metadata",
            "-A",
            "clippy::all",
            "-W",
            "clippy::zero_divided_by_zero",
        ])
        .arg(format!("-Cincremental={}", root.join("incremental").display()))
        .arg("lib.rs")
        .output()?)
}
