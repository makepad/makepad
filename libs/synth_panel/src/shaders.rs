//! The displays' shader, in Splash: a screen (scope, wave, filter,
//! envelope, LFO, spectrum) whose trace is one row of a float texture.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish
    use mod.sdf.Material

    // THE SCREENS. `row` is the texture row the trace lives in.
    set_type_default() do #(DrawScreen::script_shader(vm)){
        ..mod.draw.DrawQuad
        data: texture_2d(float)

        at: fn(u: float) -> float {
            let x = clamp(u, 0.0, 1.0) * 255.0
            let i0 = floor(x)
            let f = x - i0
            let a = self.data.sample_nearest(vec2((i0 + 0.5) / 256.0, self.row), 0.0).x
            let b = self.data.sample_nearest(vec2((min(i0 + 1.0, 255.0) + 0.5) / 256.0, self.row), 0.0).x
            return mix(a, b, f)
        }

        pixel: fn() {
            let p = self.pos * self.rect_size
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let s = self.scale
            let sz = self.rect_size
            let c = sz * 0.5
            let k = self.kind
            let bz = self.bezel
            let bez = 3.0 * s * bz
            let outer = Material.sd_box(p, c, c - vec2(0.5, 0.5), mix(3.0, 6.0 * s, bz))
            let inner_h = c - vec2(bez, bez) - vec2(0.5, 0.5) * (1.0 - bz)
            let inner = Material.sd_box(p, c, inner_h, mix(3.0, 3.5 * s, bz))
            var out = vec4(0.0, 0.0, 0.0, 0.0)
            // The bezel: darker than the face, lit on its lower lip.
            if bz > 0.01 {
                let oa = Finish.cover(outer, px)
                var bcol = self.face.rgb * 0.55
                bcol = mix(bcol, self.face.rgb * 1.3 + vec3(0.04, 0.04, 0.04), Finish.band(outer, 0.0, px, px) * step(c.y, p.y))
                out = Finish.over(out, vec4(bcol * oa, oa))
            }
            let ia = Finish.cover(inner, px)
            if ia <= 0.0 {
                return out
            }
            let lo = c - inner_h
            let wh = inner_h * 2.0
            let uv = (p - lo) / wh
            var col = self.screen.rgb * mix(1.0, (1.0 + 0.35 * (1.0 - uv.y)) * (1.0 - 0.25 * length(uv - vec2(0.5, 0.5))), bz)
            let ink = self.ink.rgb
            // The graticule.
            let g = abs(fract(vec2(uv.x * 8.0, uv.y * 4.0)) - vec2(0.5, 0.5))
            let gl = max(Finish.cover((0.5 - g.x) * wh.x / 8.0 - px * 0.5, px), Finish.cover((0.5 - g.y) * wh.y / 4.0 - px * 0.5, px))
            col = col + ink * gl * 0.055
            if k < 2.5 || (k > 4.5 && k < 5.5) {
                col = col + ink * Finish.cover(abs(uv.y - 0.5) * wh.y - px * 0.5, px) * 0.1
            }
            let du = 1.0 / max(wh.x, 1.0)
            // Below the caption line, above the bottom edge.
            let top = lo.y + self.band * s
            let span = wh.y - (self.band + 4.0) * s
            if self.under > -0.5 {
                // A spectrum under the curve, dim.
                let un = max(self.count, 1.0)
                let ub = floor(uv.x * un)
                let uv_ = self.data.sample_nearest(vec2((ub + 0.5) / 256.0, self.under), 0.0).x
                let uy = top + (1.0 - uv_) * span
                let ux = fract(uv.x * un) * wh.x / un
                let ubar = step(0.8 * s, ux) * step(ux, wh.x / un - 0.8 * s) * step(uy, p.y)
                col = mix(col, ink * 0.55, ubar * 0.3)
            }
            if k > 5.5 {
                // SPECTRUM: bars, `count` bands, bright at their tops.
                let n = max(self.count, 1.0)
                let b = floor(uv.x * n)
                let v = self.data.sample_nearest(vec2((b + 0.5) / 256.0, self.row), 0.0).x
                let bx = fract(uv.x * n) * wh.x / n
                let yv = top + (1.0 - v) * span
                let inbar = step(0.8 * s, bx) * step(bx, wh.x / n - 0.8 * s) * step(yv, p.y)
                let hot = clamp((p.y - yv) / (6.0 * s), 0.0, 1.0)
                let bar = mix(ink * 1.25 + vec3(0.2, 0.2, 0.2), ink * (0.3 + 0.4 * (1.0 - uv.y)), hot)
                col = mix(col, bar, inbar * 0.9)
                col = col + ink * Finish.glow(max(yv - p.y, 0.0), 5.0 * s) * 0.18 * step(0.8 * s, bx) * step(bx, wh.x / n - 0.8 * s) * self.glow
            } else {
                // A trace through three samples a pixel apart.
                let u = uv.x
                let y0 = top + (1.0 - self.at(u - du)) * span
                let y1 = top + (1.0 - self.at(u)) * span
                let y2 = top + (1.0 - self.at(u + du)) * span
                let d0 = abs(p.y - clamp(p.y, min(y0, y1), max(y0, y1)))
                let d2 = abs(p.y - clamp(p.y, min(y1, y2), max(y1, y2)))
                let d = max(min(d0, d2) - 0.9 * s, 0.0)
                // Filled under the curve for the shapes of a field.
                if k > 2.5 && k < 4.5 {
                    let under = step(y1, p.y) * (0.16 + 0.1 * (1.0 - (p.y - y1) / max(span, 1.0)))
                    col = col + ink * under
                }
                let line = Finish.cover(d - 0.25 * s, px)
                col = mix(col, ink + vec3(0.25, 0.25, 0.25) * line, line)
                col = col + ink * Finish.glow(d, 3.2 * s) * 0.45 * self.glow
                // Markers: the cutoff (3), the envelope's corners (4), the
                // LFO's head (5).
                if k > 2.5 && k < 3.5 {
                    let mx = lo.x + self.marks.x * wh.x
                    let dash = step(0.5, fract(p.y / (4.0 * s)))
                    col = mix(col, ink, Finish.cover(abs(p.x - mx) - 0.5 * s, px) * dash * 0.4)
                    let my = top + (1.0 - self.at(self.marks.x)) * span
                    let hd = length(p - vec2(mx, my)) - 3.5 * s
                    col = mix(col, ink + vec3(0.3, 0.3, 0.3), Finish.cover(hd, px) * (0.7 + self.hover * 0.3))
                }
                if k > 3.5 && k < 4.5 {
                    let ya = top
                    let ys = top + (1.0 - self.marks.w) * span
                    let h1 = length(p - vec2(lo.x + self.marks.x * wh.x, ya)) - 3.6 * s
                    let h2 = length(p - vec2(lo.x + self.marks.y * wh.x, ys)) - 3.6 * s
                    let h3 = length(p - vec2(lo.x + self.marks.z * wh.x, ys)) - 3.6 * s
                    let hv = self.hover
                    col = mix(col, ink + vec3(0.35, 0.35, 0.35), Finish.cover(h1 - step(abs(hv - 1.0), 0.1) * 1.5 * s, px))
                    col = mix(col, ink + vec3(0.35, 0.35, 0.35), Finish.cover(h2 - step(abs(hv - 2.0), 0.1) * 1.5 * s, px))
                    col = mix(col, ink + vec3(0.35, 0.35, 0.35), Finish.cover(h3 - step(abs(hv - 3.0), 0.1) * 1.5 * s, px))
                }
                if k > 4.5 && k < 5.5 {
                    let hy = top + (1.0 - self.at(1.0)) * span
                    let hd = length(p - vec2(lo.x + wh.x - 3.0 * s, hy)) - 3.0 * s
                    col = mix(col, ink + vec3(0.3, 0.3, 0.3), Finish.cover(hd, px))
                    col = col + ink * Finish.glow(max(hd, 0.0), 6.0 * s) * 0.5 * self.glow
                }
            }
            // Scanlines and the glass.
            col = col * mix(1.0, Finish.scan(p.y, px, 2.0 * s, 0.1), bz)
            col = col + vec3(1.0, 1.0, 1.0) * 0.045 * bz * smoothstep(0.55, 0.0, uv.y) * smoothstep(0.1, 0.9, 1.0 - uv.x * 0.6)
            out = Finish.over(out, vec4(col * ia, ia))
            return out
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawScreen {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub face: Vec4f,
    #[live]
    pub screen: Vec4f,
    #[live]
    pub ink: Vec4f,
    #[live]
    pub marks: Vec4f,
    #[live]
    pub kind: f32,
    #[live]
    pub row: f32,
    #[live]
    pub count: f32,
    #[live]
    pub hover: f32,
    #[live]
    pub scale: f32,
    #[live]
    pub glow: f32,
    /// 1 a bezelled screen; 0 a flat graph (the Code view's lenses).
    #[live(1.0)]
    pub bezel: f32,
    /// Points kept free at the top for a caption.
    #[live(15.0)]
    pub band: f32,
    /// A spectrum drawn dimly under a curve: its data row (-1 none).
    #[live(-1.0)]
    pub under: f32,
}
