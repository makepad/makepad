use std::{
    fmt,
    io::{self, Read},
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;

#[derive(Clone)]
pub enum SharedBytes {
    Owned(Rc<Vec<u8>>),
    Mapped(Rc<MappedBytes>),
}

impl SharedBytes {
    pub fn from_owned(data: Rc<Vec<u8>>) -> Self {
        OWNED_LOADS.fetch_add(1, Ordering::Relaxed);
        Self::Owned(data)
    }

    pub fn from_vec(data: Vec<u8>) -> Self {
        Self::from_owned(Rc::new(data))
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            SharedBytes::Owned(data) => data.as_slice(),
            SharedBytes::Mapped(data) => data.as_slice(),
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn from_file(path: impl AsRef<Path>) -> io::Result<Self> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = path.as_ref();
            let mut file = File::open(path)?;
            let len = file_len(&file, path)?;
            Ok(Self::from_vec(read_file_owned(&mut file, len, path)?))
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "direct file loading is unsupported on wasm32",
            ))
        }
    }

    pub fn from_file_mmap_or_read(path: impl AsRef<Path>) -> io::Result<Self> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = path.as_ref();
            let mut file = File::open(path)?;
            let len = file_len(&file, path)?;
            match MappedBytes::map_file(&file, len) {
                Ok(mapped) => {
                    MMAP_HITS.fetch_add(1, Ordering::Relaxed);
                    Ok(Self::Mapped(Rc::new(mapped)))
                }
                Err(_) => {
                    MMAP_FALLBACKS.fetch_add(1, Ordering::Relaxed);
                    Ok(Self::from_vec(read_file_owned(&mut file, len, path)?))
                }
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            MMAP_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "mmap loading is unsupported on wasm32",
            ))
        }
    }

    pub fn stats() -> SharedBytesStats {
        SharedBytesStats {
            mmap_hits: MMAP_HITS.load(Ordering::Relaxed),
            mmap_fallbacks: MMAP_FALLBACKS.load(Ordering::Relaxed),
            owned_loads: OWNED_LOADS.load(Ordering::Relaxed),
        }
    }

    pub fn reset_stats() {
        MMAP_HITS.store(0, Ordering::Relaxed);
        MMAP_FALLBACKS.store(0, Ordering::Relaxed);
        OWNED_LOADS.store(0, Ordering::Relaxed);
    }
}

impl fmt::Debug for SharedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SharedBytes::Owned(data) => f
                .debug_struct("SharedBytes::Owned")
                .field("len", &data.len())
                .finish(),
            SharedBytes::Mapped(data) => f
                .debug_struct("SharedBytes::Mapped")
                .field("len", &data.len())
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SharedBytesStats {
    pub mmap_hits: u64,
    pub mmap_fallbacks: u64,
    pub owned_loads: u64,
}

pub struct MappedBytes {
    inner: os::MappedBytesInner,
}

impl MappedBytes {
    #[cfg(not(target_arch = "wasm32"))]
    fn map_file(file: &File, len: usize) -> io::Result<Self> {
        Ok(Self {
            inner: os::MappedBytesInner::map_file(file, len)?,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        self.inner.as_slice()
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Debug for MappedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappedBytes")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn file_len(file: &File, path: &Path) -> io::Result<usize> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("empty file: {}", path.display()),
        ));
    }
    usize::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file too large to map: {}", path.display()),
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn read_file_owned(file: &mut File, len: usize, path: &Path) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity(len);
    file.read_to_end(&mut data)?;
    if data.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("empty file: {}", path.display()),
        ));
    }
    Ok(data)
}

static MMAP_HITS: AtomicU64 = AtomicU64::new(0);
static MMAP_FALLBACKS: AtomicU64 = AtomicU64::new(0);
static OWNED_LOADS: AtomicU64 = AtomicU64::new(0);

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod os {
    use std::{
        ffi::c_void,
        fs::File,
        io,
        os::{fd::AsRawFd, raw::c_int},
        ptr,
    };

    type OffT = i64;

    const PROT_READ: c_int = 1;
    const MAP_PRIVATE: c_int = 2;
    const MAP_FAILED: *mut c_void = !0usize as *mut c_void;

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            length: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: OffT,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, length: usize) -> c_int;
    }

    pub struct MappedBytesInner {
        ptr: *const u8,
        len: usize,
    }

    impl MappedBytesInner {
        pub fn map_file(file: &File, len: usize) -> io::Result<Self> {
            let ptr = unsafe {
                mmap(
                    ptr::null_mut(),
                    len,
                    PROT_READ,
                    MAP_PRIVATE,
                    file.as_raw_fd(),
                    0,
                )
            };
            if ptr == MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                ptr: ptr.cast::<u8>(),
                len,
            })
        }

        pub fn as_slice(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    impl Drop for MappedBytesInner {
        fn drop(&mut self) {
            let _ = unsafe { munmap(self.ptr.cast_mut().cast::<c_void>(), self.len) };
        }
    }
}

#[cfg(windows)]
mod os {
    use std::{ffi::c_void, fs::File, io, os::windows::io::AsRawHandle, ptr};

    type Handle = *mut c_void;

    const PAGE_READONLY: u32 = 0x0002;
    const FILE_MAP_READ: u32 = 0x0004;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(
            hFile: Handle,
            lpFileMappingAttributes: *const c_void,
            flProtect: u32,
            dwMaximumSizeHigh: u32,
            dwMaximumSizeLow: u32,
            lpName: *const u16,
        ) -> Handle;
        fn MapViewOfFile(
            hFileMappingObject: Handle,
            dwDesiredAccess: u32,
            dwFileOffsetHigh: u32,
            dwFileOffsetLow: u32,
            dwNumberOfBytesToMap: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(lpBaseAddress: *const c_void) -> i32;
        fn CloseHandle(hObject: Handle) -> i32;
    }

    pub struct MappedBytesInner {
        mapping: Handle,
        view: *const u8,
        len: usize,
    }

    impl MappedBytesInner {
        pub fn map_file(file: &File, len: usize) -> io::Result<Self> {
            let mapping = unsafe {
                CreateFileMappingW(
                    file.as_raw_handle() as Handle,
                    ptr::null(),
                    PAGE_READONLY,
                    0,
                    0,
                    ptr::null(),
                )
            };
            if mapping.is_null() {
                return Err(io::Error::last_os_error());
            }

            let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
            if view.is_null() {
                let err = io::Error::last_os_error();
                unsafe {
                    let _ = CloseHandle(mapping);
                }
                return Err(err);
            }

            Ok(Self {
                mapping,
                view: view.cast::<u8>(),
                len,
            })
        }

        pub fn as_slice(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.view, self.len) }
        }
    }

    impl Drop for MappedBytesInner {
        fn drop(&mut self) {
            unsafe {
                let _ = UnmapViewOfFile(self.view.cast::<c_void>());
                let _ = CloseHandle(self.mapping);
            }
        }
    }
}

#[cfg(not(any(all(unix, not(target_arch = "wasm32")), windows)))]
mod os {
    use std::{fs::File, io};

    pub struct MappedBytesInner;

    impl MappedBytesInner {
        pub fn map_file(_file: &File, _len: usize) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "mmap is unsupported on this target",
            ))
        }

        pub fn as_slice(&self) -> &[u8] {
            &[]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SharedBytes;
    use std::path::PathBuf;

    fn bundled_font_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../widgets/resources/IBMPlexSans-Text.ttf")
    }

    #[test]
    fn from_file_matches_fs_read() {
        let path = bundled_font_path();
        let expected = std::fs::read(&path).expect("font file should be readable");
        let bytes = SharedBytes::from_file(&path).expect("font bytes should load");
        assert_eq!(bytes.as_slice(), expected.as_slice());
        assert!(matches!(bytes, SharedBytes::Owned(_)));
        assert!(!bytes.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn from_file_mmap_or_read_matches_fs_read() {
        let path = bundled_font_path();
        let expected = std::fs::read(&path).expect("font file should be readable");
        let bytes = SharedBytes::from_file_mmap_or_read(&path).expect("font bytes should load");
        assert_eq!(bytes.as_slice(), expected.as_slice());
        assert!(matches!(bytes, SharedBytes::Mapped(_)));
        assert!(!bytes.is_empty());
    }

    #[test]
    fn from_file_errors_for_missing_file() {
        let path = bundled_font_path().with_extension("missing");
        let err = SharedBytes::from_file(&path).expect_err("missing file should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
