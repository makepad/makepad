//! Six-face environment boxes (Quake II `env/`, Quake III `env/` + `skyParms`)
//! resampled into the ONE equirectangular image the renderer's `cube` sky
//! projection samples.
//!
//! The renderer has no cube sampler and six bindings would cost every sky
//! fragment, so [`makepad_render::model::SkyProjection::Cube`] is documented as
//! "sampled as its EQUIRECT twin". Building that twin is the importer's job,
//! and it is the one step in the whole sky path that can be silently WRONG:
//! a rotated, mirrored or flipped face still renders a sky.
//!
//! So the mapping here is not invented. It is the composition of two published
//! ones, in the direction that inverts both:
//!
//! 1. **Face → direction** is id's own `st_to_vec` table (`gl_warp.c` in Quake
//!    II, `tr_sky.c` in Quake III — the table and the `{0,2,1,3,4,5}`
//!    `skytexorder` indirection are identical in both). With `suf[6] =
//!    {"rt","bk","lf","ft","up","dn"}` it resolves to:
//!
//!    | face | axis (map space, Z-up) | `s` | `t` |
//!    |------|------------------------|-----|-----|
//!    | `rt` | +X | `-y/x`  | `z/x`  |
//!    | `bk` | +Y | `x/y`   | `z/y`  |
//!    | `lf` | −X | `y/-x`  | `z/-x` |
//!    | `ft` | −Y | `x/y`   | `z/-y` |
//!    | `up` | +Z | `-y/z`  | `-x/z` |
//!    | `dn` | −Z | `y/z`   | `-x/z` |
//!
//!    with `u = (s+1)/2` and `v = (1-t)/2` — `MakeSkyVec`'s
//!    `s=(s+1)*0.5; t=(t+1)*0.5; t=1-t`. Note `ft` is −Y and `bk` is +Y: the
//!    suffixes are not compass directions, and guessing them the "obvious"
//!    way turns the sky a quarter turn.
//!
//! 2. **Direction → equirect uv** is the renderer's own `Cube` branch
//!    (`SkyPart::direction_uv`): `u = atan2(d.x, d.z)/2π + 0.5`,
//!    `v = acos(d.y)/π`, in GLB space (metres, Y-up, −Z forward). This
//!    module inverts it exactly, so encoder and decoder cannot drift.
//!
//! The bridge between the two spaces is the same one every classic converter
//! uses for geometry: map `(x, y, z)` → GLB `(x, z, -y)`, hence GLB
//! `(X, Y, Z)` → map `(X, -Z, Y)`.

use crate::classic_import::encode_png_rgba;

/// id's `suf[]` order — the index into [`cube_to_equirect_rgba`]'s array is
/// this position, NOT the drawing order (`skytexorder` is already folded into
/// the table above).
pub const FACE_SUFFIXES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];

/// Face index of each suffix, for readers that prefer the name.
pub const FACE_RT: usize = 0;
pub const FACE_BK: usize = 1;
pub const FACE_LF: usize = 2;
pub const FACE_FT: usize = 3;
pub const FACE_UP: usize = 4;
pub const FACE_DN: usize = 5;

/// Default panorama size. 1024x512 is one texel per third of a degree at the
/// equator — finer than any 256² Quake II `env` face can justify, and small
/// enough that a map's sky is a ~200 KB PNG rather than a megabyte.
pub const EQUIRECT_W: u32 = 1024;
pub const EQUIRECT_H: u32 = 512;

/// One decoded cube face, 8-bit RGBA, row 0 at the TOP (what both the PCX and
/// the TGA/JPEG decoders in this crate produce).
#[derive(Clone, Debug)]
pub struct CubeFace {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl CubeFace {
    pub fn new(w: u32, h: u32, rgba: Vec<u8>) -> Option<Self> {
        if w == 0 || h == 0 || rgba.len() != (w as usize * h as usize * 4) {
            return None;
        }
        Some(Self { w, h, rgba })
    }

    /// Bilinear sample at `u`,`v` in 0..1, clamped half a texel inside the
    /// face. The engines clamp to `sky_min`/`sky_max` for exactly this reason:
    /// a cube face has no wrap, and interpolating past its edge drags in the
    /// opposite edge as a bright seam right across the horizon.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let fw = self.w as f32;
        let fh = self.h as f32;
        let x = (u * fw - 0.5).clamp(0.0, fw - 1.0);
        let y = (v * fh - 0.5).clamp(0.0, fh - 1.0);
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let x0 = x0 as u32;
        let y0 = y0 as u32;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let px = |cx: u32, cy: u32| -> [f32; 4] {
            let o = ((cy as usize * self.w as usize) + cx as usize) * 4;
            [
                self.rgba[o] as f32,
                self.rgba[o + 1] as f32,
                self.rgba[o + 2] as f32,
                self.rgba[o + 3] as f32,
            ]
        };
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        let mut out = [0.0f32; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bot - top) * fy;
        }
        out
    }
}

/// Which face a map-space direction hits, and where on it, in id's own
/// parameterisation (`s`,`t` in −1..1).
///
/// `dir` is map space (Z-up), need not be normalised, must not be zero.
pub fn face_of_direction(dir: [f32; 3]) -> (usize, f32, f32) {
    let [x, y, z] = dir;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax >= ay && ax >= az {
        if x > 0.0 {
            (FACE_RT, -y / x, z / x)
        } else {
            (FACE_LF, y / -x, z / -x)
        }
    } else if ay >= az {
        if y > 0.0 {
            (FACE_BK, x / y, z / y)
        } else {
            (FACE_FT, x / y, z / -y)
        }
    } else if z > 0.0 {
        (FACE_UP, -y / z, -x / z)
    } else {
        (FACE_DN, y / z, -x / z)
    }
}

/// `MakeSkyVec`'s texture-coordinate half: `s`,`t` in −1..1 → `u`,`v` in 0..1
/// with `v` counted DOWN the image, which is where the engines' `t = 1.0 - t`
/// puts it.
pub fn face_uv(s: f32, t: f32) -> (f32, f32) {
    ((s + 1.0) * 0.5, (1.0 - t) * 0.5)
}

/// Resample six faces into one equirectangular RGBA buffer of `w`x`h`.
///
/// Returns `None` when fewer than four faces are present — a sky missing a
/// wall is not a sky, and publishing a half-black panorama would look like a
/// rendering bug forever. A missing `up`/`dn` alone is tolerated (opaque
/// black): plenty of maps never show their nadir, and refusing the whole sky
/// over it would be worse than the black cap.
pub fn cube_to_equirect_rgba(faces: &[Option<CubeFace>; 6], w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return None;
    }
    if faces.iter().filter(|f| f.is_some()).count() < 4 {
        return None;
    }
    let mut out = vec![0u8; w as usize * h as usize * 4];
    for py in 0..h {
        // Renderer: v = acos(d.y)/PI. Invert at the texel centre.
        let v = (py as f32 + 0.5) / h as f32;
        let phi = v * std::f32::consts::PI;
        let dy = phi.cos();
        let r = phi.sin();
        for px in 0..w {
            // Renderer: u = atan2(d.x, d.z)/TAU + 0.5.
            let u = (px as f32 + 0.5) / w as f32;
            let theta = (u - 0.5) * std::f32::consts::TAU;
            let dx = r * theta.sin();
            let dz = r * theta.cos();
            // GLB (X, Y, Z) -> map (X, -Z, Y).
            let (face, s, t) = face_of_direction([dx, -dz, dy]);
            let o = ((py as usize * w as usize) + px as usize) * 4;
            let Some(img) = faces[face].as_ref() else {
                out[o + 3] = 255;
                continue;
            };
            let (fu, fv) = face_uv(s, t);
            let c = img.sample(fu, fv);
            for i in 0..3 {
                out[o + i] = c[i].round().clamp(0.0, 255.0) as u8;
            }
            // A sky is opaque however the source face was keyed: an alpha
            // hole would show the level's clear colour through the horizon.
            out[o + 3] = 255;
        }
    }
    Some(out)
}

/// The same, encoded as the PNG an [`crate::glb_nodes::ExtraNode::sky`] layer
/// wants.
pub fn cube_to_equirect_png(faces: &[Option<CubeFace>; 6]) -> Option<Vec<u8>> {
    let rgba = cube_to_equirect_rgba(faces, EQUIRECT_W, EQUIRECT_H)?;
    encode_png_rgba(&rgba, EQUIRECT_W, EQUIRECT_H).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer's own `Cube` mapping (`SkyPart::direction_uv`), repeated
    /// here so these tests assert against the CONSUMER's formula and not
    /// against this module's inverse of it. If the two ever disagree, the
    /// sample lands on the wrong face and every test below fails.
    fn renderer_uv(d: [f32; 3]) -> (f32, f32) {
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let d = [d[0] / len, d[1] / len, d[2] / len];
        let u = d[0].atan2(d[2]) / std::f32::consts::TAU + 0.5;
        let v = (d[1].clamp(-1.0, 1.0).acos() / std::f32::consts::PI).clamp(0.001, 0.999);
        (u.rem_euclid(1.0), v)
    }

    fn solid(color: [u8; 3]) -> CubeFace {
        let mut rgba = Vec::with_capacity(16 * 16 * 4);
        for _ in 0..16 * 16 {
            rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
        CubeFace::new(16, 16, rgba).unwrap()
    }

    /// One flat colour per face, in `suf[]` order.
    const COLORS: [[u8; 3]; 6] = [
        [255, 0, 0],   // rt
        [0, 255, 0],   // bk
        [0, 0, 255],   // lf
        [255, 255, 0], // ft
        [0, 255, 255],  // up
        [255, 0, 255], // dn
    ];

    fn six_solid_faces() -> [Option<CubeFace>; 6] {
        [
            Some(solid(COLORS[0])),
            Some(solid(COLORS[1])),
            Some(solid(COLORS[2])),
            Some(solid(COLORS[3])),
            Some(solid(COLORS[4])),
            Some(solid(COLORS[5])),
        ]
    }

    fn pixel(rgba: &[u8], w: u32, h: u32, u: f32, v: f32) -> [u8; 3] {
        let x = ((u * w as f32) as u32).min(w - 1);
        let y = ((v * h as f32) as u32).min(h - 1);
        let o = ((y as usize * w as usize) + x as usize) * 4;
        [rgba[o], rgba[o + 1], rgba[o + 2]]
    }

    /// The whole point: a viewer looking along a map axis must see the face
    /// the ENGINE draws there. `rt` is +X, `lf` −X, `bk` +Y, `ft` −Y — the
    /// suffixes are not compass directions, and this is the assertion that
    /// catches a quarter turn.
    #[test]
    fn every_axis_lands_on_the_face_the_engine_draws_there() {
        let faces = six_solid_faces();
        let rgba = cube_to_equirect_rgba(&faces, 256, 128).unwrap();
        // (map-space direction, expected face)
        let cases: [([f32; 3], usize); 6] = [
            ([1.0, 0.0, 0.0], FACE_RT),
            ([0.0, 1.0, 0.0], FACE_BK),
            ([-1.0, 0.0, 0.0], FACE_LF),
            ([0.0, -1.0, 0.0], FACE_FT),
            ([0.0, 0.0, 1.0], FACE_UP),
            ([0.0, 0.0, -1.0], FACE_DN),
        ];
        for (map_dir, face) in cases {
            // map (x, y, z) -> GLB (x, z, -y).
            let glb = [map_dir[0], map_dir[2], -map_dir[1]];
            let (u, v) = renderer_uv(glb);
            assert_eq!(
                pixel(&rgba, 256, 128, u, v),
                COLORS[face],
                "{} ({map_dir:?}) at equirect uv ({u:.3}, {v:.3})",
                FACE_SUFFIXES[face]
            );
        }
    }

    /// Inside one face, the image must not be mirrored or flipped. A gradient
    /// face pins both: `s` grows toward the face's own +u, `t` toward its −v.
    #[test]
    fn a_face_is_neither_mirrored_nor_flipped() {
        // rt: red ramps with u (left dark, right bright), green with v (top
        // dark, bottom bright).
        let (fw, fh) = (32u32, 32u32);
        let mut rgba = Vec::new();
        for y in 0..fh {
            for x in 0..fw {
                let r = (x * 255 / (fw - 1)) as u8;
                let g = (y * 255 / (fh - 1)) as u8;
                rgba.extend_from_slice(&[r, g, 0, 255]);
            }
        }
        let mut faces = six_solid_faces();
        faces[FACE_RT] = CubeFace::new(fw, fh, rgba);
        let (w, h) = (512u32, 256u32);
        let out = cube_to_equirect_rgba(&faces, w, h).unwrap();
        // Sample the rt face at four interior points of its own (s, t)
        // parameterisation and check the ramp reads back in the right order.
        let sample = |s: f32, t: f32| -> [u8; 3] {
            // Invert `face_of_direction` for rt: s = -y/x, t = z/x, x = 1.
            let map_dir = [1.0, -s, t];
            let glb = [map_dir[0], map_dir[2], -map_dir[1]];
            let (u, v) = renderer_uv(glb);
            pixel(&out, w, h, u, v)
        };
        let left = sample(-0.6, 0.0);
        let right = sample(0.6, 0.0);
        let top = sample(0.0, 0.6);
        let bottom = sample(0.0, -0.6);
        assert!(
            right[0] > left[0] + 60,
            "the face's +s edge must be its own right: left {left:?} right {right:?}"
        );
        assert!(
            bottom[1] > top[1] + 60,
            "the face's +t edge must be its own top: top {top:?} bottom {bottom:?}"
        );
    }

    /// The four side faces must meet edge to edge all the way round, with no
    /// face repeated and none skipped.
    #[test]
    fn the_horizon_visits_all_four_walls_in_order() {
        let faces = six_solid_faces();
        let (w, h) = (512u32, 256u32);
        let rgba = cube_to_equirect_rgba(&faces, w, h).unwrap();
        let mut seen: Vec<usize> = Vec::new();
        for x in 0..w {
            let c = pixel(&rgba, w, h, (x as f32 + 0.5) / w as f32, 0.5);
            let face = COLORS.iter().position(|k| *k == c).expect("a wall colour");
            if seen.last() != Some(&face) {
                seen.push(face);
            }
        }
        // The scan starts mid-face and wraps, so the first and last runs are
        // the same face split in two.
        if seen.len() > 1 && seen.first() == seen.last() {
            seen.pop();
        }
        assert_eq!(seen.len(), 4, "four walls, each once: {seen:?}");
        for f in [FACE_RT, FACE_BK, FACE_LF, FACE_FT] {
            assert!(seen.contains(&f), "{} missing: {seen:?}", FACE_SUFFIXES[f]);
        }
        // Turning one way must visit them in one consistent cycle.
        let cycle = [FACE_BK, FACE_LF, FACE_FT, FACE_RT];
        let start = cycle.iter().position(|f| *f == seen[0]).unwrap();
        for (i, f) in seen.iter().enumerate() {
            assert_eq!(*f, cycle[(start + i) % 4], "wall order wrong: {seen:?}");
        }
    }

    /// A sky is opaque, whatever the source faces said.
    #[test]
    fn the_panorama_is_opaque() {
        let mut faces = six_solid_faces();
        // A keyed face (alpha 0) must still come out solid.
        let mut keyed = solid([9, 9, 9]);
        for px in keyed.rgba.chunks_exact_mut(4) {
            px[3] = 0;
        }
        faces[FACE_UP] = Some(keyed);
        let rgba = cube_to_equirect_rgba(&faces, 64, 32).unwrap();
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn a_missing_wall_refuses_but_a_missing_cap_does_not() {
        let mut faces = six_solid_faces();
        faces[FACE_DN] = None;
        assert!(
            cube_to_equirect_rgba(&faces, 64, 32).is_some(),
            "a map that never shows its nadir still has a sky"
        );
        faces[FACE_UP] = None;
        faces[FACE_RT] = None;
        assert!(
            cube_to_equirect_rgba(&faces, 64, 32).is_none(),
            "a sky missing a wall is not a sky"
        );
    }

    #[test]
    fn the_png_twin_encodes_at_the_declared_size() {
        let faces = six_solid_faces();
        let png = cube_to_equirect_png(&faces).expect("png");
        let (rgba, w, h) = crate::classic_import::decode_png_stored(&png).unwrap();
        assert_eq!((w, h), (EQUIRECT_W, EQUIRECT_H));
        assert_eq!(rgba.len(), (w * h * 4) as usize);
    }
}
