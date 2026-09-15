use std::{
    fs,
    io,
    path::{Path, PathBuf},
};

/// The filesystem volume containing a future file path.
///
/// `path` is the canonical existing directory used for the volume query. The
/// caller may use `key` to group several model files that share a volume.
pub(crate) struct Volume {
    pub key: String,
    pub path: PathBuf,
    pub available: u64,
}

pub(crate) fn for_path(path: &Path) -> io::Result<Volume> {
    let directory = existing_directory(path)?;
    let (key, available) = volume_info(&directory)?;
    Ok(Volume { key, path: directory, available })
}

fn existing_directory(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "disk volume path is empty",
        ));
    }

    let mut probe = path.to_path_buf();
    loop {
        match fs::metadata(&probe) {
            Ok(metadata) if metadata.is_dir() => return fs::canonicalize(probe),
            Ok(_) => {
                let Some(parent) = nonempty_parent(&probe) else {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "disk volume has no existing directory ancestor",
                    ));
                };
                probe = parent;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = nonempty_parent(&probe) else {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "disk volume has no existing directory ancestor",
                    ));
                };
                probe = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

fn nonempty_parent(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    if parent == path {
        return None;
    }
    if parent.as_os_str().is_empty() {
        // A bare relative path needs one attempt against the process's
        // current directory. Do not return "." for "." itself: if the cwd
        // was removed, that would make the ancestor walk spin forever.
        (path != Path::new(".")).then(|| PathBuf::from("."))
    } else {
        Some(parent.to_path_buf())
    }
}

#[cfg(unix)]
fn volume_info(path: &Path) -> io::Result<(String, u64)> {
    use std::os::unix::fs::MetadataExt;

    let key = path.metadata()?.dev().to_string();
    Ok((key, available_bytes(path)?))
}

#[cfg(windows)]
fn volume_info(path: &Path) -> io::Result<(String, u64)> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    unsafe extern "system" {
        fn GetVolumePathNameW(
            file_name: *const u16,
            volume_path_name: *mut u16,
            volume_path_name_size: u32,
        ) -> i32;
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }

    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut volume = vec![0u16; 32_768];
    if unsafe {
        GetVolumePathNameW(input.as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let length = volume.iter().position(|unit| *unit == 0).unwrap_or(volume.len());
    if length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned an empty volume path",
        ));
    }
    let mut volume_path = std::ffi::OsString::from_wide(&volume[..length]);
    if !volume_path.to_string_lossy().ends_with('\\')
        && !volume_path.to_string_lossy().ends_with('/')
    {
        volume_path.push("\\");
    }
    let volume_units: Vec<u16> = volume_path.encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    let mut total = 0;
    let mut free = 0;
    if unsafe {
        GetDiskFreeSpaceExW(
            volume_units.as_ptr(),
            &mut available,
            &mut total,
            &mut free,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((volume_path.to_string_lossy().into_owned(), available))
}

#[cfg(not(any(unix, windows)))]
fn volume_info(_path: &Path) -> io::Result<(String, u64)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "disk volume queries are unsupported on this native target",
    ))
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn available_bytes(path: &Path) -> io::Result<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt, os::raw::{c_int, c_ulong}};

    #[repr(C)]
    struct StatVfs {
        block_size: c_ulong,
        fragment_size: c_ulong,
        blocks: c_ulong,
        blocks_free: c_ulong,
        blocks_available: c_ulong,
        files: c_ulong,
        files_free: c_ulong,
        files_available: c_ulong,
        filesystem_id: c_ulong,
        flags: c_ulong,
        name_max: c_ulong,
        spare: [c_int; 6],
    }
    unsafe extern "C" {
        fn statvfs(path: *const std::ffi::c_char, out: *mut StatVfs) -> c_int;
    }

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "disk volume path contains NUL"))?;
    let mut stat: StatVfs = unsafe { std::mem::zeroed() };
    if unsafe { statvfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((stat.fragment_size.max(stat.block_size) as u64).saturating_mul(stat.blocks_available as u64))
}

#[cfg(all(target_os = "linux", not(target_pointer_width = "64")))]
fn available_bytes(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "disk volume queries are unsupported on 32-bit Linux",
    ))
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
fn available_bytes(path: &Path) -> io::Result<u64> {
    use std::{ffi::CString, os::raw::{c_int, c_uint}, os::unix::ffi::OsStrExt};

    #[repr(C)]
    struct StatFs {
        block_size: c_uint,
        io_size: c_int,
        blocks: u64,
        blocks_free: u64,
        blocks_available: u64,
        files: u64,
        files_free: u64,
        filesystem_id: [c_int; 2],
        owner: c_uint,
        filesystem_type: c_uint,
        flags: c_uint,
        filesystem_subtype: c_uint,
        filesystem_type_name: [u8; 16],
        mount_on_name: [u8; 1024],
        mount_from_name: [u8; 1024],
        flags_ext: c_uint,
        reserved: [c_uint; 7],
    }
    unsafe extern "C" {
        fn statfs(path: *const std::ffi::c_char, out: *mut StatFs) -> c_int;
    }

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "disk volume path contains NUL"))?;
    let mut stat: StatFs = unsafe { std::mem::zeroed() };
    if unsafe { statfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((stat.block_size as u64).saturating_mul(stat.blocks_available))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos", target_os = "ios", target_os = "tvos"))))]
fn available_bytes(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "disk volume queries are unsupported on this Unix target",
    ))
}

#[cfg(not(any(unix, windows)))]
fn available_bytes(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "disk volume queries are unsupported on this native target",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "makepad-disk-volume-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[cfg(any(windows, all(unix, target_pointer_width = "64")))]
    #[test]
    fn future_file_path_uses_existing_directory_ancestor() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let future = root.join("models").join("weights.bin");
        let volume = for_path(&future).unwrap();
        assert_eq!(volume.path, fs::canonicalize(&root).unwrap());
        assert!(!volume.key.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    #[test]
    fn symlinked_directory_is_resolved_before_querying_volume() {
        use std::os::unix::fs::symlink;

        let root = temp_root();
        let target = root.join("target");
        let link = root.join("link");
        fs::create_dir_all(&target).unwrap();
        symlink(&target, &link).unwrap();
        let volume = for_path(&link.join("future.bin")).unwrap();
        assert_eq!(volume.path, fs::canonicalize(target).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ancestor_walk_has_a_terminal_relative_root() {
        assert_eq!(nonempty_parent(Path::new("file")), Some(PathBuf::from(".")));
        assert!(nonempty_parent(Path::new(".")).is_none());
        assert!(nonempty_parent(Path::new("/")).is_none());
    }
}
