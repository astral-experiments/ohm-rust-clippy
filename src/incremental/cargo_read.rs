//! Consume the BLAKE3 checksum Cargo computed on the open, read-leased driver.

use std::fs::{self, File, Metadata};
use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::Path;

use rustc_span::SourceFileHash;

const HANDOFF: &str = "CARGO_OHM_CLIPPY_DRIVER_HASH_FDS";
const MAGIC: &[u8; 8] = b"OHMCLP01";

fn duplicate(fd: RawFd) -> Option<File> {
    if fd < 3 {
        return None;
    }
    // SAFETY: fcntl validates fd. F_DUPFD_CLOEXEC returns a new owned descriptor.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    (copy >= 0).then(|| unsafe { File::from_raw_fd(copy) })
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn retained_by_parent(parent: u32, fd: RawFd, file: &File) -> Option<bool> {
    let parent_fd = fs::metadata(format!("/proc/{parent}/fd/{fd}")).ok()?;
    Some(same_file(&parent_fd, &file.metadata().ok()?))
}

/// Match the producer, inode and both timestamps before using any supplied checksum.
fn decode(data: &[u8; 104], parent: u32, running: &Metadata) -> Option<SourceFileHash> {
    let expected = [
        u64::from(parent),
        running.dev(),
        running.ino(),
        running.len(),
        running.mtime() as u64,
        running.mtime_nsec() as u64,
        running.ctime() as u64,
        running.ctime_nsec() as u64,
    ];
    if &data[..8] != MAGIC
        || !data[8..72]
            .chunks_exact(8)
            .zip(expected)
            .all(|(word, value)| word == value.to_le_bytes())
    {
        return None;
    }
    Some(SourceFileHash::from_blake3(data[72..].try_into().ok()?))
}

fn read_leased(image: &File) -> bool {
    // SAFETY: F_GETLEASE queries the open file description without a pointer.
    unsafe { libc::fcntl(image.as_raw_fd(), libc::F_GETLEASE) == i32::from(libc::F_RDLCK) }
}

/// Prevent an authenticated inherited descriptor from leaking into compiler helpers.
fn close_on_exec(fd: RawFd) -> Option<()> {
    // SAFETY: both operations take an integer fd and no pointer argument.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return None;
    }
    Some(())
}

pub(super) fn verified_checksum(running: &Metadata, driver_path: &str) -> Option<SourceFileHash> {
    let handoff = std::env::var(HANDOFF).ok()?;
    let (record_fd, image_fd) = handoff.split_once(',')?;
    let record_fd = record_fd.parse::<RawFd>().ok()?;
    let image_fd = image_fd.parse::<RawFd>().ok()?;
    if record_fd == image_fd {
        return None;
    }
    let record = duplicate(record_fd)?;
    let image = duplicate(image_fd)?;
    // SAFETY: both calls query valid owned descriptors and take no pointer argument.
    let seals = unsafe { libc::fcntl(record.as_raw_fd(), libc::F_GET_SEALS) };
    let required = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if seals < 0 || seals & required != required || record.metadata().ok()?.len() != 104 {
        return None;
    }

    let mut data = [0; 104];
    record.read_exact_at(&mut data, 0).ok()?;
    // SAFETY: getppid takes no arguments and returns the current process's parent.
    let parent = unsafe { libc::getppid() } as u32;
    // Opening the kernel's executable link binds issuance to the Cargo
    // installed beside this driver. An arbitrary launcher can pass sealed
    // descriptors and fake stat fields, but cannot spoof its executed inode.
    let cargo = fs::metadata(Path::new(driver_path).with_file_name("cargo")).ok()?;
    let issuer = File::open(format!("/proc/{parent}/exe")).ok()?.metadata().ok()?;
    if !same_file(&issuer, &cargo)
        || !retained_by_parent(parent, record_fd, &record)?
        || !retained_by_parent(parent, image_fd, &image)?
    {
        return None;
    }
    if !read_leased(&image) || !super::identity::unchanged(running, &image.metadata().ok()?) {
        return None;
    }
    let checksum = decode(&data, parent, running)?;
    // A broken lease or a changed inode must send this invocation through the ordinary read.
    if !read_leased(&image) || !super::identity::unchanged(running, &image.metadata().ok()?) {
        return None;
    }
    close_on_exec(record_fd)?;
    close_on_exec(image_fd)?;
    Some(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_span::SourceFileHashAlgorithm;

    #[test]
    fn requires_the_recorded_parent_and_each_image_identity_field() {
        let meta = super::super::identity::executable().unwrap().metadata().unwrap();
        let parent = 123;
        let sample = SourceFileHash::new_in_memory(SourceFileHashAlgorithm::Blake3, b"test image");
        let fields = [
            u64::from(parent),
            meta.dev(),
            meta.ino(),
            meta.len(),
            meta.mtime() as u64,
            meta.mtime_nsec() as u64,
            meta.ctime() as u64,
            meta.ctime_nsec() as u64,
        ];
        let mut data = [0; 104];
        data[..8].copy_from_slice(MAGIC);
        for (chunk, field) in data[8..72].chunks_exact_mut(8).zip(fields) {
            chunk.copy_from_slice(&field.to_le_bytes());
        }
        data[72..].copy_from_slice(sample.hash_bytes());
        assert_eq!(decode(&data, parent, &meta), Some(sample));
        for offset in (0..72).step_by(8) {
            let mut changed = data;
            changed[offset] ^= 1;
            assert!(decode(&changed, parent, &meta).is_none());
        }
    }

    #[test]
    fn prevents_a_valid_handle_from_reaching_exec_helpers() {
        let input = File::open("/dev/null").unwrap();
        let fd = input.as_raw_fd();
        // SAFETY: fd remains owned by input until the end of this test.
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
        close_on_exec(fd).unwrap();
        assert_ne!(unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
    }
}
