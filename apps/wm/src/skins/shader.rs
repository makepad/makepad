//! The window skins' shader: one quad per window, drawn after the window's
//! content so the Float handle can sit over it. Everything a skin paints is a
//! function of window-local coordinates (`p`, in logical pixels, 0,0 at the
//! window's top-left corner); the quad reaches `margin` beyond the window on
//! every side for the glow and the shadow. Output is premultiplied alpha.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*
    mod.widgets.DrawSkin = set_type_default() do #(DrawSkin::script_shader(vm)) {
        ..mod.draw.DrawQuad
        // win: margin, corner radius, frame inset, title height.
        win: vec4(0.0, 0.0, 0.0, 0.0)
        // style: skin (1 holo, 2 float, 3 facet), focus 0..1, hovered
        // control (1 minimize, 2 maximize, 3 close, 4 title), expanded.
        style: vec4(0.0, 0.0, 0.0, 0.0)
        // btn: first control's centre x, step between centres, centre y, glyph size.
        btn: vec4(0.0, 0.0, 0.0, 0.0)
        // pill: the Float handle's rect (x y w h).
        pill: vec4(0.0, 0.0, 0.0, 0.0)
        // light: direction toward the light (xyz, y up), and the fade.
        light: vec4(0.0, 0.5, 0.85, 1.0)
        // The theme's colours; the accent is refreshed from the live theme.
        accent: mod.wm_theme.accent
        bg: mod.wm_theme.darker_background
        fg: mod.wm_theme.foreground

        sd_box: fn(p: vec2, c: vec2, hs: vec2, r: float) -> float {
            let q = abs(p - c) - hs + vec2(r, r)
            return length(max(q, vec2(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r
        }
        seg: fn(p: vec2, a: vec2, b: vec2) -> float {
            let pa = p - a
            let ba = b - a
            let t = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0)
            return length(pa - ba * t)
        }
        cov: fn(d: float) -> float {
            return clamp(0.5 - d * self.draw_pass.dpi_factor, 0.0, 1.0)
        }
        over: fn(dst: vec4, rgb: vec3, a: float) -> vec4 {
            return vec4(rgb * a + dst.rgb * (1.0 - a), a + dst.a * (1.0 - a))
        }
        // One control's glyph outline: 1 minimize, 2 maximize, 3 close.
        glyph: fn(p: vec2, c: vec2, g: float, which: float) -> float {
            if which < 1.5 {
                return self.seg(p, c + vec2(-g, g * 0.5), c + vec2(g, g * 0.5))
            }
            if which < 2.5 {
                return abs(self.sd_box(p, c, vec2(g, g), 0.0))
            }
            return min(self.seg(p, c + vec2(-g, -g), c + vec2(g, g)), self.seg(p, c + vec2(-g, g), c + vec2(g, -g)))
        }
        // One control as an outline: `ink` at rest, `hot` under the pointer.
        button: fn(dst: vec4, p: vec2, which: float, ink: vec3, hot: vec3, a: float) -> vec4 {
            var c = dst
            let on = 1.0 - step(0.5, abs(self.style.z - which))
            let ctr = vec2(self.btn.x + self.btn.y * (which - 1.0), self.btn.z)
            if on > 0.5 {
                let back = self.cov(self.sd_box(p, ctr, vec2(self.btn.y * 0.5, self.btn.y * 0.5), 4.0))
                c = self.over(c, hot, back * 0.16)
            }
            let d = self.glyph(p, ctr, self.btn.w * 0.5, which)
            return self.over(c, mix(ink, hot, on), self.cov(d - 0.5) * mix(a, 1.0, on))
        }
        buttons: fn(dst: vec4, p: vec2, ink: vec3, hot: vec3, a: float) -> vec4 {
            var c = self.button(dst, p, 1.0, ink, hot, a)
            c = self.button(c, p, 2.0, ink, hot, a)
            return self.button(c, p, 3.0, ink, hot, a)
        }

        pixel: fn() {
            let m = self.win.x
            let r = self.win.y
            let inset = self.win.z
            let th = self.win.w
            let kind = self.style.x
            let f = self.style.y
            let sz = self.rect_size - vec2(m * 2.0, m * 2.0)
            let p = self.pos * self.rect_size - vec2(m, m)
            let d = self.sd_box(p, sz * 0.5, sz * 0.5, r)
            var c = vec4(0.0, 0.0, 0.0, 0.0)
            if kind < 1.5 {
                // Holo: neon edge lines with a glow, a scanline title strip.
                let neon = mix(self.accent.rgb, vec3(1.0, 1.0, 1.0), 0.3 * f)
                let gw = mix(3.5, 6.0, f)
                c = self.over(c, neon, exp(-abs(d) / gw) * mix(0.4, 0.8, f))
                let tx = vec2(sz.x * 0.5, inset + th * 0.5)
                let strip = self.cov(self.sd_box(p, tx, vec2(sz.x * 0.5 - inset, th * 0.5), 0.0))
                let px = floor(p.y * self.draw_pass.dpi_factor)
                let sl = modf(px, 2.0)
                let sbase = mix(self.bg.rgb, neon, 0.1)
                c = self.over(c, sbase * (1.0 - 0.45 * sl) + neon * 0.16 * (1.0 - sl), strip * 0.95)
                let under = self.cov(abs(p.y - (inset + th - 0.5)) - 0.5) * strip
                c = self.over(c, neon, under * mix(0.35, 0.85, f))
                c = self.buttons(c, p, neon * mix(0.8, 1.0, f), mix(neon, vec3(1.0, 1.0, 1.0), 0.5), 0.9)
                let lt = mix(1.0, 1.8, f)
                let hs = sz * 0.5 - vec2(lt * 0.5, lt * 0.5)
                let line = self.cov(abs(self.sd_box(p, sz * 0.5, hs, max(r - lt * 0.5, 0.0))) - lt * 0.5)
                let ex = min(p.x, sz.x - p.x)
                let ey = min(p.y, sz.y - p.y)
                let corner = 1.0 - smoothstep(8.0, 20.0, max(ex, ey))
                c = self.over(c, mix(neon, vec3(1.0, 1.0, 1.0), corner * 0.6), line * mix(0.7, 1.0, f))
            } else if kind < 2.5 {
                // Float: a long soft shadow, a hairline, and the handle.
                let lift = 0.55 + 0.45 * f
                let outside = 1.0 - self.cov(d)
                let lo = vec2(0.0, 0.0)
                let hi = sz
                let near = GaussShadow.rounded_box_shadow(lo + vec2(0.0, 2.0), hi + vec2(0.0, 2.0), p, 2.5, r)
                let far = GaussShadow.rounded_box_shadow(lo + vec2(0.0, 8.0), hi + vec2(0.0, 8.0), p, 7.0, r)
                c = self.over(c, vec3(0.0, 0.0, 0.0), (near * 0.35 + far * 0.5) * lift * outside)
                c = self.over(c, self.fg.rgb, self.cov(abs(d + 0.5) - 0.5) * 0.14)
                let pc = vec2(self.pill.x + self.pill.z * 0.5, self.pill.y + self.pill.w * 0.5)
                let ph = vec2(self.pill.z * 0.5, self.pill.w * 0.5)
                let pd = self.sd_box(p, pc, ph, ph.y)
                let ps = GaussShadow.rounded_box_shadow(pc - ph + vec2(0.0, 2.0), pc + ph + vec2(0.0, 2.0), p, 3.0, ph.y)
                c = self.over(c, vec3(0.0, 0.0, 0.0), ps * 0.4 * (1.0 - self.cov(pd)))
                c = self.over(c, self.bg.rgb, self.cov(pd) * 0.86)
                let rim = mix(self.fg.rgb, self.accent.rgb, f)
                c = self.over(c, rim, self.cov(abs(pd + 0.5) - 0.5) * mix(0.22, 0.6, f))
                let ex = self.style.w
                let dots = (1.0 - smoothstep(0.0, 0.4, ex)) * 0.55
                c = self.over(c, rim, self.cov(length(p - pc - vec2(-6.0, 0.0)) - 1.3) * dots)
                c = self.over(c, rim, self.cov(length(p - pc) - 1.3) * dots)
                c = self.over(c, rim, self.cov(length(p - pc - vec2(6.0, 0.0)) - 1.3) * dots)
                c = mix(c, self.buttons(c, p, self.fg.rgb, self.accent.rgb, 0.7), smoothstep(0.6, 1.0, ex))
            } else {
                // Facet: a bevelled crystal frame lit from the background's light.
                let bw = max(inset, 1.0)
                let dt = p.y
                let db = sz.y - p.y
                let dl = p.x
                let dr = sz.x - p.x
                var n = vec2(0.0, -1.0)
                var mn = dt
                if db < mn {
                    mn = db
                    n = vec2(0.0, 1.0)
                }
                if dl < mn {
                    mn = dl
                    n = vec2(-1.0, 0.0)
                }
                if dr < mn {
                    mn = dr
                    n = vec2(1.0, 0.0)
                }
                // Two planes per side, the outer one steeper: a chamfer. Each
                // is as bright as its side faces the light (screen plane).
                let l2 = normalize(vec2(self.light.x, -self.light.y) + vec2(0.001, -0.001))
                var tilt = 0.55
                if mn < bw * 0.5 {
                    tilt = 1.0
                }
                let ndl = dot(n, l2) * tilt
                let di = self.sd_box(p, sz * 0.5, sz * 0.5 - vec2(bw, bw), 0.0)
                let band = self.cov(d) * (1.0 - self.cov(di))
                let t = clamp(0.5 + 0.5 * ndl, 0.0, 1.0)
                let dark = self.bg.rgb * 0.5 + self.accent.rgb * 0.12
                let lit = mix(self.accent.rgb, vec3(1.0, 1.0, 1.0), 0.6)
                var fr = mix(dark, lit, t) * mix(0.7, 1.0, f)
                fr = mix(fr, self.bg.rgb * 0.5, 1.0 - smoothstep(0.0, 1.0, -d))
                fr = mix(fr, vec3(1.0, 1.0, 1.0), (1.0 - smoothstep(0.0, 1.2, di)) * 0.3 * t)
                c = self.over(c, fr, band * 0.97)
                if th > 0.5 {
                    let tc = vec2(sz.x * 0.5, bw + th * 0.5)
                    let strip = self.cov(self.sd_box(p, tc, vec2(sz.x * 0.5 - bw, th * 0.5), 0.0))
                    let g = clamp((p.y - bw) / th, 0.0, 1.0)
                    let top = mix(self.bg.rgb, self.accent.rgb, 0.1 + 0.14 * f) * 1.2
                    let bot = self.bg.rgb * 0.85
                    var tr = mix(top, bot, g)
                    let spot = max(0.0, 1.0 - abs(p.x / sz.x - (0.5 + 0.5 * self.light.x)) * 2.5)
                    tr = tr + vec3(1.0, 1.0, 1.0) * 0.07 * spot * (1.0 - g)
                    tr = mix(tr, vec3(1.0, 1.0, 1.0), (1.0 - smoothstep(0.0, 1.2, p.y - bw)) * 0.2)
                    tr = mix(tr, self.bg.rgb * 0.5, smoothstep(th - 1.2, th, p.y - bw))
                    c = self.over(c, tr, strip * 0.97)
                    c = self.buttons(c, p, self.fg.rgb, self.accent.rgb, 0.65)
                }
            }
            return c * self.light.w
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkin {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub win: Vec4f,
    #[live]
    pub style: Vec4f,
    #[live]
    pub btn: Vec4f,
    #[live]
    pub pill: Vec4f,
    #[live]
    pub light: Vec4f,
    #[live]
    pub accent: Vec4f,
    #[live]
    pub bg: Vec4f,
    #[live]
    pub fg: Vec4f,
}
