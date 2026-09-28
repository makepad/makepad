//! HUD plates, arcs and images (2D, drawn after the 3D blit).

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    // ---- 2D HUD -------------------------------------------------------
    //
    // The HUD draws AFTER the 3D pass is blitted, in ordinary 2D turtle
    // space, so these are plain DrawQuads. Two shapes carry the whole kit:
    // a rounded plate (panels, bar tracks, bar fills, pips) and an arc
    // (rings). Everything else a HUD needs is text, which DrawText already
    // does, or an icon, which is an SVG or a store image and never a shader.
    mod.draw.DrawHudShape = mod.std.set_type_default() do #(DrawHudShape::script_shader(vm)){
        ..mod.draw.DrawQuad,
        // `fill`, `stroke`, `border`, `radius`, `shape`, `from`, `sweep`,
        // `thickness` and `frac` are the Rust struct's own #[live] instance
        // fields — `script_shader` declares them. Re-declaring them here as
        // `instance(...)` applies a shader-descriptor OBJECT to a typed f32
        // field, which fails the apply and leaves the shader half-built.
        //
        // How a host cuts its plates: 0 rounds them with `radius`, 1 cuts
        // them on a chamfer (the big cut, `radius`, on the top-left and
        // bottom-right corners, a third of it on the other two). A plate
        // whose radius reaches half its short side is a disc either way.
        chamfer: uniform(0.0)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // Shapes by range (1 arc, 2/3 segment, 4 dart, 5 disc, else plate).
            if self.shape > 0.5 && self.shape < 1.5 {
                let c = self.rect_size * 0.5
                let r = min(c.x, c.y) - self.thickness * 0.5 - 1.0
                // A zero-length arc must draw nothing rather than a full
                // ring: `end == start` is a degenerate sweep and the SDF
                // reads it as "everywhere".
                if self.frac > 0.001 {
                    sdf.arc_flat_caps(c.x, c.y, r, self.from, self.from + self.sweep * self.frac, self.thickness)
                    sdf.fill(self.fill)
                }
                return sdf.result
            }
            // Segments and the dart are plain distance math (no Sdf2d path):
            // coverage from the pixel's distance to the shape, premultiplied.
            let p = self.pos * self.rect_size
            let i = self.thickness * 0.5 + 1.0
            if self.shape > 1.5 && self.shape < 3.5 {
                // A segment corner to corner of its quad: 2 runs top-left to
                // bottom-right, 3 bottom-left to top-right (a map polyline,
                // a HUD line).
                var a = vec2(i, i)
                var e = vec2(self.rect_size.x - i, self.rect_size.y - i)
                if self.shape > 2.5 {
                    a = vec2(i, self.rect_size.y - i)
                    e = vec2(self.rect_size.x - i, i)
                }
                let ab = e - a
                let h = clamp(dot(p - a, ab) / max(dot(ab, ab), 0.0001), 0.0, 1.0)
                let d = length(p - a - ab * h)
                let cov = clamp(self.thickness * 0.5 - d + 0.5, 0.0, 1.0) * self.fill.w
                return vec4(self.fill.xyz * cov, cov)
            }
            if self.shape > 3.5 && self.shape < 4.5 {
                // A heading dart pointing up, turned by `from` radians: the
                // pixel is turned back into the dart's frame, then tested
                // against its two outer edges and its notch.
                let c = self.rect_size * 0.5
                let r = min(c.x, c.y) - self.border - 1.0
                let sn = sin(self.from)
                let cs = cos(self.from)
                let q0 = p - c
                let q = vec2(q0.x * cs + q0.y * sn, -q0.x * sn + q0.y * cs) / max(r, 0.001)
                // Tip (0,-1), wings (+-0.8, 0.85), notch (0, 0.45).
                let w = (q.y + 1.0) / 1.85 * 0.8
                let outer = abs(q.x) - w
                let notch = q.y - (0.45 + abs(q.x) / 0.8 * 0.4)
                let inside = max(outer * r, max(notch * r, (-1.0 - q.y) * r))
                let cov = clamp(0.5 - inside, 0.0, 1.0)
                let edge = clamp(0.5 - abs(inside + self.border * 0.5) + self.border * 0.5, 0.0, 1.0)
                var col = self.fill * cov
                if self.border > 0.0 {
                    col = mix(col, self.stroke * max(cov, edge), edge * self.stroke.w)
                }
                return vec4(col.xyz * col.w, col.w)
            }
            let b = max(self.border, 0.0)
            let c = self.rect_size * 0.5
            let disc = self.shape > 4.5 || self.radius >= min(c.x, c.y) - 0.5
            if disc {
                // A round plate (a radar, a round badge): 5, or any plate
                // rounded all the way.
                sdf.circle(c.x, c.y, min(c.x, c.y) - b * 0.5)
                sdf.fill_keep(self.fill)
                if b > 0.0 {
                    sdf.stroke(self.stroke, b)
                }
                return sdf.result
            }
            if self.chamfer > 0.5 {
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let s = p - c
                let h = max(c - vec2(0.5, 0.5), vec2(0.5, 0.5))
                let q = abs(s) - h
                let k = min(mix(self.radius * 0.34, self.radius, step(0.0, s.x * s.y)), min(h.x, h.y))
                let d = max(max(q.x, q.y), (q.x + q.y + k) * 0.7071)
                let cov = 1.0 - smoothstep(-px, px, d)
                let rim = (1.0 - smoothstep(b - px, b + px * 0.5, -d)) * step(0.01, b) * self.stroke.w
                let rgb = mix(self.fill.xyz * self.fill.w, self.stroke.xyz, rim)
                let a = max(self.fill.w, rim)
                return vec4(rgb * cov, a * cov)
            }
            sdf.box(b * 0.5, b * 0.5, self.rect_size.x - b, self.rect_size.y - b, self.radius)
            sdf.fill_keep(self.fill)
            if b > 0.0 {
                sdf.stroke(self.stroke, b)
            }
            return sdf.result
        }
    }

    // One HUD image: a catalog picture (a key sprite, a mugshot, a weapon
    // frame) on a quad, tinted and optionally ghosted. This draw type is the
    // pixel-art lane used by both HUD catalog sprites and FPS weapon/flash
    // sheets; callers can clear `pixelated` for genuinely smooth content.
    mod.draw.DrawHudImage = mod.std.set_type_default() do #(DrawHudImage::script_shader(vm)){
        ..mod.draw.DrawQuad,
        tex: texture_2d(float)
        // `tint` and the sampling mode come from the Rust struct.
        pixel: fn() {
            let uv = vec2(
                self.uv_rect.x + self.pos.x * (self.uv_rect.z - self.uv_rect.x),
                self.uv_rect.y + self.pos.y * (self.uv_rect.w - self.uv_rect.y)
            )
            var c = self.tex.sample_as_bgra(uv)
            if self.pixelated > 0.5 {
                // Exact texel, base level: no bilinear four-texel blend and
                // no mip-selected half-resolution weapon at any scale.
                c = self.tex.sample_nearest(uv, 0.0)
            }
            return Pal.premul(vec4(c.xyz * self.tint.xyz, c.w * self.tint.w))
        }
    }
}
