//! A small QR Code encoder: byte mode, error correction level L, versions
//! 1..=10 (up to 271 bytes), the best of the eight masks. It draws the
//! pairing invitation as a picture; nothing else needs it.

/// (data codewords, ec codewords per block, blocks in group 1, data per
/// group-1 block, blocks in group 2, data per group-2 block), by version-1.
const L_TABLE: [(usize, usize, usize, usize, usize, usize); 10] = [
    (19, 7, 1, 19, 0, 0),
    (34, 10, 1, 34, 0, 0),
    (55, 15, 1, 55, 0, 0),
    (80, 20, 1, 80, 0, 0),
    (108, 26, 1, 108, 0, 0),
    (136, 18, 2, 68, 0, 0),
    (156, 20, 2, 78, 0, 0),
    (194, 24, 2, 97, 0, 0),
    (232, 30, 2, 116, 0, 0),
    (274, 18, 2, 68, 2, 69),
];

const ALIGN: [&[usize]; 10] = [
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

/// A finished symbol: `size` x `size` modules, row-major, true = dark.
pub struct Qr {
    pub size: usize,
    pub modules: Vec<bool>,
}

impl Qr {
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }

    /// The symbol as an SVG with a four-module quiet zone, dark on white.
    pub fn svg(&self) -> String {
        let q = 4;
        let n = self.size + 2 * q;
        let mut s = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {n} {n}\" shape-rendering=\"crispEdges\"><rect width=\"{n}\" height=\"{n}\" fill=\"#ffffff\"/><path fill=\"#000000\" d=\""
        );
        for y in 0..self.size {
            let mut x = 0;
            while x < self.size {
                if self.dark(x, y) {
                    let start = x;
                    while x < self.size && self.dark(x, y) {
                        x += 1;
                    }
                    s.push_str(&format!("M{} {}h{}v1h-{}z", start + q, y + q, x - start, x - start));
                } else {
                    x += 1;
                }
            }
        }
        s.push_str("\"/></svg>");
        s
    }
}

pub fn encode(text: &[u8]) -> Option<Qr> {
    let version = (1..=10).find(|v| {
        let capacity_bits = L_TABLE[v - 1].0 * 8;
        let count_bits = if *v <= 9 { 8 } else { 16 };
        4 + count_bits + text.len() * 8 <= capacity_bits
    })?;
    let (data_cw, ec_len, g1, g1_len, g2, g2_len) = L_TABLE[version - 1];

    // The bit stream: mode 0100, count, bytes, terminator, padding.
    let mut bits: Vec<bool> = Vec::new();
    let put = |value: u32, count: usize, bits: &mut Vec<bool>| {
        for i in (0..count).rev() {
            bits.push((value >> i) & 1 == 1);
        }
    };
    put(0b0100, 4, &mut bits);
    put(text.len() as u32, if version <= 9 { 8 } else { 16 }, &mut bits);
    for b in text {
        put(*b as u32, 8, &mut bits);
    }
    let capacity = data_cw * 8;
    let term = (capacity - bits.len()).min(4);
    put(0, term, &mut bits);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut data: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |a, b| (a << 1) | *b as u8)).collect();
    let mut pad = 0xEC;
    while data.len() < data_cw {
        data.push(pad);
        pad = if pad == 0xEC { 0x11 } else { 0xEC };
    }

    // Split into blocks, add Reed-Solomon codewords, interleave.
    let mut blocks: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut at = 0;
    for len in std::iter::repeat(g1_len).take(g1).chain(std::iter::repeat(g2_len).take(g2)) {
        let chunk = data[at..at + len].to_vec();
        at += len;
        let ec = reed_solomon(&chunk, ec_len);
        blocks.push((chunk, ec));
    }
    let mut stream: Vec<u8> = Vec::new();
    let longest = blocks.iter().map(|b| b.0.len()).max().unwrap_or(0);
    for i in 0..longest {
        for b in &blocks {
            if i < b.0.len() {
                stream.push(b.0[i]);
            }
        }
    }
    for i in 0..ec_len {
        for b in &blocks {
            stream.push(b.1[i]);
        }
    }

    let size = 17 + 4 * version;
    let mut grid = Grid::new(size);
    grid.draw_function_patterns(version);
    grid.draw_codewords(&stream);

    // Choose the mask with the lowest penalty.
    let mut best: Option<(u32, usize, Grid)> = None;
    for mask in 0..8 {
        let mut g = grid.clone();
        g.apply_mask(mask);
        g.draw_format(mask);
        let p = g.penalty();
        if best.as_ref().map_or(true, |b| p < b.0) {
            best = Some((p, mask, g));
        }
    }
    let (_, _, g) = best?;
    Some(Qr { size, modules: g.modules })
}

#[derive(Clone)]
struct Grid {
    size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl Grid {
    fn new(size: usize) -> Grid {
        Grid { size, modules: vec![false; size * size], function: vec![false; size * size] }
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }

    fn draw_function_patterns(&mut self, version: usize) {
        let n = self.size;
        for i in 0..n {
            self.set(6, i, i % 2 == 0);
            self.set(i, 6, i % 2 == 0);
        }
        self.finder(3, 3);
        self.finder(n - 4, 3);
        self.finder(3, n - 4);
        let pos = ALIGN[version - 1];
        for (i, &cy) in pos.iter().enumerate() {
            for (j, &cx) in pos.iter().enumerate() {
                let corner = (i == 0 && j == 0) || (i == 0 && j == pos.len() - 1) || (i == pos.len() - 1 && j == 0);
                if !corner {
                    self.alignment(cx, cy);
                }
            }
        }
        // Reserve the format area; the real bits come after masking.
        self.draw_format(0);
        if version >= 7 {
            let mut rem = version as u32;
            for _ in 0..12 {
                rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
            }
            let bits = (version as u32) << 12 | rem;
            for i in 0..18 {
                let dark = (bits >> i) & 1 == 1;
                let a = n - 11 + i % 3;
                let b = i / 3;
                self.set(a, b, dark);
                self.set(b, a, dark);
            }
        }
    }

    fn finder(&mut self, cx: usize, cy: usize) {
        for dy in -4i32..=4 {
            for dx in -4i32..=4 {
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if x < 0 || y < 0 || x >= self.size as i32 || y >= self.size as i32 {
                    continue;
                }
                let dist = dx.abs().max(dy.abs());
                self.set(x as usize, y as usize, dist != 2 && dist != 4);
            }
        }
    }

    fn alignment(&mut self, cx: usize, cy: usize) {
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let dist = dx.abs().max(dy.abs());
                self.set((cx as i32 + dx) as usize, (cy as i32 + dy) as usize, dist != 1);
            }
        }
    }

    /// Format information: error correction level L (01) and the mask.
    fn draw_format(&mut self, mask: usize) {
        let data = (0b01u32 << 3) | mask as u32;
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let n = self.size;
        let bit = |i: usize| (bits >> i) & 1 == 1;
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
            self.set(n - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set(8, n - 15 + i, bit(i));
        }
        self.set(8, n - 8, true);
    }

    fn draw_codewords(&mut self, stream: &[u8]) {
        let n = self.size;
        let mut i = 0usize;
        let mut right = n as i32 - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..n {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = ((right + 1) & 2) == 0;
                    let y = if upward { n - 1 - vert } else { vert };
                    if !self.function[y * n + x] && i < stream.len() * 8 {
                        self.modules[y * n + x] = (stream[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: usize) {
        let n = self.size;
        for y in 0..n {
            for x in 0..n {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                if invert && !self.function[y * n + x] {
                    self.modules[y * n + x] = !self.modules[y * n + x];
                }
            }
        }
    }

    fn penalty(&self) -> u32 {
        let n = self.size;
        let at = |x: usize, y: usize| self.modules[y * n + x];
        let mut p = 0u32;
        // Runs of five or more, rows and columns.
        for line in 0..n {
            for vertical in [false, true] {
                let mut run = 1;
                for i in 1..n {
                    let (a, b) = if vertical { (at(line, i), at(line, i - 1)) } else { (at(i, line), at(i - 1, line)) };
                    if a == b {
                        run += 1;
                        if run == 5 {
                            p += 3;
                        } else if run > 5 {
                            p += 1;
                        }
                    } else {
                        run = 1;
                    }
                }
            }
        }
        // 2x2 blocks.
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let c = at(x, y);
                if c == at(x + 1, y) && c == at(x, y + 1) && c == at(x + 1, y + 1) {
                    p += 3;
                }
            }
        }
        // Finder-like patterns.
        let pat1 = [true, false, true, true, true, false, true, false, false, false, false];
        let pat2 = [false, false, false, false, true, false, true, true, true, false, true];
        for y in 0..n {
            for x in 0..n.saturating_sub(10) {
                let row: Vec<bool> = (0..11).map(|i| at(x + i, y)).collect();
                let col: Vec<bool> = (0..11).map(|i| at(y, x + i)).collect();
                for w in [&row, &col] {
                    if w[..] == pat1 || w[..] == pat2 {
                        p += 40;
                    }
                }
            }
        }
        // Dark balance.
        let dark = self.modules.iter().filter(|m| **m).count() as i64;
        let total = (n * n) as i64;
        let k = ((dark * 20 - total * 10).abs() + total - 1) / total - 1;
        p + (k.max(0) as u32) * 10
    }
}

// ----------------------------------------------------------- Reed-Solomon

fn gf_mul(a: u8, b: u8) -> u8 {
    let mut r: u32 = 0;
    let (mut a, mut b) = (a as u32, b as u32);
    while b > 0 {
        if b & 1 == 1 {
            r ^= a;
        }
        a <<= 1;
        if a & 0x100 != 0 {
            a ^= 0x11D;
        }
        b >>= 1;
    }
    r as u8
}

fn generator(degree: usize) -> Vec<u8> {
    let mut g = vec![0u8; degree];
    g[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            g[j] = gf_mul(g[j], root);
            if j + 1 < degree {
                g[j] ^= g[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    g
}

fn reed_solomon(data: &[u8], degree: usize) -> Vec<u8> {
    let g = generator(degree);
    let mut rem = vec![0u8; degree];
    for &b in data {
        let factor = b ^ rem[0];
        rem.rotate_left(1);
        rem[degree - 1] = 0;
        for j in 0..degree {
            rem[j] ^= gf_mul(g[j], factor);
        }
    }
    rem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reed_solomon_words_are_multiples_of_the_generator() {
        // A codeword polynomial evaluates to zero at alpha^0 .. alpha^(degree-1).
        let data: Vec<u8> = (0..40u32).map(|i| (i * 37 + 11) as u8).collect();
        let ec = reed_solomon(&data, 20);
        let word: Vec<u8> = data.iter().chain(ec.iter()).copied().collect();
        let mut x = 1u8;
        for _ in 0..20 {
            let mut acc = 0u8;
            for &c in &word {
                acc = gf_mul(acc, x) ^ c;
            }
            assert_eq!(acc, 0);
            x = gf_mul(x, 2);
        }
    }

    #[test]
    fn the_known_format_bits_for_l_and_mask_zero() {
        let mut g = Grid::new(21);
        g.draw_format(0);
        // 111011111000100 read as bit 14 .. bit 0 along the first copy.
        let want = 0b111011111000100u32;
        let mut got = 0u32;
        for i in 0..=5 {
            got |= (g.modules[i * 21 + 8] as u32) << i;
        }
        got |= (g.modules[7 * 21 + 8] as u32) << 6;
        got |= (g.modules[8 * 21 + 8] as u32) << 7;
        got |= (g.modules[8 * 21 + 7] as u32) << 8;
        for i in 9..15 {
            got |= (g.modules[8 * 21 + (14 - i)] as u32) << i;
        }
        assert_eq!(got, want);
    }

    #[test]
    fn sizes_follow_the_text_length() {
        assert_eq!(encode(b"hello").unwrap().size, 21);
        assert_eq!(encode(&[b'a'; 100]).unwrap().size, 17 + 4 * 5);
        assert!(encode(&[b'a'; 400]).is_none());
        // finder pattern corners are dark
        let q = encode(b"x").unwrap();
        assert!(q.dark(0, 0) && q.dark(6, 0) && q.dark(0, 6) && q.dark(q.size - 1, 0));
    }
}
