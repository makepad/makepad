//! Westwood MIX archives, including the encrypted Red Alert/TS header form.

use super::bignum::BigNum;
use super::blowfish::Blowfish;
use std::collections::{HashMap, HashSet};
use std::fmt;

const FLAG_CHECKSUM: u16 = 1;
const FLAG_ENCRYPTED: u16 = 2;
const KEY_SOURCE_LEN: usize = 80;
const RSA_BLOCK_LEN: usize = 40;
const RSA_OUTPUT_LEN: usize = 39;
const BLOWFISH_KEY_LEN: usize = 56;
const DIGEST_LEN: usize = 20;
const PUBLIC_KEY_BASE64: &str =
    "AihRvNoIbTn85FZRYNZRcT+i6KpU+maCsEqr3Q5q+LDB5tH7Tz2qQ38V";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixEntry {
    pub id: u32,
    /// Byte offset relative to the beginning of the MIX data block.
    pub offset: u32,
    pub size: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixHeaderKind {
    Plain,
    Checksum,
    Encrypted,
    EncryptedChecksum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HashKind {
    RotateAdd,
    Crc32,
}

impl HashKind {
    pub fn hash(self, name: &str) -> u32 {
        match self {
            Self::RotateAdd => mix_id(name),
            Self::Crc32 => mix_id_crc(name),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MixError {
    Truncated,
    /// Retained for compatibility with the old four-byte-only diagnostic.
    Encrypted,
    InvalidKeySource,
    InvalidHeader,
    InvalidEntry { id: u32 },
}

impl fmt::Display for MixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated MIX archive"),
            Self::Encrypted => f.write_str("encrypted MIX archive has no key source"),
            Self::InvalidKeySource => f.write_str("invalid encrypted MIX key source"),
            Self::InvalidHeader => f.write_str("invalid MIX header"),
            Self::InvalidEntry { id } => write!(f, "MIX entry {id:08x} is outside the data block"),
        }
    }
}

impl std::error::Error for MixError {}

#[derive(Clone, Debug)]
pub struct MixFile<'a> {
    bytes: &'a [u8],
    entries: Vec<MixEntry>,
    data_offset: usize,
    data_size: usize,
    digest_len: usize,
    header_kind: MixHeaderKind,
}

impl<'a> MixFile<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, MixError> {
        let first = read_u16(bytes, 0).ok_or(MixError::Truncated)?;
        if first != 0 {
            return Self::parse_header(bytes, bytes, 0, MixHeaderKind::Plain, 0, None);
        }

        let flags = read_u16(bytes, 2).ok_or(MixError::Truncated)?;
        let checksum = flags & FLAG_CHECKSUM != 0;
        if flags & FLAG_ENCRYPTED == 0 {
            let kind = if checksum {
                MixHeaderKind::Checksum
            } else {
                MixHeaderKind::Plain
            };
            return Self::parse_header(
                bytes,
                bytes,
                4,
                kind,
                usize::from(checksum) * DIGEST_LEN,
                None,
            );
        }
        if bytes.len() == 4 {
            return Err(MixError::Encrypted);
        }
        let key_source_end = 4usize.checked_add(KEY_SOURCE_LEN).ok_or(MixError::Truncated)?;
        let key_source = bytes.get(4..key_source_end).ok_or(MixError::Truncated)?;
        let key = derive_blowfish_key(key_source)?;
        let blowfish = Blowfish::new(&key).ok_or(MixError::InvalidKeySource)?;
        let first_encrypted_end = key_source_end.checked_add(8).ok_or(MixError::Truncated)?;
        let first_block = decrypt_blocks(
            blowfish.clone(),
            bytes
                .get(key_source_end..first_encrypted_end)
                .ok_or(MixError::Truncated)?,
        )?;
        let count = read_u16(&first_block, 0).ok_or(MixError::Truncated)? as usize;
        let header_len = count
            .checked_mul(12)
            .and_then(|size| size.checked_add(6))
            .ok_or(MixError::InvalidHeader)?;
        let encrypted_len = header_len.checked_add(7).ok_or(MixError::InvalidHeader)? & !7;
        let encrypted_end = key_source_end
            .checked_add(encrypted_len)
            .ok_or(MixError::Truncated)?;
        let encrypted = bytes
            .get(key_source_end..encrypted_end)
            .ok_or(MixError::Truncated)?;
        let decrypted = decrypt_blocks(blowfish, encrypted)?;
        let kind = if checksum {
            MixHeaderKind::EncryptedChecksum
        } else {
            MixHeaderKind::Encrypted
        };
        Self::parse_header(
            bytes,
            &decrypted,
            0,
            kind,
            usize::from(checksum) * DIGEST_LEN,
            Some(encrypted_end),
        )
    }

    fn parse_header(
        bytes: &'a [u8],
        header: &[u8],
        header_offset: usize,
        header_kind: MixHeaderKind,
        digest_len: usize,
        data_offset_override: Option<usize>,
    ) -> Result<Self, MixError> {
        let file_count = read_u16(header, header_offset).ok_or(MixError::Truncated)? as usize;
        let data_size = read_u32(header, header_offset + 2).ok_or(MixError::Truncated)? as usize;
        let index_size = file_count.checked_mul(12).ok_or(MixError::InvalidHeader)?;
        let header_end = header_offset
            .checked_add(6)
            .and_then(|value| value.checked_add(index_size))
            .ok_or(MixError::InvalidHeader)?;
        if header_end > header.len() {
            return Err(MixError::Truncated);
        }
        let data_offset = data_offset_override.unwrap_or(header_end);
        let data_end = data_offset
            .checked_add(data_size)
            .and_then(|value| value.checked_add(digest_len))
            .ok_or(MixError::Truncated)?;
        if data_end > bytes.len() {
            return Err(MixError::Truncated);
        }

        let mut entries = Vec::with_capacity(file_count);
        for index in 0..file_count {
            let at = header_offset + 6 + index * 12;
            let entry = MixEntry {
                id: read_u32(header, at).ok_or(MixError::Truncated)?,
                offset: read_u32(header, at + 4).ok_or(MixError::Truncated)?,
                size: read_u32(header, at + 8).ok_or(MixError::Truncated)?,
            };
            let end = (entry.offset as usize)
                .checked_add(entry.size as usize)
                .ok_or(MixError::InvalidEntry { id: entry.id })?;
            if end > data_size {
                return Err(MixError::InvalidEntry { id: entry.id });
            }
            entries.push(entry);
        }
        Ok(Self {
            bytes,
            entries,
            data_offset,
            data_size,
            digest_len,
            header_kind,
        })
    }

    pub fn header_kind(&self) -> MixHeaderKind {
        self.header_kind
    }

    pub fn entries(&self) -> &[MixEntry] {
        &self.entries
    }

    pub fn data_size(&self) -> usize {
        self.data_size
    }

    pub fn has_digest(&self) -> bool {
        self.digest_len != 0
    }

    pub fn by_id(&self, id: u32) -> Option<&'a [u8]> {
        let entry = self.entries.iter().find(|entry| entry.id == id)?;
        let start = self.data_offset.checked_add(entry.offset as usize)?;
        let end = start.checked_add(entry.size as usize)?;
        self.bytes.get(start..end)
    }

    pub fn by_name(&self, name: &str) -> Option<&'a [u8]> {
        self.by_name_with_hash(name, HashKind::RotateAdd)
    }

    pub fn by_name_with_hash(&self, name: &str, hash_kind: HashKind) -> Option<&'a [u8]> {
        self.by_id(hash_kind.hash(name))
    }

    pub fn mix_by_name(&self, name: &str) -> Result<Option<MixFile<'a>>, MixError> {
        self.mix_by_name_with_hash(name, HashKind::RotateAdd)
    }

    pub fn mix_by_name_with_hash(
        &self,
        name: &str,
        hash_kind: HashKind,
    ) -> Result<Option<MixFile<'a>>, MixError> {
        self.by_name_with_hash(name, hash_kind)
            .map(MixFile::parse)
            .transpose()
    }

    pub fn by_name_recursive(
        &self,
        path: &[&str],
        hash_kind: HashKind,
    ) -> Result<Option<&'a [u8]>, MixError> {
        let Some((&last, parents)) = path.split_last() else {
            return Ok(None);
        };
        let mut current = self.clone();
        for &parent in parents {
            let Some(next) = current.mix_by_name_with_hash(parent, hash_kind)? else {
                return Ok(None);
            };
            current = next;
        }
        Ok(current.by_name_with_hash(last, hash_kind))
    }
}

/// Computes the rotate-add identifier used by Tiberian Dawn and Red Alert.
pub fn mix_id(name: &str) -> u32 {
    let upper = name.as_bytes().iter().map(u8::to_ascii_uppercase).collect::<Vec<_>>();
    let mut id = 0u32;
    for chunk in upper.chunks(4) {
        let mut padded = [0u8; 4];
        padded[..chunk.len()].copy_from_slice(chunk);
        id = id.rotate_left(1).wrapping_add(u32::from_le_bytes(padded));
    }
    id
}

/// Computes the padded CRC32 identifier used by Tiberian Sun MIX archives.
pub fn mix_id_crc(name: &str) -> u32 {
    let mut padded = name
        .as_bytes()
        .iter()
        .map(u8::to_ascii_uppercase)
        .collect::<Vec<_>>();
    let original_len = padded.len();
    if original_len % 4 != 0 {
        padded.push((original_len & 3) as u8);
        let repeated = padded[original_len & !3];
        while padded.len() % 4 != 0 {
            padded.push(repeated);
        }
    }
    crc32_ieee(&padded)
}

pub fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[derive(Clone, Debug)]
pub struct NameTable {
    names: Vec<&'static str>,
    by_id: HashMap<u32, &'static str>,
    hash_kind: HashKind,
}

impl NameTable {
    /// The original TD dictionary and rotate-add hash.
    pub fn new() -> Self {
        Self::from_texts(&[include_str!("names.txt")], HashKind::RotateAdd)
    }

    /// Selects the RA rotate-add or TS CRC32 dictionary.
    pub fn with_hash_kind(hash_kind: HashKind) -> Self {
        let texts: &[&'static str] = match hash_kind {
            // The copied RA list contains theater assets while the verified
            // legacy list supplies campaign-map names; RA needs their union.
            HashKind::RotateAdd => &[include_str!("names-ra.txt"), include_str!("names.txt")],
            HashKind::Crc32 => &[include_str!("names-ts.txt")],
        };
        Self::from_texts(texts, hash_kind)
    }

    pub fn for_hash_kind(hash_kind: HashKind) -> Self {
        Self::with_hash_kind(hash_kind)
    }

    fn from_texts(texts: &[&'static str], hash_kind: HashKind) -> Self {
        let mut seen = HashSet::new();
        let names = texts
            .iter()
            .flat_map(|text| text.lines())
            .map(str::trim)
            .filter(|name| !name.is_empty() && seen.insert(*name))
            .collect::<Vec<_>>();
        let mut by_id = HashMap::with_capacity(names.len());
        for &name in &names {
            by_id.entry(hash_kind.hash(name)).or_insert(name);
        }
        Self {
            names,
            by_id,
            hash_kind,
        }
    }

    pub fn hash_kind(&self) -> HashKind {
        self.hash_kind
    }

    pub fn name_of(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).copied()
    }

    pub fn resolve_names<'b>(&'b self, mix: &MixFile<'_>) -> Vec<(u32, Option<&'b str>)> {
        mix.entries()
            .iter()
            .map(|entry| (entry.id, self.name_of(entry.id)))
            .collect()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.names.iter().copied()
    }
}

impl Default for NameTable {
    fn default() -> Self {
        Self::new()
    }
}

/// MIX key chunks are little-endian integers while the ASN.1 modulus is
/// big-endian. RSA results are exported as 39 little-endian bytes per block,
/// matching the original x86 multiprecision representation. Blowfish ECB
/// words use standard big-endian byte order. These choices were pinned by
/// decrypting the freeware RA `conquer.mix`: it has 229 entries, its length is
/// exactly `2_844 + 2_174_183 + 20`, and 190 names from the supplied combined
/// RA/campaign dictionaries resolve.
fn derive_blowfish_key(key_source: &[u8]) -> Result<[u8; BLOWFISH_KEY_LEN], MixError> {
    if key_source.len() != KEY_SOURCE_LEN {
        return Err(MixError::InvalidKeySource);
    }
    let decoded = decode_base64(PUBLIC_KEY_BASE64).ok_or(MixError::InvalidKeySource)?;
    if decoded.len() != 42 || decoded[..2] != [0x02, 0x28] {
        return Err(MixError::InvalidKeySource);
    }
    let modulus = BigNum::from_bytes_be(&decoded[2..]);
    let mut expanded = Vec::with_capacity(RSA_OUTPUT_LEN * 2);
    for block in key_source.chunks_exact(RSA_BLOCK_LEN) {
        let encrypted = BigNum::from_bytes_le(block);
        let decrypted = encrypted
            .powmod(65_537, &modulus)
            .ok_or(MixError::InvalidKeySource)?;
        let output = decrypted.to_bytes_le(RSA_BLOCK_LEN);
        expanded.extend_from_slice(&output[..RSA_OUTPUT_LEN]);
    }
    let mut key = [0; BLOWFISH_KEY_LEN];
    key.copy_from_slice(&expanded[..BLOWFISH_KEY_LEN]);
    Ok(key)
}

fn decrypt_blocks(blowfish: Blowfish, encrypted: &[u8]) -> Result<Vec<u8>, MixError> {
    if encrypted.len() % 8 != 0 {
        return Err(MixError::InvalidHeader);
    }
    let mut output = Vec::with_capacity(encrypted.len());
    for block in encrypted.chunks_exact(8) {
        let left_bytes: [u8; 4] = block[..4].try_into().map_err(|_| MixError::Truncated)?;
        let right_bytes: [u8; 4] = block[4..].try_into().map_err(|_| MixError::Truncated)?;
        let (left, right) = blowfish.decrypt_words(
            u32::from_be_bytes(left_bytes),
            u32::from_be_bytes(right_bytes),
        );
        output.extend_from_slice(&left.to_be_bytes());
        output.extend_from_slice(&right.to_be_bytes());
    }
    Ok(output)
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        bits = (bits << 6) | value as u32;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            output.push((bits >> bit_count) as u8);
            bits &= (1 << bit_count) - 1;
        }
    }
    Some(output)
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_mix_hashes_are_pinned_to_the_freeware_archives() {
        assert_eq!(mix_id("CONQUER.MIX"), 0xa236_1104);
        assert_eq!(mix_id("mtnk.shp"), 0xe6e4_fbc8);
        assert_eq!(mix_id("B1.TEM"), 0xa85c_afc9);
        assert_eq!(mix_id("ACKNO.AUD"), 0xe3af_69e7);
    }

    #[test]
    fn cnc_import_mix_rejects_encrypted_later_header() {
        assert!(matches!(
            MixFile::parse(&[0, 0, 2, 0]),
            Err(MixError::Encrypted)
        ));
    }

    #[test]
    fn cnc_import_crc32_known_answer() {
        assert_eq!(crc32_ieee(b"123456789"), 0xcbf4_3926);
        // This padded (9-byte) TS name resolves in the local conquer.mix.
        assert_eq!(mix_id_crc("120MM.SHP"), 0x17ed_bda8);
    }
}
