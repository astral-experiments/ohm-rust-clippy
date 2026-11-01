use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::Path;

pub const ENV: &str = "CLIPPY_ARGFILE_FINGERPRINT";

/// Fingerprint regular response files without waiting on a build-script producer.
pub fn fingerprint(root: &Path, names: &[&str]) -> Option<String> {
    let mut hash = DefaultHasher::new();
    for &name in names {
        let mut found = false;
        // An earlier file can enable shell syntax, so either spelling can be the input.
        for path in std::iter::once(name).chain(name.strip_prefix("shell:")) {
            let path = root.join(path);
            path.hash(&mut hash);
            let contents = match path.metadata() {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Ok(metadata) if metadata.is_file() => {
                    let mut open = std::fs::OpenOptions::new();
                    open.read(true);
                    // A writer may replace the file with a pipe after the metadata check.
                    #[cfg(unix)]
                    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut open, libc::O_NONBLOCK);
                    let file = open.open(path).ok()?;
                    if !file.metadata().ok()?.is_file() {
                        return None;
                    }
                    found = true;
                    Some(std::io::read_to_string(file).ok()?)
                },
                _ => return None,
            };
            contents.hash(&mut hash);
        }
        if !found {
            return None;
        }
    }
    Some(format!("{:016x}", hash.finish()))
}
