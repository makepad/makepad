//! The colour of a video stream: YCbCr matrix, range, primaries and transfer
//! (the four ITU-T H.273 properties every container and bitstream carries),
//! and the numbers the conversions use.
//!
//! Decoders report what a stream says ([`VideoFileInfo::color`]) and what
//! each frame's bytes are ([`DecodedVideoFrame::color`]: a decoder may hand
//! back another range than the stream's). Encoders take the colour to write
//! and tag ([`VideoFileEncoderOptions::color`]). The GPU pass that draws a
//! frame (`DrawNv12`, `mod.draw.VideoColor`) takes [`VideoColor::draw_uniforms`];
//! the CPU converters in [`crate::nv12`] use the same fixed-point coefficients,
//! so the two agree bit for bit on every SDR stream.
//!
//! A stream that leaves a property unspecified gets the usual guess: HD and up
//! is BT.709, SD BT.601 (625 lines at 576, 525 otherwise), limited range, SDR.
//!
//! HDR (PQ and HLG) goes into an SDR picture through one tone mapping, here
//! and in the shader alike: to display light in nits (HLG with its BT.2100
//! system gamma at a 1000 nit display), the BT.2408 reference white
//! ([`HDR_REFERENCE_WHITE_NITS`]) at SDR white, primaries to BT.709, then a
//! hue-preserving shoulder on the largest component that keeps everything
//! below [`TONE_KNEE`] unchanged and brings [`HDR_PEAK_NITS`] down to SDR white.
//!
//! [`VideoFileInfo::color`]: crate::VideoFileInfo::color
//! [`DecodedVideoFrame::color`]: crate::DecodedVideoFrame::color
//! [`VideoFileEncoderOptions::color`]: crate::VideoFileEncoderOptions::color

/// The YCbCr matrix (H.273 MatrixCoefficients).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoMatrix {
    #[default]
    Bt709,
    /// BT.601 / SMPTE 170M / BT.470BG (one matrix).
    Bt601,
    /// BT.2020 non-constant luminance.
    Bt2020,
}

/// The code range of the samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoRange {
    /// Video / TV range: Y 16..235, CbCr 16..240 (8 bit).
    #[default]
    Limited,
    /// Full / PC range: 0..255.
    Full,
}

/// The colour primaries (H.273 ColourPrimaries).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoPrimaries {
    #[default]
    Bt709,
    /// SMPTE 170M / BT.601 525 lines (and SMPTE 240M, the same primaries).
    Smpte170m,
    /// BT.470BG / BT.601 625 lines.
    Bt470bg,
    Bt2020,
}

/// The transfer characteristic (H.273 TransferCharacteristics), as far as it
/// changes what is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoTransfer {
    /// A display gamma curve (BT.709, BT.601, BT.2020 SDR, sRGB): the
    /// samples are shown as they are.
    #[default]
    Sdr,
    /// SMPTE ST 2084 (HDR10 and friends).
    Pq,
    /// ARIB STD-B67 hybrid log-gamma.
    Hlg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VideoColor {
    pub matrix: VideoMatrix,
    pub range: VideoRange,
    pub primaries: VideoPrimaries,
    pub transfer: VideoTransfer,
}

/// The BT.2408 HDR reference white, shown at SDR white before the shoulder.
pub const HDR_REFERENCE_WHITE_NITS: f32 = 203.0;
/// The brightest HDR light the tone mapping keeps apart from white; brighter
/// clips. (Mastering metadata is not read; 1000 nits is the common master and
/// the BT.2100 HLG reference display.)
pub const HDR_PEAK_NITS: f32 = 1000.0;
/// Linear SDR light below this (of SDR white) passes the tone mapping unchanged.
pub const TONE_KNEE: f32 = 0.75;
/// The BT.1886 display gamma SDR samples are decoded with when their
/// primaries are converted, and HDR light is encoded with for an SDR picture.
pub const SDR_DISPLAY_GAMMA: f32 = 2.4;

/// YCbCr bytes to RGB bytes in 8.8 fixed point, exactly as the CPU converters
/// and the shader compute it: with `c = Y - y_offset`, `d = Cb - 128`,
/// `e = Cr - 128`, component `i` is `clamp((k[i][0] c + k[i][1] d + k[i][2] e + 128) >> 8, 0, 255)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct YuvToRgbFixed {
    pub y_offset: i32,
    pub k: [[i32; 3]; 3],
}

/// RGB bytes to YCbCr bytes in 8.8 fixed point: `Y = ((ky . rgb) >> 8) + y_offset`,
/// and per pixel `(ku . rgb) >> 8`, `(kv . rgb) >> 8`, averaged over the 2x2
/// block and offset by 128 for the chroma.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbToYuvFixed {
    pub y_offset: i32,
    pub ky: [i32; 3],
    pub ku: [i32; 3],
    pub kv: [i32; 3],
}

/// What the GPU pass needs (`mod.draw.VideoColor`): the fixed-point matrix
/// rows over bytes, `yuv_rgb_*` (`.xyz` the Y, Cb, Cr factors in 1/256 units,
/// `.w` the constant term including the offsets and rounding), the transfer
/// (0 SDR, 1 SDR with other primaries, 2 PQ, 3 HLG) and the linear-light
/// primaries conversion to BT.709 as three rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoColorUniforms {
    pub yuv_r: [f32; 4],
    pub yuv_g: [f32; 4],
    pub yuv_b: [f32; 4],
    pub transfer: f32,
    pub gamut_r: [f32; 3],
    pub gamut_g: [f32; 3],
    pub gamut_b: [f32; 3],
}

impl VideoMatrix {
    /// (Kr, Kb)
    pub fn kr_kb(self) -> (f64, f64) {
        match self {
            VideoMatrix::Bt709 => (0.2126, 0.0722),
            VideoMatrix::Bt601 => (0.299, 0.114),
            VideoMatrix::Bt2020 => (0.2627, 0.0593),
        }
    }

    pub fn h273(self) -> u8 {
        match self {
            VideoMatrix::Bt709 => 1,
            VideoMatrix::Bt601 => 6,
            VideoMatrix::Bt2020 => 9,
        }
    }

    /// From an H.273 code point; `None` for unspecified or one we do not convert.
    pub fn from_h273(code: u32) -> Option<Self> {
        match code {
            1 => Some(VideoMatrix::Bt709),
            5 | 6 | 7 => Some(VideoMatrix::Bt601),
            // constant luminance (10) is drawn as non-constant: close, and rare
            9 | 10 => Some(VideoMatrix::Bt2020),
            _ => None,
        }
    }
}

impl VideoPrimaries {
    /// CIE xy of red, green, blue (all four have the D65 white).
    pub fn xy(self) -> [[f64; 2]; 3] {
        match self {
            VideoPrimaries::Bt709 => [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]],
            VideoPrimaries::Smpte170m => [[0.630, 0.340], [0.310, 0.595], [0.155, 0.070]],
            VideoPrimaries::Bt470bg => [[0.640, 0.330], [0.290, 0.600], [0.150, 0.060]],
            VideoPrimaries::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
        }
    }

    pub fn h273(self) -> u8 {
        match self {
            VideoPrimaries::Bt709 => 1,
            VideoPrimaries::Bt470bg => 5,
            VideoPrimaries::Smpte170m => 6,
            VideoPrimaries::Bt2020 => 9,
        }
    }

    pub fn from_h273(code: u32) -> Option<Self> {
        match code {
            1 => Some(VideoPrimaries::Bt709),
            5 => Some(VideoPrimaries::Bt470bg),
            6 | 7 => Some(VideoPrimaries::Smpte170m),
            9 => Some(VideoPrimaries::Bt2020),
            _ => None,
        }
    }
}

impl VideoTransfer {
    /// The code point written for this transfer under `primaries`: BT.2020
    /// SDR is tagged as BT.2020 (10 bit), the others as BT.709.
    pub fn h273(self, primaries: VideoPrimaries) -> u8 {
        match (self, primaries) {
            (VideoTransfer::Sdr, VideoPrimaries::Bt2020) => 14,
            (VideoTransfer::Sdr, VideoPrimaries::Smpte170m | VideoPrimaries::Bt470bg) => 6,
            (VideoTransfer::Sdr, VideoPrimaries::Bt709) => 1,
            (VideoTransfer::Pq, _) => 16,
            (VideoTransfer::Hlg, _) => 18,
        }
    }

    pub fn from_h273(code: u32) -> Option<Self> {
        match code {
            1 | 4 | 5 | 6 | 7 | 13 | 14 | 15 => Some(VideoTransfer::Sdr),
            16 => Some(VideoTransfer::Pq),
            18 => Some(VideoTransfer::Hlg),
            _ => None,
        }
    }
}

impl VideoColor {
    /// BT.709 limited range SDR: HD video, and what is written by default.
    pub const BT709: VideoColor = VideoColor { matrix: VideoMatrix::Bt709, range: VideoRange::Limited, primaries: VideoPrimaries::Bt709, transfer: VideoTransfer::Sdr };

    /// What a stream of this size is when it says nothing.
    pub fn guess(width: u32, height: u32) -> VideoColor {
        let _ = width;
        if height >= 720 {
            VideoColor::BT709
        } else {
            let primaries = if height == 576 { VideoPrimaries::Bt470bg } else { VideoPrimaries::Smpte170m };
            VideoColor { matrix: VideoMatrix::Bt601, range: VideoRange::Limited, primaries, transfer: VideoTransfer::Sdr }
        }
    }

    /// From H.273 code points (as `colr` boxes, VUI and most OS APIs carry
    /// them); each unspecified or unknown property falls back to
    /// [`VideoColor::guess`], except that the matrix follows known primaries
    /// before the size.
    pub fn from_h273(primaries: u32, transfer: u32, matrix: u32, full_range: bool, width: u32, height: u32) -> VideoColor {
        let guess = VideoColor::guess(width, height);
        let primaries = VideoPrimaries::from_h273(primaries);
        let matrix = VideoMatrix::from_h273(matrix).or(match primaries {
            Some(VideoPrimaries::Bt2020) => Some(VideoMatrix::Bt2020),
            Some(VideoPrimaries::Smpte170m | VideoPrimaries::Bt470bg) => Some(VideoMatrix::Bt601),
            Some(VideoPrimaries::Bt709) => Some(VideoMatrix::Bt709),
            None => None,
        });
        let matrix = matrix.unwrap_or(guess.matrix);
        let primaries = primaries.unwrap_or(match matrix {
            VideoMatrix::Bt2020 => VideoPrimaries::Bt2020,
            VideoMatrix::Bt601 => guess.primaries.max_sd(),
            VideoMatrix::Bt709 => VideoPrimaries::Bt709,
        });
        VideoColor {
            matrix,
            range: if full_range { VideoRange::Full } else { VideoRange::Limited },
            primaries,
            transfer: VideoTransfer::from_h273(transfer).unwrap_or(VideoTransfer::Sdr),
        }
    }

    /// (primaries, transfer, matrix, full range) as written in an `nclx`
    /// `colr` box or a VUI.
    pub fn h273(&self) -> (u8, u8, u8, bool) {
        (self.primaries.h273(), self.transfer.h273(self.primaries), self.matrix.h273(), self.range == VideoRange::Full)
    }

    pub fn is_hdr(&self) -> bool {
        self.transfer != VideoTransfer::Sdr
    }

    /// The same colour with another range (a decoder that converts it).
    pub fn with_range(self, range: VideoRange) -> VideoColor {
        VideoColor { range, ..self }
    }

    /// Scale of Y and of CbCr from bytes to (0..1, -0.5..0.5), and the Y offset.
    fn byte_scales(&self) -> (f64, f64, f64) {
        match self.range {
            VideoRange::Limited => (255.0 / 219.0, 255.0 / 224.0, 16.0),
            VideoRange::Full => (1.0, 1.0, 0.0),
        }
    }

    /// Normalised R'G'B' rows over (Y', Pb, Pr) of this matrix.
    fn ypbpr_to_rgb(&self) -> [[f64; 3]; 3] {
        let (kr, kb) = self.matrix.kr_kb();
        let kg = 1.0 - kr - kb;
        [[1.0, 0.0, 2.0 * (1.0 - kr)], [1.0, -2.0 * kb * (1.0 - kb) / kg, -2.0 * kr * (1.0 - kr) / kg], [1.0, 2.0 * (1.0 - kb), 0.0]]
    }

    pub fn yuv_to_rgb_fixed(&self) -> YuvToRgbFixed {
        let (ys, cs, y_offset) = self.byte_scales();
        let m = self.ypbpr_to_rgb();
        let fixed = |v: f64| (v * 256.0).round() as i32;
        let row = |r: [f64; 3]| [fixed(r[0] * ys), fixed(r[1] * cs), fixed(r[2] * cs)];
        YuvToRgbFixed { y_offset: y_offset as i32, k: [row(m[0]), row(m[1]), row(m[2])] }
    }

    pub fn rgb_to_yuv_fixed(&self) -> RgbToYuvFixed {
        let (kr, kb) = self.matrix.kr_kb();
        let kg = 1.0 - kr - kb;
        let (ys, cs, y_offset) = self.byte_scales();
        let fixed = |v: f64| (v * 256.0).round() as i32;
        let (ly, lc) = (1.0 / ys, 1.0 / cs);
        let pb = 0.5 / (1.0 - kb);
        let pr = 0.5 / (1.0 - kr);
        RgbToYuvFixed {
            y_offset: y_offset as i32,
            ky: [fixed(kr * ly), fixed(kg * ly), fixed(kb * ly)],
            ku: [fixed(-kr * pb * lc), fixed(-kg * pb * lc), fixed(0.5 * lc)],
            kv: [fixed(0.5 * lc), fixed(-kg * pr * lc), fixed(-kb * pr * lc)],
        }
    }

    /// R'G'B' (0..1) to Y'CbCr as rows of `(r, g, b, offset)` on 0..1
    /// values: `Y = dot(rgb, row0.rgb) + row0.w`, likewise Cb (row 1) and
    /// Cr (row 2), each a byte over 255, rounded where it is stored. The
    /// uniforms of a GPU pass that converts a picture to this colour's
    /// matrix and range ([`Self::encode_ycbcr`]'s maths).
    pub fn rgb_to_yuv_rows(&self) -> [[f32; 4]; 3] {
        let (kr, kb) = self.matrix.kr_kb();
        let kg = 1.0 - kr - kb;
        let (ys, cs, yo) = match self.range {
            VideoRange::Limited => (219.0, 224.0, 16.0),
            VideoRange::Full => (255.0, 255.0, 0.0),
        };
        let (ys, cs) = (ys / 255.0, cs / 255.0);
        let (pb, pr) = (0.5 / (1.0 - kb), 0.5 / (1.0 - kr));
        let row = |r: f64, g: f64, b: f64, w: f64| [r as f32, g as f32, b as f32, w as f32];
        [
            row(ys * kr, ys * kg, ys * kb, yo / 255.0),
            row(-cs * kr * pb, -cs * kg * pb, cs * 0.5, 128.0 / 255.0),
            row(cs * 0.5, -cs * kg * pr, -cs * kb * pr, 128.0 / 255.0),
        ]
    }

    /// RGB (linear light) of these primaries to BT.709 RGB, row major.
    pub fn gamut_to_bt709(&self) -> [[f64; 3]; 3] {
        let to_xyz = rgb_to_xyz(self.primaries);
        let from_xyz = invert3(rgb_to_xyz(VideoPrimaries::Bt709));
        mul3(from_xyz, to_xyz)
    }

    /// BT.709 RGB (linear light) to RGB of these primaries, row major.
    pub fn gamut_from_bt709(&self) -> [[f64; 3]; 3] {
        invert3(self.gamut_to_bt709())
    }

    /// The transfer selector of the shader: 0 SDR as is, 1 SDR with its
    /// primaries converted, 2 PQ, 3 HLG.
    pub fn transfer_mode(&self) -> u32 {
        match self.transfer {
            VideoTransfer::Sdr if self.primaries == VideoPrimaries::Bt709 => 0,
            VideoTransfer::Sdr => 1,
            VideoTransfer::Pq => 2,
            VideoTransfer::Hlg => 3,
        }
    }

    pub fn draw_uniforms(&self) -> VideoColorUniforms {
        let f = self.yuv_to_rgb_fixed();
        // The constant term folds the Y and chroma offsets and the +128 rounding.
        let row = |k: [i32; 3]| {
            let constant = -(k[0] * f.y_offset) - 128 * (k[1] + k[2]) + 128;
            [k[0] as f32, k[1] as f32, k[2] as f32, constant as f32]
        };
        let g = self.gamut_to_bt709();
        let g32 = |r: [f64; 3]| [r[0] as f32, r[1] as f32, r[2] as f32];
        VideoColorUniforms {
            yuv_r: row(f.k[0]),
            yuv_g: row(f.k[1]),
            yuv_b: row(f.k[2]),
            transfer: self.transfer_mode() as f32,
            gamut_r: g32(g[0]),
            gamut_g: g32(g[1]),
            gamut_b: g32(g[2]),
        }
    }

    /// One YCbCr byte triple to display RGB (0..1, BT.709 primaries, SDR)
    /// exactly as the shader computes it: the reference the decode checks
    /// compare drawn pixels with.
    pub fn reference_rgb(&self, y: u8, cb: u8, cr: u8) -> [f32; 3] {
        let u = self.draw_uniforms();
        let bytes = [y as f32, cb as f32, cr as f32];
        let fixed = |row: [f32; 4]| ((row[0] * bytes[0] + row[1] * bytes[1] + row[2] * bytes[2] + row[3]) / 256.0).floor().clamp(0.0, 255.0) / 255.0;
        let rgb = [fixed(u.yuv_r), fixed(u.yuv_g), fixed(u.yuv_b)];
        match self.transfer_mode() {
            0 => rgb,
            mode => {
                let linear = match mode {
                    1 => rgb.map(|v| v.powf(SDR_DISPLAY_GAMMA)),
                    2 => self.unquantised(bytes).map(pq_eotf_nits).map(|n| n / HDR_REFERENCE_WHITE_NITS),
                    _ => hlg_display_nits(self.unquantised(bytes)).map(|n| n / HDR_REFERENCE_WHITE_NITS),
                };
                let gm = |r: [f32; 3]| (r[0] * linear[0] + r[1] * linear[1] + r[2] * linear[2]).max(0.0);
                let mut out = [gm(u.gamut_r), gm(u.gamut_g), gm(u.gamut_b)];
                if mode >= 2 {
                    out = tone_map(out);
                }
                out.map(|v| v.clamp(0.0, 1.0).powf(1.0 / SDR_DISPLAY_GAMMA))
            }
        }
    }

    /// One R'G'B' signal triple (0..1, in this colour's primaries and
    /// transfer) to its YCbCr bytes, rounded: the reference the RGB-to-YUV
    /// passes are checked against, and how test pictures are made.
    pub fn encode_ycbcr(&self, rgb: [f32; 3]) -> [u8; 3] {
        let (kr, kb) = self.matrix.kr_kb();
        let (kr, kb) = (kr as f32, kb as f32);
        let y = kr * rgb[0] + (1.0 - kr - kb) * rgb[1] + kb * rgb[2];
        let pb = (rgb[2] - y) / (2.0 * (1.0 - kb));
        let pr = (rgb[0] - y) / (2.0 * (1.0 - kr));
        let (ys, cs, yo) = match self.range {
            VideoRange::Limited => (219.0, 224.0, 16.0),
            VideoRange::Full => (255.0, 255.0, 0.0),
        };
        let byte = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        [byte(yo + ys * y), byte(128.0 + cs * pb), byte(128.0 + cs * pr)]
    }

    /// A BT.709 SDR picture's RGB (0..1) in this colour's primaries, as the
    /// export's GPU step (`mod.draw.VideoColor.from_bt709`) computes it.
    pub fn reference_from_bt709(&self, rgb: [f32; 3]) -> [f32; 3] {
        let g = self.gamut_from_bt709();
        let linear = rgb.map(|v| v.clamp(0.0, 1.0).powf(SDR_DISPLAY_GAMMA));
        g.map(|r| (r[0] as f32 * linear[0] + r[1] as f32 * linear[1] + r[2] as f32 * linear[2]).clamp(0.0, 1.0).powf(1.0 / SDR_DISPLAY_GAMMA))
    }

    /// R'G'B' (0..1) before quantisation: the HDR paths' input.
    fn unquantised(&self, bytes: [f32; 3]) -> [f32; 3] {
        let u = self.draw_uniforms();
        let lin = |row: [f32; 4]| ((row[0] * bytes[0] + row[1] * bytes[1] + row[2] * bytes[2] + row[3] - 128.0) / (256.0 * 255.0)).clamp(0.0, 1.0);
        [lin(u.yuv_r), lin(u.yuv_g), lin(u.yuv_b)]
    }
}

impl VideoPrimaries {
    fn max_sd(self) -> VideoPrimaries {
        match self {
            VideoPrimaries::Bt470bg => VideoPrimaries::Bt470bg,
            _ => VideoPrimaries::Smpte170m,
        }
    }
}

/// SMPTE ST 2084 inverse: display light in nits to a PQ sample (0..1).
pub fn pq_inverse_eotf(nits: f32) -> f32 {
    let (m1, m2) = (0.1593017578125f32, 78.84375f32);
    let (c1, c2, c3) = (0.8359375f32, 18.8515625f32, 18.6875f32);
    let y = (nits / 10000.0).clamp(0.0, 1.0).powf(m1);
    ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
}

/// SMPTE ST 2084: a PQ sample (0..1) to display light in nits.
pub fn pq_eotf_nits(e: f32) -> f32 {
    let (m1, m2) = (0.1593017578125f32, 78.84375f32);
    let (c1, c2, c3) = (0.8359375f32, 18.8515625f32, 18.6875f32);
    let p = e.max(0.0).powf(1.0 / m2);
    10000.0 * ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
}

/// BT.2100 HLG: R'G'B' samples to display light in nits on the 1000 nit
/// reference display (inverse OETF, then the OOTF with system gamma 1.2).
pub fn hlg_display_nits(e: [f32; 3]) -> [f32; 3] {
    let (a, b, c) = (0.17883277f32, 0.28466892f32, 0.55991073f32);
    let scene = e.map(|v| if v <= 0.5 { v * v / 3.0 } else { (((v - c) / a).exp() + b) / 12.0 });
    let ys = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
    let gain = HDR_PEAK_NITS * ys.max(1e-6).powf(0.2);
    scene.map(|v| v * gain)
}

/// The SDR shoulder over linear light of SDR white (see the module).
pub fn tone_map(rgb: [f32; 3]) -> [f32; 3] {
    let m = rgb[0].max(rgb[1]).max(rgb[2]);
    if m <= TONE_KNEE {
        return rgb;
    }
    let peak = HDR_PEAK_NITS / HDR_REFERENCE_WHITE_NITS;
    let span = 1.0 - TONE_KNEE;
    let t = (m - TONE_KNEE) / span;
    let tw = (peak - TONE_KNEE) / span;
    let mapped = TONE_KNEE + span * (t * (1.0 + t / (tw * tw)) / (1.0 + t)).min(1.0);
    rgb.map(|v| v * mapped / m)
}

fn rgb_to_xyz(primaries: VideoPrimaries) -> [[f64; 3]; 3] {
    let xy = primaries.xy();
    let col = |p: [f64; 2]| [p[0] / p[1], 1.0, (1.0 - p[0] - p[1]) / p[1]];
    let (r, g, b) = (col(xy[0]), col(xy[1]), col(xy[2]));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let white = col([0.3127, 0.3290]);
    let inv = invert3(m);
    let s = [0, 1, 2].map(|i| inv[i][0] * white[0] + inv[i][1] * white[1] + inv[i][2] * white[2]);
    [0, 1, 2].map(|i| [m[i][0] * s[0], m[i][1] * s[1], m[i][2] * s[2]])
}

fn mul3(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j]))
}

fn invert3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c = |a: usize, b: usize, c: usize, d: usize| m[a][b] * m[c][d];
    [
        [(c(1, 1, 2, 2) - c(1, 2, 2, 1)) / det, (c(0, 2, 2, 1) - c(0, 1, 2, 2)) / det, (c(0, 1, 1, 2) - c(0, 2, 1, 1)) / det],
        [(c(1, 2, 2, 0) - c(1, 0, 2, 2)) / det, (c(0, 0, 2, 2) - c(0, 2, 2, 0)) / det, (c(0, 2, 1, 0) - c(0, 0, 1, 2)) / det],
        [(c(1, 0, 2, 1) - c(1, 1, 2, 0)) / det, (c(0, 1, 2, 0) - c(0, 0, 2, 1)) / det, (c(0, 0, 1, 1) - c(0, 1, 1, 0)) / det],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bt709_limited_is_the_legacy_integer_maths() {
        let f = VideoColor::BT709.yuv_to_rgb_fixed();
        assert_eq!(f, YuvToRgbFixed { y_offset: 16, k: [[298, 0, 459], [298, -55, -136], [298, 541, 0]] });
        let e = VideoColor::BT709.rgb_to_yuv_fixed();
        assert_eq!(e, RgbToYuvFixed { y_offset: 16, ky: [47, 157, 16], ku: [-26, -87, 112], kv: [112, -102, -10] });
    }

    #[test]
    fn bt601_limited_is_the_textbook_integer_maths() {
        let sd = VideoColor::guess(720, 480);
        assert_eq!(sd.matrix, VideoMatrix::Bt601);
        assert_eq!(sd.yuv_to_rgb_fixed().k, [[298, 0, 409], [298, -100, -208], [298, 516, 0]]);
        assert_eq!(sd.rgb_to_yuv_fixed().ky, [66, 129, 25]);
    }

    #[test]
    fn gamut_identity_and_white() {
        let id = VideoColor::BT709.gamut_to_bt709();
        for i in 0..3 {
            for j in 0..3 {
                assert!((id[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
        // BT.2020 -> BT.709 (the BT.2087 matrix) keeps white and matches its first row.
        let hdr = VideoColor { primaries: VideoPrimaries::Bt2020, ..VideoColor::BT709 }.gamut_to_bt709();
        assert!((hdr[0][0] - 1.6605).abs() < 1e-3 && (hdr[0][1] + 0.5876).abs() < 1e-3 && (hdr[0][2] + 0.0728).abs() < 1e-3);
        for row in hdr {
            assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn pq_and_hlg_reference_points() {
        assert!((pq_eotf_nits(0.5807) - 203.0).abs() < 1.0);
        assert!((pq_eotf_nits(0.7518) - 1000.0).abs() < 2.0);
        // HLG 75% is the BT.2408 reference white: 203 nits at the 1000 nit display.
        let w = hlg_display_nits([0.75; 3]);
        assert!((w[0] - 203.0).abs() < 2.0, "{w:?}");
        // the shoulder keeps the knee, maps the peak to white
        assert_eq!(tone_map([0.5, 0.2, 0.1]), [0.5, 0.2, 0.1]);
        let peak = tone_map([HDR_PEAK_NITS / HDR_REFERENCE_WHITE_NITS; 3]);
        assert!((peak[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn h273_round_trip_and_guesses() {
        for c in [
            VideoColor::BT709,
            VideoColor { matrix: VideoMatrix::Bt601, range: VideoRange::Full, primaries: VideoPrimaries::Smpte170m, transfer: VideoTransfer::Sdr },
            VideoColor { matrix: VideoMatrix::Bt2020, range: VideoRange::Limited, primaries: VideoPrimaries::Bt2020, transfer: VideoTransfer::Pq },
            VideoColor { matrix: VideoMatrix::Bt2020, range: VideoRange::Limited, primaries: VideoPrimaries::Bt2020, transfer: VideoTransfer::Hlg },
        ] {
            let (p, t, m, full) = c.h273();
            assert_eq!(VideoColor::from_h273(p as u32, t as u32, m as u32, full, 1920, 1080), c);
        }
        assert_eq!(VideoColor::from_h273(2, 2, 2, false, 1920, 1080), VideoColor::BT709);
        assert_eq!(VideoColor::from_h273(2, 2, 2, false, 720, 576).primaries, VideoPrimaries::Bt470bg);
        assert_eq!(VideoColor::from_h273(9, 16, 2, false, 3840, 2160).matrix, VideoMatrix::Bt2020);
    }

    #[test]
    fn reference_sdr_matches_the_cpu_converter() {
        // the CPU converters do matrix and range; primaries are the GPU's
        let sd = VideoColor { matrix: VideoMatrix::Bt601, ..VideoColor::BT709 };
        for color in [VideoColor::BT709, sd, VideoColor { range: VideoRange::Full, ..sd }, VideoColor { range: VideoRange::Full, ..VideoColor::BT709 }] {
            let mut nv12 = vec![0u8; 6];
            for (y, cb, cr) in [(16u8, 128u8, 128u8), (235, 128, 128), (81, 90, 240), (145, 54, 34), (41, 240, 110)] {
                nv12[..4].fill(y);
                nv12[4] = cb;
                nv12[5] = cr;
                let mut out = Vec::new();
                crate::nv12::nv12_to_rgb8(&nv12, 2, 2, color, &mut out);
                let want = color.reference_rgb(y, cb, cr).map(|v| (v * 255.0).round() as u8);
                assert_eq!(&out[..3], &want, "{color:?} {y} {cb} {cr}");
            }
        }
    }
}
