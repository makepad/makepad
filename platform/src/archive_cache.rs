//! Native persistent cache for byte ranges fetched from immutable HTTP archives.
//!
//! The platform owns the filesystem policy so clients can use the same optional
//! store on every target. WebAssembly deliberately returns no store: its normal
//! key/value storage is not intended for multi-megabyte archive shards.

pub const DEFAULT_ARCHIVE_CACHE_BUDGET: u64 = 8 * 1024 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::DEFAULT_ARCHIVE_CACHE_BUDGET;
    use crate::home::makepad_home;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    const ENTRY_MAGIC: &[u8; 8] = b"MPRNGC01";
    const ENTRY_HEADER_LEN: usize = 24;

    #[derive(Clone, Debug)]
    struct RangeEntry {
        shard: u32,
        offset: u64,
        len: u64,
        path: PathBuf,
    }

    impl RangeEntry {
        fn contains(&self, shard: u32, offset: u64, len: u64) -> bool {
            self.shard == shard
                && self.offset <= offset
                && self.offset.saturating_add(self.len) >= offset.saturating_add(len)
        }
    }

    /// A URL-scoped cache below `makepad_home()/maps/cache`.
    pub struct ArchiveCacheStore {
        cache_root: PathBuf,
        archive_dir: PathBuf,
        budget: u64,
        used_bytes: u64,
        ranges: Vec<RangeEntry>,
    }

    impl ArchiveCacheStore {
        /// Opens the standard per-user cache. Failure only disables persistence;
        /// archive networking remains usable.
        pub fn open_for_url(url: &str) -> Option<Self> {
            Self::open_at(
                makepad_home().join("maps/cache"),
                url,
                DEFAULT_ARCHIVE_CACHE_BUDGET,
            )
            .ok()
        }

        /// Opens a cache at an explicit root. Public for deterministic tests and
        /// embedders that provide their own Makepad home.
        pub fn open_at(
            cache_root: impl AsRef<Path>,
            url: &str,
            budget: u64,
        ) -> io::Result<Self> {
            let cache_root = cache_root.as_ref().to_path_buf();
            fs::create_dir_all(&cache_root)?;
            let used_bytes = sweep_cache_root(&cache_root, budget)?;
            let archive_dir = cache_root.join(archive_id(url));
            fs::create_dir_all(&archive_dir)?;
            let ranges = scan_ranges(&archive_dir);
            Ok(Self {
                cache_root,
                archive_dir,
                budget,
                used_bytes,
                ranges,
            })
        }

        pub fn read_root(&mut self) -> Option<Vec<u8>> {
            read_entry(&self.archive_dir.join("root.mkidx"), None)
        }

        pub fn write_root(&mut self, bytes: &[u8]) -> io::Result<()> {
            let path = self.archive_dir.join("root.mkidx");
            self.write(&path, bytes)?;
            Ok(())
        }

        /// Returns the smallest cached range containing the request.
        pub fn read_range(&mut self, shard: u32, offset: u64, len: u64) -> Option<Vec<u8>> {
            let mut candidates = self
                .ranges
                .iter()
                .filter(|entry| entry.contains(shard, offset, len))
                .cloned()
                .collect::<Vec<_>>();
            candidates.sort_unstable_by_key(|entry| entry.len);
            for entry in candidates {
                let Some(bytes) = read_entry(&entry.path, Some(entry.len)) else {
                    self.ranges.retain(|cached| cached.path != entry.path);
                    continue;
                };
                let start = usize::try_from(offset - entry.offset).ok()?;
                let len = usize::try_from(len).ok()?;
                let end = start.checked_add(len)?;
                if let Some(bytes) = bytes.get(start..end) {
                    return Some(bytes.to_vec());
                }
                let _ = fs::remove_file(&entry.path);
                self.ranges.retain(|cached| cached.path != entry.path);
            }
            None
        }

        pub fn write_range(
            &mut self,
            shard: u32,
            offset: u64,
            bytes: &[u8],
        ) -> io::Result<()> {
            let len = bytes.len() as u64;
            let directory = self.archive_dir.join(format!("{shard:03}"));
            let path = directory.join(format!("{offset}-{len}.bin"));
            self.write(&path, bytes)?;
            self.ranges.retain(|entry| entry.path != path);
            if path.is_file() {
                self.ranges.push(RangeEntry {
                    shard,
                    offset,
                    len,
                    path,
                });
            }
            Ok(())
        }

        fn write(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            let previous_len = fs::metadata(path).map_or(0, |metadata| metadata.len());
            atomic_write(path, &encode_entry(bytes))?;
            let new_len = fs::metadata(path).map_or(0, |metadata| metadata.len());
            self.used_bytes = self
                .used_bytes
                .saturating_sub(previous_len)
                .saturating_add(new_len);
            if self.used_bytes > self.budget {
                self.used_bytes = sweep_cache_root(&self.cache_root, self.budget)?;
                self.ranges = scan_ranges(&self.archive_dir);
            }
            Ok(())
        }

        #[doc(hidden)]
        pub fn archive_dir(&self) -> &Path {
            &self.archive_dir
        }
    }

    fn archive_id(url: &str) -> String {
        // Fixed seeds and byte order make this stable across processes, Rust
        // releases and platforms (unlike DefaultHasher).
        let left = stable_hash(url.as_bytes(), 0xcbf2_9ce4_8422_2325);
        let right = stable_hash(url.as_bytes(), 0x8422_2325_cbf2_9ce4);
        format!("{left:016x}{right:016x}")
    }

    fn stable_hash(bytes: &[u8], seed: u64) -> u64 {
        bytes.iter().fold(seed, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    fn encode_entry(bytes: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(ENTRY_HEADER_LEN + bytes.len());
        encoded.extend_from_slice(ENTRY_MAGIC);
        encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        encoded.extend_from_slice(&stable_hash(bytes, 0xcbf2_9ce4_8422_2325).to_le_bytes());
        encoded.extend_from_slice(bytes);
        encoded
    }

    fn read_entry(path: &Path, expected_len: Option<u64>) -> Option<Vec<u8>> {
        let encoded = match fs::read(path) {
            Ok(encoded) => encoded,
            Err(_) => return None,
        };
        let decoded = (|| {
            if encoded.len() < ENTRY_HEADER_LEN || &encoded[..8] != ENTRY_MAGIC {
                return None;
            }
            let len = u64::from_le_bytes(encoded[8..16].try_into().ok()?);
            let checksum = u64::from_le_bytes(encoded[16..24].try_into().ok()?);
            if expected_len.is_some_and(|expected| expected != len)
                || usize::try_from(len).ok()? != encoded.len() - ENTRY_HEADER_LEN
            {
                return None;
            }
            let bytes = &encoded[ENTRY_HEADER_LEN..];
            (stable_hash(bytes, 0xcbf2_9ce4_8422_2325) == checksum).then(|| bytes.to_vec())
        })();
        if decoded.is_some() {
            if let Ok(file) = File::options().write(true).open(path) {
                let _ = file.set_modified(SystemTime::now());
            }
        } else {
            let _ = fs::remove_file(path);
        }
        decoded
    }

    fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent"))?;
        fs::create_dir_all(parent)?;
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
        let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("entry");
        let temp = parent.join(format!(".{name}.tmp-{}-{nonce}", std::process::id()));
        let mut file = File::options().write(true).create_new(true).open(&temp)?;
        if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        drop(file);
        match fs::rename(&temp, path) {
            Ok(()) => Ok(()),
            // A concurrent writer may have won on platforms where rename does
            // not replace. Its checksum will be checked before it is used.
            Err(_) if path.is_file() => {
                let _ = fs::remove_file(&temp);
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                Err(error)
            }
        }
    }

    fn scan_ranges(archive_dir: &Path) -> Vec<RangeEntry> {
        let mut ranges = Vec::new();
        let Ok(shards) = fs::read_dir(archive_dir) else {
            return ranges;
        };
        for shard in shards.flatten() {
            let Some(shard_number) = shard
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(files) = fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                let Some((offset, len)) = parse_range_name(&file.file_name().to_string_lossy()) else {
                    continue;
                };
                ranges.push(RangeEntry {
                    shard: shard_number,
                    offset,
                    len,
                    path: file.path(),
                });
            }
        }
        ranges
    }

    fn parse_range_name(name: &str) -> Option<(u64, u64)> {
        let stem = name.strip_suffix(".bin")?;
        let (offset, len) = stem.split_once('-')?;
        Some((offset.parse().ok()?, len.parse().ok()?))
    }

    fn sweep_cache_root(cache_root: &Path, budget: u64) -> io::Result<u64> {
        let mut files = Vec::<(PathBuf, u64, SystemTime)>::new();
        collect_cache_files(cache_root, &mut files)?;
        let mut total = files.iter().map(|(_, len, _)| *len).sum::<u64>();
        if total <= budget {
            return Ok(total);
        }
        files.sort_unstable_by(|left, right| {
            left.2
                .cmp(&right.2)
                .then_with(|| left.0.cmp(&right.0))
        });
        for (path, len, _) in files {
            if total <= budget {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
        Ok(total)
    }

    fn collect_cache_files(
        directory: &Path,
        files: &mut Vec<(PathBuf, u64, SystemTime)>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                collect_cache_files(&path, files)?;
            } else if metadata.is_file() {
                if entry.file_name().to_string_lossy().contains(".tmp-") {
                    let _ = fs::remove_file(path);
                    continue;
                }
                files.push((
                    path,
                    metadata.len(),
                    metadata.modified().unwrap_or(UNIX_EPOCH),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::io;
    use std::path::Path;

    /// WebAssembly has no persistent archive store; the map's memory cache is
    /// still shared with the native implementation.
    pub struct ArchiveCacheStore;

    impl ArchiveCacheStore {
        pub fn open_for_url(_url: &str) -> Option<Self> {
            None
        }

        pub fn open_at(_root: impl AsRef<Path>, _url: &str, _budget: u64) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "archive disk cache is unavailable on wasm",
            ))
        }

        pub fn read_root(&mut self) -> Option<Vec<u8>> {
            None
        }

        pub fn write_root(&mut self, _bytes: &[u8]) -> io::Result<()> {
            Ok(())
        }

        pub fn read_range(&mut self, _shard: u32, _offset: u64, _len: u64) -> Option<Vec<u8>> {
            None
        }

        pub fn write_range(
            &mut self,
            _shard: u32,
            _offset: u64,
            _bytes: &[u8],
        ) -> io::Result<()> {
            Ok(())
        }

        pub fn archive_dir(&self) -> &Path {
            Path::new("")
        }
    }
}

pub use imp::ArchiveCacheStore;
