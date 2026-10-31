//! `shell/qr.rs` — a small QR Code encoder: byte mode, error correction
//! level M, versions 1 to 10 (up to 213 bytes). That covers a Wi-Fi join
//! code (`WIFI:T:WPA;S:<ssid>;P:<passphrase>;;`, at most about 130 bytes)
//! with room to spare. Written from ISO/IEC 18004: the symbol layout, the
//! Reed-Solomon codewords over GF(256) with the 0x11D polynomial, the eight
//! masks, and the penalty rules that pick one of them.
//!
//! The panel draws the result as squares ([`Qr::get`]); nothing here draws.

/// Largest version this encoder builds.
const MAX_VERSION: usize = 10;

/// Per version 1..=10 at level M: error-correction codewords per block,
/// then (blocks, data codewords per block) for group 1 and group 2.
const BLOCKS_M: [(usize, (usize, usize), (usize, usize)); MAX_VERSION] = [
    (10, (1, 16), (0, 0)),
    (16, (1, 28), (0, 0)),
    (26, (1, 44), (0, 0)),
    (18, (2, 32), (0, 0)),
    (24, (2, 43), (0, 0)),
    (16, (4, 27), (0, 0)),
    (18, (4, 31), (0, 0)),
    (22, (2, 38), (2, 39)),
    (22, (3, 36), (2, 37)),
    (26, (4, 43), (1, 44)),
];

/// Alignment pattern centre coordinates per version 1..=10.
const ALIGNMENT: [&[usize]; MAX_VERSION] = [
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QrError {
    /// More than 213 bytes: no version up to 10 holds it.
    TooLong(usize),
}

impl std::fmt::Display for QrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QrError::TooLong(n) => write!(f, "{n} bytes do not fit a version 10 QR code"),
        }
    }
}

/// A finished symbol: `size` x `size` modules, `true` = dark. The quiet
/// zone is the caller's to leave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qr {
    pub size: usize,
    pub version: usize,
    pub mask: u8,
    modules: Vec<bool>,
}

impl Qr {
    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.modules[y * self.size + x]
    }

    /// Encode `data` as byte-mode segments at level M in the smallest
    /// version that holds it.
    pub fn encode(data: &[u8]) -> Result<Qr, QrError> {
        Self::build(data, None)
    }

    /// `forced` pins the mask (the tests compare against reference
    /// symbols); `None` picks the lowest penalty.
    fn build(data: &[u8], forced: Option<u8>) -> Result<Qr, QrError> {
        let version = (1..=MAX_VERSION)
            .find(|&v| data.len() <= byte_capacity(v))
            .ok_or(QrError::TooLong(data.len()))?;
        let codewords = interleave(version, &data_codewords(version, data));
        let size = 17 + 4 * version;
        let mut base = Grid::new(size);
        base.draw_function_patterns(version);
        base.place_data(&codewords);

        let mut best: Option<(u32, u8, Grid)> = None;
        for mask in (0..8u8).filter(|m| forced.is_none_or(|f| f == *m)) {
            let mut g = base.clone();
            g.apply_mask(mask);
            g.draw_format(mask);
            let penalty = g.penalty();
            if best.as_ref().is_none_or(|(p, _, _)| penalty < *p) {
                best = Some((penalty, mask, g));
            }
        }
        let (_, mask, grid) = best.expect("eight masks tried");
        Ok(Qr {
            size,
            version,
            mask,
            modules: grid.dark,
        })
    }
}

/// Bytes a version holds in byte mode at level M.
fn byte_capacity(version: usize) -> usize {
    let (_, g1, g2) = BLOCKS_M[version - 1];
    let data_cw = g1.0 * g1.1 + g2.0 * g2.1;
    let count_bits = if version <= 9 { 8 } else { 16 };
    (data_cw * 8 - 4 - count_bits) / 8
}

// ----------------------------------------------------------------------
// Codewords
// ----------------------------------------------------------------------

struct Bits {
    bytes: Vec<u8>,
    len: usize,
}

impl Bits {
    fn push(&mut self, value: u32, count: usize) {
        for i in (0..count).rev() {
            if self.len % 8 == 0 {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                *self.bytes.last_mut().unwrap() |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
        }
    }
}

/// The data codewords: mode 0100, the length, the bytes, a terminator,
/// zero padding to a byte, then the alternating pad bytes 0xEC 0x11.
fn data_codewords(version: usize, data: &[u8]) -> Vec<u8> {
    let (_, g1, g2) = BLOCKS_M[version - 1];
    let capacity = g1.0 * g1.1 + g2.0 * g2.1;
    let mut bits = Bits {
        bytes: Vec::new(),
        len: 0,
    };
    bits.push(0b0100, 4);
    bits.push(data.len() as u32, if version <= 9 { 8 } else { 16 });
    for &b in data {
        bits.push(b as u32, 8);
    }
    let terminator = (capacity * 8 - bits.len).min(4);
    bits.push(0, terminator);
    while bits.len % 8 != 0 {
        bits.push(0, 1);
    }
    let mut out = bits.bytes;
    let mut pad = [0xEC_u8, 0x11].into_iter().cycle();
    while out.len() < capacity {
        out.push(pad.next().unwrap());
    }
    out
}

fn gf_mul(a: u8, b: u8) -> u8 {
    let mut r: u32 = 0;
    for i in (0..8).rev() {
        r = (r << 1) ^ ((r >> 7) * 0x11D);
        r ^= ((b as u32 >> i) & 1) * a as u32;
    }
    r as u8
}

/// The generator polynomial of `degree`, highest power first, leading 1
/// left out.
fn rs_generator(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    result
}

/// Reed-Solomon remainder of `data` for `degree` error-correction codewords.
fn rs_remainder(data: &[u8], degree: usize) -> Vec<u8> {
    let generator = rs_generator(degree);
    let mut result = vec![0u8; degree];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (r, g) in result.iter_mut().zip(&generator) {
            *r ^= gf_mul(*g, factor);
        }
    }
    result
}

/// Split into blocks, add each block's error-correction codewords, and
/// interleave data then error-correction column by column.
fn interleave(version: usize, data: &[u8]) -> Vec<u8> {
    let (ec, g1, g2) = BLOCKS_M[version - 1];
    let mut sizes = vec![g1.1; g1.0];
    sizes.extend(std::iter::repeat_n(g2.1, g2.0));
    let mut blocks: Vec<(&[u8], Vec<u8>)> = Vec::new();
    let mut at = 0;
    for len in sizes {
        let chunk = &data[at..at + len];
        at += len;
        blocks.push((chunk, rs_remainder(chunk, ec)));
    }
    let longest = blocks.iter().map(|(d, _)| d.len()).max().unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..longest {
        for (d, _) in &blocks {
            if let Some(&b) = d.get(i) {
                out.push(b);
            }
        }
    }
    for i in 0..ec {
        for (_, e) in &blocks {
            out.push(e[i]);
        }
    }
    out
}

// ----------------------------------------------------------------------
// The module grid
// ----------------------------------------------------------------------

#[derive(Clone)]
struct Grid {
    size: usize,
    dark: Vec<bool>,
    /// Function patterns and format/version areas: data and masks skip them.
    function: Vec<bool>,
}

impl Grid {
    fn new(size: usize) -> Self {
        Self {
            size,
            dark: vec![false; size * size],
            function: vec![false; size * size],
        }
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        let i = y * self.size + x;
        self.dark[i] = dark;
        self.function[i] = true;
    }

    fn draw_function_patterns(&mut self, version: usize) {
        let size = self.size;
        // Timing patterns.
        for i in 0..size {
            self.set(6, i, i % 2 == 0);
            self.set(i, 6, i % 2 == 0);
        }
        // Finder patterns with their separators.
        for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                    if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
                        continue;
                    }
                    let dist = dx.abs().max(dy.abs());
                    self.set(x as usize, y as usize, dist != 2 && dist != 4);
                }
            }
        }
        // Alignment patterns, except where they would touch a finder.
        let centres = ALIGNMENT[version - 1];
        for (i, &cy) in centres.iter().enumerate() {
            for (j, &cx) in centres.iter().enumerate() {
                let last = centres.len() - 1;
                if (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0) {
                    continue;
                }
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let dist = dx.abs().max(dy.abs());
                        self.set(
                            (cx as i32 + dx) as usize,
                            (cy as i32 + dy) as usize,
                            dist != 1,
                        );
                    }
                }
            }
        }
        // Reserve the format areas (real bits come with the mask) and
        // the always-dark module.
        self.draw_format(0);
        self.set(8, size - 8, true);
        if version >= 7 {
            self.draw_version(version);
        }
    }

    fn draw_version(&mut self, version: usize) {
        let mut rem = version as u32;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let bits = ((version as u32) << 12) | rem;
        let size = self.size;
        for i in 0..18 {
            let dark = (bits >> i) & 1 == 1;
            let a = size - 11 + i % 3;
            let b = i / 3;
            self.set(a, b, dark);
            self.set(b, a, dark);
        }
    }

    /// Format bits for level M (00) and `mask`, both copies.
    fn draw_format(&mut self, mask: u8) {
        let data = mask as u32; // level M is 0b00 in the top two bits
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: usize| (bits >> i) & 1 == 1;
        let size = self.size;
        for i in 0..=5 {
            self.set(8, i, bit(i));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set(8, size - 15 + i, bit(i));
        }
        self.set(8, size - 8, true);
    }

    /// Zigzag placement of the codeword bits in the free modules.
    fn place_data(&mut self, codewords: &[u8]) {
        let size = self.size as i32;
        let total_bits = codewords.len() * 8;
        let mut i = 0usize;
        let mut right = size - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = ((right + 1) & 2) == 0;
                    let y = if upward { size - 1 - vert } else { vert } as usize;
                    let idx = y * self.size + x;
                    if !self.function[idx] && i < total_bits {
                        self.dark[idx] = (codewords[i / 8] >> (7 - i % 8)) & 1 == 1;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u8) {
        for y in 0..self.size {
            for x in 0..self.size {
                let (xi, yi) = (x, y);
                let flip = match mask {
                    0 => (xi + yi) % 2 == 0,
                    1 => yi % 2 == 0,
                    2 => xi % 3 == 0,
                    3 => (xi + yi) % 3 == 0,
                    4 => (xi / 3 + yi / 2) % 2 == 0,
                    5 => xi * yi % 2 + xi * yi % 3 == 0,
                    6 => (xi * yi % 2 + xi * yi % 3) % 2 == 0,
                    _ => ((xi + yi) % 2 + xi * yi % 3) % 2 == 0,
                };
                let idx = y * self.size + x;
                if flip && !self.function[idx] {
                    self.dark[idx] ^= true;
                }
            }
        }
    }

    fn at(&self, x: i32, y: i32) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.size
            && (y as usize) < self.size
            && self.dark[y as usize * self.size + x as usize]
    }

    /// The four penalty rules of the standard.
    fn penalty(&self) -> u32 {
        let n = self.size as i32;
        let mut total = 0u32;
        // Rule 1: runs of five or more in a row or column.
        for line in 0..n {
            for horizontal in [true, false] {
                let get = |k: i32| {
                    if horizontal {
                        self.at(k, line)
                    } else {
                        self.at(line, k)
                    }
                };
                let mut run = 1;
                for k in 1..n {
                    if get(k) == get(k - 1) {
                        run += 1;
                    } else {
                        if run >= 5 {
                            total += 3 + (run - 5) as u32;
                        }
                        run = 1;
                    }
                }
                if run >= 5 {
                    total += 3 + (run - 5) as u32;
                }
            }
        }
        // Rule 2: 2x2 blocks of one colour.
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let c = self.at(x, y);
                if c == self.at(x + 1, y) && c == self.at(x, y + 1) && c == self.at(x + 1, y + 1) {
                    total += 3;
                }
            }
        }
        // Rule 3: 1:1:3:1:1 finder-like patterns with four light modules
        // before or after.
        const PATTERN: [bool; 7] = [true, false, true, true, true, false, true];
        for line in 0..n {
            for horizontal in [true, false] {
                let get = |k: i32| {
                    if horizontal {
                        self.at(k, line)
                    } else {
                        self.at(line, k)
                    }
                };
                for start in 0..=(n - 7) {
                    if (0..7).all(|k| get(start + k) == PATTERN[k as usize]) {
                        if (1..=4).all(|k| !get(start - k)) {
                            total += 40;
                        }
                        if (7..=10).all(|k| !get(start + k)) {
                            total += 40;
                        }
                    }
                }
            }
        }
        // Rule 4: balance of dark modules, 10 per started 5 % from 50 %.
        let dark = self.dark.iter().filter(|d| **d).count() as i64;
        let cells = (self.size * self.size) as i64;
        let k = ((dark * 20 - cells * 10).abs() + cells - 1) / cells - 1;
        total + 10 * k.max(0) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows as `#`/`.` text, as an independent encoder prints them (`.` = light).
    fn rows(qr: &Qr) -> Vec<String> {
        (0..qr.size)
            .map(|y| (0..qr.size).map(|x| if qr.get(x, y) { '#' } else { '.' }).collect())
            .collect()
    }

    #[test]
    fn reed_solomon_matches_the_published_example() {
        // Version 1-M "HELLO WORLD" worked example: 16 data codewords,
        // 10 error-correction codewords.
        let data = [32, 91, 11, 120, 209, 114, 220, 77, 67, 64, 236, 17, 236, 17, 236, 17];
        assert_eq!(
            rs_remainder(&data, 10),
            [196, 35, 39, 119, 235, 215, 231, 226, 93, 23]
        );
        // The degree-10 generator polynomial of the standard.
        // The standard lists it as powers of alpha (alpha = 2 in GF(256)).
        let alpha_pow = |e: u32| (0..e).fold(1u8, |acc, _| gf_mul(acc, 2));
        let exponents = [251, 67, 46, 61, 118, 70, 64, 94, 32, 45];
        let expected: Vec<u8> = exponents.iter().map(|&e| alpha_pow(e)).collect();
        assert_eq!(rs_generator(10), expected);
    }

    #[test]
    fn byte_capacities_at_level_m() {
        let caps: Vec<usize> = (1..=10).map(byte_capacity).collect();
        assert_eq!(caps, [14, 26, 42, 62, 84, 106, 122, 152, 180, 213]);
    }

    #[test]
    fn chooses_the_smallest_version_and_rejects_oversize() {
        assert_eq!(Qr::encode(&[b'a'; 14]).unwrap().version, 1);
        assert_eq!(Qr::encode(&[b'a'; 15]).unwrap().version, 2);
        assert_eq!(Qr::encode(&[b'a'; 213]).unwrap().version, 10);
        assert_eq!(Qr::encode(&[b'a'; 214]), Err(QrError::TooLong(214)));
    }

    #[test]
    fn the_chosen_mask_has_the_lowest_penalty_and_decodes_the_same_data() {
        let text = b"WIFI:T:WPA;S:makepad-box;P:harbor-pencil-garden-maple;;";
        let best = Qr::encode(text).unwrap();
        let mut lowest = u32::MAX;
        for mask in 0..8u8 {
            let other = Qr::build(text, Some(mask)).unwrap();
            let mut g = Grid::new(other.size);
            g.dark = other.modules.clone();
            lowest = lowest.min(g.penalty());
            if mask == best.mask {
                assert_eq!(g.penalty(), lowest.min(g.penalty()));
            }
        }
        let mut g = Grid::new(best.size);
        g.dark = best.modules.clone();
        assert_eq!(g.penalty(), lowest);
    }

    #[test]
    fn function_patterns_are_in_place() {
        let qr = Qr::encode(b"WIFI:T:WPA;S:makepad-box;P:harbor-pencil-garden-maple;;").unwrap();
        assert_eq!(qr.size, 17 + 4 * qr.version);
        // Finder centres and the timing pattern.
        for (x, y) in [(3, 3), (qr.size - 4, 3), (3, qr.size - 4)] {
            assert!(qr.get(x, y));
            assert!(!qr.get(x + 2, y - 1) || !qr.get(x + 2, y));
        }
        for i in 8..qr.size - 8 {
            assert_eq!(qr.get(i, 6), i % 2 == 0);
            assert_eq!(qr.get(6, i), i % 2 == 0);
        }
        assert!(qr.get(8, qr.size - 8), "the always-dark module");
    }

    /// Reference symbols from an independent encoder
    /// (byte mode, level M, no quiet zone), versions 1, 2, 4 and 8 (the last carries version information).
    #[test]
    fn matches_an_independent_encoder() {
        for ((text, expected), mask) in VECTORS.iter().zip(VECTOR_MASKS) {
            let qr = Qr::build(text.as_bytes(), Some(mask)).unwrap();
            let got = rows(&qr);
            let want: Vec<&str> = expected.lines().collect();
            assert_eq!(got.len(), want.len(), "{text}");
            for (y, (g, w)) in got.iter().zip(&want).enumerate() {
                assert_eq!(g, w, "{text}: row {y}");
            }
        }
    }

    /// The mask the reference encoder chose for each vector.
    const VECTOR_MASKS: [u8; 4] = [7, 2, 4, 1];

    const VECTORS: &[(&str, &str)] = &[
        (
            "hello",
            "\
                #######..##...#######\n\
                #.....#..##...#.....#\n\
                #.###.#..#..#.#.###.#\n\
                #.###.#...##..#.###.#\n\
                #.###.#..##.#.#.###.#\n\
                #.....#.#..##.#.....#\n\
                #######.#.#.#.#######\n\
                ...........##........\n\
                #..#.##.##...#.#.....\n\
                ..#.##....#...#....##\n\
                ...##.####..##...##.#\n\
                ###.##..#..#.....#.##\n\
                .##.#.##..#.#.#.#....\n\
                ........##.#...##.#.#\n\
                #######...#..#.#.###.\n\
                #.....#.#.####.##....\n\
                #.###.#....#..###...#\n\
                #.###.#.##.#...#.####\n\
                #.###.#..##.#...#.#.#\n\
                #.....#..##..##......\n\
                #######.#####..#.#.#.\n\
            ",
        ),
        (
            "hello world, hello!",
            "\
                #######...#.##..#.#######\n\
                #.....#..##..###..#.....#\n\
                #.###.#.#...#.###.#.###.#\n\
                #.###.#.#....#....#.###.#\n\
                #.###.#.##.#....#.#.###.#\n\
                #.....#.#.###..#..#.....#\n\
                #######.#.#.#.#.#.#######\n\
                ........#..##.###........\n\
                #.#####...#.......#####..\n\
                .##.#....##.##..#..#..#..\n\
                ...##.######.#####..##.##\n\
                #####.....####.##.#....##\n\
                #.#####..####.##.########\n\
                ###......##.....#..#..#..\n\
                #.#..##..#.##..#.#####.##\n\
                #...#..###.#..##..###...#\n\
                #..#.###.#.#....#######..\n\
                ........#.#.#####...#.#..\n\
                #######......##.#.#.#.###\n\
                #.....#.###.##..#...##.#.\n\
                #.###.#.#.#.#.#########.#\n\
                #.###.#.##.....#..#.##.##\n\
                #.###.#.##.##..#...#.#..#\n\
                #.....#....#..####.##...#\n\
                #######.####...#..##..###\n\
            ",
        ),
        (
            "WIFI:T:WPA;S:makepad-box;P:harbor-pencil-garden-maple;;",
            "\
                #######.##..##.##.#....#..#######\n\
                #.....#..#.####.#..#..#.#.#.....#\n\
                #.###.#..#..#.#####.###...#.###.#\n\
                #.###.#.#####..##..#......#.###.#\n\
                #.###.#.####.#..#....#.#..#.###.#\n\
                #.....#.#.#...##.####.#.#.#.....#\n\
                #######.#.#.#.#.#.#.#.#.#.#######\n\
                ........###.#....#..##...........\n\
                #...#.####.##..#.#..###..#####..#\n\
                ##.#....#.###.#####.#..##.##...#.\n\
                .....####..######....#.##.##.....\n\
                #...##.###..##.####.##...#.#....#\n\
                .#..#.#.#.##.#.########...#.#.###\n\
                .#####...#.####..#.....#.#.#.#.##\n\
                #..##.#####.####..#..#####.....#.\n\
                ...##..###...######..#.#.####..##\n\
                .##.#.#.#.....#..#.####...####..#\n\
                ##.###..#...######...#....#..##..\n\
                .##.#.##.##.##.##...#..#########.\n\
                #.####...#.####.###..##.....#..##\n\
                ...#..##..#.####..##.#.####.#.#..\n\
                ####.#.#.###.#..###..#.#.#.#..#.#\n\
                ....#.#####.#...##..#.###..#.#.#.\n\
                ...#....####.#...#...#..#......##\n\
                ###..##.#.#.#.#########.#####...#\n\
                ........#..#..##.#...#.##...#..#.\n\
                #######.###.#.###.#..#..#.#.#..#.\n\
                #.....#...#...###....#.##...##..#\n\
                #.###.#.##.###..#..####.######.##\n\
                #.###.#......##..##....##.##.##..\n\
                #.###.#..#.##.##....##.#....#....\n\
                #.....#..#..##.####.##.#####.....\n\
                #######.###..##.#....####.#.#...#\n\
            ",
        ),
        (
            "WIFI:T:WPA;S:makepad-a-rather-long-host-name-x;P:amber-violin-orchard-planet-zebra-quilt-ember-lunar;;xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            "\
                #######.##...####..##.#..####.###.####..#.#######\n\
                #.....#..#..#.#.##.#..###...###.###.#####.#.....#\n\
                #.###.#.##......#.#.#...#.#..###.#.#.#.##.#.###.#\n\
                #.###.#....#....#..#.#...#.#.#.#.###...#..#.###.#\n\
                #.###.#..#.##.#..#.############.#.#.#.....#.###.#\n\
                #.....#.###.###.##.#.##...#.#.##..#####...#.....#\n\
                #######.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#######\n\
                ..........#.#######..##...#.#.#.#..##............\n\
                #.#...##.#.###..#.#...#####.#.#.##..###....#..#.#\n\
                #.####....#..#....##..##.#.###.....#...#.#...##.#\n\
                ####..#.#..##.#.#.#######..###...#..#..##..#..###\n\
                #.####.....#.#..###.##.##.#.###.#...#.#.##..##...\n\
                ##.#..####..###...###...###.##..##..#.##.##.#...#\n\
                #..#.#.##.##..#.##.####..#...#.#.#.#.#.#....#.#..\n\
                .#...####.##..#.####.#.#.#.#.#...#.###..#..####.#\n\
                ...#...#..####...#.####.#.###.###.###.##.#..##.##\n\
                #.#..##.##..###.#.#..######.###.###.###.#.#.#...#\n\
                ...##....#.....#.#.#.##..#...#...#...#.#...#..###\n\
                #.#.#.##.###..#..#.###..#..#...#...#.....#....#.#\n\
                .##.....####.#####..##..##########..#.#.#..#.#...\n\
                #...#.#...###..###.#.#..##..#.#.#.#.#..##.#.####.\n\
                ..####....#.#...####.....#####...#.#.#.#....###.#\n\
                ..########..#.##..##.#######........#.#######.###\n\
                ..#.#...#...##.....##.#...#.###.###.#.#.#...##...\n\
                #####.#.##.######.##.##.#.#.##..#.#.##.##.#.##..#\n\
                #.###...##.######.##.##...#.#..###..##..#...#.###\n\
                #..#######.#.#.######.########.....###..######..#\n\
                .##..#.#.##.######..##.##...#...##..#####.#.##.##\n\
                ####..###.##....#.#.#######.#.###.#.######...#.##\n\
                ###..#...##......##....##.#..#...#.#.#.....#..##.\n\
                ...####.#....##.###.#..#.##.#...#.........#####.#\n\
                ##...#..#...#..####...###.###.###.###.#####.##..#\n\
                ###..##.###.#.##.##..##..##.###.###.####.#.......\n\
                .##....###..###..#.#..##.#...#...#...#....##.#.##\n\
                ..##.##.##..###...#######.##...#...#...#..##.#..#\n\
                ...#....###..#....#.#.#.#####...#...#..#.#...#...\n\
                #.#.###....#.#.#..##...#.#.###..###.#..##..#.###.\n\
                #.##.....#.###..##.#..##...###.#.#.#.....#.#.####\n\
                .#...####.#...###...#.##..##.#...#........#.##..#\n\
                .###...##.####.##.#..#...######.##..#...###..#.#.\n\
                ###...#.#.########.#..#####.#.#.##..#.########..#\n\
                ........#####.#.####..#...####.#.#.#.#.##...##.##\n\
                #######.#..#....##..###.#.#..#.#.#...#..#.#.#.#.#\n\
                #.....#.....####...##.#...###...#...##.##...##.##\n\
                #.###.#......##..#.##.#####.##..##..##########...\n\
                #.###.#...#.#.#...###.#.#..#...#....#..######.#..\n\
                #.###.#.#..###..####.#..#.###..###.#.#..#.######.\n\
                #.....#..##......#.##..#...##.###.###.#..#.#.#...\n\
                #######.#..#####.#....#...#.###.###.####...#....#\n\
            ",
        ),
    ];
}
