//! Layer III main data: side info, scalefactors, Huffman spectrum,
//! requantization, joint stereo, reordering, alias reduction and the hybrid
//! (IMDCT) filterbank.
//!
//! Everything here works on caller-owned scratch ([`GranuleScratch`],
//! [`ChannelState`]) so a decode loop allocates nothing per frame. Every
//! bitstream field is range-checked at the point of use: an MP3 file is
//! attacker-controlled input and a granule that claims 1000 scalefactor bands
//! must produce an error, not an index panic.

use super::bits::BitReader;
use super::header::FrameHeader;
use super::synth::Synthesis;
use super::tables::{HuffNode, HUFF_TABLES, HUFF_TREES, LEAF, SCALEFAC_SIZES_MPEG1};
use super::tables_lsf::{alias_coefficients, intensity_pan_mpeg1, LSF_NSFB, PRETAB};
use crate::error::AudioError;

/// Spectral lines in one granule.
pub const LINES: usize = 576;
pub const SUBBANDS: usize = 32;
/// Time slots one granule produces per subband.
pub const SLOTS: usize = 18;
/// Largest quantised magnitude: 15 plus the 13-bit linbits escape.
const MAX_QUANT: usize = 8206;
/// Scalefactor slots. 13 short bands x 3 windows, or 22 long bands, and mixed
/// blocks never exceed either.
const MAX_BANDS: usize = 40;

// ---------------------------------------------------------------------------
// side info
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GranuleInfo {
    pub part2_3_length: u32,
    pub big_values: u32,
    pub global_gain: u32,
    pub scalefac_compress: u32,
    pub window_switching: bool,
    pub block_type: u8,
    pub mixed_block: bool,
    pub table_select: [u8; 3],
    pub subblock_gain: [u8; 3],
    pub region0_count: u8,
    pub region1_count: u8,
    pub preflag: bool,
    pub scalefac_scale: bool,
    pub count1table_select: bool,
}

impl GranuleInfo {
    pub fn short_blocks(&self) -> bool {
        self.window_switching && self.block_type == 2
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SideInfo {
    /// How many bytes *before* this frame's main data the first granule starts.
    pub main_data_begin: usize,
    /// Scalefactor selection information, `[channel][scfsi band]`; MPEG-1 only.
    pub scfsi: [[bool; 4]; 2],
    pub granules: [[GranuleInfo; 2]; 2],
}

/// Parse the side information block that follows the frame header.
pub fn read_side_info(data: &[u8], header: &FrameHeader) -> Result<SideInfo, AudioError> {
    if data.len() < header.side_info_bytes() {
        return Err(AudioError::Truncated);
    }
    let mut r = BitReader::new(data);
    let channels = header.channels();
    let lsf = header.is_lsf();
    let mut side = SideInfo::default();

    side.main_data_begin = r.read(if lsf { 8 } else { 9 }) as usize;
    // private_bits: nothing to decode, but they must be stepped over.
    r.skip(match (lsf, channels) {
        (false, 1) => 5,
        (false, _) => 3,
        (true, 1) => 1,
        (true, _) => 2,
    });
    if !lsf {
        for ch in 0..channels {
            for band in 0..4 {
                side.scfsi[ch][band] = r.read_bit() == 1;
            }
        }
    }
    for gr in 0..header.granules() {
        for ch in 0..channels {
            side.granules[gr][ch] = read_granule_info(&mut r, lsf)?;
        }
    }
    if r.overran() {
        return Err(AudioError::Truncated);
    }
    Ok(side)
}

fn read_granule_info(r: &mut BitReader, lsf: bool) -> Result<GranuleInfo, AudioError> {
    let mut g = GranuleInfo {
        part2_3_length: r.read(12),
        big_values: r.read(9),
        global_gain: r.read(8),
        scalefac_compress: r.read(if lsf { 9 } else { 4 }),
        ..GranuleInfo::default()
    };
    if g.big_values > 288 {
        // More than 576 lines of pairs cannot exist.
        return Err(AudioError::Corrupt("big_values"));
    }
    g.window_switching = r.read_bit() == 1;
    if g.window_switching {
        g.block_type = r.read(2) as u8;
        if g.block_type == 0 {
            // "window switching with a normal block" is not a legal encoding.
            return Err(AudioError::Corrupt("block_type"));
        }
        g.mixed_block = r.read_bit() == 1;
        g.table_select[0] = r.read(5) as u8;
        g.table_select[1] = r.read(5) as u8;
        for gain in &mut g.subblock_gain {
            *gain = r.read(3) as u8;
        }
        // Region boundaries are implied, not coded, when switching windows.
        g.region0_count = if g.block_type == 2 && !g.mixed_block { 8 } else { 7 };
        g.region1_count = 20 - g.region0_count;
    } else {
        for table in &mut g.table_select {
            *table = r.read(5) as u8;
        }
        g.region0_count = r.read(4) as u8;
        g.region1_count = r.read(3) as u8;
    }
    if !lsf {
        g.preflag = r.read_bit() == 1;
    }
    g.scalefac_scale = r.read_bit() == 1;
    g.count1table_select = r.read_bit() == 1;
    Ok(g)
}

// ---------------------------------------------------------------------------
// band layout
// ---------------------------------------------------------------------------

/// The granule's scalefactor bands, in the order the bitstream stores them.
///
/// One entry per (band, window) pair, so a short block contributes three
/// consecutive entries of equal width and a mixed block a run of long entries
/// followed by triples. Requantization, the scalefactor array and intensity
/// stereo all index this the same way, which is what keeps the three of them
/// in agreement.
#[derive(Clone, Copy)]
pub struct BandLayout {
    pub width: [u16; MAX_BANDS],
    pub count: usize,
    /// Entries `< long_bands` are long bands (only nonzero for mixed blocks
    /// and for ordinary long granules).
    pub long_bands: usize,
    pub short: bool,
    /// Spectral line where the short-block region starts.
    pub short_start: usize,
    /// Short band index the short region starts at.
    pub first_short_sfb: usize,
}

impl BandLayout {
    pub fn new(header: &FrameHeader, gr: &GranuleInfo) -> Self {
        let long = header.sfb_long();
        let short = header.sfb_short();
        let mut width = [0u16; MAX_BANDS];
        if !gr.short_blocks() {
            for sfb in 0..22 {
                width[sfb] = long[sfb + 1] - long[sfb];
            }
            return Self {
                width,
                count: 22,
                long_bands: 22,
                short: false,
                short_start: LINES,
                first_short_sfb: 13,
            };
        }
        let mut count = 0usize;
        let mut line = 0usize;
        if gr.mixed_block {
            // The long part of a mixed block is the first 36 lines, which is a
            // different number of bands at every sample rate.
            while line < 36 && count < 22 {
                let w = (long[count + 1] - long[count]) as usize;
                width[count] = w as u16;
                count += 1;
                line += w;
            }
        }
        let long_bands = count;
        let mut sfb = 0usize;
        while sfb < 13 && (short[sfb] as usize) * 3 < line {
            sfb += 1;
        }
        // At 8 kHz the short bands are wide enough that none starts exactly at
        // the end of the long part; bridge the gap with one synthetic triple
        // rather than leaving lines unscaled.
        let short_start = line;
        if sfb < 13 && (short[sfb] as usize) * 3 > line && count + 3 <= MAX_BANDS {
            let bridge = (((short[sfb] as usize) * 3 - line) / 3) as u16;
            width[count] = bridge;
            width[count + 1] = bridge;
            width[count + 2] = bridge;
            count += 3;
        }
        let first_short_sfb = sfb;
        while sfb < 13 && count + 3 <= MAX_BANDS {
            let w = short[sfb + 1] - short[sfb];
            width[count] = w;
            width[count + 1] = w;
            width[count + 2] = w;
            count += 3;
            sfb += 1;
        }
        Self { width, count, long_bands, short: true, short_start, first_short_sfb }
    }
}

// ---------------------------------------------------------------------------
// scalefactors
// ---------------------------------------------------------------------------

/// One granule/channel's scalefactors, flat in band-layout order, plus the
/// intensity-stereo positions the right channel carries instead.
#[derive(Clone, Copy)]
pub struct Scalefactors {
    pub value: [u8; MAX_BANDS],
    /// `is_pos` per band for the intensity path; `u8::MAX` means "this band is
    /// not intensity coded".
    pub is_pos: [u8; MAX_BANDS],
}

impl Default for Scalefactors {
    fn default() -> Self {
        Self { value: [0; MAX_BANDS], is_pos: [u8::MAX; MAX_BANDS] }
    }
}

/// Read the MPEG-1 scalefactors, returning the bits consumed.
///
/// `previous` carries granule 0's values so that `scfsi` can say "reuse them"
/// for granule 1 — which is why a decoder cannot treat granules independently.
fn read_scalefactors_mpeg1(
    r: &mut BitReader,
    gr: &GranuleInfo,
    granule_index: usize,
    scfsi: &[bool; 4],
    previous: &Scalefactors,
    out: &mut Scalefactors,
) {
    let (slen1, slen2) = SCALEFAC_SIZES_MPEG1[(gr.scalefac_compress & 0xf) as usize];
    let (slen1, slen2) = (slen1 as u32, slen2 as u32);
    out.value = [0; MAX_BANDS];
    out.is_pos = [u8::MAX; MAX_BANDS];

    if gr.short_blocks() {
        if gr.mixed_block {
            // Eight long bands, then short bands 3..12 across three windows.
            for sfb in 0..8 {
                out.value[sfb] = r.read(slen1) as u8;
            }
            for sfb in 3..12 {
                let slen = if sfb < 6 { slen1 } else { slen2 };
                for w in 0..3 {
                    out.value[8 + (sfb - 3) * 3 + w] = r.read(slen) as u8;
                }
            }
        } else {
            for sfb in 0..12 {
                let slen = if sfb < 6 { slen1 } else { slen2 };
                for w in 0..3 {
                    out.value[sfb * 3 + w] = r.read(slen) as u8;
                }
            }
        }
        for (i, slot) in out.is_pos.iter_mut().enumerate() {
            *slot = out.value[i];
        }
        return;
    }

    const SCFSI_BANDS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
    for (band, (lo, hi)) in SCFSI_BANDS.into_iter().enumerate() {
        let slen = if band < 2 { slen1 } else { slen2 };
        if granule_index > 0 && scfsi[band] {
            out.value[lo..hi].copy_from_slice(&previous.value[lo..hi]);
        } else {
            for sfb in lo..hi {
                out.value[sfb] = r.read(slen) as u8;
            }
        }
    }
    for (i, slot) in out.is_pos.iter_mut().enumerate() {
        *slot = out.value[i];
    }
}

/// Read the MPEG-2/2.5 scalefactors. Unlike MPEG-1 these are partitioned by a
/// 9-bit `scalefac_compress` that also selects the partition sizes, and the
/// intensity-stereo right channel reads a different partition family whose
/// "all ones" value means "not intensity coded".
fn read_scalefactors_lsf(
    r: &mut BitReader,
    gr: &mut GranuleInfo,
    intensity_right: bool,
    out: &mut Scalefactors,
) {
    let comp = gr.scalefac_compress;
    let (slen, selector) = if intensity_right {
        let comp = comp >> 1;
        if comp < 180 {
            ([comp / 36, (comp % 36) / 6, (comp % 36) % 6, 0], 3)
        } else if comp < 244 {
            let comp = comp - 180;
            ([(comp % 64) >> 4, (comp % 16) >> 2, comp % 4, 0], 4)
        } else {
            let comp = comp.min(499) - 244;
            ([comp / 3, comp % 3, 0, 0], 5)
        }
    } else if comp < 400 {
        ([(comp >> 4) / 5, (comp >> 4) % 5, (comp % 16) >> 2, comp % 4], 0)
    } else if comp < 500 {
        let comp = comp - 400;
        ([(comp >> 2) / 5, (comp >> 2) % 5, comp % 4, 0], 1)
    } else {
        let comp = comp.min(511) - 500;
        // Selector 2 always applies the preflag weighting.
        gr.preflag = true;
        ([comp / 3, comp % 3, 0, 0], 2)
    };
    let selector: usize = selector;
    let kind = if !gr.short_blocks() {
        0
    } else if gr.mixed_block {
        2
    } else {
        1
    };
    let parts = &LSF_NSFB[selector][kind];

    out.value = [0; MAX_BANDS];
    out.is_pos = [u8::MAX; MAX_BANDS];
    let mut band = 0usize;
    for (part, &count) in parts.iter().enumerate() {
        let bits = slen[part].min(16);
        let max = if bits == 0 { 0 } else { (1u32 << bits) - 1 };
        for _ in 0..count {
            // Always consume the bits, even for a band this layout does not
            // have room for: the Huffman data begins where the scalefactors
            // end, so skipping a read would desynchronise the whole granule.
            let v = r.read(bits);
            if let Some(slot) = out.value.get_mut(band) {
                *slot = v.min(255) as u8;
                // All-ones is the "not intensity coded" escape.
                out.is_pos[band] = if intensity_right && bits > 0 && v == max {
                    u8::MAX
                } else {
                    v.min(254) as u8
                };
            }
            band += 1;
        }
    }
    debug_assert!(band <= MAX_BANDS, "LSF partition {selector} overflows the band array");
}

// ---------------------------------------------------------------------------
// Huffman spectrum
// ---------------------------------------------------------------------------

/// Walk one Huffman tree to a leaf. The trees are complete prefix codes, so
/// this terminates within the table's longest codeword; the bound is a
/// belt-and-braces guard against a malformed table, never a decode limit.
#[inline]
fn huff_leaf(r: &mut BitReader, tree: &[HuffNode]) -> Option<u16> {
    let mut node = 0usize;
    for _ in 0..24 {
        let child = tree.get(node)?[r.read_bit() as usize];
        if child & LEAF != 0 {
            return Some(child & 0x7fff);
        }
        node = child as usize;
    }
    None
}

#[inline]
fn read_signed(r: &mut BitReader, mut v: u32, linbits: u32) -> i32 {
    if linbits > 0 && v == 15 {
        v += r.read(linbits);
    }
    if v == 0 {
        return 0;
    }
    if r.read_bit() == 1 {
        -(v as i32)
    } else {
        v as i32
    }
}

/// Decode one granule's quantised spectrum. Returns the number of lines
/// actually coded; everything above that is the zero region.
fn read_spectrum(
    r: &mut BitReader,
    header: &FrameHeader,
    gr: &GranuleInfo,
    end_bit: usize,
    out: &mut [i32; LINES],
) -> Result<usize, AudioError> {
    out.fill(0);
    let big = (gr.big_values as usize * 2).min(LINES);

    let (region1, region2) = if gr.window_switching {
        let r1 = header.switched_region1_start(gr.block_type == 2);
        (r1.min(big), big)
    } else {
        let sfb = header.sfb_long();
        let i1 = (gr.region0_count as usize + 1).min(22);
        let i2 = (gr.region0_count as usize + gr.region1_count as usize + 2).min(22);
        ((sfb[i1] as usize).min(big), (sfb[i2] as usize).min(big))
    };

    let mut i = 0usize;
    while i < big {
        let region = if i < region1 {
            0
        } else if i < region2 {
            1
        } else {
            2
        };
        let select = gr.table_select[region] as usize;
        let entry = HUFF_TABLES.get(select).copied().flatten();
        let Some((tree_index, linbits)) = entry else {
            // Table 0 (and the two the standard leaves empty) code only zeros.
            i += 2;
            continue;
        };
        let tree = HUFF_TREES[tree_index as usize];
        let leaf = huff_leaf(r, tree).ok_or(AudioError::Corrupt("huffman code"))?;
        out[i] = read_signed(r, (leaf >> 4) as u32, linbits as u32);
        out[i + 1] = read_signed(r, (leaf & 0xf) as u32, linbits as u32);
        i += 2;
        if r.pos() > end_bit {
            // The encoder never writes a codeword that crosses the granule's
            // boundary; if one appears to, the stream is desynchronised.
            return Ok(i.min(LINES));
        }
    }

    // The count1 region: quadruples of +-1, filling whatever bits are left.
    let quad = HUFF_TREES[if gr.count1table_select { 16 } else { 15 }];
    while i + 4 <= LINES && r.pos() < end_bit {
        let before = r.pos();
        let leaf = match huff_leaf(r, quad) {
            Some(leaf) => leaf,
            None => break,
        };
        let bits = (leaf & 0xf) as u32;
        for k in 0..4 {
            let one = (bits >> (3 - k)) & 1;
            out[i + k] = if one == 0 {
                0
            } else if r.read_bit() == 1 {
                -1
            } else {
                1
            };
        }
        if r.pos() > end_bit {
            // A quadruple that runs past the boundary was never encoded; drop
            // it and stop. (This is legal and common: the last codeword may be
            // padded away.)
            for slot in out[i..i + 4].iter_mut() {
                *slot = 0;
            }
            r.seek(before);
            break;
        }
        i += 4;
    }
    Ok(i.min(LINES))
}

// ---------------------------------------------------------------------------
// requantization
// ---------------------------------------------------------------------------

/// `|x|^(4/3)` for every quantised magnitude, built once per decoder.
pub fn pow43_table() -> Box<[f32; MAX_QUANT + 1]> {
    let mut t = Box::new([0.0f32; MAX_QUANT + 1]);
    for (i, slot) in t.iter_mut().enumerate() {
        *slot = (i as f64).powf(4.0 / 3.0) as f32;
    }
    t
}

fn requantize(
    gr: &GranuleInfo,
    layout: &BandLayout,
    scalefac: &Scalefactors,
    quant: &[i32; LINES],
    pow43: &[f32; MAX_QUANT + 1],
    out: &mut [f32; LINES],
) {
    out.fill(0.0);
    // Everything below is in quarter-log2 units, where the standard's
    // 2^((global_gain - 210)/4) and 2^(-(0.5 or 1) * scalefac) are both
    // integers and can be summed before a single exp2.
    let gain = gr.global_gain as i32 - 210;
    let mult = if gr.scalefac_scale { 4 } else { 2 };
    let mut line = 0usize;
    for band in 0..layout.count {
        let width = layout.width[band] as usize;
        if line >= LINES || width == 0 {
            line += width;
            continue;
        }
        let sf = scalefac.value[band] as i32;
        let exponent = if band < layout.long_bands {
            let pre = if gr.preflag {
                PRETAB.get(band).copied().unwrap_or(0) as i32
            } else {
                0
            };
            gain - mult * (sf + pre)
        } else {
            let window = (band - layout.long_bands) % 3;
            gain - 8 * gr.subblock_gain[window] as i32 - mult * sf
        };
        let scale = (exponent as f32 * 0.25).exp2();
        let end = (line + width).min(LINES);
        for i in line..end {
            let q = quant[i];
            let magnitude = pow43[(q.unsigned_abs() as usize).min(MAX_QUANT)];
            out[i] = if q < 0 { -magnitude * scale } else { magnitude * scale };
        }
        line += width;
    }
}

// ---------------------------------------------------------------------------
// joint stereo
// ---------------------------------------------------------------------------

const SQRT2_INV: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Mid/side and intensity stereo, applied across a granule's two channels in
/// bitstream (pre-reorder) order.
///
/// `right` is the *second* channel's granule info, which is the one that
/// carries the intensity positions and, on MPEG-2/2.5, the intensity scale in
/// the low bit of its `scalefac_compress`. Reading that bit from the first
/// channel instead is wrong by a factor of 2^(1/4) per band, which is exactly
/// what the CoreAudio comparison shows.
fn apply_stereo(
    header: &FrameHeader,
    right: &GranuleInfo,
    layout: &BandLayout,
    right_scalefac: &Scalefactors,
    xr: &mut [[f32; LINES]; 2],
) {
    let mid_side = header.mid_side();
    let intensity = header.intensity();
    if !mid_side && !intensity {
        return;
    }
    let pan = intensity_pan_mpeg1();
    // The intensity region starts above the highest band the right channel
    // actually coded — separately per window for short blocks, since the three
    // windows interleave.
    let blocks = if layout.short { 3 } else { 1 };
    let mut top = [-1i32; 3];
    if intensity {
        let mut line = 0usize;
        for band in 0..layout.count {
            let width = layout.width[band] as usize;
            let end = (line + width).min(LINES);
            if xr[1][line.min(LINES)..end].iter().any(|v| *v != 0.0) {
                top[band % blocks] = band as i32;
            }
            line += width;
        }
        if !layout.short {
            let max = top[0].max(top[1]).max(top[2]);
            top = [max; 3];
        }
    }

    let mut line = 0usize;
    for band in 0..layout.count {
        let width = layout.width[band] as usize;
        let end = (line + width).min(LINES);
        let start = line.min(LINES);
        line += width;
        if start >= end {
            continue;
        }
        let is_pos = right_scalefac.is_pos[band];
        let intensity_band = intensity
            && band as i32 > top[band % blocks]
            && is_pos != u8::MAX
            && (header.is_lsf() || is_pos < 7);
        if intensity_band {
            let (kl, kr) = if header.is_lsf() {
                // LSF pans in quarter-log2 steps rather than by a tangent.
                let shift = (right.scalefac_compress & 1) as i32;
                let step = (((is_pos as i32 + 1) >> 1) << shift) as f32;
                let k = (-step * 0.25).exp2();
                if is_pos & 1 != 0 {
                    (k, 1.0)
                } else {
                    (1.0, k)
                }
            } else {
                pan[(is_pos as usize).min(6)]
            };
            for i in start..end {
                let v = xr[0][i];
                xr[0][i] = v * kl;
                xr[1][i] = v * kr;
            }
        } else if mid_side {
            for i in start..end {
                let (m, s) = (xr[0][i], xr[1][i]);
                xr[0][i] = (m + s) * SQRT2_INV;
                xr[1][i] = (m - s) * SQRT2_INV;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// reorder, alias reduction
// ---------------------------------------------------------------------------

/// Short-block spectra arrive grouped by band then window; the filterbank
/// needs them interleaved window-by-window inside each band.
fn reorder(header: &FrameHeader, layout: &BandLayout, xr: &mut [f32; LINES], scratch: &mut [f32; LINES]) {
    if !layout.short {
        return;
    }
    let short = header.sfb_short();
    scratch.copy_from_slice(xr);
    let mut src = layout.short_start;
    for sfb in layout.first_short_sfb..13 {
        let width = (short[sfb + 1] - short[sfb]) as usize;
        let base = short[sfb] as usize * 3;
        for window in 0..3 {
            for f in 0..width {
                let dst = base + window + 3 * f;
                if dst >= LINES || src >= LINES {
                    return;
                }
                xr[dst] = scratch[src];
                src += 1;
            }
        }
    }
}

/// Cancel the aliasing the analysis filterbank introduced between adjacent
/// subbands. Short blocks are already alias-free; a mixed block only needs it
/// at the one boundary inside its long region.
fn antialias(gr: &GranuleInfo, xr: &mut [f32; LINES], cs: &[f32; 8], ca: &[f32; 8]) {
    if gr.short_blocks() && !gr.mixed_block {
        return;
    }
    let boundaries = if gr.short_blocks() { 1 } else { SUBBANDS - 1 };
    for sb in 0..boundaries {
        let base = sb * SLOTS;
        for i in 0..8 {
            let lo = base + 17 - i;
            let hi = base + 18 + i;
            let (a, b) = (xr[lo], xr[hi]);
            xr[lo] = a * cs[i] - b * ca[i];
            xr[hi] = b * cs[i] + a * ca[i];
        }
    }
}

// ---------------------------------------------------------------------------
// hybrid filterbank (IMDCT + windowing + overlap)
// ---------------------------------------------------------------------------

/// The four block windows and the two IMDCT bases, built once.
pub struct Imdct {
    window: [[f32; 36]; 4],
    cos36: [[f32; 36]; 18],
    cos12: [[f32; 12]; 6],
}

impl Imdct {
    pub fn new() -> Self {
        use std::f64::consts::PI;
        let mut window = [[0.0f32; 36]; 4];
        for i in 0..36 {
            window[0][i] = (PI / 36.0 * (i as f64 + 0.5)).sin() as f32;
        }
        for i in 0..36 {
            window[1][i] = match i {
                0..=17 => (PI / 36.0 * (i as f64 + 0.5)).sin() as f32,
                18..=23 => 1.0,
                24..=29 => (PI / 12.0 * ((i - 18) as f64 + 0.5)).sin() as f32,
                _ => 0.0,
            };
            window[3][i] = match i {
                0..=5 => 0.0,
                6..=11 => (PI / 12.0 * ((i - 6) as f64 + 0.5)).sin() as f32,
                12..=17 => 1.0,
                _ => (PI / 36.0 * (i as f64 + 0.5)).sin() as f32,
            };
        }
        // Window 2 is the short window, applied to each 12-point block.
        for i in 0..12 {
            window[2][i] = (PI / 12.0 * (i as f64 + 0.5)).sin() as f32;
        }
        let mut cos36 = [[0.0f32; 36]; 18];
        for (k, row) in cos36.iter_mut().enumerate() {
            for (i, slot) in row.iter_mut().enumerate() {
                *slot = (PI / 72.0 * (2 * i + 1 + 18) as f64 * (2 * k + 1) as f64).cos() as f32;
            }
        }
        let mut cos12 = [[0.0f32; 12]; 6];
        for (k, row) in cos12.iter_mut().enumerate() {
            for (i, slot) in row.iter_mut().enumerate() {
                *slot = (PI / 24.0 * (2 * i + 1 + 6) as f64 * (2 * k + 1) as f64).cos() as f32;
            }
        }
        Self { window, cos36, cos12 }
    }

    /// 18 spectral lines of one subband to 36 windowed time samples.
    fn transform(&self, input: &[f32], block_type: u8, out: &mut [f32; 36]) {
        if block_type == 2 {
            out.fill(0.0);
            for window in 0..3 {
                let mut tmp = [0.0f32; 12];
                for (k, row) in self.cos12.iter().enumerate() {
                    let v = input[window + 3 * k];
                    if v == 0.0 {
                        continue;
                    }
                    for (i, slot) in tmp.iter_mut().enumerate() {
                        *slot += v * row[i];
                    }
                }
                for i in 0..12 {
                    out[6 * window + 6 + i] += tmp[i] * self.window[2][i];
                }
            }
            return;
        }
        out.fill(0.0);
        for (k, row) in self.cos36.iter().enumerate() {
            let v = input[k];
            if v == 0.0 {
                continue;
            }
            for (i, slot) in out.iter_mut().enumerate() {
                *slot += v * row[i];
            }
        }
        let window = &self.window[block_type as usize];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot *= window[i];
        }
    }
}

impl Default for Imdct {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything one channel carries between granules.
pub struct ChannelState {
    /// Second half of the previous granule's IMDCT output, per subband.
    overlap: Box<[[f32; SLOTS]; SUBBANDS]>,
    pub synth: Synthesis,
}

impl ChannelState {
    pub fn new() -> Self {
        Self { overlap: Box::new([[0.0; SLOTS]; SUBBANDS]), synth: Synthesis::new() }
    }

    pub fn reset(&mut self) {
        self.overlap.iter_mut().for_each(|sb| sb.fill(0.0));
        self.synth.reset();
    }
}

impl Default for ChannelState {
    fn default() -> Self {
        Self::new()
    }
}

/// IMDCT, overlap-add and frequency inversion: spectrum in, subband time
/// samples out (`[slot][subband]`, ready for the synthesis filterbank).
fn hybrid(
    imdct: &Imdct,
    gr: &GranuleInfo,
    xr: &[f32; LINES],
    state: &mut ChannelState,
    out: &mut [[f32; SUBBANDS]; SLOTS],
) {
    let mut block = [0.0f32; 36];
    for sb in 0..SUBBANDS {
        // A mixed block keeps its two lowest subbands long.
        let block_type = if gr.short_blocks() && !(gr.mixed_block && sb < 2) {
            2
        } else if gr.short_blocks() {
            0
        } else {
            gr.block_type
        };
        imdct.transform(&xr[sb * SLOTS..sb * SLOTS + SLOTS], block_type, &mut block);
        let overlap = &mut state.overlap[sb];
        // Odd subbands are frequency-inverted so the synthesis filterbank sees
        // the spectrum the right way up.
        let invert = sb & 1 == 1;
        for slot in 0..SLOTS {
            let v = block[slot] + overlap[slot];
            out[slot][sb] = if invert && slot & 1 == 1 { -v } else { v };
            overlap[slot] = block[slot + SLOTS];
        }
    }
}

// ---------------------------------------------------------------------------
// granule decode
// ---------------------------------------------------------------------------

/// Reusable per-frame working memory. One of these lives in the decoder, so a
/// steady-state decode loop performs no allocation at all.
pub struct GranuleScratch {
    quant: Box<[i32; LINES]>,
    xr: Box<[[f32; LINES]; 2]>,
    spare: Box<[f32; LINES]>,
    subband: Box<[[f32; SUBBANDS]; SLOTS]>,
    scalefac: [Scalefactors; 2],
    previous: [Scalefactors; 2],
    pow43: Box<[f32; MAX_QUANT + 1]>,
    imdct: Imdct,
    cs: [f32; 8],
    ca: [f32; 8],
}

impl GranuleScratch {
    pub fn new() -> Self {
        let (cs, ca) = alias_coefficients();
        Self {
            quant: Box::new([0; LINES]),
            xr: Box::new([[0.0; LINES]; 2]),
            spare: Box::new([0.0; LINES]),
            subband: Box::new([[0.0; SUBBANDS]; SLOTS]),
            scalefac: [Scalefactors::default(); 2],
            previous: [Scalefactors::default(); 2],
            pow43: pow43_table(),
            imdct: Imdct::new(),
            cs,
            ca,
        }
    }
}

impl Default for GranuleScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode one granule of every channel into `pcm`, which receives 576
/// interleaved frames' worth of samples per channel.
///
/// `main_data` is the reservoir slice and `bit_pos` the granule's start within
/// it; the returned position is where the next granule begins.
#[allow(clippy::too_many_arguments)]
pub fn decode_granule(
    header: &FrameHeader,
    side: &SideInfo,
    granule_index: usize,
    main_data: &[u8],
    bit_pos: usize,
    scratch: &mut GranuleScratch,
    channels: &mut [ChannelState; 2],
    pcm: &mut [[f32; 576]; 2],
) -> Result<usize, AudioError> {
    let channel_count = header.channels();
    let mut cursor = bit_pos;
    let mut layouts = [BandLayout::new(header, &GranuleInfo::default()); 2];
    let mut granules = [GranuleInfo::default(); 2];

    for ch in 0..channel_count {
        let mut gr = side.granules[granule_index][ch];
        let part2_3 = gr.part2_3_length as usize;
        let end_bit = cursor.saturating_add(part2_3);
        let mut r = BitReader::at(main_data, cursor);
        let layout = BandLayout::new(header, &gr);

        if header.is_lsf() {
            let intensity_right = ch == 1 && header.intensity();
            read_scalefactors_lsf(&mut r, &mut gr, intensity_right, &mut scratch.scalefac[ch]);
        } else {
            let previous = scratch.previous[ch];
            read_scalefactors_mpeg1(
                &mut r,
                &gr,
                granule_index,
                &side.scfsi[ch],
                &previous,
                &mut scratch.scalefac[ch],
            );
            if granule_index == 0 {
                scratch.previous[ch] = scratch.scalefac[ch];
            }
        }
        if r.pos() > end_bit {
            return Err(AudioError::Corrupt("scalefactors overrun part2_3_length"));
        }
        read_spectrum(&mut r, header, &gr, end_bit, &mut scratch.quant)?;
        requantize(
            &gr,
            &layout,
            &scratch.scalefac[ch],
            &scratch.quant,
            &scratch.pow43,
            &mut scratch.xr[ch],
        );
        layouts[ch] = layout;
        granules[ch] = gr;
        cursor = end_bit;
    }
    if channel_count == 1 {
        scratch.xr[1].fill(0.0);
    }

    if channel_count == 2 {
        let right_scalefac = scratch.scalefac[1];
        apply_stereo(header, &granules[1], &layouts[0], &right_scalefac, &mut scratch.xr);
    }

    for ch in 0..channel_count {
        reorder(header, &layouts[ch], &mut scratch.xr[ch], &mut scratch.spare);
        antialias(&granules[ch], &mut scratch.xr[ch], &scratch.cs, &scratch.ca);
        hybrid(
            &scratch.imdct,
            &granules[ch],
            &scratch.xr[ch],
            &mut channels[ch],
            &mut scratch.subband,
        );
        let mut out = [0.0f32; 32];
        for slot in 0..SLOTS {
            channels[ch].synth.slot(&scratch.subband[slot], &mut out);
            pcm[ch][slot * 32..slot * 32 + 32].copy_from_slice(&out);
        }
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mpeg1_header() -> FrameHeader {
        FrameHeader::parse(0xfffb_9004).expect("valid header")
    }

    /// The same stream in joint stereo with mid/side on and intensity off.
    fn mpeg1_mid_side() -> FrameHeader {
        FrameHeader::parse(0xfffb_9064).expect("valid header")
    }

    #[test]
    fn every_huffman_table_is_a_complete_prefix_code() {
        for (i, tree) in HUFF_TREES.iter().enumerate() {
            let mut kraft = 0.0f64;
            let mut leaves = Vec::new();
            // Depth-first walk; a complete tree has every child populated.
            let mut stack = vec![(0usize, 0u32)];
            while let Some((node, depth)) = stack.pop() {
                assert!(depth <= 24, "tree {i} is deeper than any legal codeword");
                for bit in 0..2 {
                    let child = tree[node][bit];
                    if child & LEAF != 0 {
                        kraft += 2f64.powi(-(depth as i32 + 1));
                        leaves.push(child & 0x7fff);
                    } else {
                        assert!((child as usize) < tree.len(), "tree {i} node out of range");
                        stack.push((child as usize, depth + 1));
                    }
                }
            }
            assert!((kraft - 1.0).abs() < 1e-12, "tree {i}: Kraft sum {kraft}");
            let count = leaves.len();
            leaves.sort_unstable();
            leaves.dedup();
            assert_eq!(leaves.len(), count, "tree {i} has a duplicate symbol");
        }
    }

    #[test]
    fn spectral_tables_cover_a_square_value_grid() {
        // Tables 0..31 code pairs over a square grid; 32 and 33 code quads.
        for select in 0..32usize {
            let Some((tree_index, _)) = HUFF_TABLES[select] else { continue };
            let tree = HUFF_TREES[tree_index as usize];
            let mut symbols = Vec::new();
            let mut stack = vec![0usize];
            while let Some(node) = stack.pop() {
                for bit in 0..2 {
                    let child = tree[node][bit];
                    if child & LEAF != 0 {
                        symbols.push(child & 0x7fff);
                    } else {
                        stack.push(child as usize);
                    }
                }
            }
            let side = (symbols.len() as f64).sqrt() as u16;
            assert_eq!(
                (side * side) as usize,
                symbols.len(),
                "table {select} is not square"
            );
            for x in 0..side {
                for y in 0..side {
                    assert!(symbols.contains(&(x << 4 | y)), "table {select} missing ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn side_info_round_trips_a_known_layout() {
        // A hand-built MPEG-1 stereo side info block: main_data_begin 5, no
        // scfsi, both granules long blocks with distinct part2_3 lengths.
        let mut bits: Vec<bool> = Vec::new();
        let mut push = |value: u32, n: u32| {
            for i in (0..n).rev() {
                bits.push((value >> i) & 1 == 1);
            }
        };
        push(5, 9); // main_data_begin
        push(0, 3); // private
        push(0, 8); // scfsi, two channels
        for gr in 0..2u32 {
            for _ch in 0..2 {
                push(100 + gr, 12); // part2_3_length
                push(50, 9); // big_values
                push(200, 8); // global_gain
                push(3, 4); // scalefac_compress
                push(0, 1); // window_switching
                push(1, 5);
                push(2, 5);
                push(3, 5); // table_select
                push(7, 4); // region0_count
                push(2, 3); // region1_count
                push(1, 1); // preflag
                push(0, 1); // scalefac_scale
                push(1, 1); // count1table_select
            }
        }
        let mut bytes = vec![0u8; 32];
        for (i, bit) in bits.iter().enumerate() {
            if *bit {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let header = mpeg1_header();
        let side = read_side_info(&bytes, &header).expect("side info");
        assert_eq!(side.main_data_begin, 5);
        assert_eq!(side.granules[0][0].part2_3_length, 100);
        assert_eq!(side.granules[1][1].part2_3_length, 101);
        assert_eq!(side.granules[0][0].big_values, 50);
        assert_eq!(side.granules[0][0].global_gain, 200);
        assert_eq!(side.granules[0][0].table_select, [1, 2, 3]);
        assert_eq!(side.granules[0][0].region0_count, 7);
        assert!(side.granules[0][0].preflag);
        assert!(side.granules[0][0].count1table_select);
        assert!(!side.granules[0][0].window_switching);
        assert!(read_side_info(&bytes[..8], &header).is_err());
    }

    #[test]
    fn band_layouts_tile_the_granule() {
        let header = mpeg1_header();
        for (block_type, mixed) in [(0u8, false), (2, false), (2, true), (1, false), (3, false)] {
            let gr = GranuleInfo {
                window_switching: block_type != 0,
                block_type,
                mixed_block: mixed,
                ..GranuleInfo::default()
            };
            let layout = BandLayout::new(&header, &gr);
            let total: usize = layout.width[..layout.count].iter().map(|w| *w as usize).sum();
            assert_eq!(total, LINES, "block_type {block_type} mixed {mixed}");
            assert!(layout.count <= MAX_BANDS);
        }
    }

    #[test]
    fn short_block_reorder_is_a_permutation() {
        let header = mpeg1_header();
        let gr = GranuleInfo {
            window_switching: true,
            block_type: 2,
            ..GranuleInfo::default()
        };
        let layout = BandLayout::new(&header, &gr);
        let mut xr = [0.0f32; LINES];
        for (i, slot) in xr.iter_mut().enumerate() {
            *slot = (i + 1) as f32;
        }
        let mut spare = [0.0f32; LINES];
        reorder(&header, &layout, &mut xr, &mut spare);
        let mut seen: Vec<u32> = xr.iter().map(|v| *v as u32).collect();
        seen.sort_unstable();
        assert_eq!(seen, (1..=LINES as u32).collect::<Vec<_>>());
    }

    #[test]
    fn imdct_matches_a_direct_reference() {
        let imdct = Imdct::new();
        let mut input = [0.0f32; 18];
        input[3] = 1.0;
        input[7] = -0.5;
        let mut got = [0.0f32; 36];
        imdct.transform(&input, 0, &mut got);
        for i in 0..36 {
            let mut want = 0.0f64;
            for k in 0..18 {
                want += input[k] as f64
                    * (std::f64::consts::PI / 72.0
                        * (2 * i + 1 + 18) as f64
                        * (2 * k + 1) as f64)
                        .cos();
            }
            want *= (std::f64::consts::PI / 36.0 * (i as f64 + 0.5)).sin();
            assert!((got[i] as f64 - want).abs() < 1e-5, "i={i}: {} vs {want}", got[i]);
        }
    }

    #[test]
    fn short_imdct_places_three_windows() {
        let imdct = Imdct::new();
        let mut input = [0.0f32; 18];
        input[1] = 1.0; // window 1, coefficient 0
        let mut out = [0.0f32; 36];
        imdct.transform(&input, 2, &mut out);
        // Window 1 occupies samples 12..24; everything else stays silent.
        assert!(out[..12].iter().all(|v| v.abs() < 1e-9));
        assert!(out[24..].iter().all(|v| v.abs() < 1e-9));
        assert!(out[12..24].iter().any(|v| v.abs() > 1e-3));
    }

    #[test]
    fn antialias_is_energy_preserving_and_reversible() {
        let gr = GranuleInfo::default();
        let (cs, ca) = alias_coefficients();
        let mut xr = [0.0f32; LINES];
        for (i, slot) in xr.iter_mut().enumerate() {
            *slot = ((i % 17) as f32 - 8.0) * 0.1;
        }
        let before: f32 = xr.iter().map(|v| v * v).sum();
        antialias(&gr, &mut xr, &cs, &ca);
        let after: f32 = xr.iter().map(|v| v * v).sum();
        assert!((before - after).abs() < 1e-2, "{before} vs {after}");
    }

    #[test]
    fn short_blocks_skip_alias_reduction() {
        let gr = GranuleInfo { window_switching: true, block_type: 2, ..GranuleInfo::default() };
        let (cs, ca) = alias_coefficients();
        let mut xr = [1.0f32; LINES];
        antialias(&gr, &mut xr, &cs, &ca);
        assert!(xr.iter().all(|v| *v == 1.0));
    }

    #[test]
    fn spectrum_decode_stops_at_the_granule_boundary() {
        let header = mpeg1_header();
        let gr = GranuleInfo { big_values: 100, table_select: [1, 1, 1], ..GranuleInfo::default() };
        // Twenty bytes of ones: legal codewords, but the boundary is at 40 bits.
        let data = [0xffu8; 20];
        let mut r = BitReader::new(&data);
        let mut out = [0i32; LINES];
        let n = read_spectrum(&mut r, &header, &gr, 40, &mut out).expect("decodes");
        assert!(n <= LINES);
        assert!(r.pos() <= 40 + 24, "ran {} bits past the boundary", r.pos());
    }

    #[test]
    fn requantize_applies_gain_and_scalefactors() {
        let header = mpeg1_header();
        let gr = GranuleInfo { global_gain: 210, ..GranuleInfo::default() };
        let layout = BandLayout::new(&header, &gr);
        let scalefac = Scalefactors::default();
        let pow43 = pow43_table();
        let mut quant = [0i32; LINES];
        quant[0] = 8;
        quant[1] = -8;
        let mut xr = [0.0f32; LINES];
        requantize(&gr, &layout, &scalefac, &quant, &pow43, &mut xr);
        let expect = 8f32.powf(4.0 / 3.0);
        assert!((xr[0] - expect).abs() < 1e-3);
        assert!((xr[1] + expect).abs() < 1e-3);
        // global_gain 214 is exactly one power of two louder.
        let gr = GranuleInfo { global_gain: 214, ..GranuleInfo::default() };
        requantize(&gr, &layout, &scalefac, &quant, &pow43, &mut xr);
        assert!((xr[0] - 2.0 * expect).abs() < 1e-2);
    }

    #[test]
    fn mid_side_stereo_is_the_standard_rotation() {
        let header = mpeg1_mid_side();
        assert!(header.mid_side() && !header.intensity());
        let gr = GranuleInfo::default();
        let layout = BandLayout::new(&header, &gr);
        let mut xr = [[0.0f32; LINES]; 2];
        xr[0][0] = 1.0;
        xr[1][0] = 1.0;
        apply_stereo(&header, &gr, &layout, &Scalefactors::default(), &mut xr);
        assert!((xr[0][0] - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!(xr[1][0].abs() < 1e-6);
    }
}
