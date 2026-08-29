//! Driver inputs for rustc's ordinary incremental cache. No separate cache directories or
//! crate-identity changes are needed: changed inputs invalidate the previous query graph.

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};

use rustc_data_structures::fingerprint::Fingerprint;
use rustc_data_structures::stable_hash::StableHasher;

mod identity;

/// Hash the running executable, rejecting replacements or changes during the read.
fn driver_fingerprint() -> io::Result<Fingerprint> {
    let mut file = identity::executable()?;
    let before = file.metadata()?;
    let mut hasher = StableHasher::new();
    before.len().hash(&mut hasher);
    let mut buffer = [0; 64 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.write(&buffer[..size]);
    }
    let after = file.metadata()?;
    if !identity::unchanged(&before, &after) {
        return Err(io::Error::other("Clippy executable changed while being read"));
    }
    Ok(hasher.finish())
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
    ) -> bool {
        if config.opts.incremental.is_none() || config.file_loader.is_some() {
            return false;
        }
        if lookup.has_errors() {
            return false;
        }
        // Cargo's dep-info can only represent UTF-8 paths. If it cannot track replacement
        // of this driver, retain the whole-crate pass instead of adding a cached lint.
        if !env::current_exe().ok().is_some_and(|path| path.to_str().is_some()) {
            return false;
        }
        let Ok(driver) = driver_fingerprint() else { return false };
        let mut hasher = StableHasher::new();
        (
            "ohm-clippy-module-lints-v1",
            &config.opts.incremental_driver_hash,
            driver,
            lookup,
            args,
            env::var_os("CARGO_PKG_RUST_VERSION"),
            env::var_os("CLIPPY_DISABLE_DOCS_LINKS"),
        )
            .hash(&mut hasher);
        let key: Fingerprint = hasher.finish();
        config.opts.incremental_driver_hash = Some(key.to_string());
        true
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
        assert_eq!(driver_fingerprint().unwrap(), driver_fingerprint().unwrap());
    }
}
