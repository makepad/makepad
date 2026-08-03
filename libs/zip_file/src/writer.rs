// zip fileformat writing
//
// Deliberately minimal and deterministic: no zip comment (the reader seeks to
// End(-22) and would not find the EOCD if one were present), no data
// descriptors, no zip64, and a fixed timestamp so packing the same bytes twice
// produces the same archive. That last property is what lets a package be
// content-addressed by its sha256.

use crate::{
    COMPRESS_METHOD_DEFLATED, COMPRESS_METHOD_UNCOMPRESSED, CENTRAL_DIR_FILE_HEADER_SIGNATURE,
    END_OF_CENTRAL_DIRECTORY_SIGNATURE, LOCAL_FILE_HEADER_SIGNATURE,
};
use makepad_fast_inflate::{crc32, deflate::compress_to_vec};

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
}

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

/// Builds a zip archive in memory.
pub struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<Entry>,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
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
        let (method_code, payload) = match method {
            ZipMethod::Store => (COMPRESS_METHOD_UNCOMPRESSED, data.to_vec()),
            ZipMethod::Deflate => {
                let packed = compress_to_vec(data, 6);
                if packed.len() < data.len() {
                    (COMPRESS_METHOD_DEFLATED, packed)
                } else {
                    (COMPRESS_METHOD_UNCOMPRESSED, data.to_vec())
                }
            }
        };
        if payload.len() > u32::MAX as usize || self.out.len() > u32::MAX as usize {
            return Err(ZipWriteError::TooLarge);
        }

        let local_header_offset = self.out.len() as u32;
        let version = if method_code == COMPRESS_METHOD_DEFLATED {
            VERSION_DEFLATE
        } else {
            VERSION_STORE
        };

        self.push_u32(LOCAL_FILE_HEADER_SIGNATURE);
        self.push_u16(version);
        self.push_u16(0); // general purpose flags: none
        self.push_u16(method_code);
        self.push_u16(DOS_TIME);
        self.push_u16(DOS_DATE);
        self.push_u32(crc);
        self.push_u32(payload.len() as u32);
        self.push_u32(data.len() as u32);
        self.push_u16(name.len() as u16);
        self.push_u16(0); // extra field length
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(&payload);

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

    pub fn finish(mut self) -> Result<Vec<u8>, ZipWriteError> {
        if self.entries.len() > u16::MAX as usize {
            return Err(ZipWriteError::TooLarge);
        }
        let central_start = self.out.len();
        if central_start > u32::MAX as usize {
            return Err(ZipWriteError::TooLarge);
        }

        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let version = if e.method == COMPRESS_METHOD_DEFLATED {
                VERSION_DEFLATE
            } else {
                VERSION_STORE
            };
            self.push_u32(CENTRAL_DIR_FILE_HEADER_SIGNATURE);
            self.push_u16(version); // version made by
            self.push_u16(version); // version needed to extract
            self.push_u16(0);
            self.push_u16(e.method);
            self.push_u16(DOS_TIME);
            self.push_u16(DOS_DATE);
            self.push_u32(e.crc);
            self.push_u32(e.compressed_size);
            self.push_u32(e.uncompressed_size);
            self.push_u16(e.name.len() as u16);
            self.push_u16(0); // extra field
            self.push_u16(0); // comment
            self.push_u16(0); // disk number
            self.push_u16(0); // internal attributes
            // External attributes stay 0: we never carry unix permissions, so
            // an extracted member can never arrive executable.
            self.push_u32(0);
            self.push_u32(e.local_header_offset);
            self.out.extend_from_slice(e.name.as_bytes());
        }

        let central_size = self.out.len() - central_start;
        self.push_u32(END_OF_CENTRAL_DIRECTORY_SIGNATURE);
        self.push_u16(0); // this disk
        self.push_u16(0); // disk with central dir
        self.push_u16(entries.len() as u16);
        self.push_u16(entries.len() as u16);
        self.push_u32(central_size as u32);
        self.push_u32(central_start as u32);
        self.push_u16(0); // comment length — must stay 0, see module note
        Ok(self.out)
    }

    fn push_u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn push_u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
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
}
