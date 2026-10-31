//! How a picture is cut into tiles for its encoders: plain arithmetic, on every target.

/// How a picture is cut into tiles: `cols` columns by `rows` bands (1 x 1: whole pictures).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePlan {
    pub cols: u32,
    pub rows: u32,
}

impl TilePlan {
    pub const WHOLE: TilePlan = TilePlan { cols: 1, rows: 1 };
    /// Macroblocks a second one stream may carry: H.264 level 5.2's, which hardware decoders
    /// (VideoToolbox among them) take; a faster stream is cut further.
    pub const MAX_MB_RATE: u64 = 2_073_600;
    /// Pixels a second one H.265 stream may carry for a viewer's hardware decoder (a decoder
    /// thread per stream, one picture at a time). Apple M4 VideoToolbox, 4:4:4 RExt, idle:
    /// 7680x2160 alone in 7.9 ms (2.1 Gpx/s, 94% of a picture at 120: no margin), the same in two
    /// bands decoded at once in 6.7 ms; 3840x2160 alone in 4.6 ms. So 8K wide at 120 is two
    /// bands, 3840x2160 at 120 one.
    pub const DECODE_PX_RATE: u64 = 1_300_000_000;
    /// The fewest H.265 bands of a `size` picture at `fps` that each fit [`Self::DECODE_PX_RATE`].
    pub fn decode_bands(size: [u32; 2], fps: u32) -> u32 {
        (size[0] as u64 * size[1] as u64 * fps as u64).div_ceil(Self::DECODE_PX_RATE).clamp(1, 16) as u32
    }
    pub fn tiles(&self) -> u32 {
        self.cols * self.rows
    }
    /// The plan for `size` pictures at `fps` as tiles (`tiles`; else whole pictures, interleaved or one stream) on `engines` engines whose streams take
    /// at most `max` pixels (None: not known, no limit). Tiles: the fewest tiles that are a
    /// multiple of the engines (every engine gets the same share) and that a viewer's decoder
    /// takes at `fps` (`MAX_MB_RATE` per stream), fewest columns first; never fewer than the size
    /// limit needs. Not `tiles`: the size limit's tiles only.
    ///
    /// H.265 (`hevc`): one stream as large as the encoder takes (no seams, one decoder), or more
    /// bands: as many as a viewer's decoder needs at `fps` ([`Self::decode_bands`], at least
    /// `bands`: what the viewers' measured decode asked for), and with `spread` at least a band
    /// per engine (the engines encode a picture at once). Bands are at least 64 rows.
    pub fn new(size: [u32; 2], fps: u32, tiles: bool, engines: u32, max: Option<[u32; 2]>, hevc: bool, spread: bool, bands: u32) -> TilePlan {
        let [mw, mh] = max.map_or([u32::MAX, u32::MAX], |[w, h]| [(w / 16 * 16).max(16), (h / 16 * 16).max(16)]);
        let min_cols = size[0].div_ceil(mw).max(1);
        let min_rows = size[1].div_ceil(mh).max(1);
        let least = TilePlan { cols: min_cols, rows: min_rows };
        if !tiles {
            return least;
        }
        if hevc {
            let want = Self::decode_bands(size, fps).max(bands).max(if spread { engines } else { 1 });
            let rows = want.min(size[1] / 64).max(least.rows);
            return TilePlan { cols: least.cols, rows };
        }
        if engines <= 1 {
            return least;
        }
        // tiles at least 64 pixels each way, each a stream the decoder takes at this rate
        let mb_rate = |c: u32, r: u32| (size[0] / c).div_ceil(16) as u64 * (size[1] / r + 16).div_ceil(16) as u64 * fps as u64;
        let fits = |c: u32, r: u32| c >= min_cols && r >= min_rows && size[0] / c >= 64 && size[1] / r >= 64 && mb_rate(c, r) <= Self::MAX_MB_RATE;
        for k in 1..=4 {
            let n = engines * k;
            if let Some(c) = (1..=n).find(|c| n % c == 0 && fits(*c, n / c)) {
                return TilePlan { cols: c, rows: n / c };
            }
        }
        least
    }
    /// The (start, length) of each of `parts` cuts of `len` pixels, every cut but the last a
    /// whole number of `unit`s (the last takes the rest).
    fn edges(len: u32, parts: u32, unit: u32) -> Vec<(u32, u32)> {
        let parts = parts.max(1);
        let step = (len / parts) / unit * unit;
        (0..parts).map(|i| if i + 1 == parts { (step * i, len - step * i) } else { (step * i, step) }).collect()
    }
    /// The columns of `len` pixels: 64-pixel multiples but the last (which takes the rest).
    pub fn columns(len: u32, parts: u32) -> Vec<(u32, u32)> {
        Self::edges(len, parts, 64)
    }
    /// The bands of `len` rows: whole macroblock rows but the last.
    pub fn bands(len: u32, parts: u32) -> Vec<(u32, u32)> {
        Self::edges(len, parts, 16)
    }
}
