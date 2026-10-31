//! NV12 pictures on the GPU: [`DrawNv12`] draws a tight NV12 frame (the Y
//! plane, then the interleaved UV rows, in one byte texture made with
//! `Texture::new_nv12`) as display RGB, filtered bilinearly between the four
//! converted pixels around each sample, optionally mixed with a second
//! frame of the same size and colour (a frame blend).
//!
//! A decoder's own GPU picture comes as two plane textures instead
//! ([`DrawNv12::set_planes`]: luma R8, interleaved chroma RG8 at half size, imported
//! over the picture's memory with `Cx::import_picture_plane`), drawn the same way.
//!
//! The conversion is the frame's own colour (`VideoColor`: matrix, range,
//! primaries, transfer; see `mod.draw.VideoColor`). Each byte is read at its
//! texel centre and recovered exactly, the 2x2 chroma block's nearest sample
//! is used, and an SDR frame of BT.709 primaries goes through the CPU
//! converters' integer maths, so it is bit-exact with them at its own size.

use crate::{makepad_platform::video_file::VideoColor, makepad_platform::*, shader::draw_quad::DrawQuad};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawNv12 = mod.std.set_type_default() do #(DrawNv12::script_shader(vm)){
        ..mod.draw.DrawQuad
        nv12: texture_2d(float)
        // A frame blend's second frame (the same size), mixed in by `blend`.
        nv12_next: texture_2d(float)

        // The texel centres (uv) of pixel `p`'s Y byte (xy) and of its 2x2
        // chroma block's U byte (zw); V is one texel right of U.
        nv12_taps: fn(p: vec2) -> vec4 {
            let rows = self.nv12_size.y
            let w = self.nv12_size.x
            let h = floor(rows / 1.5 + 0.5)
            let px = clamp(p.x, 0.0, w - 1.0)
            let py = clamp(p.y, 0.0, h - 1.0)
            let cu = floor(px * 0.5) * 2.0
            let cv = h + floor(py * 0.5)
            return vec4((px + 0.5) / w, (py + 0.5) / rows, (cu + 0.5) / w, (cv + 0.5) / rows)
        }

        // The Y, Cb, Cr bytes (0..255) of pixel `p` of a planar picture: luma in `nv12`,
        // chroma in `nv12_next` (`nv12_size` is the picture's size then).
        planar_bytes: fn(p: vec2) -> vec3 {
            let w = self.nv12_size.x
            let h = self.nv12_size.y
            let px = clamp(p.x, 0.0, w - 1.0)
            let py = clamp(p.y, 0.0, h - 1.0)
            // 4:2:0: the chroma plane is half size, a 2x2 block per sample; 4:4:4: a sample per pixel
            let sub = mix(0.5, 1.0, self.chroma_full)
            // the chroma plane's size: ceil(w / 2) x ceil(h / 2), or w x h
            let cw = floor(w * sub + 0.5)
            let ch = floor(h * sub + 0.5)
            let yb = floor(self.nv12.sample(vec2((px + 0.5) / w, (py + 0.5) / h)).x * 255.0 + 0.5)
            let c = self.nv12_next.sample(vec2((floor(px * sub) + 0.5) / cw, (floor(py * sub) + 0.5) / ch))
            return vec3(yb, floor(c.x * 255.0 + 0.5), floor(c.y * 255.0 + 0.5))
        }

        // The Y, Cb, Cr bytes (0..255) of pixel `p`.
        nv12_bytes: fn(p: vec2) -> vec3 {
            if self.planar > 0.5 {
                return self.planar_bytes(p)
            }
            let t = self.nv12_taps(p)
            let du = 1.0 / self.nv12_size.x
            let yb = floor(self.nv12.sample(t.xy).x * 255.0 + 0.5)
            let ub = floor(self.nv12.sample(t.zw).x * 255.0 + 0.5)
            let vb = floor(self.nv12.sample(vec2(t.z + du, t.w)).x * 255.0 + 0.5)
            return vec3(yb, ub, vb)
        }

        nv12_next_bytes: fn(p: vec2) -> vec3 {
            let t = self.nv12_taps(p)
            let du = 1.0 / self.nv12_size.x
            let yb = floor(self.nv12_next.sample(t.xy).x * 255.0 + 0.5)
            let ub = floor(self.nv12_next.sample(t.zw).x * 255.0 + 0.5)
            let vb = floor(self.nv12_next.sample(vec2(t.z + du, t.w)).x * 255.0 + 0.5)
            return vec3(yb, ub, vb)
        }

        nv12_color: fn(b: vec3) -> vec3 {
            return draw.VideoColor.display_rgb(b, self.yuv_r, self.yuv_g, self.yuv_b, self.transfer, self.gamut_r, self.gamut_g, self.gamut_b)
        }

        nv12_rgb: fn(p: vec2) -> vec3 {
            return self.nv12_color(self.nv12_bytes(p))
        }

        nv12_next_rgb: fn(p: vec2) -> vec3 {
            return self.nv12_color(self.nv12_next_bytes(p))
        }

        // Catmull-Rom weights of the source pixels at -1, 0, 1, 2 from the sample (`t` its
        // fraction): a sharper scale than the bilinear blend (text keeps its edges).
        cubic_weights: fn(t: float) -> vec4 {
            let t2 = t * t
            let t3 = t2 * t
            return vec4(-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2)
        }

        cubic_row: fn(p: vec2, w: vec4) -> vec3 {
            return self.nv12_rgb(p + vec2(-1.0, 0.0)) * w.x + self.nv12_rgb(p) * w.y + self.nv12_rgb(p + vec2(1.0, 0.0)) * w.z + self.nv12_rgb(p + vec2(2.0, 0.0)) * w.w
        }

        pixel: fn() {
            // The source pixel centre under this output pixel.
            let s = self.src.xy + self.pos * self.src.zw - vec2(0.5, 0.5)
            let f = s - floor(s)
            let p = floor(s)
            if self.sharp > 1.5 {
                // whole device-pixel blocks of each source pixel: the pixel as it is
                return vec4(self.nv12_rgb(floor(s + vec2(0.5, 0.5))), 1.0) * self.opacity
            }
            if self.sharp > 0.5 {
                let wx = self.cubic_weights(f.x)
                let wy = self.cubic_weights(f.y)
                let c = self.cubic_row(p + vec2(0.0, -1.0), wx) * wy.x + self.cubic_row(p, wx) * wy.y + self.cubic_row(p + vec2(0.0, 1.0), wx) * wy.z + self.cubic_row(p + vec2(0.0, 2.0), wx) * wy.w
                return vec4(clamp(c, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), 1.0) * self.opacity
            }
            let top = mix(self.nv12_rgb(p), self.nv12_rgb(p + vec2(1.0, 0.0)), f.x)
            let bottom = mix(self.nv12_rgb(p + vec2(0.0, 1.0)), self.nv12_rgb(p + vec2(1.0, 1.0)), f.x)
            let rgb = mix(top, bottom, f.y)
            if self.blend > 0.0 {
                let top2 = mix(self.nv12_next_rgb(p), self.nv12_next_rgb(p + vec2(1.0, 0.0)), f.x)
                let bottom2 = mix(self.nv12_next_rgb(p + vec2(0.0, 1.0)), self.nv12_next_rgb(p + vec2(1.0, 1.0)), f.x)
                return vec4(mix(rgb, mix(top2, bottom2, f.y), self.blend), 1.0) * self.opacity
            }
            return vec4(rgb, 1.0) * self.opacity
        }
    }
}

/// An NV12 frame drawn as RGB (see the module). `src` is the source pixel
/// rectangle drawn into the quad (origin, size), `nv12_size` the byte
/// texture's size ([`DrawNv12::set_nv12`] sets both textures and it).
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawNv12 {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub src: Vec4f,
    #[live]
    pub nv12_size: Vec2f,
    #[live(1.0)]
    pub opacity: f32,
    /// How much of the second frame: 0 = none.
    #[live]
    pub blend: f32,
    /// 1: the picture is two plane textures ([`DrawNv12::set_planes`]).
    #[live]
    pub planar: f32,
    /// 1: a sharper (Catmull-Rom bicubic) scale instead of the bilinear one; 2: nearest (each
    /// source pixel a whole block of output pixels: 1:1, 2x, ...); no frame blend for either.
    #[live]
    pub sharp: f32,
    /// 1: the planar picture's chroma is 4:4:4 (full size).
    #[live]
    pub chroma_full: f32,
    /// The frame's colour ([`VideoColor::draw_uniforms`]), set by [`DrawNv12::set_nv12`].
    #[live]
    pub yuv_r: Vec4f,
    #[live]
    pub yuv_g: Vec4f,
    #[live]
    pub yuv_b: Vec4f,
    #[live]
    pub transfer: f32,
    #[live]
    pub gamut_r: Vec3f,
    #[live]
    pub gamut_g: Vec3f,
    #[live]
    pub gamut_b: Vec3f,
}

impl DrawNv12 {
    /// The frame `width` x `height` in `texture` (from `Texture::new_nv12`)
    /// in `color`, and a second frame of that size and colour mixed in by
    /// `next.1`. `src` is set to the whole frame.
    pub fn set_nv12(&mut self, texture: &Texture, width: u32, height: u32, color: VideoColor, next: Option<(&Texture, f32)>) {
        self.set_color(color);
        self.draw_vars.set_texture(0, texture);
        self.draw_vars.set_texture(1, next.map_or(texture, |(next, _)| next));
        self.blend = next.map_or(0.0, |(_, blend)| blend);
        self.planar = 0.0;
        self.nv12_size = vec2(width as f32, Texture::nv12_rows(height) as f32);
        self.src = vec4(0.0, 0.0, width as f32, height as f32);
    }

    /// The frame `width` x `height` as two plane textures (`Cx::import_picture_plane`):
    /// `luma` (R8, full size) and `chroma` (RG8, interleaved Cb Cr: half size, or full size
    /// when `full_chroma`: 4:4:4), in `color`. No frame blend. `src` is set to the whole frame.
    pub fn set_planes(&mut self, luma: &Texture, chroma: &Texture, width: u32, height: u32, color: VideoColor, full_chroma: bool) {
        self.chroma_full = full_chroma as u8 as f32;
        self.set_color(color);
        self.draw_vars.set_texture(0, luma);
        self.draw_vars.set_texture(1, chroma);
        self.blend = 0.0;
        self.planar = 1.0;
        self.nv12_size = vec2(width as f32, height as f32);
        self.src = vec4(0.0, 0.0, width as f32, height as f32);
    }

    fn set_color(&mut self, color: VideoColor) {
        let u = color.draw_uniforms();
        self.yuv_r = Vec4f { x: u.yuv_r[0], y: u.yuv_r[1], z: u.yuv_r[2], w: u.yuv_r[3] };
        self.yuv_g = Vec4f { x: u.yuv_g[0], y: u.yuv_g[1], z: u.yuv_g[2], w: u.yuv_g[3] };
        self.yuv_b = Vec4f { x: u.yuv_b[0], y: u.yuv_b[1], z: u.yuv_b[2], w: u.yuv_b[3] };
        self.transfer = u.transfer;
        self.gamut_r = Vec3f { x: u.gamut_r[0], y: u.gamut_r[1], z: u.gamut_r[2] };
        self.gamut_g = Vec3f { x: u.gamut_g[0], y: u.gamut_g[1], z: u.gamut_g[2] };
        self.gamut_b = Vec3f { x: u.gamut_b[0], y: u.gamut_b[1], z: u.gamut_b[2] };
    }
}

#[cfg(test)]
mod tests {
    use crate::makepad_platform::makepad_script::script_eval;
    use crate::makepad_platform::*;

    /// The colour conversion (`mod.draw.VideoColor`, every transfer) compiles
    /// on every backend the shader compiler writes.
    #[test]
    fn draw_nv12_compiles_on_every_backend() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for (backend, value) in [
                ("metal", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawNv12, "metal", false)})),
                ("hlsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawNv12, "hlsl", false)})),
                ("glsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawNv12, "glsl", false)})),
                ("wgsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.DrawNv12, "wgsl", false)})),
            ] {
                let text = vm.heap().string_with(value, |_heap, text| text.to_string()).expect("the compiler answers with source");
                assert!(!text.starts_with("ERRORS"), "{backend} did not compile: {text}");
                for name in ["display_rgb", "rgb_bytes", "pq_nits", "hlg_nits", "tone_map", "encode_sdr"] {
                    assert!(text.contains(&format!("{name}(")), "{backend}: {name} is missing from {text}");
                }
            }
        });
    }
}
