//! A tar writer for the archives Makepad ships itself: ustar headers, a GNU
//! `L` entry for a path longer than 100 bytes and a `K` entry for such a link
//! target (the reader in this crate takes both). Regular files stream from
//! disk through a 1 MB buffer, so an archive of any size costs a few
//! megabytes of memory; a symlink is written as a symlink, never followed;
//! a directory is one header. Modes are the low twelve permission bits,
//! times whole seconds — what a phone restores from the ustar `mtime` field,
//! and what the packer already normalized on disk.

use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
    time::UNIX_EPOCH,
};

const BLOCK: usize = 512;
const COPY_CHUNK: usize = 1 << 20;
/// A size the 11-octal-digit field cannot hold goes base-256.
const OCTAL_LIMIT: u64 = 1 << 33;

/// Writes one tar archive to a sink.
pub struct TarWriter<W: Write> {
    sink: W,
    buf: Vec<u8>,
    entries: usize,
}

impl<W: Write> TarWriter<W> {
    pub fn new(sink: W) -> Self {
        Self { sink, buf: vec![0u8; COPY_CHUNK], entries: 0 }
    }

    /// Entries written so far.
    pub fn entries(&self) -> usize {
        self.entries
    }

    /// Whatever `path` is, as `name`: a symlink as a symlink (never
    /// followed), a directory as an entry, anything else as a regular file.
    pub fn add_path(&mut self, name: &str, path: &Path) -> io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        let mtime = whole_seconds(&meta);
        if meta.file_type().is_symlink() {
            let target = fs::read_link(path)?;
            return self.add_symlink(name, &target.to_string_lossy(), mtime);
        }
        if meta.is_dir() {
            return self.add_dir(name, mode_of(&meta), mtime);
        }
        self.add_file(name, path)
    }

    /// A regular file's bytes, mode and whole-second mtime, from disk.
    pub fn add_file(&mut self, name: &str, path: &Path) -> io::Result<()> {
        let mut file = fs::File::open(path)?;
        let meta = file.metadata()?;
        let size = meta.len();
        self.header(name, size, b'0', mode_of(&meta), whole_seconds(&meta), "")?;
        let mut left = size;
        while left > 0 {
            let take = left.min(self.buf.len() as u64) as usize;
            file.read_exact(&mut self.buf[..take]).map_err(|e| {
                io::Error::new(e.kind(), format!("{}: file shrank while archiving ({e})", path.display()))
            })?;
            self.sink.write_all(&self.buf[..take])?;
            left -= take as u64;
        }
        self.pad(size)?;
        self.entries += 1;
        Ok(())
    }

    /// A regular file from memory.
    pub fn add_bytes(&mut self, name: &str, data: &[u8], mode: u32, mtime: u64) -> io::Result<()> {
        self.header(name, data.len() as u64, b'0', mode, mtime, "")?;
        self.sink.write_all(data)?;
        self.pad(data.len() as u64)?;
        self.entries += 1;
        Ok(())
    }

    pub fn add_symlink(&mut self, name: &str, target: &str, mtime: u64) -> io::Result<()> {
        self.header(name, 0, b'2', 0o777, mtime, target)?;
        self.entries += 1;
        Ok(())
    }

    pub fn add_dir(&mut self, name: &str, mode: u32, mtime: u64) -> io::Result<()> {
        let name = name.trim_end_matches('/');
        let with_slash = format!("{name}/");
        self.header(&with_slash, 0, b'5', mode, mtime, "")?;
        self.entries += 1;
        Ok(())
    }

    /// The end-of-archive marker (two zero blocks); the sink is handed back
    /// flushed.
    pub fn finish(mut self) -> io::Result<W> {
        self.sink.write_all(&[0u8; BLOCK * 2])?;
        self.sink.flush()?;
        Ok(self.sink)
    }

    fn pad(&mut self, size: u64) -> io::Result<()> {
        let pad = (BLOCK - (size % BLOCK as u64) as usize) % BLOCK;
        if pad > 0 {
            self.sink.write_all(&[0u8; BLOCK][..pad])?;
        }
        Ok(())
    }

    /// One entry header, preceded by the GNU long-name / long-link entries
    /// when a field does not fit its 100 bytes.
    fn header(&mut self, name: &str, size: u64, typeflag: u8, mode: u32, mtime: u64, link: &str) -> io::Result<()> {
        if name.len() > 100 {
            self.meta_entry(b'L', name)?;
        }
        if link.len() > 100 {
            self.meta_entry(b'K', link)?;
        }
        let block = build_header(name, size, typeflag, mode, mtime, link)?;
        self.sink.write_all(&block)
    }

    /// `././@LongLink`: the value plus its NUL as the entry's data.
    fn meta_entry(&mut self, typeflag: u8, value: &str) -> io::Result<()> {
        let data_len = value.len() as u64 + 1;
        let block = build_header("././@LongLink", data_len, typeflag, 0o644, 0, "")?;
        self.sink.write_all(&block)?;
        self.sink.write_all(value.as_bytes())?;
        self.sink.write_all(&[0u8])?;
        self.pad(data_len)
    }
}

/// A ustar header block. A name or link longer than its field is cut at a
/// character boundary; the `L`/`K` entry before it carries the whole value.
/// An mtime the 11-digit octal field cannot hold is an error, never a
/// truncated value.
fn build_header(name: &str, size: u64, typeflag: u8, mode: u32, mtime: u64, link: &str) -> io::Result<[u8; BLOCK]> {
    let mut h = [0u8; BLOCK];
    put_str(&mut h[0..100], name);
    put_octal(&mut h[100..108], (mode & 0o7777) as u64)?;
    put_octal(&mut h[108..116], 0)?;
    put_octal(&mut h[116..124], 0)?;
    if size < OCTAL_LIMIT {
        put_octal(&mut h[124..136], size)?;
    } else {
        // base-256: the high bit of the first byte set, the value big-endian.
        h[124] = 0x80;
        h[128..136].copy_from_slice(&size.to_be_bytes());
    }
    put_octal(&mut h[136..148], mtime)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{name}: mtime {mtime} does not fit the tar header")))?;
    h[156] = typeflag;
    put_str(&mut h[157..257], link);
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u64 = h.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { *b as u64 }).sum();
    h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
    h[155] = b' ';
    Ok(h)
}

fn put_str(field: &mut [u8], value: &str) {
    let mut end = value.len().min(field.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    field[..end].copy_from_slice(&value.as_bytes()[..end]);
}

/// `width - 1` octal digits and a NUL, as GNU tar writes them; a value with
/// more digits than the field holds is an error.
fn put_octal(field: &mut [u8], value: u64) -> io::Result<()> {
    let digits = field.len() - 1;
    let text = format!("{value:0width$o}", width = digits);
    if text.len() > digits {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{value} does not fit {digits} octal digits")));
    }
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
    Ok(())
}

fn whole_seconds(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_of(meta: &fs::Metadata) -> u32 {
    if meta.is_dir() {
        0o755
    } else {
        0o644
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{entries, unpack, EntryKind};
    use std::{path::PathBuf, time::SystemTime};

    fn scratch(tag: &str) -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("makepad-tar-writer-{tag}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = fs::File::open(path).unwrap();
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
    }

    #[test]
    fn written_archives_read_back_through_the_reader() {
        let src = scratch("src");
        fs::create_dir_all(src.join("d")).unwrap();
        fs::write(src.join("d/a.txt"), b"hello").unwrap();
        set_mtime(&src.join("d/a.txt"), 1_700_000_001);
        let big = vec![7u8; COPY_CHUNK + 12345];
        fs::write(src.join("d/big.bin"), &big).unwrap();
        let long = format!("d/{}.txt", "n".repeat(150));
        fs::write(src.join(&long), b"long").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(src.join("d/run"), b"#!x").unwrap();
            fs::set_permissions(src.join("d/run"), fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("a.txt", src.join("d/link")).unwrap();
        }

        let mut w = TarWriter::new(Vec::new());
        w.add_dir("d", 0o755, 1_700_000_000).unwrap();
        w.add_path("d/a.txt", &src.join("d/a.txt")).unwrap();
        w.add_path("d/big.bin", &src.join("d/big.bin")).unwrap();
        w.add_path(&long, &src.join(&long)).unwrap();
        w.add_bytes("d/mem.txt", b"from memory", 0o600, 1_700_000_005).unwrap();
        #[cfg(unix)]
        {
            w.add_path("d/run", &src.join("d/run")).unwrap();
            w.add_path("d/link", &src.join("d/link")).unwrap();
        }
        let archive = w.finish().unwrap();
        assert_eq!(archive.len() % BLOCK, 0);

        let parsed = entries(&archive).unwrap();
        assert_eq!(parsed[0].kind, EntryKind::Directory);
        assert_eq!(parsed[0].path, Path::new("d"));
        assert_eq!(parsed[1].path, Path::new("d/a.txt"));
        assert_eq!(parsed[1].mtime, 1_700_000_001);
        assert_eq!(parsed[1].data, b"hello");
        assert_eq!(parsed[2].data.len(), big.len());
        assert_eq!(parsed[3].path, Path::new(&long));
        assert_eq!(parsed[4].mode, 0o600);
        assert_eq!(parsed[4].mtime, 1_700_000_005);

        let dest = scratch("dest");
        let report = unpack(&archive, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("d/a.txt")).unwrap(), "hello");
        assert_eq!(fs::read(dest.join("d/big.bin")).unwrap(), big);
        assert_eq!(fs::read_to_string(dest.join(&long)).unwrap(), "long");
        assert_eq!(fs::read_to_string(dest.join("d/mem.txt")).unwrap(), "from memory");
        let modified = fs::metadata(dest.join("d/a.txt")).unwrap().modified().unwrap();
        assert_eq!(modified.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_700_000_001);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(report.symlinks, 1);
            assert_eq!(fs::metadata(dest.join("d/run")).unwrap().permissions().mode() & 0o777, 0o755);
            assert!(fs::symlink_metadata(dest.join("d/link")).unwrap().file_type().is_symlink());
            assert_eq!(fs::read_to_string(dest.join("d/link")).unwrap(), "hello");
        }
        assert_eq!(report.directories, 1);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn long_link_targets_and_large_sizes_have_headers() {
        let target = "t".repeat(140);
        let mut w = TarWriter::new(Vec::new());
        w.add_symlink("s", &target, 0).unwrap();
        let archive = w.finish().unwrap();
        let parsed = entries(&archive).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].link.as_deref(), Some(Path::new(&target)));

        let h = build_header("x", OCTAL_LIMIT + 5, b'0', 0o644, 0, "").unwrap();
        assert_eq!(h[124], 0x80);
        assert_eq!(u64::from_be_bytes(h[128..136].try_into().unwrap()), OCTAL_LIMIT + 5);
        let h = build_header("x", 123, b'0', 0o644, 1_700_000_000, "").unwrap();
        assert_eq!(&h[124..136], b"00000000173\0");
        assert_eq!(&h[136..148], format!("{:011o}\0", 1_700_000_000u64).as_bytes());
        // An mtime past the 11 octal digits (year 2242) is refused, not cut.
        assert!(build_header("x", 1, b'0', 0o644, 1 << 33, "").is_err());
        let mut w = TarWriter::new(Vec::new());
        assert!(w.add_bytes("late", b"", 0o644, 1 << 33).is_err());
    }
}
