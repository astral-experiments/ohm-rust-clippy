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
    let mut buffer = vec![0; 64 * 1024];
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
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rustc_interface::interface::Config;
    use rustc_session::lint::Level;
    use rustc_span::source_map::{FileLoader, RealFileLoader};

    use super::*;

    /// Enable only audited module passes after all non-query inputs have a compatible key.
    pub(crate) fn configure(
        config: &mut Config,
        lookup: &io::Result<(Option<PathBuf>, Vec<String>)>,
        args: Option<&str>,
    ) -> bool {
        if config.opts.incremental.is_none() || config.file_loader.is_some() {
            return false;
        }
        // Avoid hashing the driver for ordinary runs that do not enable this pedantic
        // lint. Attribute-only activation remains correct through the whole-crate pass.
        if !config.opts.lint_opts.iter().any(|(name, level)| {
            matches!(name.as_str(), "clippy::pedantic" | "clippy::zero_sized_map_values") && *level != Level::Allow
        }) {
            return false;
        }
        let Ok((path, warnings)) = lookup else { return false };
        let contents = match path {
            Some(path) => match RealFileLoader.read_file(path) {
                Ok(contents) => Some(contents),
                Err(_) => return false,
            },
            None => None,
        };
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
            path,
            warnings,
            &contents,
            args,
            env::var_os("CARGO_PKG_RUST_VERSION"),
            env::var_os("CLIPPY_DISABLE_DOCS_LINKS"),
        )
            .hash(&mut hasher);
        let key: Fingerprint = hasher.finish();
        config.opts.incremental_driver_hash = Some(key.to_string());
        if let (Some(path), Some(contents)) = (path, contents) {
            config.file_loader = Some(Box::new(ConfigSnapshot {
                path: path.clone(),
                contents,
            }));
        }
        true
    }

    /// Ensure configuration parsing reads the exact bytes included in the cache key.
    struct ConfigSnapshot {
        path: PathBuf,
        contents: String,
    }

    impl FileLoader for ConfigSnapshot {
        fn file_exists(&self, path: &Path) -> bool {
            path == self.path || RealFileLoader.file_exists(path)
        }

        fn read_file(&self, path: &Path) -> io::Result<String> {
            if path == self.path {
                Ok(self.contents.clone())
            } else {
                RealFileLoader.read_file(path)
            }
        }

        fn read_binary_file(&self, path: &Path) -> io::Result<Arc<[u8]>> {
            if path == self.path {
                Ok(self.contents.as_bytes().into())
            } else {
                RealFileLoader.read_binary_file(path)
            }
        }

        fn current_directory(&self) -> io::Result<PathBuf> {
            RealFileLoader.current_directory()
        }
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
