//! The graphic EQ editor (`HwEq`): a bank of faders (the preamp and the
//! ten octave bands of `geq.rs`) over the response curve the filters
//! really give, with the sound playing faintly behind it.
//!
//! * **Editing.** Drag a fader's cap; press anywhere else in the graph to
//!   draw the curve: the band nearest the pointer takes its height as it
//!   sweeps, as on a hardware graphic EQ run with one finger. A double
//!   press resets a band (or the preamp). The wheel nudges the band under
//!   the pointer; with the graph focused, Left/Right pick a fader,
//!   Up/Down move it half a dB (Shift: a tenth), 0 or Backspace resets it.
//! * **The curve** is the sum of the ten biquads' magnitudes
//!   (`geq::response_db`) for the gains solved from the sliders, not a
//!   line through the caps.
//! * **The look is the style's.** Every part is a shader with hooks a style
//!   file sets (see the STYLE_GUIDE's EQ section): the fader kind (chrome
//!   slider caps, LED ladders, rubber caps, glowing light bars, stepped
//!   deco caps, Bauhaus blocks), the curve kind (neon, phosphor trace, LCD
//!   dot matrix, engraved line, printed ink), the grid and the inks.
//! * **Where it sits.** The layout's `eq` slot, or, where a layout has
//!   none, a drawer over the main list (`HwStage`), or a popover on a small
//!   device. It reflows by its own size: fewer labels and thinner faders
//!   when compact.
//! * **At rest nothing repaints.** The spectrum behind the curve asks for
//!   frames only while sound arrives and its bars are still falling, and
//!   only while the panel is drawn at all.

use crate::audio::{GAIN_MAX_DB, GAIN_MIN_DB};
use crate::geq::{self, UserPresets, BANDS};
use crate::layout::HwStageWidgetRefExt;
use crate::model::{Model, Page};
use crate::widgets::DrawMusicRound;
use crate::App;
use makepad_audio_picture::{log_bands, Fft};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The grid behind the bank: rows every 6 dB (`db_step` of the half
    // height each), a column line at every band, the 0 dB line.
    // `grid` = kind (0 lines, 1 dots at the crossings, 2 ticks at the
    // edges only, 3 graph paper), line strength, 0 dB line strength, -.
    // `grid_ink` = rgb and how much of it over the palette's text ink.
    set_type_default() do #(DrawEqGrid::script_shader(vm)){
        ..mod.draw.DrawQuad
        grid: uniform(vec4(0.0, 0.10, 0.30, 0.0))
        grid_ink: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        ink_text: uniform(vec4(0.9, 0.9, 0.9, 1.0))
        ink_accent: uniform(vec4(0.2, 0.8, 1.0, 1.0))
        db_step: uniform(0.4)
        bands: uniform(10.0)
        pixel: fn() {
            let s = self.rect_size
            let p = self.pos * s
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let ink = mix(self.ink_text.xyz, self.grid_ink.xyz, self.grid_ink.w)
            let kind = self.grid.x
            let mid = s.y * 0.5
            let rowh = max(s.y * 0.5 * self.db_step, 2.0)
            let colw = s.x / self.bands
            let ry = abs(fract((p.y - mid) / rowh + 0.5) - 0.5) * rowh
            let cxd = abs(fract(p.x / colw) - 0.5) * colw
            let inrows = step(abs(p.y - mid), rowh * 2.0 + 0.5)
            var a = 0.0
            if kind < 0.5 {
                a = (1.0 - smoothstep(0.0, px * 1.2, ry)) * inrows * self.grid.y
                a = max(a, (1.0 - smoothstep(0.0, px * 1.2, cxd)) * self.grid.y * 0.55)
            } else if kind < 1.5 {
                let d = length(vec2(cxd, ry))
                a = (1.0 - smoothstep(0.5, 1.4, d)) * inrows * min(self.grid.y * 3.0, 1.0)
            } else if kind < 2.5 {
                let edge = min(p.x, s.x - p.x)
                a = (1.0 - smoothstep(0.0, px * 1.2, ry)) * inrows * (1.0 - smoothstep(5.0, 7.0, edge)) * min(self.grid.y * 3.0, 1.0)
            } else {
                let fine = rowh / 3.0
                let fy = abs(fract((p.y - mid) / fine + 0.5) - 0.5) * fine
                let fx = abs(fract(p.x / (colw / 3.0)) - 0.5) * (colw / 3.0)
                let f = (1.0 - smoothstep(0.0, px, min(fx, fy))) * self.grid.y * 0.45
                let m = (1.0 - smoothstep(0.0, px * 1.4, min(ry, cxd))) * self.grid.y
                a = max(f, m)
            }
            let z = (1.0 - smoothstep(0.0, px * 1.4, abs(p.y - mid))) * self.grid.z
            a = clamp(max(a, z), 0.0, 1.0)
            return vec4(ink * a, a)
        }
    }

    // The sound playing, faintly behind the curve. `data` row 1 holds
    // `spec_count` levels (0..1) across the width. `spectrum` = strength,
    // kind (0 bars, 1 a smooth filled area, 2 dots), bar gap (0..1), -.
    // `spec_ink` = rgb and how much of it over the accent.
    set_type_default() do #(DrawEqSpectrum::script_shader(vm)){
        ..mod.draw.DrawQuad
        data: texture_2d(float)
        count: uniform(192.0)
        spec_count: uniform(96.0)
        spectrum: uniform(vec4(0.20, 0.0, 0.25, 0.0))
        spec_ink: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        ink_accent: uniform(vec4(0.2, 0.8, 1.0, 1.0))
        level_at: fn(i: float) -> float {
            return self.data.sample_nearest(vec2((clamp(i, 0.0, self.spec_count - 1.0) + 0.5) / self.count, 0.75)).x
        }
        pixel: fn() {
            let s = self.rect_size
            let p = self.pos * s
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let ink = mix(self.ink_accent.xyz, self.spec_ink.xyz, self.spec_ink.w)
            let n = self.spec_count
            let fx = self.pos.x * n
            let i = floor(fx)
            let up = (1.0 - self.pos.y)
            var a = 0.0
            if self.spectrum.y < 0.5 {
                let lv = self.level_at(i)
                let f = fract(fx)
                let gap = self.spectrum.z * 0.5
                let bar = step(gap, f) * step(f, 1.0 - gap)
                a = bar * step(up, lv) * (0.45 + 0.55 * up / max(lv, 0.001))
            } else if self.spectrum.y < 1.5 {
                let t = fract(fx - 0.5)
                let lv = mix(self.level_at(floor(fx - 0.5)), self.level_at(floor(fx - 0.5) + 1.0), t)
                let edge = (lv - up) * s.y
                a = smoothstep(-px, px, edge) * (0.35 + 0.65 * up / max(lv, 0.001))
                a = a + (1.0 - smoothstep(0.0, 1.5, abs(edge))) * 0.6
            } else {
                let lv = self.level_at(i)
                let cell = s.x / n
                let rows = floor(s.y / cell)
                let cy = (floor(up * rows) + 0.5) / rows
                let q = vec2(fract(fx) - 0.5, fract(up * rows) - 0.5)
                a = (1.0 - smoothstep(0.28, 0.40, length(q))) * step(cy, lv)
            }
            a = clamp(a * self.spectrum.x, 0.0, 1.0)
            return vec4(ink * a, a)
        }
    }

    // One fader: its whole column. `cap_y`, `zero_y`, `top_y`, `bot_y`
    // are points from the column's top: the cap's centre, 0 dB and the
    // travel's ends. `fader` = kind (0 chrome slider cap, 1 LED ladder,
    // 2 rubber cap, 3 glowing light bar, 4 stepped deco cap, 5 Bauhaus
    // blocks), cap width (of the column), cap height (pt), slot width
    // (pt). `segments` = segment pitch (pt), gap (pt) for the LED ladder and
    // the blocks. Inks: `cap_ink`, `track_ink`, `led_off`, and `glow_ink`
    // (rgb, and how much of it over the accent) for what lights. `scale`
    // (from Rust) thins the faders on a compact panel; `lit` is 1 while the
    // EQ is on, `hover`, `active` and `focus` say what the hand is on.
    set_type_default() do #(DrawEqFader::script_shader(vm)){
        ..mod.draw.DrawQuad
        fader: uniform(vec4(0.0, 0.62, 16.0, 4.0))
        segments: uniform(vec4(4.0, 1.4, 0.0, 0.0))
        cap_ink: uniform(vec4(0.80, 0.82, 0.85, 1.0))
        track_ink: uniform(vec4(0.03, 0.035, 0.04, 1.0))
        led_off: uniform(vec4(0.09, 0.10, 0.11, 1.0))
        glow_ink: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        ink_accent: uniform(vec4(0.2, 0.8, 1.0, 1.0))
        ink_text: uniform(vec4(0.9, 0.9, 0.9, 1.0))
        scale: uniform(1.0)
        box_d: fn(p: vec2, c: vec2, h: vec2, r: float) -> float {
            let q = abs(p - c) - h + vec2(r, r)
            return length(max(q, vec2(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r
        }
        slot: fn(p: vec2, tw: float, glow: vec3) -> vec4 {
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let c = vec2(self.rect_size.x * 0.5, (self.top_y + self.bot_y) * 0.5)
            let h = vec2(tw * 0.5, (self.bot_y - self.top_y) * 0.5 + tw * 0.5)
            let d = self.box_d(p, c, h, tw * 0.5)
            let m = 1.0 - smoothstep(-px, px, d)
            let lip = smoothstep(-1.4, 0.0, d) * step(c.y, p.y)
            var col = mix(self.track_ink.xyz, self.ink_text.xyz * 0.6, lip * 0.25)
            let y0 = min(self.zero_y, self.cap_y)
            let y1 = max(self.zero_y, self.cap_y)
            let fill = step(y0, p.y) * step(p.y, y1) * self.lit
            col = mix(col, glow * 0.85, fill * 0.9)
            return vec4(col * m, m)
        }
        cap_shadow: fn(p: vec2, c: vec2, h: vec2) -> float {
            let d = self.box_d(p, c + vec2(0.0, 2.5), h, 2.0)
            return (1.0 - smoothstep(-1.0, 5.0, d)) * 0.55
        }
        pixel: fn() {
            let s = self.rect_size
            let p = self.pos * s
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let kind = self.fader.x
            let cxm = s.x * 0.5
            let capw = max(s.x * self.fader.y * self.scale, 6.0)
            let caph = self.fader.z * mix(0.75, 1.0, self.scale)
            let tw = max(self.fader.w * self.scale, 1.0)
            let glow = mix(self.ink_accent.xyz, self.glow_ink.xyz, self.glow_ink.w)
            let hot = max(self.hover * 0.5, self.active)
            let dx = abs(p.x - cxm)
            let y0 = min(self.zero_y, self.cap_y)
            let y1 = max(self.zero_y, self.cap_y)
            let c = vec2(cxm, self.cap_y)
            let h = vec2(capw * 0.5, caph * 0.5)
            let v = clamp((p.y - c.y) / max(h.y, 1.0), -1.0, 1.0)
            // The column the hand is on, and the fader the keys move.
            var o = vec4(self.ink_text.xyz, 1.0) * (self.hover * 0.035 + self.focus * 0.05)
            if kind > 0.5 && kind < 1.5 {
                // An LED ladder, lit from 0 dB to the value.
                let pitch = max(self.segments.x, 2.0)
                let k = (p.y - self.zero_y) / pitch + 0.5
                let seg = floor(k)
                let fy = fract(k)
                let cy = self.zero_y + seg * pitch
                let inside = step(self.top_y - pitch * 0.5, p.y) * step(p.y, self.bot_y + pitch * 0.5)
                let cell = step(self.segments.y / pitch * 0.5, fy) * step(fy, 1.0 - self.segments.y / pitch * 0.5) * (1.0 - smoothstep(capw * 0.5 - px, capw * 0.5 + px, dx)) * inside
                let on = step(y0 - pitch * 0.5, cy) * step(cy, y1 + pitch * 0.5)
                let tip = 1.0 - smoothstep(0.0, pitch * 1.5, abs(cy - self.cap_y))
                let e = on * mix(0.3, 1.0, self.lit)
                let col = mix(self.led_off.xyz, mix(glow, vec3(1.0, 1.0, 1.0), tip * 0.35 + hot * 0.15), e)
                o = vec4(col, 1.0) * cell + o * (1.0 - cell)
                let halo = exp(-max(dx - capw * 0.5, 0.0) / 3.0) * e * 0.3 * (1.0 - cell) * inside
                o = o + vec4(glow * halo, halo) * (1.0 - o.w)
            } else if kind > 2.5 && kind < 3.5 {
                // A glowing light bar from 0 dB to the value, a bright tip.
                let track = (1.0 - smoothstep(tw * 0.25 - px, tw * 0.25 + px, dx)) * step(self.top_y, p.y) * step(p.y, self.bot_y)
                o = vec4(self.track_ink.xyz, 1.0) * track * 0.9 + o * (1.0 - track * 0.9)
                let bw = max(capw * 0.30, 2.0)
                let inbar = step(y0, p.y) * step(p.y, y1)
                let core = (1.0 - smoothstep(bw * 0.5 - px, bw * 0.5 + px, dx)) * inbar
                let bloom = exp(-max(dx - bw * 0.5, 0.0) / (bw + 2.0)) * inbar * 0.55
                let tipd = max(abs(p.y - self.cap_y) - 1.2, 0.0) + max(dx - capw * 0.5, 0.0)
                let tip = 1.0 - smoothstep(0.0, 1.2, tipd)
                let tipglow = exp(-length(vec2(max(dx - capw * 0.5, 0.0), abs(p.y - self.cap_y))) / 5.0) * 0.6
                let e = mix(0.35, 1.0, self.lit) * (1.0 + hot * 0.35)
                let a = clamp(max(max(core, bloom), max(tip, tipglow)) * e, 0.0, 1.0)
                let col = glow * (core * 0.8 + bloom + tipglow) * e + mix(glow, vec3(1.0, 1.0, 1.0), 0.65) * tip * e
                o = vec4(col, a) + o * (1.0 - a)
            } else if kind > 4.5 {
                // Bauhaus: a black rule, solid blocks, a square cap.
                let rule = (1.0 - smoothstep(0.6 - px, 0.6 + px, dx)) * step(self.top_y, p.y) * step(p.y, self.bot_y)
                o = vec4(self.track_ink.xyz, 1.0) * rule + o * (1.0 - rule)
                let pitch = max(self.segments.x, 4.0)
                let fy = fract((p.y - self.zero_y) / pitch)
                let blk = step(y0, p.y) * step(p.y, y1) * step(self.segments.y / pitch, fy) * (1.0 - smoothstep(capw * 0.30 - px, capw * 0.30 + px, dx))
                o = vec4(self.cap_ink.xyz, 1.0) * blk + o * (1.0 - blk)
                let dc = self.box_d(p, c, h, 0.0)
                let capm = 1.0 - smoothstep(-px, px, dc)
                let inner = 1.0 - smoothstep(-px, px, dc + 1.6)
                let face = mix(self.led_off.xyz, glow, mix(0.45, 1.0, self.lit)) * (1.0 + hot * 0.12)
                let col = mix(self.track_ink.xyz, face, inner)
                o = vec4(col, 1.0) * capm + o * (1.0 - capm)
            } else {
                // A cap in a slot: chrome (0), rubber (2) or a stepped deco cap (4).
                let sl = self.slot(p, tw, glow)
                o = sl + o * (1.0 - sl.w)
                let sh = self.cap_shadow(p, c, h)
                o = vec4(o.xyz * (1.0 - sh), max(o.w, sh))
                var d = 0.0
                var col = self.cap_ink.xyz
                if kind < 0.5 {
                    d = self.box_d(p, c, h, 2.0)
                    // Polished metal: bright over the top, dark under, grip grooves.
                    col = self.cap_ink.xyz * (1.12 - 0.28 * v)
                    let g = abs(fract((p.y - c.y) / 3.0 + 0.5) - 0.5) * 3.0
                    let groove = (1.0 - smoothstep(0.3, 0.8, g)) * step(abs(p.y - c.y), h.y - 3.0) * step(2.0, abs(p.y - c.y))
                    col = col * (1.0 - groove * 0.28)
                    let rim = smoothstep(-2.0, 0.0, d)
                    col = mix(col, col * mix(1.35, 0.55, step(0.0, v)), rim * 0.8)
                } else if kind < 2.5 {
                    d = self.box_d(p, c, h, min(h.x, h.y))
                    // Soft rubber: matte, a broad highlight on top.
                    let lift = (1.0 - smoothstep(-1.0, 0.6, v)) * 0.18
                    col = self.cap_ink.xyz * (1.0 + lift - 0.12 * max(v, 0.0))
                    col = mix(col, col * 0.7, smoothstep(-1.8, 0.0, d))
                } else {
                    // Art deco: three tiers, the widest across the middle.
                    let d1 = self.box_d(p, c, vec2(h.x, h.y * 0.42), 0.5)
                    let d2 = self.box_d(p, c, vec2(h.x * 0.76, h.y * 0.74), 0.5)
                    let d3 = self.box_d(p, c, vec2(h.x * 0.52, h.y), 0.5)
                    d = min(d1, min(d2, d3))
                    let tier = step(0.0, d1) + step(0.0, d2)
                    col = self.cap_ink.xyz * (1.18 - 0.30 * v - tier * 0.08)
                    let edge = 1.0 - smoothstep(0.0, 1.2, min(abs(d1), min(abs(d2), abs(d3))))
                    col = mix(col, col * mix(1.4, 0.5, step(0.0, v)), edge * 0.7)
                }
                // The index line across the cap.
                let idx = (1.0 - smoothstep(0.35, 0.95, abs(p.y - c.y))) * step(abs(p.x - c.x), h.x - 2.0)
                col = mix(col, mix(col * 0.25, glow * 1.1, self.lit), idx * 0.9)
                col = col * (1.0 + hot * 0.12)
                let m = 1.0 - smoothstep(-px, px, d)
                o = vec4(col, 1.0) * m + o * (1.0 - m)
            }
            return o
        }
    }

    // The response curve over the bank. `data` row 0 holds `count` points
    // across the width: the curve's height (0 top .. 1 bottom). `curve` =
    // kind (0 glowing neon line, 1 phosphor trace, 2 LCD dot matrix,
    // 3 engraved line, 4 printed ink), line width (pt), glow reach (pt; the
    // dot pitch for the LCD), fill under the curve to 0 dB (alpha).
    // `curve_ink` = rgb and how much of it over the accent; `lip_ink` is
    // the engraved line's lit edge. `lit` is 1 while the EQ is on.
    set_type_default() do #(DrawEqCurve::script_shader(vm)){
        ..mod.draw.DrawQuad
        data: texture_2d(float)
        count: uniform(192.0)
        curve: uniform(vec4(0.0, 1.8, 7.0, 0.10))
        curve_ink: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        lip_ink: uniform(vec4(1.0, 1.0, 1.0, 0.35))
        ink_accent: uniform(vec4(0.2, 0.8, 1.0, 1.0))
        lit: uniform(1.0)
        curve_y: fn(x: float) -> float {
            let n = self.count
            let fx = clamp(x, 0.0, 1.0) * (n - 1.0)
            let i0 = floor(fx)
            let a0 = self.data.sample_nearest(vec2((i0 + 0.5) / n, 0.25)).x
            let a1 = self.data.sample_nearest(vec2((min(i0 + 1.0, n - 1.0) + 0.5) / n, 0.25)).x
            return mix(a0, a1, fx - i0) * self.rect_size.y
        }
        pixel: fn() {
            let s = self.rect_size
            let p = self.pos * s
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let dxs = 1.5 / max(s.x, 1.0)
            let cy = self.curve_y(self.pos.x)
            let slope = (self.curve_y(self.pos.x + dxs) - self.curve_y(self.pos.x - dxs)) / 3.0
            let norm = sqrt(1.0 + slope * slope)
            let dv = (p.y - cy) / norm
            let d = abs(dv)
            let ink = mix(self.ink_accent.xyz, self.curve_ink.xyz, self.curve_ink.w)
            let dim = mix(0.5, 1.0, self.lit)
            let kind = self.curve.x
            let th = self.curve.y * 0.5
            let mid = s.y * 0.5
            let between = step(min(cy, mid), p.y) * step(p.y, max(cy, mid))
            var o = vec4(ink, 1.0) * self.curve.w * between * dim
            if kind < 0.5 {
                let core = 1.0 - smoothstep(th - px, th + px, d)
                let halo = exp(-max(d - th, 0.0) / max(self.curve.z * 0.5, 0.5)) * 0.6
                let a = clamp(core + halo, 0.0, 1.0) * dim
                let col = (mix(ink, vec3(1.0, 1.0, 1.0), 0.6) * core + ink * halo) * dim
                o = vec4(col, a) + o * (1.0 - a)
            } else if kind < 1.5 {
                let beam = exp(-d * d / max(th * th * 2.0, 0.35))
                let haze = exp(-d / max(self.curve.z, 1.0)) * 0.35
                let scan = 0.82 + 0.18 * sin(p.y * 2.2)
                let a = clamp((beam + haze) * scan, 0.0, 1.0) * dim
                let col = mix(ink, vec3(1.0, 1.0, 1.0), beam * 0.3) * (beam + haze) * scan * dim
                o = vec4(col, a) + o * (1.0 - a)
            } else if kind < 2.5 {
                let pitch = max(self.curve.z, 2.0)
                let cell = floor(p / pitch)
                let cc = (cell + vec2(0.5, 0.5)) * pitch
                let yl = self.curve_y(cell.x * pitch / s.x)
                let yr = self.curve_y((cell.x + 1.0) * pitch / s.x)
                let on = step(min(yl, yr) - pitch * 0.5, cc.y) * step(cc.y, max(yl, yr) + pitch * 0.5)
                let q = abs(p - cc)
                let dotm = 1.0 - smoothstep(pitch * 0.36 - px, pitch * 0.36 + px, max(q.x, q.y))
                let under = step(min(cc.y, mid), mid) * step(min(yl, yr), cc.y) * step(cc.y, mid) + step(mid, cc.y) * step(cc.y, max(yl, yr)) * step(mid, min(yl, yr))
                let a = dotm * max(0.07, max(on * 0.95, under * self.curve.w * 2.0)) * dim
                o = vec4(ink * a, a)
            } else if kind < 3.5 {
                let groove = 1.0 - smoothstep(th - px, th + px, d)
                let lip = (1.0 - smoothstep(0.0, 1.0, abs(dv - th - 0.8))) * self.lip_ink.w
                let shade = (1.0 - smoothstep(0.0, 1.2, abs(dv + th + 0.6))) * 0.35
                o = vec4(o.xyz * (1.0 - shade), max(o.w, shade))
                o = vec4(self.lip_ink.xyz * lip, lip) + o * (1.0 - lip)
                let col = ink * 0.55 * dim
                o = vec4(col * groove, groove) + o * (1.0 - groove)
            } else {
                let rough = (fract(sin(dot(floor(p * 1.5), vec2(12.9898, 78.233))) * 43758.5453) - 0.5) * 0.5
                let core = 1.0 - smoothstep(th - px + rough, th + px + rough + 0.5, d)
                let bleed = exp(-d / 1.3) * 0.22
                let a = clamp(core * 0.92 + bleed, 0.0, 1.0) * dim
                o = vec4(ink * a, a) + o * (1.0 - a)
            }
            return o
        }
    }

    mod.widgets.HwEqGraphBase = #(HwEqGraph::register_widget(vm))
    // The bank and its curve. Styles set `draw_grid`, `draw_spectrum`,
    // `draw_fader`, `draw_curve` (their uniforms, or `pixel` outright),
    // `draw_label` (the type), `label_ink` / `scale_ink` (rgb and how much
    // over the palette's muted ink), and `light` (how strongly lit faders
    // light the device; 0 = not at all).
    mod.widgets.HwEqGraph = set_type_default() do mod.widgets.HwEqGraphBase{
        width: Fill
        height: Fill
        draw_light: mod.widgets.DrawMusicRound{}
        draw_label +: {text_style: theme.font_regular{font_size: 8.0}}
        label_ink: vec4(0.0, 0.0, 0.0, 0.0)
        scale_ink: vec4(0.0, 0.0, 0.0, 0.0)
        light: 0.0
    }

    mod.widgets.HwEqPanelBase = #(HwEqPanel::register_widget(vm))
    // The EQ as a component: its keys over the graph in a display well.
    mod.widgets.HwEq = set_type_default() do mod.widgets.HwEqPanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 8
        eq_head := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 6
            align: Align{y: 0.5}
            eq_power_key := mod.widgets.HwKey{
                width: 92
                height: 32
                draw_bg +: {has_led: 1.0 role: 24.0}
                eq_power := mod.widgets.HwKeyText{text: "EQ OFF"}
            }
            eq_led := mod.widgets.HwLed{width: 16 height: 16 inset: 4}
            eq_auto_cap := mod.widgets.HwKey{
                width: 72
                height: 32
                draw_bg +: {has_led: 1.0 role: 30.0}
                eq_auto := mod.widgets.HwKeyText{text: "AUTO"}
            }
            eq_flat_cap := mod.widgets.HwKey{
                width: 68
                height: 32
                draw_bg +: {role: 25.0}
                eq_flat := mod.widgets.HwKeyText{text: "RESET"}
            }
            eq_title := mod.widgets.HwCaption{width: Fill align: Align{x: 0.5} text: "EQUALIZER"}
            // In a player's band: the presets as a key (a menu), right-aligned.
            eq_band_fill := View{visible: false width: Fill height: 1}
            eq_presets_cap := mod.widgets.HwKey{
                visible: false
                width: 92
                height: 32
                draw_bg +: {role: 33.0}
                eq_presets_key := mod.widgets.HwKeyText{text: "PRESETS"}
            }
            // Wrapped: a drop down cannot hide itself (a player's band
            // shows its PRESETS key instead).
            eq_presets_row := View{
                width: Fill
                height: Fit
                eq_presets := mod.widgets.HwDropDown{
                    width: Fill
                    height: 28
                    labels: ["Flat"]
                }
            }
            eq_save_cap := mod.widgets.HwKey{
                width: 60
                height: 32
                draw_bg +: {role: 31.0}
                eq_save := mod.widgets.HwKeyText{text: "SAVE"}
            }
            eq_close_cap := mod.widgets.HwKey{
                width: 36
                height: 32
                draw_bg +: {role: 32.0}
                eq_close := mod.widgets.HwKeyButton{text: "\u{f00d}"}
            }
        }
        eq_well := mod.widgets.HwDisplay{
            draw_bg +: {plate_id: 10.0}
            width: Fill
            height: Fill
            padding: Inset{left: 10 right: 10 top: 10 bottom: 6}
            eq_graph := mod.widgets.HwEqGraph{}
        }
    }
}

/// The display's span either way, dB: the sliders' ±12 and room for the
/// curve past them.
const SPAN_DB: f32 = 15.0;
/// Points across the curve's texture, and bands in the spectrum behind it.
const CURVE_POINTS: usize = 192;
const SPEC_BANDS: usize = 96;
/// The spectrum's floor, tilt, fall and staleness (as the screen's).
const FLOOR_DB: f32 = -66.0;
const TILT_DB: f32 = 3.0;
const FALL: f32 = 1.4;
const STALE: f64 = 0.25;
const SILENCE: f32 = 1.0e-5;
/// Faders: the preamp is 0, the bands 1..=10.
const FADERS: usize = BANDS + 1;

// ---- draw shaders ----

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEqGrid {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEqSpectrum {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEqCurve {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEqFader {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub cap_y: f32,
    #[live]
    pub zero_y: f32,
    #[live]
    pub top_y: f32,
    #[live]
    pub bot_y: f32,
    #[live]
    pub band: f32,
    #[live]
    pub lit: f32,
    #[live]
    pub hover: f32,
    #[live]
    pub active: f32,
    #[live]
    pub focus: f32,
}

// ---- the graph ----

#[derive(Clone, Debug, Default)]
pub enum EqGraphAction {
    #[default]
    None,
    /// The faders (preamp, then the bands, dB) moved; `done` when let go
    /// (kept on disk then).
    Changed { values: [f32; FADERS], done: bool },
}

/// Where the parts sit, from the last draw.
#[derive(Clone, Copy, Default)]
struct Geo {
    plot: Rect,
    pre: Rect,
    col_w: f64,
    cap_h: f64,
}

impl Geo {
    fn y_of(&self, db: f32) -> f64 {
        let mid = self.plot.pos.y + self.plot.size.y * 0.5;
        mid - (db / SPAN_DB) as f64 * self.plot.size.y * 0.5
    }

    fn db_at(&self, y: f64) -> f32 {
        let mid = self.plot.pos.y + self.plot.size.y * 0.5;
        ((mid - y) / (self.plot.size.y * 0.5).max(1.0)) as f32 * SPAN_DB
    }

    /// The fader column a point is over: 0 the preamp, 1..=10 the bands.
    fn fader_at(&self, x: f64, nearest: bool) -> Option<usize> {
        if x >= self.pre.pos.x && x < self.pre.pos.x + self.pre.size.x {
            return Some(0);
        }
        let rel = (x - self.plot.pos.x) / self.col_w.max(1.0);
        if nearest {
            return Some((rel.floor().clamp(0.0, BANDS as f64 - 1.0)) as usize + 1);
        }
        (rel >= 0.0 && rel < BANDS as f64).then(|| rel as usize + 1)
    }

    fn centre_x(&self, fader: usize) -> f64 {
        if fader == 0 {
            self.pre.pos.x + self.pre.size.x * 0.5
        } else {
            self.plot.pos.x + (fader as f64 - 0.5) * self.col_w
        }
    }
}

#[derive(Clone, Copy)]
struct Drag {
    fader: usize,
    /// Drawing the curve: the band nearest the pointer follows it.
    paint: bool,
    /// A cap held off its centre stays where it was taken.
    offset: f32,
}

/// The spectrum behind the curve.
struct Spectrum {
    fft: Fft,
    samples: Vec<f32>,
    bins: Vec<f32>,
    bands: Vec<f32>,
    tilt: Vec<f32>,
    level: Vec<f32>,
    written: u64,
    fresh_at: f64,
    drawn_at: f64,
}

impl Spectrum {
    fn new() -> Self {
        let tilt = (0..SPEC_BANDS).map(|b| TILT_DB * (geq::axis_hz((b as f32 + 0.5) / SPEC_BANDS as f32) / 1000.0).log2()).collect();
        Self {
            fft: Fft::new(4096),
            samples: vec![0.0; 4096],
            bins: vec![-120.0; 2048],
            bands: vec![-120.0; SPEC_BANDS],
            tilt,
            level: vec![0.0; SPEC_BANDS],
            written: 0,
            fresh_at: f64::MIN,
            drawn_at: 0.0,
        }
    }

    /// Take the newest samples and move the bars; `true` while anything
    /// is left to animate.
    fn step(&mut self, model: &Model, now: f64) -> bool {
        let dt = (now - self.drawn_at).clamp(0.0, 0.1) as f32;
        self.drawn_at = now;
        let sound = model.sound();
        if let Some(analyzer) = sound {
            let written = analyzer.written();
            if written != self.written {
                self.written = written;
                analyzer.latest(&mut self.samples);
                if self.samples.iter().any(|s| s.abs() > SILENCE) {
                    self.fresh_at = now;
                    self.fft.magnitudes_db(&self.samples, &mut self.bins);
                    log_bands(&self.bins, analyzer.sample_rate(), geq::AXIS_LOW_HZ, geq::AXIS_HIGH_HZ, &mut self.bands);
                }
            }
        }
        let fresh = sound.is_some() && now - self.fresh_at < STALE;
        let mut moving = fresh;
        for band in 0..SPEC_BANDS {
            let target = if fresh { ((self.bands[band] + self.tilt[band] - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0) } else { 0.0 };
            let level = &mut self.level[band];
            *level = if target >= *level { target } else { (*level - FALL * dt).max(target) };
            moving |= *level > 0.0;
        }
        moving
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct HwEqGraph {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_grid: DrawEqGrid,
    #[live]
    draw_spectrum: DrawEqSpectrum,
    #[live]
    draw_fader: DrawEqFader,
    #[live]
    draw_curve: DrawEqCurve,
    /// Invisible quads over the lit faders: their light on the device.
    #[live]
    draw_light: DrawMusicRound,
    #[live]
    draw_label: DrawText,
    /// Band names and the preamp's (rgb, and how much over the muted ink).
    #[live]
    label_ink: Vec4f,
    /// The dB scale's figures.
    #[live]
    scale_ink: Vec4f,
    /// How strongly lit faders light the device (0 = not at all).
    #[live(0.0)]
    light: f64,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    geo: Geo,
    /// The faders as last drawn or dragged: preamp, then bands.
    #[rust]
    values: [f32; FADERS],
    #[rust]
    drag: Option<Drag>,
    #[rust]
    hover: Option<usize>,
    /// The fader the keys move, once the graph has been pressed.
    #[rust]
    focus: Option<usize>,
    /// The bands the curve was worked for, and its points (0..1 down).
    #[rust]
    curve_for: Option<[f32; BANDS]>,
    #[rust]
    curve: Vec<f32>,
    #[rust]
    tex: Option<Texture>,
    #[rust]
    tex_dirty: bool,
    #[rust]
    spectrum: Option<Spectrum>,
}

/// A fader's range, dB.
fn range(fader: usize) -> (f32, f32) {
    if fader == 0 {
        (GAIN_MIN_DB.max(-geq::PRE_DB), GAIN_MAX_DB.min(geq::PRE_DB))
    } else {
        (-geq::RANGE_DB, geq::RANGE_DB)
    }
}

/// A dragged value: tenths of a dB, caught at 0.
fn settle(fader: usize, db: f32) -> f32 {
    let (low, high) = range(fader);
    let db = db.clamp(low, high);
    if db.abs() < 0.35 { 0.0 } else { (db * 10.0).round() / 10.0 }
}

impl HwEqGraph {
    fn values_of(model: &Model) -> [f32; FADERS] {
        let mut values = [0.0; FADERS];
        values[0] = model.eq.gain_db;
        values[1..].copy_from_slice(&model.eq.bands);
        values
    }

    fn emit(&mut self, cx: &mut Cx, done: bool) {
        self.area.redraw(cx);
        cx.widget_action(self.uid, EqGraphAction::Changed { values: self.values, done });
    }

    fn set_value(&mut self, fader: usize, db: f32) -> bool {
        let db = settle(fader, db);
        let changed = self.values[fader] != db;
        self.values[fader] = db;
        changed
    }

    /// Work the curve again if the bands changed.
    fn update_curve(&mut self) {
        let mut bands = [0.0; BANDS];
        bands.copy_from_slice(&self.values[1..]);
        if self.curve_for == Some(bands) {
            return;
        }
        self.curve_for = Some(bands);
        let gains = geq::solve(&bands);
        self.curve = (0..CURVE_POINTS)
            .map(|i| {
                let db = geq::response_db(&gains, geq::axis_hz(i as f32 / (CURVE_POINTS - 1) as f32));
                (0.5 - db / SPAN_DB * 0.5).clamp(-0.2, 1.2)
            })
            .collect();
        self.tex_dirty = true;
    }

    fn upload(&mut self, cx: &mut Cx2d) {
        let n = CURVE_POINTS;
        let tex = self.tex.get_or_insert_with(|| {
            Texture::new_with_format(cx.cx, TextureFormat::VecRf32 { width: n, height: 2, data: Some(vec![0.0; n * 2]), updated: TextureUpdated::Full })
        });
        if self.tex_dirty {
            let mut data = tex.take_vec_f32(cx.cx);
            data.resize(n * 2, 0.0);
            data[..n].copy_from_slice(&self.curve);
            let levels = self.spectrum.as_ref().map(|s| s.level.as_slice()).unwrap_or(&[]);
            for (i, slot) in data[n..].iter_mut().enumerate() {
                *slot = levels.get(i).copied().unwrap_or(0.0);
            }
            tex.put_back_vec_f32(cx.cx, data, None);
            self.tex_dirty = false;
        }
        self.draw_spectrum.draw_vars.set_texture(0, tex);
        self.draw_curve.draw_vars.set_texture(0, tex);
    }

    fn lay_out(&mut self, rect: Rect) -> bool {
        let compact = rect.size.x < 420.0 || rect.size.y < 120.0;
        let label_h = if compact { 13.0 } else { 16.0 };
        let scale_w = if compact { 0.0 } else { 24.0 };
        let top = rect.pos.y + 6.0;
        let height = (rect.size.y - label_h - 8.0).max(20.0);
        // The preamp's column: as wide as a band's, within reason.
        let pre_w = ((rect.size.x - scale_w - 8.0) / FADERS as f64).min(if compact { 30.0 } else { 44.0 });
        let pre = Rect { pos: dvec2(rect.pos.x, top), size: dvec2(pre_w, height) };
        let left = pre.pos.x + pre_w + 8.0 + scale_w;
        let plot = Rect { pos: dvec2(left, top), size: dvec2((rect.pos.x + rect.size.x - left - 2.0).max(40.0), height) };
        let col_w = plot.size.x / BANDS as f64;
        // The cap: as tall as the style asks, smaller on a short panel.
        let cap_h = (height * 0.12).clamp(8.0, 18.0);
        self.geo = Geo { plot, pre, col_w, cap_h };
        compact
    }
}

impl Widget for HwEqGraph {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.area.redraw(cx);
        }
        if !self.visible {
            return;
        }
        let geo = self.geo;
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let hover = geo.fader_at(e.abs.x, false);
                cx.set_cursor(if hover.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hover != self.hover {
                    self.hover = hover;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::FingerDown(e) => {
                self.hover = None;
                if let Some(model) = scope.data_mut().get::<Model>() {
                    self.values = Self::values_of(model);
                }
                cx.set_key_focus(self.area);
                let Some(fader) = geo.fader_at(e.abs.x, false) else { return };
                self.focus = Some(fader);
                if e.tap_count == 2 {
                    // A double press puts it back to 0 dB.
                    self.drag = None;
                    self.set_value(fader, 0.0);
                    self.emit(cx, true);
                    return;
                }
                let cap = geo.y_of(self.values[fader]);
                let on_cap = (e.abs.y - cap).abs() <= geo.cap_h * 0.5 + 4.0;
                if on_cap || fader == 0 {
                    let offset = if on_cap { self.values[fader] - geo.db_at(e.abs.y) } else { 0.0 };
                    self.drag = Some(Drag { fader, paint: false, offset });
                    if !on_cap {
                        self.set_value(fader, geo.db_at(e.abs.y));
                    }
                } else {
                    self.drag = Some(Drag { fader, paint: true, offset: 0.0 });
                    self.set_value(fader, geo.db_at(e.abs.y));
                }
                self.emit(cx, false);
            }
            Hit::FingerMove(e) => {
                let Some(drag) = self.drag else { return };
                let fader = if drag.paint { geo.fader_at(e.abs.x, true).filter(|f| *f > 0).unwrap_or(drag.fader) } else { drag.fader };
                self.focus = Some(fader);
                if self.set_value(fader, geo.db_at(e.abs.y) + drag.offset) {
                    self.emit(cx, false);
                }
            }
            Hit::FingerUp(e) => {
                self.hover = geo.fader_at(e.abs.x, false).filter(|_| e.is_over);
                if self.drag.take().is_some() && !e.cancelled {
                    self.emit(cx, true);
                }
            }
            Hit::FingerScroll(e) => {
                let Some(fader) = geo.fader_at(e.abs.x, false) else { return };
                if let Some(model) = scope.data_mut().get::<Model>() {
                    self.values = Self::values_of(model);
                }
                let db = self.values[fader] - (e.scroll.y as f32) * 0.02;
                let (low, high) = range(fader);
                let db = ((db.clamp(low, high)) * 10.0).round() / 10.0;
                if db != self.values[fader] {
                    self.values[fader] = db;
                    self.focus = Some(fader);
                    self.emit(cx, true);
                }
            }
            Hit::KeyDown(ke) => {
                if let Some(model) = scope.data_mut().get::<Model>() {
                    self.values = Self::values_of(model);
                }
                let at = self.focus.unwrap_or(1);
                let step = if ke.modifiers.shift { 0.1 } else { 0.5 };
                match ke.key_code {
                    KeyCode::ArrowLeft => {
                        self.focus = Some(at.saturating_sub(1));
                        self.area.redraw(cx);
                    }
                    KeyCode::ArrowRight => {
                        self.focus = Some((at + 1).min(FADERS - 1));
                        self.area.redraw(cx);
                    }
                    KeyCode::ArrowUp | KeyCode::ArrowDown => {
                        let sign = if ke.key_code == KeyCode::ArrowUp { 1.0 } else { -1.0 };
                        let (low, high) = range(at);
                        self.values[at] = (((self.values[at] + sign * step).clamp(low, high)) * 10.0).round() / 10.0;
                        self.focus = Some(at);
                        self.emit(cx, true);
                    }
                    KeyCode::Key0 | KeyCode::Backspace | KeyCode::Delete => {
                        self.values[at] = 0.0;
                        self.focus = Some(at);
                        self.emit(cx, true);
                    }
                    _ => {}
                }
            }
            Hit::KeyFocusLost(_) => {
                if self.focus.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let Some(model) = scope.data_mut().get::<Model>() else { return DrawStep::done() };
        if self.drag.is_none() {
            self.values = Self::values_of(model);
        }
        let theme = model.theme;
        let on = model.eq.on;
        let now = cx.seconds_since_app_start();
        let moving = self.spectrum.get_or_insert_with(Spectrum::new).step(model, now);
        if moving {
            self.tex_dirty = true;
        }
        let compact = self.lay_out(rect);
        self.update_curve();
        self.upload(cx);
        let geo = self.geo;
        let accent = [theme.accent.x, theme.accent.y, theme.accent.z, 1.0];
        let text = [theme.foreground.x, theme.foreground.y, theme.foreground.z, 1.0];

        // The grid and the sound, behind the bank.
        let dv = &mut self.draw_grid.draw_vars;
        dv.set_uniform(cx, live_id!(ink_text), &text);
        dv.set_uniform(cx, live_id!(ink_accent), &accent);
        dv.set_uniform(cx, live_id!(db_step), &[6.0 / SPAN_DB]);
        dv.set_uniform(cx, live_id!(bands), &[BANDS as f32]);
        self.draw_grid.draw_abs(cx, geo.plot);
        let dv = &mut self.draw_spectrum.draw_vars;
        dv.set_uniform(cx, live_id!(ink_accent), &accent);
        dv.set_uniform(cx, live_id!(count), &[CURVE_POINTS as f32]);
        dv.set_uniform(cx, live_id!(spec_count), &[SPEC_BANDS as f32]);
        self.draw_spectrum.draw_abs(cx, geo.plot);

        // The faders: the preamp, then the bands.
        let dv = &mut self.draw_fader.draw_vars;
        dv.set_uniform(cx, live_id!(ink_text), &text);
        dv.set_uniform(cx, live_id!(ink_accent), &accent);
        dv.set_uniform(cx, live_id!(scale), &[if compact { 0.7 } else { 1.0 }]);
        let top = geo.y_of(geq::RANGE_DB) - geo.plot.pos.y;
        let bottom = geo.y_of(-geq::RANGE_DB) - geo.plot.pos.y;
        let dragging = self.drag.map(|d| d.fader);
        let mut lights = Vec::new();
        for fader in 0..FADERS {
            let (x, w) = if fader == 0 { (geo.pre.pos.x, geo.pre.size.x) } else { (geo.plot.pos.x + (fader - 1) as f64 * geo.col_w, geo.col_w) };
            let column = Rect { pos: dvec2(x, geo.plot.pos.y), size: dvec2(w, geo.plot.size.y) };
            let cap_y = geo.y_of(self.values[fader]) - geo.plot.pos.y;
            let zero_y = geo.y_of(0.0) - geo.plot.pos.y;
            let f = &mut self.draw_fader;
            f.cap_y = cap_y as f32;
            f.zero_y = zero_y as f32;
            f.top_y = top as f32;
            f.bot_y = bottom as f32;
            f.band = fader as f32 - 1.0;
            f.lit = if on { 1.0 } else { 0.0 };
            f.hover = (self.hover == Some(fader)) as u8 as f32;
            f.active = (dragging == Some(fader)) as u8 as f32;
            f.focus = (self.focus == Some(fader)) as u8 as f32;
            f.draw_abs(cx, column);
            if on && self.values[fader] != 0.0 {
                let (y0, y1) = if cap_y < zero_y { (cap_y, zero_y) } else { (zero_y, cap_y) };
                lights.push(Rect { pos: dvec2(geo.centre_x(fader) - w * 0.2, geo.plot.pos.y + y0), size: dvec2(w * 0.4, (y1 - y0).max(2.0)) });
            }
        }

        // The curve over it all.
        let dv = &mut self.draw_curve.draw_vars;
        dv.set_uniform(cx, live_id!(ink_accent), &accent);
        dv.set_uniform(cx, live_id!(count), &[CURVE_POINTS as f32]);
        dv.set_uniform(cx, live_id!(lit), &[if on { 1.0 } else { 0.0 }]);
        self.draw_curve.draw_abs(cx, geo.plot);

        // The names under the faders, the scale beside the bank.
        let ink = |style: Vec4f| crate::mix(theme.muted, vec4(style.x, style.y, style.z, 1.0), style.w.clamp(0.0, 1.0));
        self.draw_label.color = ink(self.label_ink);
        let label_y = geo.plot.pos.y + geo.plot.size.y + 3.0;
        for fader in 0..FADERS {
            let name = if fader == 0 { "PRE" } else { geq::BAND_NAMES[fader - 1] };
            // Compact: every other band's name.
            if compact && fader > 0 && fader % 2 == 1 {
                continue;
            }
            let width = crate::widgets::text_width(&self.draw_label, cx, name);
            self.draw_label.draw_abs(cx, dvec2(geo.centre_x(fader) - width * 0.5, label_y), name);
        }
        if !compact {
            self.draw_label.color = ink(self.scale_ink);
            let right = geo.plot.pos.x - 4.0;
            for db in [12.0f32, 6.0, 0.0, -6.0, -12.0] {
                let text = if db > 0.0 { format!("+{db:.0}") } else { format!("{db:.0}") };
                let width = crate::widgets::text_width(&self.draw_label, cx, &text);
                self.draw_label.draw_abs(cx, dvec2(right - width, geo.y_of(db) - 6.0), &text);
            }
        }

        // Lit faders light the device, where the style asks.
        if self.light > 0.0 {
            for rect in lights {
                self.draw_light.color = vec4(0.0, 0.0, 0.0, 0.0);
                self.draw_light.radius = 1.0;
                self.draw_light.draw_abs(cx, rect);
                let slot = relief_open(cx, 0.0);
                let emissive = vec4(theme.accent.x, theme.accent.y, theme.accent.z, self.light as f32);
                relief_close(cx, slot, self.draw_light.draw_vars.area, Some(ReliefProxy { inset: 0.0, radius: 1.0, disc: false, emissive }));
            }
        }
        if moving {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

impl HwEqGraphRef {
    /// The faders' values as moved, and whether they were let go.
    pub fn changed(&self, actions: &Actions) -> Option<([f32; FADERS], bool)> {
        let mut out: Option<([f32; FADERS], bool)> = None;
        for action in actions.filter_widget_actions(self.widget_uid()) {
            if let EqGraphAction::Changed { values, done } = action.cast() {
                out = Some((values, done || out.map(|(_, d)| d).unwrap_or(false)));
            }
        }
        out
    }
}

// ---- the panel ----

/// The EQ component: shows fewer of its keys the smaller it is drawn
/// (compact: no title or LED; tiny: no AUTO, SAVE or RESET).
#[derive(Script, ScriptHook, Widget)]
pub struct HwEqPanel {
    #[deref]
    view: View,
}

impl Widget for HwEqPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.peek_walk_turtle(walk);
        // 0 full, 1 compact, 2 tiny: by the room it is given.
        let fit = if rect.size.x < 380.0 || rect.size.y < 170.0 {
            2
        } else if rect.size.x < 600.0 || rect.size.y < 230.0 {
            1
        } else {
            0
        };
        // Set only on change; a restyle puts the template's back, so every draw looks.
        let show = |view: &mut View, cx: &mut Cx2d, id: &[LiveId], on: bool| {
            let widget = view.widget(cx, id);
            if widget.visible() != on {
                widget.set_visible(cx, on);
            }
        };
        show(&mut self.view, cx, ids!(eq_title), fit == 0);
        show(&mut self.view, cx, ids!(eq_led), fit == 0);
        show(&mut self.view, cx, ids!(eq_save_cap), fit != 2);
        // AUTO stays wherever the width holds it (a player's short band too).
        show(&mut self.view, cx, ids!(eq_auto_cap), rect.size.x >= 300.0);
        // Tiny: the Flat preset resets.
        show(&mut self.view, cx, ids!(eq_flat_cap), fit != 2);
        self.view.draw_walk(cx, scope, walk)
    }
}

// ---- the app's side ----

/// The person's presets and what the chooser shows.
#[derive(Default)]
struct EqUi {
    presets: Option<UserPresets>,
    /// The chooser's labels as last set, and for which restyle.
    labels: Vec<String>,
    style_gen: u64,
    /// Whether the layout on screen has an `eq` slot, by layout name.
    slotted: Option<(String, bool)>,
}

const CUSTOM: &str = "Custom";

impl App {
    fn eq_ui(cx: &mut Cx) -> &mut EqUi {
        if !cx.has_global::<EqUi>() {
            cx.set_global(EqUi::default());
        }
        let ui = cx.global::<EqUi>();
        if ui.presets.is_none() {
            ui.presets = Some(UserPresets::load());
        }
        ui
    }

    fn eq_presets(cx: &mut Cx) -> &mut UserPresets {
        Self::eq_ui(cx).presets.get_or_insert_with(UserPresets::load)
    }

    /// Open or close the EQ panel (the EQ key, Cmd+E).
    pub(crate) fn toggle_eq(&mut self) {
        self.model.eq.open = !self.model.eq.open;
        self.save_eq();
        self.model.requests.redraw = true;
    }

    /// The EQ panel from the model: the drawer, the keys' lights, the
    /// preset chooser.
    pub(crate) fn update_eq(&mut self, cx: &mut Cx) {
        // A run left on the old equalizer page opens the panel instead.
        if self.model.page == Page::Equalizer {
            let back = self.before_eq.take().unwrap_or(Page::Search);
            self.model.open(back);
            self.model.eq.open = true;
        }
        let eq = self.model.eq;
        let active = crate::layout::active(cx);
        let ui = Self::eq_ui(cx);
        let slotted = match &ui.slotted {
            Some((name, slotted)) if *name == active => *slotted,
            _ => {
                let slotted = crate::layout::layout_source(&active).map(|s| s.contains("component: \"eq\"")).unwrap_or(false);
                ui.slotted = Some((active.clone(), slotted));
                slotted
            }
        };
        self.ui.hw_stage(cx, ids!(stage)).set_eq_open(cx, eq.open && !slotted);
        self.set_visible(cx, ids!(eq_close_cap), !slotted);
        let lit = |on: bool| if on { 1.0 } else { 0.0 };
        self.ui.relief_view(cx, ids!(eq_page_cap)).set_lit(cx, lit(eq.open || slotted));
        self.ui.relief_view(cx, ids!(eq_auto_cap)).set_lit(cx, lit(eq.auto));

        // The chooser: the built-in presets, the person's, and "Custom"
        // while the faders are off every preset.
        let style_gen = self.model.style_gen;
        let presets = Self::eq_presets(cx).clone();
        let mut labels = presets.names();
        let current = if labels.contains(&presets.current) { presets.current.clone() } else { CUSTOM.to_string() };
        if current == CUSTOM {
            labels.push(CUSTOM.to_string());
        }
        let ui = Self::eq_ui(cx);
        let stale = ui.labels != labels || ui.style_gen != style_gen;
        if stale {
            ui.labels = labels.clone();
            ui.style_gen = style_gen;
        }
        let chooser = self.ui.drop_down(cx, ids!(eq_presets));
        if stale {
            chooser.set_labels(cx, labels.clone());
        }
        if let Some(at) = labels.iter().position(|l| *l == current) {
            if chooser.selected_item() != at {
                chooser.set_selected_item(cx, at);
            }
        }
    }

    /// The faders, keys and chooser of the EQ panel.
    pub(crate) fn eq_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut changed = false;
        let mut save = false;
        if let Some((values, done)) = self.ui.hw_eq_graph(cx, ids!(eq_graph)).changed(actions) {
            let eq = &mut self.model.eq;
            if values[0] != eq.gain_db {
                // A hand on the preamp takes it from AUTO.
                eq.auto = false;
            }
            eq.gain_db = values[0];
            eq.bands.copy_from_slice(&values[1..]);
            Self::eq_presets(cx).current = CUSTOM.to_string();
            changed = true;
            save = done;
        }
        if self.ui.button(cx, ids!(eq_flat)).clicked(actions) {
            self.model.eq.bands = [0.0; BANDS];
            self.model.eq.gain_db = 0.0;
            Self::eq_presets(cx).current = "Flat".to_string();
            changed = true;
            save = true;
        }
        if self.ui.button(cx, ids!(eq_auto)).clicked(actions) {
            self.model.eq.auto = !self.model.eq.auto;
            if !self.model.eq.auto {
                self.model.eq.gain_db = 0.0;
            }
            changed = true;
            save = true;
        }
        // The band's PRESETS key: the same presets as a menu.
        if self.ui.button(cx, ids!(eq_presets_key)).clicked(actions) {
            let presets = Self::eq_presets(cx).clone();
            let rows: Vec<MenuRow> = presets
                .names()
                .iter()
                .enumerate()
                .map(|(at, name)| MenuRow::new(LiveId::from_str(&format!("eqp:{at}")), name).radio(*name == presets.current))
                .collect();
            let anchor = self.ui.widget(cx, ids!(eq_presets_cap)).area().rect(cx);
            self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(eq_preset_menu), rows, anchor, MenuPlace::Below);
        }
        let menu_pick = menu_picked(actions, live_id!(eq_preset_menu))
            .and_then(|id| (0..Self::eq_presets(cx).names().len()).find(|at| LiveId::from_str(&format!("eqp:{at}")) == id));
        if let Some(at) = self.ui.drop_down(cx, ids!(eq_presets)).selected(actions).or(menu_pick) {
            let presets = Self::eq_presets(cx);
            let names = presets.names();
            if let Some((pre, bands)) = names.get(at).and_then(|name| presets.find(name)) {
                presets.current = names[at].clone();
                self.model.eq.bands = bands;
                if !self.model.eq.auto {
                    self.model.eq.gain_db = pre;
                }
                changed = true;
                save = true;
            }
        }
        if self.ui.button(cx, ids!(eq_save)).clicked(actions) {
            let (pre, bands) = (self.model.eq.gain_db, self.model.eq.bands);
            let presets = Self::eq_presets(cx);
            let name = presets.store(pre, bands);
            presets.save();
            log!("amp: EQ preset {name} saved");
            self.model.requests.redraw = true;
        }
        if self.ui.button(cx, ids!(eq_close)).clicked(actions) {
            self.model.eq.open = false;
            self.save_eq();
            self.model.requests.redraw = true;
        }
        if changed {
            if self.model.eq.auto {
                self.model.eq.gain_db = geq::auto_preamp(&self.model.eq.bands).max(GAIN_MIN_DB);
            }
            if let Some(knobs) = self.knobs() {
                knobs.apply(&self.model.eq);
            }
            if save {
                self.save_eq();
                let presets = Self::eq_presets(cx).clone();
                presets.save();
            }
            self.model.requests.redraw = true;
        }
    }
}
