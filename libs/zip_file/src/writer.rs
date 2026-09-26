// zip fileformat writing
//
// Deliberately minimal and deterministic: no zip comment (the reader seeks to
// End(-22) and would not find the EOCD if one were present), no data
// descriptors, no zip64, and a fixed timestamp so packing the same bytes twice
// produces the same archive. That last property is what lets a package be
// content-addressed by its sha256.
//
// The archive goes to any `Write` sink: `ZipWriter::new()` builds it in
// memory, `ZipWriter::to(file)` streams it to disk one member at a time, so
// an APK of hundreds of megabytes never sits in memory whole. Only the
// central directory (a few dozen bytes per member) is kept until `finish`.

use crate::{
    CENTRAL_DIR_FILE_HEADER_SIGNATURE, COMPRESS_METHOD_DEFLATED, COMPRESS_METHOD_UNCOMPRESSED,
    END_OF_CENTRAL_DIRECTORY_SIGNATURE, LOCAL_FILE_HEADER_SIGNATURE,
};
use makepad_fast_inflate::{crc32, deflate::compress_to_vec};
use std::io::{self, Write};

/// 1980-01-01 00:00:00, the zero of the MS-DOS date format. Fixed rather than
/// wall-clock so archives are reproducible.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;

const VERSION_STORE: u16 = 10;
const VERSION_DEFLATE: u16 = 20;

#[derive(Debug)]
pub enum ZipWriteError {
    /// A member name that no reader should have to defend against.
    InvalidName(String),
    /// zip32 holds sizes and offsets in u32.
    TooLarge,
    DuplicateName(String),
    /// The sink refused bytes.
    Io(io::Error),
}

impl From<io::Error> for ZipWriteError {
    fn from(e: io::Error) -> Self {
        ZipWriteError::Io(e)
    }
}

impl std::fmt::Display for ZipWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZipWriteError::InvalidName(name) => write!(f, "invalid zip member name {name:?}"),
            ZipWriteError::TooLarge => write!(f, "zip archive or member exceeds 4 GB"),
            ZipWriteError::DuplicateName(name) => write!(f, "duplicate zip member {name:?}"),
            ZipWriteError::Io(e) => write!(f, "zip write: {e}"),
        }
    }
}

impl std::error::Error for ZipWriteError {}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ZipMethod {
    Store,
    Deflate,
}

struct Entry {
    name: String,
    method: u16,
    crc: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
}

/// Builds a zip archive, in memory by default or into any sink.
pub struct ZipWriter<W: Write = Vec<u8>> {
    out: W,
    /// Bytes written to `out` so far: the next member's local header offset.
    offset: u64,
    entries: Vec<Entry>,
}

impl Default for ZipWriter<Vec<u8>> {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter<Vec<u8>> {
    /// An archive built in memory; `finish` returns its bytes.
    pub fn new() -> Self {
        Self::to(Vec::new())
    }
}

impl<W: Write> ZipWriter<W> {
    /// An archive written to `sink` as members are added.
    pub fn to(sink: W) -> Self {
        Self {
            out: sink,
            offset: 0,
            entries: Vec::new(),
        }
    }

    /// Reject names the extractor would have to defend against anyway. Writing
    /// them is never intentional, so failing here keeps a hostile archive from
    /// originating with us.
    fn check_name(&self, name: &str) -> Result<(), ZipWriteError> {
        let bad = name.is_empty()
            || name.len() > 4096
            || name.starts_with('/')
            || name.starts_with('\\')
            || name.contains('\\')
            || name.contains('\0')
            || name.split('/').any(|c| c == ".." || c == ".")
            || name.chars().nth(1) == Some(':');
        if bad {
            return Err(ZipWriteError::InvalidName(name.to_string()));
        }
        if self.entries.iter().any(|e| e.name == name) {
            return Err(ZipWriteError::DuplicateName(name.to_string()));
        }
        Ok(())
    }

    pub fn add(
        &mut self,
        name: &str,
        data: &[u8],
        method: ZipMethod,
    ) -> Result<(), ZipWriteError> {
        self.check_name(name)?;
        if data.len() > u32::MAX as usize {
            return Err(ZipWriteError::TooLarge);
        }
        let crc = crc32(data);

        // Fall back to Store when deflate does not pay: a stored member is
        // cheaper to read back and never larger than the input.
        let packed;
        let (method_code, payload): (u16, &[u8]) = match method {
            ZipMethod::Store => (COMPRESS_METHOD_UNCOMPRESSED, data),
            ZipMethod::Deflate => {
                packed = compress_to_vec(data, 6);
                if packed.len() < data.len() {
                    (COMPRESS_METHOD_DEFLATED, &packed)
                } else {
                    (COMPRESS_METHOD_UNCOMPRESSED, data)
                }
            }
        };
        if payload.len() > u32::MAX as usize || self.offset > u32::MAX as u64 {
            return Err(ZipWriteError::TooLarge);
        }

        let local_header_offset = self.offset as u32;
        let version = if method_code == COMPRESS_METHOD_DEFLATED {
            VERSION_DEFLATE
        } else {
            VERSION_STORE
        };

        self.push_u32(LOCAL_FILE_HEADER_SIGNATURE)?;
        self.push_u16(version)?;
        self.push_u16(0)?; // general purpose flags: none
        self.push_u16(method_code)?;
        self.push_u16(DOS_TIME)?;
        self.push_u16(DOS_DATE)?;
        self.push_u32(crc)?;
        self.push_u32(payload.len() as u32)?;
        self.push_u32(data.len() as u32)?;
        self.push_u16(name.len() as u16)?;
        self.push_u16(0)?; // extra field length
        self.push_bytes(name.as_bytes())?;
        self.push_bytes(payload)?;

        self.entries.push(Entry {
            name: name.to_string(),
            method: method_code,
            crc,
            compressed_size: payload.len() as u32,
            uncompressed_size: data.len() as u32,
            local_header_offset,
        });
        Ok(())
    }

    /// The central directory and its end record; the sink is handed back
    /// flushed.
    pub fn finish(mut self) -> Result<W, ZipWriteError> {
        if self.entries.len() > u16::MAX as usize {
            return Err(ZipWriteError::TooLarge);
        }
        let central_start = self.offset;
        if central_start > u32::MAX as u64 {
            return Err(ZipWriteError::TooLarge);
        }

        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let version = if e.method == COMPRESS_METHOD_DEFLATED {
                VERSION_DEFLATE
            } else {
                VERSION_STORE
            };
            self.push_u32(CENTRAL_DIR_FILE_HEADER_SIGNATURE)?;
            self.push_u16(version)?; // version made by
            self.push_u16(version)?; // version needed to extract
            self.push_u16(0)?;
            self.push_u16(e.method)?;
            self.push_u16(DOS_TIME)?;
            self.push_u16(DOS_DATE)?;
            self.push_u32(e.crc)?;
            self.push_u32(e.compressed_size)?;
            self.push_u32(e.uncompressed_size)?;
            self.push_u16(e.name.len() as u16)?;
            self.push_u16(0)?; // extra field
            self.push_u16(0)?; // comment
            self.push_u16(0)?; // disk number
            self.push_u16(0)?; // internal attributes
            // External attributes stay 0: we never carry unix permissions, so
            // an extracted member can never arrive executable.
            self.push_u32(0)?;
            self.push_u32(e.local_header_offset)?;
            self.push_bytes(e.name.as_bytes())?;
        }

        let central_size = self.offset - central_start;
        self.push_u32(END_OF_CENTRAL_DIRECTORY_SIGNATURE)?;
        self.push_u16(0)?; // this disk
        self.push_u16(0)?; // disk with central dir
        self.push_u16(entries.len() as u16)?;
        self.push_u16(entries.len() as u16)?;
        self.push_u32(central_size as u32)?;
        self.push_u32(central_start as u32)?;
        self.push_u16(0)?; // comment length — must stay 0, see module note
        self.out.flush()?;
        Ok(self.out)
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), ZipWriteError> {
        self.out.write_all(bytes)?;
        self.offset += bytes.len() as u64;
        Ok(())
    }

    fn push_u16(&mut self, v: u16) -> Result<(), ZipWriteError> {
        self.push_bytes(&v.to_le_bytes())
    }

    fn push_u32(&mut self, v: u32) -> Result<(), ZipWriteError> {
        self.push_bytes(&v.to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip_read_central_directory;
    use std::io::Cursor;

    fn roundtrip(members: &[(&str, &[u8])], method: ZipMethod) {
        let mut w = ZipWriter::new();
        for (name, data) in members {
            w.add(name, data, method).unwrap();
        }
        let bytes = w.finish().unwrap();

        let mut cursor = Cursor::new(&bytes);
        let dir = zip_read_central_directory(&mut cursor).unwrap();
        assert_eq!(dir.file_headers.len(), members.len());
        for (i, (name, data)) in members.iter().enumerate() {
            let h = &dir.file_headers[i];
            assert_eq!(&h.file_name, name);
            let got = h.extract(&mut cursor).unwrap();
            assert_eq!(&got[..], *data, "member {name}");
        }
    }

    #[test]
    fn stored_members_round_trip_through_our_reader() {
        roundtrip(
            &[
                ("game.splash", b"game.box({pos: vec3(0,0,0)})"),
                ("manifest.toml", b"name = \"demo\"\n"),
                ("assets/deadbeef.png", &[0u8, 1, 2, 3, 255]),
            ],
            ZipMethod::Store,
        );
    }

    #[test]
    fn deflated_members_round_trip_through_our_reader() {
        let big = vec![b'a'; 100_000];
        roundtrip(
            &[("repetitive.txt", &big), ("small.txt", b"hello")],
            ZipMethod::Deflate,
        );
    }

    #[test]
    fn an_archive_comment_after_the_end_record_is_skipped() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"alpha", ZipMethod::Deflate).unwrap();
        let mut bytes = w.finish().unwrap();
        // Rewrite the end record's comment length and append a comment,
        // as GitHub's generated archives do.
        let comment = b"0180df21f5e0bd39b9060cc5de420ed2f1f9e509";
        let n = bytes.len();
        bytes[n - 2..].copy_from_slice(&(comment.len() as u16).to_le_bytes());
        bytes.extend_from_slice(comment);
        let mut cursor = Cursor::new(&bytes);
        let dir = zip_read_central_directory(&mut cursor).unwrap();
        assert_eq!(dir.file_headers.len(), 1);
        assert_eq!(dir.file_headers[0].extract(&mut cursor).unwrap(), b"alpha");
    }

    #[test]
    fn empty_archive_and_empty_member_are_readable() {
        let bytes = ZipWriter::new().finish().unwrap();
        let mut cursor = Cursor::new(&bytes);
        let dir = zip_read_central_directory(&mut cursor).unwrap();
        assert_eq!(dir.file_headers.len(), 0);

        roundtrip(&[("empty", b"")], ZipMethod::Deflate);
    }

    #[test]
    fn deflate_falls_back_to_store_when_it_would_grow() {
        // Incompressible input: the deflate wrapper would exceed the original.
        let mut data = Vec::new();
        let mut x: u32 = 0x1234_5678;
        for _ in 0..512 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            data.push(x as u8);
        }
        let mut w = ZipWriter::new();
        w.add("noise.bin", &data, ZipMethod::Deflate).unwrap();
        let bytes = w.finish().unwrap();
        let mut cursor = Cursor::new(&bytes);
        let dir = zip_read_central_directory(&mut cursor).unwrap();
        assert_eq!(
            dir.file_headers[0].compression_method,
            COMPRESS_METHOD_UNCOMPRESSED
        );
        assert_eq!(dir.file_headers[0].extract(&mut cursor).unwrap(), data);
    }

    #[test]
    fn hostile_and_duplicate_names_are_refused() {
        let mut w = ZipWriter::new();
        for bad in [
            "/etc/passwd",
            "../escape",
            "a/../../b",
            "C:/windows",
            "back\\slash",
            "",
            "nul\0byte",
            "./relative",
        ] {
            assert!(
                w.add(bad, b"x", ZipMethod::Store).is_err(),
                "should refuse {bad:?}"
            );
        }
        w.add("ok.txt", b"x", ZipMethod::Store).unwrap();
        assert!(w.add("ok.txt", b"y", ZipMethod::Store).is_err());
    }

    #[test]
    fn packing_is_deterministic() {
        let build = || {
            let mut w = ZipWriter::new();
            w.add("a.txt", b"alpha", ZipMethod::Deflate).unwrap();
            w.add("b/c.txt", b"beta", ZipMethod::Store).unwrap();
            w.finish().unwrap()
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn a_sink_gets_the_same_bytes_as_memory() {
        let in_memory = {
            let mut w = ZipWriter::new();
            w.add("a.txt", b"alpha", ZipMethod::Deflate).unwrap();
            w.add("b/c.txt", b"beta", ZipMethod::Store).unwrap();
            w.finish().unwrap()
        };
        let streamed = {
            let mut w = ZipWriter::to(Cursor::new(Vec::new()));
            w.add("a.txt", b"alpha", ZipMethod::Deflate).unwrap();
            w.add("b/c.txt", b"beta", ZipMethod::Store).unwrap();
            w.finish().unwrap().into_inner()
        };
        assert_eq!(in_memory, streamed);
    }
}
