use std::{
    fmt, io,
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;

#[cfg(not(target_arch = "wasm32"))]
use memmap2::MmapOptions;

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

    pub fn from_file_mmap_or_read(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();

        #[cfg(not(target_arch = "wasm32"))]
        {
            match MappedBytes::map_file(path) {
                Ok(Some(mapped)) => {
                    MMAP_HITS.fetch_add(1, Ordering::Relaxed);
                    return Ok(Self::Mapped(Rc::new(mapped)));
                }
                Ok(None) => {
                    MMAP_FALLBACKS.fetch_add(1, Ordering::Relaxed);
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("cannot mmap empty file: {}", path.display()),
                    ));
                }
                Err(err) => {
                    MMAP_FALLBACKS.fetch_add(1, Ordering::Relaxed);
                    return Err(err);
                }
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            MMAP_FALLBACKS.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "mmap font loading is unsupported on wasm32",
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

#[derive(Debug)]
pub struct MappedBytes {
    #[cfg(not(target_arch = "wasm32"))]
    mmap: memmap2::Mmap,
    #[cfg(target_arch = "wasm32")]
    _unused: (),
}

impl MappedBytes {
    #[cfg(not(target_arch = "wasm32"))]
    fn map_file(path: &Path) -> io::Result<Option<Self>> {
        let file = File::open(path)?;
        if file.metadata()?.len() == 0 {
            return Ok(None);
        }
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        Ok(Some(Self { mmap }))
    }

    pub fn as_slice(&self) -> &[u8] {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.mmap.as_ref()
        }
        #[cfg(target_arch = "wasm32")]
        {
            &[]
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

static MMAP_HITS: AtomicU64 = AtomicU64::new(0);
static MMAP_FALLBACKS: AtomicU64 = AtomicU64::new(0);
static OWNED_LOADS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::SharedBytes;
    use std::path::PathBuf;

    fn bundled_font_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources/IBMPlexSans-Text.ttf")
    }

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
    fn from_file_mmap_or_read_errors_for_missing_file() {
        let path = bundled_font_path().with_extension("missing");
        let err = SharedBytes::from_file_mmap_or_read(&path).expect_err("missing file should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
