//! Residue decode: the spectral detail layered under the floor.
//!
//! All three residue types share one classification walk and differ only in how
//! a partition's codewords scatter into the vector.
//!
//! Adapted from the same author's sandbox decoder, with the per-channel
//! `Vec<Vec<f32>>` replaced by one flat channel-major buffer so a block costs
//! no allocations.

use super::bits::BitReader;
use super::codebook::Codebook;
use crate::error::AudioError;

/// Working buffers, owned by the decoder and reused for every packet.
#[derive(Default)]
pub struct ResidueScratch {
    /// Partition classes, `classes[member * class_stride + partition]`.
    classes: Vec<u32>,
    /// Type 2's interleaved staging vector.
    flat: Vec<f32>,
}

impl ResidueScratch {
    /// Grow once, up front, so no packet ever allocates.
    pub fn reserve(&mut self, classes: usize, flat: usize) {
        if self.classes.len() < classes {
            self.classes.resize(classes, 0);
        }
        if self.flat.len() < flat {
            self.flat.resize(flat, 0.0);
        }
    }
}

pub struct Residue {
    kind: u16,
    begin: u32,
    end: u32,
    partition_size: u32,
    classifications: u32,
    classbook: usize,
    /// `books[class][pass]`, -1 where a pass has no book.
    books: Vec<[i32; 8]>,
}

impl Residue {
    pub fn read(r: &mut BitReader, codebook_count: usize) -> Result<Residue, AudioError> {
        let kind = r.read(16).ok_or(AudioError::Truncated)? as u16;
        if kind > 2 {
            return Err(AudioError::Corrupt("residue type"));
        }
        let begin = r.read(24).ok_or(AudioError::Truncated)?;
        let end = r.read(24).ok_or(AudioError::Truncated)?;
        let partition_size = r.read(24).ok_or(AudioError::Truncated)? + 1;
        let classifications = r.read(6).ok_or(AudioError::Truncated)? + 1;
        let classbook = r.read(8).ok_or(AudioError::Truncated)? as usize;
        if classbook >= codebook_count || end < begin {
            return Err(AudioError::Corrupt("residue header"));
        }
        let mut cascade = vec![0u32; classifications as usize];
        for c in cascade.iter_mut() {
            let low = r.read(3).ok_or(AudioError::Truncated)?;
            let bitflag = r.read_bit().ok_or(AudioError::Truncated)?;
            let high = if bitflag { r.read(5).ok_or(AudioError::Truncated)? } else { 0 };
            *c = high * 8 + low;
        }
        let mut books = Vec::with_capacity(classifications as usize);
        for &c in &cascade {
            let mut row = [-1i32; 8];
            for (j, slot) in row.iter_mut().enumerate() {
                if c & (1 << j) != 0 {
                    let b = r.read(8).ok_or(AudioError::Truncated)? as i32;
                    if b as usize >= codebook_count {
                        return Err(AudioError::Corrupt("residue book"));
                    }
                    *slot = b;
                }
            }
            books.push(row);
        }
        Ok(Residue { kind, begin, end, partition_size, classifications, classbook, books })
    }

    /// Largest scratch this residue can ask for, so the decoder can size its
    /// buffers from the headers instead of growing them mid-stream.
    pub fn scratch_needed(&self, half: usize, channels: usize, books: &[Codebook]) -> (usize, usize) {
        let classwords = books.get(self.classbook).map(|b| b.dimensions as usize).unwrap_or(0);
        let total = if self.kind == 2 { half * channels } else { half };
        let begin = (self.begin as usize).min(total);
        let end = (self.end as usize).min(total);
        let partitions = end.saturating_sub(begin) / self.partition_size.max(1) as usize;
        let stride = partitions + classwords;
        let flat = if self.kind == 2 { half * channels } else { 0 };
        (stride * channels.max(1), flat)
    }

    /// Decode this submap's residue into `out`, a channel-major buffer of
    /// `stride` values per channel. `members[k]` is the channel index of the
    /// k-th channel in this submap and `skip[k]` marks the ones whose floor
    /// said "silent".
    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &self,
        r: &mut BitReader,
        books: &[Codebook],
        out: &mut [f32],
        stride: usize,
        members: &[usize],
        skip: &[bool],
        n: usize,
        scratch: &mut ResidueScratch,
    ) -> Result<(), AudioError> {
        if self.kind == 2 {
            return self.decode_interleaved(r, books, out, stride, members, skip, n, scratch);
        }
        let ch = members.len();
        if ch == 0 {
            return Ok(());
        }
        let begin = (self.begin as usize).min(n);
        let end = (self.end as usize).min(n);
        if end <= begin {
            return Ok(());
        }
        let classbook = books.get(self.classbook).ok_or(AudioError::Corrupt("residue book"))?;
        let classwords = classbook.dimensions as usize;
        let partitions_to_read = (end - begin) / self.partition_size as usize;
        if partitions_to_read == 0 || classwords == 0 {
            return Ok(());
        }
        let cstride = partitions_to_read + classwords;
        scratch.reserve(cstride * ch, 0);
        let classes = &mut scratch.classes[..cstride * ch];
        classes.fill(0);

        for pass in 0..8usize {
            let mut partition_count = 0usize;
            while partition_count < partitions_to_read {
                if pass == 0 {
                    for j in 0..ch {
                        if skip[j] {
                            continue;
                        }
                        let mut temp = classbook.decode(r)?;
                        for i in (0..classwords).rev() {
                            classes[j * cstride + partition_count + i] =
                                temp % self.classifications;
                            temp /= self.classifications;
                        }
                    }
                }
                for _ in 0..classwords {
                    if partition_count >= partitions_to_read {
                        break;
                    }
                    for (j, &c) in members.iter().enumerate() {
                        if skip[j] {
                            continue;
                        }
                        let vqclass = classes[j * cstride + partition_count] as usize;
                        let book = self
                            .books
                            .get(vqclass)
                            .map(|row| row[pass])
                            .ok_or(AudioError::Corrupt("residue class"))?;
                        if book >= 0 {
                            let cb =
                                books.get(book as usize).ok_or(AudioError::Corrupt("residue book"))?;
                            let offset = begin + partition_count * self.partition_size as usize;
                            let base = c * stride;
                            let v = out
                                .get_mut(base..base + stride)
                                .ok_or(AudioError::Corrupt("residue channel"))?;
                            self.decode_partition(r, cb, v, offset)?;
                        }
                    }
                    partition_count += 1;
                }
            }
        }
        Ok(())
    }

    /// Type 2 flattens every channel into one vector, decodes, then splits.
    #[allow(clippy::too_many_arguments)]
    fn decode_interleaved(
        &self,
        r: &mut BitReader,
        books: &[Codebook],
        out: &mut [f32],
        stride: usize,
        members: &[usize],
        skip: &[bool],
        n: usize,
        scratch: &mut ResidueScratch,
    ) -> Result<(), AudioError> {
        let ch = members.len();
        if ch == 0 || skip.iter().all(|&s| s) {
            // Nothing in the packet belongs to us.
            return Ok(());
        }
        let total = n * ch;
        let begin = (self.begin as usize).min(total);
        let end = (self.end as usize).min(total);
        if end <= begin {
            return Ok(());
        }
        let classbook = books.get(self.classbook).ok_or(AudioError::Corrupt("residue book"))?;
        let classwords = classbook.dimensions as usize;
        let partitions_to_read = (end - begin) / self.partition_size as usize;
        if partitions_to_read == 0 || classwords == 0 {
            return Ok(());
        }
        scratch.reserve(partitions_to_read + classwords, total);
        let classes = &mut scratch.classes[..partitions_to_read + classwords];
        classes.fill(0);
        let flat = &mut scratch.flat[..total];
        flat.fill(0.0);

        for pass in 0..8usize {
            let mut partition_count = 0usize;
            while partition_count < partitions_to_read {
                if pass == 0 {
                    let mut temp = classbook.decode(r)?;
                    for i in (0..classwords).rev() {
                        classes[partition_count + i] = temp % self.classifications;
                        temp /= self.classifications;
                    }
                }
                for _ in 0..classwords {
                    if partition_count >= partitions_to_read {
                        break;
                    }
                    let vqclass = classes[partition_count] as usize;
                    let book = self
                        .books
                        .get(vqclass)
                        .map(|row| row[pass])
                        .ok_or(AudioError::Corrupt("residue class"))?;
                    if book >= 0 {
                        let cb =
                            books.get(book as usize).ok_or(AudioError::Corrupt("residue book"))?;
                        let offset = begin + partition_count * self.partition_size as usize;
                        decode_partition_type1(r, cb, flat, offset, self.partition_size as usize)?;
                    }
                    partition_count += 1;
                }
            }
        }

        for (i, v) in flat.iter().enumerate() {
            let k = i % ch;
            let pos = i / ch;
            if pos < n {
                if let Some(slot) = out.get_mut(members[k] * stride + pos) {
                    *slot += v;
                }
            }
        }
        Ok(())
    }

    fn decode_partition(
        &self,
        r: &mut BitReader,
        cb: &Codebook,
        v: &mut [f32],
        offset: usize,
    ) -> Result<(), AudioError> {
        if self.kind != 0 {
            return decode_partition_type1(r, cb, v, offset, self.partition_size as usize);
        }
        // Type 0 scatters one codeword's dimensions across the partition with a
        // stride, rather than laying them down consecutively.
        let dim = cb.dimensions as usize;
        if dim == 0 {
            return Err(AudioError::Corrupt("residue dimension"));
        }
        let step = self.partition_size as usize / dim;
        for i in 0..step {
            let vector = cb.decode_vector(r)?;
            for (j, val) in vector.iter().enumerate() {
                let idx = offset + i + j * step;
                if idx < v.len() {
                    v[idx] += val;
                }
            }
        }
        Ok(())
    }
}

/// Fill a whole partition: codewords are read until `partition_size` values
/// have been written, not just one vector's worth.
fn decode_partition_type1(
    r: &mut BitReader,
    cb: &Codebook,
    v: &mut [f32],
    offset: usize,
    partition_size: usize,
) -> Result<(), AudioError> {
    let dim = cb.dimensions as usize;
    if dim == 0 {
        return Err(AudioError::Corrupt("residue dimension"));
    }
    let mut i = 0usize;
    while i < partition_size {
        let vector = cb.decode_vector(r)?;
        for &val in vector.iter() {
            if i >= partition_size {
                break;
            }
            let idx = offset + i;
            if idx < v.len() {
                v[idx] += val;
            }
            i += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitstream(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        for &(val, n) in fields {
            for i in 0..n {
                bits.push((val >> i) & 1 == 1);
            }
        }
        let mut bytes = vec![0u8; bits.len().div_ceil(8) + 8];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        bytes
    }

    #[test]
    fn rejects_unknown_residue_type() {
        let bytes = bitstream(&[(9, 16)]);
        let mut r = BitReader::new(&bytes);
        assert!(matches!(Residue::read(&mut r, 4), Err(AudioError::Corrupt(_))));
    }

    #[test]
    fn rejects_end_before_begin() {
        let bytes = bitstream(&[(1, 16), (500, 24), (100, 24), (0, 24), (0, 6), (0, 8)]);
        let mut r = BitReader::new(&bytes);
        assert!(matches!(Residue::read(&mut r, 4), Err(AudioError::Corrupt(_))));
    }

    #[test]
    fn rejects_a_classbook_that_does_not_exist() {
        let bytes = bitstream(&[(1, 16), (0, 24), (100, 24), (0, 24), (0, 6), (200, 8)]);
        let mut r = BitReader::new(&bytes);
        assert!(matches!(Residue::read(&mut r, 4), Err(AudioError::Corrupt(_))));
    }

    #[test]
    fn header_round_trips_its_fields() {
        // One classification whose cascade is 1 (pass 0 only), book 2.
        let bytes = bitstream(&[
            (1, 16),
            (0, 24),
            (128, 24),
            (15, 24), // partition size - 1
            (0, 6),   // classifications - 1
            (1, 8),   // classbook
            (1, 3),   // cascade low bits
            (0, 1),   // no high bits
            (2, 8),   // book for pass 0
        ]);
        let mut r = BitReader::new(&bytes);
        let res = Residue::read(&mut r, 4).unwrap();
        assert_eq!((res.kind, res.begin, res.end), (1, 0, 128));
        assert_eq!(res.partition_size, 16);
        assert_eq!(res.classifications, 1);
        assert_eq!(res.classbook, 1);
        assert_eq!(res.books[0][0], 2);
        assert_eq!(res.books[0][1], -1);
    }

    #[test]
    fn truncated_header_is_truncated_not_a_panic() {
        for len in 0..12usize {
            let bytes = bitstream(&[(1, 16), (0, 24), (128, 24)])[..len].to_vec();
            let mut r = BitReader::new(&bytes);
            assert!(Residue::read(&mut r, 4).is_err());
        }
    }
}
