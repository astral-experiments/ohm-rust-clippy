fn main() {
    // Don't rebuild even if nothing changed
    println!("cargo:rerun-if-changed=build.rs");
    rustc_tools_util::setup_version_info!();
    println!("cargo:rustc-check-cfg=cfg(ohm_incremental_driver_hash)");
    println!("cargo:rustc-check-cfg=cfg(ohm_source_file_hash)");
    println!("cargo:rustc-check-cfg=cfg(ohm_driver_read_checksum)");
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    probe(
        "incremental_driver_hash",
        "extern crate rustc_session;\npub fn probe(options: &mut rustc_session::config::Options) { options.incremental_driver_hash = None; }\n",
    );
    probe(
        "source_file_hash",
        "extern crate rustc_middle;\nextern crate rustc_span;\npub fn probe(tcx: rustc_middle::ty::TyCtxt<'_>, id: rustc_span::StableSourceFileId) { let _ = tcx.source_file_hash(id); }\n",
    );
    probe(
        "driver_read_checksum",
        "extern crate rustc_session;\nextern crate rustc_span;\npub fn probe(sess: &rustc_session::Session, path: rustc_span::Symbol, sum: rustc_span::SourceFileHash) { let _ = &sess.opts.incremental_driver_hash; sess.file_depinfo.borrow_mut().record_read(path, 0, sum); }\n",
    );
    println!("cargo:rustc-check-cfg=cfg(ohm_verified_driver_checksum)");
    probe(
        "verified_driver_checksum",
        "extern crate rustc_span;\npub fn probe(bytes: [u8; 32]) -> rustc_span::SourceFileHash { rustc_span::SourceFileHash::from_blake3(bytes) }\n",
    );
}

/// Keep the fork buildable with upstream rustc, using incremental lints only when the
/// compiler can track driver inputs independently of crate identity.
fn probe(name: &str, body: &str) {
    use std::path::PathBuf;
    use std::process::Command;
    use std::{env, fs};

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = out.join(format!("{name}_probe.rs"));
    fs::write(&source, format!("#![feature(rustc_private)]\n{body}")).unwrap();
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
            let _ = fs::write(out.join(format!("{name}_probe.stderr")), output.stderr);
            output.status.success()
        },
        Err(_) => false,
    };
    if available {
        println!("cargo:rustc-cfg=ohm_{name}");
    }
}
