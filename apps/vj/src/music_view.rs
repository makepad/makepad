//! The two-deck music surface: scrolling band-coloured waveforms with one
//! shared playhead, whole-track overview strips, and the content explorer
//! and queue underneath.
//!
//! The waveforms are drawn from the analysis tiles as a texture — one texel
//! per column, three bands packed into RGB — so scrolling and zooming are a
//! uniform change rather than a per-frame CPU repaint. The beat grid is
//! computed in the same shader from the deck's grid, which is what makes
//! phase alignment between the two decks visible: when the decks are synced
//! their bar lines stand in the same places.
//!
//! Pointer input on a zoomed lane is a scratch: the widget turns pointer
//! velocity into a playback rate and hands it to the host as events, which
//! the deck engine routes to the mixer's vinyl ramps.

use crate::decks::DeckId;
use crate::wave_analysis::{TrackGrid, WaveTiles, ZOOM_COLS_PER_SEC};
use makepad_asset_data::AssetId;
use makepad_widgets::*;
use std::path::PathBuf;

/// Texture width for the packed tile columns; the rest wraps onto further
/// rows, so even a long track is one small texture.
const TILE_TEX_WIDTH: usize = 2048;
/// The stem palette — vocals, drums, bass, other, in the deck's stem
/// order. The waveform shader is fed these, and the STEM MIX knobs are
/// painted with them, so a colour in the wave and the knob that controls it
/// are the same colour by construction.
pub const STEM_COLORS: [[f32; 4]; 4] = [
    [0.133, 0.827, 0.933, 1.0], // vocals — cyan/teal
    [1.000, 0.624, 0.110, 1.0], // drums  — amber
    [0.753, 0.149, 0.827, 1.0], // bass   — magenta violet
    [0.357, 0.553, 0.937, 1.0], // other  — steel blue
];

/// A killed lane's knob: the same hue, drained of it.
pub const STEM_COLOR_KILLED: [f32; 4] = [0.35, 0.38, 0.42, 1.0];

/// Deepest pyramid level built: 2^15 finest columns is about five minutes
/// in one texel, past which a level holds a single column.
const MAX_WAVE_LEVELS: usize = 16;
/// One entry of [`STEM_COLORS`] as a shader colour.
pub fn stem_color(stem: usize) -> Vec4f {
    let c = STEM_COLORS[stem.min(STEM_COLORS.len() - 1)];
    vec4(c[0], c[1], c[2], c[3])
}

/// The same, greyed out for a killed lane.
pub fn stem_color_killed() -> Vec4f {
    let c = STEM_COLOR_KILLED;
    vec4(c[0], c[1], c[2], c[3])
}

/// Zoom limits, seconds of audio across the full lane width.
pub const ZOOM_MIN_SECS: f64 = 1.5;
pub const ZOOM_MAX_SECS: f64 = 32.0;
pub const ZOOM_DEFAULT_SECS: f64 = 8.0;
/// A pointer that has not moved for this long is holding the record still.
const SCRATCH_IDLE_SECS: f64 = 0.045;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // ---- one zoomed waveform lane ----------------------------------------
    set_type_default() do #(DrawWaveLane::script_shader(vm)){
        ..mod.draw.DrawQuad
        tiles: texture_2d(float)
        stem_tiles: texture_2d(float)

        color_bg: uniform(#x0a0d12)
        // Before separation a wave is grey peaks and nothing else: colour
        // in this view always means a real separated stem.
        color_grey: uniform(#x8b98a6)
        color_grid: uniform(#xffffff1e)
        color_grid_bar: uniform(#xffffff6e)
        color_head: uniform(#xf4f7fa)

        // One column of one pyramid level. Each level is stored as its own
        // block of rows in the same texture, so a level is a row offset and
        // a column count — hand-encoded mips.
        level_at: fn(column: float, base_row: float, level_cols: float, scale: float) -> vec3 {
            let c = clamp(floor(column / scale), 0.0, max(level_cols - 1.0, 0.0))
            let wrap = floor(c / self.tex_w)
            let u = (c - wrap * self.tex_w + 0.5) / self.tex_w
            let v = (base_row + wrap + 0.5) / self.tex_h
            let t = self.tiles.sample_as_bgra(vec2(u, v))
            return vec3(t.x, t.y, t.z)
        }

        // What this PIXEL covers, alias-free at any zoom. Each level is a
        // max-reduction of the one below, so a transient never disappears
        // as the view pulls back; the two levels either side of the current
        // scale are blended so zooming does not pop.
        tile_span: fn(column: float) -> vec3 {
            let lo = self.level_at(column, self.lo_row, self.lo_cols, self.lo_scale)
            if self.lod_blend <= 0.0 {
                if self.lo_scale <= 1.0 {
                    // Zoomed past one column per pixel: interpolate along
                    // the finest level so the envelope stays smooth. A
                    // column MEASURES a hop, so its value belongs at the
                    // hop's centre — sampling on column boundaries instead
                    // slides the whole waveform half a column (5 ms) left of
                    // the beat grid and of the stem colours, which are drawn
                    // by the un-interpolated path just below.
                    let base = floor(column - 0.5)
                    let f = column - 0.5 - base
                    let a = self.level_at(base, self.lo_row, self.lo_cols, 1.0)
                    let b = self.level_at(base + 1.0, self.lo_row, self.lo_cols, 1.0)
                    return a * (1.0 - f) + b * f
                }
                return lo
            }
            let hi = self.level_at(column, self.hi_row, self.hi_cols, self.hi_scale)
            return lo * (1.0 - self.lod_blend) + hi * self.lod_blend
        }

        // The same column of the stem pyramid — identical layout, so the
        // level selection above serves both.
        stem_level_at: fn(column: float, base_row: float, level_cols: float, scale: float) -> vec4 {
            let c = clamp(floor(column / scale), 0.0, max(level_cols - 1.0, 0.0))
            let wrap = floor(c / self.tex_w)
            let u = (c - wrap * self.tex_w + 0.5) / self.tex_w
            let v = (base_row + wrap + 0.5) / self.tex_h
            return self.stem_tiles.sample_as_bgra(vec2(u, v))
        }

        stem_span: fn(column: float) -> vec4 {
            let lo = self.stem_level_at(column, self.lo_row, self.lo_cols, self.lo_scale)
            if self.lod_blend <= 0.0 {
                return lo
            }
            let hi = self.stem_level_at(column, self.hi_row, self.hi_cols, self.hi_scale)
            return lo * (1.0 - self.lod_blend) + hi * self.lod_blend
        }

        // Beat and bar rulings, drawn UNDER the waveform.
        grid_at: fn(column: float) -> vec4 {
            if self.beat_cols < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let b = (column - self.beat_phase) / self.beat_cols
            let nb = floor(b + 0.5)
            let d = abs(b - nb) * self.beat_cols / max(self.cols_per_px, 0.0001)
            let is_bar = step(modf(nb + 4096.0, 4.0), 0.5)
            let half = mix(0.45, 1.0, is_bar)
            let a = 1.0 - smoothstep(half, half + 1.0, d)
            let c = self.color_grid.mix(self.color_grid_bar, is_bar)
            return vec4(c.x, c.y, c.z, c.w * a)
        }

        pixel: fn() {
            let px = self.pos.x * self.rect_size.x
            let column = self.centre_col + (px - self.rect_size.x * 0.5) * self.cols_per_px
            let bg = self.color_bg
            // No track: a quiet centre rule where the waveform will be.
            if self.cols < 1.0 {
                let y0 = abs(self.pos.y - 0.5) * self.rect_size.y
                return bg.mix(self.color_grid, 1.0 - smoothstep(0.5, 1.5, y0))
            }
            if column < 0.0 || column >= self.cols {
                return bg
            }
            let t = self.tile_span(column) * self.amp_scale
            // Each stem's contribution is scaled by its knob, so the SHAPE
            // of the wave is the shape of what the deck will play. The
            // overview strip passes ones here and stays the reference.
            let raw = self.stem_span(column) * self.amp_scale
            let stem = vec4(
                raw.x * self.gain_vocals,
                raw.y * self.gain_drums,
                raw.z * self.gain_bass,
                raw.w * self.gain_other
            )
            // A column the separator has reached is coloured by WHAT it is;
            // one it has not is coloured by frequency. Both are the same
            // mirrored, layered envelope, so the picture only gains meaning
            // as the separation catches up — it never jumps.
            let present = raw.x + raw.y + raw.z + raw.w
            let separated = step(0.004, present) * self.has_stems

            // Unseparated: one grey peak envelope, the honest picture of
            // audio nobody has taken apart yet.
            let peak = max(max(t.x, t.y), t.z)
            let grey_h = clamp(peak, 0.0, 1.0) * 0.78
            // Separated: bass at the core, then drums, vocals, other.
            let s_bass = clamp(stem.z, 0.0, 1.0) * 0.55
            let s_drums = clamp(stem.y, 0.0, 1.0) * 0.40
            let s_vocals = clamp(stem.x, 0.0, 1.0) * 0.36
            let s_other = clamp(stem.w, 0.0, 1.0) * 0.30

            let e0 = mix(grey_h, s_bass, separated)
            let e1 = mix(grey_h, s_bass + s_drums, separated)
            let e2raw = mix(grey_h, s_bass + s_drums + s_vocals, separated)
            let e2 = min(e2raw, 1.0)
            let e3 = min(mix(grey_h, e2raw + s_other, separated), 1.0)

            let c0 = self.color_grey.mix(self.color_bass, separated)
            let c1 = self.color_grey.mix(self.color_drums, separated)
            let c2 = self.color_grey.mix(self.color_vocals, separated)
            let c3 = self.color_grey.mix(self.color_other, separated)

            // Half-pixel feathering: the envelope edge stays smooth while
            // the whole thing scrolls, instead of crawling pixel to pixel.
            let y = abs(self.pos.y - 0.5) * 2.0
            let feather = 2.0 / max(self.rect_size.y, 2.0)
            let in0 = 1.0 - smoothstep(e0 - feather, e0 + feather, y)
            let in1 = 1.0 - smoothstep(e1 - feather, e1 + feather, y)
            let in2 = 1.0 - smoothstep(e2 - feather, e2 + feather, y)
            let in3 = 1.0 - smoothstep(e3 - feather, e3 + feather, y)

            let band = c0 * in0
                + c1 * (in1 - in0)
                + c2 * (in2 - in1)
                + c3 * (in3 - in2)
            let cover = clamp(max(in3, in2), 0.0, 1.0)

            // Loud passages glow: the core of a big hit lifts toward white.
            let energy = clamp(e3 * 1.35, 0.0, 1.0)
            let glow = energy * energy * (1.0 - smoothstep(0.0, e3 + 0.001, y)) * mix(0.18, 0.45, separated)
            let lit = vec3(band.x + glow, band.y + glow, band.z + glow)

            // Behind the playhead the music has been played; ahead of it is
            // what is coming, and that reads brighter.
            let played = step(column, self.centre_col)
            let level = mix(1.0, 0.58, played) * mix(0.45, 1.0, self.active)

            let g = self.grid_at(column)
            let under = bg.mix(vec4(g.x, g.y, g.z, 1.0), g.w)
            let wave = vec4(lit.x * level, lit.y * level, lit.z * level, 1.0)
            let body = under.mix(wave, cover)
            // A whisper of the rulings survives on top, so the two decks
            // can be read against each other through a loud passage.
            let ruled = body.mix(vec4(g.x, g.y, g.z, 1.0), g.w * 0.30)
            // The overview strip carries its own playhead; the zoomed lanes
            // share one drawn over both, so they pass head_on = 0.
            let hd = abs(column - self.head_col) / max(self.cols_per_px, 0.0001)
            let ha = (1.0 - smoothstep(0.5, 1.8, hd)) * self.head_on
            return ruled.mix(self.color_head, ha)
        }
    }

    // ---- whole-track overview strip ---------------------------------------
    mod.widgets.VjWaveScrollBase = #(VjWaveScroll::register_widget(vm))
    mod.widgets.VjWaveScroll = set_type_default() do mod.widgets.VjWaveScrollBase{
        width: Fill
        height: Fill
        draw_text +: {
            color: #x8e9aa7
            text_style: theme.font_bold{font_size: 8}
        }
        draw_head +: {
            color: uniform(#xffffff)
            glow: uniform(#x46e8a8)
            pixel: fn() {
                let px = (self.pos.x - 0.5) * self.rect_size.x
                let d = abs(px)
                // A hard 2px core with a soft halo either side: unmissable
                // over a bright waveform, not a bar across the picture.
                let core = 1.0 - smoothstep(0.8, 1.6, d)
                let halo = (1.0 - smoothstep(1.5, 7.0, d)) * 0.30
                let a = clamp(core + halo, 0.0, 1.0)
                let c = self.glow.mix(self.color, core)
                return vec4(c.x * a, c.y * a, c.z * a, a)
            }
        }
    }

    mod.widgets.VjWaveOverviewBase = #(VjWaveOverview::register_widget(vm))
    mod.widgets.VjWaveOverview = set_type_default() do mod.widgets.VjWaveOverviewBase{
        width: Fill
        height: 44
    }

    // ---- explorer / queue rows --------------------------------------------
    let TrackText = Label{
        flow: Flow.Right{wrap: false}
        max_lines: 1
        draw_text.color: #xd6dee6
        draw_text.text_style.font_size: 9
    }

    mod.widgets.VjTrackListBase = #(VjTrackList::register_widget(vm))
    mod.widgets.VjTrackList = set_type_default() do mod.widgets.VjTrackListBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            spacing: 1
            // VJ law: a drag belongs to a control, never to a view.
            drag_scrolling: false
            TrackRow := RoundedView{
                width: Fill
                height: 22
                padding: 0
                draw_bg +: {
                    color: #x141920
                    color_alt: #x11161c
                    color_live: #x1d2a2a
                    live: instance(0.0)
                    odd: instance(0.0)
                    border_radius: 3.0
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
                        sdf.fill(self.color.mix(self.color_alt, self.odd).mix(self.color_live, self.live))
                        return sdf.result
                    }
                }
                row_body := View{
                width: Fill
                height: Fill
                flow: Right
                spacing: 6
                padding: Inset{left: 6.0 right: 6.0 top: 0.0 bottom: 0.0}
                align: Align{x: 0.0, y: 0.5}
                cursor: MouseCursor.Hand
                row_badge := Label{
                    width: 26
                    text: ""
                    draw_text.color: #x3ee0b0
                    draw_text.text_style: theme.font_bold{font_size: 8}
                }
                row_title := TrackText{width: Fill}
                row_artist := TrackText{width: 150 draw_text.color: #x9fabb7}
                row_bpm := TrackText{
                    width: 54
                    draw_text.color: #x3ee0b0
                    draw_text.text_style: theme.font_bold{font_size: 9}
                }
                row_key := TrackText{width: 40 draw_text.color: #xc6a0f0}
                row_time := TrackText{width: 52 draw_text.color: #x9fabb7}
                row_tags := TrackText{width: 190 draw_text.color: #x6f7b87}
                row_queue := Button{
                    width: 26
                    height: 18
                    text: "+"
                    draw_bg +: {
                        color: #x1f262f
                        color_hover: #x2b3440
                        color_down: #x161b22
                        border_color: #xffffff26
                        border_radius: 4.0
                        border_size: 1.0
                    }
                    draw_text +: {
                        color: #xd6dee6
                        text_style: theme.font_bold{font_size: 9}
                    }
                }
                }
            }
            TrackEmpty := View{
                width: Fill
                height: 40
                align: Align{x: 0.5, y: 0.5}
                empty_label := Label{
                    text: "no tracks"
                    draw_text.color: #x8e9aa7
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // the deck surface
    // -----------------------------------------------------------------

    let MusicLabel = Label{
        draw_text.color: #xa6b1bd
        draw_text.text_style: theme.font_bold{font_size: 8}
    }

    let MusicValue = Label{
        flow: Flow.Right{wrap: false}
        max_lines: 1
        draw_text.color: #xe8eef4
        draw_text.text_style.font_size: 11
    }

    let MusicButton = Button{
        draw_bg +: {
            color: #x1f262f
            color_focus: #x1f262f
            color_hover: #x2b3440
            color_down: #x161b22
            border_color: #xffffff2e
            border_radius: 6.0
            border_size: 1.0
        }
        draw_text +: {
            color: #xd6dee6
            color_focus: #xd6dee6
            color_hover: #xfffaf4
            text_style: theme.font_bold{font_size: 9}
        }
    }

    let MusicIconButton = ButtonIcon{
        width: 30
        height: 24
        icon_walk: Walk{width: 12 height: Fit}
        draw_bg +: {
            color: #x1f262f
            color_focus: #x1f262f
            color_hover: #x2b3440
            color_down: #x161b22
            border_color: #xffffff26
            border_radius: 5.0
            border_size: 1.0
        }
        draw_icon +: {
            color: #xd6dee6
        }
    }

    // Kill switch under a knob: the host paints it hot through draw_bg.color.
    let KillButton = MusicButton{
        width: Fill
        height: 13
        text: "KILL"
        draw_text +: {
            text_style: theme.font_bold{font_size: 7}
        }
        draw_bg +: {
            border_radius: 3.0
        }
    }

    let MusicKnob = Rotary{
        width: 42
        height: 42
        min: 0.0
        max: 2.0
        default: 1.0
        text: ""
        flow: Down
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x1c222b)
            body_color_hover: uniform(#x2a323d)
            rim_color: uniform(#xffffff40)
            ring_color: uniform(#x2f3842)
            val_color: uniform(#x3ee0b0)
            pointer_color: uniform(#xf2f6fa)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let r = min(self.rect_size.x, self.rect_size.y) * 0.5
                let start = PI * 0.25
                let sweep = PI * 1.5
                sdf.arc_round_caps(c.x, c.y, r - 2.5, start, start + sweep, 2.5)
                sdf.fill(self.ring_color)
                let lit = max(self.slide_pos, 0.01)
                sdf.arc_round_caps(c.x, c.y, r - 2.5, start, start + sweep * lit, 2.5)
                sdf.fill(self.val_color)
                sdf.circle(c.x, c.y, r - 7.5)
                sdf.fill_keep(self.body_color.mix(self.body_color_hover, max(self.hover, self.drag)))
                sdf.stroke(self.rim_color, 1.0)
                let a = start + sweep * self.slide_pos
                let d = vec2(-sin(a), cos(a))
                let p0 = c + d * (r - 13.0)
                let p1 = c + d * (r - 8.0)
                sdf.move_to(p0.x, p0.y)
                sdf.line_to(p1.x, p1.y)
                sdf.stroke(self.pointer_color, 2.0)
                return sdf.result
            }
        }
    }

    // A knob's legend: fills its stack, never widens it, never wraps.
    let KnobLabel = MusicLabel{
        width: Fill
        flow: Flow.Right{wrap: false}
        max_lines: 1
        draw_text.text_style: theme.font_bold{font_size: 7}
    }

    let KnobStack = View{
        width: 46
        height: Fit
        flow: Down
        spacing: 2
        align: Align{x: 0.5, y: 0.0}
    }

    // Four stems have to fit the same width three tone bands do.
    let StemStack = KnobStack{width: 44}
    let StemKnob = MusicKnob{width: 40 height: 40}

    let MusicFader = Slider{
        axis: DragAxis.Vertical
        width: 40
        height: Fill
        text: ""
        flow: Down
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x151a21)
            track_color: uniform(#x2b343f)
            fill_color: uniform(#x3ee0b0)
            cap_color: uniform(#xe8eef4)
            cap_shadow: uniform(#x8d98a7)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(3., 2., self.rect_size.x - 6., self.rect_size.y - 4., 6.)
                sdf.fill(self.body_color)
                let top = 10.
                let h = self.rect_size.y - top - 10.
                let track_w = 7.
                let track_x = (self.rect_size.x - track_w) * 0.5
                sdf.box(track_x, top, track_w, h, 3.)
                sdf.fill(self.track_color)
                let fill_h = max(1., h * self.slide_pos)
                sdf.box(track_x + 1.5, top + (h - fill_h) + 1.5, track_w - 3., max(1., fill_h - 3.), 2.)
                sdf.fill(self.fill_color)
                let cap_h = 13.
                let cap_y = top + (h - fill_h) - cap_h * 0.5
                sdf.box(5., cap_y + 1.5, self.rect_size.x - 10., cap_h, 4.)
                sdf.fill(self.cap_shadow)
                sdf.box(4., cap_y, self.rect_size.x - 8., cap_h, 4.)
                sdf.fill(self.cap_color)
                return sdf.result
            }
        }
    }

    let CrossFader = Slider{
        width: Fill
        height: 40
        min: 0.0
        max: 1.0
        text: ""
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x151a21)
            track_color: uniform(#x2b343f)
            fill_color: uniform(#x3ee0b0)
            cap_color: uniform(#xe8eef4)
            cap_shadow: uniform(#x8d98a7)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(2., 6., self.rect_size.x - 4., self.rect_size.y - 12., 8.)
                sdf.fill(self.body_color)
                let left = 10.
                let w = self.rect_size.x - left - 10.
                let track_h = 9.
                let track_y = (self.rect_size.y - track_h) * 0.5
                sdf.box(left, track_y, w, track_h, 4.)
                sdf.fill(self.track_color)
                let fill_w = max(1., w * self.slide_pos)
                sdf.box(left + 1.5, track_y + 1.5, max(1., fill_w - 3.), track_h - 3., 3.)
                sdf.fill(self.fill_color)
                let cap_w = 20.
                let cap_x = left + fill_w - cap_w * 0.5
                sdf.box(cap_x + 1.5, 8., cap_w, self.rect_size.y - 16., 6.)
                sdf.fill(self.cap_shadow)
                sdf.box(cap_x, 6., cap_w, self.rect_size.y - 14., 6.)
                sdf.fill(self.cap_color)
                return sdf.result
            }
        }
    }

    // A channel meter: one uniform, painted as a segmented column.
    let DeckMeter = SolidView{
        width: 9
        height: Fill
        draw_bg +: {
            level: uniform(0.0)
            color: uniform(#x151a21)
            color_lit: uniform(#x3ee0b0)
            color_hot: uniform(#xff5a4e)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 3.0)
                sdf.fill(self.color)
                let y = 1.0 - self.pos.y
                let on = step(y, self.level)
                let hot = smoothstep(0.72, 0.95, y)
                let seg = step(0.35, fract(self.pos.y * self.rect_size.y / 4.0))
                let c = self.color_lit.mix(self.color_hot, hot)
                let h = max(1.0, self.rect_size.y * self.level)
                sdf.box(1.5, self.rect_size.y - h, self.rect_size.x - 3.0, h, 2.0)
                sdf.fill(vec4(c.x, c.y, c.z, c.w * on * seg))
                return sdf.result
            }
        }
    }

    let DeckWell = RoundedView{
        width: Fill
        height: Fill
        padding: 1
        draw_bg +: {
            color: #x000000
            border_color: #xffffff26
            border_size: 1.0
            border_radius: 8.0
        }
    }

    mod.widgets.MusicDeckPage = View{
        width: Fill
        height: Fill
        flow: Down
        spacing: 6

        // ---- deck headers: art, title, tempo, key slot, elapsed ----
        //
        // Every panel of this page carries `new_batch: true`, which gives it
        // its own draw list. The lanes repaint at the display's rate while a
        // deck plays; without the split, each of those frames re-walked and
        // re-drew the whole console — the panels below, the track lists, and
        // (through the status bar) the offscreen 3D passes. A panel now
        // redraws only when its own contents change.
        View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 10
            new_batch: true
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                align: Align{x: 0.0, y: 0.5}
                Label{
                    text: "A"
                    draw_text.color: #x3ee0b0
                    draw_text.text_style: theme.font_bold{font_size: 13}
                }
                deck_a_art := Image{width: 44 height: 44}
                View{
                    width: Fill
                    height: Fit
                    flow: Down
                    deck_a_title := MusicValue{width: Fill text: "empty"}
                    deck_a_artist := MusicLabel{width: Fill text: ""}
                }
                View{
                    width: Fit
                    height: Fit
                    flow: Down
                    align: Align{x: 1.0, y: 0.5}
                    deck_a_bpm := Label{
                        text: "---.-"
                        draw_text.color: #x3ee0b0
                        draw_text.text_style: theme.font_bold{font_size: 17}
                    }
                    deck_a_pitch_text := MusicLabel{text: "+0.0%"}
                }
                deck_a_key := Label{
                    width: 34
                    text: "—"
                    draw_text.color: #xc6a0f0
                    draw_text.text_style: theme.font_bold{font_size: 11}
                }
                deck_a_time := MusicLabel{width: 78 text: "0:00 / 0:00"}
            }
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                align: Align{x: 0.0, y: 0.5}
                deck_b_time := MusicLabel{width: 78 text: "0:00 / 0:00"}
                deck_b_key := Label{
                    width: 34
                    text: "—"
                    draw_text.color: #xc6a0f0
                    draw_text.text_style: theme.font_bold{font_size: 11}
                }
                View{
                    width: Fit
                    height: Fit
                    flow: Down
                    align: Align{x: 0.0, y: 0.5}
                    deck_b_bpm := Label{
                        text: "---.-"
                        draw_text.color: #x6aa8ff
                        draw_text.text_style: theme.font_bold{font_size: 17}
                    }
                    deck_b_pitch_text := MusicLabel{text: "+0.0%"}
                }
                View{
                    width: Fill
                    height: Fit
                    flow: Down
                    deck_b_title := MusicValue{width: Fill text: "empty"}
                    deck_b_artist := MusicLabel{width: Fill text: ""}
                }
                deck_b_art := Image{width: 44 height: 44}
                Label{
                    text: "B"
                    draw_text.color: #x6aa8ff
                    draw_text.text_style: theme.font_bold{font_size: 13}
                }
            }
        }

        // ---- whole-track overview strips ----
        View{
            width: Fill
            height: 46
            flow: Right
            spacing: 10
            new_batch: true
            DeckWell{
                width: Fill
                deck_a_overview := mod.widgets.VjWaveOverview{height: Fill}
            }
            DeckWell{
                width: Fill
                deck_b_overview := mod.widgets.VjWaveOverview{height: Fill}
            }
        }

        // ---- controls | stacked zoomed waveforms | controls ----
        View{
            width: Fill
            height: Fill
            flow: Right
            spacing: 8

            View{
                width: 316
                height: Fill
                flow: Down
                spacing: 5
                new_batch: true
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 4
                    align: Align{x: 0.0, y: 0.5}
                    deck_a_sync := MusicButton{width: Fill height: 22 text: "SYNC"}
                    deck_a_keylock := MusicButton{width: 44 height: 22 text: "KEY"}
                    deck_a_range := MusicButton{width: 46 height: 22 text: "±8%"}
                }
                View{
                    width: Fill
                    height: Fill
                    flow: Right
                    spacing: 8
                    View{
                        width: 104
                        height: Fill
                        flow: Right
                        spacing: 6
                        View{
                            width: 44
                            height: Fill
                            flow: Down
                            spacing: 2
                            align: Align{x: 0.5, y: 0.0}
                            MusicLabel{text: "PITCH"}
                            deck_a_pitch := MusicFader{min: -1.0 max: 1.0 default: 0.0}
                            deck_a_pitch_reset := MusicButton{width: Fill height: 14 text: "0"}
                        }
                        View{
                            width: 44
                            height: Fill
                            flow: Down
                            spacing: 2
                            align: Align{x: 0.5, y: 0.0}
                            MusicLabel{text: "VOL"}
                            deck_a_gain := MusicFader{min: 0.0 max: 1.5 default: 1.0}
                        }
                        View{
                            width: 10
                            height: Fill
                            flow: Down
                            spacing: 2
                            align: Align{x: 0.5, y: 0.0}
                            MusicLabel{text: ""}
                            deck_a_vu := DeckMeter{}
                        }
                    }
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 4
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 3
                            KnobStack{
                                KnobLabel{text: "HIGH"}
                                deck_a_eq_high := MusicKnob{}
                                deck_a_kill_high := KillButton{}
                            }
                            KnobStack{
                                KnobLabel{text: "MID"}
                                deck_a_eq_mid := MusicKnob{}
                                deck_a_kill_mid := KillButton{}
                            }
                            KnobStack{
                                KnobLabel{text: "LOW"}
                                deck_a_eq_low := MusicKnob{}
                                deck_a_kill_low := KillButton{}
                            }
                            KnobStack{
                                KnobLabel{text: "FILTER"}
                                deck_a_filter := MusicKnob{min: 0.0 max: 1.0 default: 0.5}
                            }
                        }
                        MusicLabel{text: "STEM MIX"}
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 3
                            StemStack{
                                deck_a_label_drums := KnobLabel{text: "DRUMS"}
                                deck_a_stem_drums := StemKnob{}
                                deck_a_kill_drums := KillButton{}
                            }
                            StemStack{
                                deck_a_label_bass := KnobLabel{text: "BASS"}
                                deck_a_stem_bass := StemKnob{}
                                deck_a_kill_bass := KillButton{}
                            }
                            StemStack{
                                deck_a_label_vocals := KnobLabel{text: "VOCALS"}
                                deck_a_stem_vocals := StemKnob{}
                                deck_a_kill_vocals := KillButton{}
                            }
                            StemStack{
                                deck_a_label_other := KnobLabel{text: "OTHER"}
                                deck_a_stem_other := StemKnob{}
                                deck_a_kill_other := KillButton{}
                            }
                        }
                        deck_a_stem_state := KnobLabel{text: "stems: full mix"}
                        deck_a_grid_state := KnobLabel{text: ""}
                        View{width: Fill height: Fill}
                    }
                }
            }

            DeckWell{
                width: Fill
                height: Fill
                // The lanes repaint every frame while a deck plays. Their
                // own draw list keeps that off the rest of the console:
                // scrolling costs two textured quads, not a whole UI pass.
                View{
                    width: Fill
                    height: Fill
                    new_batch: true
                    music_waves := mod.widgets.VjWaveScroll{}
                }
            }

            View{
                width: 316
                height: Fill
                flow: Down
                spacing: 5
                new_batch: true
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 4
                    align: Align{x: 0.0, y: 0.5}
                    deck_b_range := MusicButton{width: 46 height: 22 text: "±8%"}
                    deck_b_keylock := MusicButton{width: 44 height: 22 text: "KEY"}
                    deck_b_sync := MusicButton{width: Fill height: 22 text: "SYNC"}
                }
                View{
                    width: Fill
                    height: Fill
                    flow: Right
                    spacing: 8
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 4
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 3
                            KnobStack{
                                KnobLabel{text: "FILTER"}
                                deck_b_filter := MusicKnob{min: 0.0 max: 1.0 default: 0.5}
                            }
                            KnobStack{
                                KnobLabel{text: "LOW"}
                                deck_b_eq_low := MusicKnob{}
                                deck_b_kill_low := KillButton{}
                            }
                            KnobStack{
                                KnobLabel{text: "MID"}
                                deck_b_eq_mid := MusicKnob{}
                                deck_b_kill_mid := KillButton{}
                            }
                            KnobStack{
                                KnobLabel{text: "HIGH"}
                                deck_b_eq_high := MusicKnob{}
                                deck_b_kill_high := KillButton{}
                            }
                        }
                        MusicLabel{text: "STEM MIX"}
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 3
                            StemStack{
                                deck_b_label_drums := KnobLabel{text: "DRUMS"}
                                deck_b_stem_drums := StemKnob{}
                                deck_b_kill_drums := KillButton{}
                            }
                            StemStack{
                                deck_b_label_bass := KnobLabel{text: "BASS"}
                                deck_b_stem_bass := StemKnob{}
                                deck_b_kill_bass := KillButton{}
                            }
                            StemStack{
                                deck_b_label_vocals := KnobLabel{text: "VOCALS"}
                                deck_b_stem_vocals := StemKnob{}
                                deck_b_kill_vocals := KillButton{}
                            }
                            StemStack{
                                deck_b_label_other := KnobLabel{text: "OTHER"}
                                deck_b_stem_other := StemKnob{}
                                deck_b_kill_other := KillButton{}
                            }
                        }
                        deck_b_stem_state := KnobLabel{text: "stems: full mix"}
                        deck_b_grid_state := KnobLabel{text: ""}
                        View{width: Fill height: Fill}
                    }
                    View{
                        width: 104
                        height: Fill
                        flow: Right
                        spacing: 6
                        View{
                            width: 10
                            height: Fill
                            flow: Down
                            spacing: 2
                            align: Align{x: 0.5, y: 0.0}
                            MusicLabel{text: ""}
                            deck_b_vu := DeckMeter{}
                        }
                        View{
                            width: 44
                            height: Fill
                            flow: Down
                            spacing: 2
                            align: Align{x: 0.5, y: 0.0}
                            MusicLabel{text: "VOL"}
                            deck_b_gain := MusicFader{min: 0.0 max: 1.5 default: 1.0}
                        }
                        View{
                            width: 44
                            height: Fill
                            flow: Down
                            spacing: 2
                            align: Align{x: 0.5, y: 0.0}
                            MusicLabel{text: "PITCH"}
                            deck_b_pitch := MusicFader{min: -1.0 max: 1.0 default: 0.0}
                            deck_b_pitch_reset := MusicButton{width: Fill height: 14 text: "0"}
                        }
                    }
                }
            }
        }

        // ---- transport + crossfader ----
        View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            new_batch: true
            align: Align{x: 0.0, y: 0.5}
            View{
                width: 316
                height: Fit
                flow: Right
                spacing: 3
                align: Align{x: 0.0, y: 0.5}
                deck_a_play := MusicIconButton{
                    draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") }
                }
                deck_a_cue := MusicButton{width: 40 height: 24 text: "CUE"}
                deck_a_loop := MusicIconButton{
                    draw_icon +: { svg: crate_resource("self:resources/icons/loop.svg") }
                }
                deck_a_loop_halve := MusicButton{width: 22 height: 24 text: "<"}
                deck_a_loop_len := MusicLabel{width: 26 text: "4"}
                deck_a_loop_double := MusicButton{width: 22 height: 24 text: ">"}
                deck_a_mute := MusicIconButton{
                    draw_icon +: { svg: crate_resource("self:resources/icons/mute.svg") }
                }
            }
            View{
                width: Fill
                height: Fit
                flow: Down
                spacing: 3
                align: Align{x: 0.5, y: 0.5}
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.0, y: 0.5}
                    MusicLabel{width: 12 text: "A"}
                    xfader := CrossFader{}
                    MusicLabel{width: 12 text: "B"}
                }
                View{
                    width: Fit
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.5, y: 0.5}
                    fade_to_a := MusicButton{width: 46 height: 22 text: "◀ A"}
                    auto_sync := MusicButton{width: 92 height: 22 text: "AUTO SYNC"}
                    decks_swap := MusicButton{width: 56 height: 22 text: "SWAP"}
                    xfade_secs := Slider{
                        width: 130
                        text: "fade"
                        min: 0.05
                        max: 20.0
                        default: 4.0
                    }
                    xcurve := DropDown{labels: ["Equal power" "Linear"]}
                    fade_to_b := MusicButton{width: 46 height: 22 text: "B ▶"}
                }
            }
            View{
                width: 316
                height: Fit
                flow: Right
                spacing: 3
                align: Align{x: 1.0, y: 0.5}
                deck_b_mute := MusicIconButton{
                    draw_icon +: { svg: crate_resource("self:resources/icons/mute.svg") }
                }
                deck_b_loop_halve := MusicButton{width: 22 height: 24 text: "<"}
                deck_b_loop_len := MusicLabel{width: 26 text: "4"}
                deck_b_loop_double := MusicButton{width: 22 height: 24 text: ">"}
                deck_b_loop := MusicIconButton{
                    draw_icon +: { svg: crate_resource("self:resources/icons/loop.svg") }
                }
                deck_b_cue := MusicButton{width: 40 height: 24 text: "CUE"}
                deck_b_play := MusicIconButton{
                    draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") }
                }
            }
        }

        // ---- content explorer + queue ----
        View{
            width: Fill
            height: 236
            flow: Right
            spacing: 8
            new_batch: true
            View{
                width: Fill
                height: Fill
                flow: Down
                spacing: 4
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.0, y: 0.5}
                    music_search := TextInput{
                        width: Fill
                        empty_text: "search music…"
                    }
                    music_category := TextInput{
                        width: 96
                        empty_text: "category"
                    }
                    music_go := MusicButton{width: 60 height: 22 text: "Search"}
                    music_more := MusicButton{width: 52 height: 22 text: "More"}
                    music_local := MusicButton{width: 84 height: 22 text: "LOCAL FILES"}
                    music_count := MusicLabel{width: 90 text: ""}
                    MusicLabel{text: "load"}
                    deck_target := DropDown{labels: ["Auto" "Deck A" "Deck B"]}
                }
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    padding: Inset{left: 6.0 right: 6.0 top: 0.0 bottom: 0.0}
                    MusicLabel{width: 26 text: ""}
                    MusicLabel{width: Fill text: "TITLE"}
                    MusicLabel{width: 150 text: "ARTIST"}
                    MusicLabel{width: 54 text: "BPM"}
                    MusicLabel{width: 40 text: "KEY"}
                    MusicLabel{width: 52 text: "TIME"}
                    MusicLabel{width: 190 text: "TAGS"}
                    MusicLabel{width: 26 text: ""}
                }
                music_tracks := mod.widgets.VjTrackList{show_queue_button: true}
            }
            View{
                width: 320
                height: Fill
                flow: Down
                spacing: 4
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.0, y: 0.5}
                    Label{
                        text: "QUEUE"
                        draw_text.color: #x3ee0b0
                        draw_text.text_style: theme.font_bold{font_size: 10}
                    }
                    queue_count := MusicLabel{width: Fill text: ""}
                    queue_clear := MusicButton{width: 50 height: 20 text: "Clear"}
                }
                music_queue := mod.widgets.VjTrackList{show_queue_button: false}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// shaders
// ---------------------------------------------------------------------------

/// One zoomed lane. Only `#[live]` instance fields sit after the `#[deref]`,
/// per the draw-shader layout law.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWaveLane {
    #[deref]
    pub draw_super: DrawQuad,
    /// Tile-texture dimensions, in texels.
    #[live(1.0)]
    pub tex_w: f32,
    #[live(1.0)]
    pub tex_h: f32,
    /// Valid tile columns for this track, at the finest level.
    #[live]
    pub cols: f32,
    /// Finer of the two pyramid levels in play: first row, column count,
    /// and how many finest-level columns each of its columns covers.
    #[live]
    pub lo_row: f32,
    #[live(1.0)]
    pub lo_cols: f32,
    #[live(1.0)]
    pub lo_scale: f32,
    /// The next level up, and the blend between them.
    #[live]
    pub hi_row: f32,
    #[live(1.0)]
    pub hi_cols: f32,
    #[live(2.0)]
    pub hi_scale: f32,
    #[live]
    pub lod_blend: f32,
    /// The tile column under the centre playhead.
    #[live]
    pub centre_col: f32,
    /// Zoom: tile columns per screen pixel.
    #[live(1.0)]
    pub cols_per_px: f32,
    /// Beat period in tile columns; 0 = no grid yet.
    #[live]
    pub beat_cols: f32,
    /// A column that is a downbeat, so bars rule where the music does.
    #[live]
    pub beat_phase: f32,
    /// Display gain on the packed band energies.
    #[live(1.0)]
    pub amp_scale: f32,
    /// 1 for a deck that is playing, less for a parked one.
    #[live(1.0)]
    pub active: f32,
    /// 1 once a stem texture is bound; 0 keeps the band colouring.
    #[live]
    pub has_stems: f32,
    /// The stem palette, pushed from [`STEM_COLORS`] every draw so the
    /// waveform and the knobs cannot disagree.
    #[live]
    pub color_vocals: Vec4f,
    #[live]
    pub color_drums: Vec4f,
    #[live]
    pub color_bass: Vec4f,
    #[live]
    pub color_other: Vec4f,
    /// The stem knobs, so the wave shows the mix that will play.
    #[live(1.0)]
    pub gain_vocals: f32,
    #[live(1.0)]
    pub gain_drums: f32,
    #[live(1.0)]
    pub gain_bass: f32,
    #[live(1.0)]
    pub gain_other: f32,
    /// Playhead column and whether to draw it in-shader (the overview does;
    /// the zoomed lanes share one drawn across both).
    #[live]
    pub head_col: f32,
    #[live]
    pub head_on: f32,
}

// ---------------------------------------------------------------------------
// tile textures
// ---------------------------------------------------------------------------

/// One level of the waveform pyramid inside the shared texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaveLevel {
    /// First texture row this level occupies.
    pub base_row: usize,
    /// Columns at this level.
    pub cols: usize,
}

/// The whole waveform store for one track: a stack of max-reduced levels in
/// one texture. Scrolling, zooming and the playhead are uniform changes
/// against this; only new audio appends anything.
#[derive(Clone)]
pub struct WavePyramid {
    pub texture: Texture,
    pub width: usize,
    pub height: usize,
    pub levels: Vec<WaveLevel>,
}

/// The two levels that bracket `cols_per_px`, and the blend between them.
///
/// Level L holds one column per 2^L of the finest ones, so the level whose
/// columns are about one screen pixel wide is the one that cannot alias.
/// Blending toward the next keeps a zoom smooth instead of popping.
pub fn levels_for(
    levels: &[WaveLevel],
    cols_per_px: f64,
) -> (WaveLevel, f64, WaveLevel, f64, f64) {
    if levels.is_empty() {
        let empty = WaveLevel { base_row: 0, cols: 0 };
        return (empty, 1.0, empty, 1.0, 0.0);
    }
    let last = levels.len() - 1;
    let lod = if cols_per_px > 1.0 { cols_per_px.log2() } else { 0.0 };
    let lo_index = (lod.floor().max(0.0) as usize).min(last);
    let hi_index = (lo_index + 1).min(last);
    let blend = if hi_index == lo_index {
        0.0
    } else {
        (lod - lo_index as f64).clamp(0.0, 1.0)
    };
    (
        levels[lo_index],
        (1u64 << lo_index) as f64,
        levels[hi_index],
        (1u64 << hi_index) as f64,
        blend,
    )
}

impl WavePyramid {
    pub fn levels_for(&self, cols_per_px: f64) -> (WaveLevel, f64, WaveLevel, f64, f64) {
        levels_for(&self.levels, cols_per_px)
    }
}

/// Build a pyramid from four-channel columns.
///
/// Level 0 is the source resolution; each level above is a MAX reduction of
/// the pair below it — peaks survive all the way up, which is what makes a
/// pulled-back view read as music instead of mush, and what stops a
/// zoomed-out waveform aliasing.
pub fn build_pyramid(cx: &mut Cx, columns: &[[u8; 4]]) -> Option<WavePyramid> {
    if columns.is_empty() {
        return None;
    }
    let width = TILE_TEX_WIDTH.min(columns.len().max(1));
    let mut pyramid: Vec<Vec<[u8; 4]>> = vec![columns.to_vec()];
    while pyramid.last().map(|level| level.len()).unwrap_or(0) > 1
        && pyramid.len() < MAX_WAVE_LEVELS
    {
        let below = pyramid.last().expect("checked");
        let mut level = Vec::with_capacity(below.len().div_ceil(2));
        for pair in below.chunks(2) {
            let mut column = pair[0];
            if let Some(second) = pair.get(1) {
                for channel in 0..4 {
                    column[channel] = column[channel].max(second[channel]);
                }
            }
            level.push(column);
        }
        pyramid.push(level);
    }

    let mut levels = Vec::with_capacity(pyramid.len());
    let mut height = 0usize;
    for level in &pyramid {
        levels.push(WaveLevel { base_row: height, cols: level.len() });
        height += level.len().div_ceil(width);
    }

    let mut data = vec![0u32; width * height];
    for (level, info) in pyramid.iter().zip(&levels) {
        for (index, column) in level.iter().enumerate() {
            let row = info.base_row + index / width;
            let texel = row * width + (index % width);
            data[texel] = ((column[3] as u32) << 24)
                | ((column[0] as u32) << 16)
                | ((column[1] as u32) << 8)
                | column[2] as u32;
        }
    }
    let texture = Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 {
            width,
            height,
            data: Some(data),
            updated: TextureUpdated::Full,
        },
    );
    Some(WavePyramid { texture, width, height, levels })
}

/// The three-band pyramid: red = low, green = mid, blue = high.
pub fn zoom_texture(cx: &mut Cx, tiles: &WaveTiles) -> Option<WavePyramid> {
    let columns: Vec<[u8; 4]> = tiles
        .zoom
        .iter()
        .map(|c| [c[0], c[1], c[2], 255])
        .collect();
    build_pyramid(cx, &columns)
}

/// The stem-energy pyramid, laid out identically to the band one so the
/// shader can sample both with the same level selection: red = vocals,
/// green = drums, blue = bass, alpha = other.
pub fn stem_texture(cx: &mut Cx, columns: &[[u8; 4]]) -> Option<WavePyramid> {
    build_pyramid(cx, columns)
}

// ---------------------------------------------------------------------------
// the stacked scrolling waveforms
// ---------------------------------------------------------------------------

/// Everything one lane needs to draw itself.
#[derive(Clone, Default)]
pub struct WaveLane {
    /// The track's waveform pyramid, once analysis has produced one.
    pub pyramid: Option<WavePyramid>,
    /// The stem-energy pyramid, once the separator has covered anything.
    /// Laid out identically, so one level selection serves both.
    pub stem_pyramid: Option<WavePyramid>,
    pub cols: usize,
    /// Source seconds under the shared playhead.
    pub position_secs: f64,
    pub grid: Option<TrackGrid>,
    /// Playback rate, so the grid rules where the music actually lands.
    pub rate: f64,
    pub playing: bool,
    pub loaded: bool,
    /// The deck's stem knobs, so the wave shows the mix that will play.
    pub stem_gain: [f32; 4],
    /// A hand is on this record: the playhead is whatever the mixer last
    /// said, never extrapolated — a scrub does not move at tempo.
    pub scratching: bool,
    /// App-clock reading when `position_secs` was last set. The host only
    /// samples the device clock a few times a second; between those the
    /// lane carries the playhead forward itself, which is what makes the
    /// scroll smooth instead of stepping twenty times a second.
    pub stamp: f64,
}

impl WaveLane {
    /// Where the playhead is now: the last sampled position, carried
    /// forward at the deck's rate.
    pub fn position_at(&self, now: f64) -> f64 {
        if !self.playing || self.scratching {
            return self.position_secs;
        }
        let elapsed = (now - self.stamp).clamp(0.0, 0.5);
        (self.position_secs + elapsed * self.rate.max(0.0)).max(0.0)
    }

    /// The tile column under the playhead.
    pub fn head_column(&self) -> f64 {
        self.position_secs * ZOOM_COLS_PER_SEC
    }

    /// The tile column under the playhead at `now`.
    pub fn head_column_at(&self, now: f64) -> f64 {
        self.position_at(now) * ZOOM_COLS_PER_SEC
    }

    /// Beat period in tile columns, and a downbeat column, for the ruling.
    /// The grid is in SOURCE time, which is exactly the tile timebase, so
    /// the rate does not enter here — a tempo-matched deck rules the same
    /// columns, it just crosses them faster.
    pub fn grid_columns(&self) -> Option<(f64, f64)> {
        let grid = self.grid.filter(|grid| grid.has_grid())?;
        let beat_cols = grid.beat_secs * ZOOM_COLS_PER_SEC;
        // Anchor on a downbeat so the bar lines are the heavy ones.
        let first_downbeat_beat = -(grid.downbeat_phase as f64);
        let phase = grid.secs_at_beat(first_downbeat_beat) * ZOOM_COLS_PER_SEC;
        Some((beat_cols, phase))
    }

    /// Bar number at a tile column, for the ruler labels.
    pub fn bar_at_column(&self, column: f64) -> Option<i64> {
        let grid = self.grid.filter(|grid| grid.has_grid())?;
        Some(grid.bar_at(column / ZOOM_COLS_PER_SEC).floor() as i64)
    }
}

/// What the surface reports back to the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaveEvent {
    /// A pointer landed on a lane.
    ScratchStart { deck: DeckId },
    /// Scrub at this rate (1.0 = normal speed forward, negative = back).
    ScratchRate { deck: DeckId, rate: f32 },
    ScratchEnd { deck: DeckId },
    /// Wheel over the lanes: `secs` is the new window width.
    Zoom { secs: f64 },
    /// A display-cadence tick while a hand is on a record. The host answers
    /// with a fresh playhead, so the wave tracks the drag at the display's
    /// rate instead of the console's poll rate.
    Tick,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjWaveScroll {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_lane: DrawWaveLane,
    #[live]
    draw_head: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[rust]
    area: Area,
    #[rust]
    lanes: [WaveLane; 2],
    #[rust(ZOOM_DEFAULT_SECS)]
    zoom_secs: f64,
    #[rust]
    lane_rects: [Rect; 2],
    /// Which lane a pointer is holding, and where/when it was last seen.
    #[rust]
    drag: Option<DragState>,
    #[rust]
    events: Vec<WaveEvent>,
    #[rust]
    next_frame: NextFrame,
    /// Frame-time instrumentation, on when `VJ_DEBUG_FRAMETIME` is set.
    #[rust]
    frame_probe: Option<Box<FrameProbe>>,
}

/// Inter-draw intervals for the wave view, bucketed and reported once a
/// second. This is the number that matters for a scratch: how often the
/// lanes actually reach the screen.
struct FrameProbe {
    last: f64,
    window_start: f64,
    frames: u32,
    /// Frame requests that came back as events — if these run at display
    /// cadence but `frames` does not, the cost is the draw, not the pacing.
    ticks: u32,
    worst: f64,
    /// <8.3 ms (120 Hz), <16.7 (60), <33.3 (30), slower.
    buckets: [u32; 4],
}

impl FrameProbe {
    fn new(now: f64) -> FrameProbe {
        FrameProbe {
            last: now,
            window_start: now,
            frames: 0,
            ticks: 0,
            worst: 0.0,
            buckets: [0; 4],
        }
    }

    fn note(&mut self, cx: &mut Cx, now: f64) -> Option<String> {
        let delta = (now - self.last).max(0.0);
        self.last = now;
        if delta > 0.0 {
            self.frames += 1;
            self.worst = self.worst.max(delta);
            let bucket = if delta < 0.0083 {
                0
            } else if delta < 0.0167 {
                1
            } else if delta < 0.0333 {
                2
            } else {
                3
            };
            self.buckets[bucket] += 1;
        }
        if now - self.window_start < 1.0 {
            return None;
        }
        let report = format!(
            "wave frames {} (ticks {}) in {:.2}s · <8.3ms {} · <16.7ms {} · <33ms {} · slower {} · worst {:.1}ms{}",
            self.frames,
            self.ticks,
            now - self.window_start,
            self.buckets[0],
            self.buckets[1],
            self.buckets[2],
            self.buckets[3],
            self.worst * 1000.0,
            self.cpu_report(cx),
        );
        self.window_start = now;
        self.frames = 0;
        self.ticks = 0;
        self.worst = 0.0;
        self.buckets = [0; 4];
        Some(report)
    }

    /// Where a frame's time went, from the platform's own frame ring: the
    /// widget-tree walk (`cpu`, the whole event dispatch including the draw
    /// event), the Metal pass encode (`enc`), and the wait for a drawable.
    /// A wave frame that costs more than its slice of the refresh period
    /// shows up here as CPU, not as pacing.
    fn cpu_report(&self, cx: &mut Cx) -> String {
        use makepad_widgets::makepad_platform::perf_monitor::{
            PERF_CHANNEL_DRAW, PERF_CHANNEL_DRAWABLE_WAIT, PERF_CHANNEL_EVENT,
        };
        if !cx.perf_monitor.enabled() {
            return String::new();
        }
        let mut frames = Vec::new();
        cx.perf_monitor.read(&mut frames);
        // The ring holds 240 frames; a second at display cadence is the tail.
        let tail = frames.len().saturating_sub(self.frames.max(1) as usize);
        let frames = &frames[tail..];
        let live: Vec<&makepad_widgets::makepad_platform::perf_monitor::PerfMonitorFrame> =
            frames.iter().filter(|f| f.gap_ms > 0.0).collect();
        if live.is_empty() {
            return String::new();
        }
        let mean = |pick: fn(&makepad_widgets::makepad_platform::perf_monitor::PerfMonitorFrame) -> u32| {
            live.iter().map(|f| pick(f) as f64).sum::<f64>() / live.len() as f64 / 1000.0
        };
        let worst = |pick: fn(&makepad_widgets::makepad_platform::perf_monitor::PerfMonitorFrame) -> u32| {
            live.iter().map(|f| pick(f)).max().unwrap_or(0) as f64 / 1000.0
        };
        format!(
            " · cpu {:.1}/{:.1}ms · enc {:.1}/{:.1}ms · wait {:.1}/{:.1}ms",
            mean(|f| f.channel_us[PERF_CHANNEL_EVENT.0]),
            worst(|f| f.channel_us[PERF_CHANNEL_EVENT.0]),
            mean(|f| f.channel_us[PERF_CHANNEL_DRAW.0]),
            worst(|f| f.channel_us[PERF_CHANNEL_DRAW.0]),
            mean(|f| f.channel_us[PERF_CHANNEL_DRAWABLE_WAIT.0]),
            worst(|f| f.channel_us[PERF_CHANNEL_DRAWABLE_WAIT.0]),
        )
    }
}

#[derive(Clone, Copy)]
struct DragState {
    deck: DeckId,
    last_x: f64,
    last_time: f64,
    /// The last rate we published, so an idle pointer decays to a stop.
    idle_since: f64,
}

impl VjWaveScroll {
    pub fn set_lane(&mut self, cx: &mut Cx, deck: DeckId, lane: WaveLane) {
        let stamp = cx.seconds_since_app_start();
        self.lanes[deck.index()] = WaveLane { stamp, ..lane };
        self.area.redraw(cx);
    }

    pub fn lane(&self, deck: DeckId) -> &WaveLane {
        &self.lanes[deck.index()]
    }

    pub fn set_position(
        &mut self,
        cx: &mut Cx,
        deck: DeckId,
        secs: f64,
        playing: bool,
        scratching: bool,
    ) {
        let now = cx.seconds_since_app_start();
        let lane = &mut self.lanes[deck.index()];
        let same = (lane.position_secs - secs).abs() < 1e-6
            && lane.playing == playing
            && lane.scratching == scratching;
        lane.position_secs = secs;
        lane.playing = playing;
        lane.scratching = scratching;
        lane.stamp = now;
        if !same {
            self.area.redraw(cx);
        }
    }

    pub fn set_grid(&mut self, cx: &mut Cx, deck: DeckId, grid: Option<TrackGrid>, rate: f64) {
        let lane = &mut self.lanes[deck.index()];
        lane.grid = grid;
        lane.rate = rate;
        self.area.redraw(cx);
    }

    /// Push the deck's stem knobs into the lane: a layer shrinks as its
    /// knob comes down and vanishes when it is killed.
    pub fn set_stem_gain(&mut self, cx: &mut Cx, deck: DeckId, gains: [f32; 4]) {
        let lane = &mut self.lanes[deck.index()];
        if lane.stem_gain == gains {
            return;
        }
        lane.stem_gain = gains;
        self.area.redraw(cx);
    }

    pub fn zoom_secs(&self) -> f64 {
        self.zoom_secs
    }

    pub fn set_zoom(&mut self, cx: &mut Cx, secs: f64) {
        let secs = secs.clamp(ZOOM_MIN_SECS, ZOOM_MAX_SECS);
        if (secs - self.zoom_secs).abs() > 1e-9 {
            self.zoom_secs = secs;
            self.area.redraw(cx);
        }
    }

    /// Drain what the pointer did since the last call.
    pub fn take_events(&mut self) -> Vec<WaveEvent> {
        std::mem::take(&mut self.events)
    }

    /// One draw happened: bucket the interval and report once a second.
    fn note_frame(&mut self, cx: &mut Cx, now: f64) {
        if self.frame_probe.is_none() {
            if std::env::var("VJ_DEBUG_FRAMETIME").is_err() {
                return;
            }
            // The platform frame ring answers "where did the frame go"; it
            // only collects while something asks for it.
            cx.perf_monitor.set_enabled(true);
            self.frame_probe = Some(Box::new(FrameProbe::new(now)));
            return;
        }
        let report = self
            .frame_probe
            .as_mut()
            .and_then(|probe| probe.note(cx, now));
        if let Some(report) = report {
            log!("{}", report);
        }
    }

    fn lane_at(&self, position: DVec2) -> Option<DeckId> {
        for (index, rect) in self.lane_rects.iter().enumerate() {
            if rect.contains(position) {
                return Some(if index == 0 { DeckId::A } else { DeckId::B });
            }
        }
        None
    }

    /// Pointer velocity in pixels/second becomes a playback rate: dragging
    /// the waveform left runs the track forward, exactly like pushing a
    /// record in the direction it spins.
    fn drag_rate(&self, width: f64, delta_x: f64, delta_secs: f64) -> f32 {
        if delta_secs <= 1e-6 || width <= 1.0 {
            return 0.0;
        }
        let secs_per_px = self.zoom_secs / width;
        (-delta_x * secs_per_px / delta_secs) as f32
    }
}

impl WidgetNode for VjWaveScroll {
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

impl Widget for VjWaveScroll {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // A held-still pointer must stop the record, and no FingerMove
        // arrives to say so — a frame tick does.
        if self.next_frame.is_event(event).is_some() {
            if let Some(probe) = self.frame_probe.as_mut() {
                probe.ticks += 1;
            }
            if let Some(drag) = self.drag {
                let now = cx.seconds_since_app_start();
                if now - drag.idle_since > SCRATCH_IDLE_SECS {
                    self.events.push(WaveEvent::ScratchRate { deck: drag.deck, rate: 0.0 });
                }
            }
            if self.drag.is_some() {
                // Ask for a fresh playhead every frame of the drag.
                self.events.push(WaveEvent::Tick);
            }
            if self.drag.is_some() || self.lanes.iter().any(|lane| lane.playing) {
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
        if let Event::Scroll(scroll) = event {
            if self.area.rect(cx).contains(scroll.abs) {
                let factor = (1.0 + scroll.scroll.y * 0.01).clamp(0.5, 2.0);
                let secs = (self.zoom_secs * factor).clamp(ZOOM_MIN_SECS, ZOOM_MAX_SECS);
                if (secs - self.zoom_secs).abs() > 1e-9 {
                    self.zoom_secs = secs;
                    self.events.push(WaveEvent::Zoom { secs });
                    self.area.redraw(cx);
                }
                scroll.handled_x.set(true);
                scroll.handled_y.set(true);
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let Some(deck) = self.lane_at(fe.abs) else { return };
                let now = cx.seconds_since_app_start();
                self.drag = Some(DragState {
                    deck,
                    last_x: fe.abs.x,
                    last_time: now,
                    idle_since: now,
                });
                self.events.push(WaveEvent::ScratchStart { deck });
                self.next_frame = cx.new_next_frame();
            }
            Hit::FingerMove(fe) => {
                let Some(mut drag) = self.drag else { return };
                let now = cx.seconds_since_app_start();
                let delta_x = fe.abs.x - drag.last_x;
                let delta_secs = now - drag.last_time;
                // Coalesce very small steps: a rate from a sub-millisecond
                // delta is noise, not a gesture.
                if delta_secs < 0.006 && delta_x.abs() < 1.0 {
                    return;
                }
                let width = self.area.rect(cx).size.x;
                let rate = self.drag_rate(width, delta_x, delta_secs);
                drag.last_x = fe.abs.x;
                drag.last_time = now;
                drag.idle_since = now;
                self.drag = Some(drag);
                self.events.push(WaveEvent::ScratchRate { deck: drag.deck, rate });
            }
            Hit::FingerUp(_) => {
                if let Some(drag) = self.drag.take() {
                    self.events.push(WaveEvent::ScratchEnd { deck: drag.deck });
                }
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 8.0 || rect.size.y < 16.0 {
            return DrawStep::done();
        }
        let now = cx.seconds_since_app_start();
        // A playing deck redraws every frame: the waveform scrolls with the
        // music rather than in twenty-hertz steps.
        if self.lanes.iter().any(|lane| lane.playing) {
            self.next_frame = cx.new_next_frame();
        }
        self.note_frame(cx.cx, now);
        // Two lanes with a ruler gutter between them.
        let gutter = 14.0f64;
        let lane_h = ((rect.size.y - gutter) * 0.5).max(8.0);
        let cols_per_px = (self.zoom_secs * ZOOM_COLS_PER_SEC / rect.size.x) as f32;
        for index in 0..2 {
            let y = if index == 0 {
                rect.pos.y
            } else {
                rect.pos.y + lane_h + gutter
            };
            let lane_rect = Rect { pos: dvec2(rect.pos.x, y), size: dvec2(rect.size.x, lane_h) };
            self.lane_rects[index] = lane_rect;
            let lane = &self.lanes[index];
            match lane.pyramid.as_ref() {
                Some(pyramid) => {
                    self.draw_lane.draw_vars.set_texture(0, &pyramid.texture);
                    self.draw_lane.tex_w = pyramid.width.max(1) as f32;
                    self.draw_lane.tex_h = pyramid.height.max(1) as f32;
                    // Pick the pyramid levels this zoom needs and hand the
                    // shader their row offsets: the whole zoom is a uniform.
                    let (lo, lo_scale, hi, hi_scale, blend) =
                        pyramid.levels_for(cols_per_px as f64);
                    self.draw_lane.lo_row = lo.base_row as f32;
                    self.draw_lane.lo_cols = lo.cols.max(1) as f32;
                    self.draw_lane.lo_scale = lo_scale as f32;
                    self.draw_lane.hi_row = hi.base_row as f32;
                    self.draw_lane.hi_cols = hi.cols.max(1) as f32;
                    self.draw_lane.hi_scale = hi_scale as f32;
                    self.draw_lane.lod_blend = blend as f32;
                }
                None => {
                    self.draw_lane.draw_vars.empty_texture(0);
                    self.draw_lane.tex_w = 1.0;
                    self.draw_lane.tex_h = 1.0;
                    self.draw_lane.lod_blend = 0.0;
                }
            }
            match lane.stem_pyramid.as_ref() {
                Some(stems) => {
                    self.draw_lane.draw_vars.set_texture(1, &stems.texture);
                    self.draw_lane.has_stems = 1.0;
                }
                None => {
                    self.draw_lane.draw_vars.empty_texture(1);
                    self.draw_lane.has_stems = 0.0;
                }
            }
            self.draw_lane.color_vocals = stem_color(0);
            self.draw_lane.color_drums = stem_color(1);
            self.draw_lane.color_bass = stem_color(2);
            self.draw_lane.color_other = stem_color(3);
            self.draw_lane.gain_vocals = lane.stem_gain[0];
            self.draw_lane.gain_drums = lane.stem_gain[1];
            self.draw_lane.gain_bass = lane.stem_gain[2];
            self.draw_lane.gain_other = lane.stem_gain[3];
            // The shared playhead is one quad over both lanes.
            self.draw_lane.head_on = 0.0;
            self.draw_lane.cols = lane.cols as f32;
            // Snap the scroll to whole device pixels. A sub-pixel offset
            // makes every column's sample point crawl between neighbours
            // frame to frame, which reads as a shimmer over the whole wave.
            let raw_centre = lane.head_column_at(now);
            let columns_per_pixel = cols_per_px as f64;
            let centre = if columns_per_pixel > 0.0 {
                (raw_centre / columns_per_pixel).round() * columns_per_pixel
            } else {
                raw_centre
            };
            self.draw_lane.centre_col = centre as f32;
            self.draw_lane.cols_per_px = cols_per_px;
            let (beat_cols, phase) = lane.grid_columns().unwrap_or((0.0, 0.0));
            self.draw_lane.beat_cols = beat_cols as f32;
            self.draw_lane.beat_phase = phase as f32;
            self.draw_lane.amp_scale = 1.6;
            self.draw_lane.active = if lane.playing { 1.0 } else { 0.55 };
            self.draw_lane.draw_abs(cx, lane_rect);
        }

        // Bar numbers, ruled off whichever deck is leading the view.
        let ruler = if self.lanes[0].grid.is_some() { 0 } else { 1 };
        let lane = &self.lanes[ruler];
        if let Some((beat_cols, phase)) = lane.grid_columns() {
            let bar_cols = beat_cols * 4.0;
            if bar_cols > 1.0 {
                let centre = lane.head_column_at(now);
                let half_cols = self.zoom_secs * ZOOM_COLS_PER_SEC * 0.5;
                let first = ((centre - half_cols - phase) / bar_cols).floor();
                let last = ((centre + half_cols - phase) / bar_cols).ceil();
                // Thin the labels out when the bars crowd together.
                let px_per_bar = bar_cols / cols_per_px.max(1e-4) as f64;
                let stride = if px_per_bar < 28.0 {
                    (28.0 / px_per_bar).ceil() as i64
                } else {
                    1
                };
                self.draw_text.text_style.font_size = 8.0;
                let mut bar = first as i64;
                while bar <= last as i64 {
                    if bar >= 0 && bar % stride == 0 {
                        let col = phase + bar as f64 * bar_cols;
                        let x = rect.pos.x + rect.size.x * 0.5
                            + (col - centre) / cols_per_px.max(1e-4) as f64;
                        if x >= rect.pos.x && x <= rect.pos.x + rect.size.x - 12.0 {
                            self.draw_text.draw_abs(
                                cx,
                                dvec2(x + 2.0, rect.pos.y + lane_h + 1.0),
                                &format!("{}", bar + 1),
                            );
                        }
                    }
                    bar += 1;
                }
            }
        }

        // An empty lane says so, rather than reading as a dead panel.
        for index in 0..2 {
            if self.lanes[index].cols > 0 {
                continue;
            }
            let lane_rect = self.lane_rects[index];
            self.draw_text.text_style.font_size = 9.0;
            let hint = if index == 0 {
                "deck A — load a track from the list below"
            } else {
                "deck B — load a track from the list below"
            };
            self.draw_text.draw_abs(
                cx,
                dvec2(lane_rect.pos.x + 14.0, lane_rect.pos.y + lane_rect.size.y * 0.5 - 18.0),
                hint,
            );
        }

        // The one shared playhead, straight through both lanes.
        self.draw_head.draw_abs(
            cx,
            Rect {
                pos: dvec2(rect.pos.x + rect.size.x * 0.5 - 6.0, rect.pos.y),
                size: dvec2(12.0, rect.size.y),
            },
        );
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// whole-track overview
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverviewEvent {
    /// Click or drag: seek to this fraction of the track.
    Seek { fraction: f64 },
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjWaveOverview {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The same shader the zoomed lanes use — the whole track in one quad,
    /// at whichever pyramid level fits, with every stem at full weight so
    /// the strip stays the reference picture of the song.
    #[live]
    draw_lane: DrawWaveLane,
    #[rust]
    area: Area,
    #[rust]
    pyramid: Option<WavePyramid>,
    #[rust]
    stem_pyramid: Option<WavePyramid>,
    #[rust]
    cols: usize,
    #[rust]
    head: f64,
    #[rust]
    active: bool,
    #[rust]
    dragging: bool,
    #[rust]
    events: Vec<OverviewEvent>,
}

impl VjWaveOverview {
    pub fn set_track(
        &mut self,
        cx: &mut Cx,
        pyramid: Option<WavePyramid>,
        stem_pyramid: Option<WavePyramid>,
        cols: usize,
    ) {
        self.pyramid = pyramid;
        self.stem_pyramid = stem_pyramid;
        self.cols = cols;
        self.area.redraw(cx);
    }

    pub fn set_head(&mut self, cx: &mut Cx, fraction: f64, active: bool) {
        let fraction = fraction.clamp(0.0, 1.0);
        if (self.head - fraction).abs() < 1e-5 && self.active == active {
            return;
        }
        self.head = fraction;
        self.active = active;
        self.area.redraw(cx);
    }

    pub fn take_events(&mut self) -> Vec<OverviewEvent> {
        std::mem::take(&mut self.events)
    }

    fn seek_at(&mut self, cx: &mut Cx, x: f64) {
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 {
            return;
        }
        let fraction = ((x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0);
        self.events.push(OverviewEvent::Seek { fraction });
    }
}

impl WidgetNode for VjWaveOverview {
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

impl Widget for VjWaveOverview {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.dragging = true;
                self.seek_at(cx, fe.abs.x);
            }
            Hit::FingerMove(fe) => {
                if self.dragging {
                    self.seek_at(cx, fe.abs.x);
                }
            }
            Hit::FingerUp(_) => self.dragging = false,
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 4.0 {
            return DrawStep::done();
        }
        // The whole track across the strip: one column per pixel or coarser,
        // which is exactly what the pyramid's deep levels hold.
        let cols_per_px = (self.cols.max(1) as f64 / rect.size.x).max(0.001);
        match self.pyramid.as_ref() {
            Some(pyramid) => {
                self.draw_lane.draw_vars.set_texture(0, &pyramid.texture);
                self.draw_lane.tex_w = pyramid.width.max(1) as f32;
                self.draw_lane.tex_h = pyramid.height.max(1) as f32;
                let (lo, lo_scale, hi, hi_scale, blend) = pyramid.levels_for(cols_per_px);
                self.draw_lane.lo_row = lo.base_row as f32;
                self.draw_lane.lo_cols = lo.cols.max(1) as f32;
                self.draw_lane.lo_scale = lo_scale as f32;
                self.draw_lane.hi_row = hi.base_row as f32;
                self.draw_lane.hi_cols = hi.cols.max(1) as f32;
                self.draw_lane.hi_scale = hi_scale as f32;
                self.draw_lane.lod_blend = blend as f32;
            }
            None => {
                self.draw_lane.draw_vars.empty_texture(0);
                self.draw_lane.tex_w = 1.0;
                self.draw_lane.tex_h = 1.0;
                self.draw_lane.lod_blend = 0.0;
            }
        }
        match self.stem_pyramid.as_ref() {
            Some(stems) => {
                self.draw_lane.draw_vars.set_texture(1, &stems.texture);
                self.draw_lane.has_stems = 1.0;
            }
            None => {
                self.draw_lane.draw_vars.empty_texture(1);
                self.draw_lane.has_stems = 0.0;
            }
        }
        self.draw_lane.cols = self.cols as f32;
        self.draw_lane.centre_col = (self.cols as f64 * 0.5) as f32;
        self.draw_lane.cols_per_px = cols_per_px as f32;
        // No beat rulings at this scale — the strip is about shape.
        self.draw_lane.beat_cols = 0.0;
        self.draw_lane.beat_phase = 0.0;
        self.draw_lane.amp_scale = 1.6;
        self.draw_lane.active = if self.active { 1.0 } else { 0.7 };
        self.draw_lane.color_vocals = stem_color(0);
        self.draw_lane.color_drums = stem_color(1);
        self.draw_lane.color_bass = stem_color(2);
        self.draw_lane.color_other = stem_color(3);
        // The reference picture: every layer at full weight, whatever the
        // knobs are doing to the mix.
        self.draw_lane.gain_vocals = 1.0;
        self.draw_lane.gain_drums = 1.0;
        self.draw_lane.gain_bass = 1.0;
        self.draw_lane.gain_other = 1.0;
        self.draw_lane.head_col = (self.head * self.cols.max(1) as f64) as f32;
        self.draw_lane.head_on = if self.pyramid.is_some() { 1.0 } else { 0.0 };
        self.draw_lane.draw_abs(cx, rect);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// explorer / queue rows// ---------------------------------------------------------------------------
// explorer / queue rows
// ---------------------------------------------------------------------------

/// What a row refers to: a catalog asset, or a file on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TrackKey {
    Asset(AssetId),
    Local(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackRowEntry {
    pub key: TrackKey,
    pub title: String,
    pub artist: String,
    /// Pre-formatted so the list stays a pure view.
    pub bpm: String,
    pub musical_key: String,
    pub duration: String,
    pub tags: String,
    /// "A", "B", "Q3" — where this track already is.
    pub badge: String,
    /// Highlight: on a deck, or playing.
    pub live: bool,
}

impl TrackRowEntry {
    pub fn blank(key: TrackKey, title: String) -> TrackRowEntry {
        TrackRowEntry {
            key,
            title,
            artist: String::new(),
            bpm: String::new(),
            musical_key: String::new(),
            duration: String::new(),
            tags: String::new(),
            badge: String::new(),
            live: false,
        }
    }
}

/// A click in a track list.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackListHit {
    /// Row body clicked: load it.
    Load(usize),
    /// The row's `+` button: queue it.
    Queue(usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjTrackList {
    #[deref]
    view: View,
    #[rust]
    entries: Vec<TrackRowEntry>,
    /// Queue lists have no `+` button — the rows are already queued.
    #[live]
    show_queue_button: bool,
}

impl VjTrackList {
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<TrackRowEntry>) {
        if self.entries != entries {
            self.entries = entries;
            self.view.redraw(cx);
        }
    }

    pub fn entry_at(&self, index: usize) -> Option<&TrackRowEntry> {
        self.entries.get(index)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Widget for VjTrackList {
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
                    let item = list.item(cx, row_id, id!(TrackEmpty));
                    item.draw_all(cx, &mut Scope::empty());
                }
                continue;
            }
            list.set_item_range(cx, 0, self.entries.len());
            while let Some(row_id) = list.next_visible_item(cx) {
                if row_id >= self.entries.len() {
                    continue;
                }
                let mut item = list.item(cx, row_id, id!(TrackRow));
                if let Some(entry) = self.entries.get(row_id) {
                    item.label(cx, ids!(row_badge)).set_text(cx, &entry.badge);
                    item.label(cx, ids!(row_title)).set_text(cx, &entry.title);
                    item.label(cx, ids!(row_artist)).set_text(cx, &entry.artist);
                    item.label(cx, ids!(row_bpm)).set_text(cx, &entry.bpm);
                    item.label(cx, ids!(row_key)).set_text(cx, &entry.musical_key);
                    item.label(cx, ids!(row_time)).set_text(cx, &entry.duration);
                    item.label(cx, ids!(row_tags)).set_text(cx, &entry.tags);
                    item.button(cx, ids!(row_queue))
                        .set_visible(cx, self.show_queue_button);
                    let live = if entry.live { 1.0f32 } else { 0.0 };
                    let odd = if row_id % 2 == 1 { 1.0f32 } else { 0.0 };
                    script_apply_eval!(cx, item, {
                        draw_bg +: { live: #(live) odd: #(odd) }
                    });
                }
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

/// Read row clicks out of a frame's actions, the same way the tile grids do.
pub fn track_list_hits(
    ui: &WidgetRef,
    cx: &mut Cx,
    path: &[LiveId],
    actions: &Actions,
) -> Vec<TrackListHit> {
    let widget = ui.widget(cx, path);
    let len = widget.borrow::<VjTrackList>().map(|list| list.len()).unwrap_or(0);
    if len == 0 {
        return Vec::new();
    }
    let list = widget.portal_list(cx, ids!(list));
    let mut hits: Vec<TrackListHit> = Vec::new();
    for (row_id, item) in list.items_with_actions(actions) {
        if row_id >= len {
            continue;
        }
        // A recycled row can surface the same press more than once; one
        // press is one load, whatever the list reports.
        let hit = if item.button(cx, ids!(row_queue)).clicked(actions) {
            TrackListHit::Queue(row_id)
        } else if item.view(cx, ids!(row_body)).finger_down(actions).is_some() {
            TrackListHit::Load(row_id)
        } else {
            continue;
        };
        if !hits.contains(&hit) {
            hits.push(hit);
        }
    }
    // And one press is one ROW: never two decks from one click.
    hits.truncate(1);
    hits
}

// ---------------------------------------------------------------------------
// formatting helpers shared with the host
// ---------------------------------------------------------------------------

/// `m:ss`, or `—` for an unknown length.
pub fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "—".to_string();
    }
    let total = secs.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// A deck's tempo readout: the grid's BPM scaled by the playback rate.
pub fn format_bpm(grid: Option<TrackGrid>, rate: f64) -> String {
    match grid.filter(|grid| grid.has_grid()) {
        Some(grid) => format!("{:.1}", grid.effective_bpm(rate)),
        None => "---.-".to_string(),
    }
}

/// Pitch slider readout, signed percent.
pub fn format_pitch(pitch: f64) -> String {
    format!("{:+.1}%", pitch * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(bpm: f64, first: f64, downbeat_phase: u32) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs: first,
            downbeat_phase,
            confidence: 1.0,
        }
    }

    fn lane(bpm: f64, position: f64) -> WaveLane {
        WaveLane {
            grid: Some(grid(bpm, 0.25, 0)),
            position_secs: position,
            rate: 1.0,
            cols: 100_000,
            loaded: true,
            ..WaveLane::default()
        }
    }

    #[test]
    fn the_playhead_column_follows_source_time() {
        let lane = lane(120.0, 3.0);
        assert!((lane.head_column() - 300.0).abs() < 1e-9, "100 columns a second");
    }

    #[test]
    fn the_grid_rules_beats_and_anchors_on_a_downbeat() {
        let lane = lane(120.0, 0.0);
        let (beat_cols, phase) = lane.grid_columns().expect("a grid");
        assert!((beat_cols - 50.0).abs() < 1e-9, "0.5 s a beat = 50 columns");
        // With downbeat_phase 0 the first beat IS a downbeat.
        assert!((phase - 25.0).abs() < 1e-9, "first beat at 0.25 s = column 25");

        // A shifted downbeat moves the anchor back by that many beats.
        let mut shifted = lane;
        shifted.grid = Some(grid(120.0, 0.25, 1));
        let (_, phase) = shifted.grid_columns().expect("a grid");
        assert!((phase - (-25.0)).abs() < 1e-9, "anchor {phase}");
        // The anchor is still ON the beat network.
        let beats = (phase - 25.0) / 50.0;
        assert!((beats - beats.round()).abs() < 1e-9);
    }

    #[test]
    fn two_synced_decks_rule_their_bars_in_the_same_place() {
        // Deck A at 120, deck B at 100 played 1.2x = 120: after a phase
        // align their bar lines must land on the same screen column.
        let a = lane(120.0, 8.25);
        let mut b = WaveLane {
            grid: Some(grid(100.0, 0.0, 0)),
            rate: 1.2,
            position_secs: 0.0,
            ..WaveLane::default()
        };
        // Put B where its phase matches A's.
        let a_grid = a.grid.unwrap();
        let b_grid = b.grid.unwrap();
        let a_bar_phase = a_grid.bar_at(a.position_secs).rem_euclid(1.0);
        b.position_secs = b_grid.secs_at_beat((12.0 + a_bar_phase * 4.0) - 0.0);

        // Distance from each playhead to the previous bar line, in seconds
        // of AUDIBLE time (source seconds / rate) must be equal.
        let a_bars = a_grid.bar_at(a.position_secs);
        let b_bars = b_grid.bar_at(b.position_secs);
        assert!(
            (a_bars.rem_euclid(1.0) - b_bars.rem_euclid(1.0)).abs() < 1e-9,
            "bar phase {} vs {}",
            a_bars.rem_euclid(1.0),
            b_bars.rem_euclid(1.0)
        );
    }

    #[test]
    fn no_grid_means_no_ruling() {
        let mut lane = lane(120.0, 1.0);
        lane.grid = None;
        assert!(lane.grid_columns().is_none());
        lane.grid = Some(TrackGrid::default());
        assert!(lane.grid_columns().is_none(), "an empty grid rules nothing");
        assert!(lane.bar_at_column(100.0).is_none());
    }

    #[test]
    fn bar_numbers_count_from_the_downbeat() {
        let lane = lane(120.0, 0.0);
        // Bar 0 starts at the first beat (0.25 s = column 25).
        assert_eq!(lane.bar_at_column(25.0), Some(0));
        // Four beats later is bar 1 (0.25 + 2.0 s = column 225).
        assert_eq!(lane.bar_at_column(225.0), Some(1));
        assert_eq!(lane.bar_at_column(224.0), Some(0));
    }

    #[test]
    fn a_playing_lane_carries_its_playhead_between_host_updates() {
        // The host samples the device clock a few times a second; the lane
        // has to fill in the frames between, or the scroll steps.
        let mut lane = lane(120.0, 10.0);
        lane.stamp = 100.0;
        lane.playing = true;
        assert!((lane.position_at(100.0) - 10.0).abs() < 1e-9);
        assert!((lane.position_at(100.25) - 10.25).abs() < 1e-9);
        // A tempo-matched deck moves through its source faster.
        lane.rate = 1.08;
        assert!((lane.position_at(100.5) - (10.0 + 0.54)).abs() < 1e-9);
        // A stopped lane sits exactly where it was put.
        lane.playing = false;
        assert!((lane.position_at(200.0) - 10.0).abs() < 1e-9);
        // And a very stale stamp cannot run the playhead away.
        lane.playing = true;
        assert!(lane.position_at(1_000.0) - 10.0 <= 0.55);
    }

    #[test]
    fn an_empty_lane_has_nothing_to_rule_or_scroll() {
        let lane = WaveLane::default();
        assert_eq!(lane.cols, 0);
        assert!(lane.grid_columns().is_none());
        assert!((lane.head_column_at(123.0)).abs() < 1e-9);
    }

    #[test]
    fn readouts_format_for_the_deck_header() {
        assert_eq!(format_duration(0.0), "—");
        assert_eq!(format_duration(65.4), "1:05");
        assert_eq!(format_duration(3_599.0), "59:59");
        assert_eq!(format_bpm(None, 1.0), "---.-");
        assert_eq!(format_bpm(Some(grid(128.0, 0.0, 0)), 1.0), "128.0");
        assert_eq!(format_bpm(Some(grid(100.0, 0.0, 0)), 1.04), "104.0");
        assert_eq!(format_pitch(0.0), "+0.0%");
        assert_eq!(format_pitch(-0.032), "-3.2%");
    }

    #[test]
    fn the_pyramid_halves_each_level_and_stacks_them() {
        // The layout maths the shader inverts, checked without a GPU.
        let cols = TILE_TEX_WIDTH * 3 + 7;
        let width = TILE_TEX_WIDTH;
        let mut levels: Vec<WaveLevel> = Vec::new();
        let mut height = 0usize;
        let mut level_cols = cols;
        while levels.len() < MAX_WAVE_LEVELS {
            levels.push(WaveLevel { base_row: height, cols: level_cols });
            height += level_cols.div_ceil(width);
            if level_cols <= 1 {
                break;
            }
            level_cols = level_cols.div_ceil(2);
        }
        assert_eq!(levels[0].base_row, 0);
        assert_eq!(levels[0].cols, cols);
        assert_eq!(levels[1].cols, cols.div_ceil(2));
        // Every level starts after the one below it, and none overlap.
        for pair in levels.windows(2) {
            let rows = pair[0].cols.div_ceil(width);
            assert_eq!(pair[1].base_row, pair[0].base_row + rows);
        }
        // The whole stack costs about twice the finest level.
        assert!(height < cols.div_ceil(width) * 2 + MAX_WAVE_LEVELS);
    }

    #[test]
    fn the_shader_gets_the_level_that_cannot_alias() {
        let levels: Vec<WaveLevel> = (0..8)
            .map(|index| WaveLevel { base_row: index * 4, cols: 4096 >> index })
            .collect();
        // One column per pixel or finer: the finest level, no blend.
        let (lo, lo_scale, _hi, _hi_scale, blend) = levels_for(&levels, 0.4);
        assert_eq!(lo.base_row, 0);
        assert_eq!(lo_scale, 1.0);
        assert_eq!(blend, 0.0);
        // Four columns per pixel: level 2, exactly (no blend needed).
        let (lo, lo_scale, _hi, _, blend) = levels_for(&levels, 4.0);
        assert_eq!(lo_scale, 4.0);
        assert_eq!(lo.cols, 1024);
        assert!(blend.abs() < 1e-9);
        // Between levels: blend proportionally, so a zoom never pops.
        let (_, lo_scale, _, hi_scale, blend) = levels_for(&levels, 6.0);
        assert_eq!((lo_scale, hi_scale), (4.0, 8.0));
        assert!((blend - (6.0f64.log2() - 2.0)).abs() < 1e-9);
        // Past the top: clamp to the coarsest, never index off the end.
        let (lo, _, hi, _, blend) = levels_for(&levels, 1_000_000.0);
        assert_eq!(lo.base_row, hi.base_row);
        assert_eq!(blend, 0.0);
        // No pyramid at all is answered, not panicked on.
        let (lo, _, _, _, _) = levels_for(&[], 4.0);
        assert_eq!(lo.cols, 0);
    }

    #[test]
    fn tiles_pack_into_a_wrapped_texture_shape() {
        // The packing maths the shader inverts: index -> (row, column).
        let count = TILE_TEX_WIDTH * 3 + 7;
        let width = TILE_TEX_WIDTH.min(count.max(1));
        let height = count.div_ceil(width);
        assert_eq!(width, TILE_TEX_WIDTH);
        assert_eq!(height, 4);
        for index in [0usize, 1, TILE_TEX_WIDTH - 1, TILE_TEX_WIDTH, count - 1] {
            let row = index / width;
            let column = index - row * width;
            assert!(row < height && column < width, "index {index} out of the texture");
        }
        // A short track uses one row and no more texture than it needs.
        let short = 300;
        assert_eq!(TILE_TEX_WIDTH.min(short), short);
        assert_eq!(short.div_ceil(short), 1);
    }
}
