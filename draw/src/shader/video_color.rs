//! Video colour on the GPU: `mod.draw.VideoColor` turns YCbCr bytes into
//! display RGB for any stream colour, the shader half of
//! `makepad_platform::video_file::VideoColor` (whose `draw_uniforms` are
//! these functions' arguments and whose `reference_rgb` is the same maths on
//! the CPU, so tests compare drawn pixels against it).
//!
//! - The matrix and range are fixed-point rows over bytes (`yuv_r/g/b`):
//!   exactly the CPU converters' integer maths, so an SDR frame of BT.709
//!   primaries is bit-exact with them.
//! - `transfer` 1: SDR in other primaries, decoded with the BT.1886 gamma,
//!   converted to BT.709 in linear light (`gamut_r/g/b`) and encoded again.
//! - `transfer` 2 / 3: PQ / HLG to display light (HLG on the 1000 nit
//!   reference display), BT.2408 reference white at SDR white, BT.2020 to
//!   BT.709, then the hue-preserving shoulder of `video_file::color::tone_map`
//!   (knee 0.75, 1000 nits to white) and the BT.1886 encode.
//!
//! - `from_bt709`: the other way for an export, BT.709 RGB to the
//!   output's primaries (`VideoColor::gamut_from_bt709`).
//!
//! The numbers below mirror `platform/video/src/color.rs`; change both.

use crate::makepad_platform::*;

script_mod! {
    use mod.pod.*
    use mod.math.*

    mod.draw.VideoColor = {
        // SMPTE ST 2084 to nits.
        fn pq_nits(e: vec3) -> vec3 {
            let p = pow(max(e, vec3(0.0, 0.0, 0.0)), vec3(1.0 / 78.84375, 1.0 / 78.84375, 1.0 / 78.84375))
            let n = max(p - vec3(0.8359375, 0.8359375, 0.8359375), vec3(0.0, 0.0, 0.0)) / (vec3(18.8515625, 18.8515625, 18.8515625) - 18.6875 * p)
            return 10000.0 * pow(n, vec3(1.0 / 0.1593017578125, 1.0 / 0.1593017578125, 1.0 / 0.1593017578125))
        }

        fn hlg_scene(v: float) -> float {
            if v <= 0.5 {
                return v * v / 3.0
            }
            return (exp((v - 0.55991073) / 0.17883277) + 0.28466892) / 12.0
        }

        // BT.2100 HLG to nits on the 1000 nit display (system gamma 1.2).
        fn hlg_nits(e: vec3) -> vec3 {
            let s = vec3(hlg_scene(e.x), hlg_scene(e.y), hlg_scene(e.z))
            let ys = 0.2627 * s.x + 0.6780 * s.y + 0.0593 * s.z
            return s * (1000.0 * pow(max(ys, 0.000001), 0.2))
        }

        // The SDR shoulder on the largest component (linear, of SDR white).
        fn tone_map(rgb: vec3) -> vec3 {
            let m = max(rgb.x, max(rgb.y, rgb.z))
            let knee = 0.75
            if m <= knee {
                return rgb
            }
            let span = 1.0 - knee
            let t = (m - knee) / span
            let tw = (1000.0 / 203.0 - knee) / span
            let mapped = knee + span * min(t * (1.0 + t / (tw * tw)) / (1.0 + t), 1.0)
            return rgb * (mapped / m)
        }

        fn encode_sdr(linear: vec3) -> vec3 {
            return pow(clamp(linear, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), vec3(1.0 / 2.4, 1.0 / 2.4, 1.0 / 2.4))
        }

        // The fixed-point rows over the bytes (Y, Cb, Cr in 0..255): the
        // classic integer maths, floored and clamped (0..1 out).
        fn rgb_bytes(b: vec3, r: vec4, g: vec4, bl: vec4) -> vec3 {
            let v = vec4(b.x, b.y, b.z, 1.0)
            let q = floor(vec3(dot(r, v), dot(g, v), dot(bl, v)) / 256.0)
            return clamp(q, vec3(0.0, 0.0, 0.0), vec3(255.0, 255.0, 255.0)) / 255.0
        }

        // A BT.709 SDR picture's RGB (0..1) in other primaries: decoded with
        // the BT.1886 gamma, `rows` (`VideoColor::gamut_from_bt709`) in
        // linear light, encoded again. What an export in another colour is
        // converted with before its YCbCr.
        from_bt709: fn(rgb: vec3, gr: vec3, gg: vec3, gb: vec3) -> vec3 {
            let linear = pow(clamp(rgb, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), vec3(2.4, 2.4, 2.4))
            return encode_sdr(vec3(dot(gr, linear), dot(gg, linear), dot(gb, linear)))
        }

        // Display RGB (BT.709 SDR, 0..1) of one YCbCr byte triple.
        display_rgb: fn(b: vec3, r: vec4, g: vec4, bl: vec4, transfer: float, gr: vec3, gg: vec3, gb: vec3) -> vec3 {
            if transfer < 0.5 {
                return rgb_bytes(b, r, g, bl)
            }
            let mut linear = vec3(0.0, 0.0, 0.0)
            if transfer < 1.5 {
                linear = pow(rgb_bytes(b, r, g, bl), vec3(2.4, 2.4, 2.4))
            } else {
                // unquantised R'G'B': the rows without the +128 rounding
                let v = vec4(b.x, b.y, b.z, 1.0)
                let e = clamp((vec3(dot(r, v), dot(g, v), dot(bl, v)) - vec3(128.0, 128.0, 128.0)) / (256.0 * 255.0), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
                if transfer < 2.5 {
                    linear = pq_nits(e) / 203.0
                } else {
                    linear = hlg_nits(e) / 203.0
                }
            }
            let mut out = max(vec3(dot(gr, linear), dot(gg, linear), dot(gb, linear)), vec3(0.0, 0.0, 0.0))
            if transfer > 1.5 {
                out = tone_map(out)
            }
            return encode_sdr(out)
        }
    }
}
