//! Tile payload compression codecs for MBTiles archives.
//!
//! Archives declare their tile codec in the `metadata` table:
//! - `compression` = `"gzip"` | `"br"` | `"br:dict-v1"` (absent = gzip, the
//!   historical default; gzip archives may also contain zlib or raw tiles,
//!   which are sniffed by magic bytes exactly as the legacy readers did)
//! - `compression_dict` = base64 of the raw shared-dictionary bytes, present
//!   only for `br:dict-v1`.
//!
//! Brotli tiles are standalone brotli streams (optionally encoded against a
//! raw LZ77 prefix dictionary), so a server can hand them out verbatim as
//! HTTP `Content-Encoding: br`.

use crate::{Error, Result};
use std::collections::HashMap;
use std::io::{Error as IoError, ErrorKind};

/// Metadata key naming the tile codec.
pub const COMPRESSION_METADATA_KEY: &str = "compression";
/// Metadata key carrying the base64 shared dictionary for `br:dict-v1`.
pub const COMPRESSION_DICT_METADATA_KEY: &str = "compression_dict";

const COMPRESSION_GZIP: &str = "gzip";
const COMPRESSION_BR: &str = "br";
const COMPRESSION_BR_DICT_V1: &str = "br:dict-v1";

/// How tile payloads should be compressed when writing an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileCompression {
    Gzip,
    Brotli { quality: u32 },
}

impl TileCompression {
    /// The `compression` metadata value for this codec, given whether a
    /// shared dictionary accompanies it.
    pub fn metadata_value(&self, has_dict: bool) -> &'static str {
        match (self, has_dict) {
            (TileCompression::Gzip, _) => COMPRESSION_GZIP,
            (TileCompression::Brotli { .. }, false) => COMPRESSION_BR,
            (TileCompression::Brotli { .. }, true) => COMPRESSION_BR_DICT_V1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TileCodecKind {
    /// Legacy default: gzip tiles, with zlib/raw sniffed by magic bytes.
    Gzip,
    Brotli,
    BrotliDict,
}

/// The decode side of an archive's tile codec, parsed once from metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileCodec {
    kind: TileCodecKind,
    dict: Vec<u8>,
}

impl TileCodec {
    /// The legacy gzip codec (also correct for archives without metadata).
    pub fn gzip() -> Self {
        TileCodec {
            kind: TileCodecKind::Gzip,
            dict: Vec::new(),
        }
    }

    /// Parse the codec from an archive's metadata rows.
    pub fn from_metadata(metadata: &HashMap<String, String>) -> Result<Self> {
        match metadata.get(COMPRESSION_METADATA_KEY).map(String::as_str) {
            None | Some(COMPRESSION_GZIP) => Ok(Self::gzip()),
            Some(COMPRESSION_BR) => Ok(TileCodec {
                kind: TileCodecKind::Brotli,
                dict: Vec::new(),
            }),
            Some(COMPRESSION_BR_DICT_V1) => {
                let dict = metadata
                    .get(COMPRESSION_DICT_METADATA_KEY)
                    .ok_or_else(|| {
                        Error::Codec("br:dict-v1 archive has no compression_dict".to_string())
                    })?;
                Ok(TileCodec {
                    kind: TileCodecKind::BrotliDict,
                    dict: base64_decode(dict)?,
                })
            }
            Some(other) => Err(Error::Codec(format!(
                "unsupported tile compression '{other}'"
            ))),
        }
    }

    /// The `compression` metadata value this codec was parsed from.
    pub fn metadata_value(&self) -> &'static str {
        match self.kind {
            TileCodecKind::Gzip => COMPRESSION_GZIP,
            TileCodecKind::Brotli => COMPRESSION_BR,
            TileCodecKind::BrotliDict => COMPRESSION_BR_DICT_V1,
        }
    }

    /// The raw shared dictionary bytes, when the archive declares one.
    pub fn dict(&self) -> Option<&[u8]> {
        if self.dict.is_empty() {
            None
        } else {
            Some(&self.dict)
        }
    }

    /// Whether stored tiles are standalone brotli streams.
    pub fn is_brotli(&self) -> bool {
        matches!(self.kind, TileCodecKind::Brotli | TileCodecKind::BrotliDict)
    }

    /// Decode one stored tile payload to raw bytes (usually MVT protobuf).
    pub fn decode(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        self.decode_limited(bytes, usize::MAX)
    }

    /// Decode while refusing an advertised or produced output above `limit`.
    pub fn decode_limited(&self, bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
        match self.kind {
            TileCodecKind::Gzip => {
                // Mirror the historical per-tile sniffing: gzip, zlib, or raw.
                if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
                    return gzip_decompress_limited(bytes, limit);
                }
                if bytes.len() >= 2 && bytes[0] == 0x78 {
                    match zlib_decompress_limited(bytes, limit) {
                        Ok(out) => return Ok(out),
                        Err(Error::Codec(error)) if error.starts_with("zlib decode failed") => {}
                        Err(error) => return Err(error),
                    }
                }
                (bytes.len() <= limit)
                    .then(|| bytes.to_vec())
                    .ok_or_else(|| Error::Codec("decoded tile exceeds byte limit".to_string()))
            }
            TileCodecKind::Brotli => brotli_decompress_limited(bytes, &[], limit),
            TileCodecKind::BrotliDict => brotli_decompress_limited(bytes, &self.dict, limit),
        }
    }
}

fn gzip_decompress_limited(bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
    if limit == usize::MAX {
        return makepad_fast_inflate::gzip_decompress_vec(bytes)
            .map_err(|err| Error::Codec(format!("gzip decode failed: {err:?}")));
    }
    let size = bytes
        .get(bytes.len().saturating_sub(4)..)
        .and_then(|size| size.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| Error::Codec("gzip decode failed: truncated trailer".to_string()))?
        as usize;
    if size > limit {
        return Err(Error::Codec("decoded tile exceeds byte limit".to_string()));
    }
    let mut out = vec![0_u8; size];
    let (_, written) = makepad_fast_inflate::gzip_decompress(bytes, &mut out).map_err(|err| {
        if matches!(err, makepad_fast_inflate::DecompressError::InsufficientSpace) {
            Error::Codec("decoded tile exceeds byte limit".to_string())
        } else {
            Error::Codec(format!("gzip decode failed: {err:?}"))
        }
    })?;
    out.truncate(written);
    Ok(out)
}

fn zlib_decompress_limited(bytes: &[u8], limit: usize) -> Result<Vec<u8>> {
    if limit == usize::MAX {
        return makepad_fast_inflate::zlib_decompress_vec(bytes)
            .map_err(|err| Error::Codec(format!("zlib decode failed: {err:?}")));
    }
    let mut capacity = bytes.len().saturating_mul(3).max(4096).min(limit);
    loop {
        let mut out = vec![0_u8; capacity];
        match makepad_fast_inflate::zlib_decompress(bytes, &mut out) {
            Ok((_, written)) => {
                out.truncate(written);
                return Ok(out);
            }
            Err(makepad_fast_inflate::DecompressError::InsufficientSpace) => {
                if capacity >= limit {
                    return Err(Error::Codec("decoded tile exceeds byte limit".to_string()));
                }
                capacity = capacity.saturating_mul(2).min(limit);
            }
            Err(err) => return Err(Error::Codec(format!("zlib decode failed: {err:?}"))),
        }
    }
}

/// Metadata rows describing a codec + optional dictionary; write these with
/// the tiles so readers (and tile servers) can decode the archive.
pub fn compression_metadata_rows(
    compression: &TileCompression,
    dict: Option<&[u8]>,
) -> Vec<(String, String)> {
    let dict = dict.filter(|d| !d.is_empty());
    let mut rows = vec![(
        COMPRESSION_METADATA_KEY.to_string(),
        compression.metadata_value(dict.is_some()).to_string(),
    )];
    if let (TileCompression::Brotli { .. }, Some(dict)) = (compression, dict) {
        rows.push((
            COMPRESSION_DICT_METADATA_KEY.to_string(),
            base64_encode(dict),
        ));
    }
    rows
}

/// Compress one raw tile payload for storage. Safe to call from worker
/// threads; the writer accepts the resulting bytes verbatim.
pub fn compress_tile(
    compression: &TileCompression,
    dict: Option<&[u8]>,
    raw: &[u8],
) -> Result<Vec<u8>> {
    match compression {
        TileCompression::Gzip => Ok(gzip_compress(raw)),
        TileCompression::Brotli { quality } => {
            brotli_compress(raw, *quality, dict.unwrap_or(&[]))
        }
    }
}

// ---------------------------------------------------------------------------
// gzip (fast level, matching the historical Compression::fast() output role)
// ---------------------------------------------------------------------------

fn gzip_compress(raw: &[u8]) -> Vec<u8> {
    let deflate = makepad_fast_inflate::deflate_compress(raw, 1);
    let mut out = Vec::with_capacity(deflate.len() + 18);
    // 10-byte header: magic, CM=deflate, no flags, no mtime, XFL=0, OS=unknown
    out.extend_from_slice(&[0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 0xff]);
    out.extend_from_slice(&deflate);
    out.extend_from_slice(&crc32(raw).to_le_bytes());
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, entry) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *entry = c;
    }
    let mut crc = 0xffff_ffff_u32;
    for &byte in bytes {
        crc = table[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

// ---------------------------------------------------------------------------
// brotli
// ---------------------------------------------------------------------------

fn brotli_compress(raw: &[u8], quality: u32, dict: &[u8]) -> Result<Vec<u8>> {
    use brotli::enc::{BrotliEncoderParams, StandardAlloc};
    use brotli::interface::{InputPair, InputReferenceMut, PredictionModeContextMap, StaticCommand};
    use brotli::{IoReaderWrapper, IoWriterWrapper};

    let mut params = BrotliEncoderParams::default();
    params.quality = quality.min(11) as i32;
    params.lgwin = 22;
    params.size_hint = raw.len();
    let mut input = raw;
    let mut output = Vec::with_capacity(raw.len() / 2 + 64);
    let mut input_buffer = [0u8; 8192];
    let mut output_buffer = [0u8; 8192];
    let mut nop_callback = |_data: &mut PredictionModeContextMap<InputReferenceMut>,
                            _cmds: &mut [StaticCommand],
                            _mb: InputPair,
                            _m: &mut StandardAlloc| ();
    brotli::BrotliCompressCustomIoCustomDict(
        &mut IoReaderWrapper(&mut input),
        &mut IoWriterWrapper(&mut output),
        &mut input_buffer,
        &mut output_buffer,
        &params,
        StandardAlloc::default(),
        &mut nop_callback,
        dict,
        IoError::new(ErrorKind::UnexpectedEof, "unexpected EOF"),
    )
    .map_err(|err| Error::Codec(format!("brotli encode failed: {err}")))?;
    Ok(output)
}

fn brotli_decompress_limited(bytes: &[u8], dict: &[u8], limit: usize) -> Result<Vec<u8>> {
    use brotli::{Allocator, HeapAlloc, HuffmanCode, IoReaderWrapper, IoWriterWrapper};
    use brotli::SliceWrapperMut;
    use std::io::Write;

    struct LimitedWriter {
        bytes: Vec<u8>,
        limit: usize,
    }

    impl Write for LimitedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.bytes.len().saturating_add(bytes.len()) > self.limit {
                return Err(IoError::new(ErrorKind::InvalidData, "decoded byte limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut alloc_u8 = HeapAlloc::<u8>::new(0);
    let dict_mem = if dict.is_empty() {
        <HeapAlloc<u8> as Allocator<u8>>::AllocatedMemory::default()
    } else {
        let mut cell = alloc_u8.alloc_cell(dict.len());
        cell.slice_mut().copy_from_slice(dict);
        cell
    };
    let mut input = bytes;
    let mut output = LimitedWriter {
        bytes: Vec::with_capacity(bytes.len().saturating_mul(4).max(4096).min(limit)),
        limit,
    };
    let mut input_buffer = [0u8; 8192];
    let mut output_buffer = [0u8; 8192];
    brotli::BrotliDecompressCustomIoCustomDict(
        &mut IoReaderWrapper(&mut input),
        &mut IoWriterWrapper(&mut output),
        &mut input_buffer,
        &mut output_buffer,
        alloc_u8,
        HeapAlloc::<u32>::new(0),
        HeapAlloc::<HuffmanCode>::new(HuffmanCode::default()),
        dict_mem,
        IoError::new(ErrorKind::UnexpectedEof, "unexpected EOF"),
    )
    .map_err(|err| Error::Codec(format!("brotli decode failed: {err}")))?;
    Ok(output.bytes)
}

#[cfg(test)]
fn brotli_decompress(bytes: &[u8], dict: &[u8]) -> Result<Vec<u8>> {
    brotli_decompress_limited(bytes, dict, usize::MAX)
}

// ---------------------------------------------------------------------------
// base64 (standard alphabet with padding; std-only, no dependency)
// ---------------------------------------------------------------------------

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(BASE64_ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>> {
    fn value(byte: u8) -> Result<u32> {
        match byte {
            b'A'..=b'Z' => Ok(u32::from(byte - b'A')),
            b'a'..=b'z' => Ok(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Ok(u32::from(byte - b'0') + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(Error::Codec(format!("invalid base64 byte {byte}"))),
        }
    }
    let bytes = text.trim().as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(Error::Codec("base64 length is not a multiple of 4".to_string()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || chunk[..4 - pad].iter().any(|&b| b == b'=') {
            return Err(Error::Codec("invalid base64 padding".to_string()));
        }
        let mut triple = 0u32;
        for &byte in &chunk[..4 - pad] {
            triple = (triple << 6) | value(byte)?;
        }
        triple <<= 6 * pad as u32;
        out.push((triple >> 16) as u8);
        if pad < 2 {
            out.push((triple >> 8) as u8);
        }
        if pad < 1 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for len in 0..67 {
            let bytes: Vec<u8> = (0..len).map(|n| (n * 37 + 11) as u8).collect();
            let encoded = base64_encode(&bytes);
            assert_eq!(base64_decode(&encoded).unwrap(), bytes);
        }
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert!(base64_decode("Zm9v!mFy").is_err());
    }

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn gzip_round_trips_through_fast_inflate() {
        let raw: Vec<u8> = (0..100_000).map(|n| ((n * 13) % 251) as u8).collect();
        let gz = gzip_compress(&raw);
        assert_eq!(&gz[..2], &[0x1f, 0x8b]);
        assert_eq!(
            makepad_fast_inflate::gzip_decompress_vec(&gz).unwrap(),
            raw
        );
    }

    #[test]
    fn limited_decode_rejects_gzip_zlib_and_brotli_expansion() {
        let raw = vec![42_u8; 16_384];
        let gzip = gzip_compress(&raw);
        let zlib = makepad_fast_inflate::zlib_compress(&raw, 1);
        let brotli = compress_tile(&TileCompression::Brotli { quality: 5 }, None, &raw).unwrap();
        let gzip_codec = TileCodec::gzip();
        let brotli_codec = TileCodec::from_metadata(
            &[(COMPRESSION_METADATA_KEY.to_string(), COMPRESSION_BR.to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap();
        assert!(gzip_codec.decode_limited(&gzip, 1024).is_err());
        assert!(gzip_codec.decode_limited(&zlib, 1024).is_err());
        assert!(brotli_codec.decode_limited(&brotli, 1024).is_err());
    }

    #[test]
    fn brotli_round_trips_with_and_without_dict() {
        let raw = b"the water_polygons layer holds water polygons; streets hold streets"
            .repeat(50);
        let dict = b"water_polygons streets layer polygons".to_vec();

        let plain = compress_tile(&TileCompression::Brotli { quality: 9 }, None, &raw).unwrap();
        assert_eq!(brotli_decompress(&plain, &[]).unwrap(), raw);

        let with_dict =
            compress_tile(&TileCompression::Brotli { quality: 9 }, Some(&dict), &raw).unwrap();
        assert_eq!(brotli_decompress(&with_dict, &dict).unwrap(), raw);

        // A dict-encoded stream must NOT decode to the same bytes without the
        // dictionary; equality would mean the dictionary was ignored.
        let without = brotli_decompress(&with_dict, &[]);
        assert!(without.is_err() || without.unwrap() != raw);
    }

    #[test]
    fn codec_parses_metadata_rows() {
        let dict = vec![1u8, 2, 3, 4, 5];
        let rows = compression_metadata_rows(&TileCompression::Brotli { quality: 11 }, Some(&dict));
        let map: HashMap<String, String> = rows.into_iter().collect();
        assert_eq!(map.get("compression").unwrap(), "br:dict-v1");
        let codec = TileCodec::from_metadata(&map).unwrap();
        assert_eq!(codec.dict(), Some(dict.as_slice()));
        assert!(codec.is_brotli());

        let mut plain = HashMap::new();
        assert_eq!(TileCodec::from_metadata(&plain).unwrap(), TileCodec::gzip());
        plain.insert("compression".to_string(), "br".to_string());
        let codec = TileCodec::from_metadata(&plain).unwrap();
        assert!(codec.is_brotli());
        assert_eq!(codec.dict(), None);
    }

    #[test]
    fn gzip_codec_sniffs_gzip_zlib_and_raw() {
        let codec = TileCodec::gzip();
        let raw = b"raw mvt bytes without any compression magic".to_vec();
        assert_eq!(codec.decode(&raw).unwrap(), raw);
        let gz = gzip_compress(&raw);
        assert_eq!(codec.decode(&gz).unwrap(), raw);
        let zlib = makepad_fast_inflate::zlib_compress(&raw, 6);
        assert_eq!(codec.decode(&zlib).unwrap(), raw);
    }
}
