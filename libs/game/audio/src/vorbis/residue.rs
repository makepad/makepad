//! Residue decode: the spectral detail layered under the floor.
//!
//! All three residue types share one classification walk and differ only in
//! how a partition's codewords scatter into the vector.

use super::codebook::Codebook;
use crate::bitread::BitReader;
use crate::AudioError;

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
            return Err(AudioError::Malformed);
        }
        let begin = r.read(24).ok_or(AudioError::Truncated)?;
        let end = r.read(24).ok_or(AudioError::Truncated)?;
        let partition_size = r.read(24).ok_or(AudioError::Truncated)? + 1;
        let classifications = r.read(6).ok_or(AudioError::Truncated)? + 1;
        let classbook = r.read(8).ok_or(AudioError::Truncated)? as usize;
        if classbook >= codebook_count || end < begin || partition_size == 0 {
            return Err(AudioError::Malformed);
        }
        let mut cascade = vec![0u32; classifications as usize];
        for c in cascade.iter_mut() {
            let low = r.read(3).ok_or(AudioError::Truncated)?;
            let bitflag = r.read_bit().ok_or(AudioError::Truncated)?;
            let high = if bitflag {
                r.read(5).ok_or(AudioError::Truncated)?
            } else {
                0
            };
            *c = high * 8 + low;
        }
        let mut books = Vec::with_capacity(classifications as usize);
        for &c in &cascade {
            let mut row = [-1i32; 8];
            for (j, slot) in row.iter_mut().enumerate() {
                if c & (1 << j) != 0 {
                    let b = r.read(8).ok_or(AudioError::Truncated)? as i32;
                    if b as usize >= codebook_count {
                        return Err(AudioError::Malformed);
                    }
                    *slot = b;
                }
            }
            books.push(row);
        }
        Ok(Residue {
            kind,
            begin,
            end,
            partition_size,
            classifications,
            classbook,
            books,
        })
    }

    /// Decode into `vectors` (one per channel, each `n` long). `skip[j]` marks
    /// channels whose floor said "silent".
    pub fn decode(
        &self,
        r: &mut BitReader,
        books: &[Codebook],
        vectors: &mut [Vec<f32>],
        skip: &[bool],
        n: usize,
    ) -> Result<(), AudioError> {
        if self.kind == 2 {
            return self.decode_interleaved(r, books, vectors, skip, n);
        }
        let ch = vectors.len();
        let (begin, end) = self.window(n);
        if end <= begin {
            return Ok(());
        }
        let classbook = books.get(self.classbook).ok_or(AudioError::Malformed)?;
        let classwords = classbook.dimensions as usize;
        let partitions_to_read = ((end - begin) / self.partition_size as usize).max(0);
        if partitions_to_read == 0 || classwords == 0 {
            return Ok(());
        }
        let mut classes = vec![vec![0u32; partitions_to_read + classwords]; ch];

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
                            classes[j][partition_count + i] = temp % self.classifications;
                            temp /= self.classifications;
                        }
                    }
                }
                for _ in 0..classwords {
                    if partition_count >= partitions_to_read {
                        break;
                    }
                    for j in 0..ch {
                        if skip[j] {
                            continue;
                        }
                        let vqclass = classes[j][partition_count] as usize;
                        let book = self
                            .books
                            .get(vqclass)
                            .map(|row| row[pass])
                            .ok_or(AudioError::Malformed)?;
                        if book >= 0 {
                            let cb = books.get(book as usize).ok_or(AudioError::Malformed)?;
                            let offset = begin + partition_count * self.partition_size as usize;
                            self.decode_partition(r, cb, &mut vectors[j], offset)?;
                        }
                    }
                    partition_count += 1;
                }
            }
        }
        Ok(())
    }

    /// Type 2 flattens every channel into one vector, decodes, then splits.
    fn decode_interleaved(
        &self,
        r: &mut BitReader,
        books: &[Codebook],
        vectors: &mut [Vec<f32>],
        skip: &[bool],
        n: usize,
    ) -> Result<(), AudioError> {
        let ch = vectors.len();
        if ch == 0 {
            return Ok(());
        }
        // If every channel is silent there is nothing in the packet for us.
        if skip.iter().all(|&s| s) {
            return Ok(());
        }
        let total = n * ch;
        let begin = (self.begin as usize).min(total);
        let end = (self.end as usize).min(total);
        if end <= begin {
            return Ok(());
        }
        let classbook = books.get(self.classbook).ok_or(AudioError::Malformed)?;
        let classwords = classbook.dimensions as usize;
        let partitions_to_read = (end - begin) / self.partition_size as usize;
        if partitions_to_read == 0 || classwords == 0 {
            return Ok(());
        }
        let mut flat = vec![0f32; total];
        let mut classes = vec![0u32; partitions_to_read + classwords];

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
                        .ok_or(AudioError::Malformed)?;
                    if book >= 0 {
                        let cb = books.get(book as usize).ok_or(AudioError::Malformed)?;
                        let offset = begin + partition_count * self.partition_size as usize;
                        decode_partition_type1(
                            r,
                            cb,
                            &mut flat,
                            offset,
                            self.partition_size as usize,
                        )?;
                    }
                    partition_count += 1;
                }
            }
        }

        for (i, v) in flat.iter().enumerate() {
            let c = i % ch;
            let pos = i / ch;
            if pos < n {
                vectors[c][pos] += v;
            }
        }
        Ok(())
    }

    fn window(&self, n: usize) -> (usize, usize) {
        let begin = (self.begin as usize).min(n);
        let end = (self.end as usize).min(n);
        (begin, end)
    }

    fn decode_partition(
        &self,
        r: &mut BitReader,
        cb: &Codebook,
        v: &mut [f32],
        offset: usize,
    ) -> Result<(), AudioError> {
        if self.kind == 0 {
            let dim = cb.dimensions as usize;
            if dim == 0 {
                return Err(AudioError::Malformed);
            }
            let step = self.partition_size as usize / dim;
            for i in 0..step {
                let vec = cb.decode_vector(r)?.to_vec();
                for (j, val) in vec.iter().enumerate() {
                    let idx = offset + i + j * step;
                    if idx < v.len() {
                        v[idx] += val;
                    }
                }
            }
            Ok(())
        } else {
            decode_partition_type1(r, cb, v, offset, self.partition_size as usize)
        }
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
        return Err(AudioError::Malformed);
    }
    let mut i = 0usize;
    while i < partition_size {
        let entry = cb.decode(r)? as usize;
        let start = entry.checked_mul(dim).ok_or(AudioError::Malformed)?;
        let vec = cb
            .vectors
            .get(start..start + dim)
            .ok_or(AudioError::Malformed)?;
        for &val in vec.iter() {
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

    #[test]
    fn rejects_unknown_residue_type() {
        // 16 bits = 9 is not a valid residue type.
        let mut data = vec![0u8; 32];
        data[0] = 9;
        let mut r = BitReader::new(&data);
        assert!(Residue::read(&mut r, 4).is_err());
    }

    #[test]
    fn rejects_end_before_begin() {
        // Build a header with begin > end by hand.
        let mut bits: Vec<bool> = Vec::new();
        let mut push = |val: u32, n: u32| {
            for i in 0..n {
                bits.push((val >> i) & 1 == 1);
            }
        };
        push(1, 16); // type 1
        push(500, 24); // begin
        push(100, 24); // end  (< begin)
        push(0, 24); // partition size - 1
        push(0, 6); // classifications - 1
        push(0, 8); // classbook
        let mut bytes = vec![0u8; bits.len().div_ceil(8) + 4];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        let mut r = BitReader::new(&bytes);
        assert!(Residue::read(&mut r, 4).is_err());
    }
}
