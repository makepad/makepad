//! BC1 (a.k.a. DXT1) block texture codec.
//!
//! A BC1 block encodes a 4×4 texel tile in 8 bytes:
//!
//! ```text
//! bytes 0..2   c0 : u16 little-endian, RGB565
//! bytes 2..4   c1 : u16 little-endian, RGB565
//! bytes 4..8   indices : u32 little-endian, 2 bits per texel
//!              texel (x, y) uses bits [2*(4*y + x) .. +2]
//! ```
//!
//! RGB565 packing is `RRRRR GGGGGG BBBBB` (R in the high bits).
//!
//! # Colour tables
//!
//! The table is selected by an **unsigned comparison of the raw u16
//! endpoint values**, not by luminance or by the decoded RGB:
//!
//! - `c0 > c1`  — four-colour, fully opaque
//! - `c0 <= c1` — three-colour plus transparent black
//!
//! Equal endpoints therefore take the three-colour path. Index 3 is
//! transparent black in that mode and must not be emitted for opaque
//! texels.
//!
//! # Interpolation
//!
//! 1/3 and 2/3 mixes use **round-to-nearest** integer division
//! `(2 * a + b + 1) / 3` (and `(a + 2 * b + 1) / 3`). Hardware
//! implementations differ on this rounding; this codec picks the rounded
//! form. The 1/2 mix in three-colour mode is truncating `(a + b) / 2`.
//!
//! # Encoder
//!
//! Endpoints are a cheap diameter of the opaque colour cloud: the texel
//! farthest from the mean, then the texel farthest from that one. That is
//! not a full principal-axis (PCA) fit, so a tile whose true extrema are
//! not the two Euclidean-furthest points can land slightly off; the tests
//! only require a bounded error. Correctness of the bitstream matters
//! more than rate-distortion here.

/// One decoded 4×4 tile, row-major, RGBA8.
pub fn decode_block(block: &[u8; 8]) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let pal = palette(c0, c1);
    let mut out = [[0u8; 4]; 16];
    for i in 0..16 {
        let idx = ((indices >> (2 * i)) & 3) as usize;
        out[i] = pal[idx];
    }
    out
}

/// Decode a whole BC1 image.
///
/// `width` / `height` are in texels and need not be multiples of 4; blocks
/// cover `ceil(w/4) × ceil(h/4)` and texels past the edge are discarded.
/// Returns RGBA8, row-major, `width * height * 4` bytes.
///
/// Returns `Err` if `data` is shorter than the block count requires.
pub fn decode_image(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let need = encoded_len(width, height);
    if data.len() < need {
        return Err(format!(
            "BC1 data too short: need {need} bytes for {width}x{height}, got {}",
            data.len()
        ));
    }
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w.saturating_mul(h).saturating_mul(4)];
    if w == 0 || h == 0 {
        return Ok(out);
    }
    let blocks_x = (w + 3) / 4;
    let blocks_y = (h + 3) / 4;
    let mut off = 0;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let mut block = [0u8; 8];
            block.copy_from_slice(&data[off..off + 8]);
            off += 8;
            let texels = decode_block(&block);
            for y in 0..4 {
                let py = by * 4 + y;
                if py >= h {
                    continue;
                }
                for x in 0..4 {
                    let px = bx * 4 + x;
                    if px >= w {
                        continue;
                    }
                    let dst = (py * w + px) * 4;
                    out[dst..dst + 4].copy_from_slice(&texels[y * 4 + x]);
                }
            }
        }
    }
    Ok(out)
}

/// Encode one 4×4 RGBA8 tile to a BC1 block.
///
/// Alpha below `alpha_cutoff` selects the three-colour + transparent mode;
/// pass 0 to always use the four-colour mode (`u8` alpha is never `< 0`).
pub fn encode_block(texels: &[[u8; 4]; 16], alpha_cutoff: u8) -> [u8; 8] {
    let mut opaque = [[0u8; 3]; 16];
    let mut n_opaque = 0usize;
    let mut is_trans = [false; 16];
    for i in 0..16 {
        if texels[i][3] < alpha_cutoff {
            is_trans[i] = true;
        } else {
            opaque[n_opaque] = [texels[i][0], texels[i][1], texels[i][2]];
            n_opaque += 1;
        }
    }

    let (e0, e1) = pick_endpoints(&opaque[..n_opaque]);
    let mut c0 = pack_rgb565(e0[0], e0[1], e0[2]);
    let mut c1 = pack_rgb565(e1[0], e1[1], e1[2]);

    // Mode is decided by the raw u16 comparison. Transparent texels force
    // three-colour (c0 <= c1) so that index 3 is (0,0,0,0). All-opaque
    // tiles prefer four-colour (c0 > c1); equal endpoints fall through to
    // three-colour, which is correct for a solid colour provided we never
    // emit index 3.
    if n_opaque == 16 {
        if c0 < c1 {
            let t = c0;
            c0 = c1;
            c1 = t;
        }
    } else if c0 > c1 {
        let t = c0;
        c0 = c1;
        c1 = t;
    }

    let pal = palette(c0, c1);
    // In three-colour mode pal[3] is transparent black; opaque texels must
    // not be matched against it.
    let n_fit = if c0 > c1 { 4 } else { 3 };
    let mut indices = 0u32;
    for i in 0..16 {
        let idx = if is_trans[i] {
            3
        } else {
            nearest_index([texels[i][0], texels[i][1], texels[i][2]], &pal, n_fit)
        };
        indices |= idx << (2 * i);
    }
    pack_block(c0, c1, indices)
}

/// Encode a whole RGBA8 image.
///
/// Same edge rules as [`decode_image`]; texels past the edge are clamped
/// from the nearest in-bounds texel.
pub fn encode_image(
    rgba: &[u8],
    width: u32,
    height: u32,
    alpha_cutoff: u8,
) -> Result<Vec<u8>, String> {
    let w = width as usize;
    let h = height as usize;
    let need = w.saturating_mul(h).saturating_mul(4);
    if rgba.len() < need {
        return Err(format!(
            "RGBA data too short: need {need} bytes for {width}x{height}, got {}",
            rgba.len()
        ));
    }
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    let blocks_x = (w + 3) / 4;
    let blocks_y = (h + 3) / 4;
    let mut out = vec![0u8; blocks_x * blocks_y * 8];
    let mut off = 0;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let mut texels = [[0u8; 4]; 16];
            for y in 0..4 {
                let py = (by * 4 + y).min(h - 1);
                for x in 0..4 {
                    let px = (bx * 4 + x).min(w - 1);
                    let src = (py * w + px) * 4;
                    texels[y * 4 + x] = [rgba[src], rgba[src + 1], rgba[src + 2], rgba[src + 3]];
                }
            }
            let block = encode_block(&texels, alpha_cutoff);
            out[off..off + 8].copy_from_slice(&block);
            off += 8;
        }
    }
    Ok(out)
}

/// Bytes a BC1 image of this size occupies.
pub fn encoded_len(width: u32, height: u32) -> usize {
    let blocks_x = (width as usize + 3) / 4;
    let blocks_y = (height as usize + 3) / 4;
    blocks_x.saturating_mul(blocks_y).saturating_mul(8)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn pack_block(c0: u16, c1: u16, indices: u32) -> [u8; 8] {
    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&c0.to_le_bytes());
    block[2..4].copy_from_slice(&c1.to_le_bytes());
    block[4..8].copy_from_slice(&indices.to_le_bytes());
    block
}

/// Expand RGB565 → RGB888 by replicating high bits into the low bits.
fn expand5(v: u8) -> u8 {
    (v << 3) | (v >> 2)
}

fn expand6(v: u8) -> u8 {
    (v << 2) | (v >> 4)
}

fn unpack_rgb565(c: u16) -> (u8, u8, u8) {
    let r5 = ((c >> 11) & 0x1f) as u8;
    let g6 = ((c >> 5) & 0x3f) as u8;
    let b5 = (c & 0x1f) as u8;
    (expand5(r5), expand6(g6), expand5(b5))
}

fn pack_rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// Round-to-nearest `(2a + b) / 3`.
fn interp_21(a: u8, b: u8) -> u8 {
    ((2 * a as u16 + b as u16 + 1) / 3) as u8
}

/// Round-to-nearest `(a + 2b) / 3`.
fn interp_12(a: u8, b: u8) -> u8 {
    ((a as u16 + 2 * b as u16 + 1) / 3) as u8
}

/// Truncating `(a + b) / 2`.
fn interp_11(a: u8, b: u8) -> u8 {
    ((a as u16 + b as u16) / 2) as u8
}

fn palette(c0: u16, c1: u16) -> [[u8; 4]; 4] {
    let (r0, g0, b0) = unpack_rgb565(c0);
    let (r1, g1, b1) = unpack_rgb565(c1);
    if c0 > c1 {
        // Four-colour, opaque. colour2 is 2/3 of c0 toward c1; colour3 is
        // 1/3 of c0 toward c1. Swapping these two is a classic bug.
        [
            [r0, g0, b0, 255],
            [r1, g1, b1, 255],
            [interp_21(r0, r1), interp_21(g0, g1), interp_21(b0, b1), 255],
            [interp_12(r0, r1), interp_12(g0, g1), interp_12(b0, b1), 255],
        ]
    } else {
        // Three-colour + transparent. Includes the c0 == c1 boundary.
        [
            [r0, g0, b0, 255],
            [r1, g1, b1, 255],
            [interp_11(r0, r1), interp_11(g0, g1), interp_11(b0, b1), 255],
            [0, 0, 0, 0],
        ]
    }
}

fn sqdist(a: [u8; 3], b: [u8; 3]) -> u32 {
    let mut s = 0u32;
    for i in 0..3 {
        let d = a[i] as i32 - b[i] as i32;
        s += (d * d) as u32;
    }
    s
}

fn nearest_index(rgb: [u8; 3], pal: &[[u8; 4]; 4], n_fit: usize) -> u32 {
    let mut best = 0u32;
    let mut best_d = u32::MAX;
    for i in 0..n_fit {
        let d = sqdist(rgb, [pal[i][0], pal[i][1], pal[i][2]]);
        if d < best_d {
            best_d = d;
            best = i as u32;
        }
    }
    best
}

/// Cheap diameter of the colour cloud. See the module docs for the
/// quality trade-off versus a full PCA fit.
fn pick_endpoints(colors: &[[u8; 3]]) -> ([u8; 3], [u8; 3]) {
    match colors.len() {
        0 => return ([0, 0, 0], [0, 0, 0]),
        1 => return (colors[0], colors[0]),
        _ => {}
    }

    let mut sum = [0u32; 3];
    for c in colors {
        for k in 0..3 {
            sum[k] += c[k] as u32;
        }
    }
    let n = colors.len() as u32;

    // Farthest from the mean, compared as ||n*c - sum||^2 to stay integer.
    let mut best_i = 0;
    let mut best_d = 0u64;
    for (i, c) in colors.iter().enumerate() {
        let mut d = 0u64;
        for k in 0..3 {
            let diff = (c[k] as u32 * n) as i64 - sum[k] as i64;
            d += (diff * diff) as u64;
        }
        if d > best_d {
            best_d = d;
            best_i = i;
        }
    }
    let e0 = colors[best_i];

    let mut best_j = 0;
    let mut best_d2 = 0u32;
    for (j, c) in colors.iter().enumerate() {
        let d = sqdist(e0, *c);
        if d > best_d2 {
            best_d2 = d;
            best_j = j;
        }
    }
    (e0, colors[best_j])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec-side expansion, duplicated here so a broken expand5/expand6 in
    // the codec cannot hide behind a shared helper.
    fn spec_expand5(v: u8) -> u8 {
        (v << 3) | (v >> 2)
    }
    fn spec_expand6(v: u8) -> u8 {
        (v << 2) | (v >> 4)
    }

    fn make_block(c0: u16, c1: u16, indices: u32) -> [u8; 8] {
        pack_block(c0, c1, indices)
    }

    fn indices_rows(row0: u8, row1: u8, row2: u8, row3: u8) -> u32 {
        u32::from_le_bytes([row0, row1, row2, row3])
    }

    fn rgba565(r5: u8, g6: u8, b5: u8) -> [u8; 4] {
        [spec_expand5(r5), spec_expand6(g6), spec_expand5(b5), 255]
    }

    fn solid_tile(c: [u8; 4]) -> [[u8; 4]; 16] {
        [c; 16]
    }

    fn pixel(img: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    fn max_channel_err(a: [u8; 4], b: [u8; 4]) -> u8 {
        let mut m = 0u8;
        for i in 0..4 {
            let d = (a[i] as i16 - b[i] as i16).unsigned_abs() as u8;
            if d > m {
                m = d;
            }
        }
        m
    }

    fn max_tile_err(a: &[[u8; 4]; 16], b: &[[u8; 4]; 16]) -> u8 {
        let mut m = 0u8;
        for i in 0..16 {
            let d = max_channel_err(a[i], b[i]);
            if d > m {
                m = d;
            }
        }
        m
    }

    /// Numerical Recipes LCG; no `rand` dependency.
    struct Lcg(u32);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
            self.0
        }
        fn byte(&mut self) -> u8 {
            (self.next() >> 16) as u8
        }
    }

    // -- 1. Hand-built blocks ------------------------------------------------

    #[test]
    fn solid_colour_c0_eq_c1() {
        // Pure red: RGB565 0xF800. c0 == c1 selects three-colour mode, but
        // indices 0/1/2 all decode to the same opaque red.
        let block = make_block(0xf800, 0xf800, 0);
        let got = decode_block(&block);
        let red = [255, 0, 0, 255];
        for t in &got {
            assert_eq!(*t, red, "solid red block decoded to {t:?}");
        }
    }

    #[test]
    fn four_colour_all_four_indices() {
        // c0 = white 0xFFFF, c1 = black 0x0000. 0xFFFF > 0x0000 so this is
        // four-colour. Each row's bytes pack indices 0,1,2,3 as bits
        // 00_01_10_11 from LSB: 0b11100100 = 0xE4.
        //
        // colour2 = (2*255 + 0 + 1)/3 = 170
        // colour3 = (255 + 0 + 1)/3   = 85
        // If someone swapped the 1/3 and 2/3 interpolants, columns 2 and 3
        // would be exchanged. If they inverted the c0>c1 test, index 3
        // would be transparent black.
        let idx = indices_rows(0xe4, 0xe4, 0xe4, 0xe4);
        let block = make_block(0xffff, 0x0000, idx);
        let got = decode_block(&block);
        let c0 = [255, 255, 255, 255];
        let c1 = [0, 0, 0, 255];
        let c2 = [170, 170, 170, 255];
        let c3 = [85, 85, 85, 255];
        for y in 0..4 {
            assert_eq!(got[y * 4 + 0], c0, "index 0 at column 0 row {y}");
            assert_eq!(got[y * 4 + 1], c1, "index 1 at column 1 row {y}");
            assert_eq!(got[y * 4 + 2], c2, "index 2 at column 2 row {y}");
            assert_eq!(got[y * 4 + 3], c3, "index 3 at column 3 row {y}");
        }
    }

    #[test]
    fn three_colour_transparent_texel() {
        // c0 = black, c1 = white. c0 < c1 → three-colour + transparent.
        // colour2 = (0+255)/2 = 127 (truncating).
        // Same 0xE4 row pattern: indices 0,1,2,3 per row.
        let idx = indices_rows(0xe4, 0xe4, 0xe4, 0xe4);
        let block = make_block(0x0000, 0xffff, idx);
        let got = decode_block(&block);
        let c0 = [0, 0, 0, 255];
        let c1 = [255, 255, 255, 255];
        let c2 = [127, 127, 127, 255];
        let c3 = [0, 0, 0, 0];
        for y in 0..4 {
            assert_eq!(got[y * 4 + 0], c0);
            assert_eq!(got[y * 4 + 1], c1);
            assert_eq!(got[y * 4 + 2], c2);
            assert_eq!(got[y * 4 + 3], c3, "index 3 must be transparent black");
        }
    }

    #[test]
    fn c0_equals_c1_selects_three_colour() {
        // The boundary c0 == c1 is `<=`, so three-colour + transparent.
        // Index 3 is (0,0,0,0), not an interpolated opaque colour.
        let mut indices = 0u32;
        for i in 0..16 {
            indices |= 3 << (2 * i);
        }
        let block = make_block(0x7e0, 0x7e0, indices); // both pure green
        let got = decode_block(&block);
        for t in &got {
            assert_eq!(*t, [0, 0, 0, 0]);
        }

        // Index 0 is still the expanded endpoint.
        let block = make_block(0x7e0, 0x7e0, 0);
        let got = decode_block(&block);
        for t in &got {
            assert_eq!(*t, [0, 255, 0, 255]);
        }
    }

    #[test]
    fn little_endian_endpoints() {
        // 0xF800 stored LE is bytes [00, F8]. Reading as BE would yield
        // 0x00F8 (almost pure green), not red.
        let block = [0x00, 0xf8, 0x00, 0xf8, 0, 0, 0, 0];
        let got = decode_block(&block);
        assert_eq!(got[0], [255, 0, 0, 255]);
    }

    #[test]
    fn index_bit_order_is_x_then_y() {
        // Only texel (1, 2) is index 1; everything else is index 0.
        // Address = 4*y + x = 9, so bits 18..19.
        // A y-major layout (4*x + y = 6) would light a different texel.
        let indices = 1u32 << (2 * (4 * 2 + 1));
        let block = make_block(0xffff, 0x0000, indices);
        let got = decode_block(&block);
        for y in 0..4 {
            for x in 0..4 {
                let t = got[y * 4 + x];
                if x == 1 && y == 2 {
                    assert_eq!(t, [0, 0, 0, 255], "texel (1,2) should be colour1");
                } else {
                    assert_eq!(t, [255, 255, 255, 255], "texel ({x},{y}) should be colour0");
                }
            }
        }
    }

    #[test]
    fn interpolation_rounding_is_plus_one_over_three() {
        // r5=2 expands to 16. Four-colour against black:
        //   colour2 r = (2*16 + 0 + 1)/3 = 11   (truncating 32/3 would be 10)
        //   colour3 r = (16 + 0 + 1)/3   = 5
        let idx = indices_rows(0xe4, 0, 0, 0);
        let block = make_block(0x1000, 0x0000, idx);
        let got = decode_block(&block);
        assert_eq!(got[0], [16, 0, 0, 255], "colour0");
        assert_eq!(got[1], [0, 0, 0, 255], "colour1");
        assert_eq!(got[2], [11, 0, 0, 255], "colour2 rounded 2/3");
        assert_eq!(got[3], [5, 0, 0, 255], "colour3 rounded 1/3");

        // r5=1 expands to 8. colour3 is where +1 changes the 1/3 mix:
        //   colour3 r = (8 + 1)/3 = 3   (truncating 8/3 would be 2)
        let block = make_block(0x0800, 0x0000, idx);
        let got = decode_block(&block);
        assert_eq!(got[0], [8, 0, 0, 255]);
        assert_eq!(got[3], [3, 0, 0, 255], "colour3 rounded 1/3 of 8");
    }

    // -- 2. Endpoint expansion ----------------------------------------------

    #[test]
    fn endpoint_expansion_zero_and_max() {
        // 0 → 0 and max → 255 for each channel width, proving bit
        // replication rather than a shift-only expand (which would give
        // 248 / 252 at the top).
        assert_eq!(spec_expand5(0), 0);
        assert_eq!(spec_expand5(31), 255);
        assert_eq!(spec_expand6(0), 0);
        assert_eq!(spec_expand6(63), 255);

        let cases: [(u16, [u8; 4]); 4] = [
            (0x0000, [0, 0, 0, 255]),       // all zero
            (0xf800, [255, 0, 0, 255]),     // R max
            (0x07e0, [0, 255, 0, 255]),     // G max
            (0x001f, [0, 0, 255, 255]),     // B max
        ];
        for (c, expected) in cases {
            let got = decode_block(&make_block(c, c, 0));
            assert_eq!(got[0], expected, "endpoint 0x{c:04x}");
        }

        // White is all channels at max.
        let got = decode_block(&make_block(0xffff, 0xffff, 0));
        assert_eq!(got[0], [255, 255, 255, 255]);
    }

    // -- 3. Round trip ------------------------------------------------------

    #[test]
    fn solid_tiles_round_trip_exactly() {
        // Solids built from RGB565 expansions must come back bit-identical.
        let colours = [
            rgba565(0, 0, 0),
            rgba565(31, 0, 0),
            rgba565(0, 63, 0),
            rgba565(0, 0, 31),
            rgba565(31, 63, 31),
            rgba565(16, 32, 8),
            rgba565(1, 1, 1),
            rgba565(31, 31, 0),
        ];
        for c in colours {
            let tile = solid_tile(c);
            let enc = encode_block(&tile, 0);
            let dec = decode_block(&enc);
            for (i, t) in dec.iter().enumerate() {
                assert_eq!(*t, c, "solid {c:?} texel {i} became {t:?}");
            }
        }
    }

    #[test]
    fn two_tone_gradient_random_round_trip() {
        // Per-channel absolute-error budgets. Solids are asserted exact
        // above; two-tone of representable colours should also be exact
        // because the diameter fit picks the two source colours.
        //
        // A 16-level gradient is squeezed onto four palette entries, so
        // the worst texel sits about 1/6 of the endpoint span away from
        // the nearest colour (~20 on a 0..123 channel). 24 is a tight
        // bound that still leaves room for 565 quantisation of the ends.
        const TWO_TONE: u8 = 0;
        const GRADIENT: u8 = 24;
        const RANDOM: u8 = 96;

        let red = rgba565(31, 0, 0);
        let blue = rgba565(0, 0, 31);
        let mut two_tone = [[0u8; 4]; 16];
        for i in 0..16 {
            two_tone[i] = if i < 8 { red } else { blue };
        }
        let enc = encode_block(&two_tone, 0);
        let dec = decode_block(&enc);
        assert!(
            max_tile_err(&two_tone, &dec) <= TWO_TONE,
            "two-tone error {} > {TWO_TONE}",
            max_tile_err(&two_tone, &dec)
        );

        // Gray-ish gradient using representable channel values.
        let mut gradient = [[0u8; 4]; 16];
        for i in 0..16 {
            let t = i as u8;
            gradient[i] = rgba565(t, t * 2 + 1, t);
        }
        let enc = encode_block(&gradient, 0);
        let dec = decode_block(&enc);
        assert!(
            max_tile_err(&gradient, &dec) <= GRADIENT,
            "gradient error {} > {GRADIENT}",
            max_tile_err(&gradient, &dec)
        );

        // Seeded-random tiles that still lie on a colour line (two LCG
        // endpoints, each texel a random blend). Unconstrained 16-colour
        // clouds are outside what four collinear palette entries can
        // represent; this is the distribution BC1 is meant for.
        let mut rng = Lcg(0xC0FFEE);
        for trial in 0..8 {
            let a = [rng.byte(), rng.byte(), rng.byte()];
            let b = [rng.byte(), rng.byte(), rng.byte()];
            let mut random = [[0u8; 4]; 16];
            for i in 0..16 {
                let t = rng.byte() as u16;
                let mut c = [0u8; 4];
                for k in 0..3 {
                    c[k] = ((a[k] as u16 * (255 - t) + b[k] as u16 * t) / 255) as u8;
                }
                c[3] = 255;
                random[i] = c;
            }
            let enc = encode_block(&random, 0);
            let dec = decode_block(&enc);
            let err = max_tile_err(&random, &dec);
            assert!(
                err <= RANDOM,
                "random trial {trial} error {err} > {RANDOM}"
            );
        }
    }

    #[test]
    fn punch_through_alpha_uses_three_colour() {
        let red = rgba565(31, 0, 0);
        let mut tile = solid_tile(red);
        tile[5] = [0, 0, 0, 0];
        tile[10] = [10, 20, 30, 1]; // below cutoff 128
        let enc = encode_block(&tile, 128);
        let dec = decode_block(&enc);
        assert_eq!(dec[5], [0, 0, 0, 0]);
        assert_eq!(dec[10], [0, 0, 0, 0]);
        // Opaque texels stay red (representable solid).
        assert_eq!(dec[0], red);
        assert_eq!(dec[15], red);

        // cutoff 0 must never punch through, even if alpha is 0.
        let enc = encode_block(&tile, 0);
        let dec = decode_block(&enc);
        assert_eq!(dec[5][3], 255, "cutoff 0 is always four-colour/opaque");
    }

    // -- 4. Non-multiple-of-4 image sizes -----------------------------------

    #[test]
    fn decode_image_odd_sizes_discard_edge_texels() {
        // Hand-built blocks so we assert exact edge pixels, independent of
        // the encoder. Blocks are stored row-major.
        let red = make_block(0xf800, 0xf800, 0);
        let green = make_block(0x07e0, 0x07e0, 0);
        let blue = make_block(0x001f, 0x001f, 0);
        let white = make_block(0xffff, 0xffff, 0);

        // 1×1: one block, keep texel (0,0).
        let img = decode_image(&red, 1, 1).unwrap();
        assert_eq!(img.len(), 4);
        assert_eq!(pixel(&img, 1, 0, 0), [255, 0, 0, 255]);

        // 5×4: two horizontal blocks. x=0..3 from red, x=4 from green.
        let mut data = Vec::new();
        data.extend_from_slice(&red);
        data.extend_from_slice(&green);
        let img = decode_image(&data, 5, 4).unwrap();
        assert_eq!(img.len(), 5 * 4 * 4);
        assert_eq!(pixel(&img, 5, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&img, 5, 3, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&img, 5, 4, 0), [0, 255, 0, 255]);
        assert_eq!(pixel(&img, 5, 4, 3), [0, 255, 0, 255]);

        // 3×5: two vertical blocks. y=0..3 from red, y=4 from blue.
        let mut data = Vec::new();
        data.extend_from_slice(&red);
        data.extend_from_slice(&blue);
        let img = decode_image(&data, 3, 5).unwrap();
        assert_eq!(img.len(), 3 * 5 * 4);
        assert_eq!(pixel(&img, 3, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&img, 3, 2, 3), [255, 0, 0, 255]);
        assert_eq!(pixel(&img, 3, 1, 4), [0, 0, 255, 255]);

        // 7×7: 2×2 blocks = red, green / blue, white.
        let mut data = Vec::new();
        data.extend_from_slice(&red);
        data.extend_from_slice(&green);
        data.extend_from_slice(&blue);
        data.extend_from_slice(&white);
        let img = decode_image(&data, 7, 7).unwrap();
        assert_eq!(img.len(), 7 * 7 * 4);
        assert_eq!(pixel(&img, 7, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&img, 7, 6, 0), [0, 255, 0, 255]);
        assert_eq!(pixel(&img, 7, 0, 6), [0, 0, 255, 255]);
        assert_eq!(pixel(&img, 7, 6, 6), [255, 255, 255, 255]);

        // 4×4 and 16×16 length checks with a repeating red block.
        let img = decode_image(&red, 4, 4).unwrap();
        assert_eq!(img.len(), 4 * 4 * 4);
        assert_eq!(pixel(&img, 4, 3, 3), [255, 0, 0, 255]);

        let data = red.repeat(4 * 4); // 16×16 = 4×4 blocks
        let img = decode_image(&data, 16, 16).unwrap();
        assert_eq!(img.len(), 16 * 16 * 4);
        assert_eq!(pixel(&img, 16, 15, 15), [255, 0, 0, 255]);
    }

    #[test]
    fn encode_image_odd_sizes_clamp_and_round_trip_solid() {
        let sizes = [(1u32, 1u32), (3, 5), (7, 7), (4, 4), (16, 16)];
        let red = rgba565(31, 0, 0);
        for (w, h) in sizes {
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            for p in rgba.chunks_mut(4) {
                p.copy_from_slice(&red);
            }
            let enc = encode_image(&rgba, w, h, 0).unwrap();
            assert_eq!(enc.len(), encoded_len(w, h), "{w}x{h} encoded_len");
            let dec = decode_image(&enc, w, h).unwrap();
            assert_eq!(dec.len(), (w * h * 4) as usize);
            for p in dec.chunks(4) {
                assert_eq!(p, red, "solid red {w}x{h}");
            }
        }
    }

    #[test]
    fn encode_image_preserves_distinct_corners() {
        // Unique representable colours at the four corners of a 7×7.
        let mut rgba = vec![0u8; 7 * 7 * 4];
        let fill = rgba565(8, 16, 8);
        for p in rgba.chunks_mut(4) {
            p.copy_from_slice(&fill);
        }
        let corners = [
            (0u32, 0u32, rgba565(31, 0, 0)),
            (6, 0, rgba565(0, 63, 0)),
            (0, 6, rgba565(0, 0, 31)),
            (6, 6, rgba565(31, 63, 31)),
        ];
        for (x, y, c) in corners {
            let i = ((y * 7 + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&c);
        }
        let enc = encode_image(&rgba, 7, 7, 0).unwrap();
        let dec = decode_image(&enc, 7, 7).unwrap();
        // Each corner lives in its own 4×4 block, so a diameter fit of a
        // mostly-flat block plus one corner colour should keep that
        // colour within a couple of 565 steps.
        for (x, y, c) in corners {
            let got = pixel(&dec, 7, x, y);
            assert!(
                max_channel_err(got, c) <= 8,
                "corner ({x},{y}) {got:?} vs {c:?}"
            );
        }
    }

    // -- 5. encoded_len agrees with encode_image ----------------------------

    #[test]
    fn encoded_len_matches_encode_image() {
        let sizes = [
            (0, 0),
            (0, 7),
            (7, 0),
            (1, 1),
            (3, 5),
            (7, 7),
            (4, 4),
            (16, 16),
            (5, 1),
            (1, 5),
            (4, 5),
            (5, 4),
        ];
        for (w, h) in sizes {
            let rgba = vec![0u8; (w * h * 4) as usize];
            let enc = encode_image(&rgba, w, h, 0).unwrap();
            assert_eq!(enc.len(), encoded_len(w, h), "{w}x{h}");
        }
        assert_eq!(encoded_len(1, 1), 8);
        assert_eq!(encoded_len(3, 5), 16);
        assert_eq!(encoded_len(7, 7), 32);
        assert_eq!(encoded_len(4, 4), 8);
        assert_eq!(encoded_len(16, 16), 128);
    }

    // -- 6. Truncated input -------------------------------------------------

    #[test]
    fn decode_image_truncated_returns_err() {
        assert!(decode_image(&[], 1, 1).is_err());
        assert!(decode_image(&[0; 7], 1, 1).is_err());
        assert!(decode_image(&[0; 8], 5, 4).is_err()); // needs 16
        assert!(decode_image(&[0; 31], 7, 7).is_err()); // needs 32
        // Exactly the right length is Ok, not Err.
        assert!(decode_image(&[0; 8], 1, 1).is_ok());
        assert!(decode_image(&[0; 8], 4, 4).is_ok());
    }

    #[test]
    fn encode_image_truncated_rgba_returns_err() {
        assert!(encode_image(&[0; 3], 1, 1, 0).is_err());
        assert!(encode_image(&[], 2, 2, 0).is_err());
    }

    // -- 7. Decode → encode → decode stability ------------------------------

    #[test]
    fn decode_encode_decode_stability() {
        // An already-quantised four-colour tile should re-encode near itself.
        let idx = indices_rows(0xe4, 0xe4, 0xe4, 0xe4);
        let original = make_block(0xffff, 0x0000, idx);
        let first = decode_block(&original);
        let reenc = encode_block(&first, 0);
        let second = decode_block(&reenc);
        assert!(
            max_tile_err(&first, &second) <= 2,
            "stability error {}",
            max_tile_err(&first, &second)
        );

        // Three-colour + transparent: the transparent texels must stay
        // transparent, and the opaque palette colours must stay close.
        let original = make_block(0x0000, 0xffff, idx);
        let first = decode_block(&original);
        let reenc = encode_block(&first, 128);
        let second = decode_block(&reenc);
        for i in 0..16 {
            if first[i][3] == 0 {
                assert_eq!(second[i], [0, 0, 0, 0]);
            } else {
                assert!(max_channel_err(first[i], second[i]) <= 2);
            }
        }
    }

    #[test]
    fn all_transparent_block() {
        let tile = solid_tile([1, 2, 3, 0]);
        let enc = encode_block(&tile, 1);
        let dec = decode_block(&enc);
        for t in &dec {
            assert_eq!(*t, [0, 0, 0, 0]);
        }
    }
}
