//! NV12 pictures packed by the GPU for a hardware encoder: an image's luma and chroma drawn into
//! a BGRA8 texture whose bytes ARE the NV12 planes (`makepad_video`'s `nv12_picture` layout):
//! texture rows 0..h four luma samples per texel ([`DrawNv12Luma`], memory order B G R A =
//! samples 2 1 0 3 of the draw's output), rows h..3h/2 U0 V0 U1 V1 per texel, 4:2:0 2x2
//! averages ([`DrawNv12Chroma`]); for 4:4:4 (`full`) w/4 x 3h, each chroma row the U V pairs of
//! every pixel of one source row over two texture rows. The encoder copies the texture's rows into
//! its picture planes on the GPU (no CPU pass). Colour: BT.709 limited range.

use crate::{makepad_platform::*, shader::draw_quad::DrawQuad};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // Four luma samples of row j, columns 4i..4i+3 of the source rect, in memory order B G R A.
    mod.draw.DrawNv12Luma = mod.std.set_type_default() do #(DrawNv12Luma::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        source: vec4(0.0, 0.0, 1.0, 1.0)
        extent: vec2(1.0, 1.0)
        pixel: fn() {
            let t = floor(self.pos * self.rect_size)
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            let row = self.source.y + t.y + 0.5
            let x = self.source.x + t.x * 4.0 + 0.5
            let w = vec3(0.2126, 0.7152, 0.0722)
            let y0 = (16.0 + 219.0 * dot(self.image.sample(vec2(x, row) * inv).xyz, w)) / 255.0
            let y1 = (16.0 + 219.0 * dot(self.image.sample(vec2(x + 1.0, row) * inv).xyz, w)) / 255.0
            let y2 = (16.0 + 219.0 * dot(self.image.sample(vec2(x + 2.0, row) * inv).xyz, w)) / 255.0
            let y3 = (16.0 + 219.0 * dot(self.image.sample(vec2(x + 3.0, row) * inv).xyz, w)) / 255.0
            return vec4(y2, y1, y0, y3)
        }
    }

    // Two chroma samples, memory order U0 V0 U1 V1: 4:2:0 each the average of a 2x2 block (one
    // bilinear tap at its centre); 4:4:4 (`full`) two neighbouring pixels of one row as they
    // are, a source row over two texture rows (its left half, then its right half).
    mod.draw.DrawNv12Chroma = mod.std.set_type_default() do #(DrawNv12Chroma::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        source: vec4(0.0, 0.0, 1.0, 1.0)
        extent: vec2(1.0, 1.0)
        full: 0.0
        pixel: fn() {
            let t = floor(self.pos * self.rect_size)
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            // which half of its source row this texture row holds (Splash shaders have no `mod`)
            let half = t.y - floor(t.y * 0.5) * 2.0
            let full_p = vec2(self.source.x + half * self.rect_size.x * 2.0 + t.x * 2.0 + 0.5, self.source.y + floor(t.y * 0.5) + 0.5)
            let sub_p = vec2(self.source.x + t.x * 4.0 + 1.0, self.source.y + t.y * 2.0 + 1.0)
            let p0 = mix(sub_p, full_p, self.full)
            let step = mix(2.0, 1.0, self.full)
            let c0 = self.image.sample(p0 * inv).xyz
            let c1 = self.image.sample((p0 + vec2(step, 0.0)) * inv).xyz
            let cu = vec3(-0.1146, -0.3854, 0.5)
            let cv = vec3(0.5, -0.4542, -0.0458)
            let u0 = (128.0 + 224.0 * dot(c0, cu)) / 255.0
            let v0 = (128.0 + 224.0 * dot(c0, cv)) / 255.0
            let u1 = (128.0 + 224.0 * dot(c1, cu)) / 255.0
            let v1 = (128.0 + 224.0 * dot(c1, cv)) / 255.0
            return vec4(u1, v0, u0, v1)
        }
    }

}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12Luma {
    #[deref]
    pub draw_super: DrawQuad,
    /// The packed rect of the image, in its pixels.
    #[live]
    pub source: Vec4f,
    /// The image's size in pixels.
    #[live]
    pub extent: Vec2f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12Chroma {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub source: Vec4f,
    #[live]
    pub extent: Vec2f,
    /// 1: 4:4:4 (every pixel's chroma), 0: 4:2:0.
    #[live]
    pub full: f32,
}

