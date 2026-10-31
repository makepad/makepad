//! Borrowed GLB chunks and container-only assembly, without parsing the JSON.

use crate::GltfError;

const JSON_CHUNK: u32 = u32::from_le_bytes(*b"JSON");
const BIN_CHUNK: u32 = u32::from_le_bytes(*b"BIN\0");

#[derive(Clone, Copy, Debug)]
pub struct GlbChunks<'a> {
    /// JSON text, including its original padding.
    pub json: &'a str,
    /// BIN payload, including padding; empty if the BIN chunk is absent.
    pub bin: &'a [u8],
    /// Complete chunks after BIN (or JSON when BIN is absent), unchanged.
    pub rest: &'a [u8],
}

/// Read a chunk header and return its type, payload and remaining bytes.
pub(crate) fn read_chunk(bytes: &[u8]) -> Result<(u32, &[u8], &[u8]), GltfError> {
    let header = bytes
        .get(..8)
        .ok_or_else(|| GltfError::InvalidGlb("chunk header overruns file bounds".to_string()))?;
    let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let kind = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let bytes = &bytes[8..];
    let data = bytes
        .get(..len)
        .ok_or_else(|| GltfError::InvalidGlb("chunk data overruns file bounds".to_string()))?;
    Ok((kind, data, &bytes[len..]))
}

/// Split a GLB 2.0 container, validating every chunk's bounds and alignment.
/// JSON must come first, followed by an optional BIN and any extra chunks.
/// The JSON is checked for UTF-8, but its contents are not parsed.
pub fn split_glb(glb: &[u8]) -> Result<GlbChunks<'_>, GltfError> {
    let header = glb
        .get(..12)
        .ok_or_else(|| GltfError::InvalidGlb("header is shorter than 12 bytes".to_string()))?;
    if &header[..4] != b"glTF" {
        return Err(GltfError::InvalidGlb(
            "bad magic, expected 'glTF'".to_string(),
        ));
    }
    let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
    if version != 2 {
        return Err(GltfError::InvalidGlb(format!(
            "unsupported glb version {version}, only version 2 is supported"
        )));
    }
    let declared_length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
    if declared_length != glb.len() {
        return Err(GltfError::InvalidGlb(format!(
            "declared length {declared_length} does not match actual length {}",
            glb.len()
        )));
    }
    let (kind, json, mut rest) = read_chunk(&glb[12..])?;
    if kind != JSON_CHUNK {
        return Err(GltfError::InvalidGlb(
            "first chunk must be JSON for glTF 2.0".to_string(),
        ));
    }
    let json = std::str::from_utf8(json)?;
    let mut bin = &[][..];
    if !rest.is_empty() {
        let (kind, data, tail) = read_chunk(rest)?;
        if kind == BIN_CHUNK {
            bin = data;
            rest = tail;
        }
    }
    let mut tail = rest;
    while !tail.is_empty() {
        let (_, _, next) = read_chunk(tail)?;
        tail = next;
    }
    Ok(GlbChunks { json, bin, rest })
}

/// Assemble JSON and BIN with space/zero padding to 4-byte boundaries.
/// An empty BIN is omitted. Panics if the container exceeds GLB's u32 length.
pub fn assemble_glb(json: &str, bin: &[u8]) -> Vec<u8> {
    assemble_glb_with_rest(json, (!bin.is_empty()).then_some(bin), &[])
}

/// Assemble JSON, an optional BIN, and already framed `rest` chunks.
/// `Some(&[])` writes an explicit empty BIN; `None` omits it. `rest` is
/// appended byte-for-byte and must contain valid, 4-byte-aligned chunks.
/// Panics if the container exceeds GLB's u32 length.
pub fn assemble_glb_with_rest(json: &str, bin: Option<&[u8]>, rest: &[u8]) -> Vec<u8> {
    let padded_len = |len: usize| {
        len.checked_add((4 - len % 4) % 4)
            .expect("GLB length overflow")
    };
    let json_len = padded_len(json.len());
    let bin_len = bin.map_or(0, |bin| padded_len(bin.len()));
    let total = [
        20,
        json_len,
        if bin.is_some() { 8 } else { 0 },
        bin_len,
        rest.len(),
    ]
    .into_iter()
    .try_fold(0usize, usize::checked_add)
    .and_then(|len| u32::try_from(len).ok())
    .expect("GLB length exceeds u32");
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&(json_len as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(json.as_bytes());
    out.resize(20 + json_len, b' ');
    if let Some(bin) = bin {
        out.extend_from_slice(&(bin_len as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(bin);
        out.resize(total as usize - rest.len(), 0);
    }
    out.extend_from_slice(rest);
    out
}

/// Rewrite only the JSON, retaining every following byte, including BIN
/// padding, an explicit empty BIN header, and unknown trailing chunks.
pub fn rewrite_json_chunk(
    glb: &[u8],
    f: impl FnOnce(&str) -> Result<String, GltfError>,
) -> Result<Vec<u8>, GltfError> {
    let chunks = split_glb(glb)?;
    let json = f(chunks.json)?;
    Ok(assemble_glb_with_rest(
        &json,
        None,
        &glb[20 + chunks.json.len()..],
    ))
}

#[cfg(test)]
mod tests {
    use super::{assemble_glb, assemble_glb_with_rest, rewrite_json_chunk, split_glb};
    use crate::GltfError;

    const REST: &[u8] = b"\x04\0\0\0TEST\x01\x02\x03\x04\0\0\0\0ZERO";

    #[test]
    fn split_assemble_roundtrip() {
        for suffix in ["", " ", "  ", "   "] {
            let json = format!("{{}}{suffix}");
            for bin_len in 0..8 {
                let bin = vec![0x81; bin_len];
                let glb = assemble_glb(&json, &bin);
                let chunks = split_glb(&glb).unwrap();
                assert_eq!(&chunks.json[..json.len()], json);
                assert!(chunks.json.as_bytes()[json.len()..]
                    .iter()
                    .all(|b| *b == b' '));
                assert_eq!(&chunks.bin[..bin_len], bin);
                assert!(chunks.bin[bin_len..].iter().all(|b| *b == 0));
                assert_eq!(chunks.json.len() % 4, 0);
                assert_eq!(chunks.bin.len() % 4, 0);
                assert!(chunks.rest.is_empty());
                assert_eq!(assemble_glb(chunks.json, chunks.bin), glb);
            }
        }
        assert_eq!(
            assemble_glb("{}", &[]),
            b"glTF\x02\0\0\0\x18\0\0\0\x04\0\0\0JSON{}  "
        );
    }

    #[test]
    fn roundtrip_preserves_extra_chunks_and_empty_bin_presence() {
        for bin in [None, Some(&[][..]), Some(&[1, 2, 3, 0][..])] {
            let glb = assemble_glb_with_rest(r#"{"asset":{"version":"2.0"}}"#, bin, REST);
            let chunks = split_glb(&glb).unwrap();
            assert_eq!(chunks.bin, bin.unwrap_or_default());
            assert_eq!(chunks.rest, REST);
            assert_eq!(assemble_glb_with_rest(chunks.json, bin, chunks.rest), glb);
            let parsed = crate::parse_glb_bytes(&glb).unwrap();
            assert_eq!(parsed.bin_chunk.as_deref(), bin);
            assert_eq!(parsed.extra_chunks.len(), 2);
            assert_eq!(
                parsed.extra_chunks[0].chunk_type,
                u32::from_le_bytes(*b"TEST")
            );
            assert_eq!(parsed.extra_chunks[0].data, [1, 2, 3, 4]);
            assert_eq!(
                parsed.extra_chunks[1].chunk_type,
                u32::from_le_bytes(*b"ZERO")
            );
            assert!(parsed.extra_chunks[1].data.is_empty());
        }
    }

    #[test]
    fn truncated_input_returns_errors() {
        let glb = assemble_glb_with_rest("{}", Some(&[1, 2, 3, 4]), REST);
        for len in 0..glb.len() {
            assert!(split_glb(&glb[..len]).is_err(), "truncated at {len}");
            // Also exercise chunk bounds after repairing the outer length.
            // 24, 36 and 48 are complete JSON/BIN/extra-chunk boundaries.
            if len >= 12 && ![24, 36, 48].contains(&len) {
                let mut truncated = glb[..len].to_vec();
                truncated[8..12].copy_from_slice(&(len as u32).to_le_bytes());
                assert!(split_glb(&truncated).is_err(), "chunk truncated at {len}");
            }
        }
    }

    #[test]
    fn invalid_headers_lengths_and_utf8_return_errors() {
        let glb = assemble_glb_with_rest("{}", Some(&[1, 2, 3, 4]), REST);
        for (offset, value) in [
            (0, 0),
            (4, 1),
            (8, 12),
            (16, 0),
            (12, u32::MAX - 3),
            (24, u32::MAX - 3),
            (36, u32::MAX - 3),
            (12, 3),
            (24, 3),
            (36, 3),
        ] {
            let mut invalid = glb.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(matches!(split_glb(&invalid), Err(GltfError::InvalidGlb(_))));
        }
        let mut invalid = glb;
        invalid[20] = 0xff;
        assert!(matches!(split_glb(&invalid), Err(GltfError::Utf8(_))));
    }

    #[test]
    fn rewrite_keeps_bin_and_later_chunks_byte_identical() {
        for bin in [None, Some(&[][..]), Some(&[1, 2, 3, 0xfe][..])] {
            for rest in [&[][..], REST, &b"\0\0\0\0JSON\0\0\0\0BIN\0"[..]] {
                let glb = assemble_glb_with_rest("{\"v\":0}\t", bin, rest);
                assert_eq!(
                    rewrite_json_chunk(&glb, |json| Ok(json.to_string())).unwrap(),
                    glb
                );
                let before = split_glb(&glb).unwrap();
                let rewritten = rewrite_json_chunk(&glb, |json| {
                    assert_eq!(json, before.json);
                    Ok("{\"value\":12345}".to_string())
                })
                .unwrap();
                let after = split_glb(&rewritten).unwrap();
                assert_eq!(after.json.trim_end(), "{\"value\":12345}");
                assert_eq!(after.bin, before.bin);
                assert_eq!(after.rest, before.rest);
                assert_eq!(
                    &rewritten[20 + after.json.len()..],
                    &glb[20 + before.json.len()..]
                );
            }
        }
    }

    #[test]
    fn rewrite_propagates_errors() {
        let glb = assemble_glb("{}", &[]);
        assert!(matches!(
            rewrite_json_chunk(&glb, |_| Err(GltfError::Validation("edit failed".to_string()))),
            Err(GltfError::Validation(message)) if message == "edit failed"
        ));
        assert!(
            rewrite_json_chunk(&glb[..10], |_| panic!("invalid input must not be edited")).is_err()
        );
    }
}
