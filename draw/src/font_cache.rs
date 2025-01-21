use {
    makepad_platform::*,
    std::{
        collections::HashMap,
        fs::{File, OpenOptions},
        io,
        io::{Read, Write},
        path::Path,
    },
};

/// A cache for rasterized glyph data.
///
/// The cache is split into two parts:
/// - The rasterized glyph data.
/// - An index for the rasterized glyph data.
///
/// The index contains a mapping from keys to index entries. Each index entry contains:
/// - The dimensions of the glyph.
/// - The offset of its rasterized glyph data.
/// - The length of its rasterized glyph data.
///
/// This cache can optionally be backed on disk, allowing it to be persisted across runs of the
/// application. This should improve startup time, as the cache does not need to be re-generated
/// every time the application is started.
#[derive(Debug)]
pub struct FontCache {
    /// The rasterized glyph data.
    data: Vec<u8>,
    /// An optional file, backing the rasterized glyph data.
    data_file: Option<File>,
    /// An index for the rasterized glyph data.
    index: HashMap<Key, IndexEntry>,
    /// An optional file, backing the index.
    index_file: Option<File>,
}

impl FontCache {
    /// Creates a new `FontCache`.
    ///
    /// If a directory is provided, the rasterized glyph data will be initialized from the file
    /// `font_cache`, and the index from the file `font_cache_index`. Moreover, every time a
    /// new glyph is inserted into the cache, the rasterized glyph data will be appended to
    /// `font_cache`, and the corresponding index entry to `font_cache_index`.
    ///
    /// If no directory is provided, the cache will start out empty, and will not be backed by a
    /// file on disk.
    pub fn new(dir: Option<impl AsRef<Path>>) -> Self {
        Self::new_inner(dir.as_ref().map(|dir| dir.as_ref()))
    }

    fn new_inner(dir: Option<&Path>) -> Self {
        // Open the data file, if a directory was provided.
        let mut data_file = dir.map(|dir| {
            OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(dir.join("font_cache"))
                .expect("couldn't open font cache data file")
        });

        // Initialize the rasterized glyph data from the data file, if it exists.
        let mut data = Vec::new();
        if let Some(data_file) = &mut data_file {
            data_file
                .read_to_end(&mut data)
                .expect("couldn't read from font cache data file");
        }

        // Open the index file, if a directory was provided.
        let mut index_file = dir.map(|dir| {
            OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(dir.join("font_cache_index"))
                .expect("couldn't open font cache index file")
        });

        // Initialize the cache index from the index file, if it exists.
        let mut index = HashMap::new();
        if let Some(index_file) = &mut index_file {
            loop {
                let mut buffer = [0; 32];
                match index_file.read_exact(&mut buffer) {
                    Ok(_) => (),
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(_) => panic!("couldn't read from font cache index file"),
                }
                let key = Key(LiveId(u64::from_be_bytes(buffer[0..8].try_into().unwrap())));
                let width: usize = u32::from_be_bytes(buffer[8..12].try_into().unwrap())
                    .try_into()
                    .unwrap();
                let height: usize = u32::from_be_bytes(buffer[12..16].try_into().unwrap())
                    .try_into()
                    .unwrap();
                let offset: usize = u64::from_be_bytes(buffer[16..24].try_into().unwrap())
                    .try_into()
                    .unwrap();
                let len = u64::from_be_bytes(buffer[16..24].try_into().unwrap())
                    .try_into()
                    .unwrap();
                index.insert(
                    key,
                    IndexEntry {
                        size: SizeUsize::new(width, height),
                        offset,
                        len,
                    },
                );
            }
        }
        Self {
            data,
            data_file,
            index,
            index_file,
        }
    }

    pub fn get(&self, key: Key) -> Option<Entry<'_>> {
        let IndexEntry { size, offset, len } = self.index.get(&key).copied()?;
        Some(Entry {
            size,
            bytes: &self.data[offset..][..len],
        })
    }

    pub fn insert_with(&mut self, key: Key, f: impl FnOnce(&mut Vec<u8>) -> SizeUsize) {
        let offset = self.data.len();
        let size = f(&mut self.data);
        let len = self.data.len() - offset;
        if let Some(data_file) = &mut self.data_file {
            data_file
                .write_all(&self.data[offset..][..len])
                .expect("couldn't write to font cache data file");
        }
        self.index.insert(key, IndexEntry { size, offset, len });
        if let Some(index_file) = &mut self.index_file {
            let mut buffer = [0; 32];
            buffer[0..8].copy_from_slice(&key.0 .0.to_be_bytes());
            buffer[8..12].copy_from_slice(&u32::try_from(size.width).unwrap().to_be_bytes());
            buffer[12..16].copy_from_slice(&u32::try_from(size.height).unwrap().to_be_bytes());
            buffer[16..24].copy_from_slice(&u64::try_from(offset).unwrap().to_be_bytes());
            buffer[24..32].copy_from_slice(&u64::try_from(len).unwrap().to_be_bytes());
            index_file
                .write_all(&buffer)
                .expect("couldn't write to font cache index file");
        }
    }

    pub fn get_or_insert_with(
        &mut self,
        key: Key,
        f: impl FnOnce(&mut Vec<u8>) -> SizeUsize,
    ) -> Entry<'_> {
        if !self.index.contains_key(&key) {
            self.insert_with(key, f);
        }
        self.get(key).unwrap()
    }
}

/// A unique key for a rasterized glyph.
///
/// A rasterized glyph is uniquely identified by:
/// - The path to the font file that contains the glyph.
/// - The id of the glyph in the font file that contains the glyph.
/// - The font size with which the glyph was rasterized.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Key(LiveId);

impl Key {
    pub fn new(font_path: &str, glyph_id: usize, font_size: f64) -> Self {
        Self(
            LiveId::empty()
                .bytes_append(font_path.as_bytes())
                .bytes_append(&glyph_id.to_ne_bytes())
                .bytes_append(&font_size.to_ne_bytes()),
        )
    }
}

/// An entry in the font cache.
/// 
/// Each entry corresponds to a rasterized glyph. It contains:
/// - The dimensions of the rasterized glyph.
/// - The rasterized glyph data.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub size: SizeUsize,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug)]
struct IndexEntry {
    size: SizeUsize,
    offset: usize,
    len: usize,
}
