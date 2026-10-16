fn main() {
    // Don't rebuild even if nothing changed
    println!("cargo:rerun-if-changed=build.rs");
    rustc_tools_util::setup_version_info!();
    println!("cargo:rustc-check-cfg=cfg(ohm_incremental_driver_hash)");
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    probe_incremental_driver_hash();
}

/// Keep the fork buildable with upstream rustc, using incremental lints only when the
/// compiler can track driver inputs independently of crate identity.
fn probe_incremental_driver_hash() {
    use std::path::PathBuf;
    use std::process::Command;
    use std::{env, fs};

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = out.join("incremental_driver_hash_probe.rs");
    fs::write(&source, "#![feature(rustc_private)]\nextern crate rustc_session;\npub fn probe(options: &mut rustc_session::config::Options) { options.incremental_driver_hash = None; }\n").unwrap();
    let available = Command::new(env::var_os("RUSTC").unwrap())
        // Bootstrap can supply the rustc-private library search path through these flags.
        .args(
            env::var("CARGO_ENCODED_RUSTFLAGS")
                .unwrap_or_default()
                .split('\u{1f}')
                .filter(|flag| !flag.is_empty()),
        )
        .arg(&source)
        // Bootstrap selects its compiler-library sysroot only for target compilations.
        .arg("--target")
        .arg(env::var_os("TARGET").unwrap())
        .args([
            "--crate-name",
            "clippy_incremental_driver_hash_probe",
            "--crate-type",
            "lib",
            "--emit",
            "metadata",
        ])
        .arg("--out-dir")
        .arg(&out)
        .output();
    let available = match available {
        Ok(output) => {
            // Retain the reason for fallback in the build directory, without adding noise
            // to ordinary builds with an upstream compiler.
            let _ = fs::write(out.join("incremental_driver_hash_probe.stderr"), output.stderr);
            output.status.success()
        },
        Err(_) => false,
    };
    if available {
        println!("cargo:rustc-cfg=ohm_incremental_driver_hash");
    }
}
