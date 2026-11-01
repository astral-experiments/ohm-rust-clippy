use std::fs::{self, File};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Substitute copies of streams in the compiler's directory, after dependencies have run.
pub fn prepare(dir: &Path, args: &mut [String], handles: &mut Vec<File>) -> io::Result<()> {
    let cwd = std::env::current_dir()?;
    let last_shell = args.iter().rposition(|arg| arg.starts_with("@shell:")).unwrap_or(0);
    let mut shell_enabled = false;
    let mut next_unstable = false;
    for (index, arg) in args.iter_mut().enumerate().skip(1) {
        let inspect = index < last_shell && !shell_enabled;
        if let Some(name) = arg.strip_prefix('@') {
            let shell_name = name.strip_prefix("shell:").filter(|_| shell_enabled);
            let path = cwd.join(shell_name.unwrap_or(name));
            let copy = capture(dir, &path)?;
            // Until shell mode is enabled, every earlier file uses rustc's line format.
            if inspect && let Some(text) = crate::argfile::read_regular(copy.as_ref().unwrap_or(&path)) {
                shell_enabled = text.lines().any(|line| enables_shell(line, &mut next_unstable));
            }
            if let Some(copy) = copy {
                *arg = format!(
                    "{}{}",
                    if shell_name.is_some() { "@shell:" } else { "@" },
                    argument_path(&copy, handles)?
                );
            }
        } else if inspect {
            shell_enabled = enables_shell(arg, &mut next_unstable);
        }
    }
    Ok(())
}

/// A split unstable option consumes its next argument even when that argument starts with -Z.
fn enables_shell(arg: &str, next_unstable: &mut bool) -> bool {
    if std::mem::take(next_unstable) {
        return arg == "shell-argfiles";
    }
    *next_unstable = arg == "-Z";
    arg == "-Zshell-argfiles"
}

/// Rustc takes Unicode arguments. A descriptor keeps a copy in the requested Unix temp directory
/// available even when that directory's name cannot be represented as an argument.
fn argument_path(copy: &Path, handles: &mut Vec<File>) -> io::Result<String> {
    if let Some(path) = copy.to_str() {
        return Ok(path.to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;
        let file = File::open(copy)?;
        let path = format!("/dev/fd/{}", file.as_raw_fd());
        handles.push(file);
        Ok(path)
    }
    #[cfg(not(unix))]
    {
        let _ = handles;
        Err(io::Error::other("temporary argument file path is not Unicode"))
    }
}

/// Record the resolved stream before reading it. A producer may unlink the path before
/// verification; multiple names for the same stream must also agree on which writer to consume.
fn stream_key(dir: &Path, path: &Path) -> io::Result<Option<String>> {
    let mut name = DefaultHasher::new();
    path.hash(&mut name);
    let entry = dir.join(format!("{:016x}.source", name.finish()));
    match fs::read_to_string(&entry) {
        Ok(key) => return Ok(Some(key)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => return Err(error),
    }
    let Ok(meta) = path.metadata() else { return Ok(None) };
    if meta.is_file() || meta.is_dir() {
        return Ok(None);
    }
    let mut object = DefaultHasher::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        (meta.dev(), meta.ino(), meta.created().ok()).hash(&mut object);
    }
    #[cfg(not(unix))]
    path.canonicalize()?.hash(&mut object);
    let key = format!("{:016x}", object.finish());
    let mut record = tempfile::NamedTempFile::new_in(dir)?;
    record.write_all(key.as_bytes())?;
    match record.persist_noclobber(&entry) {
        Ok(_) => Ok(Some(key)),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => fs::read_to_string(entry).map(Some),
        Err(error) => Err(error.error),
    }
}

/// Publish only after EOF so a concurrent compiler cannot open a partial copy.
fn capture(dir: &Path, path: &Path) -> io::Result<Option<PathBuf>> {
    let Some(key) = stream_key(dir, path)? else {
        return Ok(None);
    };
    let saved = dir.join(format!("{key}.args"));
    let mut lock = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(saved.with_extension("lock"))?;
    lock.lock()?;
    if !saved.is_file() {
        // A failed or interrupted reader may already have consumed part of the stream.
        if lock.metadata()?.len() != 0 {
            return Err(io::Error::other(format!(
                "previous read of {} did not finish",
                path.display()
            )));
        }
        lock.write_all(b"reading")?;
        let mut copy = tempfile::NamedTempFile::new_in(dir)?;
        io::copy(&mut File::open(path)?, &mut copy)?;
        copy.persist(&saved)?;
    }
    Ok(Some(saved))
}

#[cfg(all(test, unix))]
mod tests {
    use super::capture;
    use std::process::Command;
    use std::{fs, thread};

    #[test]
    fn verification_replays_the_first_writer() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("flags");
        assert!(Command::new("mkfifo").arg(&pipe).status().unwrap().success());
        thread::scope(|scope| {
            scope.spawn(|| fs::write(&pipe, b"-Wclippy::needless_return\n").unwrap());
            let saved = capture(dir.path(), &pipe).unwrap().unwrap();
            assert_eq!(fs::read(&saved).unwrap(), b"-Wclippy::needless_return\n");
            fs::remove_file(&pipe).unwrap();
            assert_eq!(capture(dir.path(), &pipe).unwrap().unwrap(), saved);
        });
    }
}
