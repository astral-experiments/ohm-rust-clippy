//! Driver inputs for rustc's ordinary incremental cache. No separate cache directories or
//! crate-identity changes are needed: changed inputs invalidate the previous query graph.

use std::fs::File;
use std::hash::{Hash as _, Hasher as _};
use std::io::{self, Read as _};

use rustc_data_structures::fingerprint::Fingerprint;
use rustc_data_structures::stable_hash::StableHasher;

mod identity;

/// Content identity for the query graph and, when supported, the matching dep-info read.
pub(super) struct DriverInput {
    fingerprint: Fingerprint,
    #[cfg(ohm_driver_read_checksum)]
    pub(super) observed: Option<DriverRead>,
}

#[cfg(ohm_driver_read_checksum)]
pub(super) struct DriverRead {
    pub(super) path: String,
    pub(super) len: u64,
    pub(super) checksum: rustc_span::SourceFileHash,
}

/// Hash the actual executing image and reject changes while its bytes are being read.
fn fingerprint_file(mut file: File, path: &str, want_checksum: bool) -> io::Result<DriverInput> {
    let before = file.metadata()?;
    let mut hasher = StableHasher::new();
    before.len().hash(&mut hasher);
    let mut checksum = None;
    if want_checksum {
        // Bind the name to the executing image before reading it. A later replacement must
        // leave the old checksum in dep-info so that Cargo detects the new contents.
        if !identity::unchanged(&before, &std::fs::metadata(path)?) {
            return Err(io::Error::other(
                "Clippy's executable path no longer identifies the running image",
            ));
        }
        let digest = rustc_span::SourceFileHash::new(rustc_span::SourceFileHashAlgorithm::Blake3, &mut file)?;
        hasher.write(digest.hash_bytes());
        checksum = Some(digest);
    } else {
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let size = file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            hasher.write(&buffer[..size]);
        }
    }
    if !identity::unchanged(&before, &file.metadata()?) {
        return Err(io::Error::other("Clippy executable changed while being read"));
    }
    #[cfg(not(ohm_driver_read_checksum))]
    let _ = checksum;
    Ok(DriverInput {
        fingerprint: hasher.finish(),
        #[cfg(ohm_driver_read_checksum)]
        observed: checksum.map(|checksum| DriverRead {
            path: path.to_owned(),
            len: before.len(),
            checksum,
        }),
    })
}

#[cfg(ohm_incremental_driver_hash)]
mod configuration {
    use std::env;

    use rustc_interface::interface::Config;

    use super::*;

    /// Enable only audited module passes after all non-query inputs have a compatible key.
    pub(crate) fn configure(
        config: &mut Config,
        lookup: &clippy_config::ConfigSnapshot,
        args: Option<&str>,
    ) -> Option<DriverInput> {
        if config.opts.incremental.is_none() || config.file_loader.is_some() {
            return None;
        }
        if !clippy_lints::incremental_lints::enabled(&config.opts) {
            return None;
        }
        if lookup.has_errors() {
            return None;
        }
        // Cargo's dep-info can only represent UTF-8 paths. If it cannot track replacement
        // of this driver, retain the whole-crate pass instead of adding a cached lint.
        let path = env::current_exe().ok()?;
        let path = path.to_str()?;
        let want_checksum = cfg!(ohm_driver_read_checksum)
            && config
                .opts
                .output_types
                .contains_key(&rustc_session::config::OutputType::DepInfo)
            && config.opts.unstable_opts.checksum_hash_algorithm() == Some(rustc_span::SourceFileHashAlgorithm::Blake3);
        let driver = fingerprint_file(identity::executable().ok()?, path, want_checksum).ok()?;
        let mut hasher = StableHasher::new();
        (
            "ohm-clippy-module-lints-v1",
            &config.opts.incremental_driver_hash,
            driver.fingerprint,
            lookup,
            args,
            env::var_os("CARGO_PKG_RUST_VERSION"),
            env::var_os("CLIPPY_DISABLE_DOCS_LINKS"),
        )
            .hash(&mut hasher);
        let key: Fingerprint = hasher.finish();
        config.opts.incremental_driver_hash = Some(key.to_string());
        Some(driver)
    }
}

#[cfg(ohm_incremental_driver_hash)]
pub(super) use configuration::configure;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn running_driver_identity_is_stable() {
        let path = std::env::current_exe().unwrap();
        for checksum in [false, true] {
            let fingerprint = || {
                fingerprint_file(identity::executable().unwrap(), path.to_str().unwrap(), checksum)
                    .unwrap()
                    .fingerprint
            };
            assert_eq!(fingerprint(), fingerprint());
        }
    }

    #[test]
    #[cfg(all(ohm_driver_read_checksum, any(target_os = "linux", target_os = "macos")))]
    fn dep_info_preserves_the_verified_name_and_bytes_after_replacement() {
        use rustc_span::{SourceFileHash, SourceFileHashAlgorithm};
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("driver");
        let name = path.to_str().unwrap();
        let original = vec![b'x'; 2 * 64 * 1024 + 1];
        fs::write(&path, &original).unwrap();
        let original_image = File::open(&path).unwrap();
        let read = fingerprint_file(File::open(&path).unwrap(), name, true).unwrap();
        let digest = SourceFileHash::new_in_memory(SourceFileHashAlgorithm::Blake3, &original);

        let replacement = dir.path().join("replacement");
        let mut changed = original.clone();
        *changed.last_mut().unwrap() = b'y';
        fs::write(&replacement, changed).unwrap();
        fs::rename(&replacement, &path).unwrap();

        let observed = read.observed.unwrap();
        assert_eq!(observed.path, name);
        assert_eq!(observed.len, original.len() as u64);
        assert_eq!(observed.checksum, digest);
        let new = fingerprint_file(File::open(&path).unwrap(), name, true).unwrap();
        assert_ne!(read.fingerprint, new.fingerprint);
        assert_ne!(observed.checksum, new.observed.unwrap().checksum);

        assert!(fingerprint_file(original_image, name, true).is_err());
        assert!(
            fingerprint_file(File::open(&path).unwrap(), name, false)
                .unwrap()
                .observed
                .is_none()
        );
    }
}
