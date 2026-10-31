//! Tests: hand-built frames from a small test-side encoder (bit writers, FSE / Huffman encoders that invert
//! the decoder's own tables), a randomized round trip, and, when the `zstd` tool exists, real fixtures.

use super::*;

// ---------------------------------------------------------------------------------------------------------
// test-side writers

/// Backward bitstream: fields pushed in the order the decoder reads them.
#[derive(Default)]
struct Back(Vec<(u64, u32)>);

impl Back {
    fn put(&mut self, v: u64, n: u32) {
        assert!(n == 64 || v < (1u64 << n), "value {} does not fit {} bits", v, n);
        self.0.push((v, n));
    }
    fn finish(&self) -> Vec<u8> {
        let mut bits = Vec::new();
        for &(v, n) in self.0.iter().rev() {
            for k in 0..n {
                bits.push((v >> k) & 1 == 1);
            }
        }
        bits.push(true);
        pack(&bits)
    }
}

fn pack(bits: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    for (i, &b) in bits.iter().enumerate() {
        if b {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

/// Forward little-endian bit writer.
#[derive(Default)]
struct Fwd(Vec<bool>);

impl Fwd {
    fn put(&mut self, v: u64, n: u32) {
        for k in 0..n {
            self.0.push((v >> k) & 1 == 1);
        }
    }
}

/// Writes an FSE table description (the inverse of `fse::read_counts`).
fn write_counts(counts: &[i16], log: u32) -> Vec<u8> {
    let mut w = Fwd::default();
    w.put((log - 5) as u64, 4);
    let mut remaining: i32 = (1 << log) + 1;
    let mut threshold: i32 = 1 << log;
    let mut nb = log + 1;
    let mut i = 0;
    while remaining > 1 {
        let c = counts[i] as i32;
        let value = c + 1;
        let max = 2 * threshold - 1 - remaining;
        if value < max {
            w.put(value as u64, nb - 1);
        } else if value < threshold {
            w.put(value as u64, nb);
        } else {
            w.put((value + max) as u64, nb);
        }
        remaining -= c.abs();
        i += 1;
        while remaining < threshold && remaining >= 1 {
            nb -= 1;
            threshold >>= 1;
        }
        if c == 0 && remaining > 1 {
            let mut z = 0;
            while i + z < counts.len() && counts[i + z] == 0 {
                z += 1;
            }
            i += z;
            while z >= 3 {
                w.put(3, 2);
                z -= 3;
            }
            w.put(z as u64, 2);
        }
    }
    assert_eq!(remaining, 1);
    pack(&w.0)
}

/// Decoder states that emit `syms` in order (searched backwards through the table's state ranges).
fn chain(t: &FseTable, syms: &[u8], last_needs_bits: bool) -> Vec<usize> {
    let n = syms.len();
    let mut st = vec![0usize; n];
    st[n - 1] = (0..t.entries.len())
        .find(|&j| t.entries[j].symbol == syms[n - 1] && (!last_needs_bits || t.entries[j].nb_bits > 0))
        .expect("symbol not in table");
    for i in (0..n - 1).rev() {
        let next = st[i + 1];
        st[i] = (0..t.entries.len())
            .find(|&j| {
                let e = t.entries[j];
                e.symbol == syms[i] && (e.base as usize) <= next && next < e.base as usize + (1 << e.nb_bits)
            })
            .expect("no state reaches the next one");
    }
    st
}

fn update_field(t: &FseTable, from: usize, to: usize) -> (u64, u32) {
    let e = t.entries[from];
    ((to - e.base as usize) as u64, e.nb_bits as u32)
}

// ----- literals

/// Huffman codes per symbol: (code, length), from the decoder's own table.
fn huf_codes(t: &HufTable) -> Vec<Option<(u64, u32)>> {
    let mut codes = vec![None; 256];
    for (idx, &(sym, nb)) in t.entries.iter().enumerate() {
        if codes[sym as usize].is_none() {
            codes[sym as usize] = Some(((idx >> (t.max_bits - nb as u32)) as u64, nb as u32));
        }
    }
    codes
}

fn huf_stream(t: &HufTable, data: &[u8]) -> Vec<u8> {
    let codes = huf_codes(t);
    let mut b = Back::default();
    for &s in data {
        let (c, n) = codes[s as usize].expect("literal without a code");
        b.put(c, n);
    }
    b.finish()
}

/// Direct (4-bit) tree description of the transmitted weights.
fn huf_tree_direct(weights: &[u8]) -> Vec<u8> {
    let mut out = vec![127 + weights.len() as u8];
    for pair in weights.chunks(2) {
        out.push((pair[0] << 4) | pair.get(1).copied().unwrap_or(0));
    }
    out
}

/// FSE-compressed tree description of the transmitted weights, with the given weight distribution.
fn huf_tree_fse(weights: &[u8], counts: &[i16], log: u32) -> Vec<u8> {
    let t = FseTable::build(counts, log).unwrap();
    // symbols alternate between state 1 (even positions) and state 2 (odd positions); the last update of the
    // second-to-last symbol's state overflows, and the other state then yields the last symbol.
    let k = weights.len();
    assert!(k >= 2);
    let even: Vec<u8> = weights.iter().step_by(2).copied().collect();
    let odd: Vec<u8> = weights.iter().skip(1).step_by(2).copied().collect();
    let x_is_even = (k - 2) % 2 == 0;
    let ce = chain(&t, &even, x_is_even);
    let co = chain(&t, &odd, !x_is_even);
    let mut b = Back::default();
    b.put(ce[0] as u64, log);
    b.put(co[0] as u64, log);
    for j in 0..k - 2 {
        let (c, m) = if j % 2 == 0 { (&ce, j / 2) } else { (&co, j / 2) };
        let (v, n) = update_field(&t, c[m], c[m + 1]);
        b.put(v, n);
    }
    let mut out = vec![0u8];
    out.extend(write_counts(counts, log));
    out.extend(b.finish());
    out[0] = (out.len() - 1) as u8;
    assert!(out[0] < 128);
    out
}

fn lit_raw(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut out = if n < 32 {
        vec![(n << 3) as u8]
    } else if n < 4096 {
        vec![(1 << 2 | (n << 4)) as u8, (n >> 4) as u8]
    } else {
        vec![(3 << 2 | (n << 4)) as u8, (n >> 4) as u8, (n >> 12) as u8]
    };
    out.extend_from_slice(data);
    out
}

fn lit_rle(b: u8, n: usize) -> Vec<u8> {
    let mut out = if n < 32 {
        vec![(1 | (n << 3)) as u8]
    } else if n < 4096 {
        vec![(1 | 1 << 2 | (n << 4)) as u8, (n >> 4) as u8]
    } else {
        vec![(1 | 3 << 2 | (n << 4)) as u8, (n >> 4) as u8, (n >> 12) as u8]
    };
    out.push(b);
    out
}

/// Huffman literals: `tree` = Some(description) for type 2, None for treeless (type 3).
fn lit_huf(t: &HufTable, tree: Option<&[u8]>, data: &[u8], four: bool, big_header: bool) -> Vec<u8> {
    let mut body = tree.map(|t| t.to_vec()).unwrap_or_default();
    if four {
        let seg = data.len().div_ceil(4);
        let parts = [&data[..seg], &data[seg..2 * seg], &data[2 * seg..3 * seg], &data[3 * seg..]];
        let streams: Vec<Vec<u8>> = parts.iter().map(|p| huf_stream(t, p)).collect();
        for s in &streams[..3] {
            body.extend_from_slice(&(s.len() as u16).to_le_bytes());
        }
        for s in &streams {
            body.extend_from_slice(s);
        }
    } else {
        body.extend(huf_stream(t, data));
    }
    let ty = if tree.is_some() { 2u64 } else { 3 };
    let (regen, csize) = (data.len() as u64, body.len() as u64);
    let mut out = if !four {
        assert!(regen < 1024 && csize < 1024);
        let v = ty | regen << 4 | csize << 14;
        v.to_le_bytes()[..3].to_vec()
    } else if !big_header && regen < 1024 && csize < 1024 {
        let v = ty | 1 << 2 | regen << 4 | csize << 14;
        v.to_le_bytes()[..3].to_vec()
    } else if regen < 16384 && csize < 16384 {
        let v = ty | 2 << 2 | regen << 4 | csize << 18;
        v.to_le_bytes()[..4].to_vec()
    } else {
        let v = ty | 3 << 2 | regen << 4 | csize << 22;
        v.to_le_bytes()[..5].to_vec()
    };
    out.extend(body);
    out
}

// ----- sequences

#[derive(Clone, Copy, Debug)]
struct Seq {
    ll: u32,
    ml: u32,
    /// The coded offset value (> 3 = offset + 3; 1..=3 = repeat codes).
    of: u64,
}

fn code_of(codes: &[(u32, u32)], v: u32) -> (u8, u64, u32) {
    let c = codes.iter().rposition(|&(base, _)| base <= v).unwrap();
    let (base, bits) = codes[c];
    assert!(((v - base) as u64) < (1u64 << bits) || (bits == 0 && v == base));
    (c as u8, (v - base) as u64, bits)
}

fn of_code(v: u64) -> (u8, u64, u32) {
    let c = 63 - v.leading_zeros();
    (c as u8, v - (1 << c), c)
}

enum Mode<'a> {
    Predefined,
    Rle(u8),
    Fse(&'a [i16], u32),
    Repeat(&'a FseTable),
}

fn mode_parts(m: &Mode, default: &[i16], default_log: u32) -> (u8, Vec<u8>, FseTable) {
    match m {
        Mode::Predefined => (0, vec![], FseTable::build(default, default_log).unwrap()),
        Mode::Rle(s) => (1, vec![*s], FseTable::rle(*s)),
        Mode::Fse(c, l) => (2, write_counts(c, *l), FseTable::build(c, *l).unwrap()),
        Mode::Repeat(t) => (3, vec![], (*t).clone()),
    }
}

/// Encodes a sequences section; returns it with the three tables used (LL, OF, ML).
fn seq_section(seqs: &[Seq], ll: Mode, of: Mode, ml: Mode) -> (Vec<u8>, [FseTable; 3]) {
    let n = seqs.len();
    let mut out = if n < 128 {
        vec![n as u8]
    } else if n < 0x7F00 {
        vec![(n >> 8) as u8 + 128, n as u8]
    } else {
        vec![255, (n - 0x7F00) as u8, ((n - 0x7F00) >> 8) as u8]
    };
    let (llm, llh, llt) = mode_parts(&ll, &LL_DEFAULT, 6);
    let (ofm, ofh, oft) = mode_parts(&of, &OF_DEFAULT, 5);
    let (mlm, mlh, mlt) = mode_parts(&ml, &ML_DEFAULT, 6);
    if n == 0 {
        return (out, [llt, oft, mlt]);
    }
    out.push(llm << 6 | ofm << 4 | mlm << 2);
    out.extend(llh);
    out.extend(ofh);
    out.extend(mlh);
    let llc: Vec<_> = seqs.iter().map(|s| code_of(&LL_CODES, s.ll)).collect();
    let mlc: Vec<_> = seqs.iter().map(|s| code_of(&ML_CODES, s.ml)).collect();
    let ofc: Vec<_> = seqs.iter().map(|s| of_code(s.of)).collect();
    let lls = chain(&llt, &llc.iter().map(|c| c.0).collect::<Vec<_>>(), false);
    let mls = chain(&mlt, &mlc.iter().map(|c| c.0).collect::<Vec<_>>(), false);
    let ofs = chain(&oft, &ofc.iter().map(|c| c.0).collect::<Vec<_>>(), false);
    let mut b = Back::default();
    b.put(lls[0] as u64, llt.log);
    b.put(ofs[0] as u64, oft.log);
    b.put(mls[0] as u64, mlt.log);
    for i in 0..n {
        b.put(ofc[i].1, ofc[i].2);
        b.put(mlc[i].1, mlc[i].2);
        b.put(llc[i].1, llc[i].2);
        if i + 1 < n {
            let (v, k) = update_field(&llt, lls[i], lls[i + 1]);
            b.put(v, k);
            let (v, k) = update_field(&mlt, mls[i], mls[i + 1]);
            b.put(v, k);
            let (v, k) = update_field(&oft, ofs[i], ofs[i + 1]);
            b.put(v, k);
        }
    }
    out.extend(b.finish());
    (out, [llt, oft, mlt])
}

// ----- blocks and frames

fn block(ty: u32, size: usize, body: &[u8], last: bool) -> Vec<u8> {
    let h = last as u32 | ty << 1 | (size as u32) << 3;
    let mut out = h.to_le_bytes()[..3].to_vec();
    out.extend_from_slice(body);
    out
}

fn raw_block(data: &[u8], last: bool) -> Vec<u8> {
    block(0, data.len(), data, last)
}

fn rle_block(b: u8, n: usize, last: bool) -> Vec<u8> {
    block(1, n, &[b], last)
}

fn comp_block(lits: &[u8], seqs: &[u8], last: bool) -> Vec<u8> {
    let mut body = lits.to_vec();
    body.extend_from_slice(seqs);
    block(2, body.len(), &body, last)
}

/// A frame with a window descriptor (no content size) or single segment (with content size).
fn frame(content: &[u8], blocks: &[Vec<u8>], single: bool, checksum: bool) -> Vec<u8> {
    let mut out = MAGIC.to_le_bytes().to_vec();
    let n = content.len() as u64;
    if single {
        let (flag, bytes) = if n < 256 {
            (0u8, vec![n as u8])
        } else if n < 65536 + 256 {
            (1, ((n - 256) as u16).to_le_bytes().to_vec())
        } else {
            (2, (n as u32).to_le_bytes().to_vec())
        };
        out.push(flag << 6 | 0x20 | (checksum as u8) << 2);
        out.extend(bytes);
    } else {
        out.push((checksum as u8) << 2);
        out.push(10 << 3); // window 1 MiB
    }
    for b in blocks {
        out.extend_from_slice(b);
    }
    if checksum {
        out.extend_from_slice(&(xxh64(content, 0) as u32).to_le_bytes());
    }
    out
}

fn ok(input: &[u8]) -> Vec<u8> {
    decode(input, usize::MAX).unwrap_or_else(|e| panic!("decode failed: {}", e))
}

// ---------------------------------------------------------------------------------------------------------
// hash

#[test]
fn xxh64_vectors() {
    assert_eq!(xxh64(b"", 0), 0xEF46_DB37_51D8_E999);
    assert_eq!(xxh64(b"a", 0), 0xD24E_C4F1_A98C_6E5B);
    assert_eq!(xxh64(b"abc", 0), 0x44BC_2CF5_AD77_0999);
    assert_eq!(xxh64(b"Nobody inspects the spammish repetition", 0), 0xFBCE_A83C_8A37_8BF1);
}

// ---------------------------------------------------------------------------------------------------------
// frames and simple blocks

#[test]
fn empty_frame_with_checksum() {
    // what the reference tool emits for empty input
    let f = [0x28, 0xB5, 0x2F, 0xFD, 0x24, 0x00, 0x01, 0x00, 0x00, 0x99, 0xE9, 0xD8, 0x51];
    assert_eq!(ok(&f), b"");
    let (h, n) = read_frame_header(&f[4..]).unwrap();
    assert_eq!(n, 2);
    assert_eq!(h.content_size, Some(0));
    assert!(h.single_segment && h.has_checksum);
}

#[test]
fn raw_and_rle_blocks() {
    let mut want = b"hello ".to_vec();
    want.extend([b'z'; 300]);
    want.extend(b"world");
    let blocks = [raw_block(b"hello ", false), rle_block(b'z', 300, false), raw_block(b"world", true)];
    assert_eq!(ok(&frame(&want, &blocks, true, true)), want);
    assert_eq!(ok(&frame(&want, &blocks, false, false)), want);
}

#[test]
fn header_fields() {
    // window descriptor exponent 3 mantissa 5, 2-byte dictionary id, 4-byte content size
    let src = [0x80 | 0x02, 3 << 3 | 5, 0x34, 0x12, 0x10, 0x00, 0x01, 0x00];
    let (h, n) = read_frame_header(&src).unwrap();
    assert_eq!(n, 8);
    assert_eq!(h.window_size, (1 << 13) + (1 << 10) * 5);
    assert_eq!(h.dictionary_id, 0x1234);
    assert_eq!(h.content_size, Some(0x10010));
    // 2-byte content size has an offset of 256
    let (h, _) = read_frame_header(&[0x40 | 0x20, 0x00, 0x01]).unwrap();
    assert_eq!(h.content_size, Some(256 + 256));
    assert_eq!(h.window_size, 512);
    assert!(read_frame_header(&[0x08, 0]).is_err());
}

#[test]
fn concatenated_and_skippable_frames() {
    let a = frame(b"abc", &[raw_block(b"abc", true)], true, true);
    let b = frame(b"defg", &[raw_block(b"defg", true)], false, true);
    let mut input = a.clone();
    input.extend(0x184D_2A53u32.to_le_bytes());
    input.extend(5u32.to_le_bytes());
    input.extend(b"12345");
    input.extend(&b);
    input.extend(0x184D_2A50u32.to_le_bytes());
    input.extend(0u32.to_le_bytes());
    assert_eq!(ok(&input), b"abcdefg");
    // a skippable frame alone decodes to nothing
    assert_eq!(ok(&input[a.len()..a.len() + 13]), b"");
    // trailing garbage fails
    let mut bad = a.clone();
    bad.extend(b"xyz!");
    assert!(decode(&bad, usize::MAX).is_err());
    assert!(decode(&[], usize::MAX).is_err());
}

#[test]
fn errors_and_limits() {
    let want = b"0123456789".to_vec();
    let good = frame(&want, &[raw_block(&want, true)], true, true);
    assert_eq!(decode(&good, 10).unwrap(), want);
    assert!(decode(&good, 9).is_err());
    // the limit also applies without a declared content size
    let nofcs = frame(&want, &[raw_block(&want, false), rle_block(b'x', 1000, true)], false, false);
    assert!(decode(&nofcs, 500).is_err());
    assert_eq!(decode(&nofcs, 1010).unwrap().len(), 1010);
    // checksum mismatch
    let mut bad = good.clone();
    let n = bad.len();
    bad[n - 1] ^= 1;
    assert!(decode(&bad, usize::MAX).unwrap_err().contains("checksum"));
    // content size mismatch
    let mut bad = frame(&want, &[raw_block(&want, true)], true, false);
    bad[5] = 11;
    assert!(decode(&bad, usize::MAX).is_err());
    // truncation anywhere fails
    for cut in 0..good.len() {
        assert!(decode(&good[..cut], usize::MAX).is_err(), "cut {}", cut);
    }
    // reserved block type
    let bad = frame(b"", &[block(3, 0, &[], true)], true, false);
    assert!(decode(&bad, usize::MAX).is_err());
}

// ---------------------------------------------------------------------------------------------------------
// literals

#[test]
fn compressed_block_literals_only() {
    // raw literals of all three header sizes, RLE literals, no sequences
    for n in [0usize, 5, 31, 32, 4095, 4096, 70000] {
        let data: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
        let (seqs, _) = seq_section(&[], Mode::Predefined, Mode::Predefined, Mode::Predefined);
        let f = frame(&data, &[comp_block(&lit_raw(&data), &seqs, true)], true, true);
        assert_eq!(ok(&f), data, "raw {}", n);
        let rle = vec![b'q'; n];
        let f = frame(&rle, &[comp_block(&lit_rle(b'q', n), &seqs, true)], false, true);
        assert_eq!(ok(&f), rle, "rle {}", n);
    }
}

/// Complete weights for a skewed 6-symbol alphabet 'a'..'f' (others zero); transmitted = all but 'f'.
fn skewed_weights() -> (Vec<u8>, HufTable) {
    // weights a:4 b:3 c:2 d:1 e:1 f:(implied 2): 8+4+2+1+1 = 16, left 16 = 2^4 ... adjust: f implied
    let mut w = vec![0u8; b'f' as usize];
    w[b'a' as usize] = 4;
    w[b'b' as usize] = 3;
    w[b'c' as usize] = 2;
    w[b'd' as usize] = 1;
    w[b'e' as usize] = 1;
    let t = HufTable::from_weights(w.clone()).unwrap();
    (w, t)
}

#[test]
fn huffman_table_shape() {
    let (_, t) = skewed_weights();
    // sum 8+4+2+1+1 = 16 -> max_bits 5, left 16 -> f gets weight 5
    assert_eq!(t.max_bits, 5);
    let codes = huf_codes(&t);
    assert_eq!(codes[b'f' as usize].unwrap().1, 1);
    assert_eq!(codes[b'a' as usize].unwrap().1, 2);
    assert_eq!(codes[b'e' as usize].unwrap().1, 5);
    assert!(HufTable::from_weights(vec![0, 0]).is_err());
    assert!(HufTable::from_weights(vec![12]).is_err());
}

#[test]
fn huffman_literals_direct_tree() {
    let (w, t) = skewed_weights();
    let tree = huf_tree_direct(&w);
    let data: Vec<u8> = b"ffffaaabbcdeffafbfcfdfef".iter().cycle().take(500).copied().collect();
    let (seqs, _) = seq_section(&[], Mode::Predefined, Mode::Predefined, Mode::Predefined);
    for (four, big) in [(false, false), (true, false), (true, true)] {
        let lits = lit_huf(&t, Some(&tree), &data[..300], four, big);
        let f = frame(&data[..300], &[comp_block(&lits, &seqs, true)], true, true);
        assert_eq!(ok(&f), &data[..300], "four {} big {}", four, big);
    }
    // treeless reuse in the next block, with a single and with four streams
    let b1 = comp_block(&lit_huf(&t, Some(&tree), &data[..100], false, false), &seqs, false);
    let b2 = comp_block(&lit_huf(&t, None, &data[100..300], true, false), &seqs, false);
    let b3 = comp_block(&lit_huf(&t, None, &data[300..], false, false), &seqs, true);
    assert_eq!(ok(&frame(&data, &[b1, b2.clone(), b3], false, true)), data);
    // treeless without a previous table fails
    assert!(decode(&frame(&data[100..300], &[b2], false, false), usize::MAX).is_err());
}

#[test]
fn huffman_literals_fse_tree() {
    let (w, t) = skewed_weights();
    // weight distribution over symbols 0..=4 (accuracy log 5): zeros dominate the transmitted weights
    let counts = [20i16, 4, 3, 3, 2];
    let tree = huf_tree_fse(&w, &counts, 5);
    let data: Vec<u8> = b"abcdefedcbaffff".iter().cycle().take(777).copied().collect();
    let lits = lit_huf(&t, Some(&tree), &data, true, false);
    let (seqs, _) = seq_section(&[], Mode::Predefined, Mode::Predefined, Mode::Predefined);
    assert_eq!(ok(&frame(&data, &[comp_block(&lits, &seqs, true)], true, true)), data);
    // a "less than one" count in the weight distribution
    let counts = [24i16, 3, 2, 2, -1];
    let tree = huf_tree_fse(&w, &counts, 5);
    let lits = lit_huf(&t, Some(&tree), &data, false, false);
    assert_eq!(ok(&frame(&data, &[comp_block(&lits, &seqs, true)], true, true)), data);
}

// ---------------------------------------------------------------------------------------------------------
// sequences

#[test]
fn predefined_sequences() {
    // "abc" then a 9-byte match at offset 3, then literal tail "XY"
    let lits = b"abcXY";
    let seqs = [Seq { ll: 3, ml: 9, of: 3 + 3 }];
    let (s, _) = seq_section(&seqs, Mode::Predefined, Mode::Predefined, Mode::Predefined);
    let want = b"abcabcabcabcXY";
    assert_eq!(ok(&frame(want, &[comp_block(&lit_raw(lits), &s, true)], true, true)), want);
    // overlapping match with offset 1 and long lengths needing extra bits
    let seqs = [Seq { ll: 1, ml: 5000, of: 1 + 3 }, Seq { ll: 20, ml: 70, of: 5021 + 3 }];
    let lits: Vec<u8> = (0..21u8).collect();
    let mut want = vec![0u8; 5001];
    want.extend(1..21u8);
    let from = want.len() - 5021;
    for k in 0..70 {
        want.push(want[from + k]);
    }
    let (s, _) = seq_section(&seqs, Mode::Predefined, Mode::Predefined, Mode::Predefined);
    assert_eq!(ok(&frame(&want, &[comp_block(&lit_raw(&lits), &s, true)], false, true)), want);
}

#[test]
fn rle_and_fse_tables_and_repeat_mode() {
    // block 1: LL RLE (code 4), OF FSE, ML FSE; block 2: all repeat; block 3: predefined again
    let of_counts: Vec<i16> = {
        let mut c = vec![0i16; 6];
        c[2] = 10; // offsets 4..=7 coded 1..4 (+3)
        c[3] = 12;
        c[5] = -1;
        c[4] = 9;
        c
    };
    let ml_counts: Vec<i16> = {
        let mut c = vec![0i16; 40];
        c[0] = 30;
        c[1] = 1;
        c[5] = -1;
        c[39] = 32;
        c
    };
    let mk = |n: usize, seed: u8| -> Vec<u8> { (0..n).map(|i| seed.wrapping_add(i as u8 * 13)).collect() };
    let seqs1 = [
        Seq { ll: 4, ml: 3, of: 4 + 3 },
        Seq { ll: 4, ml: 60, of: 7 + 3 },
        Seq { ll: 4, ml: 8, of: 12 + 3 },
        Seq { ll: 4, ml: 4, of: 33 + 3 },
    ];
    let (s1, tables) = seq_section(&seqs1, Mode::Rle(4), Mode::Fse(&of_counts, 5), Mode::Fse(&ml_counts, 6));
    let seqs2 = [Seq { ll: 4, ml: 63, of: 5 + 3 }, Seq { ll: 4, ml: 3, of: 6 + 3 }];
    let (s2, _) = seq_section(&seqs2, Mode::Repeat(&tables[0]), Mode::Repeat(&tables[1]), Mode::Repeat(&tables[2]));
    let seqs3 = [Seq { ll: 0, ml: 10, of: 2 }];
    let (s3, _) = seq_section(&seqs3, Mode::Predefined, Mode::Predefined, Mode::Predefined);
    let l1 = mk(17, 1);
    let l2 = mk(9, 50);
    let mut want = Vec::new();
    let mut reps = [1usize, 4, 8];
    run(&mut want, &l1, &seqs1, &mut reps);
    run(&mut want, &l2, &seqs2, &mut reps);
    run(&mut want, &[], &seqs3, &mut reps);
    let blocks = [comp_block(&lit_raw(&l1), &s1, false), comp_block(&lit_raw(&l2), &s2, false), comp_block(&lit_raw(&[]), &s3, true)];
    assert_eq!(ok(&frame(&want, &blocks, false, true)), want);
    // repeat mode in a frame's first block fails
    let f = frame(&want, &[comp_block(&lit_raw(&l2), &s2, true)], false, false);
    assert!(decode(&f, usize::MAX).unwrap_err().contains("repeat"));
}

/// Reference executor for test expectations, resolving repeat codes straight from RFC 8878 3.1.2.5's
/// table (the decoder resolves them by index arithmetic).
fn run(out: &mut Vec<u8>, lits: &[u8], seqs: &[Seq], reps: &mut [usize; 3]) {
    let mut lp = 0;
    for s in seqs {
        out.extend_from_slice(&lits[lp..lp + s.ll as usize]);
        lp += s.ll as usize;
        let o = match (s.of, s.ll == 0) {
            (v, _) if v > 3 => {
                let o = v as usize - 3;
                *reps = [o, reps[0], reps[1]];
                o
            }
            (1, false) => reps[0],
            (2, false) => {
                *reps = [reps[1], reps[0], reps[2]];
                reps[0]
            }
            (3, false) => {
                *reps = [reps[2], reps[0], reps[1]];
                reps[0]
            }
            (1, true) => {
                *reps = [reps[1], reps[0], reps[2]];
                reps[0]
            }
            (2, true) => {
                *reps = [reps[2], reps[0], reps[1]];
                reps[0]
            }
            _ => {
                *reps = [reps[0] - 1, reps[0], reps[1]];
                reps[0]
            }
        };
        for _ in 0..s.ml {
            out.push(out[out.len() - o]);
        }
    }
    out.extend_from_slice(&lits[lp..]);
}

#[test]
fn repeat_offsets() {
    // establish offsets 10, 20, 30 then exercise every repeat code with and without literals
    let mut seqs = vec![
        Seq { ll: 40, ml: 4, of: 30 + 3 },
        Seq { ll: 1, ml: 4, of: 20 + 3 },
        Seq { ll: 1, ml: 4, of: 10 + 3 },
    ];
    for of in 1..=3u64 {
        for ll in [0u32, 2] {
            seqs.push(Seq { ll, ml: 5, of });
        }
    }
    seqs.push(Seq { ll: 0, ml: 3, of: 3 }); // rep0 - 1
    seqs.push(Seq { ll: 0, ml: 3, of: 1 });
    seqs.push(Seq { ll: 3, ml: 3, of: 1 });
    let total_ll: u32 = seqs.iter().map(|s| s.ll).sum();
    let lits: Vec<u8> = (0..total_ll + 5).map(|i| (i * 37 % 256) as u8).collect();
    let mut want = Vec::new();
    let mut reps = [1, 4, 8];
    run(&mut want, &lits, &seqs, &mut reps);
    let (s, _) = seq_section(&seqs, Mode::Predefined, Mode::Predefined, Mode::Predefined);
    assert_eq!(ok(&frame(&want, &[comp_block(&lit_raw(&lits), &s, true)], true, true)), want);
    // repeat offsets carry across blocks: second block starts with code 1 (rep0) and literals
    let (s2, _) = seq_section(&[Seq { ll: 1, ml: 6, of: 1 }], Mode::Predefined, Mode::Predefined, Mode::Predefined);
    let mut want2 = want.clone();
    run(&mut want2, b"!", &[Seq { ll: 1, ml: 6, of: 1 }], &mut reps);
    let blocks = [comp_block(&lit_raw(&lits), &s, false), comp_block(&lit_raw(b"!"), &s2, true)];
    assert_eq!(ok(&frame(&want2, &blocks, false, true)), want2);
}

#[test]
fn bad_offsets() {
    let (s, _) = seq_section(&[Seq { ll: 2, ml: 3, of: 5 + 3 }], Mode::Predefined, Mode::Predefined, Mode::Predefined);
    let f = frame(b"", &[comp_block(&lit_raw(b"ab"), &s, true)], false, false);
    assert!(decode(&f, usize::MAX).unwrap_err().contains("offset"));
    // a match reaching past the window
    let mut f = MAGIC.to_le_bytes().to_vec();
    f.push(0);
    f.push(0); // window 1 KiB
    f.extend(rle_block(b'a', 2000, false));
    let (s, _) = seq_section(&[Seq { ll: 0, ml: 3, of: 1500 + 3 }], Mode::Predefined, Mode::Predefined, Mode::Predefined);
    f.extend(comp_block(&lit_raw(b""), &s, true));
    assert!(decode(&f, usize::MAX).unwrap_err().contains("window"));
}

// ---------------------------------------------------------------------------------------------------------
// randomized round trip through the test encoder

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Random normalized counts covering `used` symbols with accuracy `log`.
fn random_counts(rng: &mut Rng, used: &[u8], log: u32) -> Vec<i16> {
    let size = 1i32 << log;
    let maxs = *used.iter().max().unwrap() as usize;
    let mut c = vec![0i16; maxs + 1];
    let mut left = size;
    for (i, &s) in used.iter().enumerate() {
        if i > 0 && rng.below(4) == 0 {
            c[s as usize] = -1;
        } else {
            c[s as usize] = 1;
        }
        left -= 1;
    }
    while left > 0 {
        let s = used[rng.below(used.len() as u64) as usize] as usize;
        if c[s] == -1 {
            continue;
        }
        let add = 1 + rng.below(left as u64) as i32 / 2;
        c[s] += add as i16;
        left -= add;
    }
    c
}

fn distinct(v: impl Iterator<Item = u8>) -> Vec<u8> {
    let mut seen = [false; 256];
    let mut out = Vec::new();
    for s in v {
        if !seen[s as usize] {
            seen[s as usize] = true;
            out.push(s);
        }
    }
    out
}

/// Complete prefix code (weights 1 and 2) for the distinct symbols; returns (transmitted weights, table).
fn random_huffman(rng: &mut Rng, syms: &[u8]) -> (Vec<u8>, HufTable) {
    let n = syms.len();
    let l = (n as f64).log2().ceil() as u32;
    let short = (1usize << l) - n;
    let mut order = syms.to_vec();
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i as u64 + 1) as usize);
    }
    let last = *syms.iter().max().unwrap() as usize;
    let mut w = vec![0u8; last + 1];
    for (i, &s) in order.iter().enumerate() {
        w[s as usize] = if i < short { 2 } else { 1 };
    }
    w.pop();
    let t = HufTable::from_weights(w.clone()).unwrap();
    (w, t)
}

fn table_has(t: &FseTable, syms: &[u8]) -> bool {
    syms.iter().all(|&s| t.entries.iter().any(|e| e.symbol == s))
}

#[test]
fn random_round_trip() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..300 {
        let mut want = Vec::new();
        let mut blocks = Vec::new();
        let mut reps = [1usize, 4, 8];
        let mut huf: Option<HufTable> = None;
        let mut prev: Option<[FseTable; 3]> = None;
        let nblocks = 1 + rng.below(4) as usize;
        let alphabet = 2 + rng.below(60) as u8;
        for bi in 0..nblocks {
            let last = bi + 1 == nblocks;
            match rng.below(6) {
                0 => {
                    let d: Vec<u8> = (0..rng.below(50)).map(|_| rng.below(256) as u8).collect();
                    want.extend(&d);
                    blocks.push(raw_block(&d, last));
                    continue;
                }
                1 => {
                    let n = rng.below(300) as usize;
                    let b = rng.below(256) as u8;
                    want.extend(std::iter::repeat_n(b, n));
                    blocks.push(rle_block(b, n, last));
                    continue;
                }
                _ => {}
            }
            // sequences against the current output
            let cap = if rng.below(5) == 0 { 400 } else { 20 };
            let nseq = rng.below(cap) as usize;
            let mut seqs = Vec::new();
            let mut lits = Vec::new();
            let block_start = want.len();
            let mut produced = block_start;
            let mut r = reps;
            for _ in 0..nseq {
                let ll = match rng.below(5) {
                    0 => 0,
                    1 => rng.below(300) as u32,
                    _ => rng.below(12) as u32,
                };
                for _ in 0..ll {
                    lits.push(b'0' + rng.below(alphabet as u64) as u8);
                }
                produced += ll as usize;
                if produced == 0 {
                    continue;
                }
                let ml = match rng.below(4) {
                    0 => 3 + rng.below(1000) as u32,
                    _ => 3 + rng.below(20) as u32,
                };
                // pick a repeat code or a fresh offset, keep it within what exists
                let code = rng.below(5);
                let ll0 = ll == 0;
                let cand = match (code, ll0) {
                    (0, false) => Some((1, r[0])),
                    (1, false) => Some((2, r[1])),
                    (2, false) => Some((3, r[2])),
                    (0, true) => Some((1, r[1])),
                    (1, true) => Some((2, r[2])),
                    (2, true) => Some((3, r[0].wrapping_sub(1))),
                    _ => None,
                };
                let of = match cand {
                    Some((c, o)) if o >= 1 && o <= produced => c as u64,
                    _ => 1 + rng.below(produced.min(5000) as u64) + 3,
                };
                // keep the encoder-side rep state in step (literals length matters for repeat codes)
                apply_reps(&mut r, of, ll0);
                seqs.push(Seq { ll, ml, of });
                produced += ml as usize;
                if produced - block_start > 100_000 {
                    break;
                }
            }
            let tail = rng.below(10) as usize;
            for _ in 0..tail {
                lits.push(b'0' + rng.below(alphabet as u64) as u8);
            }
            run(&mut want, &lits, &seqs, &mut reps);
            assert_eq!(reps, r);

            // literals section
            let ds = distinct(lits.iter().copied());
            let lit_bytes = if ds.len() >= 2 && rng.below(3) != 0 {
                let reuse = huf.as_ref().is_some_and(|t| {
                    let codes = huf_codes(t);
                    ds.iter().all(|&s| codes[s as usize].is_some_and(|c| c.1 > 0))
                }) && rng.below(2) == 0;
                let four = lits.len() >= 16 && (lits.len() >= 1024 || rng.below(2) == 0);
                if lits.len() >= 1024 && !four {
                    lit_raw(&lits)
                } else if reuse {
                    lit_huf(huf.as_ref().unwrap(), None, &lits, four, false)
                } else {
                    let (w, t) = random_huffman(&mut rng, &ds);
                    let tree = if w.len() <= 128 && rng.below(2) == 0 { huf_tree_direct(&w) } else { fse_tree(&mut rng, &w) };
                    let b = lit_huf(&t, Some(&tree), &lits, four, false);
                    huf = Some(t);
                    b
                }
            } else if ds.len() == 1 && rng.below(2) == 0 {
                lit_rle(ds[0], lits.len())
            } else {
                lit_raw(&lits)
            };

            // sequence tables
            let codes = [
                distinct(seqs.iter().map(|s| code_of(&LL_CODES, s.ll).0)),
                distinct(seqs.iter().map(|s| of_code(s.of).0)),
                distinct(seqs.iter().map(|s| code_of(&ML_CODES, s.ml).0)),
            ];
            let defaults: [(&[i16], u32, u32); 3] = [(&LL_DEFAULT, 6, 9), (&OF_DEFAULT, 5, 8), (&ML_DEFAULT, 6, 9)];
            let mut counts: [Vec<i16>; 3] = Default::default();
            let mut modes: Vec<Mode> = Vec::new();
            let mut picks = [0u8; 3];
            for k in 0..3 {
                let c = &codes[k];
                let pick = if c.is_empty() {
                    0
                } else {
                    loop {
                        let p = rng.below(4) as u8;
                        let fits = match p {
                            0 => c.iter().all(|&s| (s as usize) < defaults[k].0.len()),
                            1 => c.len() == 1,
                            2 => true,
                            _ => prev.as_ref().is_some_and(|t| table_has(&t[k], c)),
                        };
                        if fits {
                            break p;
                        }
                    }
                };
                picks[k] = pick;
                if pick == 2 {
                    let min_log = (c.len() as f64).log2().ceil().max(5.0) as u32;
                    let log = min_log + rng.below((defaults[k].2 - min_log + 1) as u64) as u32;
                    counts[k] = random_counts(&mut rng, c, log);
                }
            }
            for k in 0..3 {
                modes.push(match picks[k] {
                    0 => Mode::Predefined,
                    1 => Mode::Rle(codes[k][0]),
                    2 => Mode::Fse(&counts[k], (counts[k].iter().map(|c| c.unsigned_abs() as u32).sum::<u32>()).trailing_zeros()),
                    _ => Mode::Repeat(&prev.as_ref().unwrap()[k]),
                });
            }
            let ml_mode = modes.pop().unwrap();
            let of_mode = modes.pop().unwrap();
            let ll_mode = modes.pop().unwrap();
            let (s, tables) = seq_section(&seqs, ll_mode, of_mode, ml_mode);
            if !seqs.is_empty() {
                prev = Some(tables);
            }
            blocks.push(comp_block(&lit_bytes, &s, last));
        }
        let single = rng.below(2) == 0;
        let f = frame(&want, &blocks, single, rng.below(2) == 0);
        match decode(&f, usize::MAX) {
            Ok(got) => assert!(got == want, "case {}: output differs", case),
            Err(e) => panic!("case {}: {}", case, e),
        }
    }
}

fn apply_reps(r: &mut [usize; 3], of: u64, ll0: bool) {
    *r = match (of, ll0) {
        (v, _) if v > 3 => [v as usize - 3, r[0], r[1]],
        (1, false) => *r,
        (2, false) | (1, true) => [r[1], r[0], r[2]],
        (3, false) | (2, true) => [r[2], r[0], r[1]],
        _ => [r[0] - 1, r[0], r[1]],
    };
}

/// FSE tree description with a random weight distribution covering the weights used.
fn fse_tree(rng: &mut Rng, w: &[u8]) -> Vec<u8> {
    let used = distinct(w.iter().copied());
    if used.len() < 2 && w.len() < 2 {
        return huf_tree_direct(w);
    }
    loop {
        let log = 5 + rng.below(2) as u32;
        let counts = random_counts(rng, &used, log);
        let log = counts.iter().map(|c| c.unsigned_abs() as u32).sum::<u32>().trailing_zeros();
        // the second-to-last weight's state must read bits; retry distributions until it can
        let t = FseTable::build(&counts, log).unwrap();
        let x = w[w.len() - 2];
        if t.entries.iter().any(|e| e.symbol == x && e.nb_bits > 0) {
            let tree = huf_tree_fse(w, &counts, log);
            if tree[0] < 128 {
                return tree;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// fixtures from the reference tool, when installed on this machine

#[test]
fn against_zstd_tool() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if Command::new("zstd").arg("--version").output().is_err() {
        eprintln!("zstd tool not found; skipping");
        return;
    }
    let mut rng = Rng(12345);
    let mut inputs: Vec<Vec<u8>> = vec![Vec::new(), b"a".to_vec(), vec![7u8; 1 << 20]];
    // text-like data with repeats
    let words = ["alpha ", "beta ", "gamma ", "delta\n", "epsilon ", "zeta, ", "eta. ", "theta "];
    let mut text = Vec::new();
    while text.len() < 600_000 {
        text.extend(words[rng.below(8) as usize].as_bytes());
    }
    inputs.push(text);
    // random bytes (raw blocks), then skewed bytes (Huffman literals)
    inputs.push((0..300_000).map(|_| rng.next() as u8).collect());
    inputs.push((0..400_000).map(|_| (rng.below(16) * rng.below(16)) as u8).collect());
    // structured binary
    let mut bin = Vec::new();
    for i in 0..200_000u32 {
        bin.extend((i / 3).to_le_bytes());
    }
    inputs.push(bin);
    let opts: &[&[&str]] = &[
        &["-1"],
        &["-3", "--no-check"],
        &["-9"],
        &["-19"],
        &["--ultra", "-22"],
        &["--fast=5"],
        &["-3", "--long=27"],
        &["-6", "--no-check", "--content-size"],
        &["-5", "-B4096"],
    ];
    for (i, input) in inputs.iter().enumerate() {
        for o in opts {
            let mut child = Command::new("zstd")
                .args(*o)
                .args(["-c", "-q"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let data = input.clone();
            let writer = std::thread::spawn(move || stdin.write_all(&data).unwrap());
            let out = child.wait_with_output().unwrap();
            writer.join().unwrap();
            assert!(out.status.success());
            if i == 3 {
                // the text input must really exercise compressed blocks
                assert!(out.stdout.len() * 3 < input.len(), "opts {:?}: {} bytes", o, out.stdout.len());
            }
            let got = decode(&out.stdout, usize::MAX).unwrap_or_else(|e| panic!("input {} opts {:?}: {}", i, o, e));
            assert!(got == *input, "input {} opts {:?}: output differs", i, o);
        }
    }
    // two frames from two runs, concatenated
    let mut cat = Vec::new();
    for part in [&inputs[3][..1000], &inputs[5][..5000]] {
        let mut child = Command::new("zstd").args(["-c", "-q"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(part).unwrap();
        cat.extend(child.wait_with_output().unwrap().stdout);
    }
    let mut want = inputs[3][..1000].to_vec();
    want.extend(&inputs[5][..5000]);
    assert_eq!(decode(&cat, usize::MAX).unwrap(), want);
}
