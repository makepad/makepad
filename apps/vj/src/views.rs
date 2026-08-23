//! Presentation widgets: the crossfading video program view and the tile
//! grid shared by every catalog surface.
//!
//! `VideoProgram` composites the two playback-slot textures with an
//! aspect-preserving letterbox per source and a mix factor driven by the cue
//! engine's timed fade — one instance in the output window, one as the
//! console preview (textures are shared handles).
//!
//! `VjTileGrid` is the ai-content gallery pattern: PortalList rows of fixed
//! card slots. Recycled rows restart from the template with no texture, so
//! every visible pass rebinds thumbnails from the entry list — textures are
//! keyed upstream by immutable revision, never by list position.

use makepad_asset_data::AssetId;
use makepad_widgets::*;
use crate::gen::{GenJob, GenJobState, GenJobTone};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawProgram::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_a: texture_2d(float)
        tex_b: texture_2d(float)

        // The program is a 16:9 canvas letterboxed into this quad. Effects
        // warp CANVAS coordinates (aspect-corrected), and warped samples
        // wrap with mirror-repeat so a kaleidoscope or tunnel fills the whole
        // canvas instead of falling off the edge into black. Each source is
        // letterboxed inside the canvas by its own aspect.
        canvas_aspect: fn() -> float {
            return 16.0 / 9.0
        }

        // MASTER FADEOUT: the final composite multiplies by 1-fadeout
        // right here in the existing pass — a clean blackout with zero
        // extra render-to-texture, on every path (plain mix, transition
        // engaged, all of it).
        fragment: fn(){
            let c = self.pixel()
            let keep = 1.0 - clamp(self.fadeout, 0.0, 1.0)
            self.fb0 = depth_clip(
                self.world,
                vec4(c.x * keep, c.y * keep, c.z * keep, c.w),
                self.depth_clip
            )
        }

        // Quad uv -> canvas uv (+ inside flag for the letterbox bars).
        to_canvas: fn(uv: vec2) -> vec3 {
            let ra = self.rect_size.x / max(self.rect_size.y, 1.0)
            let ca = self.canvas_aspect()
            let w = min(1.0, ca / ra)
            let h = min(1.0, ra / ca)
            let u = vec2(
                (uv.x - (1.0 - w) * 0.5) / w,
                (uv.y - (1.0 - h) * 0.5) / h
            )
            let inside = step(0.0, u.x) * step(u.x, 1.0)
                * step(0.0, u.y) * step(u.y, 1.0)
            return vec3(u.x, u.y, inside)
        }

        // Mirror-repeat: 0..1 stays, 1..2 reflects, and so on.
        wrap: fn(p: vec2) -> vec2 {
            let t = fract(p * 0.5) * 2.0
            return vec2(1.0, 1.0) - abs(vec2(1.0, 1.0) - t)
        }

        // One source letterboxed inside the canvas by its own aspect.
        fit_src: fn(p: vec2, aspect: float) -> vec3 {
            let ca = self.canvas_aspect()
            let w = min(1.0, aspect / ca)
            let h = min(1.0, ca / aspect)
            let u = vec2(
                (p.x - (1.0 - w) * 0.5) / w,
                (p.y - (1.0 - h) * 0.5) / h
            )
            let inside = step(0.0, u.x) * step(u.x, 1.0)
                * step(0.0, u.y) * step(u.y, 1.0)
            return vec3(u.x, u.y, inside)
        }

        // One bus, letterboxed and premultiplied, at canvas position `p`.
        sample_a: fn(p: vec2) -> vec4 {
            let f = self.fit_src(self.wrap(p), self.aspect_a)
            let u = clamp(vec2(f.x, f.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let s = self.tex_a.sample_as_bgra(u)
            let a = s.w * f.z * self.has_a
            return vec4(s.xyz * a, a)
        }

        sample_b: fn(p: vec2) -> vec4 {
            let f = self.fit_src(self.wrap(p), self.aspect_b)
            let u = clamp(vec2(f.x, f.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let s = self.tex_b.sample_as_bgra(u)
            let a = s.w * f.z * self.has_b
            return vec4(s.xyz * a, a)
        }

        // RGB -> hue in turns (0..1). Undefined hue (grey) returns 0.
        hue_of: fn(c: vec3) -> float {
            let hi = max(c.x, max(c.y, c.z))
            let lo = min(c.x, min(c.y, c.z))
            let d = hi - lo
            if d < 0.0001 {
                return 0.0
            }
            let mut h = 0.0
            if hi == c.x {
                h = modf((c.y - c.z) / d + 6.0, 6.0)
            } else if hi == c.y {
                h = (c.z - c.x) / d + 2.0
            } else {
                h = (c.x - c.y) / d + 4.0
            }
            return h / 6.0
        }

        // Shortest distance between two hues on the colour wheel (0..0.5).
        hue_dist: fn(x: float, y: float) -> float {
            let d = abs(x - y)
            return min(d, 1.0 - d)
        }

        // A wipe pattern's signed distance from its edge, in canvas units:
        // negative inside the B region, positive outside. `t` is the
        // crossfader (0 = no B, 1 = all B).
        wipe_field: fn(uv: vec2, t: float) -> float {
            let ca = self.canvas_aspect()
            let c = vec2((uv.x - 0.5) * ca, uv.y - 0.5)
            let mode = self.mix_mode
            if mode < 4.5 {
                // WIPE H: a vertical edge travelling across. FLIP reverses it.
                let x = mix(uv.x, 1.0 - uv.x, step(0.5, self.mix_p2))
                return x - t
            }
            if mode < 5.5 {
                let y = mix(uv.y, 1.0 - uv.y, step(0.5, self.mix_p2))
                return y - t
            }
            if mode < 6.5 {
                // BOX: a rectangle growing from one corner (CORNER picks it).
                let q = vec2(
                    mix(uv.x, 1.0 - uv.x, step(0.5, self.mix_p2)),
                    mix(uv.y, 1.0 - uv.y, step(0.25, self.mix_p2) * step(self.mix_p2, 0.75))
                )
                return max(q.x, q.y) - t
            }
            // IRIS: a circle (ASPECT squashes it toward an ellipse).
            let k = mix(1.0, 1.0 / ca, self.mix_p2)
            let e = vec2(c.x * k, c.y)
            // The half-diagonal is the radius that covers the whole canvas.
            let full = length(vec2(0.5 * ca * k, 0.5))
            return length(e) - t * full
        }

        // The downstream stage: how B reaches the program over A. Both
        // inputs are premultiplied; the result is opaque program.
        combine: fn(a: vec4, b: vec4, uv: vec2) -> vec4 {
            let t = self.mix_ab
            let mode = self.mix_mode
            if mode < 0.5 {
                // MIX: plain dissolve.
                return vec4(a.mix(b, t).xyz, 1.0)
            }
            if mode < 1.5 {
                // OVER: B over A by B's own alpha, faded in by the fader.
                let cover = b.w * t
                return vec4(b.xyz * t + a.xyz * (1.0 - cover), 1.0)
            }
            let mut key = 0.0
            if mode < 2.5 {
                // CHROMA: cut B wherever it matches the picked hue. HUE is
                // the wheel position, TOL the width; the same width again
                // is the soft shoulder, so a matte has an edge, not a
                // staircase.
                let un = b.xyz / max(b.w, 0.0001)
                let h = self.hue_of(un)
                let sat = max(un.x, max(un.y, un.z)) - min(un.x, min(un.y, un.z))
                let tol = mix(0.01, 0.35, self.mix_p2)
                let d = self.hue_dist(h, self.mix_p1)
                // Desaturated pixels have no hue to match: never keyed.
                let matched = (1.0 - smoothstep(tol, tol * 2.0, d)) * smoothstep(0.05, 0.2, sat)
                key = 1.0 - matched
            } else if mode < 3.5 {
                // LUMA: keep B where it is brighter than LEVEL, with SOFT
                // as the width of the ramp. Beyond 0.5, FLIP inverts.
                let un = b.xyz / max(b.w, 0.0001)
                let y = dot(un, vec3(0.299, 0.587, 0.114))
                let soft = mix(0.005, 0.5, self.mix_p2)
                key = smoothstep(self.mix_p1 - soft, self.mix_p1 + soft, y)
            } else {
                // Wipes: the crossfader IS the pattern's progress, and SOFT
                // is the edge width in canvas units.
                let d = self.wipe_field(uv, t)
                let soft = max(mix(0.0005, 0.35, self.mix_p1), 0.0005)
                key = 1.0 - smoothstep(-soft, soft, d)
            }
            // Every keyed/wiped mode reaches the program the same way: B's
            // own coverage times the key.
            let cover = b.w * clamp(key, 0.0, 1.0)
            return vec4(b.xyz * clamp(key, 0.0, 1.0) + a.xyz * (1.0 - cover), 1.0)
        }

        // Composite A/B at canvas position `p`, with the FX chain's warped
        // coordinate routed onto the bus (or buses) the operator selected.
        // `plain` is the unwarped canvas position, which is what the wipe
        // pattern and the untargeted bus use.
        src_at: fn(p: vec2, plain: vec2) -> vec4 {
            let mut pa = p
            let mut pb = p
            if self.fx_bus > 1.5 {
                pa = plain
            } else if self.fx_bus > 0.5 {
                pb = plain
            }
            return self.combine(self.sample_a(pa), self.sample_b(pb), plain)
        }

        // Composite with the FX chain on every bus (no routing) — the plain
        // read used before any effect warps anything.
        src: fn(p: vec2) -> vec4 {
            return self.combine(self.sample_a(p), self.sample_b(p), p)
        }

        // Rotate a colour around the grey axis and re-saturate.
        hue_rot: fn(c: vec3, ang: float, sat: float) -> vec3 {
            let k = vec3(0.57735027, 0.57735027, 0.57735027)
            let rotated = c * cos(ang) + cross(k, c) * sin(ang) + k * dot(k, c) * (1.0 - cos(ang))
            let grey = vec3(dot(rotated, vec3(0.299, 0.587, 0.114)))
            return grey.mix(rotated, sat)
        }

        pulse: fn(base: float, link: float) -> float {
            let hit = 1.0 - self.fx_beat
            return mix(base, mix(base * 0.12, base, hit), link)
        }

        pixel: fn() {
            let canvas = self.to_canvas(self.pos)
            if canvas.z < 0.5 {
                return vec4(0.0, 0.0, 0.0, 1.0)
            }
            let uv = vec2(canvas.x, canvas.y)
            let p1 = self.pulse(self.fx_p1, self.fx_link1)
            let p2 = self.pulse(self.fx_p2, self.fx_link2)
            // Aspect-corrected centred coordinates: circles are circles.
            let ca = self.canvas_aspect()
            let c = vec2((uv.x - 0.5) * ca, uv.y - 0.5)
            let back = vec2(1.0 / ca, 1.0)
            let kind = self.fx_kind
            let tau = 6.2831853
            let rgb = self.src(uv)
            if kind < 0.5 {
                return rgb
            }
            if kind < 1.5 {
                // KALEIDO: SEGS folds the angle, SPIN is an accumulated rotation.
                let segs = floor(mix(2.0, 12.0, p1) + 0.5)
                let ang = atan2(c.y, c.x) + self.fx_phase2
                let slice = tau / max(segs, 2.0)
                let folded = abs(modf(ang + slice * 100.0, slice) - slice * 0.5)
                let r = length(c)
                let w = vec2(cos(folded), sin(folded)) * r
                return self.src_at(w * back + vec2(0.5, 0.5), uv)
            }
            if kind < 2.5 {
                // TUNNEL: DEPTH is the fly speed (phase1), SPIN rotates (phase2).
                let r = length(c)
                let ang = atan2(c.y, c.x) / tau + self.fx_phase2
                let z = 0.25 / max(r, 0.02) + self.fx_phase1
                return self.src_at(vec2(fract(ang) * 2.0, fract(z)), uv)
            }
            if kind < 3.5 {
                let axis = step(0.5, p1)
                let mu = vec2(abs(c.x), c.y) * axis + vec2(c.x, abs(c.y)) * (1.0 - axis)
                return rgb.mix(self.src_at(mu * back + vec2(0.5, 0.5), uv), p2)
            }
            if kind < 4.5 {
                let a = p2 * tau
                let off = vec2(cos(a), sin(a)) * p1 * 0.08
                return vec4(self.src_at(uv + off, uv).x, rgb.y, self.src_at(uv - off, uv).z, 1.0)
            }
            if kind < 5.5 {
                // STROBE: RATE is accumulated (phase1), DUTY is the
                // on-fraction. A colour effect, so it is applied to the
                // routed bus BEFORE the downstream stage combines them —
                // strobing only B under a key is the point.
                let duty = mix(0.04, 0.7, p2)
                let gate = step(fract(self.fx_phase1), duty)
                let mut a = self.sample_a(uv)
                let mut b = self.sample_b(uv)
                if self.fx_bus < 1.5 {
                    a = vec4(a.xyz * gate, a.w)
                }
                if self.fx_bus < 0.5 || self.fx_bus > 1.5 {
                    b = vec4(b.xyz * gate, b.w)
                }
                return self.combine(a, b, uv)
            }
            if kind < 6.5 {
                // PIXEL: a real mosaic. SIZE is how low-res the picture
                // gets (log scale, 120 rows of blocks down to 3), SNAP
                // quantises that to power-of-two steps so the block size
                // JUMPS on the beat instead of sliding.
                //
                // The old branch blended the source uv with a corner-snapped
                // uv, which offsets every block by a fraction of itself —
                // that is the "tile effect" the picture showed, not
                // pixelation. Quantising to the block CENTRE makes every
                // pixel in a block sample one texel, so the block is flat
                // whatever the sampler filter does.
                let rows = exp(mix(log(120.0), log(3.0), p1))
                let stepped = pow(2.0, floor(log(rows) / log(2.0) + 0.5))
                let ry = max(floor(mix(rows, stepped, p2) + 0.5), 2.0)
                // Square blocks: the canvas is 16:9, so there are `ca` times
                // as many columns as rows.
                let rx = max(floor(ry * ca + 0.5), 2.0)
                let cells = vec2(rx, ry)
                return self.src_at((floor(uv * cells) + vec2(0.5, 0.5)) / cells, uv)
            }
            if kind < 7.5 {
                let r = length(c)
                let fall = 1.0 - smoothstep(0.0, mix(0.2, 1.1, p2), r)
                let a = p1 * 6.0 * fall
                let cs = cos(a)
                let sn = sin(a)
                let w = vec2(c.x * cs - c.y * sn, c.x * sn + c.y * cs)
                return self.src_at(w * back + vec2(0.5, 0.5), uv)
            }
            if kind < 8.5 {
                let r = length(c)
                let wave = sin(r * mix(6.0, 40.0, p2) - self.fx_time * 6.0)
                let dir = c / max(r, 0.0001)
                return self.src_at(uv + dir * back * wave * p1 * 0.08, uv)
            }
            if kind < 9.5 {
                let slices = mix(4.0, 28.0, p1)
                let row = floor(uv.y * slices)
                let n = fract(sin(row * 12.9898 + self.fx_time * 7.1) * 43758.5453)
                return self.src_at(vec2(uv.x + (n - 0.5) * p2 * 0.35, uv.y), uv)
            }
            if kind < 10.5 {
                // HUE: rotate the routed bus around the grey axis, then
                // combine — so a chroma key still keys the ORIGINAL hue of
                // the bus it is cutting when the FX sits on the other one.
                let ang = p1 * tau
                let sat = mix(0.0, 1.6, p2)
                let mut sa = self.sample_a(uv)
                let mut sb = self.sample_b(uv)
                if self.fx_bus < 1.5 {
                    sa = vec4(self.hue_rot(sa.xyz, ang, sat), sa.w)
                }
                if self.fx_bus < 0.5 || self.fx_bus > 1.5 {
                    sb = vec4(self.hue_rot(sb.xyz, ang, sat), sb.w)
                }
                return self.combine(sa, sb, uv)
            }
            if kind < 11.5 {
                let punch = 1.0 + p2 * (1.0 - self.fx_beat) * 0.55
                let z = mix(1.0, 2.4, p1) * punch
                return self.src_at(c / z * back + vec2(0.5, 0.5), uv)
            }
            let r = length(c)
            let f = 1.0 + r * r * mix(0.2, 2.4, p1)
            return rgb.mix(self.src_at(c * f * back + vec2(0.5, 0.5), uv), p2)
        }
    }

    mod.widgets.VideoProgramBase = #(VideoProgram::register_widget(vm))
    mod.widgets.VideoProgram = set_type_default() do mod.widgets.VideoProgramBase{
        width: Fill
        height: Fill
        // Karaoke subtitles. The size is set per draw from the picture's
        // height (the same widget is a 200px console preview and a 4K
        // projector output), so this is only the family and a sane default.
        draw_lyric +: {
            color: #xf2f6fa
            text_style: theme.font_bold{font_size: 24}
        }
    }

    // Same as asset-ui SpriteFitImage's rotation-aware, nearest-filtered
    // sampling (crisp Doom/Quake texels) — extended with a shader-only
    // ASPECT-FIT / ASPECT-FILL toggle. The widget's own Walk rect is
    // always full-bleed now (`ImageFit.Stretch`, no CPU aspect resize);
    // FIT (letterbox) vs FILL (cover, centre-cropped) is purely a UV remap
    // against the actual draw rect (`self.rect_size`) and the bound
    // texture's aspect (`img_aspect`). `fit_scale`/`cover_scale` in this
    // file are the pure-Rust equivalent of `fill_uv`'s min/max maths
    // (unit-tested in `tile_fit_tests`).
    let SpriteTileImage = Image{
        width: Fill
        height: Fill
        fit: ImageFit.Stretch
        draw_bg +: {
            // 1.0 = ASPECT-FILL (cover, default for grid/pad tiles): the
            // image fills the rect, cropping the overflow. 0.0 =
            // ASPECT-FIT (letterbox): the whole image stays visible,
            // inset with transparent bars — VjTileGrid/VjPadMatrix force
            // this to 0 whenever an entry has more than one frame (a
            // sprite-sheet / billboard animation), because cropping into
            // a packed sheet would show a neighbour cell.
            fill: instance(1.0)
            // Aspect (w/h) of the currently bound texture; the host
            // pushes this in every time it rebinds the thumbnail.
            img_aspect: instance(1.0)
            // FIT inscribes the image with `min` (the axis that would
            // overflow shrinks below 1, centring the image with bars on
            // the other axis); FILL covers with `max` (the axis that
            // would underflow instead grows past 1, cropping it) — same
            // computation, `mix`ed by `fill`. Returns (u, v, alpha-mask):
            // FIT's mask is 0 outside the inscribed image (the letterbox
            // bar); FILL's mask is always 1 (it always fully covers, so
            // the clamp below never smears real content, just guards
            // float roundoff at the very edge).
            fill_uv: fn(p: vec2) -> vec3 {
                let ra = max(self.rect_size.x, 1.0) / max(self.rect_size.y, 1.0)
                let ia = max(self.img_aspect, 0.0001)
                let ratio = ia / ra
                // Not all the way to cover: full FILL crops a 16:9 clip's
                // sides so hard the subject leaves the tile. `fill` is the
                // BLEND between contain and cover (see TILE_CROP), so the
                // picture reads big without losing its middle.
                let w = mix(min(1.0, ratio), max(1.0, ratio), self.fill)
                let h = mix(min(1.0, 1.0 / ratio), max(1.0, 1.0 / ratio), self.fill)
                let u = vec2(
                    (p.x - (1.0 - w) * 0.5) / w,
                    (p.y - (1.0 - h) * 0.5) / h
                )
                let inside = step(0.0, u.x) * step(u.x, 1.0) * step(0.0, u.y) * step(u.y, 1.0)
                let uc = clamp(u, vec2(0.0, 0.0), vec2(1.0, 1.0))
                return vec3(uc.x, uc.y, inside)
            }
            get_color_scale_pan: fn(scale: vec2, pan: vec2) {
                if self.image_dim_w > 0.0 {
                    let angle = self.rotation * 3.141592653589793 / 180.0
                    let cos_a = cos(-angle)
                    let sin_a = sin(-angle)
                    let c = (self.pos - vec2(0.5, 0.5)) * self.rect_size
                    let cr = vec2(c.x * cos_a - c.y * sin_a, c.x * sin_a + c.y * cos_a)
                    let iuv = cr / vec2(self.image_dim_w, self.image_dim_h) + vec2(0.5, 0.5)
                    let uv = iuv * scale + pan
                    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                        return vec4(0.0, 0.0, 0.0, 0.0)
                    }
                    return self.image_texture.sample_nearest(uv)
                }
                let framed = self.fill_uv(self.pos)
                let uv = vec2(framed.x, framed.y) * scale + pan
                let c = self.image_texture.sample_nearest(uv)
                // Outside the inscribed picture the sample is a clamped
                // edge texel — drawing it read as "stretched edges". The
                // bars are BLACK now: opaque for photo/clip tiles (any
                // fill), transparent for pure-FIT sprites (fill 0) so the
                // pad face still shows around an icon.
                let bar = step(0.0001, self.fill)
                return vec4(c.xyz * framed.z, c.w * mix(framed.z, 1.0, bar))
            }
            // Subtle top/bottom vignette so the pad number (PadCell's top
            // row) and the title (both cells' bottom row) still read over
            // a full-bleed FILL photo. Scaled by `fill` so it never
            // touches FIT sprite icons; TileCell's own opaque label
            // backdrop (`#x000000b8`) already covers its bottom row, so
            // this is redundant-but-harmless there and only load-bearing
            // for PadCell, which has no backdrop of its own.
            edge_scrim: fn(y: float) -> float {
                // smoothstep(edge0, edge1, x) is only well-defined for
                // edge0 < edge1, so the top fade is built as an inverted
                // ascending smoothstep rather than a descending one.
                let top = 1.0 - smoothstep(0.0, 0.32, y)
                let bottom = smoothstep(0.62, 1.0, y)
                return max(top, bottom) * 0.45 * self.fill
            }
            pixel: fn() {
                let color = mix(self.get_color(), #3, self.async_load)
                let dim = 1.0 - self.edge_scrim(self.pos.y)
                return Pal.premul(vec4(color.xyz * dim, color.w * self.opacity))
            }
        }
    }
    // The tile says "your click landed and I am working on it".
    //
    // A cue is a fetch, a decode and an upload — a second or more for a
    // world — and until this existed the only sign a click had registered
    // was that eventually the picture changed. It sits ON TOP of the
    // thumbnail (last child of the overlay stack), scrims it so the ring
    // reads over any picture, and turns into a still red ring when the load
    // failed. `spin` is fed from the app clock at draw time, like the beat
    // LED's `since`, so nothing here holds animation state.
    let TileBusy = SolidView{
        width: Fill
        height: Fill
        visible: false
        draw_bg +: {
            spin: instance(0.0)
            failed: instance(0.0)
            color_ring: uniform(#xff5c39)
            color_fail: uniform(#xff5c5c)
            pixel: fn() {
                let p = (self.pos - vec2(0.5, 0.5)) * self.rect_size
                let radius = min(self.rect_size.x, self.rect_size.y) * 0.17
                let width = max(radius * 0.34, 1.5)
                let ring = 1.0 - smoothstep(
                    width * 0.5 - 0.75,
                    width * 0.5 + 0.75,
                    abs(length(p) - radius)
                )
                let tau = 6.283185307179586
                // A comet head that fades round the circle reads as motion
                // from one frame alone; a FAILED load lights the whole ring
                // instead, so a stopped spinner can never be mistaken for a
                // slow one. Sweep runs spin-minus-angle so the BRIGHT head
                // LEADS in the direction of rotation and the tail fades out
                // behind it — the other way round it read as spinning
                // backwards.
                let sweep = fract((self.spin - atan2(p.y, p.x)) / tau)
                let comet = (1.0 - sweep) * (1.0 - sweep)
                let mask = ring * mix(comet, 1.0, self.failed)
                let tint = self.color_ring.mix(self.color_fail, self.failed)
                let scrim = 0.42
                return Pal.premul(vec4(
                    vec3(0.0, 0.0, 0.0).mix(tint.rgb, mask),
                    max(mask, scrim)
                ))
            }
        }
    }
    let TileCell = RoundedView{
        width: 164
        height: 104
        padding: 0
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: #x1e232b
            border_color: #xffffff2a
            border_color_selected: #xff5c39
            selected: instance(0.0)
            border_size: 1.0
            border_radius: 7.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, self.border_radius)
                sdf.fill_keep(self.color)
                let stroke = self.border_color.mix(self.border_color_selected, self.selected)
                sdf.stroke(stroke, self.border_size + self.selected)
                return sdf.result
            }
        }
        View{
            width: Fill
            height: Fill
            flow: Overlay
            View{
                width: Fill
                height: Fill
                padding: 3
                align: Align{x: 0.5 y: 0.5}
                grid_thumb := SpriteTileImage{}
            }
            View{
                width: Fill
                height: Fill
                flow: Down
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 4
                    padding: Inset{left: 6.0 right: 6.0 top: 5.0 bottom: 0.0}
                    align: Align{x: 0.0, y: 0.5}
                    grid_pad := Label{
                        text: ""
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 8}
                    }
                    View{width: Fill height: 1}
                    grid_state := Label{
                        text: ""
                        draw_text.color: #xff5c39
                        draw_text.text_style.font_size: 8
                    }
                }
                View{width: Fill height: Fill}
                SolidView{
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: Inset{left: 6.0 right: 6.0 top: 8.0 bottom: 5.0}
                    draw_bg.color: #x000000b8
                    grid_title := Label{
                        width: Fill
                        text: ""
                        draw_text.color: #xf2f6fa
                        draw_text.text_style.font_size: 9
                    }
                    grid_sub := Label{
                        width: Fill
                        text: ""
                        draw_text.color: #xb4bec8
                        draw_text.text_style.font_size: 8
                    }
                }
            }
            grid_busy := TileBusy{}
        }
    }

    let JobProgressBar = SolidView{
        width: Fill
        height: 6
        draw_bg +: {
            progress: uniform(0.0)
            color_track: uniform(#x343e4a)
            color_fill: uniform(#x4f9ee8)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 2.0)
                sdf.fill(self.color_track)
                let w = max(clamp(self.progress, 0.0, 1.0) * self.rect_size.x, 4.0)
                let visible = clamp(self.progress * 1000.0, 0.0, 1.0)
                sdf.box(0.0, 0.0, w, self.rect_size.y, 2.0)
                sdf.fill(vec4(self.color_fill.rgb, self.color_fill.a * visible))
                return sdf.result
            }
        }
    }

    mod.widgets.VjJobListBase = #(VjJobList::register_widget(vm))
    mod.widgets.VjJobList = set_type_default() do mod.widgets.VjJobListBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            spacing: 4
            // VJ law: a drag belongs to a control, never to a view.
            drag_scrolling: false
            JobRow := RoundedView{
                width: Fill
                height: Fit
                flow: Down
                spacing: 4
                padding: 7
                draw_bg +: {
                    color: #x222831
                    border_color: #xffffff2a
                    border_size: 1.0
                    border_radius: 6.0
                }
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{y: 0.5}
                    job_title := Label{
                        width: Fill
                        text: ""
                        draw_text.color: #xe5edf5
                        draw_text.text_style.font_size: 9
                    }
                    job_progress_text := Label{
                        text: ""
                        draw_text.color: #x73b9ef
                        draw_text.text_style.font_size: 8
                    }
                    job_cancel := Button{text: "Stop"}
                }
                job_stage := Label{visible: false width: Fill text: ""}
                job_message := Label{visible: false width: Fill text: ""}
                job_meta := Label{visible: false width: Fill text: ""}
                job_elapsed := Label{visible: false text: ""}
                job_progress := JobProgressBar{}
            }
            JobEmpty := View{
                width: Fill
                height: 28
                align: Align{x: 0.5, y: 0.5}
                Label{
                    text: "empty"
                    draw_text.color: #x8e9aa7
                }
            }
        }
    }

    mod.widgets.VjTileGridBase = #(VjTileGrid::register_widget(vm))
    mod.widgets.VjTileGrid = set_type_default() do mod.widgets.VjTileGridBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            spacing: 6
            // VJ law: a drag belongs to a control, never to a view.
            drag_scrolling: false
            Row := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                c1 := TileCell{}
                c2 := TileCell{}
                c3 := TileCell{}
                c4 := TileCell{}
                c5 := TileCell{}
                c6 := TileCell{}
                c7 := TileCell{}
                c8 := TileCell{}
            }
            Empty := View{
                width: Fill
                height: 60
                align: Align{x: 0.5, y: 0.5}
                empty_label := Label{
                    text: "no results"
                    draw_text.color: #x8e9aa7
                }
            }
        }
    }

    let PadCell = RoundedView{
        width: Fill
        height: Fill
        padding: 0
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: #x1d222a
            border_color: #xffffff2e
            border_color_selected: #xff5c39
            selected: instance(0.0)
            empty: instance(0.0)
            border_size: 1.0
            border_radius: 5.0
            // One state, one mark: the tile last clicked wears a green
            // ring, every other tile is a plain outline. Anything else the
            // grid used to paint (LIVE / CUE, dim second-place rings) read
            // as noise across forty pads.
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, self.border_radius)
                // Empty pads fade to a faint outline.
                sdf.fill_keep(self.color.mix(vec4(0.0, 0.0, 0.0, 0.0), self.empty * 0.85))
                sdf.stroke(
                    self.border_color.mix(self.border_color_selected, self.selected)
                        * (1.0 - self.empty * 0.6),
                    self.border_size + self.selected * 1.5
                )
                return sdf.result
            }
        }
        View{
            width: Fill
            height: Fill
            flow: Overlay
            View{
                width: Fill
                height: Fill
                padding: 3
                align: Align{x: 0.5 y: 0.5}
                grid_thumb := SpriteTileImage{}
            }
            View{
                width: Fill
                height: Fill
                flow: Down
                padding: 3
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    grid_pad := Label{
                        text: ""
                        draw_text.color: #xff5c39cc
                        draw_text.text_style: theme.font_bold{font_size: 7}
                    }
                    View{width: Fill height: 1}
                }
                View{width: Fill height: Fill}
                // Small name so a pad is readable before its thumbnail lands.
                grid_title := Label{
                    width: Fill
                    flow: Flow.Right{wrap: false}
                    max_lines: 1
                    text: ""
                    draw_text.color: #xd6dee6
                    draw_text.text_style.font_size: 7
                }
                grid_sub := Label{
                    visible: false
                    width: Fill
                    text: ""
                }
            }
            grid_busy := TileBusy{}
        }
    }

    // ---- the chrome bar's beat cluster ------------------------------------
    //
    // A miniature of the zoomed DJ waveform: the captured envelope of the
    // last couple of seconds, with the beat grid ruled over it in the SAME
    // time axis, so a ruling that sits on a transient means the clock is on
    // the music. Everything is a single quad reading one 256-texel envelope
    // texture — the whole picture is uniforms, not geometry.
    set_type_default() do #(DrawBeatWave::script_shader(vm)){
        ..mod.draw.DrawQuad
        wave: texture_2d(float)

        color_bg: uniform(#x0b1016)
        color_wave: uniform(#x2fb894)
        color_core: uniform(#x9df3d8)
        color_grid: uniform(#xffffff2b)
        color_grid_bar: uniform(#xffffffa0)
        color_dead: uniform(#x243039)

        // The envelope at an age in seconds. Column 0 of the texture is the
        // OLDEST kept column; `cols - 1` is the newest.
        env_at: fn(age: float) -> vec2 {
            let c = clamp(age * self.wave_hz, 0.0, max(self.cols - 1.0, 0.0))
            let u = (self.cols - 1.0 - c + 0.5) / max(self.tex_w, 1.0)
            let t = self.wave.sample_as_bgra(vec2(u, 0.5))
            return vec2(t.x, t.y)
        }

        // One pixel covers more than one column at this scale, and a peak
        // that falls between two samples is exactly the transient the eye is
        // looking for — so a pixel takes the LOUDEST column it covers.
        env_px: fn(age: float, span: float) -> vec2 {
            let a = self.env_at(age - span * 0.5)
            let b = self.env_at(age)
            let c = self.env_at(age + span * 0.5)
            return vec2(max(max(a.x, b.x), c.x), max(max(a.y, b.y), c.y))
        }

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let width = max(self.rect_size.x, 1.0)
            // Right edge = now (minus the capture ring's own latency).
            let age = self.right_age + (1.0 - self.pos.x) * self.window_secs
            let span = self.window_secs / width
            let e = self.env_px(age, span)

            // The grid, ruled on the same axis as the envelope above.
            let b = self.beat_at_right - age / max(self.beat_secs, 0.0001)
            let nb = floor(b + 0.5)
            let px_per_beat = self.beat_secs / max(self.window_secs, 0.0001) * width
            let d = abs(b - nb) * px_per_beat
            let is_bar = step(modf(nb + 4096.0, 4.0), 0.5)
            // The one rules floor to ceiling; the other three are a short
            // tick in the middle, so a bar is countable at a glance.
            let half = mix(0.4, 0.9, is_bar)
            let vpos = abs(self.pos.y - 0.5) * 2.0
            let reach = mix(0.62, 1.0, is_bar)
            let ga = (1.0 - smoothstep(half, half + 1.0, d))
                * (1.0 - smoothstep(reach - 0.12, reach, vpos))
                * self.grid_on
            let grid_c = self.color_grid.mix(self.color_grid_bar, is_bar)

            // Mirrored envelope: peak outside, RMS core inside.
            let feather = 2.0 / max(self.rect_size.y, 2.0)
            let peak = clamp(e.x, 0.0, 1.0) * 0.92
            let core = clamp(e.y, 0.0, 1.0) * 0.92
            let in_peak = 1.0 - smoothstep(peak - feather, peak + feather, vpos)
            let in_core = 1.0 - smoothstep(core - feather, core + feather, vpos)

            let bg = self.color_bg
            // No capture: a quiet centre rule where the wave will be, so the
            // block reads as "nothing coming in", never as "broken".
            let dead = 1.0 - smoothstep(0.5, 1.5, vpos * self.rect_size.y * 0.5)
            let base = bg.mix(self.color_dead, dead * (1.0 - self.live))

            let under = base.mix(vec4(grid_c.x, grid_c.y, grid_c.z, 1.0), grid_c.w * ga)
            let wave_c = self.color_wave.mix(self.color_core, in_core)
            let body = under.mix(vec4(wave_c.x, wave_c.y, wave_c.z, 1.0), in_peak * self.live)
            // A whisper of the ruling survives on top of a loud passage.
            let ruled = body.mix(vec4(grid_c.x, grid_c.y, grid_c.z, 1.0), grid_c.w * ga * 0.35)

            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 1.5)
            sdf.fill(ruled)
            return sdf.result
        }
    }
    mod.widgets.VjBeatWaveBase = #(VjBeatWave::register_widget(vm))
    mod.widgets.VjBeatWave = set_type_default() do mod.widgets.VjBeatWaveBase{
        width: 120
        height: 22
    }

    // The lock LED: one dot that flashes on every beat of whatever clock is
    // driving the room, brighter and warmer on the one. Brightness is a
    // function of the phase uniform alone — the host never animates it.
    set_type_default() do #(DrawBeatLed::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_off: uniform(#x1b232c)
        color_rim: uniform(#xffffff2e)
        color_beat: uniform(#xff5c39)
        color_down: uniform(#xfff1c8)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            let r = min(self.rect_size.x, self.rect_size.y) * 0.5 - 3.0
            // Bright ON the beat, gone by `fall`. Squared so the attack is a
            // flash and the tail a glow, which is what the eye reads as a
            // pulse rather than a blink.
            let f = clamp(1.0 - self.since / max(self.fall, 0.001), 0.0, 1.0)
            let env = f * f * self.live
            let lit = self.color_beat.mix(self.color_down, self.accent)
            // The one throws a halo; the other three stay inside the dot.
            sdf.circle(c.x, c.y, r + 3.0)
            sdf.fill(vec4(lit.x, lit.y, lit.z, env * self.accent * 0.42))
            sdf.circle(c.x, c.y, r)
            sdf.fill_keep(self.color_off.mix(lit, env * mix(0.86, 1.0, self.accent)))
            sdf.stroke(self.color_rim, 1.0)
            return sdf.result
        }
    }
    mod.widgets.VjBeatLedBase = #(VjBeatLed::register_widget(vm))
    mod.widgets.VjBeatLed = set_type_default() do mod.widgets.VjBeatLedBase{
        width: 18
        height: 22
    }

    // SCRATCH SHUTTLE: a jog well with a sprung knob. `pos` -1..1 drawn
    // from the uniform alone; the widget springs it home on release.
    set_type_default() do #(DrawVjShuttle::script_shader(vm)){
        ..mod.draw.DrawQuad
        shuttle: uniform(0.0)
        active: uniform(0.0)
        color_well: uniform(#x1d222a)
        color_rim: uniform(#xffffff26)
        color_detent: uniform(#x39404a)
        color_knob: uniform(#xe8eef4)
        color_hot: uniform(#xff5c39)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            sdf.box(0.5, 2.0, w - 1.0, h - 4.0, 5.0)
            sdf.fill(self.color_well)
            sdf.stroke(self.color_rim, 1.0)
            // centre detent tick
            sdf.box(w * 0.5 - 0.75, 4.5, 1.5, h - 9.0, 0.75)
            sdf.fill(self.color_detent)
            // sprung knob: centre at the shuttle position
            let half = w * 0.5 - 7.0
            let kx = w * 0.5 + self.shuttle * half
            sdf.box(kx - 3.0, 3.5, 6.0, h - 7.0, 3.0)
            sdf.fill(self.color_knob.mix(self.color_hot, self.active))
            return sdf.result
        }
    }
    // BEATS-PER-SWEEP DROPDOWN: a chip that opens a compact list
    // (1/2/4/8/16 beats, — = free) in an overlay under itself.
    set_type_default() do #(DrawVjBeatsChip::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: uniform(0.0)
        open: uniform(0.0)
        inert: uniform(0.0)
        color: uniform(#x272e38)
        color_hover: uniform(#x2f3842)
        border_color: uniform(#xffffff26)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 5.0)
            sdf.fill(self.color.mix(self.color_hover, max(self.hover, self.open)))
            sdf.stroke(self.border_color, 1.0)
            // tiny drop arrow at the right edge
            let ax = self.rect_size.x - 8.0
            let ay = self.rect_size.y * 0.5 - 1.0
            sdf.move_to(ax - 2.5, ay)
            sdf.line_to(ax + 2.5, ay)
            sdf.line_to(ax, ay + 3.0)
            sdf.close_path()
            sdf.fill(vec4(0.66, 0.70, 0.75, 1.0 - self.inert * 0.6))
            return sdf.result * (1.0 - self.inert * 0.45)
        }
    }
    mod.widgets.VjBeatsDropBase = #(VjBeatsDrop::register_widget(vm))
    mod.widgets.VjBeatsDrop = set_type_default() do mod.widgets.VjBeatsDropBase{
        width: 34
        height: 22
        draw_text +: {
            color: #xf4f7fa
            text_style: theme.font_bold{font_size: 9}
        }
        draw_panel +: {
            color: #x181c23
            border_color: #xffffff2e
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 6.0)
                sdf.fill(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_hover +: {
            color: #xff5c39
        }
    }

    mod.widgets.VjShuttleBase = #(VjShuttle::register_widget(vm))
    mod.widgets.VjShuttle = set_type_default() do mod.widgets.VjShuttleBase{
        width: 72
        height: 22
    }

    mod.widgets.VjPadMatrixBase = #(VjPadMatrix::register_widget(vm))
    mod.widgets.VjPadMatrix = set_type_default() do mod.widgets.VjPadMatrixBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 4
        View{
            width: Fill
            height: Fill
            flow: Right
            spacing: 6
            View{
                width: Fill
                height: Fill
                flow: Down
                spacing: 4
                r0 := View{
                    width: Fill height: Fill flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r1 := View{
                    width: Fill height: Fill flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r2 := View{
                    width: Fill height: Fill flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r3 := View{
                    width: Fill height: Fill flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r4 := View{
                    width: Fill height: Fill flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
            }
        }
        // The strip's horizontal scrollbar. A painted view: an empty View
        // has no area, so the thumb would never find its track.
        // Layout slot only: it must NOT carry `cursor`, or the View
        // hit-tests and swallows the press before the track's own area
        // (drawn over it) ever sees a FingerDown — that is why the thumb
        // could not be grabbed.
        scroll_slot := View{
            width: Fill
            height: 10
            margin: Inset{top: 4}
        }
        draw_track +: {
            color: #x2b343f
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 3.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_thumb +: {
            color: #xff5c39
            hover: instance(0.0)
            down: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 3.0)
                sdf.fill(self.color.mix(#xd6fff0, self.hover * 0.5).mix(#xffffff, self.down * 0.4))
                return sdf.result
            }
        }
    }
}

// ---------------------------------------------------------------------------
// crossfading program view
// ---------------------------------------------------------------------------

/// Two-source crossfade blit. Per the draw-shader layout law, only `#[live]`
/// instance fields sit after the `#[deref]`.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawProgram {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub mix_ab: f32,
    #[live(1.7777)]
    pub aspect_a: f32,
    #[live(1.7777)]
    pub aspect_b: f32,
    #[live]
    pub has_a: f32,
    #[live]
    pub has_b: f32,
    /// Downstream mix mode: 0 dissolve, 1 over, 2 chroma key, 3 luma key,
    /// 4/5 wipe H/V, 6 corner box, 7 iris. See `crate::mix::MixId`.
    #[live]
    pub mix_mode: f32,
    /// The mode's two knobs (hue/level/soft, tolerance/flip/aspect).
    #[live]
    pub mix_p1: f32,
    #[live]
    pub mix_p2: f32,
    /// Which bus the FX chain is inserted on: 0 both, 1 A, 2 B.
    #[live]
    pub fx_bus: f32,
    #[live]
    pub fx_kind: f32,
    #[live]
    pub fx_p1: f32,
    #[live]
    pub fx_p2: f32,
    #[live]
    pub fx_link1: f32,
    #[live]
    pub fx_link2: f32,
    #[live]
    pub fx_beat: f32,
    #[live]
    pub fx_time: f32,
    /// Host-accumulated phases (speed knobs advance these; changing a
    /// speed changes the rate, never the position).
    #[live]
    pub fx_phase1: f32,
    #[live]
    pub fx_phase2: f32,
    /// Master video fadeout 0..1: the final composite dims to black by it
    /// (the crossfader cluster's FADEOUT knob).
    #[live]
    pub fadeout: f32,
}

/// One frame of karaoke, as the program should draw it: the line being sung,
/// the line after it, and how far the sweep has crossed the current one.
/// Text, not indices — the widget knows nothing about decks or transcripts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KaraokeOverlay {
    pub current: Option<String>,
    pub next: Option<String>,
    pub progress: f32,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VideoProgram {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_program: DrawProgram,
    /// Karaoke subtitles, drawn over the composited program. They live in the
    /// program widget rather than as a sibling overlay so that EVERY surface
    /// showing the program — the output window on the projector and the
    /// console's preview — carries the same words with no extra layout.
    #[live]
    draw_lyric: DrawText,
    #[rust]
    area: Area,
    #[rust]
    tex_a: Option<Texture>,
    #[rust]
    tex_b: Option<Texture>,
    #[rust]
    karaoke: Option<KaraokeOverlay>,
}

impl VideoProgram {
    /// Bind the slot textures + fade state for this frame. `mix_ab` 0 = all
    /// A, 1 = all B; aspects preserve each source's shape independently.
    /// `mix` is the downstream stage (dissolve / over / key / wipe) plus
    /// the bus the FX chain is routed onto.
    pub fn set_sources(
        &mut self,
        cx: &mut Cx,
        tex_a: Option<(Texture, f32)>,
        tex_b: Option<(Texture, f32)>,
        mix_ab: f32,
        mix: crate::mix::MixState,
    ) {
        self.draw_program.has_a = if tex_a.is_some() { 1.0 } else { 0.0 };
        self.draw_program.has_b = if tex_b.is_some() { 1.0 } else { 0.0 };
        if let Some((tex, aspect)) = tex_a {
            self.draw_program.aspect_a = aspect.max(0.05);
            self.tex_a = Some(tex);
        } else {
            self.tex_a = None;
        }
        if let Some((tex, aspect)) = tex_b {
            self.draw_program.aspect_b = aspect.max(0.05);
            self.tex_b = Some(tex);
        } else {
            self.tex_b = None;
        }
        self.draw_program.mix_ab = mix_ab.clamp(0.0, 1.0);
        self.draw_program.mix_mode = mix.mode.as_f32();
        self.draw_program.mix_p1 = mix.p1.clamp(0.0, 1.0);
        self.draw_program.mix_p2 = mix.p2.clamp(0.0, 1.0);
        self.draw_program.fx_bus = mix.bus.as_f32();
        self.area.redraw(cx);
    }

    /// Bind (or clear) the karaoke subtitle. Only a CHANGE redraws: the
    /// overlay is pushed every frame the program is pumped and the words
    /// change a few times a minute.
    /// Master video fadeout (post-everything dim to black).
    pub fn set_fadeout(&mut self, cx: &mut Cx, fadeout: f32) {
        let fadeout = fadeout.clamp(0.0, 1.0);
        if (self.draw_program.fadeout - fadeout).abs() > 1e-4 {
            self.draw_program.fadeout = fadeout;
            self.area.redraw(cx);
        }
    }

    pub fn set_karaoke(&mut self, cx: &mut Cx, overlay: Option<KaraokeOverlay>) {
        if self.karaoke == overlay {
            return;
        }
        self.karaoke = overlay;
        self.area.redraw(cx);
    }

}

impl WidgetNode for VideoProgram {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for VideoProgram {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        match &self.tex_a {
            Some(tex) => self.draw_program.draw_vars.set_texture(0, tex),
            None => self.draw_program.draw_vars.empty_texture(0),
        }
        match &self.tex_b {
            Some(tex) => self.draw_program.draw_vars.set_texture(1, tex),
            None => self.draw_program.draw_vars.empty_texture(1),
        }
        self.draw_program.draw_abs(cx, rect);
        self.area = self.draw_program.area();
        self.draw_karaoke(cx, rect);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// karaoke subtitles
// ---------------------------------------------------------------------------

/// Words not yet sung: near-white, because it has to read over anything.
const LYRIC_AHEAD: Vec4f = Vec4f { x: 0.95, y: 0.97, z: 1.0, w: 1.0 };
/// Words already sung: the console's accent, sweeping left to right.
const LYRIC_SUNG: Vec4f = Vec4f { x: 0.243, y: 0.878, z: 0.690, w: 1.0 };
/// The line after this one, dimmed — always on screen, always one ahead.
const LYRIC_NEXT: Vec4f = Vec4f { x: 0.78, y: 0.84, z: 0.90, w: 0.72 };
/// The outline. Video is not a background you can choose, so the text carries
/// its own: a ring of near-black stamped under the WHOLE line, so the sung
/// half and the unsung half are equally legible over anything.
const LYRIC_OUTLINE: Vec4f = Vec4f { x: 0.0, y: 0.0, z: 0.0, w: 0.92 };
/// Depth granted to each text pass. Comfortably above the 1e-6 a glyph index
/// contributes and far below the 10.0 the dock's overlays use.
const LYRIC_DEPTH_STEP: f32 = 0.01;

/// The eight directions the outline is stamped in.
const OUTLINE_RING: [(f64, f64); 8] = [
    (-1.0, 0.0),
    (1.0, 0.0),
    (0.0, -1.0),
    (0.0, 1.0),
    (-0.7, -0.7),
    (0.7, -0.7),
    (-0.7, 0.7),
    (0.7, 0.7),
];

impl VideoProgram {
    fn measure(&self, cx: &mut Cx2d, text: &str) -> (f64, f64) {
        let laid = self
            .draw_lyric
            .layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let scale = self.draw_lyric.font_scale;
        (
            (laid.size_in_lpxs.width * scale) as f64,
            (laid.size_in_lpxs.height * scale) as f64,
        )
    }

    /// Greedy word wrap against a measured width. Lyric lines are phrases, so
    /// this almost never fires; when a run-on segment does arrive it breaks
    /// rather than running off the projector.
    fn wrap_rows(&self, cx: &mut Cx2d, text: &str, max_width: f64) -> Vec<String> {
        if max_width <= 0.0 || self.measure(cx, text).0 <= max_width {
            return vec![text.to_string()];
        }
        let mut rows: Vec<String> = Vec::new();
        let mut row = String::new();
        for word in text.split_whitespace() {
            if row.is_empty() {
                row.push_str(word);
                continue;
            }
            let candidate = format!("{row} {word}");
            if self.measure(cx, &candidate).0 > max_width {
                rows.push(std::mem::take(&mut row));
                row.push_str(word);
            } else {
                row = candidate;
            }
        }
        if !row.is_empty() {
            rows.push(row);
        }
        if rows.is_empty() {
            rows.push(text.to_string());
        }
        rows
    }

    /// One centred row.
    ///
    /// The WHOLE line is drawn first, always: karaoke never reveals text a
    /// letter at a time — a singer has to be able to read the line before
    /// singing it. Then the part already sung is drawn again over the top in
    /// the accent colour, so the green boundary sweeps across letters that
    /// were legible all along. Text and progress are independent.
    ///
    /// Every pass takes its own `draw_depth` slice. Glyph quads carry their
    /// index as depth, so two passes over the same string land their k-th
    /// glyphs at exactly the same depth and the second one loses the depth
    /// test — which is a line that draws its outline and then silently drops
    /// half its letters. Stepping the depth per pass is what makes the
    /// overdraw legal.
    fn draw_lyric_row(
        &mut self,
        cx: &mut Cx2d,
        row: &str,
        centre_x: f64,
        top: f64,
        fill: Vec4f,
        sung_chars: usize,
        depth: &mut f32,
    ) {
        let (width, _) = self.measure(cx, row);
        let left = centre_x - width * 0.5;
        let ring = (self.draw_lyric.text_style.font_size as f64 * 0.075).max(1.25);
        self.draw_lyric.color = LYRIC_OUTLINE;
        for (dx, dy) in OUTLINE_RING {
            self.draw_lyric.draw_depth = *depth;
            *depth += LYRIC_DEPTH_STEP;
            self.draw_lyric
                .draw_abs(cx, dvec2(left + dx * ring, top + dy * ring), row);
        }
        self.draw_lyric.draw_depth = *depth;
        *depth += LYRIC_DEPTH_STEP;
        self.draw_lyric.color = fill;
        self.draw_lyric.draw_abs(cx, dvec2(left, top), row);
        let split = row
            .char_indices()
            .nth(sung_chars)
            .map(|(at, _)| at)
            .unwrap_or(row.len());
        let sung = &row[..split];
        if !sung.is_empty() {
            self.draw_lyric.draw_depth = *depth;
            *depth += LYRIC_DEPTH_STEP;
            self.draw_lyric.color = LYRIC_SUNG;
            self.draw_lyric.draw_abs(cx, dvec2(left, top), sung);
        }
    }

    /// The two-row karaoke block, anchored to the bottom of the picture:
    /// the line being sung on top with the sweep across it, the line after it
    /// dim below. The lower row is the whole point — a singer always has the
    /// next words in view before they are needed.
    fn draw_karaoke(&mut self, cx: &mut Cx2d, rect: Rect) {
        let Some(overlay) = self.karaoke.clone() else { return };
        if overlay.current.is_none() && overlay.next.is_none() {
            return;
        }
        if rect.size.x < 40.0 || rect.size.y < 30.0 {
            return;
        }
        // The subtitle scales with the picture, so the console preview and a
        // projector-sized output window read the same.
        let base = (rect.size.y as f32 * 0.052).clamp(9.0, 40.0);
        let max_width = rect.size.x * 0.90;
        let centre_x = rect.pos.x + rect.size.x * 0.5;

        self.draw_lyric.text_style.font_size = base;
        let current = overlay
            .current
            .as_ref()
            .map(|text| self.wrap_rows(cx, text, max_width))
            .unwrap_or_default();
        let current_height = if current.is_empty() {
            0.0
        } else {
            self.measure(cx, "Hg").1
        };

        self.draw_lyric.text_style.font_size = base * 0.72;
        let next = overlay
            .next
            .as_ref()
            .map(|text| self.wrap_rows(cx, text, max_width))
            .unwrap_or_default();
        let next_height = if next.is_empty() {
            0.0
        } else {
            self.measure(cx, "Hg").1
        };

        let gap = if current.is_empty() || next.is_empty() {
            0.0
        } else {
            current_height * 0.22
        };
        let block = current.len() as f64 * current_height
            + gap
            + next.len() as f64 * next_height;
        let margin = (rect.size.y * 0.06).max(6.0);
        let mut y = rect.pos.y + rect.size.y - margin - block;
        // A block taller than the picture is pinned to the top rather than
        // pushed off it.
        y = y.max(rect.pos.y + 2.0);

        let mut depth = 0.0f32;
        if !current.is_empty() {
            self.draw_lyric.text_style.font_size = base;
            let total: usize = current.iter().map(|row| row.chars().count()).sum();
            let mut sung = (overlay.progress.clamp(0.0, 1.0) as f64 * total as f64).round() as usize;
            for row in &current {
                let count = row.chars().count();
                let take = sung.min(count);
                sung -= take;
                self.draw_lyric_row(cx, row, centre_x, y, LYRIC_AHEAD, take, &mut depth);
                y += current_height;
            }
            y += gap;
        }
        if !next.is_empty() {
            self.draw_lyric.text_style.font_size = base * 0.72;
            for row in &next {
                self.draw_lyric_row(cx, row, centre_x, y, LYRIC_NEXT, 0, &mut depth);
                y += next_height;
            }
        }
        self.draw_lyric.draw_depth = 0.0;
    }
}

// ---------------------------------------------------------------------------
// generation job list
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct JobRowEntry {
    /// Engine tag the cancel button reports back.
    pub tag: u64,
    pub title: String,
    pub stage: String,
    pub message: String,
    pub meta: String,
    pub elapsed: String,
    pub progress: Option<f32>,
    pub progress_text: String,
    pub tone: GenJobTone,
    pub cancellable: bool,
}

impl JobRowEntry {
    pub fn from_job(job: &GenJob, now_ms: u64, queue_ahead: Option<usize>) -> JobRowEntry {
        let display = job.display(now_ms);
        let progress = display.progress_permille.map(|value| value as f32 / 1000.0);
        let progress_text = match display.progress_permille {
            Some(value) => format!("{:.1}%", value as f32 / 10.0),
            None => match &job.state {
                GenJobState::Submitting => "SENDING".to_string(),
                // A busy fleet is not a broken one: say WHERE the job
                // stands instead of a vague wait.
                GenJobState::Pending => match queue_ahead {
                    Some(0) => "NEXT".to_string(),
                    Some(n) => format!("#{}", n + 1),
                    None => "QUEUED".to_string(),
                },
                GenJobState::CancelRequested => "STOPPING".to_string(),
                GenJobState::Failed(_) => "FAILED".to_string(),
                GenJobState::Cancelled => "CANCELLED".to_string(),
                _ => "—".to_string(),
            },
        };
        JobRowEntry {
            tag: job.tag,
            title: job.title.clone(),
            stage: display.stage,
            message: display.message,
            meta: format!("profile: {} · {}", job.profile_label, display.assignment),
            elapsed: format_elapsed(display.elapsed_ms, job.state.is_terminal()),
            progress,
            progress_text,
            tone: display.tone,
            cancellable: !job.state.is_terminal()
                && !matches!(&job.state, GenJobState::CancelRequested),
        }
    }
}

fn format_elapsed(elapsed_ms: u64, terminal: bool) -> String {
    let total_seconds = elapsed_ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if terminal {
        format!("total {minutes}:{seconds:02}")
    } else {
        format!("elapsed {minutes}:{seconds:02}")
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjJobList {
    #[deref]
    view: View,
    #[rust]
    entries: Vec<JobRowEntry>,
}

impl VjJobList {
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<JobRowEntry>) {
        if self.entries != entries {
            self.entries = entries;
            self.view.redraw(cx);
        }
    }

    pub fn entry_at(&self, index: usize) -> Option<&JobRowEntry> {
        self.entries.get(index)
    }
}

impl Widget for VjJobList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else { continue };
            if self.entries.is_empty() {
                list.set_item_range(cx, 0, 1);
                while let Some(row_id) = list.next_visible_item(cx) {
                    if row_id >= 1 {
                        continue;
                    }
                    let item = list.item(cx, row_id, id!(JobEmpty));
                    item.draw_all(cx, &mut Scope::empty());
                }
                continue;
            }
            list.set_item_range(cx, 0, self.entries.len());
            while let Some(row_id) = list.next_visible_item(cx) {
                if row_id >= self.entries.len() {
                    continue;
                }
                let item = list.item(cx, row_id, id!(JobRow));
                if let Some(entry) = self.entries.get(row_id) {
                    item.label(cx, ids!(job_title)).set_text(cx, &entry.title);
                    item.label(cx, ids!(job_stage)).set_text(cx, &entry.stage);
                    let message = item.label(cx, ids!(job_message));
                    message.set_text(cx, &entry.message);
                    message.set_visible(cx, !entry.message.is_empty());
                    item.label(cx, ids!(job_meta)).set_text(cx, &entry.meta);
                    item.label(cx, ids!(job_elapsed)).set_text(cx, &entry.elapsed);
                    item.label(cx, ids!(job_progress_text))
                        .set_text(cx, &entry.progress_text);
                    let bar = item.view(cx, ids!(job_progress));
                    bar.set_uniform(cx, live_id!(progress), &[entry.progress.unwrap_or(0.0)]);
                    let fill: [f32; 4] = match entry.tone {
                        GenJobTone::Waiting => [0.76, 0.58, 0.24, 1.0],
                        GenJobTone::Active => [0.31, 0.62, 0.91, 1.0],
                        GenJobTone::Success => [0.35, 0.77, 0.63, 1.0],
                        GenJobTone::Failed => [0.88, 0.34, 0.31, 1.0],
                        GenJobTone::Cancelled => [0.42, 0.47, 0.53, 1.0],
                    };
                    bar.set_uniform(cx, live_id!(color_fill), &fill);
                    item.button(cx, ids!(job_cancel))
                        .set_visible(cx, entry.cancellable);
                }
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// tile grid
// ---------------------------------------------------------------------------

pub const GRID_SLOTS: usize = 8;
const CARD_W: f64 = 164.0;
const CARD_SPACING: f64 = 8.0;

#[derive(Clone)]
pub struct GridEntry {
    pub asset: AssetId,
    pub title: String,
    pub sub: String,
    pub state: String,
    pub pad: String,
    pub texture: Option<Texture>,
    pub frames: Vec<Texture>,
    pub fps: f32,
    /// The thumbnail DECLARED a cell layout, so what this tile shows is one
    /// of those cells: a sprite/mesh preview tile its producer already
    /// aspect-fit and centred, padding and all. Such a tile is shown WHOLE
    /// (see [`thumb_fill`]) — cropping into it eats the sprite, and for a
    /// single-frame actor there is no second frame to give the game away.
    /// Straight off the manifest, never measured.
    pub cells: bool,
    /// This tile's cue is being prepared right now — fetching, decoding,
    /// uploading. It wears the spinner until the media is ready (an ARMED
    /// cue is ready: it is only waiting for its beat).
    pub loading: bool,
    /// This tile's load failed; it wears a still red ring until the next
    /// click.
    pub failed: bool,
    /// The one marked tile: on the clip grid the last one CLICKED (green
    /// ring, nothing else), on the SFX bank a pad with voices playing.
    pub active: bool,
    /// A reserved-but-empty cell of the PENDING head column. It draws as
    /// the quiet grey placeholder and cannot be clicked — the column keeps
    /// its full height from the moment it opens so that filling it never
    /// moves a tile the operator is already reaching for.
    pub placeholder: bool,
    /// A PREFAB cell: a thing the app KNOWS is in the library (a compiled-in
    /// effect preset) whose catalog row has not come back yet. It draws in
    /// full — number, title, the same procedural art the store holds — so
    /// the grid is complete in the first frame and the real row replaces it
    /// in place, with no pop-in and no hole. It just cannot be CLICKED yet:
    /// there is no asset id to fire.
    pub pending: bool,
    /// An EFFECT tile. Its thumbnail walks three phases (prefab art, the
    /// store's placeholder, the baked animated sheet) whose textures may
    /// carry different pixel dims — old stores hold square placeholders —
    /// so the PAINT is what holds the geometry still: an effect tile
    /// always draws full-bleed cover at the tile's own aspect, and a phase
    /// swap can never change the picture's size or framing.
    pub fx: bool,
}

/// A tile that has to keep redrawing: a cycling sheet, or a spinner.
fn entry_animates(entry: &GridEntry) -> bool {
    entry.frames.len() > 1 || entry.loading
}

/// Spinner phase (radians) at `time`, ~0.45 turns a second.
fn busy_spin(time: f64) -> f32 {
    const TAU: f64 = std::f64::consts::TAU;
    (time * 2.8).rem_euclid(TAU) as f32
}

/// `pad` staggers a tile's play position by a HASH of its grid slot: forty
/// one-second sheets on one clock pulse like a metronome, and a diagonal
/// wave is still one readable rhythm pulling the eye — decorrelated random
/// phases make the wall shimmer with no pattern to follow at all.
fn entry_frame(entry: &GridEntry, time: f64, pad: usize) -> Option<Texture> {
    if entry.frames.len() > 1 {
        let hash = (pad as u32).wrapping_mul(2654435761) >> 8;
        let phase = (hash & 0xffff) as f64 / 65536.0;
        let time = time + phase;
        let fps = entry.fps.max(1.0) as f64;
        let i = ((time * fps).floor() as usize) % entry.frames.len();
        return Some(entry.frames[i].clone());
    }
    entry.texture.clone()
}

/// `img_aspect / rect_aspect`, guarding non-finite or non-positive inputs
/// (an unloaded texture, a zero-size rect mid-layout) by falling back to an
/// aspect of 1 for whichever side is degenerate — never NaN/inf out.
fn safe_aspect_ratio(rect_aspect: f32, img_aspect: f32) -> f32 {
    let ra = if rect_aspect.is_finite() && rect_aspect > 0.0 { rect_aspect } else { 1.0 };
    let ia = if img_aspect.is_finite() && img_aspect > 0.0 { img_aspect } else { 1.0 };
    ia / ra
}

/// Scale factors `(w, h)` applied to centred UVs to inscribe an image of
/// aspect `img_aspect` inside a rect of aspect `rect_aspect` — ASPECT-FIT /
/// letterbox: the axis that would overflow the rect shrinks below 1, so the
/// whole image stays visible and the other axis is bordered by transparent
/// bars. The pure-Rust mirror of the `fill_uv` shader function's FIT branch
/// (`fill == 0.0`) on `SpriteTileImage` above; kept for the unit tests
/// below (the runtime crop happens on the GPU, from `self.rect_size`).
#[allow(dead_code)]
fn fit_scale(rect_aspect: f32, img_aspect: f32) -> (f32, f32) {
    let ratio = safe_aspect_ratio(rect_aspect, img_aspect);
    (ratio.min(1.0), (1.0 / ratio).min(1.0))
}

/// Scale factors `(w, h)` for ASPECT-FILL / cover: the same computation as
/// [`fit_scale`] with `max` in place of `min` — the axis that would
/// *underflow* the rect grows past 1 instead of the other axis shrinking
/// below it, so the image fully covers the rect and the grown axis crops
/// its overflow. A wide image in a narrower rect crops left/right (`w`
/// grows); a tall image in a wider rect crops top/bottom (`h` grows).
/// Mirrors `fill_uv`'s FILL branch (`fill == 1.0`).
#[allow(dead_code)]
fn cover_scale(rect_aspect: f32, img_aspect: f32) -> (f32, f32) {
    let ratio = safe_aspect_ratio(rect_aspect, img_aspect);
    (ratio.max(1.0), (1.0 / ratio).max(1.0))
}

/// Aspect ratio (`width / height`) of a bound texture, defaulting to 1.0
/// when unknown (not yet uploaded, zero-sized) — matches `fill_uv`'s own
/// `max(self.img_aspect, 0.0001)` guard, so an unknown aspect never NaNs
/// the crop, just treats the tile as square until a real size lands.
fn thumb_aspect(cx: &mut Cx, frame: Option<&Texture>) -> f32 {
    frame
        .and_then(|tex| tex.get_format(cx).vec_width_height())
        .filter(|(w, h)| *w > 0 && *h > 0)
        .map(|(w, h)| w as f32 / h as f32)
        .unwrap_or(1.0)
}

/// ASPECT-FILL (full-bleed cover) is the default for every tile; only a
/// sprite-sheet / billboard animation (more than one frame swapped over
/// time — see `entry_frame`) stays ASPECT-FIT, because cropping into a
/// packed sheet's cell grid would show a neighbour frame.
/// How far a tile crops toward COVER. 0 is contain (the whole picture with
/// letterbox bars), 1 is full cover (edge to edge, whatever falls outside
/// is gone). Full cover eats too much of a 16:9 clip's sides in a 56px pad,
/// so tiles sit most of the way there: the picture reads large and keeps
/// its middle. One constant — dial it here.
const TILE_CROP: f32 = 0.6;

fn thumb_fill(entry: &GridEntry) -> f32 {
    if entry.fx {
        // Effect tiles are geometry-stable BY LAW (see `GridEntry::fx`):
        // full cover, every phase, whatever texture is bound. The real
        // texture aspect still feeds `img_aspect`, so a square placeholder
        // centre-crops to the tile instead of stretching.
        return 1.0;
    }
    fill_for_thumb(entry.frames.len(), entry.cells)
}

/// How far a tile showing `frames` frames of a `cells`-declared (or not)
/// thumbnail crops toward cover.
///
/// A packed sprite sheet is a GRID of frames: crop it and the cell grid
/// shows a neighbour's arm. Sheets stay contain, always — including the
/// ONE-cell sheet a single-frame actor publishes (`GridEntry::cells`), whose
/// single tile is a whole sprite the producer already letterboxed into a
/// square. Cover-cropping that square in a 164x104 tile would take a quarter
/// of the sprite's height off the top and bottom.
fn fill_for_thumb(frames: usize, cells: bool) -> f32 {
    if frames > 1 || cells {
        0.0
    } else {
        TILE_CROP
    }
}

#[cfg(test)]
mod tile_fit_tests {
    use super::*;

    #[test]
    /// A tile whose thumbnail DECLARED cells shows its cell whole, whether
    /// the declaration held one cell or eight: the producer already fit the
    /// sprite into that square, so cropping into it cuts the sprite. Only a
    /// plain picture (a clip still, a photo) crops toward cover.
    fn declared_cell_tiles_are_contained_even_with_a_single_frame() {
        assert_eq!(fill_for_thumb(0, true), 0.0, "one-cell sprite strip");
        assert_eq!(fill_for_thumb(4, true), 0.0, "cycling sprite strip");
        assert_eq!(fill_for_thumb(4, false), 0.0, "any multi-frame sheet");
        assert_eq!(fill_for_thumb(0, false), TILE_CROP, "a plain still crops");
        assert_eq!(fill_for_thumb(1, false), TILE_CROP, "a still set frame-wise");
        // FIT is what a contained tile then does with the cell: the whole
        // square stays visible, bars on the wide axis of a 164x104 tile.
        let (w, h) = fit_scale(164.0 / 104.0, 1.0);
        assert!(w < 1.0 && h == 1.0, "a square cell letterboxes: {w} {h}");
        // What cropping would have cost: a quarter of the sprite's height.
        let (_, ch) = cover_scale(164.0 / 104.0, 1.0);
        assert!(ch > 1.5, "cover would grow the vertical axis past 1.5: {ch}");
    }

    #[test]
    /// The spinner's phase: a real angle that keeps turning and never
    /// leaves [0, tau), whatever the app clock says.
    fn the_busy_spinner_turns_and_stays_in_range() {
        let tau = std::f32::consts::TAU;
        for t in [0.0, 0.37, 5.0, 1234.5, 86_400.0] {
            let a = busy_spin(t);
            assert!((0.0..tau).contains(&a), "phase out of range at {t}: {a}");
        }
        // Turning, and slow enough to read: well under a turn in a frame.
        let step = busy_spin(1.0 / 60.0) - busy_spin(0.0);
        assert!(step > 0.0 && step < 0.2, "one frame of spin: {step}");
        // A tile that is loading has to keep redrawing; a still one need not.
        let mut entry = GridEntry {
            asset: AssetId::from_bytes([0; 16]),
            title: String::new(),
            sub: String::new(),
            state: String::new(),
            pad: String::new(),
            texture: None,
            frames: Vec::new(),
            fps: 0.0,
            cells: false,
            loading: false,
            failed: false,
            active: false,
            placeholder: false,
            pending: false,
            fx: false,
        };
        assert!(!entry_animates(&entry));
        entry.loading = true;
        assert!(entry_animates(&entry), "a spinner needs frames");
        entry.loading = false;
        entry.failed = true;
        assert!(!entry_animates(&entry), "a failed ring is still");
    }

    #[test]
    fn cover_crops_a_wide_image_left_and_right_in_a_square_tile() {
        // A 2:1 image in a 1:1 tile: the wide axis must grow past 1 (it
        // samples a narrower band of the image, i.e. crops the sides) and
        // the other axis stays exactly 1 (uses the full image height).
        let (w, h) = cover_scale(1.0, 2.0);
        assert!(w > 1.0, "horizontal axis grows past 1 to crop: {w}");
        assert_eq!(h, 1.0);
    }

    #[test]
    fn cover_crops_a_tall_image_top_and_bottom_in_a_square_tile() {
        // A 1:2 (portrait) image in a 1:1 tile: the vertical axis crops.
        let (w, h) = cover_scale(1.0, 0.5);
        assert_eq!(w, 1.0);
        assert!(h > 1.0, "vertical axis grows past 1 to crop: {h}");
    }

    #[test]
    fn fit_letterboxes_a_wide_image_with_bars_top_and_bottom() {
        // Same wide image, same square tile: FIT bars the axis COVER
        // would otherwise have kept full (opposite of the crop case).
        let (w, h) = fit_scale(1.0, 2.0);
        assert_eq!(w, 1.0);
        assert!(h < 1.0, "vertical axis shrinks to letterbox: {h}");
    }

    #[test]
    fn fit_letterboxes_a_tall_image_with_bars_left_and_right() {
        let (w, h) = fit_scale(1.0, 0.5);
        assert!(w < 1.0, "horizontal axis shrinks to letterbox: {w}");
        assert_eq!(h, 1.0);
    }

    #[test]
    fn equal_aspects_are_the_identity_for_both_modes() {
        assert_eq!(cover_scale(16.0 / 9.0, 16.0 / 9.0), (1.0, 1.0));
        assert_eq!(fit_scale(16.0 / 9.0, 16.0 / 9.0), (1.0, 1.0));
        assert_eq!(cover_scale(1.0, 1.0), (1.0, 1.0));
        assert_eq!(fit_scale(1.0, 1.0), (1.0, 1.0));
    }

    #[test]
    fn cover_never_shrinks_and_fit_never_grows() {
        // For any non-degenerate pair, FIT's axes are always <= 1 (it only
        // ever shrinks to letterbox) and COVER's are always >= 1 (it only
        // ever grows to crop) — the `min` vs `max` inversion the task asks
        // for, checked across a spread of aspects.
        for (ra, ia) in [
            (1.0, 2.0),
            (1.0, 0.5),
            (16.0 / 9.0, 4.0 / 3.0),
            (9.0 / 16.0, 3.0),
            (4.0 / 3.0, 4.0 / 3.0),
        ] {
            let (fw, fh) = fit_scale(ra, ia);
            let (cw, ch) = cover_scale(ra, ia);
            assert!(fw <= 1.0 && fh <= 1.0, "fit never exceeds 1: {fw} {fh}");
            assert!(cw >= 1.0 && ch >= 1.0, "cover never goes below 1: {cw} {ch}");
        }
    }

    #[test]
    fn degenerate_inputs_are_guarded_not_nan_or_panicking() {
        for (rect_aspect, img_aspect) in [
            (0.0, 2.0),
            (2.0, 0.0),
            (0.0, 0.0),
            (f32::NAN, 1.0),
            (1.0, f32::NAN),
            (f32::NAN, f32::NAN),
            (-1.0, 2.0),
            (f32::INFINITY, 1.0),
            (1.0, f32::INFINITY),
        ] {
            let (fw, fh) = fit_scale(rect_aspect, img_aspect);
            let (cw, ch) = cover_scale(rect_aspect, img_aspect);
            assert!(
                fw.is_finite() && fh.is_finite() && cw.is_finite() && ch.is_finite(),
                "rect_aspect={rect_aspect} img_aspect={img_aspect} produced non-finite output"
            );
            assert!(fw > 0.0 && fh > 0.0 && cw > 0.0 && ch > 0.0);
        }
    }
}

/// The window of history the bar's wave shows, in seconds. About a bar of
/// house music: long enough to read the groove, short enough that a single
/// kick is still its own spike.
pub const WAVE_WINDOW_SECS: f64 = 2.0;
/// How long a beat LED stays lit. Short enough to read as a flash on the
/// beat, long enough to survive a dropped frame.
const LED_FALL_SECS: f32 = 0.12;

/// The beat clock as the chrome bar draws it.
///
/// Times are in `Cx::seconds_since_app_start`, so both widgets resolve the
/// phase at DRAW time from a fixed reference: there is no per-frame
/// animation state on the host, and a late frame lands where it belongs
/// instead of replaying a missed pulse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatRef {
    /// App-clock time of the next beat.
    pub next_beat_secs: f64,
    pub period_secs: f64,
    /// Bar position of that beat: 0 = the one.
    pub next_index: u32,
    /// Beats per bar of the clock driving this.
    pub bar_beats: u32,
    /// The clock is flying on its own — no confident source behind it. It
    /// keeps the beat; the LED says so by burning lower.
    pub coasting: bool,
}

impl BeatRef {
    /// Seconds since the last beat at `now`, and that beat's bar position.
    pub fn at(&self, now: f64) -> (f64, u32) {
        let bar = self.bar_beats.max(1) as f64;
        if !(self.period_secs > 0.0) || !self.period_secs.is_finite() {
            return (f64::MAX, 1);
        }
        // The nudge matters: a frame that lands one ulp before a beat
        // boundary must read as ON that beat, not a whole beat behind it.
        let beats = (now - self.next_beat_secs) / self.period_secs;
        let n = (beats + 1e-9).floor();
        let since = now - (self.next_beat_secs + n * self.period_secs);
        let index = (self.next_index as f64 + n).rem_euclid(bar) as u32;
        (since.max(0.0), index)
    }

    /// The continuous beat coordinate at `now`, offset so that its floor is
    /// a multiple of the bar length exactly on a downbeat (and so that it
    /// stays positive across the whole visible window).
    pub fn coordinate(&self, now: f64) -> f64 {
        let (since, index) = self.at(now);
        if !(self.period_secs > 0.0) {
            return 0.0;
        }
        4096.0 + index as f64 + (since / self.period_secs).clamp(0.0, 1.0)
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBeatWave {
    #[deref]
    draw_super: DrawQuad,
    /// Envelope-texture width in texels, and the columns actually valid.
    #[live(1.0)]
    pub tex_w: f32,
    #[live(1.0)]
    pub cols: f32,
    /// Columns per second of history.
    #[live(1.0)]
    pub wave_hz: f32,
    /// How old the newest column is, in seconds, at this draw.
    #[live]
    pub right_age: f32,
    /// Seconds of history across the quad.
    #[live(2.0)]
    pub window_secs: f32,
    /// Beat coordinate at the right edge, and the beat period in seconds.
    #[live]
    pub beat_at_right: f32,
    #[live(0.5)]
    pub beat_secs: f32,
    /// 1 while a beat grid is worth ruling, 0 while there is none.
    #[live]
    pub grid_on: f32,
    /// 1 while audio is actually arriving.
    #[live]
    pub live: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBeatLed {
    #[deref]
    draw_super: DrawQuad,
    /// Seconds since the last beat of the driving clock.
    #[live(9.0)]
    pub since: f32,
    /// Seconds for the flash to fall to black.
    #[live(0.12)]
    pub fall: f32,
    /// 1 when the last beat was the one.
    #[live]
    pub accent: f32,
    /// 1 while a clock is actually running.
    #[live]
    pub live: f32,
}

/// The chrome bar's live wave: the captured envelope of the last
/// [`WAVE_WINDOW_SECS`], with the beat grid ruled over it.
///
/// A waveform's job here is to prove the RIGHT AUDIO is coming in; the grid
/// over it proves the clock is on that audio. Both are drawn by one quad
/// from one small texture, so the picture costs a 512-byte upload per pump
/// and nothing at all per frame.
/// A pick from the beats dropdown: beats-per-sweep, 0 = free-running.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum VjBeatsDropAction {
    Picked(u32),
    #[default]
    None,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjBeatsChip {
    #[deref]
    draw_super: DrawQuad,
}

/// The rows, top to bottom: 1 = a sweep per beat (fastest) … 16 =
/// slowest, then free-running.
const BEATS_ROWS: [(u32, &str); 6] =
    [(1, "1"), (2, "2"), (4, "4"), (8, "8"), (16, "16"), (0, "—")];
const BEATS_ROW_H: f64 = 18.0;
const BEATS_PANEL_W: f64 = 42.0;
const BEATS_PANEL_PAD: f64 = 4.0;
const BEATS_PANEL_GAP: f64 = 4.0;

/// BEATS-PER-SWEEP as a real little dropdown (the cycle button grew too
/// many stops): click opens the list, pick closes it. The chip shows the
/// value — or "—" while a scratch hand overrides the transport.
#[derive(Script, Widget)]
pub struct VjBeatsDrop {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawVjBeatsChip,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_panel: DrawQuad,
    #[live]
    draw_hover: DrawColor,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust(1u32)]
    value: u32,
    /// Transient display override (the scratch hand's "—").
    #[rust]
    dash: bool,
    #[rust]
    inert: bool,
    #[rust]
    open: bool,
    #[rust]
    hover_row: Option<usize>,
    #[rust]
    area: Area,
}

impl VjBeatsDrop {
    fn panel_rect(&self, cx: &mut Cx) -> Rect {
        let chip = self.area.rect(cx);
        Rect {
            pos: dvec2(
                chip.pos.x + (chip.size.x - BEATS_PANEL_W) * 0.5,
                chip.pos.y + chip.size.y + BEATS_PANEL_GAP,
            ),
            size: dvec2(
                BEATS_PANEL_W,
                BEATS_ROWS.len() as f64 * BEATS_ROW_H + BEATS_PANEL_PAD * 2.0,
            ),
        }
    }

    fn face(&self) -> &'static str {
        if self.dash {
            return "—";
        }
        BEATS_ROWS
            .iter()
            .find(|(v, _)| *v == self.value)
            .map(|(_, label)| *label)
            .unwrap_or("—")
    }

    pub fn set_value(&mut self, cx: &mut Cx, value: u32) {
        if self.value != value {
            self.value = value;
            self.area.redraw(cx);
        }
    }

    pub fn set_dash(&mut self, cx: &mut Cx, dash: bool) {
        if self.dash != dash {
            self.dash = dash;
            self.area.redraw(cx);
        }
    }

    pub fn set_inert(&mut self, cx: &mut Cx, inert: bool) {
        if self.inert != inert {
            self.inert = inert;
            self.draw_bg.set_uniform(cx, id!(inert), &[if inert { 1.0 } else { 0.0 }]);
            self.area.redraw(cx);
        }
    }

    /// Select the row under `y` (if any), emit, close.
    fn pick_at(&mut self, cx: &mut Cx, uid: WidgetUid, panel: Rect, y: f64) {
        let index = ((y - panel.pos.y - BEATS_PANEL_PAD) / BEATS_ROW_H).floor();
        if index >= 0.0 && (index as usize) < BEATS_ROWS.len() {
            let (value, _) = BEATS_ROWS[index as usize];
            if value != 0 {
                self.value = value;
            }
            cx.widget_action(uid, VjBeatsDropAction::Picked(value));
        }
        self.set_open(cx, false);
    }

    fn set_open(&mut self, cx: &mut Cx, open: bool) {
        if self.open != open {
            self.open = open;
            self.hover_row = None;
            self.draw_bg.set_uniform(cx, id!(open), &[if open { 1.0 } else { 0.0 }]);
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
            self.area.redraw(cx);
        }
    }
}

impl ScriptHook for VjBeatsDrop {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }
}

impl Widget for VjBeatsDrop {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, rect);
        // centre the face text, biased left of the drop arrow
        let face = self.face();
        self.draw_text.draw_abs(
            cx,
            dvec2(
                rect.pos.x + (rect.size.x - 10.0) * 0.5 - 2.0 * face.len() as f64 + 1.0,
                rect.pos.y + 5.0,
            ),
            face,
        );
        cx.end_turtle_with_area(&mut self.area);

        if self.open {
            if let Some(draw_list) = self.draw_list.as_mut() {
                // The proven popup idiom: turtle content at the overlay
                // root, shifted under the chip.
                draw_list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                let h = BEATS_ROWS.len() as f64 * BEATS_ROW_H + BEATS_PANEL_PAD * 2.0;
                self.draw_panel.begin(
                    cx,
                    Walk::fixed(BEATS_PANEL_W, h),
                    Layout::default(),
                );
                let panel = cx.turtle().rect();
                if let Some(row) = self.hover_row {
                    self.draw_hover.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(
                                panel.pos.x + 2.0,
                                panel.pos.y + BEATS_PANEL_PAD + row as f64 * BEATS_ROW_H,
                            ),
                            size: dvec2(BEATS_PANEL_W - 4.0, BEATS_ROW_H),
                        },
                    );
                }
                for (row, (_, label)) in BEATS_ROWS.iter().enumerate() {
                    self.draw_text.draw_abs(
                        cx,
                        dvec2(
                            panel.pos.x + 14.0,
                            panel.pos.y + BEATS_PANEL_PAD + row as f64 * BEATS_ROW_H + 3.0,
                        ),
                        label,
                    );
                }
                self.draw_panel.end(cx);
                let chip = self.area.rect(cx);
                cx.end_pass_sized_turtle_with_shift(
                    self.area,
                    dvec2((chip.size.x - BEATS_PANEL_W) * 0.5, chip.size.y + BEATS_PANEL_GAP),
                );
                draw_list.end(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.open {
            let panel = self.panel_rect(cx);
            match event {
                Event::MouseMove(me) => {
                    let row = if panel.contains(me.abs) {
                        let index = ((me.abs.y - panel.pos.y - BEATS_PANEL_PAD)
                            / BEATS_ROW_H)
                            .floor();
                        (index >= 0.0 && (index as usize) < BEATS_ROWS.len())
                            .then(|| index as usize)
                    } else {
                        None
                    };
                    if row != self.hover_row {
                        self.hover_row = row;
                        if let Some(draw_list) = &self.draw_list {
                            draw_list.redraw(cx);
                        }
                    }
                }
                Event::MouseDown(me) => {
                    if panel.contains(me.abs) {
                        self.pick_at(cx, uid, panel, me.abs.y);
                    } else {
                        let chip = self.area.rect(cx);
                        if !chip.contains(me.abs) {
                            self.set_open(cx, false);
                        }
                    }
                }
                Event::MouseUp(me) => {
                    // THE MENU GESTURE (macOS/DAW standard): press on the
                    // chip, DRAG onto an item, release = select + close —
                    // one fluid motion. A release back on the chip keeps
                    // the list open (the click-then-click mode); a release
                    // in dead space dismisses.
                    if panel.contains(me.abs) {
                        self.pick_at(cx, uid, panel, me.abs.y);
                    } else {
                        let chip = self.area.rect(cx);
                        if !chip.contains(me.abs) {
                            self.set_open(cx, false);
                        }
                    }
                }
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                    self.set_open(cx, false);
                }
                _ => {}
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[1.0]);
                self.area.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.set_uniform(cx, id!(hover), &[0.0]);
                self.area.redraw(cx);
            }
            Hit::FingerDown(_) if !self.inert => {
                self.set_open(cx, !self.open);
            }
            _ => {}
        }
    }
}

/// What the operator's hand is doing to the shuttle. `Scratch(0.0)` is
/// the release settling home — the host restores the beat transport on it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum VjShuttleAction {
    Scratch(f32),
    #[default]
    None,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjShuttle {
    #[deref]
    draw_super: DrawQuad,
}

/// SCRATCH / SHUTTLE: centre = neutral, drag right = forward (faster with
/// distance), drag left = reverse, and the knob SPRINGS home on release —
/// a performance jog, never a latched rate.
#[derive(Script, ScriptHook, Widget)]
pub struct VjShuttle {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawVjShuttle,
    #[rust]
    area: Area,
    #[rust]
    pos: f32,
    #[rust]
    dragging: bool,
    #[rust]
    springing: bool,
    #[rust]
    next_frame: NextFrame,
}

impl VjShuttle {
    fn set_pos(&mut self, cx: &mut Cx, uid: WidgetUid, pos: f32) {
        let pos = pos.clamp(-1.0, 1.0);
        if (pos - self.pos).abs() > f32::EPSILON {
            self.pos = pos;
            cx.widget_action(uid, VjShuttleAction::Scratch(pos));
            self.area.redraw(cx);
        }
    }

    fn pos_at(&self, cx: &mut Cx, x: f64) -> f32 {
        let rect = self.area.rect(cx);
        if rect.size.x <= 14.0 {
            return 0.0;
        }
        let half = rect.size.x * 0.5 - 7.0;
        (((x - rect.pos.x) - rect.size.x * 0.5) / half) as f32
    }
}

impl Widget for VjShuttle {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.set_uniform(cx, id!(shuttle), &[self.pos]);
        self.draw_bg.set_uniform(
            cx,
            id!(active),
            &[if self.dragging || self.pos.abs() > 0.01 { 1.0 } else { 0.0 }],
        );
        self.draw_bg.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        // The spring: ease home after release, one action per step so the
        // host rides the whole return (beat transport re-engages at 0).
        // NEVER while a finger holds the knob — a stale NextFrame from a
        // previous spring cycle once fired a phantom mid-hold release.
        if self.next_frame.is_event(event).is_some() && self.springing && !self.dragging {
            let pos = self.pos * 0.55;
            let pos = if pos.abs() < 0.02 { 0.0 } else { pos };
            self.set_pos(cx, uid, pos);
            if pos == 0.0 {
                self.springing = false;
            } else {
                self.next_frame = cx.new_next_frame();
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                self.dragging = true;
                self.springing = false;
                let pos = self.pos_at(cx, fe.abs.x);
                self.set_pos(cx, uid, pos);
            }
            Hit::FingerMove(fe) if self.dragging => {
                // A held finger owns the knob outright.
                self.springing = false;
                let pos = self.pos_at(cx, fe.abs.x);
                self.set_pos(cx, uid, pos);
            }
            Hit::FingerUp(fe) if self.dragging && fe.is_primary_hit() => {
                self.dragging = false;
                self.springing = true;
                self.next_frame = cx.new_next_frame();
            }
            _ => {}
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjBeatWave {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_wave: DrawBeatWave,
    #[rust]
    area: Area,
    #[rust]
    texture: Option<Texture>,
    /// Scratch for the texel upload, kept so a pump allocates nothing.
    #[rust]
    texels: Vec<u32>,
    #[rust]
    cols: usize,
    /// App-clock time the newest column was captured.
    #[rust]
    stamp_secs: f64,
    #[rust]
    beat: Option<BeatRef>,
    #[rust]
    live: bool,
    #[rust]
    anim_frame: NextFrame,
}

impl VjBeatWave {
    /// Replace the whole envelope history. `cols` runs oldest to newest, one
    /// entry per `1.0 / hz` seconds, packed `peak << 8 | rms`. `stamp_secs`
    /// is the app-clock time of the newest column.
    pub fn set_wave(&mut self, cx: &mut Cx, cols: &[u16], hz: f64, stamp_secs: f64, live: bool) {
        self.stamp_secs = stamp_secs;
        self.live = live;
        self.draw_wave.wave_hz = hz as f32;
        self.texels.clear();
        self.texels.extend(cols.iter().map(|packed| {
            let peak = (packed >> 8) as u32;
            let rms = (packed & 0xff) as u32;
            // BGRA-32: peak in R, RMS in G, opaque.
            0xff00_0000 | (peak << 16) | (rms << 8)
        }));
        if self.texels.is_empty() {
            self.texels.push(0xff00_0000);
        }
        let width = self.texels.len();
        match self.texture.as_ref().filter(|_| width == self.cols) {
            // Same shape as last time: swap the scratch buffer in and keep
            // the old one, so a pump allocates nothing at all.
            Some(texture) => texture.swap_vec_u32(cx, &mut self.texels),
            None => {
                self.texture = Some(Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width,
                        height: 1,
                        data: Some(std::mem::take(&mut self.texels)),
                        updated: TextureUpdated::Full,
                    },
                ));
            }
        }
        self.cols = width;
        self.area.redraw(cx);
    }

    /// The grid to rule over the wave; `None` while there is no clock.
    pub fn set_beat(&mut self, cx: &mut Cx, beat: Option<BeatRef>) {
        if self.beat != beat {
            self.beat = beat;
            self.area.redraw(cx);
        }
    }
}

impl Widget for VjBeatWave {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.anim_frame.is_event(event).is_some() {
            self.area.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 2.0 || rect.size.y < 2.0 {
            return DrawStep::done();
        }
        let now = cx.seconds_since_app_start();
        match self.texture.as_ref() {
            Some(texture) => {
                self.draw_wave.draw_vars.set_texture(0, texture);
                self.draw_wave.tex_w = self.cols.max(1) as f32;
                self.draw_wave.cols = self.cols.max(1) as f32;
            }
            None => {
                self.draw_wave.draw_vars.empty_texture(0);
                self.draw_wave.tex_w = 1.0;
                self.draw_wave.cols = 1.0;
            }
        }
        self.draw_wave.window_secs = WAVE_WINDOW_SECS as f32;
        // The wave and the grid share ONE time axis: both are placed by the
        // age of the newest column, so a ruling that lands on a transient
        // means the clock really is on that transient.
        self.draw_wave.right_age = (now - self.stamp_secs).clamp(0.0, WAVE_WINDOW_SECS) as f32;
        self.draw_wave.live = if self.live { 1.0 } else { 0.0 };
        match self.beat {
            Some(beat) if beat.period_secs > 0.0 => {
                self.draw_wave.beat_at_right = beat.coordinate(now - self.draw_wave.right_age as f64)
                    as f32;
                self.draw_wave.beat_secs = beat.period_secs as f32;
                // Same signal on the rulings: a coasted grid is drawn, and
                // drawn fainter, because nothing is confirming it.
                self.draw_wave.grid_on = if beat.coasting { 0.5 } else { 1.0 };
                // The grid scrolls with real time, so this animates itself.
                self.anim_frame = cx.new_next_frame();
            }
            _ => {
                self.draw_wave.grid_on = 0.0;
                self.draw_wave.beat_secs = 0.5;
            }
        }
        self.draw_wave.draw_abs(cx, rect);
        DrawStep::done()
    }
}

/// The lock LED: a dot that flashes on every beat of the clock actually
/// driving the room, accented on the one.
///
/// It carries no animation state: the host hands it a fixed beat reference
/// and the shader works out the brightness from where in the beat the frame
/// falls, so nothing has to be pumped to keep it honest.
#[derive(Script, ScriptHook, Widget)]
pub struct VjBeatLed {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_led: DrawBeatLed,
    #[rust]
    area: Area,
    #[rust]
    beat: Option<BeatRef>,
    #[rust]
    anim_frame: NextFrame,
}

impl VjBeatLed {
    pub fn set_beat(&mut self, cx: &mut Cx, beat: Option<BeatRef>) {
        if self.beat != beat {
            self.beat = beat;
            self.area.redraw(cx);
        }
    }
}

impl Widget for VjBeatLed {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.anim_frame.is_event(event).is_some() {
            self.area.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 2.0 || rect.size.y < 2.0 {
            return DrawStep::done();
        }
        self.draw_led.fall = LED_FALL_SECS;
        match self.beat {
            Some(beat) if beat.period_secs > 0.0 => {
                let (since, index) = beat.at(cx.seconds_since_app_start());
                self.draw_led.since = since as f32;
                self.draw_led.accent = if index == 0 { 1.0 } else { 0.0 };
                // A coasting clock still flashes — that is the whole point
                // of the flywheel — but it burns lower, so the operator can
                // see at a glance that nothing is confirming it.
                self.draw_led.live = if beat.coasting { 0.45 } else { 1.0 };
                // A pulse is only a pulse if it is redrawn; a dark LED is
                // not, so an idle app stays idle.
                self.anim_frame = cx.new_next_frame();
            }
            _ => {
                self.draw_led.since = 9.0;
                self.draw_led.accent = 0.0;
                self.draw_led.live = 0.0;
            }
        }
        self.draw_led.draw_abs(cx, rect);
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjTileGrid {
    #[deref]
    view: View,
    #[rust]
    entries: Vec<GridEntry>,
    #[rust(1usize)]
    pub last_cols: usize,
    #[rust]
    anim_frame: NextFrame,
}

impl VjTileGrid {
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<GridEntry>) {
        self.entries = entries;
        self.view.redraw(cx);
    }

    /// Mark which tile is loading and which one failed, from the cue engine
    /// (or the pad loader) — pushed every pump rather than waiting for the
    /// next grid rebuild, because a click has to answer NOW and a rebuild
    /// only happens when the catalog changes.
    pub fn set_busy(&mut self, cx: &mut Cx, loading: Option<AssetId>, failed: Option<AssetId>) {
        let mut changed = false;
        for entry in &mut self.entries {
            let is_loading = loading == Some(entry.asset);
            let has_failed = failed == Some(entry.asset);
            if entry.loading != is_loading || entry.failed != has_failed {
                entry.loading = is_loading;
                entry.failed = has_failed;
                changed = true;
            }
        }
        if changed {
            self.view.redraw(cx);
        }
    }

    pub fn set_thumb(&mut self, cx: &mut Cx, asset: AssetId, texture: Texture) {
        self.set_thumb_anim(cx, asset, vec![texture], 0.0);
    }

    pub fn set_thumb_anim(&mut self, cx: &mut Cx, asset: AssetId, frames: Vec<Texture>, fps: f32) {
        let mut hit = false;
        for entry in &mut self.entries {
            if entry.asset == asset {
                entry.texture = frames.first().cloned();
                entry.frames = frames.clone();
                entry.fps = fps;
                hit = true;
            }
        }
        if hit {
            self.view.redraw(cx);
        }
    }

    pub fn has_anims(&self) -> bool {
        self.entries.iter().any(|e| e.frames.len() > 1)
    }

    pub fn entry_at(&self, index: usize) -> Option<&GridEntry> {
        self.entries.get(index)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Widget for VjTileGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.anim_frame.is_event(event).is_some()
            && self.entries.iter().any(entry_animates)
        {
            self.view.redraw(cx);
            self.anim_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.entries.iter().any(entry_animates) {
            self.anim_frame = cx.new_next_frame();
        }
        let width = self.view.area().rect(cx).size.x;
        if width > CARD_W {
            self.last_cols = (((width + CARD_SPACING) / (CARD_W + CARD_SPACING)) as usize)
                .clamp(1, GRID_SLOTS);
        }
        let cols = self.last_cols.max(1);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else { continue };
            if self.entries.is_empty() {
                list.set_item_range(cx, 0, 1);
                while let Some(row_id) = list.next_visible_item(cx) {
                    if row_id >= 1 {
                        continue;
                    }
                    let item = list.item(cx, row_id, id!(Empty));
                    item.draw_all(cx, &mut Scope::empty());
                }
                continue;
            }
            let rows = self.entries.len().div_ceil(cols);
            list.set_item_range(cx, 0, rows);
            let slots = [
                ids!(c1),
                ids!(c2),
                ids!(c3),
                ids!(c4),
                ids!(c5),
                ids!(c6),
                ids!(c7),
                ids!(c8),
            ];
            while let Some(row_id) = list.next_visible_item(cx) {
                // PortalList deliberately probes past a short item range to
                // fill the viewport. Instantiating one of those synthetic
                // rows gives it only hidden, zero-height cells, so it asks
                // for another forever and grows the widget tree without
                // bound. Only real catalog rows may create widgets.
                if row_id >= rows {
                    continue;
                }
                let item = list.item(cx, row_id, id!(Row));
                for (slot, path) in slots.iter().enumerate() {
                    let index = row_id * cols + slot;
                    let visible = slot < cols && index < self.entries.len();
                    item.view(cx, *path).set_visible(cx, visible);
                    if !visible {
                        continue;
                    }
                    let entry = &self.entries[index];
                    let mut cell = item.view(cx, *path);
                    cell.label(cx, ids!(grid_title)).set_text(cx, &entry.title);
                    cell.label(cx, ids!(grid_sub)).set_text(cx, &entry.sub);
                    cell.label(cx, ids!(grid_pad)).set_text(cx, &entry.pad);
                    let state = if entry.active {
                        format!("LIVE {}", entry.state)
                    } else {
                        entry.state.clone()
                    };
                    cell.label(cx, ids!(grid_state)).set_text(cx, &state);
                    // Recycled rows lose their texture: rebind every pass.
                    let now = cx.seconds_since_app_start();
                    let frame = entry_frame(entry, now, index);
                    let aspect = thumb_aspect(cx, frame.as_ref());
                    let fill = thumb_fill(entry);
                    let mut thumb = cell.image(cx, ids!(grid_thumb));
                    thumb.set_texture(cx, frame);
                    script_apply_eval!(cx, thumb, {
                        draw_bg +: { fill: #(fill) img_aspect: #(aspect) }
                    });
                    // The click's own feedback: spinner while the cue loads,
                    // a still red ring if it failed — and an effect tile
                    // still on placeholder art spins too: bake in progress.
                    let baking = entry.state == "FX" && entry.frames.len() <= 1;
                    let mut busy = cell.view(cx, ids!(grid_busy));
                    busy.set_visible(cx, entry.loading || entry.failed || baking);
                    if entry.loading || entry.failed || baking {
                        let spin = busy_spin(now);
                        let failed = f32::from(u8::from(entry.failed));
                        script_apply_eval!(cx, busy, {
                            draw_bg +: { spin: #(spin) failed: #(failed) }
                        });
                    }
                    let selected = f32::from(u8::from(entry.active));
                    script_apply_eval!(cx, cell, {
                        draw_bg +: { selected: #(selected) }
                    });
                }
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// APC40 8×5 clip matrix
// ---------------------------------------------------------------------------

pub const PAD_ROWS: usize = 5;
pub const PAD_COLS: usize = 8;
pub const PAD_MATRIX: usize = PAD_ROWS * PAD_COLS;

/// The grid is one giant horizontal strip: entries fill a column top to
/// bottom, then the next column. An item keeps its row for good — the
/// window scrolls by whole COLUMNS (wheel / ◀ ▶ = one column, |◀ ▶| = a
/// page of PAD_COLS), every row moving together, never a per-row "snake".
/// `bank` is the first visible column.
pub fn pad_cols(len: usize) -> usize {
    len.div_ceil(PAD_ROWS)
}

/// Clamp a first-visible-column so the window never runs past the content.
pub fn clamp_pad_offset(first_col: usize, len: usize) -> usize {
    let cols = pad_cols(len);
    if cols <= PAD_COLS {
        return 0;
    }
    first_col.min(cols - PAD_COLS)
}

/// The entry-count basis every scroll computation (clamping, thumb size,
/// thumb position) should measure against: the catalog's reported TOTAL
/// once known, so the scrollbar represents the whole result set — not how
/// much of it has streamed in via `load_more()` yet. Before a total is
/// known (`total == 0`, e.g. a host that never wires one in) this falls
/// back to what is actually loaded, matching the old behavior.
pub fn scroll_basis(total: usize, loaded: usize) -> usize {
    if total > 0 {
        total
    } else {
        loaded
    }
}

/// Smallest thumb the operator can still grab, in pixels.
pub const SCROLL_THUMB_MIN: f64 = 24.0;

/// Scrollbar thumb geometry: `(offset_from_track_left, width)`.
///
/// Length is the visible fraction of the content (so a 777-column strip
/// gets a short thumb and a 9-column one a long thing that nearly fills
/// the track), floored at [`SCROLL_THUMB_MIN`] so it stays grabbable, and
/// the position maps the CURRENT first column onto the remaining travel.
pub fn scroll_thumb_geom(track_w: f64, len: usize, bank: usize) -> (f64, f64) {
    if track_w <= 0.0 {
        return (0.0, 0.0);
    }
    let total = pad_cols(len).max(1) as f64;
    let visible = (PAD_COLS as f64).min(total);
    let width = (track_w * (visible / total)).clamp(SCROLL_THUMB_MIN.min(track_w), track_w);
    let travel = (track_w - width).max(0.0);
    let max_off = clamp_pad_offset(usize::MAX, len);
    let t = if max_off == 0 {
        0.0
    } else {
        (bank.min(max_off) as f64) / max_off as f64
    };
    (travel * t, width)
}

/// Which column the thumb's LEFT edge at `thumb_x` corresponds to — the
/// inverse of [`scroll_thumb_geom`], for dragging.
pub fn scroll_offset_for_thumb(track_w: f64, len: usize, thumb_x: f64) -> usize {
    let (_, width) = scroll_thumb_geom(track_w, len, 0);
    let travel = (track_w - width).max(0.0);
    let max_off = clamp_pad_offset(usize::MAX, len);
    if travel <= 0.0 || max_off == 0 {
        return 0;
    }
    let t = (thumb_x / travel).clamp(0.0, 1.0);
    (t * max_off as f64).round() as usize
}

/// A press on the bare track (not the thumb) pages one visible-page of
/// columns toward wherever the operator clicked: `-1` (page left/back) if
/// the click landed before the thumb's left edge, `1` (page right/forward)
/// otherwise, including a click past the thumb's right edge.
pub fn scroll_page_dir(local_x: f64, thumb_x: f64) -> i32 {
    if local_x < thumb_x {
        -1
    } else {
        1
    }
}

/// Physical pad (row-major on the APC40 / screen: `row * PAD_COLS + slot`)
/// → entry index, given the first visible column.
pub fn pad_entry_index(first_col: usize, pad: usize) -> usize {
    let row = pad / PAD_COLS;
    let slot = pad % PAD_COLS;
    (first_col + slot) * PAD_ROWS + row
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjPadMatrix {
    #[deref]
    view: View,
    #[live]
    draw_track: DrawQuad,
    #[live]
    draw_thumb: DrawQuad,
    #[rust]
    entries: Vec<GridEntry>,
    /// The catalog's reported result count across ALL pages (not just the
    /// ones that have streamed in). Once known, this — not `entries.len()`
    /// — is what the scrollbar sizes and clamps against, so the thumb does
    /// not shrink as `load_more()` pages arrive. Zero means "unknown yet"
    /// (no catalog wired in, e.g. plain unit-test construction): scrolling
    /// then falls back to the loaded count, same as before.
    #[rust]
    total: usize,
    #[rust]
    pub bank: usize,
    /// Last (selected, empty) pushed into each pad's shader: a script
    /// evaluation per cell per draw was the app's biggest per-frame cost.
    #[rust]
    cell_state: Vec<(f32, f32)>,
    /// Last (fill, img_aspect) pushed into each pad's thumbnail shader —
    /// same rationale as `cell_state`: skip the script evaluation whenever
    /// the bound texture's mode/aspect hasn't actually changed.
    #[rust]
    thumb_state: Vec<(f32, f32)>,
    #[rust]
    scroll_area: Area,
    /// Grab offset INSIDE the thumb while dragging (so the thumb does not
    /// jump to the cursor), or `None` when not dragging.
    #[rust]
    dragging: Option<f64>,
    #[rust]
    thumb_hover: bool,
    /// Trackpad deltas are fractional: accumulate to whole-column notches.
    #[rust]
    wheel_accum: f64,
    #[rust]
    anim_frame: NextFrame,
}

/// Wheel travel (px) per column step; a mouse notch is ~10–20 px, a
/// trackpad swipe many small deltas.
const WHEEL_NOTCH: f64 = 24.0;

impl VjPadMatrix {
    /// Entry-count basis for scroll clamping/geometry — see
    /// [`scroll_basis`]: the catalog TOTAL once known, else the loaded
    /// count.
    fn scroll_len(&self) -> usize {
        scroll_basis(self.total, self.entries.len())
    }

    /// The catalog's reported result total (across all pages). Call this
    /// whenever the host learns/updates it (e.g. from the first search
    /// page) — it does NOT need to be called again as more pages stream
    /// in via `load_more()`; the scrollbar stays put until the total
    /// itself changes (a new search, a refresh).
    pub fn set_total(&mut self, cx: &mut Cx, total: usize) {
        if self.total != total {
            self.total = total;
            self.bank = clamp_pad_offset(self.bank, self.scroll_len());
            self.view.redraw(cx);
        }
    }

    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<GridEntry>) {
        self.entries = entries;
        self.bank = clamp_pad_offset(self.bank, self.scroll_len());
        self.view.redraw(cx);
    }

    /// Mark which tile is loading and which one failed, from the cue engine
    /// (or the pad loader) — pushed every pump rather than waiting for the
    /// next grid rebuild, because a click has to answer NOW and a rebuild
    /// only happens when the catalog changes.
    pub fn set_busy(&mut self, cx: &mut Cx, loading: Option<AssetId>, failed: Option<AssetId>) {
        let mut changed = false;
        for entry in &mut self.entries {
            let is_loading = loading == Some(entry.asset);
            let has_failed = failed == Some(entry.asset);
            if entry.loading != is_loading || entry.failed != has_failed {
                entry.loading = is_loading;
                entry.failed = has_failed;
                changed = true;
            }
        }
        if changed {
            self.view.redraw(cx);
        }
    }

    pub fn set_thumb(&mut self, cx: &mut Cx, asset: AssetId, texture: Texture) {
        self.set_thumb_anim(cx, asset, vec![texture], 0.0);
    }

    pub fn set_thumb_anim(&mut self, cx: &mut Cx, asset: AssetId, frames: Vec<Texture>, fps: f32) {
        let mut hit = false;
        for entry in &mut self.entries {
            if entry.asset == asset {
                entry.texture = frames.first().cloned();
                entry.frames = frames.clone();
                entry.fps = fps;
                hit = true;
            }
        }
        if hit {
            self.view.redraw(cx);
        }
    }

    pub fn visible_assets(&self) -> Vec<AssetId> {
        (0..40)
            .filter_map(|pad| self.visible_at(pad).map(|entry| entry.asset))
            .collect()
    }

    pub fn set_bank(&mut self, bank: usize) {
        self.bank = clamp_pad_offset(bank, self.scroll_len());
    }

    pub fn set_offset(&mut self, cx: &mut Cx, offset: usize) {
        let next = clamp_pad_offset(offset, self.scroll_len());
        if next != self.bank {
            self.bank = next;
            self.view.redraw(cx);
        }
    }

    /// Scroll the whole strip by `cols` columns (a page = PAD_COLS).
    pub fn nudge_cols(&mut self, cx: &mut Cx, cols: i32) {
        let next = (self.bank as i32 + cols).max(0) as usize;
        self.set_offset(cx, next);
    }

    pub fn entry_at(&self, index: usize) -> Option<&GridEntry> {
        self.entries.get(index)
    }

    /// The entry a pad ACTS on: a prefab cell is drawn but not yet clickable
    /// (see [`GridEntry::pending`]), so it is not one.
    pub fn visible_at(&self, pad: usize) -> Option<&GridEntry> {
        self.paint_at(pad).filter(|entry| !entry.pending)
    }

    /// The entry a pad PAINTS — prefabs included.
    pub fn paint_at(&self, pad: usize) -> Option<&GridEntry> {
        self.entry_at(pad_entry_index(self.bank, pad))
            .filter(|entry| !entry.placeholder)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The window shows the tail of what is loaded: the host should page
    /// the catalog (server-side) rather than let the bank run dry.
    pub fn at_tail(&self) -> bool {
        (self.bank + PAD_COLS) * PAD_ROWS >= self.entries.len()
    }

    fn bind_pads(&mut self, cx: &mut Cx) {
        let rows = [ids!(r0), ids!(r1), ids!(r2), ids!(r3), ids!(r4)];
        let slots = [
            ids!(c1),
            ids!(c2),
            ids!(c3),
            ids!(c4),
            ids!(c5),
            ids!(c6),
            ids!(c7),
            ids!(c8),
        ];
        for (row, row_id) in rows.iter().enumerate() {
            let row_view = self.view.view(cx, *row_id);
            for (slot, path) in slots.iter().enumerate() {
                let pad = row * PAD_COLS + slot;
                let mut cell = row_view.view(cx, *path);
                let (selected, empty, fill, aspect) = if let Some(entry) = self.paint_at(pad) {
                    cell.label(cx, ids!(grid_pad)).set_text(cx, &format!("{:02}", pad + 1));
                    cell.label(cx, ids!(grid_title)).set_text(cx, &entry.title);
                    let now = cx.seconds_since_app_start();
                    let frame = entry_frame(entry, now, pad);
                    let aspect = thumb_aspect(cx, frame.as_ref());
                    let fill = thumb_fill(entry);
                    // No thumbnail yet = no image at all (an empty Image
                    // paints a black square).
                    let image = cell.image(cx, ids!(grid_thumb));
                    image.set_visible(cx, frame.is_some());
                    image.set_texture(cx, frame);
                    // The grid's ONLY state paint: a green ring on the tile
                    // last clicked. LIVE / CUE / HOLD live in the program
                    // strip's labels, not on forty tiles at once.
                    let selected = f32::from(u8::from(entry.active));
                    // The click's own feedback: spinner while this pad's cue
                    // loads, a still red ring if it failed. An effect tile
                    // still on its placeholder art gets the same spinner —
                    // the bake is in progress, and a tile that visibly moves
                    // says so (a static placeholder reads as done-and-boring).
                    let baking = entry.state == "FX" && entry.frames.len() <= 1;
                    let mut busy = cell.view(cx, ids!(grid_busy));
                    busy.set_visible(cx, entry.loading || entry.failed || baking);
                    if entry.loading || entry.failed || baking {
                        let spin = busy_spin(now);
                        let failed = f32::from(u8::from(entry.failed));
                        script_apply_eval!(cx, busy, {
                            draw_bg +: { spin: #(spin) failed: #(failed) }
                        });
                    }
                    (selected, 0.0, fill, aspect)
                } else {
                    // No content: a quiet, greyed placeholder, not a black pad.
                    cell.label(cx, ids!(grid_pad)).set_text(cx, "");
                    cell.label(cx, ids!(grid_title)).set_text(cx, "");
                    let image = cell.image(cx, ids!(grid_thumb));
                    image.set_visible(cx, false);
                    image.set_texture(cx, None);
                    cell.view(cx, ids!(grid_busy)).set_visible(cx, false);
                    (0.0, 1.0, 1.0, 1.0)
                };
                if self.cell_state.len() <= pad {
                    self.cell_state.resize(pad + 1, (-1.0, -1.0));
                }
                if self.cell_state[pad] != (selected, empty) {
                    self.cell_state[pad] = (selected, empty);
                    script_apply_eval!(cx, cell, {
                        draw_bg +: { selected: #(selected) empty: #(empty) }
                    });
                }
                if self.thumb_state.len() <= pad {
                    self.thumb_state.resize(pad + 1, (-1.0, -1.0));
                }
                if self.thumb_state[pad] != (fill, aspect) {
                    self.thumb_state[pad] = (fill, aspect);
                    let mut thumb = cell.image(cx, ids!(grid_thumb));
                    script_apply_eval!(cx, thumb, {
                        draw_bg +: { fill: #(fill) img_aspect: #(aspect) }
                    });
                }
            }
        }
    }
}

impl Widget for VjPadMatrix {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.anim_frame.is_event(event).is_some()
            && self.entries.iter().any(entry_animates)
        {
            self.view.redraw(cx);
            self.anim_frame = cx.new_next_frame();
        }
        if let Event::Scroll(scroll) = event {
            // Wheel over the strip = one column per notch, like ◀ ▶. A
            // horizontal trackpad swipe scrolls too; either axis claims
            // the event so the pane behind does not scroll as well.
            if self.view.area().rect(cx).contains(scroll.abs) {
                let delta = if scroll.scroll.x.abs() > scroll.scroll.y.abs() {
                    scroll.scroll.x
                } else {
                    scroll.scroll.y
                };
                self.wheel_accum += delta;
                let mut step = 0;
                while self.wheel_accum >= WHEEL_NOTCH {
                    self.wheel_accum -= WHEEL_NOTCH;
                    step += 1;
                }
                while self.wheel_accum <= -WHEEL_NOTCH {
                    self.wheel_accum += WHEEL_NOTCH;
                    step -= 1;
                }
                if step != 0 {
                    self.nudge_cols(cx, step);
                }
                scroll.handled_x.set(true);
                scroll.handled_y.set(true);
            }
        }
        match event.hits(cx, self.scroll_area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let track = self.scroll_area.rect(cx);
                let (thumb_x, thumb_w) =
                    scroll_thumb_geom(track.size.x, self.scroll_len(), self.bank);
                let local = fe.abs.x - track.pos.x;
                if local >= thumb_x && local <= thumb_x + thumb_w {
                    // Grabbed the thumb: keep the grab point under the
                    // cursor instead of snapping the thumb to it.
                    self.dragging = Some(local - thumb_x);
                } else {
                    // Clicked the bare track: page toward the click.
                    let dir = scroll_page_dir(local, thumb_x);
                    self.nudge_cols(cx, dir * PAD_COLS as i32);
                }
                self.view.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                if let Some(grab) = self.dragging {
                    let track = self.scroll_area.rect(cx);
                    let want = fe.abs.x - track.pos.x - grab;
                    let offset =
                        scroll_offset_for_thumb(track.size.x, self.scroll_len(), want);
                    self.set_offset(cx, offset);
                }
            }
            Hit::FingerUp(_) => {
                self.dragging = None;
                self.view.redraw(cx);
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let track = self.scroll_area.rect(cx);
                let (thumb_x, thumb_w) =
                    scroll_thumb_geom(track.size.x, self.scroll_len(), self.bank);
                let local = fe.abs.x - track.pos.x;
                let over = local >= thumb_x && local <= thumb_x + thumb_w;
                if over != self.thumb_hover {
                    self.thumb_hover = over;
                    self.view.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOut(_) => {
                if self.thumb_hover {
                    self.thumb_hover = false;
                    self.view.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.entries.iter().any(entry_animates) {
            self.anim_frame = cx.new_next_frame();
        }
        self.bind_pads(cx);
        let step = self.view.draw_walk(cx, scope, walk);
        let slot = self.view.view(cx, ids!(scroll_slot)).area().rect(cx);
        if slot.size.x > 1.0 {
            self.draw_track.draw_abs(cx, slot);
            // The TRACK's own quad is the hit surface, and it covers the
            // whole slot — so a press anywhere on the bar is ours.
            self.scroll_area = self.draw_track.area();
            let (thumb_x, thumb_w) = scroll_thumb_geom(slot.size.x, self.scroll_len(), self.bank);
            self.draw_thumb.set_uniform(cx, id!(hover), &[f32::from(u8::from(self.thumb_hover))]);
            self.draw_thumb.set_uniform(
                cx,
                id!(down),
                &[f32::from(u8::from(self.dragging.is_some()))],
            );
            self.draw_thumb.draw_abs(
                cx,
                Rect {
                    pos: dvec2(slot.pos.x + thumb_x, slot.pos.y),
                    size: dvec2(thumb_w, slot.size.y),
                },
            );
        }
        step
    }
}

#[cfg(test)]
mod pad_matrix_tests {
    use super::*;

    #[test]
    fn scrollbar_thumb_shows_how_much_content_there_is() {
        let track = 400.0;
        // Everything fits: a full-length thumb parked at the left.
        let (x, w) = scroll_thumb_geom(track, 40, 0);
        assert_eq!((x, w), (0.0, track), "8 columns of 8 fill the track");
        // Half the strip visible → half the track, at the left.
        let (x, w) = scroll_thumb_geom(track, PAD_ROWS * 16, 0);
        assert!((w - 200.0).abs() < 1e-6, "8 of 16 columns → half a track: {w}");
        assert_eq!(x, 0.0);
        // …and at the far right end when scrolled to the tail.
        let last = clamp_pad_offset(usize::MAX, PAD_ROWS * 16);
        assert_eq!(last, 8);
        let (x, w) = scroll_thumb_geom(track, PAD_ROWS * 16, last);
        assert!((x + w - track).abs() < 1e-6, "the tail parks the thumb at the end");
        // Past the end clamps rather than running off the track.
        let (x_over, _) = scroll_thumb_geom(track, PAD_ROWS * 16, 9999);
        assert_eq!(x_over, x);
        // A huge strip still leaves something grabbable.
        let (_, w) = scroll_thumb_geom(track, PAD_ROWS * 4000, 0);
        assert_eq!(w, SCROLL_THUMB_MIN, "tiny fraction floors at the min size");
        // A track narrower than the minimum thumb never overflows.
        let (_, w) = scroll_thumb_geom(12.0, PAD_ROWS * 4000, 0);
        assert!(w <= 12.0, "thumb fits its track: {w}");
        assert_eq!(scroll_thumb_geom(0.0, 100, 0), (0.0, 0.0));
    }

    #[test]
    fn dragging_the_thumb_maps_back_to_columns() {
        let track = 400.0;
        let len = PAD_ROWS * 16; // 16 columns, 8 visible, 8 of travel
        assert_eq!(scroll_offset_for_thumb(track, len, 0.0), 0);
        assert_eq!(scroll_offset_for_thumb(track, len, -50.0), 0, "clamps left");
        assert_eq!(scroll_offset_for_thumb(track, len, 999.0), 8, "clamps right");
        // Half the travel is half the columns.
        let (_, w) = scroll_thumb_geom(track, len, 0);
        let travel = track - w;
        assert_eq!(scroll_offset_for_thumb(track, len, travel * 0.5), 4);
        // Round trip: every column maps to a thumb position that maps back.
        for bank in 0..=8 {
            let (x, _) = scroll_thumb_geom(track, len, bank);
            assert_eq!(scroll_offset_for_thumb(track, len, x), bank, "bank {bank}");
        }
        // Nothing to scroll: every drag stays at zero.
        assert_eq!(scroll_offset_for_thumb(track, 40, 300.0), 0);
    }

    #[test]
    fn strip_scrolls_by_whole_columns_and_rows_stay_put() {
        // 50 entries = 10 columns of 5; the 8-column window stops at col 2.
        assert_eq!(pad_cols(50), 10);
        assert_eq!(clamp_pad_offset(0, 50), 0);
        assert_eq!(clamp_pad_offset(100, 50), 2);
        assert_eq!(clamp_pad_offset(0, 10), 0);
        // Column order: the pad under pad 0 (next row) is the next entry;
        // the pad to the right starts the next column.
        assert_eq!(pad_entry_index(0, 0), 0);
        assert_eq!(pad_entry_index(0, PAD_COLS), 1);
        assert_eq!(pad_entry_index(0, 1), PAD_ROWS);
        // Scrolling one column: every row keeps its item's row.
        assert_eq!(pad_entry_index(1, 0), PAD_ROWS);
        assert_eq!(pad_entry_index(1, PAD_COLS + 1), 2 * PAD_ROWS + 1);
    }

    #[test]
    fn thumb_fills_track_when_content_is_a_single_page() {
        // 20 entries = 4 columns, well under the 8-column window: nothing
        // to scroll, so the thumb should be the full track and undraggable
        // (zero travel).
        let (x, w) = scroll_thumb_geom(200.0, 20, 0);
        assert_eq!(x, 0.0);
        assert_eq!(w, 200.0);
        assert_eq!(clamp_pad_offset(usize::MAX, 20), 0);
    }

    #[test]
    fn thumb_clamps_to_the_minimum_when_content_is_huge() {
        // 100_000 entries = 20_000 columns; the proportional width
        // (8/20_000 of 200px ~= 0.08px) must floor at SCROLL_THUMB_MIN.
        let (_, w) = scroll_thumb_geom(200.0, 100_000, 0);
        assert_eq!(w, SCROLL_THUMB_MIN);
    }

    #[test]
    fn thumb_position_tracks_the_column_offset() {
        // 50 entries = 10 columns; window of 8 leaves 2 columns of travel.
        let track_w = 200.0;
        let len = 50;
        let max_off = clamp_pad_offset(usize::MAX, len);
        assert_eq!(max_off, 2);

        let (x0, w) = scroll_thumb_geom(track_w, len, 0);
        assert_eq!(x0, 0.0);

        let (x_max, w_max) = scroll_thumb_geom(track_w, len, max_off);
        assert_eq!(w_max, w);
        assert_eq!(x_max, track_w - w);
    }

    #[test]
    fn drag_math_round_trips_through_every_offset() {
        // For every reachable column offset, converting it to a thumb
        // position and back must land on the exact same offset (pixel
        // math must not drift the drag around).
        let track_w = 240.0;
        let len = 137; // an offbeat count, not a multiple of anything
        let max_off = clamp_pad_offset(usize::MAX, len);
        assert!(max_off > 0, "fixture should actually be scrollable");
        for offset in 0..=max_off {
            let (x, _) = scroll_thumb_geom(track_w, len, offset);
            let back = scroll_offset_for_thumb(track_w, len, x);
            assert_eq!(back, offset, "offset {offset} did not round-trip");
        }
    }

    #[test]
    fn drag_math_clamps_positions_outside_the_track() {
        let track_w = 200.0;
        let len = 50;
        let max_off = clamp_pad_offset(usize::MAX, len);
        // Dragging the thumb's left edge past either end of the track
        // clamps to the first/last column, it does not panic or wrap.
        assert_eq!(scroll_offset_for_thumb(track_w, len, -500.0), 0);
        assert_eq!(scroll_offset_for_thumb(track_w, len, 5_000.0), max_off);
    }

    #[test]
    fn track_click_pages_toward_the_click_on_either_side() {
        let (thumb_x, thumb_w) = scroll_thumb_geom(200.0, 50, 4);
        // Click left of the thumb: page back.
        assert_eq!(scroll_page_dir(thumb_x - 10.0, thumb_x), -1);
        // Click right of the thumb (past its trailing edge): page forward.
        assert_eq!(scroll_page_dir(thumb_x + thumb_w + 10.0, thumb_x), 1);
    }

    #[test]
    fn scroll_geometry_guards_against_division_by_zero() {
        // Zero-width track: nothing to divide by, must not panic or NaN.
        let (x, w) = scroll_thumb_geom(0.0, 500, 0);
        assert_eq!((x, w), (0.0, 0.0));
        assert_eq!(scroll_offset_for_thumb(0.0, 500, 10.0), 0);

        // Empty content: pad_cols(0) == 0, still no panic/NaN, and the
        // thumb fills the (degenerate) track.
        let (x0, w0) = scroll_thumb_geom(150.0, 0, 0);
        assert_eq!(x0, 0.0);
        assert_eq!(w0, 150.0);
        assert_eq!(scroll_offset_for_thumb(150.0, 0, 75.0), 0);

        // A track narrower than the minimum thumb width must not panic on
        // an inverted `clamp(min, max)` range.
        let (_, w_narrow) = scroll_thumb_geom(10.0, 100_000, 0);
        assert_eq!(w_narrow, 10.0);
    }

    // -----------------------------------------------------------------
    // Regression: the thumb must size off the catalog TOTAL, not how many
    // pages `load_more()` has streamed in so far — otherwise it visibly
    // shrinks mid-drag as more pages land.
    // -----------------------------------------------------------------

    #[test]
    fn thumb_geometry_is_identical_before_and_after_pages_load() {
        // Same total (500), only the loaded count differs: one page in
        // (50) vs. everything loaded (500). `scroll_basis` must collapse
        // both to the same number, so every downstream computation is
        // byte-for-byte identical.
        let total = 500;
        let basis_first_page = scroll_basis(total, 50);
        let basis_fully_loaded = scroll_basis(total, 500);
        assert_eq!(basis_first_page, total);
        assert_eq!(basis_first_page, basis_fully_loaded);

        let track_w = 240.0;
        for bank in [0, 5, 50, 92] {
            assert_eq!(
                scroll_thumb_geom(track_w, basis_first_page, bank),
                scroll_thumb_geom(track_w, basis_fully_loaded, bank),
                "bank {bank} must not move/resize the thumb as pages stream in"
            );
        }
    }

    #[test]
    fn thumb_geometry_changes_only_when_total_itself_changes() {
        let track_w = 240.0;
        // Loaded count swinging from 10 to 400 with the SAME total: no
        // change (the case above generalized to width alone).
        let (_, w_before) = scroll_thumb_geom(track_w, scroll_basis(500, 10), 0);
        let (_, w_after) = scroll_thumb_geom(track_w, scroll_basis(500, 400), 0);
        assert_eq!(w_before, w_after);

        // A genuinely different total (a new search/refresh) DOES change
        // it.
        let (_, w_new_search) = scroll_thumb_geom(track_w, scroll_basis(120, 10), 0);
        assert_ne!(w_before, w_new_search);
    }

    #[test]
    fn max_offset_and_far_end_position_derive_from_total_not_loaded_count() {
        let track_w = 240.0;
        let total = 500;
        let loaded = 50; // only the first page has streamed in
        let basis = scroll_basis(total, loaded);
        assert_eq!(basis, total, "the loaded count must not leak into the basis");

        // total=500, rows=PAD_ROWS(5) -> total_cols = ceil(500/5) = 100;
        // visible=PAD_COLS(8) -> max_offset = 100 - 8 = 92.
        let max_off = clamp_pad_offset(usize::MAX, basis);
        assert_eq!(max_off, pad_cols(total) - PAD_COLS);
        assert_eq!(max_off, 92);

        // Dragging to the far end must reach the LAST column of the whole
        // catalog result, i.e. x == track_w - thumb_w, even though only
        // one page (50 of 500) is actually loaded.
        let (x, w) = scroll_thumb_geom(track_w, basis, max_off);
        assert_eq!(x, track_w - w);

        // And it must be a small (min-clamped) thumb, not one sized off
        // the 50 loaded entries (10 columns, which would fill most of the
        // track instead of floor at the minimum).
        assert_eq!(w, SCROLL_THUMB_MIN);
    }
}
