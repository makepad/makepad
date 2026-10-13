//! A pack (git's pack-format.txt, version 2/3) read whole in memory: every object inflated,
//! ofs-deltas and ref-deltas applied (a thin pack's bases outside it come from the caller),
//! each object's id computed. No .idx: a fetched pack is read once, front to back.
use super::{hash_object, GitError, Object, ObjectId, ObjectKind};
use std::collections::HashMap;

enum Entry {
    Full { kind: ObjectKind, data: Vec<u8> },
    DeltaOfs { base_offset: u64, delta: Vec<u8> },
    DeltaRef { base_oid: ObjectId, delta: Vec<u8> },
}

struct Resolved {
    kind: ObjectKind,
    data: Vec<u8>,
    oid: ObjectId,
}

fn corrupt(m: impl Into<String>) -> GitError {
    GitError::CorruptPack(m.into())
}

/// Every object of `pack` with its id, deltas resolved in memory (ref-delta bases outside the
/// pack come from `external`), in pack order. Nothing is written anywhere.
pub fn unpack_pack(pack: &[u8], external: &mut dyn FnMut(&ObjectId) -> Option<Object>) -> Result<Vec<(ObjectId, Object)>, GitError> {
    if pack.len() < 32 {
        return Err(corrupt("pack too small"));
    }
    if &pack[..4] != b"PACK" {
        return Err(corrupt("missing PACK header"));
    }
    let version = read_u32_be(pack, 4);
    if version != 2 && version != 3 {
        return Err(corrupt(format!("unsupported pack version {version}")));
    }
    let checksum_at = pack.len() - 20;
    let mut checksum = super::sha1::Sha1::new();
    checksum.update(&pack[..checksum_at]);
    if checksum.finalize() != pack[checksum_at..] {
        return Err(corrupt("pack checksum mismatch"));
    }
    let count = read_u32_be(pack, 8) as usize;
    if count > checksum_at {
        return Err(corrupt("invalid object count"));
    }

    // 1. every entry inflated, in pack order
    let mut pos = 12usize;
    let mut entries: Vec<(u64, Entry)> = Vec::with_capacity(count);
    for _ in 0..count {
        let offset = pos as u64;
        let mut byte = *pack.get(pos).ok_or_else(|| corrupt("unexpected end of pack while reading object header"))?;
        pos += 1;
        let type_num = (byte >> 4) & 0x07;
        let mut size = (byte & 0x0f) as usize;
        let mut shift = 4usize;
        while byte & 0x80 != 0 {
            byte = *pack.get(pos).ok_or_else(|| corrupt("truncated pack size varint"))?;
            pos += 1;
            if shift < usize::BITS as usize {
                size |= ((byte & 0x7f) as usize) << shift;
            }
            shift += 7;
        }
        let entry = match type_num {
            1..=4 => {
                let kind = ObjectKind::from_type_num(type_num)?;
                let data = inflate(pack, &mut pos, size)?;
                Entry::Full { kind, data }
            }
            6 => {
                let mut b = *pack.get(pos).ok_or_else(|| corrupt("truncated ofs-delta base offset"))?;
                pos += 1;
                let mut back = (b & 0x7f) as u64;
                while b & 0x80 != 0 {
                    b = *pack.get(pos).ok_or_else(|| corrupt("truncated ofs-delta base offset"))?;
                    pos += 1;
                    back = ((back + 1) << 7) | (b & 0x7f) as u64;
                }
                let base_offset = offset.checked_sub(back).ok_or_else(|| corrupt("invalid ofs-delta base offset"))?;
                Entry::DeltaOfs { base_offset, delta: inflate(pack, &mut pos, size)? }
            }
            7 => {
                if pos > checksum_at || checksum_at - pos < 20 {
                    return Err(corrupt("truncated ref-delta base oid"));
                }
                let base_oid = ObjectId::from_slice(&pack[pos..pos + 20])?;
                pos += 20;
                Entry::DeltaRef { base_oid, delta: inflate(pack, &mut pos, size)? }
            }
            _ => return Err(corrupt(format!("unsupported pack object type {type_num}"))),
        };
        entries.push((offset, entry));
    }
    if pos != checksum_at {
        return Err(corrupt("pack missing trailing checksum"));
    }

    // 2. resolved in rounds: a delta waits until its base is resolved (or comes from outside)
    let mut by_offset = HashMap::<u64, Resolved>::with_capacity(count);
    let mut oid_offset = HashMap::<ObjectId, u64>::with_capacity(count);
    let mut external_cache = HashMap::<ObjectId, Object>::new();
    let mut unresolved: Vec<usize> = (0..entries.len()).collect();
    while !unresolved.is_empty() {
        let mut next = Vec::new();
        let mut progressed = false;
        for idx in unresolved {
            let (offset, entry) = &mut entries[idx];
            let (kind, data) = match entry {
                Entry::Full { kind, data } => (*kind, std::mem::take(data)),
                Entry::DeltaOfs { base_offset, delta } => {
                    let Some(base) = by_offset.get(base_offset) else {
                        next.push(idx);
                        continue;
                    };
                    (base.kind, apply_delta(&base.data, delta)?)
                }
                Entry::DeltaRef { base_oid, delta } => {
                    if let Some(base) = oid_offset.get(base_oid).and_then(|o| by_offset.get(o)) {
                        (base.kind, apply_delta(&base.data, delta)?)
                    } else {
                        if !external_cache.contains_key(base_oid) {
                            match external(base_oid) {
                                Some(obj) => {
                                    external_cache.insert(*base_oid, obj);
                                }
                                None => {
                                    next.push(idx);
                                    continue;
                                }
                            }
                        }
                        let base = &external_cache[base_oid];
                        (base.kind, apply_delta(&base.data, delta)?)
                    }
                }
            };
            let oid = hash_object(kind, &data);
            oid_offset.insert(oid, *offset);
            by_offset.insert(*offset, Resolved { kind, data, oid });
            progressed = true;
        }
        if !progressed {
            return Err(corrupt("could not resolve all delta objects (thin pack base missing?)"));
        }
        unresolved = next;
    }
    Ok(entries
        .iter()
        .map(|(offset, _)| {
            let r = by_offset.remove(offset).expect("resolved");
            (r.oid, Object { kind: r.kind, data: r.data })
        })
        .collect())
}

/// One zlib stream at `*pos` that inflates to exactly `size` bytes; `*pos` moves past it.
fn inflate(pack: &[u8], pos: &mut usize, size: usize) -> Result<Vec<u8>, GitError> {
    let mut out = vec![0u8; size];
    let (consumed, written) =
        crate::inflate::zlib_decompress(&pack[*pos..], &mut out).map_err(|e| corrupt(format!("zlib decompress failed: {e}")))?;
    if written != size {
        return Err(corrupt(format!("zlib size mismatch: expected {size}, got {written}")));
    }
    *pos += consumed;
    Ok(out)
}

fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, GitError> {
    let mut pos = 0;
    let _base_size = read_delta_size(delta, &mut pos)?;
    let result_size = read_delta_size(delta, &mut pos)? as usize;
    let mut result = Vec::with_capacity(result_size);
    let byte = |pos: &mut usize| -> Result<u32, GitError> {
        let b = *delta.get(*pos).ok_or_else(|| corrupt("delta copy truncated"))?;
        *pos += 1;
        Ok(b as u32)
    };
    while pos < delta.len() {
        let cmd = delta[pos];
        pos += 1;
        if cmd & 0x80 != 0 {
            let mut offset = 0u32;
            let mut size = 0u32;
            for i in 0..4 {
                if cmd & (1 << i) != 0 {
                    offset |= byte(&mut pos)? << (8 * i);
                }
            }
            for i in 0..3 {
                if cmd & (0x10 << i) != 0 {
                    size |= byte(&mut pos)? << (8 * i);
                }
            }
            if size == 0 {
                size = 0x10000;
            }
            let (start, end) = (offset as usize, offset as usize + size as usize);
            if end > base.len() {
                return Err(corrupt(format!("delta copy out of range: {start}..{end} but base len is {}", base.len())));
            }
            result.extend_from_slice(&base[start..end]);
        } else if cmd > 0 {
            let n = cmd as usize;
            let insert = delta.get(pos..pos + n).ok_or_else(|| corrupt("delta insert truncated"))?;
            result.extend_from_slice(insert);
            pos += n;
        } else {
            return Err(corrupt("delta reserved opcode 0x00"));
        }
    }
    if result.len() != result_size {
        return Err(corrupt(format!("delta result size mismatch: expected {result_size}, got {}", result.len())));
    }
    Ok(result)
}

fn read_delta_size(data: &[u8], pos: &mut usize) -> Result<u64, GitError> {
    let mut size = 0u64;
    let mut shift = 0;
    loop {
        let b = *data.get(*pos).ok_or_else(|| corrupt("truncated delta size"))?;
        *pos += 1;
        if shift < 64 {
            size |= ((b & 0x7f) as u64) << shift;
        }
        shift += 7;
        if b & 0x80 == 0 {
            return Ok(size);
        }
    }
}

fn read_u32_be(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_of(entries: &[(u8, Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let mut p = b"PACK\0\0\0\x02".to_vec();
        p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (ty, prefix, data) in entries {
            let mut size = data.len();
            let mut b = (ty << 4) | (size & 0x0f) as u8;
            size >>= 4;
            while size > 0 {
                p.push(b | 0x80);
                b = (size & 0x7f) as u8;
                size >>= 7;
            }
            p.push(b);
            p.extend_from_slice(prefix);
            p.extend_from_slice(&crate::inflate::zlib_compress(data, 6));
        }
        let mut h = super::super::sha1::Sha1::new();
        h.update(&p);
        p.extend_from_slice(&h.finalize());
        p
    }

    #[test]
    fn full_ofs_and_ref_deltas() {
        let base = b"hello world, hello pack\n".to_vec();
        let base_oid = hash_object(ObjectKind::Blob, &base);
        // delta: base 24 -> result 30: copy 0..24, insert "again\n"
        let mut delta = vec![24, 30, 0x90, 24, 6];
        delta.extend_from_slice(b"again\n");
        let want = b"hello world, hello pack\nagain\n".to_vec();
        // the ofs-delta's base sits 12 bytes back (the pack header), at offset 12
        let first_len = pack_of(&[(3, vec![], base.clone())]).len() - 32;
        let back = first_len as u8; // small pack: one byte offset
        assert!(back < 0x80);
        let p = pack_of(&[(3, vec![], base.clone()), (6, vec![back], delta.clone()), (7, base_oid.0.to_vec(), delta.clone())]);
        let objs = unpack_pack(&p, &mut |_| None).unwrap();
        assert_eq!(objs.len(), 3);
        assert_eq!(objs[0].0, base_oid);
        assert_eq!(objs[1].1.data, want);
        assert_eq!(objs[2].1.data, want);
        assert_eq!(objs[1].0, hash_object(ObjectKind::Blob, &want));
        // a thin pack: the ref-delta's base from outside
        let thin = pack_of(&[(7, base_oid.0.to_vec(), delta)]);
        assert!(unpack_pack(&thin, &mut |_| None).is_err());
        let objs = unpack_pack(&thin, &mut |o| (*o == base_oid).then(|| Object { kind: ObjectKind::Blob, data: base.clone() })).unwrap();
        assert_eq!(objs[0].1.data, want);
        // a flipped checksum byte
        let mut bad = p.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(unpack_pack(&bad, &mut |_| None).is_err());
    }
}
