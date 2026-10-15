//! The hardware: Music as a device. Every moulded part is a template here
//! (`mod.widgets.Hw*`), and a *style* — one hotloadable Splash file in
//! `resources/styles/<name>.splash` — restyles them: the chassis finish, the
//! keys, the knobs, the screen bezel and glass, the LEDs, the label inks and
//! the palette the lists and the spectrum paint with.
//!
//! How a style reaches the screen: `App::script_mod` evaluates these base
//! templates, then the style's file (read from disk in a source checkout, so
//! editing it needs no rebuild), then the app's own layout, which names the
//! templates. Changing the style — the STYLE key on the device, `--style
//! <name>` at start, or the file changing on disk while it is watched —
//! asks for a style reload, which runs that sequence again and reapplies the
//! widget tree in place (`Apply::ScriptReapply`): the app keeps its state.
//!
//! The parts light each other through the window's relief buffer
//! (`makepad_widgets::relief`): the screen's picture is a texture light, the
//! LEDs are emissive surfaces, and every surface here binds the buffer.
//!
//! See `local/agent_state/skeuomorph/music/STYLE_GUIDE.md` for the hooks a
//! style fills.

use crate::model::Model;
use crate::widgets::MusicBarAction;
use crate::Theme;
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // ---- palette ----
    // Read by Rust after the style runs (`palette`): the lists, the spectrum,
    // the bars and the album well paint with these.
    mod.widgets.hw_palette = {
        // Lists and the display: their glass, text and secondary text.
        screen_bg: #x0e1012
        screen_text: #xe6e8ea
        screen_muted: #x868c95
        // Selection, the playing row, the spectrum's bars, lit marks.
        accent: #x39c6ff
        // Type on recessed panels (headings, the sidebar's buttons, the
        // presets tree): dark on a light floor, light on a dark one.
        panel_text: #x2b2e33
        panel_muted: #x5a5e66
        // The popup menus (the style picker, the row menus).
        menu_bg: #x1b1d20
        // Neighbour light, style-wide: graded toward `spill_color` by
        // `spill_mono` (0 keeps each light's own colour, 1 = monochrome),
        // eased under `spill_knee` (0 = no ceiling).
        spill_color: #xffffff
        spill_mono: 0.0
        spill_knee: 0.0
        // How much neighbour light a black surface takes against a white
        // one's 1: dark bezels are not washed out by a white screen.
        spill_dark: 0.5
    }

    // ---- surfaces ----
    // One moulded surface with a procedural finish. Every Hw part derives
    // from it; a style sets `pattern*`, `sheen` and the inks, or replaces
    // `surface` (the flat colour of the face before lighting) outright.
    mod.widgets.HwSurface = ReliefView{
        draw_bg +: {
            light_ink: vec4(1.0, 1.0, 1.0, 1.0)
            shadow_ink: vec4(0.02, 0.025, 0.03, 1.0)
            glow_ink: vec4(0.22, 0.78, 1.0, 1.0)
            ground: #x2a2d31

            /** finish: kind (0 plain, 1 brushed, 2 wood, 3 lacquer band, 4 hammertone, 5 grain, 6 plates and screws, 7 hazard stripes), scale, strength, anisotropy */
            pattern: uniform(vec4(0.0, 1.0, 0.0, 0.0))
            /** second colour of the finish (grain lines, stripes, seams) and its amount */
            pattern_ink: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            /** plates: seam spacing x, y (pt), seam width, screw radius; stripes: period, width of the edge band */
            pattern_seam: uniform(vec4(160.0, 120.0, 1.0, 2.5))
            /** reflected sheen: strength, width (0..1 of the face), position (0..1 across), tilt */
            sheen: uniform(vec4(0.0, 0.3, 0.35, 0.25))
            /** which plate or panel this is (see the style guide's plate table), 0 = none */
            plate_id: instance(0.0)

            // Value noise from a world position, for the finishes.
            hw_hash: fn(q: vec2) -> float {
                return fract(sin(dot(q, vec2(12.9898, 78.233))) * 43758.5453)
            }
            hw_noise: fn(q: vec2) -> float {
                let i = floor(q)
                let f = fract(q)
                let u = f * f * (vec2(3.0, 3.0) - f * 2.0)
                let a = self.hw_hash(i)
                let b = self.hw_hash(i + vec2(1.0, 0.0))
                let c = self.hw_hash(i + vec2(0.0, 1.0))
                let d = self.hw_hash(i + vec2(1.0, 1.0))
                return mix(mix(a, b, u.x), mix(c, d, u.x), u.y)
            }

            // The face's flat colour at `p` (points in the quad) before any
            // lighting. `w` is the window position, so neighbouring parts
            // share one continuous finish.
            surface: fn(p: vec2, base: vec3) -> vec3 {
                let kind = self.pattern.x
                let sc = max(self.pattern.y, 0.001)
                let k = self.pattern.z
                let w = self.world.xy
                var col = base
                if kind > 0.5 && kind < 1.5 {
                    // Brushed: fine streaks along x, varying row by row.
                    let row = floor(w.y * 1.5 / sc)
                    let streak = self.hw_noise(vec2(w.x * 0.012 / sc + row * 7.13, row * 0.37))
                    let fine = self.hw_hash(vec2(row, floor(w.x * 0.25 / sc)))
                    let s = (streak - 0.5) * 0.8 + (fine - 0.5) * 0.25 * self.pattern.w
                    col = col * (1.0 + s * k)
                }
                if kind > 1.5 && kind < 2.5 {
                    // Wood: warped rings along the length, darker late wood.
                    let q = w / sc
                    let warp = self.hw_noise(q * vec2(0.004, 0.02)) * 18.0 + self.hw_noise(q * vec2(0.02, 0.1)) * 2.0
                    let ring = fract((q.y + warp) * 0.11)
                    let late = smoothstep(0.55, 0.95, ring) * (0.6 + 0.4 * self.hw_noise(q * vec2(0.05, 0.5)))
                    let pores = step(0.93, self.hw_hash(floor(q * vec2(0.5, 3.0)))) * 0.35
                    col = mix(col, self.pattern_ink.xyz, clamp((late + pores) * k * self.pattern_ink.w, 0.0, 1.0))
                }
                if kind > 3.5 && kind < 4.5 {
                    // Hammertone: soft cells.
                    let n = self.hw_noise(w * 0.35 / sc) * 0.6 + self.hw_noise(w * 0.9 / sc) * 0.4
                    col = col * (1.0 + (n - 0.5) * k)
                }
                if kind > 4.5 && kind < 5.5 {
                    // Fine grain.
                    col = col * (1.0 + (self.hw_hash(floor(w * 2.0 / sc)) - 0.5) * k)
                }
                if kind > 5.5 && kind < 6.5 {
                    // Plates: seams on a grid and a screw near each corner.
                    let cell = vec2(max(self.pattern_seam.x, 8.0), max(self.pattern_seam.y, 8.0))
                    let f = fract(w / cell)
                    let e = min(min(f.x, 1.0 - f.x) * cell.x, min(f.y, 1.0 - f.y) * cell.y)
                    let seam = 1.0 - smoothstep(0.0, self.pattern_seam.z + 0.5, e)
                    let corner = min(f, vec2(1.0, 1.0) - f) * cell
                    let screw = 1.0 - smoothstep(self.pattern_seam.w - 0.6, self.pattern_seam.w + 0.4, length(corner - vec2(9.0, 9.0)))
                    col = mix(col, self.pattern_ink.xyz, clamp(seam * self.pattern_ink.w, 0.0, 1.0))
                    col = mix(col, col * 0.62, screw * k)
                    col = col * (1.0 + (self.hw_noise(w * 0.4 / sc) - 0.5) * 0.08)
                }
                if kind > 6.5 && kind < 7.5 {
                    // Hazard stripes in a band round the edge.
                    let band = self.pattern_seam.y
                    let e = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y)) - self.inset
                    let inb = 1.0 - smoothstep(band - 0.5, band + 0.5, e)
                    let st = step(0.5, fract((w.x + w.y) / max(self.pattern_seam.x, 2.0)))
                    col = mix(col, self.pattern_ink.xyz, inb * st * self.pattern_ink.w)
                }
                // The sheen: a soft band of reflected light across the face.
                if self.sheen.x > 0.001 {
                    let uv = p / max(self.rect_size, vec2(1.0, 1.0))
                    let t = uv.x + (uv.y - 0.5) * self.sheen.w - self.sheen.z
                    let band = exp(-(t * t) / max(self.sheen.y * self.sheen.y, 0.0001))
                    col = col + self.light_ink.xyz * band * self.sheen.x
                }
                if kind > 2.5 && kind < 3.5 {
                    // Lacquer: a sharp reflection band in window space.
                    let t = (w.x * 0.35 + w.y) / max(sc * 300.0, 1.0)
                    let b = smoothstep(0.0, 0.02, fract(t) - 0.42) * (1.0 - smoothstep(0.0, 0.1, fract(t) - 0.5))
                    col = col + self.light_ink.xyz * b * k
                }
                return col
            }

            pixel: fn() {
                return self.layer(self.surface(self.pos * self.rect_size, self.ground.xyz))
            }
        }
    }

    // The whole device: the chassis the panels are set into.
    mod.widgets.HwChassis = mod.widgets.HwSurface{
        width: Fill
        height: Fill
        proxy: false
        draw_bg +: {
            // A large face: a whisper of face gradient, no hairline.
            tune: vec4(0.10, 0.0, 1.10, 3.0)
            /** the front's falloff: centre x, y (0..1), darkening at the edges, brightness at the centre */
            vignette: uniform(vec4(0.5, 0.35, 0.22, 1.08))
            pixel: fn() {
                let p = self.pos * self.rect_size
                var col = self.surface(p, self.ground.xyz)
                // A soft top-lit falloff over the whole front.
                let v = length((self.pos - self.vignette.xy) * vec2(0.9, 1.3))
                col = col * (self.vignette.w - self.vignette.z * v * v)
                col = self.spill_apply(col * (1.0 - self.env_ao(0.0)), self.env_spill(vec3(0.0, 0.0, 1.0)))
                return vec4(col + self.dither(), 1.0)
            }
        }
    }

    // A recessed panel: a shallow step down into the chassis.
    mod.widgets.HwPanel = mod.widgets.HwSurface{
        depth: -2.5
        radius: 10
        draw_bg +: {
            inner: vec4(0.85, 9.0, 0.0, 0.55)
            tune: vec4(0.12, 0.30, 1.10, 3.0)
        }
    }

    // A raised plate: a group of controls standing off the chassis.
    mod.widgets.HwPlate = mod.widgets.HwSurface{
        depth: 2.0
        inset: 6
        radius: 10
        draw_bg +: {
            tune: vec4(0.18, 0.40, 1.10, 3.0)
        }
    }

    // A wing either side of the screen: a raised plate whose `padding`
    // keeps its contents inside the style's decorative border.
    mod.widgets.HwWing = mod.widgets.HwPlate{
        padding: Inset{left: 14 right: 14 top: 16 bottom: 14}
    }

    // The screen's surround: a deep well round the picture.
    mod.widgets.HwBezel = mod.widgets.HwSurface{
        depth: -4.0
        radius: 14
        draw_bg +: {
            ground: #x17191c
            inner: vec4(0.95, 12.0, 0.0, 0.55)
        }
    }

    // Over the picture: the screen's glass. Scanlines, a reflection, a
    // darker rim; `glass` = scanline strength, reflection, rim darkening,
    // curvature (a CRT's bulge darkens its corners).
    mod.widgets.HwGlass = View{
        width: Fill
        height: Fill
        show_bg: true
        draw_bg +: {
            glass: uniform(vec4(0.0, 0.10, 0.0, 0.0))
            tint: uniform(vec4(1.0, 1.0, 1.0, 0.0))
            radius: uniform(8.0)
            /** 1 while the screen filter (THEME COLOURS) is on: what a style draws over the picture itself (scanlines, a pixel grid, a phosphor wash, marks) is multiplied by it, so the picture is clean with the filter off; the rim and the reflection stay */
            filter_on: uniform(0.0)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let d = Material.sd_box(p, c, c, self.radius)
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                // Scanlines darken, as alpha over the picture (with the filter).
                let line = 0.5 + 0.5 * sin(p.y * 3.14159 * self.draw_pass.dpi_factor)
                var dark = line * self.glass.x * self.filter_on
                // The rim and, for a curved tube, the corners.
                dark = dark + (1.0 - smoothstep(-26.0, 0.0, d) ) * 0.0
                let edge = smoothstep(-30.0, 0.0, d) * self.glass.z
                let q = (self.pos - vec2(0.5, 0.5)) * 2.0
                let bulge = clamp(dot(q * q, q * q) * self.glass.w, 0.0, 1.0)
                dark = clamp(dark + edge + bulge, 0.0, 1.0)
                // The reflection: a soft diagonal sheet in the top half.
                let r = smoothstep(0.62, 0.0, self.pos.y + (self.pos.x - 0.5) * 0.25) * self.glass.y
                let cov = 1.0 - smoothstep(-px, px, d)
                let o = vec4(self.tint.xyz * r, r) * cov + vec4(0.0, 0.0, 0.0, dark * cov) * (1.0 - r)
                // A tinted tube (amber, green) multiplies the picture.
                return vec4(o.xyz, min(o.w + self.tint.w * cov * 0.0, 1.0))
            }
        }
    }

    // A key: a moulded cap round a flat button. Pressing the button
    // presses the cap in.
    mod.widgets.HwKey = mod.widgets.HwSurface{
        width: 60
        height: 52
        inset: 7
        radius: 6
        depth: 3.0
        press: -3.5
        press_secs: 0.06
        latch: 0.0
        follow_press: true
        flow: Overlay
        align: Align{x: 0.5 y: 0.5}
        emissive: vec4(0.22, 0.78, 1.0, 1.0)
        draw_bg +: {
            ground: #x303338
            // Lit (a switch that is on): a faint glow on the face.
            inner: vec4(0.72, 9.0, 0.0, 0.12)
            /** the key's own LED, on keys with `has_led`: x, y from the face's top-left (pt), radius (pt), 1 = LEDs shown in this style */
            led: uniform(vec4(8.0, 8.0, 2.0, 1.0))
            led_off: uniform(vec4(0.12, 0.12, 0.13, 1.0))
            /** a critical key's marking: rgb, amount (0 = none in this style) */
            critical_ink: uniform(vec4(1.0, 0.78, 0.08, 0.0))
            /** kind (0 tint, 1 hazard band round the edge), band width (pt), stripe period (pt) */
            critical_band: uniform(vec4(1.0, 4.0, 7.0, 0.0))
            // Per key, from the layout.
            has_led: instance(0.0)
            critical: instance(0.0)
            /** which key this is (see the style guide's role table) */
            role: instance(0.0)
            /** a per-key tint: rgb, amount */
            key_ink: instance(vec4(0.0, 0.0, 0.0, 0.0))

            /** a mechanical key (a tape deck's piano key): on, the top face's share of the slot at rest, dimple radius (0..1 of the room), label-window height (pt, 0 = none). Its drop is the widget's `travel` */
            mech: uniform(vec4(0.0, 0.62, 0.8, 0.0))
            housing_ink: uniform(vec4(0.05, 0.05, 0.06, 1.0))
            front_ink: uniform(vec4(0.55, 0.56, 0.58, 1.0))
            window_ink: uniform(vec4(0.11, 0.11, 0.12, 1.0))
            /** the dimple: kind (0 none, 1 spun metal), ring pitch, rim darkness, sheen */
            dimple: uniform(vec4(1.0, 3.0, 0.35, 0.12))

            // A mechanical key in its slot: the housing, the key's front
            // face (shorter as it drops), its top with the label window and
            // the spun dimple, all moved down by `held * travel`.
            mech_key: fn() -> vec4 {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let x0 = self.inset
                let x1 = self.rect_size.x - self.inset
                let y0 = self.inset
                let y1 = self.rect_size.y - self.inset
                let slot = Material.sd_box(p, self.rect_size * 0.5, vec2((x1 - x0) * 0.5, (y1 - y0) * 0.5), min(self.radius, 3.0))
                let cov = 1.0 - smoothstep(-px, px, slot)
                // The housing: darker toward the back, where the key hides it.
                var col = self.housing_ink.xyz * (0.55 + 0.45 * smoothstep(y0, y1, p.y))
                let drop = self.held * self.travel
                let kx0 = x0 + 1.0
                let kx1 = x1 - 1.0
                let top_h = (y1 - y0) * self.mech.y
                let yt = y0 + drop
                let yb = yt + top_h
                let inx = smoothstep(kx0 - px, kx0 + px, p.x) * (1.0 - smoothstep(kx1 - px, kx1 + px, p.x))
                let side = 1.0 - 0.3 * (1.0 - smoothstep(0.0, 3.0, min(p.x - kx0, kx1 - p.x)))
                // The front face, from the top's edge down to the lip.
                let front_m = smoothstep(yb - px, yb + px, p.y) * (1.0 - smoothstep(y1 - px, y1 + px, p.y)) * inx
                let ft = clamp((p.y - yb) / max(y1 - yb, 1.0), 0.0, 1.0)
                var front = self.front_ink.xyz * mix(1.0, 0.55, ft) * side
                front = front + self.light_ink.xyz * 0.35 * (1.0 - smoothstep(0.0, 1.5, p.y - yb))
                front = self.spill_apply(front, self.env_spill(vec3(0.0, 0.8, 0.6)) * 0.6)
                col = mix(col, front, front_m)
                // The top face.
                let top_m = smoothstep(yt - px, yt + px, p.y) * (1.0 - smoothstep(yb - px, yb + px, p.y)) * inx
                var top = self.key_face(p, self.ground.xyz) * mix(1.08, 0.94, clamp((p.y - yt) / max(top_h, 1.0), 0.0, 1.0)) * side
                top = mix(top, self.light_ink.xyz, 0.35 * (1.0 - smoothstep(0.0, 1.2, p.y - yt)))
                // The label window along its back edge.
                let wh = self.mech.w
                var room_top = yt + 2.0
                if wh > 0.5 {
                    let wy0 = yt + 2.5
                    let wy1 = wy0 + wh
                    let win = smoothstep(wy0 - px, wy0 + px, p.y) * (1.0 - smoothstep(wy1 - px, wy1 + px, p.y)) * smoothstep(kx0 + 4.0 - px, kx0 + 4.0 + px, p.x) * (1.0 - smoothstep(kx1 - 4.0 - px, kx1 - 4.0 + px, p.x))
                    top = mix(top, self.window_ink.xyz, win)
                    room_top = wy1 + 1.0
                }
                // The dimple, below the window.
                if self.dimple.x > 0.5 {
                    let dc = vec2((kx0 + kx1) * 0.5, (room_top + yb) * 0.5)
                    let dr = max(min(kx1 - kx0 - 8.0, yb - room_top - 3.0) * 0.5 * self.mech.z, 0.0)
                    let dq = p - dc
                    let dl = length(dq)
                    let lt = normalize(self.light.xy + vec2(0.000001, 0.000001))
                    let inside = 1.0 - smoothstep(dr - px, dr + px, dl)
                    let rings = 0.5 + 0.5 * sin(dl * self.dimple.y)
                    let shade = dot(dq / max(dr, 1.0), lt) * 0.2
                    let aniso = 0.5 + 0.5 * cos((atan2(dq.x, -dq.y) - atan2(lt.x, -lt.y)) * 2.0)
                    var dim = top * (0.88 + 0.08 * rings + shade) + self.light_ink.xyz * aniso * self.dimple.w
                    dim = dim * (1.0 - self.dimple.z * (1.0 - smoothstep(0.0, 1.4, dr - dl)))
                    top = mix(top, dim, inside)
                }
                top = self.spill_apply(top, self.env_spill(vec3(0.0, 0.0, 1.0)))
                col = mix(col, top, top_m)
                // The gap the drop opens above the key: a shadow into the housing.
                let gap = smoothstep(y0, yt + 0.5, p.y) * (1.0 - smoothstep(yt - 0.5, yt + 0.5, p.y)) * inx
                col = col * (1.0 - 0.5 * gap * self.held)
                var o = vec4(col * cov, cov)
                o = self.key_marks(o, p)
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }

            // The key's face colour: the finish, and a critical key's marking.
            key_face: fn(p: vec2, base: vec3) -> vec3 {
                var c = self.surface(p, base)
                c = mix(c, self.key_ink.xyz, clamp(self.key_ink.w, 0.0, 1.0))
                if self.critical > 0.5 && self.critical_ink.w > 0.001 {
                    if self.critical_band.x < 0.5 {
                        c = mix(c, self.critical_ink.xyz, self.critical_ink.w * 0.4)
                    } else {
                        let e = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y)) - self.inset
                        let inb = 1.0 - smoothstep(self.critical_band.y - 0.5, self.critical_band.y + 0.5, e)
                        let w = self.world.xy
                        let st = step(0.5, fract((w.x + w.y) / max(self.critical_band.z, 2.0)))
                        c = mix(c, mix(self.critical_ink.xyz, vec3(0.05, 0.05, 0.04), st), inb * self.critical_ink.w)
                    }
                }
                return c
            }

            // The key's LED over the lit face: follows `lit`.
            key_marks: fn(o: vec4, p: vec2) -> vec4 {
                if self.led.w < 0.5 || self.has_led < 0.5 {
                    return o
                }
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let lp = vec2(self.inset + self.led.x, self.inset + self.led.y)
                let d = length(p - lp) - self.led.z
                let core = 1.0 - smoothstep(-px, px, d)
                let on = clamp(self.lit, 0.0, 1.0)
                let col = mix(self.led_off.xyz, mix(self.glow_ink.xyz, vec3(1.0, 1.0, 1.0), 0.3), on)
                let r = vec4(mix(o.xyz, col, core * o.w), o.w)
                let halo = exp(-max(d, 0.0) / 2.2) * 0.5 * (1.0 - core) * on
                return vec4(r.xyz + self.glow_ink.xyz * halo * r.w, r.w)
            }

            pixel: fn() {
                if self.mech.x > 0.5 {
                    return self.mech_key()
                }
                let p = self.pos * self.rect_size
                var o = self.layer(self.key_face(p, self.ground.xyz))
                o = self.key_marks(o, p)
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    // The transport keys (play, previous, next, shuffle, repeat, radio):
    // a key a style may make mechanical — `draw_bg.mech`, a `travel` in
    // points, `latch` 1 to hold it down while its function is on.
    mod.widgets.HwTransportKey = mod.widgets.HwKey{
        latch: 0.0
    }
    // The play key: the widest transport key, as tall as the others (a
    // row of keys shares its top edge and height).
    mod.widgets.HwPlayKey = mod.widgets.HwTransportKey{
        width: 96
        height: 52
    }

    // The button inside a key: only its label shows.
    mod.widgets.HwKeyButton = ButtonFlat{
        width: Fill
        height: Fill
        padding: 0
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            pixel: fn() {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
        }
        draw_text +: {
            color: #x2c2e33
            color_hover: #x2c2e33
            color_down: #x2c2e33
            color_focus: #x2c2e33
            text_style: theme.font_icons{font_size: 13.0}
            // A key's legend is printed on it: focus does not recolour it.
            get_color: fn() {
                let ink = self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                return vec4(ink.rgb, ink.a * (1.0 - self.loading))
            }
        }
    }

    // The legend on a transport key (a style may move it into a
    // mechanical key's label window: `align`, `padding`, `draw_text`).
    mod.widgets.HwTransportLegend = mod.widgets.HwKeyButton{}
    // The play key's larger legend.
    mod.widgets.HwPlayLegend = mod.widgets.HwTransportLegend{
        draw_text +: {text_style: theme.font_icons{font_size: 17.0}}
    }

    // A button in the window's caption (STYLE, LAYOUT, AI, PARTY): the same
    // clean pill in every style, like a toolbar button: a hairline outline
    // in the caption's ink, tinted by the style's accent under the pointer,
    // while held and while its function is on (`lit`, with a small dot).
    // Only the colours follow the style (caption_text, accent).
    mod.widgets.HwCaptionButton = ButtonFlat{
        width: Fit
        height: 24
        margin: 0
        padding: Inset{left: 13 right: 13}
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            ink: uniform(vec4(0.84, 0.86, 0.90, 1.0))
            accent: uniform(vec4(0.30, 0.72, 1.0, 1.0))
            lit: instance(0.0)
            pixel: fn() {
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let h = self.rect_size.y - 1.0
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, h, h * 0.5)
                let tint = clamp(self.hover * 0.14 + self.down * 0.22 + self.lit * 0.16, 0.0, 0.4)
                sdf.fill_keep(vec4(self.accent.xyz * tint, tint))
                let edge = mix(vec4(self.ink.xyz, 0.30), vec4(self.accent.xyz, 0.95), clamp(max(self.hover, self.lit), 0.0, 1.0))
                sdf.stroke(edge, px)
                // The lit dot, left of the word.
                let d = length(self.pos * self.rect_size - vec2(7.0, self.rect_size.y * 0.5)) - 2.2
                let dot = (1.0 - smoothstep(-px, px, d)) * self.lit
                return mix(sdf.result, vec4(self.accent.xyz, 1.0), dot)
            }
        }
        draw_text +: {
            color: #xd6d9de
            color_hover: #xffffff
            color_down: #xffffff
            color_focus: #xd6d9de
            text_style: theme.font_regular{font_size: 10.5}
        }
    }

    // The same with a vector glyph on it (an SVG), inked like the legends:
    // `palette_defaults` gives it the style's legend colour.
    // A ring round it while Tab has it (the legends never recolour).
    mod.widgets.HwKeyIcon = mod.widgets.HwKeyButton{
        text: ""
        spacing: 0
        icon_walk: Walk{width: 13 height: 13}
        draw_icon +: {color: #x2c2e33}
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, 4.0)
                sdf.stroke(vec4(0.30, 0.72, 1.0, self.focus), 1.5)
                return sdf.result
            }
        }
    }

    // The same with a word on it.
    mod.widgets.HwKeyText = mod.widgets.HwKeyButton{
        draw_text +: {
            text_style: theme.font_bold{font_size: 9.0}
        }
    }

    // An indicator dash under a key: dark when off, lit and glowing on.
    mod.widgets.HwDash = mod.widgets.HwSurface{
        emissive_from_glow: true
        width: 44
        height: 20
        inset: 7
        radius: 2
        depth: 0.3
        emissive: vec4(0.22, 0.78, 1.0, 7.0)
        draw_bg +: {
            ground: #x0b0c0d
            inner: vec4(0.72, 9.0, 0.0, 0.24)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                var o = self.layer(self.ground.xyz)
                if self.lit > 0.001 {
                    let d = self.sd(p) + 0.4
                    let core = (1.0 - smoothstep(-px, px, d)) * self.lit
                    let col = mix(self.glow_ink.xyz * 1.15, vec3(0.95, 1.0, 1.0), 0.35)
                    o = vec4(col * core, core) + o * (1.0 - core)
                    let edge = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
                    let bloom = exp(-max(self.sd(p), 0.0) / 4.0) * 0.55 * self.lit * smoothstep(0.0, self.inset, edge) * (1.0 - core)
                    o = o + vec4(self.glow_ink.xyz * bloom, bloom) * (1.0 - o.w)
                }
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    // A round LED.
    mod.widgets.HwLed = mod.widgets.HwDash{
        width: 22
        height: 22
        inset: 7
        disc: true
        emissive: vec4(0.22, 0.78, 1.0, 3.0)
    }

    // A knob's face: a turned disc, a pointer at `value`, and an optional
    // LED ring round it. `knob` = kind (0 dome, 1 skirted with knurling,
    // 2 pointer bar, 3 spun metal cap), cap radius (0..1 of the knob),
    // knurl count, knurl depth. `ring` = LED count (0 none), ring radius
    // (x knob radius), dot radius (pt), sweep (radians each side).
    mod.widgets.HwKnobFace = mod.widgets.HwSurface{
        emissive_from_glow: true
        width: Fill
        height: Fill
        inset: 26
        disc: true
        depth: 9.0
        draw_bg +: {
            ground: #x2b2e33
            shadow: vec4(0.85, 9.0, 1.0, 0.8)
            // Its light is the LED ring's, not a glowing face.
            inner: vec4(0.72, 9.0, 0.0, 0.0)
            knob: uniform(vec4(1.0, 0.72, 48.0, 0.35))
            cap_ink: uniform(vec4(0.20, 0.21, 0.23, 1.0))
            ptr_ink: uniform(vec4(0.95, 0.97, 1.0, 1.0))
            ring: uniform(vec4(0.0, 1.32, 2.6, 2.35619))
            led_off: uniform(vec4(0.05, 0.05, 0.06, 1.0))
            value: uniform(0.5)

            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let q = p - c
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let r = min(c.x, c.y) - self.inset
                let kind = self.knob.x
                // The body: the moulded disc with its cast shadow.
                var o = self.layer(self.surface(p, self.ground.xyz))
                let rq = length(q)
                let ang = atan2(q.x, -q.y)
                let inside = 1.0 - smoothstep(-px, px, rq - r)
                var face = o.xyz
                if kind > 0.5 && kind < 1.5 {
                    // Knurled skirt, a cap stepped in.
                    let cr = r * self.knob.y
                    let skirt = smoothstep(cr - px, cr + px, rq) * inside
                    let ridge = 0.5 + 0.5 * cos(ang * self.knob.z)
                    face = face * (1.0 - skirt * ridge * self.knob.w)
                    let capm = 1.0 - smoothstep(cr - px, cr + px, rq)
                    let lt = normalize(self.light.xy + vec2(0.000001, 0.000001))
                    let lip = (1.0 - smoothstep(0.0, 2.5, abs(rq - cr))) * clamp(dot(q / max(rq, 0.001), lt), -1.0, 1.0)
                    let capcol = self.surface(p, self.cap_ink.xyz) * (1.0 + dot(q / max(r, 1.0), -lt) * 0.18)
                    face = mix(face, capcol, capm * self.cap_ink.w)
                    face = face + self.light_ink.xyz * max(lip, 0.0) * 0.25 - vec3(0.2, 0.2, 0.2) * max(-lip, 0.0) * 0.5
                }
                if kind > 2.5 {
                    // Spun metal: concentric rings on the cap.
                    let cr = r * self.knob.y
                    let capm = 1.0 - smoothstep(cr - px, cr + px, rq)
                    let spun = 0.5 + 0.5 * sin(rq * 2.2 + self.hw_noise(vec2(rq * 0.5, 0.0)) * 3.0)
                    let lt = normalize(self.light.xy + vec2(0.000001, 0.000001))
                    let aniso = 0.5 + 0.5 * cos((ang - atan2(lt.x, -lt.y)) * 2.0)
                    face = mix(face, self.cap_ink.xyz * (0.85 + 0.3 * spun) + self.light_ink.xyz * aniso * 0.25, capm * self.cap_ink.w)
                }
                // The pointer at the value, over a 270 degree sweep.
                let spin = (self.value * 2.0 - 1.0) * self.ring.w
                let dir = vec2(sin(spin), -cos(spin))
                var pd = 0.0
                if kind > 1.5 && kind < 2.5 {
                    // A bar from the centre to the rim.
                    let t = clamp(dot(q, dir), 0.0, r * 0.92)
                    pd = length(q - dir * t) - max(r * 0.09, 1.5)
                } else {
                    let len0 = r * 0.30
                    let len1 = r * 0.82
                    let t = clamp(dot(q, dir), len0, len1)
                    pd = length(q - dir * t) - max(r * 0.045, 1.0)
                }
                face = mix(face, self.ptr_ink.xyz, (1.0 - smoothstep(-px, px, pd)) * inside)
                o = vec4(face * inside, inside) + vec4(o.xyz, o.w) * (1.0 - inside)
                // The LED ring.
                let n = self.ring.x
                if n > 0.5 {
                    let rr = r * self.ring.y
                    let a0 = -self.ring.w
                    let stepa = (self.ring.w * 2.0) / max(n - 1.0, 1.0)
                    let fi = clamp(floor((ang - a0) / stepa + 0.5), 0.0, n - 1.0)
                    let a = a0 + fi * stepa
                    let dc = q - vec2(sin(a), -cos(a)) * rr
                    let dd = length(dc) - self.ring.z
                    let on = step((fi + 0.5) / n, self.value + 0.001)
                    let dot_a = 1.0 - smoothstep(-px, px, dd)
                    let col = mix(self.led_off.xyz, mix(self.glow_ink.xyz, vec3(1.0, 1.0, 1.0), 0.3), on)
                    o = vec4(col * dot_a, dot_a) + o * (1.0 - dot_a)
                    let halo = exp(-max(dd, 0.0) / 3.0) * 0.6 * on * (1.0 - dot_a)
                    o = o + vec4(self.glow_ink.xyz * halo, halo) * (1.0 - o.w)
                }
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    mod.widgets.HwKnobBase = #(HwKnob::register_widget(vm))
    mod.widgets.HwKnob = set_type_default() do mod.widgets.HwKnobBase{
        width: 140
        height: 140
        flow: Overlay
        face := mod.widgets.HwKnobFace{}
    }

    // A level meter: `meter` = kind (0 LED ladders, 1 needle),
    // segments, lit colour mix to the peak ink, needle length.
    // `level` (set every frame while sound moves) = left, right, the held
    // peak and a needle on VU ballistics, all 0..1: a style's own `pixel`
    // reads these for dials, magic eyes, Nixie or 7-segment readouts.
    mod.widgets.HwMeterFace = mod.widgets.HwSurface{
        emissive_from_glow: true
        width: Fill
        height: Fill
        depth: -2.0
        radius: 6
        emissive: vec4(0.22, 0.78, 1.0, 0.5)
        draw_bg +: {
            ground: #x0c0d0f
            inner: vec4(0.72, 9.0, 0.0, 0.0)
            meter: uniform(vec4(0.0, 18.0, 0.8, 0.8))
            hot_ink: uniform(vec4(1.0, 0.35, 0.2, 1.0))
            led_off: uniform(vec4(0.06, 0.06, 0.07, 1.0))
            dial_ink: uniform(vec4(0.95, 0.85, 0.6, 1.0))
            level: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                var o = self.layer(self.surface(p, self.ground.xyz))
                let pad = 8.0
                let a = vec2(pad, pad)
                let b = self.rect_size - vec2(pad, pad)
                let u = (p - a) / max(b - a, vec2(1.0, 1.0))
                let inside = step(0.0, u.x) * step(u.x, 1.0) * step(0.0, u.y) * step(u.y, 1.0)
                let kind = self.meter.x
                if kind < 0.5 {
                    // Two ladders, one row each (left over right), lit from
                    // the left, hot at the right end.
                    let n = max(self.meter.y, 2.0)
                    let lane = step(0.5, u.y)
                    let lv = mix(self.level.x, self.level.y, lane)
                    let fy = fract(u.y * 2.0)
                    let seg = floor(u.x * n)
                    let fx = fract(u.x * n)
                    let cell = step(0.14, fx) * step(fx, 0.86) * step(0.16, fy) * step(fy, 0.84)
                    let on = step((seg + 0.5) / n, lv)
                    let hot = smoothstep(0.65, 0.95, (seg + 0.5) / n)
                    let lit = mix(self.glow_ink.xyz, self.hot_ink.xyz, hot * self.meter.z)
                    let col = mix(self.led_off.xyz, lit, on)
                    o = mix(o, vec4(col, 1.0), cell * inside)
                } else {
                    // A needle over a lit dial.
                    let piv = vec2(self.rect_size.x * 0.5, self.rect_size.y * 1.05)
                    let lv = max(self.level.x, self.level.y)
                    let dial = mix(o.xyz, self.dial_ink.xyz, inside * 0.85)
                    o = vec4(dial, o.w)
                    let ang = (lv - 0.5) * 1.6
                    let dir = vec2(sin(ang), -cos(ang))
                    let q = p - piv
                    let t = clamp(dot(q, dir), 0.0, self.rect_size.y * self.meter.w)
                    let nd = length(q - dir * t) - 0.7
                    o = mix(o, vec4(0.08, 0.06, 0.05, 1.0), (1.0 - smoothstep(-px, px, nd)) * inside)
                }
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    // The meter's bar form, for a short wide rect (a pocket player's main
    // window): two thin bars, left over right, lit from the left toward
    // `hot_ink`, with ticks every tenth and the held peak as a bright mark.
    // `level` as the dial's. A style replaces `pixel` for its own (an amber
    // backlit strip, LCD segments, a glowing line).
    mod.widgets.HwMeterBarFace = mod.widgets.HwSurface{
        emissive_from_glow: true
        width: Fill
        height: Fill
        depth: -1.5
        radius: 4
        emissive: vec4(0.22, 0.78, 1.0, 0.4)
        draw_bg +: {
            ground: #x0c0d0f
            inner: vec4(0.6, 4.0, 0.0, 0.0)
            hot_ink: uniform(vec4(1.0, 0.35, 0.2, 1.0))
            led_off: uniform(vec4(0.10, 0.10, 0.11, 1.0))
            level: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                var o = self.layer(self.surface(p, self.ground.xyz))
                let pad = vec2(6.0, 3.0)
                let w = max(self.rect_size.x - pad.x * 2.0, 1.0)
                let rows = self.rect_size.y - pad.y * 2.0
                let gap = max(rows * 0.18, 1.0)
                let bh = max((rows - gap) * 0.5, 1.0)
                let u = (p.x - pad.x) / w
                let lane = step(pad.y + bh + gap * 0.5, p.y)
                let y0 = pad.y + lane * (bh + gap)
                let inside = step(pad.y, p.y) * step(p.y, y0 + bh) * step(y0, p.y) * step(0.0, u) * step(u, 1.0)
                let lv = mix(self.level.x, self.level.y, lane)
                let on = 1.0 - smoothstep(lv - px / w, lv + px / w, u)
                let hot = smoothstep(0.72, 0.95, u)
                let lit = mix(self.glow_ink.xyz, self.hot_ink.xyz, hot)
                var col = mix(self.led_off.xyz, lit, on)
                // A hair of shadow between tenths, so it reads as a scale.
                let tick = 1.0 - smoothstep(0.0, px * 1.2, abs(fract(u * 10.0 + 0.5) - 0.5) * w / 10.0)
                col = mix(col, col * 0.55, tick * 0.6)
                let peak = (1.0 - smoothstep(0.0, px * 1.5, abs(u - self.level.z) * w)) * step(0.02, self.level.z)
                col = mix(col, mix(lit, vec3(1.0, 1.0, 1.0), 0.4), peak)
                o = mix(o, vec4(col, 1.0), inside)
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    mod.widgets.HwMeterBase = #(HwMeter::register_widget(vm))
    mod.widgets.HwMeter = set_type_default() do mod.widgets.HwMeterBase{
        width: Fill
        height: 70
        flow: Overlay
        face := mod.widgets.HwMeterFace{}
        bar_face := mod.widgets.HwMeterBarFace{visible: false}
    }

    // What the album-art well shows with no art: a record. `vinyl` = the
    // disc, `label_ink` = its label (defaults to the accent), `groove` =
    // groove strength.
    mod.widgets.HwArtBlank = mod.widgets.HwSurface{
        width: Fill
        height: Fill
        depth: -1.0
        radius: 4
        proxy: false
        draw_bg +: {
            ground: #x16171a
            vinyl: uniform(vec4(0.06, 0.06, 0.07, 1.0))
            label_ink: uniform(vec4(0.22, 0.78, 1.0, 1.0))
            groove: uniform(0.12)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                var col = self.surface(p, self.ground.xyz)
                let r = min(c.x, c.y) - 3.0
                let d = length(p - c)
                let disc = 1.0 - smoothstep(r - px, r + px, d)
                let rings = 0.5 + 0.5 * sin(d * 3.1)
                let lt = normalize(self.light.xy + vec2(0.000001, 0.000001))
                let ang = dot(normalize(p - c + vec2(0.0001, 0.0001)), lt)
                let sheen = pow(abs(ang), 6.0) * 0.18
                let vinyl = self.vinyl.xyz * (1.0 - self.groove + self.groove * 2.0 * rings) + vec3(sheen, sheen, sheen)
                col = mix(col, vinyl, disc)
                let lr = r * 0.34
                let label = 1.0 - smoothstep(lr - px, lr + px, d)
                col = mix(col, self.label_ink.xyz * (0.9 + 0.1 * sin(d * 0.7)), label)
                let hole = 1.0 - smoothstep(1.4 - px, 1.4 + px, d)
                col = mix(col, self.ground.xyz * 0.5, hole)
                return vec4(col + self.dither(), 1.0)
            }
        }
    }

    // ---- the picture ----
    // The screen's two pictures and how strongly each lights the device:
    // `light` scales the picture's colour, `light_knee` is the soft ceiling
    // its brightest frames approach (so a white flash does not flood the
    // bevels).
    mod.widgets.HwSpectrum = Visualizer{
        light: 2.5
    }
    mod.widgets.HwMilk = MilkdropView{
        light: 1.3
        light_knee: 1.0
        // A monochrome tube: rgb, amount (0 = the picture's own colours).
        tint: vec4(1.0, 1.0, 1.0, 0.0)
    }

    // The lower half of the right wing: a slot a style fills. Hidden
    // unless a style opts in with `HwOrnament.visible = true` and draws
    // its own `HwOrnamentFace` (its `pixel` sees `level`, `self.lit`); the
    // unstyled needle dial looked the same in every style.
    mod.widgets.HwOrnamentFace = mod.widgets.HwMeterFace{
        depth: -2.0
        draw_bg +: {
            meter: uniform(vec4(1.0, 18.0, 0.8, 0.85))
            plate_id: 13.0
        }
    }
    mod.widgets.HwOrnament = mod.widgets.HwMeter{
        visible: false
        // Left out when the wing leaves it less room than this (never cut).
        min_height: 72.0
        forms: false
        width: Fill
        height: Fill
        face := mod.widgets.HwOrnamentFace{}
    }

    // A readout on a display (position, length): text that can glow onto
    // the glass round it. `glow` = rgb, intensity (0 = no glow), `glow_pad`
    // how far past the text (pt).
    mod.widgets.HwReadoutBase = #(HwReadout::register_widget(vm))
    mod.widgets.HwReadout = set_type_default() do mod.widgets.HwReadoutBase{
        width: Fit
        height: Fit
        padding: 0
        glow: vec4(0.0, 0.0, 0.0, 0.0)
        glow_pad: 3.0
        draw_text +: {
            color: #xc8ccd2
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    // ---- type ----
    // Printed on the chassis: captions over controls, and larger titles.
    mod.widgets.HwCaption = Label{
        padding: 0
        draw_text +: {
            color: #x3b3e44
            text_style: theme.font_bold{font_size: 8.0}
        }
    }
    mod.widgets.HwTitle = Label{
        padding: 0
        draw_text +: {
            color: #x2a2c31
            text_style: theme.font_bold{font_size: 12.5}
        }
    }
    mod.widgets.HwText = Label{
        padding: 0
        draw_text +: {
            color: #x55595f
            text_style: theme.font_regular{font_size: 10.0}
        }
    }
    // On a recessed panel: its headings and notes.
    mod.widgets.HwPanelTitle = Label{
        padding: 0
        draw_text +: {
            color: #xe6e8ea
            text_style: theme.font_bold{font_size: 13.0}
        }
    }
    mod.widgets.HwPanelText = Label{
        padding: 0
        draw_text +: {
            color: #x868c95
            text_style: theme.font_regular{font_size: 9.5}
        }
    }
    // On a display (the progress window, the equalizer's): light on dark.
    mod.widgets.HwDisplayText = Label{
        padding: 0
        draw_text +: {
            color: #xc8ccd2
            text_style: theme.font_regular{font_size: 9.5}
        }
    }
    // A dark display window set into the front: what paints from the
    // palette (bars, levels) sits on one of these.
    mod.widgets.HwDisplay = mod.widgets.HwBezel{
        radius: 8
        depth: -3.0
    }
    // A small recessed window a readout sits in (the preset's name): a
    // display strip with inner padding that clips what it holds. Styles
    // give it their own glass (`HwLabelWindow.draw_bg`: an LCD strip, an
    // engraved slot, a paper label); plate 21.
    mod.widgets.HwLabelWindow = mod.widgets.HwDisplay{
        draw_bg +: {plate_id: 21.0}
        width: Fill
        height: 28
        radius: 5
        depth: -2.0
        flow: Overlay
        align: Align{y: 0.5}
        padding: Inset{left: 9 right: 9 top: 2 bottom: 2}
    }
    // A text button on a panel (the sidebar's navigation): an engraved
    // plate that lifts under the pointer. `plate` = its fill (alpha = how
    // much), `plate_hover`, `rim` = its edge light.
    mod.widgets.HwNavButton = ButtonFlat{
        width: Fill
        height: 28
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
        draw_bg +: {
            plate: uniform(vec4(0.0, 0.0, 0.0, 0.06))
            plate_hover: uniform(vec4(1.0, 1.0, 1.0, 0.10))
            rim: uniform(vec4(1.0, 1.0, 1.0, 0.16))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 4.0)
                let fill = mix(self.plate, self.plate_hover, self.hover)
                // A brighter lip along the top edge.
                let lit = 1.0 + (1.0 - smoothstep(0.0, 3.0, p.y)) * 0.5
                sdf.fill_keep(vec4(fill.xyz * lit, fill.w * (1.0 + self.down * 0.6)))
                sdf.stroke(vec4(self.rim.xyz, self.rim.w * (0.6 + 0.4 * self.hover)), 1.0)
                return sdf.result
            }
        }
        draw_text +: {text_style: theme.font_regular{font_size: 10.5}}
    }

    // A two- or three-way switch (Spectrum | MilkDrop, All | Available):
    // a dark slot with a lit cap sliding in it. `well` = the slot, `rim`
    // its edge, the pill's `cap` = the cap's face and `glow` = its light.
    mod.widgets.HwSegmented = SegmentedControl{
        // Every segment as wide as the widest (its bold, selected label)
        // plus room either side, so a lit cap never crowds its word.
        equal_width: true
        segment_padding: 16
        draw_bg +: {
            well: uniform(vec4(0.09, 0.10, 0.11, 1.0))
            rim: uniform(vec4(1.0, 1.0, 1.0, 0.10))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 5.0)
                let top = 1.0 - smoothstep(0.0, 7.0, p.y) * 0.5
                sdf.fill_keep(vec4(self.well.xyz * top, self.well.w))
                sdf.stroke(self.rim, 1.0)
                return sdf.result
            }
        }
        draw_pill +: {
            cap: uniform(vec4(1.0, 1.0, 1.0, 0.16))
            glow: uniform(vec4(0.22, 0.78, 1.0, 1.0))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(2.0, 2.0, self.rect_size.x - 4.0, self.rect_size.y - 4.0, 4.0)
                let shade = mix(1.25, 0.8, self.pos.y)
                sdf.fill_keep(vec4(self.cap.xyz * shade, self.cap.w + 0.12))
                sdf.stroke(vec4(self.cap.xyz, self.cap.w * 1.6), 1.0)
                // The lit dash along the cap's foot.
                let dash = (1.0 - smoothstep(0.0, 1.2, abs(p.y - (self.rect_size.y - 4.5)))) * step(6.0, p.x) * step(p.x, self.rect_size.x - 6.0)
                return sdf.result * (1.0 - dash) + vec4(self.glow.xyz * dash, dash)
            }
        }
    }
    // Choosers and the search field: small dark display windows.
    // `well` = the glass, `rim` = its edge light, `arrow` = the chooser's mark.
    // A drop down's list, in the style's menu colours (set from the
    // palette with the menus': `menu_bg`, the menu ink, the accent).
    mod.widgets.HwPopupMenuItem = mod.widgets.PopupMenuItem{
        draw_text +: {
            color: #xd6d9de
            color_hover: #xffffff
            color_active: #xffffff
        }
        draw_bg +: {
            color_hover: #x39c6ff55
            color_active: #x39c6ff88
            border_radius: 4.0
        }
    }
    mod.widgets.HwPopupMenu = mod.widgets.PopupMenuFlat{
        menu_item: mod.widgets.HwPopupMenuItem{}
        draw_bg +: {
            color: #x1b1d20
            border_color: #x39c6ff73
            border_size: 1.0
            border_radius: 6.0
        }
    }

    mod.widgets.HwDropDown = DropDown{
        popup_menu: mod.widgets.HwPopupMenu{}
        // The label ends in "…" before the chevron.
        padding: Inset{left: 8 right: 24 top: 4 bottom: 4}
        draw_text +: {text_overflow: TextOverflow.Ellipsis}
        draw_bg +: {
            well: uniform(vec4(0.09, 0.10, 0.11, 1.0))
            rim: uniform(vec4(1.0, 1.0, 1.0, 0.10))
            arrow: uniform(vec4(0.80, 0.82, 0.85, 1.0))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 4.0)
                let top = 1.0 - smoothstep(0.0, 6.0, p.y) * 0.5
                sdf.fill_keep(vec4(self.well.xyz * top, self.well.w))
                sdf.stroke(self.rim, 1.0)
                let x = self.rect_size.x - 13.0
                let y = self.rect_size.y * 0.5
                sdf.move_to(x - 3.5, y - 1.8)
                sdf.line_to(x, y + 1.8)
                sdf.line_to(x + 3.5, y - 1.8)
                sdf.stroke(self.arrow, 1.2)
                return sdf.result
            }
        }
        draw_text +: {
            color: #xc9cdd3
            color_hover: #xe6e8ea
            color_focus: #xe6e8ea
            color_down: #xe6e8ea
        }
    }
    mod.widgets.HwTextInput = TextInput{
        draw_bg +: {
            well: uniform(vec4(0.09, 0.10, 0.11, 1.0))
            rim: uniform(vec4(1.0, 1.0, 1.0, 0.10))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 5.0)
                let top = 1.0 - smoothstep(0.0, 7.0, p.y) * 0.5
                sdf.fill_keep(vec4(self.well.xyz * top, self.well.w))
                sdf.stroke(self.rim, 1.0)
                return sdf.result
            }
        }
        draw_text +: {
            color: #xe6e8ea
            color_hover: #xe6e8ea
            color_focus: #xffffff
            color_down: #xe6e8ea
            color_empty: #x80868f
            color_empty_hover: #x9aa0a8
            color_empty_focus: #x9aa0a8
        }
    }

    // The popup menus, as a small lit display hung off the device. Its
    // colours default from the palette (`palette_defaults`).
    mod.widgets.HwMenuLayer = MenuLayer{
        // The theme's text face has no ⌘ ⇧ ⌥ ⌃: Inter after it has them.
        draw_label +: {text_style +: {font_family +: {symbols := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") asc: -0.1 desc: 0.0}}}}
        draw_shortcut +: {text_style +: {font_family +: {symbols := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") asc: -0.1 desc: 0.0}}}}
        draw_bg +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 6.0)
                let g = mix(self.color.xyz * 1.18, self.color.xyz * 0.88, self.pos.y)
                sdf.fill_keep(vec4(g, self.color.w))
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
    }

    // The MilkDrop preset browser, in the style: its search field, its
    // switches, and (from Rust) its list in the palette.
    mod.widgets.HwBrowserSearch = mod.widgets.HwTextInput{
        width: Fill
        empty_text: "Search presets"
    }
    mod.widgets.HwBrowserCheck = CheckBox{
        draw_text +: {color: #xc8ccd2}
    }
    mod.widgets.HwBrowser = MilkdropBrowser{
        filters +: {
            search := mod.widgets.HwBrowserSearch{}
            favourites := mod.widgets.HwBrowserCheck{text: "Favourites"}
            rated := mod.widgets.HwBrowserCheck{text: "Rated"}
        }
        // No pack yet: the display's text, the style's keys.
        missing +: {
            missing_group +: {
                missing_title := mod.widgets.HwDisplayText{width: Fill align: Align{x: 0.5} text: "No visualizer presets yet." draw_text +: {text_style: theme.font_bold{font_size: 12}}}
                missing_about := mod.widgets.HwDisplayText{width: Fill align: Align{x: 0.5} text: "projectM's \"cream of the crop\" pack: 9,795 presets, about 160 MB." draw_text +: {text_style: theme.font_regular{font_size: 9.5}}}
                missing_keys +: {
                    download_key := mod.widgets.HwKey{width: Fit height: 32 inset: 3 draw_bg +: {role: 43.0} download := mod.widgets.HwKeyText{text: "DOWNLOAD PRESETS" width: Fit padding: Inset{left: 16 right: 16}}}
                    cancel_key := mod.widgets.HwKey{visible: false width: Fit height: 32 inset: 3 draw_bg +: {role: 44.0} cancel := mod.widgets.HwKeyText{text: "CANCEL" width: Fit padding: Inset{left: 16 right: 16}}}
                }
                progress := mod.widgets.HwDisplayText{width: Fill align: Align{x: 0.5} text: "" draw_text +: {text_style: theme.font_regular{font_size: 9.0}}}
            }
        }
    }

    // The groove between the device and the lists: a rail with a grip in
    // its middle, dragged to make the screen taller or shorter. `groove` =
    // the rail's shadow, `glint` its lit edge, `grip` the ridges' ink.
    mod.widgets.HwDeckSplitter = Splitter{
        size: 14.0
        // A drag keeps the layout's anchor (FromA stays FromA), so what
        // is saved is what the layout declared, in its own kind.
        exact_floors: true
        // A folded pane (Up Next) comes back when its rail is dragged.
        bar_reopens: true
        draw_bg +: {
            groove: uniform(vec4(0.0, 0.0, 0.0, 0.22))
            glint: uniform(vec4(1.0, 1.0, 1.0, 0.25))
            grip: uniform(vec4(0.0, 0.0, 0.0, 0.35))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let emphasis = max(self.hover, self.drag)
                // The rail: a dark line with a lit line under it.
                let rail = 1.0 - smoothstep(0.5, 1.5, abs(p.y - c.y + 0.5))
                let lip = (1.0 - smoothstep(0.5, 1.5, abs(p.y - c.y - 1.0))) * 0.8
                // The grip: short vertical ridges in the middle.
                let gx = abs(p.x - c.x)
                let ridge = step(gx, 26.0) * (1.0 - smoothstep(0.4, 1.2, abs(fract(p.x / 4.0) - 0.5) * 4.0)) * (1.0 - smoothstep(3.0, 4.0, abs(p.y - c.y)))
                var a = rail * (self.groove.w + emphasis * 0.15)
                var col = self.groove.xyz * a
                let lb = lip * self.glint.w * (1.0 - rail)
                col = col + self.glint.xyz * lb
                a = a + lb
                let gb = ridge * (self.grip.w + emphasis * 0.2)
                col = col * (1.0 - gb) + self.grip.xyz * gb
                a = clamp(a + gb, 0.0, 1.0)
                return vec4(col, a)
            }
        }
    }

    // The splitter between the main panel and Up Next: a groove.
    mod.widgets.HwSplitter = Splitter{
        size: 14.0
        // A drag keeps the layout's anchor (FromA stays FromA), so what
        // is saved is what the layout declared, in its own kind.
        exact_floors: true
        // A folded pane (Up Next) comes back when its rail is dragged.
        bar_reopens: true
        draw_bg +: {
            groove: uniform(vec4(0.0, 0.0, 0.0, 0.18))
            glint: uniform(vec4(1.0, 1.0, 1.0, 0.10))
            pixel: fn() {
                let p = self.pos * self.rect_size
                let emphasis = max(self.hover, self.drag)
                var a = 0.0
                var col = vec3(0.0, 0.0, 0.0)
                if self.is_vertical > 0.5 {
                    let x = abs(p.x - self.rect_size.x * 0.5)
                    let g = 1.0 - smoothstep(0.5, 1.5, x)
                    let h = (1.0 - smoothstep(0.5, 1.5, abs(p.x - self.rect_size.x * 0.5 - 1.5))) * 0.7
                    a = g * (self.groove.w + emphasis * 0.2)
                    col = self.groove.xyz
                    let o = vec4(col * a, a)
                    let hb = h * self.glint.w
                    return vec4(self.glint.xyz * hb, hb) + o * (1.0 - hb)
                }
                let y = abs(p.y - self.rect_size.y * 0.5)
                a = (1.0 - smoothstep(0.5, 1.5, y)) * (self.groove.w + emphasis * 0.2)
                return vec4(self.groove.xyz * a, a)
            }
        }
    }
}

// ---- styles ----

/// The first styles, in picker order: file stem and the name printed on
/// the device. Any other style file (embedded, or found in
/// `resources/styles/` in a source checkout) follows them, ordered by the
/// `// order: N` and `// title: Name` lines at its top, then by title.
pub const STYLES: [(&str, &str); 8] = [
    ("brushed", "Brushed Aluminium"),
    ("rack", "Dark Rack"),
    ("walnut", "Walnut & Brass"),
    ("plastic", "Moulded Plastic"),
    ("piano", "Piano Black"),
    ("nostromo", "Ship Console"),
    ("military", "Military"),
    ("orbital", "Orbital Lab"),
];

// `EMBEDDED`, `EMBEDDED_LAYOUTS`: the finished styles and layouts (embedded_styles.rs).
#[path = "embedded_styles.rs"]
mod embedded_styles;
pub use embedded_styles::{EMBEDDED, EMBEDDED_LAYOUTS};

/// One entry of the picker.
#[derive(Clone, Debug)]
pub struct StyleEntry {
    pub name: String,
    pub title: String,
    order: f64,
}

/// A style file's `// title:` and `// order:` lines, near its top.
fn header(source: &str, key: &str) -> Option<String> {
    source.lines().take(24).find_map(|line| {
        let rest = line.trim().strip_prefix("//")?.trim();
        let value = rest.strip_prefix(key)?.trim_start().strip_prefix(':')?;
        Some(value.trim().to_string())
    })
}

/// Every style there is, in picker order.
pub fn style_list() -> Vec<StyleEntry> {
    let mut names: Vec<String> = EMBEDDED.iter().map(|(n, _)| n.to_string()).collect();
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(entries) = std::fs::read_dir(style_path("x").parent().unwrap()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) == Some("splash") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if !names.iter().any(|n| n == stem) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    // The person's own styles (<data>/styles), see style_store.
    names.extend(crate::style_store::user_names());
    let mut list: Vec<StyleEntry> = names
        .into_iter()
        .map(|name| {
            let source = style_source(&name).unwrap_or_default();
            let fixed = STYLES.iter().position(|(n, _)| *n == name);
            let title = header(&source, "title")
                .or_else(|| fixed.map(|at| STYLES[at].1.to_string()))
                .unwrap_or_else(|| {
                    let mut c = name.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                });
            let order = match fixed {
                Some(at) => at as f64,
                None => 100.0 + header(&source, "order").and_then(|o| o.parse().ok()).unwrap_or(1000.0),
            };
            StyleEntry { name, title, order }
        })
        .collect();
    list.sort_by(|a, b| a.order.partial_cmp(&b.order).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.title.cmp(&b.title)));
    crate::style_store::arrange_picker(&mut list);
    list
}

const DEFAULT_STYLE: &str = "brushed";

/// Which style is on, and whether its file is watched.
#[derive(Default)]
pub struct HwStyle {
    pub name: String,
    /// The style's colour map for the picture: four stops, how strongly
    /// it is blended in, and whether it is on unless the person says.
    pub map: [Vec4f; 4],
    pub map_strength: f32,
    pub map_default: bool,
    /// The style's screen filter: kind, strength, cell (pt), curvature.
    pub filter: Vec4f,
    /// The screen's light graded to one colour (palette `spill_color`,
    /// `spill_mono`).
    pub light_tint: Vec4f,
    /// The person's choice per style (`theme_colours.txt`).
    pub map_choice: Option<std::collections::HashMap<String, bool>>,
    /// `--style` or `MUSIC_STYLE` pinned it: the file is watched and the
    /// choice is not saved over the person's.
    pub pinned: bool,
    /// The file's modification time when it was last read.
    pub stamp: Option<std::time::SystemTime>,
}

fn style_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("styles").join(format!("{name}.splash"))
}

fn stamp(name: &str) -> Option<std::time::SystemTime> {
    if let Some(stamp) = crate::style_store::user_stamp(name) {
        return Some(stamp);
    }
    std::fs::metadata(style_path(name)).and_then(|m| m.modified()).ok()
}

/// A style's Splash: the file in a source checkout, else the built-in copy.
fn style_source(name: &str) -> Option<String> {
    if let Some(text) = crate::style_store::user_source(name) {
        return Some(text);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(text) = std::fs::read_to_string(style_path(name)) {
        return Some(text);
    }
    EMBEDDED.iter().find(|(n, _)| *n == name).map(|(_, s)| s.to_string())
}

/// Whether `name` can be shown: a file or a built-in copy.
pub fn available(name: &str) -> bool {
    style_path(name).is_file() || EMBEDDED.iter().any(|(n, _)| *n == name) || crate::style_store::is_user(name)
}

/// The printed name of a style.
pub fn label(name: &str) -> String {
    style_list().into_iter().find(|e| e.name == name).map(|e| e.title).unwrap_or_else(|| "Custom".into())
}

/// `--style <name>`, `--style=<name>` or `MUSIC_STYLE`.
fn pinned_style() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for (at, arg) in args.iter().enumerate() {
        if let Some(name) = arg.strip_prefix("--style=") {
            return Some(name.to_string());
        }
        if arg == "--style" {
            if let Some(name) = args.get(at + 1) {
                return Some(name.clone());
            }
        }
    }
    std::env::var("MUSIC_STYLE").ok().filter(|s| !s.is_empty())
}

fn saved_path() -> Option<PathBuf> {
    Some(crate::worker::data_dir()?.join("style.txt"))
}

/// The style to start with: pinned, else the one last chosen, else the default.
fn startup_style() -> HwStyle {
    if let Some(name) = pinned_style() {
        return HwStyle { stamp: stamp(&name), name, pinned: true, ..Default::default() };
    }
    let saved = saved_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.trim().to_string());
    let name = saved.filter(|s| available(s)).unwrap_or_else(|| DEFAULT_STYLE.to_string());
    HwStyle { stamp: stamp(&name), name, pinned: false, ..Default::default() }
}

/// Keep the person's choice for the next start (not a pinned test run's).
pub fn save_choice(cx: &mut Cx) {
    let style = cx.global::<HwStyle>();
    if style.pinned {
        return;
    }
    if let Some(path) = saved_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(path, &style.name);
    }
}

/// Evaluate the current style over the base templates. Called from
/// `App::script_mod` between `script_mod` (above) and the app's layout.
pub fn apply_style(vm: &mut ScriptVm) {
    if !vm.cx_mut().has_global::<HwStyle>() {
        let style = startup_style();
        vm.cx_mut().set_global(style);
    }
    let name = vm.cx_mut().global::<HwStyle>().name.clone();
    let code = match style_source(&name) {
        Some(code) => code,
        None => {
            error!("amp: no style named {name}; using {DEFAULT_STYLE}");
            vm.cx_mut().global::<HwStyle>().name = DEFAULT_STYLE.to_string();
            match style_source(DEFAULT_STYLE) {
                Some(code) => code,
                None => return,
            }
        }
    };
    vm.cx_mut().global::<HwStyle>().stamp = stamp(&name);
    // The style, then the templates that follow the palette it set, then
    // the style again so what it says about those templates wins.
    eval_style(vm, &name, code.clone());
    let defaults = palette_defaults(vm);
    vm.eval(ScriptMod::new(env!("CARGO_MANIFEST_DIR"), "music_style_palette", "src/hw.rs", 0, 0, &defaults, &[]));
    eval_style(vm, &name, code);
    read_map(vm);
}

/// The style's colour map, from its palette (`map0`..`map3`, `map_strength`,
/// `map_default`).
fn read_map(vm: &mut ScriptVm) {
    let widgets = vm.module(id!(widgets));
    let value = vm.heap().value(widgets, id!(hw_palette).into(), NoTrap);
    let Some(obj) = value.as_object() else { return };
    let color = |vm: &mut ScriptVm, key: &str, d: Vec4f| vm.heap().value(obj, LiveId::from_str(key).into(), NoTrap).as_color().map(Vec4f::from_u32).unwrap_or(d);
    let map = [
        color(vm, "map0", vec4(0.0, 0.0, 0.0, 1.0)),
        color(vm, "map1", vec4(0.33, 0.33, 0.33, 1.0)),
        color(vm, "map2", vec4(0.66, 0.66, 0.66, 1.0)),
        color(vm, "map3", vec4(1.0, 1.0, 1.0, 1.0)),
    ];
    let strength = vm.heap().value(obj, id!(map_strength).into(), NoTrap).as_f64().unwrap_or(0.75) as f32;
    let default = vm.heap().value(obj, id!(map_default).into(), NoTrap).as_f64().unwrap_or(0.0) > 0.5;
    let float = |vm: &mut ScriptVm, key: &str, d: f64| vm.heap().value(obj, LiveId::from_str(key).into(), NoTrap).as_f64().unwrap_or(d) as f32;
    let filter = vec4(
        float(vm, "screen_filter", 0.0),
        float(vm, "screen_filter_strength", 1.0),
        float(vm, "screen_cell", 3.0),
        float(vm, "screen_curve", 0.08),
    );
    let spill = color(vm, "spill_color", vec4(1.0, 1.0, 1.0, 1.0));
    let mono = float(vm, "spill_mono", 0.0);
    let style = vm.cx_mut().global::<HwStyle>();
    style.light_tint = vec4(spill.x, spill.y, spill.z, mono);
    style.filter = filter;
    style.map = map;
    style.map_strength = strength;
    style.map_default = default;
}

fn map_path() -> Option<PathBuf> {
    Some(crate::worker::data_dir()?.join("theme_colours.txt"))
}

/// Whether the picture is shown in the style's colours: the person's
/// choice for this style, else the style's default.
pub fn color_map_on(cx: &mut Cx) -> bool {
    let style = cx.global::<HwStyle>();
    if style.map_choice.is_none() {
        let mut choice = std::collections::HashMap::new();
        if let Some(text) = map_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                if let (Some(name), Some(on)) = (parts.next(), parts.next()) {
                    choice.insert(name.to_string(), on == "1");
                }
            }
        }
        style.map_choice = Some(choice);
    }
    let name = style.name.clone();
    style.map_choice.as_ref().and_then(|c| c.get(&name).copied()).unwrap_or(style.map_default)
}

/// Remember the person's choice for the style on screen.
pub fn set_color_map(cx: &mut Cx, on: bool) {
    color_map_on(cx);
    let style = cx.global::<HwStyle>();
    let name = style.name.clone();
    let choice = style.map_choice.get_or_insert_with(Default::default);
    choice.insert(name, on);
    let text: String = choice.iter().map(|(n, v)| format!("{n} {}\n", *v as u8)).collect();
    if let Some(path) = map_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(path, text);
    }
}

/// The map and filter to hand the picture now: the stops, the map's
/// amount and the filter (its strength 0 when off).
pub fn color_map(cx: &mut Cx) -> ([Vec4f; 4], f32, Vec4f, Vec4f) {
    let on = color_map_on(cx);
    let style = cx.global::<HwStyle>();
    let mut filter = style.filter;
    if !on {
        filter.y = 0.0;
    }
    (style.map, if on { style.map_strength } else { 0.0 }, filter, style.light_tint)
}

fn eval_style(vm: &mut ScriptVm, name: &str, code: String) {
    vm.eval(ScriptMod::new(env!("CARGO_MANIFEST_DIR"), &format!("music_style_{name}"), &format!("resources/styles/{name}.splash"), 0, 0, &code, &[]));
}

/// Every Hw surface template: the ones a style-wide setting reaches.
const SURFACES: [&str; 13] = [
    "HwSurface", "HwChassis", "HwPanel", "HwPlate", "HwBezel", "HwDisplay", "HwKey", "HwDash", "HwLed", "HwKnobFace", "HwMeterFace", "HwClockFace", "HwMeterBarFace",
];

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: Vec4f) -> f32 {
    let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(c.x) + 0.7152 * lin(c.y) + 0.0722 * lin(c.z)
}

/// WCAG contrast ratio of two colours (1..21).
pub fn contrast(a: Vec4f, b: Vec4f) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `want` if it reads on `bg` (4.5:1), else the first of `others` that
/// does, else whichever of them reads best.
fn readable(want: Vec4f, bg: Vec4f, others: &[Vec4f]) -> Vec4f {
    if contrast(want, bg) >= 4.5 {
        return want;
    }
    others
        .iter()
        .copied()
        .find(|c| contrast(*c, bg) >= 4.5)
        .unwrap_or_else(|| others.iter().copied().fold(want, |best, c| if contrast(c, bg) > contrast(best, bg) { c } else { best }))
}

/// How strong a list's selection and hover tints (the accent over the
/// list's floor) may be: the strongest of the usual strengths under which
/// the rows' text still reads (4.5:1) and a playing row's accent text still
/// stands out (3:1) on the tinted floor.
pub fn row_tint_alphas(bg: Vec4f, text: Vec4f, accent: Vec4f) -> (f32, f32) {
    let reads = |alpha: f32| {
        let floor = mix_v(vec4(bg.x, bg.y, bg.z, 1.0), vec4(accent.x, accent.y, accent.z, 1.0), alpha);
        contrast(text, floor) >= 4.5 && contrast(accent, floor) >= 3.0
    };
    let pick = |from: &[f32]| from.iter().copied().find(|a| reads(*a)).unwrap_or(*from.last().unwrap());
    (pick(&[0.30, 0.26, 0.22, 0.18, 0.15, 0.12]), pick(&[0.10, 0.08, 0.06]))
}

fn mix_v(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, 1.0)
}

fn hex(c: Vec4f) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    format!("#x{:02x}{:02x}{:02x}{:02x}", b(c.x), b(c.y), b(c.z), b(c.w))
}

fn with_a(mut c: Vec4f, a: f32) -> Vec4f {
    c.w = a;
    c
}

/// Splash giving the palette-driven templates their colours from the
/// palette the style set.
fn palette_defaults(vm: &mut ScriptVm) -> String {
    let widgets = vm.module(id!(widgets));
    let value = vm.heap().value(widgets, id!(hw_palette).into(), NoTrap);
    let Some(obj) = value.as_object() else { return "true".into() };
    let color = |vm: &mut ScriptVm, key: &str| vm.heap().value(obj, LiveId::from_str(key).into(), NoTrap).as_color().map(Vec4f::from_u32).unwrap_or(vec4(1.0, 0.0, 1.0, 1.0));
    let float = |vm: &mut ScriptVm, key: &str| vm.heap().value(obj, LiveId::from_str(key).into(), NoTrap).as_f64().unwrap_or(0.0);
    let screen_bg = color(vm, "screen_bg");
    let screen_text = color(vm, "screen_text");
    let screen_muted = color(vm, "screen_muted");
    let accent = color(vm, "accent");
    let panel_text = color(vm, "panel_text");
    let panel_muted = color(vm, "panel_muted");
    let menu_bg = color(vm, "menu_bg");
    let spill_color = color(vm, "spill_color");
    let spill_mono = float(vm, "spill_mono");
    let spill_knee = float(vm, "spill_knee");
    let spill_dark = float(vm, "spill_dark");
    let mut out = String::from("use mod.prelude.widgets_internal.*\n");
    let mut set = |path: &str, value: String| out.push_str(&format!("mod.widgets.{path} = {value}\n"));
    set("HwMenuLayer.draw_bg.color", hex(menu_bg));
    set("HwMenuLayer.draw_bg.border_color", hex(with_a(accent, 0.45)));
    // The menu's ink is paired with its floor (`menu_text`, else the
    // screen's text) and always readable on it: under 4.5:1 it becomes the
    // palette's darker or lighter ink, whichever reads.
    let menu_text_key = vm.heap().value(obj, id!(menu_text).into(), NoTrap).as_color().map(Vec4f::from_u32);
    let menu_ink = readable(menu_text_key.unwrap_or(screen_text), menu_bg, &[screen_text, panel_text, vec4(0.08, 0.08, 0.09, 1.0), vec4(0.95, 0.95, 0.96, 1.0)]);
    let menu_muted = mix_v(menu_ink, menu_bg, 0.35);
    set("HwMenuLayer.draw_label.color", hex(menu_ink));
    set("HwPopupMenu.draw_bg.color", hex(menu_bg));
    set("HwPopupMenu.draw_bg.border_color", hex(with_a(accent, 0.45)));
    set("HwPopupMenuItem.draw_text.color", hex(menu_ink));
    set("HwPopupMenuItem.draw_text.color_hover", hex(menu_ink));
    set("HwPopupMenuItem.draw_text.color_active", hex(menu_ink));
    set("HwPopupMenuItem.draw_bg.color_hover", hex(with_a(accent, 0.28)));
    set("HwPopupMenuItem.draw_bg.color_active", hex(with_a(accent, 0.45)));
    set("HwPopupMenuItem.draw_bg.color_2_hover", hex(with_a(accent, 0.28)));
    set("HwPopupMenuItem.draw_bg.color_2_active", hex(with_a(accent, 0.45)));
    set("HwMenuLayer.draw_shortcut.color", hex(menu_muted));
    set("HwMenuLayer.draw_section.color", hex(menu_muted));
    set("HwMenuLayer.draw_row.color_hover", hex(with_a(accent, 0.28)));
    set("HwMenuLayer.draw_row.color_down", hex(with_a(accent, 0.55)));
    set("HwMenuLayer.draw_row.color_mark", hex(accent));
    set("HwSegmented.draw_bg.well", format!("vec4({:.4}, {:.4}, {:.4}, 1.0)", screen_bg.x, screen_bg.y, screen_bg.z));
    set("HwSegmented.draw_pill.cap", format!("vec4({:.4}, {:.4}, {:.4}, 0.16)", screen_text.x, screen_text.y, screen_text.z));
    set("HwSegmented.draw_pill.glow", format!("vec4({:.4}, {:.4}, {:.4}, 1.0)", accent.x, accent.y, accent.z));
    set("HwSegmented.draw_text.color", hex(screen_muted));
    set("HwSegmented.draw_text_selected.color", hex(screen_text));
    set("HwNavButton.draw_text.color", hex(panel_text));
    set("HwNavButton.draw_text.color_hover", hex(panel_text));
    set("HwNavButton.draw_text.color_down", hex(panel_text));
    set("HwPanelTitle.draw_text.color", hex(panel_text));
    set("HwPanelText.draw_text.color", hex(panel_muted));
    set("HwKey.draw_bg.glow_ink", hex(accent));
    set("HwKeyIcon.draw_icon.color", "mod.widgets.HwKeyButton.draw_text.color".into());
    // The caption buttons: the caption's ink (or the screen text), the accent.
    // The chassis's ground: what the rail (and its keys' labels) sit on.
    let chassis = vm.heap().value(widgets, id!(HwChassis).into(), NoTrap).as_object();
    let draw_bg = chassis.and_then(|c| vm.heap().value(c, id!(draw_bg).into(), NoTrap).as_object());
    let ground = draw_bg.and_then(|d| vm.heap().value(d, id!(ground).into(), NoTrap).as_color()).map(Vec4f::from_u32).unwrap_or(vec4(0.09, 0.1, 0.11, 1.0));
    // The rail's ink: the style's `caption_text` (else the screen's text),
    // unless it does not read on the chassis: then the palette's panel ink,
    // or plain dark or light, whichever reads.
    let caption_want = vm.heap().value(obj, id!(caption_text).into(), NoTrap).as_color().map(Vec4f::from_u32).unwrap_or(screen_text);
    let caption_ink = readable(caption_want, ground, &[panel_text, screen_text, vec4(0.10, 0.10, 0.11, 1.0), vec4(0.94, 0.94, 0.95, 1.0)]);
    set("HwCaptionButton.draw_bg.ink", format!("vec4({:.4}, {:.4}, {:.4}, 1.0)", caption_ink.x, caption_ink.y, caption_ink.z));
    set("HwCaptionButton.draw_bg.accent", format!("vec4({:.4}, {:.4}, {:.4}, 1.0)", accent.x, accent.y, accent.z));
    set("HwCaptionButton.draw_text.color", hex(with_a(caption_ink, 0.92)));
    set("HwCaptionButton.draw_text.color_hover", hex(caption_ink));
    set("HwCaptionButton.draw_text.color_down", hex(caption_ink));
    set("HwCaptionButton.draw_text.color_focus", hex(with_a(caption_ink, 0.92)));
    set("HwKey.emissive", format!("vec4({:.4}, {:.4}, {:.4}, 1.0)", accent.x, accent.y, accent.z));
    set("HwArtBlank.draw_bg.label_ink", hex(accent));
    set("HwBrowserSearch.draw_text.color", hex(screen_text));
    // The small components (micro.rs): the time in the display's ink (or
    // the palette's `clock_ink`) on the display's floor, the title in its
    // text colour.
    let clock_ink = vm.heap().value(obj, id!(clock_ink).into(), NoTrap).as_color().map(Vec4f::from_u32).unwrap_or(screen_text);
    set("HwClockFace.draw_bg.glow_ink", hex(clock_ink));
    set("HwClockFace.draw_bg.ground", hex(screen_bg));
    set("HwMeterBarFace.draw_bg.glow_ink", hex(accent));
    set("HwMeterBarFace.draw_bg.ground", hex(screen_bg));
    set("HwMeterBarFace.emissive", format!("vec4({:.4}, {:.4}, {:.4}, 0.4)", accent.x, accent.y, accent.z));
    set("HwClockFace.emissive", format!("vec4({:.4}, {:.4}, {:.4}, 0.45)", clock_ink.x, clock_ink.y, clock_ink.z));
    set("HwMarquee.draw_text.color", hex(screen_text));
    set("HwBrowserCheck.draw_text.color", hex(screen_muted));
    // The audio connection's sheet and faint hints (connection.rs).
    set("HwConnText.draw_text.color", hex(screen_text));
    set("HwConnMuted.draw_text.color", hex(screen_muted));
    set("HwConnectHint.draw_text.color", hex(with_a(screen_muted, 0.85)));
    set("HwConnectHint.draw_text.color_focus", hex(with_a(screen_muted, 0.85)));
    set("HwConnectHint.draw_text.color_hover", hex(screen_text));
    set("HwConnectHint.draw_text.color_down", hex(screen_text));
    set("HwBrowser.color_text", hex(screen_text));
    set("HwBrowser.color_muted", hex(screen_muted));
    set("HwBrowser.color_accent", hex(accent));
    set("HwBrowser.color_hover", hex(with_a(accent, row_tint_alphas(screen_bg, screen_text, accent).1)));
    // The window's caption bar, when the style names its colours.
    let optional = |vm: &mut ScriptVm, key: &str| vm.heap().value(obj, LiveId::from_str(key).into(), NoTrap).as_color().map(Vec4f::from_u32);
    if let Some(bg) = optional(vm, "caption_bg") {
        set("Window.caption_bar.draw_bg.color", hex(bg));
    }
    if let Some(text) = optional(vm, "caption_text") {
        set("Window.caption_bar.caption_label.label.draw_text.color", hex(text));
        set("Window.draw_note.color", hex(text));
    }
    // The style-wide grade reaches the screen's light (`HwMilk.light_tint`,
    // from Rust); every other emitter keeps its own colour unless the style
    // asks for all of them with `spill_tint_all`.
    let tint_all = float(vm, "spill_tint_all") > 0.5;
    let surface_mono = if tint_all { spill_mono } else { 0.0 };
    for surface in SURFACES {
        set(&format!("{surface}.draw_bg.spill_tint"), format!("vec4({:.4}, {:.4}, {:.4}, {:.4})", spill_color.x, spill_color.y, spill_color.z, surface_mono));
        set(&format!("{surface}.draw_bg.spill_knee"), format!("{:.4}", spill_knee));
        set(&format!("{surface}.draw_bg.spill_dark"), format!("{:.4}", spill_dark));
    }
    out.push_str("true\n");
    out
}

/// A style's chip for the picker: its front's colour and its light, read
/// from the style's own file (so the chip is whatever the style says).
pub fn swatch(name: &str) -> (Vec4f, Vec4f) {
    let source = style_source(name).unwrap_or_default();
    let find = |key: &str| {
        source.lines().find_map(|line| {
            let rest = line.trim().strip_prefix(key)?.trim().strip_prefix('=')?.trim();
            let hex = rest.strip_prefix("#x").or_else(|| rest.strip_prefix('#'))?;
            let hex: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if hex.len() != 6 {
                return None;
            }
            Some(Vec4f::from_u32((u32::from_str_radix(&hex, 16).ok()? << 8) | 0xff))
        })
    };
    let fill = find("mod.widgets.HwChassis.draw_bg.ground").unwrap_or(vec4(0.5, 0.5, 0.5, 1.0));
    let dot = find("mod.widgets.hw_palette.accent").unwrap_or(vec4(1.0, 1.0, 1.0, 1.0));
    (fill, dot)
}

/// Switch to `name` and restyle in place.
pub fn set_style(cx: &mut Cx, name: &str) {
    let style = cx.global::<HwStyle>();
    if style.name == name {
        return;
    }
    style.name = name.to_string();
    save_choice(cx);
    cx.request_style_reload();
}

/// Show `name` without keeping it as the person's choice (the showcase).
pub fn show_style(cx: &mut Cx, name: &str) {
    let style = cx.global::<HwStyle>();
    if style.name == name {
        return;
    }
    style.name = name.to_string();
    cx.request_style_reload();
}

/// The file of the style on screen changed on disk: restyle. Checked on a
/// timer only while the style is pinned (a style author's run).
pub fn check_file(cx: &mut Cx) {
    let style = cx.global::<HwStyle>();
    let now = stamp(&style.name);
    if now.is_some() && now != style.stamp {
        style.stamp = now;
        cx.request_style_reload();
    }
}

pub fn pinned(cx: &mut Cx) -> bool {
    cx.has_global::<HwStyle>() && cx.global::<HwStyle>().pinned
}

pub fn current(cx: &mut Cx) -> String {
    if !cx.has_global::<HwStyle>() {
        return DEFAULT_STYLE.to_string();
    }
    cx.global::<HwStyle>().name.clone()
}

/// The lists' and the spectrum's palette, from the style.
pub fn palette(vm: &mut ScriptVm) -> Theme {
    let fallback = Theme::default();
    let widgets = vm.module(id!(widgets));
    let value = vm.heap().value(widgets, id!(hw_palette).into(), NoTrap);
    let Some(obj) = value.as_object() else { return fallback };
    let read = |key: &str, default: Vec4f| {
        vm.heap()
            .value(obj, LiveId::from_str(key).into(), NoTrap)
            .as_color()
            .map(Vec4f::from_u32)
            .unwrap_or(default)
    };
    let accent = read("accent", fallback.accent);
    let muted = read("screen_muted", fallback.muted);
    let caps = vm.heap().value(obj, id!(caps).into(), NoTrap).as_f64().unwrap_or(0.0) > 0.5;
    let float = |key: &str, d: f32| vm.heap().value(obj, LiveId::from_str(key).into(), NoTrap).as_f64().map(|v| v as f32).unwrap_or(d);
    Theme {
        background: read("screen_bg", fallback.background),
        foreground: read("screen_text", fallback.foreground),
        muted,
        accent,
        legend_on: read("legend_on", accent),
        legend_off: read("legend_off", muted),
        caps,
        legend_scale: float("legend_scale", fallback.legend_scale),
        legend_text_scale: float("legend_text_scale", fallback.legend_text_scale),
        legend_offset: float("legend_offset", fallback.legend_offset),
        legend_fit: float("legend_fit", 1.0) > 0.5,
        legend_transport: float("legend_transport", 1.0) > 0.5,
        row_invert: float("row_invert", 0.0) > 0.5,
        dark_floor: {
            let p = read("panel_text", vec4(0.17, 0.18, 0.2, 1.0));
            float("dark_floor", if 0.2126 * p.x + 0.7152 * p.y + 0.0722 * p.z > 0.5 { 1.0 } else { 0.0 }) > 0.5
        },
    }
}

// ---- knob ----

/// A rotary control: drag up or right to turn it up. Reports like a
/// [`MusicBar`](crate::widgets::MusicBar): the value under the hand as it
/// moves, and the value let go at.
#[derive(Script, ScriptHook, Widget)]
pub struct HwKnob {
    #[deref]
    view: View,
    #[rust]
    value: f64,
    #[rust]
    enabled: bool,
    /// Where the drag started and the value then.
    #[rust]
    drag: Option<(Vec2d, f64)>,
    #[rust]
    dragging: Option<f64>,
}

impl HwKnob {
    fn face_area(&self, cx: &Cx) -> Area {
        self.view.widget(cx, ids!(face)).area()
    }

    pub fn set(&mut self, cx: &mut Cx, value: Option<f64>) {
        let enabled = value.is_some();
        let value = value.unwrap_or(0.0).clamp(0.0, 1.0);
        if (value - self.value).abs() > 1e-4 || enabled != self.enabled {
            self.value = value;
            self.enabled = enabled;
            self.view.redraw(cx);
        }
    }
}

impl Widget for HwKnob {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if !self.enabled {
            return;
        }
        let area = self.face_area(cx);
        match event.hits(cx, area) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(e) => {
                self.drag = Some((e.abs, self.value));
                self.dragging = Some(self.value);
            }
            Hit::FingerMove(e) => {
                if let Some((start, from)) = self.drag {
                    let delta = (start.y - e.abs.y) + (e.abs.x - start.x);
                    let value = (from + delta / 240.0).clamp(0.0, 1.0);
                    self.dragging = Some(value);
                    self.view.redraw(cx);
                    cx.widget_action(self.widget_uid(), MusicBarAction::Moved(value));
                }
            }
            Hit::FingerUp(e) => {
                let value = self.dragging.take().unwrap_or(self.value);
                self.drag = None;
                if !e.cancelled {
                    self.value = value;
                    cx.widget_action(self.widget_uid(), MusicBarAction::Set(value));
                }
                self.view.redraw(cx);
            }
            Hit::FingerScroll(e) => {
                let value = (self.value - e.scroll.y / 600.0).clamp(0.0, 1.0);
                self.value = value;
                self.view.redraw(cx);
                cx.widget_action(self.widget_uid(), MusicBarAction::Set(value));
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let value = self.dragging.unwrap_or(self.value) as f32;
        if let Some(mut face) = self.view.relief_view(cx, ids!(face)).borrow_mut() {
            face.view.draw_bg.set_uniform(cx, live_id!(value), &[value]);
            face.lit = value as f64;
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

impl HwKnobRef {
    pub fn set(&self, cx: &mut Cx, value: Option<f64>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set(cx, value);
        }
    }

    pub fn released(&self, actions: &Actions) -> Option<f64> {
        actions.filter_widget_actions(self.widget_uid()).filter_map(|action| match action.cast() {
            MusicBarAction::Set(value) => Some(value),
            _ => None,
        }).last()
    }

    pub fn moved(&self, actions: &Actions) -> Option<f64> {
        actions.filter_widget_actions(self.widget_uid()).filter_map(|action| match action.cast() {
            MusicBarAction::Moved(value) => Some(value),
            _ => None,
        }).last()
    }
}

// ---- meter ----

/// The level of the sound playing, as a meter. Frames only while the level
/// moves; at rest nothing redraws.
#[derive(Script, ScriptHook, Widget)]
pub struct HwMeter {
    #[deref]
    view: View,
    /// Draw nothing when given less height than this (an ornament in a
    /// wing's leftover room is left out rather than cut).
    #[live(0.0)]
    min_height: f64,
    /// Picks the dial or the bars by its rect (off for an ornament).
    #[live(true)]
    forms: bool,
    #[rust]
    samples: Vec<f32>,
    #[rust]
    written: u64,
    /// When the newest block came (app seconds): no block for a while and
    /// the meter falls to rest, whether the source sends silence or nothing.
    #[rust]
    fresh_at: f64,
    #[rust]
    level: [f32; 2],
    /// The held peak of either channel, when it was set.
    #[rust]
    peak: f32,
    #[rust]
    peak_at: f64,
    /// A needle on a spring (VU ballistics): position and speed.
    #[rust]
    needle: f32,
    #[rust]
    needle_v: f32,
    #[rust]
    drawn_at: f64,
    #[rust]
    next_frame: NextFrame,
}

/// A meter with no new block for this long (s) falls to rest.
const METER_STALE: f64 = 0.5;

impl Widget for HwMeter {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.view.redraw(cx);
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The room left under the parent's content (a fixed height can be
        // taller than what is left: it would be cut by the parent's edge).
        if self.min_height > 0.0 && cx.peek_walk_turtle(walk).size.y.min(cx.turtle().unused_inner_height()) < self.min_height {
            cx.walk_turtle(walk);
            return DrawStep::done();
        }
        let now = cx.seconds_since_app_start();
        let dt = (now - self.drawn_at).clamp(0.0, 0.1) as f32;
        self.drawn_at = now;
        let mut target = [0.0f32; 2];
        let model: Option<&Model> = scope.data_mut().get::<Model>().map(|model| &*model);
        // The level heard: after our volume.
        let gain = model.map(Model::level_gain).unwrap_or(1.0);
        if let Some(analyzer) = model.and_then(|model| model.sound()) {
            let written = analyzer.written();
            if written != self.written {
                self.written = written;
                self.fresh_at = now;
                self.samples.resize(1024, 0.0);
                analyzer.latest(&mut self.samples);
            } else if now - self.fresh_at > METER_STALE {
                // Nothing arrives (a tap that stopped calling back): the
                // last block is not what plays now.
                self.samples.clear();
            }
            // Two halves of the newest block stand in for two channels.
            for (lane, half) in self.samples.chunks(512).take(2).enumerate() {
                let rms = gain * (half.iter().map(|s| s * s).sum::<f32>() / half.len().max(1) as f32).sqrt();
                let db = 20.0 * rms.max(1.0e-6).log10();
                target[lane] = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
            }
        }
        let mut moving = false;
        for lane in 0..2 {
            let level = &mut self.level[lane];
            *level = if target[lane] >= *level { target[lane] } else { (*level - 1.8 * dt).max(target[lane]) };
            moving |= *level > 0.0005;
        }
        // Peak hold: held 1.2 s, then falling.
        let top = self.level[0].max(self.level[1]);
        if top >= self.peak {
            self.peak = top;
            self.peak_at = now;
        } else if now - self.peak_at > 1.2 {
            self.peak = (self.peak - 0.6 * dt).max(top);
        }
        moving |= self.peak > 0.0005;
        // The needle: critically damped toward the louder channel (~0.3 s).
        let k = 90.0f32;
        let accel = k * (top - self.needle) - 2.0 * k.sqrt() * self.needle_v;
        self.needle_v += accel * dt;
        self.needle = (self.needle + self.needle_v * dt).clamp(-0.05, 1.1);
        moving |= self.needle.abs() > 0.0005 || self.needle_v.abs() > 0.0005;
        let level = self.level;
        // The form that fits the rect: the dial where there is room for
        // one, else the bars (never a dial shrunk to a crumb).
        let rect = cx.peek_walk_turtle(walk);
        let bars = self.forms && (rect.size.y < 60.0 || rect.size.x > rect.size.y * 3.5);
        for (id, show) in [(ids!(face), !bars), (ids!(bar_face), bars)] {
            let widget = self.view.widget(cx, id);
            if widget.visible() != show {
                widget.set_visible(cx, show);
            }
        }
        let id = if bars { ids!(bar_face) } else { ids!(face) };
        if let Some(mut face) = self.view.relief_view(cx, id).borrow_mut() {
            face.view.draw_bg.set_uniform(cx, live_id!(level), &[level[0], level[1], self.peak, self.needle]);
            face.lit = ((level[0] + level[1]) * 0.5) as f64;
        }
        let step = self.view.draw_walk(cx, scope, walk);
        if moving {
            self.next_frame = cx.new_next_frame();
        }
        step
    }
}

/// `MUSIC_RELIEF_STATS=1`: what the relief buffer did, logged when it
/// changes and at most once a second (for the at-rest proof: nothing
/// rebuilt, nothing repainted), so the log does not overflow while playing.
#[derive(Default)]
struct StatsLog {
    last: Option<(usize, u64, u64)>,
    at: f64,
}

pub fn log_relief_stats(cx: &mut Cx) {
    if std::env::var_os("MUSIC_RELIEF_STATS").is_none() {
        return;
    }
    let now = cx.seconds_since_app_start();
    let windows: Vec<WindowId> = cx.windows.id_iter().collect();
    for window in windows {
        let Some(st) = relief_stats(cx, window) else { continue };
        let key = (st.proxies, st.rebuilds, st.source_painted);
        let log = cx.global::<StatsLog>();
        if log.last == Some(key) || now - log.at < 1.0 {
            continue;
        }
        log.last = Some(key);
        log.at = now;
        log!(
            "relief: proxies={} rebuilds={} source_painted={} source_dirty={} output_painted={}",
            st.proxies, st.rebuilds, st.source_painted, st.source_dirty, st.output_painted
        );
    }
}

// ---- readout ----

/// A label on a display that lights the glass round it (a Nixie tube, a
/// phosphor readout): its text rect goes into the relief buffer as an
/// emissive surface.
#[derive(Script, ScriptHook, Widget)]
pub struct HwReadout {
    #[deref]
    label: Label,
    #[live]
    glow: Vec4f,
    #[live(3.0)]
    glow_pad: f64,
}

impl Widget for HwReadout {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.label.draw_walk(cx, scope, walk);
        if self.glow.w > 0.0 && !self.label.text().is_empty() {
            let slot = relief_open(cx, 0.0);
            relief_close(cx, slot, self.label.area(), Some(ReliefProxy { inset: -self.glow_pad, radius: 3.0, disc: false, emissive: self.glow }));
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.label.handle_event(cx, event, scope);
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.label.set_text(cx, v);
    }

    fn text(&self) -> String {
        self.label.text()
    }
}

#[cfg(test)]
mod contrast_tests {
    use super::*;

    /// Every style: the rows' text, a playing row's accent and the muted
    /// text over the list floor, the hover and the selection.
    #[test]
    fn every_style_reads_on_its_row_tints() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/styles");
        let hexv = |t: &str| {
            let h = t.trim().trim_start_matches("#x");
            let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f32 / 255.0;
            vec4(v(0), v(2), v(4), 1.0)
        };
        let mut bad = Vec::new();
        let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "splash")).collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path).unwrap();
            let key = |k: &str, d: &str| {
                let at = format!("mod.widgets.hw_palette.{k} = ");
                text.lines().find_map(|l| l.strip_prefix(at.as_str())).map(hexv).unwrap_or_else(|| hexv(d))
            };
            let (bg, fg, accent) = (key("screen_bg", "#x0e1012"), key("screen_text", "#xe6e8ea"), key("accent", "#x39c6ff"));
            let (sel, hover) = row_tint_alphas(bg, fg, accent);
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            for (what, alpha) in [("hover", hover), ("selection", sel)] {
                let floor = mix_v(bg, accent, alpha);
                let (t, a) = (contrast(fg, floor), contrast(accent, floor));
                println!("{name:10} {what:9} a={alpha:.2} text {t:5.2} accent {a:5.2}");
                if t < 4.5 || a < 3.0 {
                    bad.push(format!("{name} {what}: text {t:.2} accent {a:.2}"));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn row_tints_leave_the_text_readable() {
        // Light text on a dark list with a pale accent: the selection gives
        // way until the text reads.
        let bg = vec4(0.05, 0.05, 0.06, 1.0);
        let text = vec4(0.6, 0.62, 0.64, 1.0);
        let accent = vec4(0.9, 0.9, 0.85, 1.0);
        let (sel, hover) = row_tint_alphas(bg, text, accent);
        assert!(sel < 0.30 && sel >= 0.12);
        assert!(hover <= 0.10);
        // A saturated accent on black keeps the usual strengths.
        assert_eq!(row_tint_alphas(vec4(0.02, 0.02, 0.03, 1.0), vec4(0.9, 0.93, 0.95, 1.0), vec4(0.1, 0.85, 1.0, 1.0)), (0.30, 0.10));
    }

    #[test]
    fn menu_ink_is_readable_on_its_floor() {
        let cream = vec4(0.937, 0.914, 0.855, 1.0);
        let white_text = vec4(0.95, 0.93, 0.88, 1.0);
        let dark = vec4(0.08, 0.08, 0.09, 1.0);
        assert!(contrast(white_text, cream) < 4.5);
        let ink = readable(white_text, cream, &[white_text, dark]);
        assert!(contrast(ink, cream) >= 4.5);
        // A readable ink is kept.
        let navy = vec4(0.05, 0.1, 0.2, 1.0);
        assert_eq!(readable(white_text, navy, &[dark]), white_text);
    }
}
