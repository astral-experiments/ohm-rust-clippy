use super::*;

#[cfg(target_os = "linux")]
pub(super) fn executable() -> io::Result<File> {
    // This identifies the running image even if its original path has been replaced.
    File::open("/proc/self/exe")
}

#[cfg(target_os = "macos")]
pub(super) fn executable() -> io::Result<File> {
    use std::mem::{MaybeUninit, size_of};
    use std::os::unix::fs::MetadataExt;

    // libproc's public proc_regionwithpathinfo ABI (sys/proc_info.h). libc exposes
    // vnode_info_path and proc_pidinfo, but not the region structure or this selector.
    #[repr(C)]
    struct RegionInfo {
        protection: u32,
        max_protection: u32,
        inheritance: u32,
        flags: u32,
        offset: u64,
        behavior: u32,
        user_wired_count: u32,
        user_tag: u32,
        pages_resident: u32,
        pages_shared_now_private: u32,
        pages_swapped_out: u32,
        pages_dirtied: u32,
        ref_count: u32,
        shadow_depth: u32,
        share_mode: u32,
        private_pages_resident: u32,
        shared_pages_resident: u32,
        obj_id: u32,
        depth: u32,
        address: u64,
        size: u64,
    }
    #[repr(C)]
    struct RegionWithPath {
        region: RegionInfo,
        vnode: libc::vnode_info_path,
    }
    const PROC_PIDREGIONPATHINFO: libc::c_int = 8;
    unsafe extern "C" {
        fn _dyld_get_image_header(index: u32) -> *const std::ffi::c_void;
    }

    let file = File::open(std::env::current_exe()?)?;
    // SAFETY: dyld returns the read-only header of the main image; we only use its address.
    let header = unsafe { _dyld_get_image_header(0) };
    if header.is_null() {
        return Err(io::Error::other("dyld did not identify the main executable"));
    }
    let mut info = MaybeUninit::<RegionWithPath>::uninit();
    // SAFETY: the buffer has the public ABI's size and alignment and remains live for the
    // call. Query our own mapped executable, not a pathname that could identify a replacement.
    let written = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            PROC_PIDREGIONPATHINFO,
            header as u64,
            info.as_mut_ptr().cast(),
            size_of::<RegionWithPath>() as libc::c_int,
        )
    };
    if written != size_of::<RegionWithPath>() as libc::c_int {
        return Err(io::Error::other("could not identify the mapped Clippy executable"));
    }
    // SAFETY: a successful full-size result initialized every field of the C structure.
    let info = unsafe { info.assume_init() };
    let metadata = file.metadata()?;
    if metadata.dev() as u32 != info.vnode.vip_vi.vi_stat.vst_dev || metadata.ino() != info.vnode.vip_vi.vi_stat.vst_ino
    {
        return Err(io::Error::other("Clippy's executable path was replaced after startup"));
    }
    Ok(file)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn executable() -> io::Result<File> {
    Err(io::Error::other("running executable identity is unavailable"))
}

pub(super) fn unchanged(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        before.len() == after.len() && before.modified().ok() == after.modified().ok()
    }
}
