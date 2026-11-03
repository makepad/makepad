use std::{
    fmt, io,
    ops::Deref,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[cfg(not(target_arch = "wasm32"))]
use std::{fs::File, io::Read};

mod sealed {
    pub trait Sealed {}
}

/// Plain numeric elements with no invalid bit patterns or drop glue.
pub trait Scalar: sealed::Sealed + Copy + Send + Sync + 'static {}

macro_rules! scalars {
    ($($ty:ty),* $(,)?) => {$(
        impl sealed::Sealed for $ty {}
        impl Scalar for $ty {}
    )*};
}
scalars!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64);

/// Shared CPU storage. Cloning retains the allocation; mutation uses copy-on-write.
#[derive(Clone)]
pub struct HostBuf<T: Scalar = u8> {
    store: Arc<Store<T>>,
}

enum Store<T: Scalar> {
    Heap(Vec<T>),
    // Only HostBuf<u8>'s constructors create this variant.
    File(Arc<MappedBytes>),
    Lease {
        ptr: *const T,
        len: usize,
        token: u64,
        release: fn(u64),
    },
}

// SAFETY: heap elements and mappings are Send + Sync. A lease's constructor
// requires immutable storage and a release hook that can run on any thread.
unsafe impl<T: Scalar> Send for Store<T> {}
unsafe impl<T: Scalar> Sync for Store<T> {}

impl<T: Scalar> Drop for Store<T> {
    fn drop(&mut self) {
        if let Self::Lease { token, release, .. } = self {
            release(*token);
        }
    }
}

impl<T: Scalar> From<Vec<T>> for HostBuf<T> {
    fn from(data: Vec<T>) -> Self {
        OWNED_LOADS.fetch_add(1, Ordering::Relaxed);
        Self {
            store: Arc::new(Store::Heap(data)),
        }
    }
}

impl<T: Scalar> HostBuf<T> {
    pub fn from_vec(data: Vec<T>) -> Self {
        data.into()
    }

    /// Takes ownership of a read-only lease without copying its elements.
    ///
    /// # Safety
    /// `ptr` must be non-null and aligned, even for an empty lease, and valid
    /// for reads of `len` initialized elements in one allocation (at most
    /// `isize::MAX` bytes). The storage must stay immutable and readable from
    /// any thread until `release(token)` runs. Ownership of this release is
    /// transferred here: the hook must safely release it exactly once, on any
    /// thread, and must not unwind. The token must identify this lease uniquely
    /// wherever the hook needs to distinguish outstanding leases.
    pub unsafe fn from_lease(ptr: *const T, len: usize, token: u64, release: fn(u64)) -> Self {
        Self {
            store: Arc::new(Store::Lease {
                ptr,
                len,
                token,
                release,
            }),
        }
    }

    pub fn as_slice(&self) -> &[T] {
        match self.store.as_ref() {
            Store::Heap(data) => data.as_slice(),
            Store::File(data) => {
                // SAFETY: File is constructed only for T = u8.
                unsafe { std::slice::from_raw_parts(data.as_slice().as_ptr().cast(), data.len()) }
            }
            Store::Lease { ptr, len, .. } => {
                // SAFETY: from_lease's contract keeps this immutable range alive
                // until the last Arc drops, after every reference to this slice.
                unsafe { std::slice::from_raw_parts(*ptr, *len) }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn path(&self) -> Option<&Path> {
        match self.store.as_ref() {
            Store::File(data) => Some(data.path()),
            _ => None,
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.store, &other.store)
    }

    /// Mutable heap storage, only when no other buffer shares it.
    pub fn get_mut(&mut self) -> Option<&mut Vec<T>> {
        match Arc::get_mut(&mut self.store)? {
            Store::Heap(data) => Some(data),
            _ => None,
        }
    }

    /// Mutable heap storage, copying shared, file or leased storage once.
    pub fn make_mut(&mut self) -> &mut Vec<T> {
        if self.get_mut().is_none() {
            let data = self.as_slice().to_vec();
            self.store = Arc::new(Store::Heap(data));
            COW.fetch_add(1, Ordering::Relaxed);
        }
        self.get_mut().expect("unique heap storage")
    }
}

impl<T: Scalar> Deref for HostBuf<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl HostBuf<u8> {
    /// Retains a read-only mapping another holder keeps.
    pub fn from_mapped(mapped: Arc<MappedBytes>) -> Self {
        Self {
            store: Arc::new(Store::File(mapped)),
        }
    }

    pub fn mapped(&self) -> Option<&Arc<MappedBytes>> {
        match self.store.as_ref() {
            Store::File(data) => Some(data),
            _ => None,
        }
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
            match MappedBytes::map_file(&file, len, path) {
                Ok(mapped) => {
                    MMAP_HITS.fetch_add(1, Ordering::Relaxed);
                    Ok(Self::from_mapped(Arc::new(mapped)))
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

    /// Maps a nonempty file range read-only without taking ownership of the descriptor.
    /// The descriptor may be closed afterwards; the backing file must remain unchanged.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    pub fn map_fd_range(fd: std::os::fd::RawFd, offset: u64, len: usize) -> io::Result<Self> {
        Self::map_fd_range_named(fd, offset, len, "")
    }

    /// [`Self::map_fd_range`] for a range with a name another component can open again (an APK
    /// asset, `apk:<asset path>`): `path()` reports it.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    pub fn map_fd_range_named(
        fd: std::os::fd::RawFd,
        offset: u64,
        len: usize,
        name: &str,
    ) -> io::Result<Self> {
        let mapped = MappedBytes {
            inner: os::MappedBytesInner::map_fd_range(fd, offset, len)?,
            path: PathBuf::from(name),
        };
        MMAP_HITS.fetch_add(1, Ordering::Relaxed);
        Ok(Self::from_mapped(Arc::new(mapped)))
    }

    pub fn stats() -> HostBufStats {
        HostBufStats {
            mmap_hits: MMAP_HITS.load(Ordering::Relaxed),
            mmap_fallbacks: MMAP_FALLBACKS.load(Ordering::Relaxed),
            owned_loads: OWNED_LOADS.load(Ordering::Relaxed),
            cow: COW.load(Ordering::Relaxed),
        }
    }

    pub fn reset_stats() {
        MMAP_HITS.store(0, Ordering::Relaxed);
        MMAP_FALLBACKS.store(0, Ordering::Relaxed);
        OWNED_LOADS.store(0, Ordering::Relaxed);
        COW.store(0, Ordering::Relaxed);
    }
}

impl<T: Scalar> fmt::Debug for HostBuf<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.store.as_ref() {
            Store::Heap(_) => "HostBuf::Heap",
            Store::File(_) => "HostBuf::File",
            Store::Lease { .. } => "HostBuf::Lease",
        };
        f.debug_struct(kind).field("len", &self.len()).finish()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostBufStats {
    pub mmap_hits: u64,
    pub mmap_fallbacks: u64,
    pub owned_loads: u64,
    /// Copies made by make_mut across all element types.
    pub cow: u64,
}

pub struct MappedBytes {
    inner: os::MappedBytesInner,
    path: PathBuf,
}

// SAFETY: the mapping is private and read-only; nothing writes through it, so any thread may read it.
unsafe impl Send for MappedBytes {}
unsafe impl Sync for MappedBytes {}

impl MappedBytes {
    #[cfg(not(target_arch = "wasm32"))]
    fn map_file(file: &File, len: usize, path: &Path) -> io::Result<Self> {
        Ok(Self {
            inner: os::MappedBytesInner::map_file(file, len)?,
            path: path.to_path_buf(),
        })
    }

    /// Maps the file read-only (no fallback to reading it).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let len = file_len(&file, path)?;
        Self::map_file(&file, len, path)
    }

    /// The file this is mapped from.
    pub fn path(&self) -> &Path {
        &self.path
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
static COW: AtomicU64 = AtomicU64::new(0);

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
        // On 32-bit bionic and glibc, the plain `mmap` symbol takes a 32-bit
        // `off_t`; `mmap64` takes 64 bits on every ABI (on 64-bit, it's an alias).
        #[cfg_attr(
            any(target_os = "android", all(target_os = "linux", target_env = "gnu")),
            link_name = "mmap64"
        )]
        fn mmap(
            addr: *mut c_void,
            length: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: OffT,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, length: usize) -> c_int;
        fn getpagesize() -> c_int;
    }

    pub struct MappedBytesInner {
        ptr: *const u8,
        len: usize,
        mapping: *mut c_void,
        mapped_len: usize,
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
                mapping: ptr,
                mapped_len: len,
            })
        }

        pub fn map_fd_range(fd: c_int, offset: u64, len: usize) -> io::Result<Self> {
            let offset = OffT::try_from(offset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "file offset exceeds off_t")
            })?;
            if len == 0 || len > isize::MAX as usize || offset.checked_add(len as OffT).is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "empty or overflowing file range",
                ));
            }
            let page_size = unsafe { getpagesize() };
            if page_size <= 0 {
                return Err(io::Error::last_os_error());
            }
            let slice_offset = (offset % OffT::from(page_size)) as usize;
            let mapped_len = len
                .checked_add(slice_offset)
                .filter(|len| *len <= isize::MAX as usize)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "mapped range exceeds address space",
                    )
                })?;
            let ptr = unsafe {
                mmap(
                    ptr::null_mut(),
                    mapped_len,
                    PROT_READ,
                    MAP_PRIVATE,
                    fd,
                    offset - slice_offset as OffT,
                )
            };
            if ptr == MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                ptr: unsafe { ptr.cast::<u8>().add(slice_offset) },
                len,
                mapping: ptr,
                mapped_len,
            })
        }

        pub fn as_slice(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    impl Drop for MappedBytesInner {
        fn drop(&mut self) {
            let _ = unsafe { munmap(self.mapping, self.mapped_len) };
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
    #[cfg(not(target_arch = "wasm32"))]
    use std::{fs::File, io};

    pub struct MappedBytesInner;

    impl MappedBytesInner {
        #[cfg(not(target_arch = "wasm32"))]
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
    use super::HostBuf;
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
        thread,
    };

    #[test]
    fn from_vec_keeps_allocation() {
        let mut data = Vec::with_capacity(8);
        data.extend([1u32, 2, 3]);
        let ptr = data.as_ptr();
        let mut buffer = HostBuf::from(data);
        assert_eq!(buffer.as_ptr(), ptr, "moving a Vec must not copy its data");
        assert_eq!(buffer.get_mut().unwrap().capacity(), 8);
        assert_eq!(&*buffer, &[1, 2, 3]);
        assert!(!buffer.is_empty());
        assert!(buffer.path().is_none());
        let empty = HostBuf::from(Vec::<f32>::new());
        assert!(empty.is_empty());
    }

    #[test]
    fn copy_on_write_counts_only_copies() {
        HostBuf::reset_stats();
        let mut buffer = HostBuf::from(vec![1f32, 2.0]);
        let ptr = buffer.as_ptr();
        buffer.make_mut()[0] = 3.0;
        assert_eq!(buffer.as_ptr(), ptr, "unique heap mutation must not copy");
        assert_eq!(HostBuf::stats().cow, 0);
        let mut shared = buffer.clone();
        assert!(buffer.ptr_eq(&shared));
        assert!(buffer.get_mut().is_none());
        shared.make_mut()[0] = 4.0;
        assert_eq!(HostBuf::stats().cow, 1);
        assert!(!buffer.ptr_eq(&shared));
        assert_eq!(buffer[0], 3.0, "the original must retain its contents");
        assert_eq!(shared[0], 4.0);
        shared.make_mut().push(5.0);
        assert_eq!(
            HostBuf::stats().cow,
            1,
            "subsequent unique mutation must not copy"
        );
        assert!(buffer.get_mut().is_some());

        let mut file = HostBuf::from_file_mmap_or_read(bundled_font_path()).unwrap();
        let mapping = file.mapped().unwrap().clone();
        let first = file[0];
        assert!(file.get_mut().is_none(), "a unique file is still read-only");
        file.make_mut()[0] = first.wrapping_add(1);
        assert_eq!(HostBuf::stats().cow, 2);
        assert_eq!(mapping.as_slice()[0], first);
        assert!(file.path().is_none());
        assert!(file.get_mut().is_some());

        static DATA: [u16; 2] = [7, 8];
        static RELEASED: AtomicU64 = AtomicU64::new(0);
        fn release(token: u64) {
            RELEASED.fetch_add(token, Ordering::SeqCst);
        }
        RELEASED.store(0, Ordering::SeqCst);
        // SAFETY: static immutable storage; the hook only records its release.
        let mut lease = unsafe { HostBuf::from_lease(DATA.as_ptr(), DATA.len(), 1, release) };
        assert!(lease.get_mut().is_none());
        lease.make_mut()[0] = 9;
        assert_eq!(HostBuf::stats().cow, 3);
        assert_eq!(RELEASED.load(Ordering::SeqCst), 1);
        assert_eq!(DATA, [7, 8]);
        drop(lease);
        assert_eq!(RELEASED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lease_releases_once_across_clones_and_threads() {
        static RELEASED: AtomicU64 = AtomicU64::new(0);
        fn release(token: u64) {
            // SAFETY: the constructor transfers this Box to the lease's hook.
            drop(unsafe { Box::from_raw(token as usize as *mut [u32; 3]) });
            RELEASED.fetch_add(1, Ordering::SeqCst);
        }
        RELEASED.store(0, Ordering::SeqCst);
        let ptr = Box::into_raw(Box::new([11u32, 12, 13]));
        // SAFETY: this allocation is immutable until the hook frees it. Its
        // sole ownership is transferred to the lease, including across threads.
        let buffer: HostBuf<u32> =
            unsafe { HostBuf::from_lease(ptr.cast(), 3, ptr as usize as u64, release) };
        let shared = Arc::new(buffer.clone());
        let reader = shared.clone();
        thread::spawn(move || {
            assert_eq!(&**reader, &[11, 12, 13]);
            let clone = reader.as_ref().clone();
            assert!(clone.ptr_eq(&reader));
        })
        .join()
        .unwrap();
        drop(buffer);
        assert_eq!(
            RELEASED.load(Ordering::SeqCst),
            0,
            "the last holder is still alive"
        );
        thread::spawn(move || drop(shared)).join().unwrap();
        assert_eq!(
            RELEASED.load(Ordering::SeqCst),
            1,
            "the last thread releases once"
        );
    }

    fn bundled_font_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../widgets/resources/IBMPlexSans-Text.ttf")
    }

    #[test]
    fn from_file_matches_fs_read() {
        let path = bundled_font_path();
        let expected = std::fs::read(&path).expect("font file should be readable");
        let bytes = HostBuf::from_file(&path).expect("font bytes should load");
        assert_eq!(bytes.as_slice(), expected.as_slice());
        assert!(bytes.path().is_none());
        assert!(!bytes.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn from_file_mmap_or_read_matches_fs_read() {
        let path = bundled_font_path();
        let expected = std::fs::read(&path).expect("font file should be readable");
        let bytes = HostBuf::from_file_mmap_or_read(&path).expect("font bytes should load");
        assert_eq!(bytes.as_slice(), expected.as_slice());
        assert_eq!(bytes.path(), Some(path.as_path()));
        assert!(!bytes.is_empty());
    }

    #[test]
    fn from_file_errors_for_missing_file() {
        let path = bundled_font_path().with_extension("missing");
        let err = HostBuf::from_file(&path).expect_err("missing file should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
