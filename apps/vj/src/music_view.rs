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
use std::sync::Arc;

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
/// How much of the half-lane the loudest column of a track fills. Mirrored
/// by the `0.78` in `DrawWaveLane::pixel`: a column at the track's own
/// reference level draws this tall, and nothing draws taller.
pub const WAVE_ENVELOPE: f32 = 0.78;
/// One entry of [`STEM_COLORS`] as a shader colour.
pub fn stem_color(stem: usize) -> Vec4f {
    let c = STEM_COLORS[stem.min(STEM_COLORS.len() - 1)];
    vec4(c[0], c[1], c[2], c[3])
}

/// Push the stem palette into the wave-lane shader's four colour uniforms.
fn set_stem_color_uniforms(lane: &mut DrawWaveLane, cx: &Cx2d) {
    for (id, stem) in [
        (live_id!(color_vocals), 0),
        (live_id!(color_drums), 1),
        (live_id!(color_bass), 2),
        (live_id!(color_other), 3),
    ] {
        let c = stem_color(stem);
        lane.draw_vars.set_uniform(cx, id, &[c.x, c.y, c.z, c.w]);
    }
}

/// Push the loop band's span, in tile columns, into the lane shader.
/// `end <= start` is how the shader is told there is no loop, so a lane
/// without one sends zeroes rather than skipping the write — the uniform
/// is shared, and a stale span would otherwise paint the wrong lane.
fn set_loop_span_uniform(lane: &mut DrawWaveLane, cx: &Cx2d, span: Option<(f64, f64)>) {
    let (start, end) = match span {
        Some((start, end)) if end > start => (start as f32, end as f32),
        _ => (0.0, 0.0),
    };
    lane.draw_vars.set_uniform(cx, live_id!(loop_span), &[start, end, 0.0, 0.0]);
}

/// The drag preview band, same encoding and the same every-draw rule: the
/// zoomed lanes push zeroes so an overview drag cannot bleed onto them.
fn set_preview_span_uniform(lane: &mut DrawWaveLane, cx: &Cx2d, span: Option<(f64, f64)>) {
    let (start, end) = match span {
        Some((start, end)) if end > start => (start as f32, end as f32),
        _ => (0.0, 0.0),
    };
    lane.draw_vars.set_uniform(cx, live_id!(preview_span), &[start, end, 0.0, 0.0]);
}

/// What a click in the marker strip at the top of the overview means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkerHit {
    /// The green marker: keep the running loop as a blue one.
    Save,
    /// A blue marker: go into that saved loop again.
    Recall(usize),
    /// The red marker: drag to move where CUE lands.
    Cue,
    /// A yellow marker on the BOTTOM edge: a scanner-found loop.
    Found(usize),
}

/// Resolve a click at `secs` against the markers. Blue wins over green on
/// overlap — recalling a saved loop is the deliberate act; saving it again
/// would be a no-op anyway. Nearest within `tol` takes it.
fn marker_hit(
    saved: &[(f64, f64)],
    running_in: Option<f64>,
    cue_secs: f64,
    secs: f64,
    tol: f64,
) -> Option<MarkerHit> {
    let nearest_saved = saved
        .iter()
        .enumerate()
        .map(|(index, span)| (index, (span.0 - secs).abs()))
        .filter(|(_, distance)| *distance <= tol)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((index, _)) = nearest_saved {
        return Some(MarkerHit::Recall(index));
    }
    match running_in {
        Some(start) if (start - secs).abs() <= tol => Some(MarkerHit::Save),
        _ if (cue_secs - secs).abs() <= tol => Some(MarkerHit::Cue),
        _ => None,
    }
}

/// Resolve a click in the strip's BOTTOM band against the scanner's yellow
/// markers. Nearest IN within `tol` takes it; the bands never compete —
/// blue and red live on the top edge, yellow on the bottom.
fn found_marker_hit(found: &[(f64, f64)], secs: f64, tol: f64) -> Option<MarkerHit> {
    found
        .iter()
        .enumerate()
        .map(|(index, span)| (index, (span.0 - secs).abs()))
        .filter(|(_, distance)| *distance <= tol)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| MarkerHit::Found(index))
}

/// How far outside the loop band, in PIXELS, a grab still counts as
/// grabbing it. A one-beat loop is under two pixels on a whole-track
/// strip, so without some forgiveness the band would be uncatchable at
/// exactly the sizes the loop cutter produces. Pixels rather than seconds
/// so the forgiveness is the same size under the finger on a three-minute
/// edit and on a ten-minute one.
const BAND_GRAB_PX: f64 = 5.0;

/// The top band of the strip where marker chips live and clicks mean
/// marker business rather than seeks or loop drags.
const MARKER_STRIP_PX: f64 = 14.0;
/// Horizontal forgiveness for a marker click, pixels.
const MARKER_GRAB_PX: f64 = 6.0;
/// How far a blue marker must be dragged from home before letting go
/// DELETES it instead of recalling it.
const MARKER_DELETE_PX: f64 = 50.0;

/// Where inside the loop band `secs` landed, or `None` if it did not. The
/// offset is what makes a drag feel pinned: the band travels with the
/// finger instead of snapping its in point under the cursor.
fn band_grab(span: Option<(f64, f64)>, secs: f64, tolerance_secs: f64) -> Option<f64> {
    let (start, end) = span?;
    if secs < start - tolerance_secs || secs > end + tolerance_secs {
        return None;
    }
    Some((secs - start).clamp(0.0, end - start))
}

/// Where a drag to `raw_start` would land: the SNAPPED span the commit
/// will produce, or `None` when it will not fit. The reference is the
/// GHOST's in point — the live loop, which does not move during a drag —
/// so this is the same arithmetic on the same inputs as the engine's
/// commit, and the preview cannot disagree with what release does.
fn move_preview(
    span: Option<(f64, f64)>,
    raw_start: f64,
    grid: Option<TrackGrid>,
    unit_beats: u32,
    duration: f64,
) -> Option<(f64, f64)> {
    let (start, end) = span?;
    let len = end - start;
    let snapped = match grid {
        Some(grid) => grid.snap_translate(raw_start, start, unit_beats),
        None => raw_start,
    };
    if snapped < 0.0 || snapped + len > duration {
        return None;
    }
    Some((snapped, snapped + len))
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
        // The running loop, in the app-wide accent. Low alpha: a wash the
        // waveform stays readable through, not a fill that replaces it.
        color_loop: uniform(#xff5c3924)
        // The loop's span as (start_col, end_col, _, _). A UNIFORM and not
        // two `#[live]` fields: those become per-instance VERTEX INPUTS,
        // and this lane already sits on the vs_5_0 limit of 32 — two more
        // attributes fail the shader compile outright (X4506). Nothing
        // here varies per instance anyway.
        loop_span: uniform(#x00000000)
        // A loop drag's would-be landing, same encoding as `loop_span`.
        // Drawn dimmer beside the ghost so the operator sees both where
        // the loop IS and where release will put it.
        preview_span: uniform(#x00000000)
        color_head: uniform(#xf4f7fa)
        // The stem palette, pushed from STEM_COLORS every draw so the
        // waveform and the knobs cannot disagree. Uniforms, not instances:
        // they are per-draw constants, and as instances they blew the
        // D3D11 vs_5_0 32-input limit.
        color_vocals: uniform(#fff)
        color_drums: uniform(#fff)
        color_bass: uniform(#fff)
        color_other: uniform(#fff)

        // One column of one pyramid level. Each level is stored as its own
        // block of rows in the same texture, so a level is a row offset and
        // a column count — hand-encoded mips. `xyz` are the bands, `w` is
        // the column's level against the whole track: its height.
        level_at: fn(column: float, base_row: float, level_cols: float, scale: float) -> vec4 {
            let c = clamp(floor(column / scale), 0.0, max(level_cols - 1.0, 0.0))
            let wrap = floor(c / self.tex_w)
            let u = (c - wrap * self.tex_w + 0.5) / self.tex_w
            let v = (base_row + wrap + 0.5) / self.tex_h
            return self.tiles.sample_as_bgra(vec2(u, v))
        }

        // What this PIXEL covers, alias-free at any zoom. Each level is a
        // max-reduction of the one below, so a transient never disappears
        // as the view pulls back; the two levels either side of the current
        // scale are blended so zooming does not pop.
        tile_span: fn(column: float) -> vec4 {
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

        // The running loop: a wash across the span, and a hard rule on each
        // edge. The edges are the seam — where the sound actually jumps —
        // so they are drawn as firmly as a bar ruling rather than fading
        // out with the wash.
        loop_at: fn(column: float) -> float {
            if self.loop_span.y <= self.loop_span.x {
                return 0.0
            }
            let inside = step(self.loop_span.x, column) * step(column, self.loop_span.y)
            let de = min(abs(column - self.loop_span.x), abs(column - self.loop_span.y))
            let edge = 1.0 - smoothstep(0.5, 1.8, de / max(self.cols_per_px, 0.0001))
            return max(inside * self.color_loop.w, edge)
        }

        // A drag's would-be landing: the same band at reduced weight, so
        // the ghost (the loop still playing) stays the louder of the two.
        preview_at: fn(column: float) -> float {
            if self.preview_span.y <= self.preview_span.x {
                return 0.0
            }
            let inside = step(self.preview_span.x, column) * step(column, self.preview_span.y)
            let de = min(abs(column - self.preview_span.x), abs(column - self.preview_span.y))
            let edge = 1.0 - smoothstep(0.5, 1.8, de / max(self.cols_per_px, 0.0001))
            return max(inside * self.color_loop.w, edge * 0.7) * 0.6
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
            let t = self.tile_span(column)
            // THE HEIGHT OF A COLUMN IS HOW LOUD THE TRACK IS THERE. The
            // level channel was normalized once, against the whole track,
            // when the tiles were built; nothing here may raise it. A quiet
            // intro draws short and a drop draws tall, in the grey region
            // and in the separated one alike, so the seam between them is
            // invisible in height and only the colouring changes.
            let level = clamp(t.w, 0.0, 1.0) * 0.78

            // A column the separator has reached is coloured by WHAT it is;
            // one it has not is a single honest grey. Both are the same
            // mirrored, layered envelope, so the picture only gains meaning
            // as the separation catches up — it never jumps.
            let raw = self.stem_span(column)
            let present = raw.x + raw.y + raw.z + raw.w
            let separated = step(0.004, present) * self.has_stems

            // The stems PARTITION that height in proportion to what each
            // one contributes to the column — they never scale it up. A
            // killed stem takes its share away with it, so the shape of the
            // wave is the shape of what the deck will play; the overview
            // strip passes ones here and stays the reference picture.
            let inverse = 1.0 / max(present, 0.0001)
            let grey_h = level
            let s_bass = level * raw.z * inverse * self.gain_bass
            let s_drums = level * raw.y * inverse * self.gain_drums
            let s_vocals = level * raw.x * inverse * self.gain_vocals
            let s_other = level * raw.w * inverse * self.gain_other

            let e0 = mix(grey_h, s_bass, separated)
            let e1 = mix(grey_h, s_bass + s_drums, separated)
            let e2 = mix(grey_h, s_bass + s_drums + s_vocals, separated)
            let e3 = mix(grey_h, s_bass + s_drums + s_vocals + s_other, separated)

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
            // what is coming, and that reads brighter. WHERE the playhead
            // is depends on the surface: the zoomed lanes scroll so it sits
            // at the window centre, but the overview strip fits the whole
            // track and carries its own head — using centre_col there dimmed
            // a fixed left half of the song whatever was playing.
            let played_ref = mix(self.centre_col, self.head_col, self.head_on)
            let played = step(column, played_ref)
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
            // The loop sits over the picture, under the playhead: you have
            // to be able to see the band through a loud passage.
            let la = max(self.loop_at(column), self.preview_at(column))
            let banded = ruled.mix(vec4(self.color_loop.x, self.color_loop.y, self.color_loop.z, 1.0), la)
            let hd = abs(column - self.head_col) / max(self.cols_per_px, 0.0001)
            let ha = (1.0 - smoothstep(0.5, 1.8, hd)) * self.head_on
            return banded.mix(self.color_head, ha)
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
        // The marker chips: FCP's rounded flag, green while it is the
        // running loop's handle, blue once saved.
        draw_marker_live +: {
            color: uniform(#x35c05f)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let split = h * 0.58
                sdf.move_to(0.5, 0.5)
                sdf.line_to(w - 0.5, 0.5)
                sdf.line_to(w - 0.5, split)
                sdf.line_to(w * 0.5, h - 0.5)
                sdf.line_to(0.5, split)
                sdf.close_path()
                sdf.fill_keep(self.color)
                sdf.stroke(#x00000066, 1.0)
                return sdf.result
            }
        }
        draw_marker_cue +: {
            color: uniform(#xe5484d)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let split = h * 0.58
                sdf.move_to(0.5, 0.5)
                sdf.line_to(w - 0.5, 0.5)
                sdf.line_to(w - 0.5, split)
                sdf.line_to(w * 0.5, h - 0.5)
                sdf.line_to(0.5, split)
                sdf.close_path()
                sdf.fill_keep(self.color)
                sdf.stroke(#x00000066, 1.0)
                return sdf.result
            }
        }
        draw_marker_cue_hot +: {
            color: uniform(#xf2f6fa)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let split = h * 0.58
                sdf.move_to(0.5, 0.5)
                sdf.line_to(w - 0.5, 0.5)
                sdf.line_to(w - 0.5, split)
                sdf.line_to(w * 0.5, h - 0.5)
                sdf.line_to(0.5, split)
                sdf.close_path()
                sdf.fill_keep(self.color)
                sdf.stroke(#x00000066, 1.0)
                return sdf.result
            }
        }
        draw_marker_cue_ghost +: {
            color: uniform(#xe5484d55)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let split = h * 0.58
                sdf.move_to(0.5, 0.5)
                sdf.line_to(w - 0.5, 0.5)
                sdf.line_to(w - 0.5, split)
                sdf.line_to(w * 0.5, h - 0.5)
                sdf.line_to(0.5, split)
                sdf.close_path()
                sdf.fill_keep(self.color)
                sdf.stroke(#x00000033, 1.0)
                return sdf.result
            }
        }
        draw_marker_saved +: {
            color: uniform(#x3d8bff)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let split = h * 0.58
                sdf.move_to(0.5, 0.5)
                sdf.line_to(w - 0.5, 0.5)
                sdf.line_to(w - 0.5, split)
                sdf.line_to(w * 0.5, h - 0.5)
                sdf.line_to(0.5, split)
                sdf.close_path()
                sdf.fill_keep(self.color)
                sdf.stroke(#x00000066, 1.0)
                return sdf.result
            }
        }
        // The span lines wear their chips' own colours: green for the
        // running loop, blue for a saved one, yellow for a find.
        draw_edge_live +: { color: #x35c05f }
        draw_edge_saved +: { color: #x3d8bff }
        draw_edge_found +: { color: #xf5c542 }
        draw_marker_found +: {
            color: uniform(#xf5c542)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let split = h * 0.42
                sdf.move_to(w * 0.5, 0.5)
                sdf.line_to(w - 0.5, split)
                sdf.line_to(w - 0.5, h - 0.5)
                sdf.line_to(0.5, h - 0.5)
                sdf.line_to(0.5, split)
                sdf.close_path()
                sdf.fill_keep(self.color)
                sdf.stroke(#x00000066, 1.0)
                return sdf.result
            }
        }
    }

    // ---- the lyrics reader (shared widget, VJ name kept) -------------------
    mod.widgets.VjLyricReader = set_type_default() do mod.widgets.LyricReader{}

    // ---- explorer / queue rows --------------------------------------------
    let TrackText = Label{
        flow: Flow.Right{wrap: false}
        max_lines: 1
        draw_text.color: #xd6dee6
        draw_text.text_style.font_size: 9
    }

    mod.widgets.VjWrapStripBase = #(VjWrapStrip::register_widget(vm))
    mod.widgets.VjWrapStrip = set_type_default() do mod.widgets.VjWrapStripBase{
        width: Fill
        height: Fit
    }

    // The track row's inner strip, shared by the plain row template and
    // the inline-player one — hoisted so the two can never drift apart.
    let TrackRowBody = View{
        width: Fill
        height: 22
        flow: Right
        spacing: 6
        padding: Inset{left: 6.0 right: 6.0 top: 0.0 bottom: 0.0}
        align: Align{x: 0.0, y: 0.5}
        cursor: MouseCursor.Hand
        row_badge := Label{
            width: 26
            text: ""
            draw_text.color: #xff5c39
            draw_text.text_style: theme.font_bold{font_size: 8}
        }
        row_title := TrackText{width: Fill{weight: 400. min: 180.}}
        row_artist := TrackText{width: Fill{max: 150.} draw_text.color: #x9fabb7}
        row_bpm := TrackText{
            width: 54
            draw_text.color: #xff5c39
            draw_text.text_style: theme.font_bold{font_size: 9}
        }
        row_key := TrackText{width: 40 draw_text.color: #xc6a0f0}
        row_time := TrackText{width: 52 draw_text.color: #x9fabb7}
        // The processed marks: a green tick under STEM when the
        // store holds this track's four stems, under KRK when it
        // holds the word-aligned transcript.
        row_stem := TrackText{width: 36 draw_text.color: #x35c05f}
        row_krk := TrackText{width: 30 draw_text.color: #x35c05f}
        row_tags := TrackText{width: Fill{max: 190.} draw_text.color: #x6f7b87}
        // Headphone pre-listen: green while this row is the one in
        // the phones. Painted per row from the host's active key.
        // ButtonIcon, not Button: an icon-only button carries no label and
        // no label spacing, which is what centres the glyph in the well.
        row_hp := ButtonIcon{
            width: 22
            height: 18
            padding: 0
            align: Align{x: 0.5, y: 0.5}
            icon_walk: Walk{width: 10 height: Fit}
            draw_bg +: {
                color: #x272e38
                color_hover: #x2b3440
                color_down: #x1e232b
                border_color: #xffffff26
                border_radius: 4.0
                border_size: 1.0
            }
            draw_icon +: {
                svg: crate_resource("self:resources/icons/headphones.svg")
                color: #x9fabb7
            }
        }
        row_queue := Button{
            width: 26
            height: 18
            text: "+"
            // A 26x18 chip: the theme's default button padding sat
            // the glyph off-centre.
            padding: 0
            align: Align{x: 0.5, y: 0.5}
            draw_bg +: {
                color: #x272e38
                color_hover: #x2b3440
                color_down: #x1e232b
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

    // ---- the headphone pre-listen player ----
    // The seek strip: the decoded track's peaks as amber bins (the mockup's
    // tape), a playhead line, press-or-drag to jump. Cue-bus territory, so
    // its accents stay in the phones green/amber family, never the
    // program's orange.
    mod.widgets.VjPhonesWaveBase = #(VjPhonesWave::register_widget(vm))
    mod.widgets.VjPhonesWave = set_type_default() do mod.widgets.VjPhonesWaveBase{
        width: Fill
        height: 34
        draw_bg +: {
            // Clearly DARKER than the player card behind it: an invisible
            // well gives the eye no container, and a waveform with no
            // visible room around it reads as one cut off at the edges.
            color: #x05070a
        }
        draw_bin +: {
            color: #xe8a33d
        }
        draw_head +: {
            color: #xf2f6fa
        }
    }

    // One player, three homes (docked / inline / floating): the host fills
    // whichever instance the placement preference points at.
    mod.widgets.VjPhonesPlayer = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: 4
        padding: Inset{left: 8.0 right: 8.0 top: 6.0 bottom: 8.0}
        draw_bg +: {
            color: #x16161b
            border_color: #x35c05f55
            border_size: 1.0
            border_radius: 6.0
        }
        View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 6
            align: Align{x: 0.0, y: 0.5}
            hp_play := ButtonIcon{
                visible: false
                width: 24
                height: 20
                padding: 0
                align: Align{x: 0.5, y: 0.5}
                icon_walk: Walk{width: 9 height: Fit}
                draw_bg +: {
                    color: #x272e38
                    color_hover: #x2b3440
                    color_down: #x1e232b
                    border_color: #xffffff26
                    border_radius: 4.0
                    border_size: 1.0
                }
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/play.svg")
                    color: #xd6dee6
                }
            }
            hp_pause := ButtonIcon{
                width: 24
                height: 20
                padding: 0
                align: Align{x: 0.5, y: 0.5}
                icon_walk: Walk{width: 9 height: Fit}
                draw_bg +: {
                    color: #x272e38
                    color_hover: #x2b3440
                    color_down: #x1e232b
                    border_color: #xffffff26
                    border_radius: 4.0
                    border_size: 1.0
                }
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/pause.svg")
                    color: #xd6dee6
                }
            }
            // The title clips here and scrolls as a ticker when it does
            // not fit — the host advances the margin while playing.
            hp_title_clip := View{
                width: Fill{min: 64.}
                height: Fit
                flow: Right
                clip_x: true
                hp_title := Label{
                    width: Fit
                    text: ""
                    draw_text.color: #xe8eef4
                    draw_text.text_style: theme.font_bold{font_size: 9}
                }
            }
            hp_time := Label{
                width: Fit
                text: ""
                draw_text.color: #x9fabb7
                draw_text.text_style.font_size: 9
            }
            // What the pre-listen is FOR: the verdict. A and B send the
            // track to a deck, + puts it at the back of the set — and +
            // is absent once the track is already in the queue, because a
            // control that cannot do anything should not ask to be
            // pressed.
            hp_load_a := Button{
                width: 18
                height: 20
                padding: 0
                text: "A"
                align: Align{x: 0.5, y: 0.5}
                draw_bg +: {
                    color: #x272e38
                    color_hover: #x2b3440
                    color_down: #x1e232b
                    border_color: #xffffff26
                    border_radius: 4.0
                    border_size: 1.0
                }
                draw_text +: {
                    color: #xff5c39
                    text_style: theme.font_bold{font_size: 9}
                }
            }
            hp_load_b := Button{
                width: 18
                height: 20
                padding: 0
                text: "B"
                align: Align{x: 0.5, y: 0.5}
                draw_bg +: {
                    color: #x272e38
                    color_hover: #x2b3440
                    color_down: #x1e232b
                    border_color: #xffffff26
                    border_radius: 4.0
                    border_size: 1.0
                }
                draw_text +: {
                    color: #x5aa9ff
                    text_style: theme.font_bold{font_size: 9}
                }
            }
            hp_queue := Button{
                width: 18
                height: 20
                padding: 0
                text: "+"
                align: Align{x: 0.5, y: 0.5}
                draw_bg +: {
                    color: #x272e38
                    color_hover: #x2b3440
                    color_down: #x1e232b
                    border_color: #xffffff26
                    border_radius: 4.0
                    border_size: 1.0
                }
                draw_text +: {
                    color: #xd6dee6
                    text_style: theme.font_bold{font_size: 10}
                }
            }
            hp_close := Button{
                width: 20
                height: 20
                padding: 0
                text: "×"
                align: Align{x: 0.5, y: 0.5}
                draw_bg +: {
                    color: #x272e38
                    color_hover: #x2b3440
                    color_down: #x1e232b
                    border_color: #xffffff26
                    border_radius: 4.0
                    border_size: 1.0
                }
                draw_text +: {
                    color: #xd6dee6
                    text_style: theme.font_bold{font_size: 10}
                }
            }
        }
        hp_seek := mod.widgets.VjPhonesWave{}
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
                    color: #x1c2129
                    color_alt: #x11161c
                    color_live: #x1d2a2a
                    // A picked row, for the hand that is about to drag it.
                    color_sel: #x2c3a4e
                    // The row that IS in the hand: the ghost's own accent.
                    color_carry: #xff5c39
                    live: instance(0.0)
                    odd: instance(0.0)
                    sel: instance(0.0)
                    carry: instance(0.0)
                    border_radius: 3.0
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        // Inset by the outline's half width: a stroke on
                        // the row's own edge would spill half of itself
                        // onto the neighbour above and below.
                        sdf.box(
                            0.75,
                            0.75,
                            self.rect_size.x - 1.5,
                            self.rect_size.y - 1.5,
                            self.border_radius
                        )
                        sdf.fill_keep(self.color
                            .mix(self.color_alt, self.odd)
                            .mix(self.color_live, self.live)
                            .mix(self.color_sel, self.sel)
                            .mix(self.color_sel, self.carry))
                        // The carried row wears the outline. The order
                        // rearranges live under the pointer, so this one
                        // mark answers both questions at once: what is in
                        // the hand, and where letting go would leave it.
                        sdf.stroke(
                            vec4(
                                self.color_carry.x,
                                self.color_carry.y,
                                self.color_carry.z,
                                self.carry
                            ),
                            1.5
                        )
                        return sdf.result
                    }
                }
                row_body := TrackRowBody{height: Fill}
            }
            // The previewing row when the player preference says INLINE:
            // the same body with the player unfolded beneath it.
            TrackRowPlayer := RoundedView{
                width: Fill
                height: Fit
                padding: 0
                flow: Down
                spacing: 0
                draw_bg +: {
                    color: #x1d2a2a
                    border_radius: 3.0
                }
                row_body := TrackRowBody{}
                View{
                    width: Fill
                    height: Fit
                    padding: Inset{left: 6.0 right: 6.0 top: 0.0 bottom: 4.0}
                    row_player := mod.widgets.VjPhonesPlayer{
                        draw_bg +: {
                            color: #x10161a
                            border_color: #x35c05f33
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


    // A column header that sorts. Same ink and the same box as the label it
    // replaces, so the row reads as headings rather than a strip of buttons —
    // the arrow is what says a column is holding the order.
    let MusicColHead = Button{
        height: Fit
        padding: 0
        margin: 0
        align: Align{x: 0.0, y: 0.5}
        flow: Flow.Right{wrap: false}
        draw_bg +: {
            color: #x00000000
            color_focus: #x00000000
            color_hover: #x00000000
            color_down: #x00000000
            border_size: 0.0
            border_radius: 0.0
        }
        draw_text +: {
            color: #xa6b1bd
            color_focus: #xa6b1bd
            color_hover: #xd6dee6
            color_down: #x8e9aa7
            text_style: theme.font_bold{font_size: 8}
        }
    }

    let MusicValue = Label{
        flow: Flow.Right{wrap: false}
        max_lines: 1
        draw_text.color: #xe8eef4
        draw_text.text_style.font_size: 11
    }

    let MusicButton = Button{
        draw_bg +: {
            color: #x272e38
            color_focus: #x272e38
            color_hover: #x2b3440
            color_down: #x1e232b
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
            color: #x272e38
            color_focus: #x272e38
            color_hover: #x2b3440
            color_down: #x1e232b
            border_color: #xffffff26
            border_radius: 5.0
            border_size: 1.0
        }
        draw_icon +: {
            color: #xd6dee6
        }
    }

    // An accordion chevron: bare, quiet, and the height of the heading it
    // sits beside. It says which way the block will go, and nothing else.
    let ChevronIcon = ButtonIcon{
        width: 16
        height: 13
        icon_walk: Walk{width: 10 height: Fit}
        draw_bg +: {
            color: #x00000000
            color_focus: #x00000000
            color_hover: #xffffff14
            color_down: #xffffff1f
            border_color: #x00000000
            border_size: 0.0
            border_radius: 3.0
        }
        draw_icon +: { color: #x8e9aa7 }
    }

    // A bare mode icon: no chrome at rest, so a row of them reads as marks
    // rather than as four more buttons competing with the tabs beside them.
    // The state lives in the MARK — accent when in force, muted when not —
    // because an SVG has one colour and no states of its own. Only hover
    // puts anything behind it, which is what keeps them findable.
    let ModeIcon = ButtonIcon{
        width: 26
        height: 22
        icon_walk: Walk{width: 14 height: Fit}
        draw_bg +: {
            color: #x00000000
            color_focus: #x00000000
            color_hover: #xffffff14
            color_down: #xffffff1f
            border_color: #x00000000
            border_size: 0.0
            border_radius: 5.0
        }
        draw_icon +: { color: #x5f6a76 }
    }

    // A library-row chip: icon first, then its word. Fit width, so when the
    // console narrows and `App::sync_library_density` takes the word away,
    // the chip closes up around its icon and the row gets those pixels back.
    // The radius is half the height: labelled it reads as a pill, bare as a
    // round icon key.
    let MusicChipButton = Button{
        width: Fit
        height: 22
        padding: Inset{left: 6.0 right: 6.0 top: 0.0 bottom: 0.0}
        spacing: 5.0
        icon_walk: Walk{width: 10 height: Fit}
        draw_bg +: {
            color: #x272e38
            color_focus: #x272e38
            color_hover: #x2b3440
            color_down: #x1e232b
            border_color: #xffffff2e
            border_radius: 11.0
            border_size: 1.0
        }
        draw_text +: {
            color: #xd6dee6
            color_focus: #xd6dee6
            color_hover: #xfffaf4
            text_style: theme.font_bold{font_size: 9}
        }
        draw_icon +: {
            color: #xd6dee6
        }
    }

    // Half of a lane's M/S pair — the console idiom KILL grew into: mute
    // this lane, or solo it against the rest of its bus. Stem lanes and
    // EQ bands alike; the host paints them hot through paint_lit.
    let MSButton = MusicButton{
        width: Fill
        height: 13
        padding: 0
        align: Align{x: 0.5, y: 0.5}
        draw_text +: {
            text_style: theme.font_bold{font_size: 7}
        }
        draw_bg +: {
            border_radius: 3.0
        }
    }
    let MSRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: 2
    }

    let MusicKnob = Rotary{
        width: 42
        height: 42
        min: 0.0
        max: 2.0
        default: 1.0
        scroll_step: 0.025
        text: ""
        flow: Down
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x1c222b)
            body_color_hover: uniform(#x2a323d)
            rim_color: uniform(#xffffff40)
            ring_color: uniform(#x2f3842)
            val_color: uniform(#xff5c39)
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
    // A flat Button rather than a Label so a click on it resets its knob.
    let KnobLabel = Button{
        width: Fill
        height: Fit
        padding: 0
        margin: 0
        align: Align{x: 0.5, y: 0.0}
        draw_bg +: {
            color: #x00000000
            color_focus: #x00000000
            color_hover: #x00000000
            color_down: #x00000000
            border_size: 0.0
            border_radius: 0.0
        }
        draw_text +: {
            color: #xa6b1bd
            color_focus: #xa6b1bd
            color_hover: #xd6dee6
            color_down: #x8e9aa7
            text_style: theme.font_bold{font_size: 7}
        }
    }

    // The key-shift readout, beside the BPM it transposes. A flat Button
    // rather than a Label because it is also the way home: clicking the
    // number drops the deck back to the track's own key, the same
    // click-the-legend-to-reset move the knob labels use.
    let KeyReadout = Button{
        width: 34
        height: Fit
        padding: 0
        margin: 0
        align: Align{x: 0.5, y: 0.5}
        draw_bg +: {
            color: #x00000000
            color_focus: #x00000000
            color_hover: #x00000000
            color_down: #x00000000
            border_size: 0.0
            border_radius: 0.0
        }
        draw_text +: {
            color: #xc6a0f0
            color_focus: #xc6a0f0
            color_hover: #xe2ccff
            color_down: #xa27fc9
            text_style: theme.font_bold{font_size: 11}
        }
    }

    // A deck's one-line status readouts (grid, stems/lyrics). These are
    // REAL labels: the host writes them through LabelRef, which silently
    // no-ops on anything Button-shaped — the KnobLabel rebase to Button
    // took them along by accident and killed both lines.
    let StatusLabel = MusicLabel{
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
        scroll_step: 0.025
        width: 40
        height: Fill
        text: ""
        flow: Down
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x1d222a)
            track_color: uniform(#x2b343f)
            fill_color: uniform(#xff5c39)
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
        scroll_step: 0.025
        text: ""
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x1d222a)
            track_color: uniform(#x2b343f)
            fill_color: uniform(#xff5c39)
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
            color: uniform(#x1d222a)
            color_lit: uniform(#xff5c39)
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
            // y 0.5: the QUANT cluster is shorter than the heads flanking
            // it — centering seats it on the readout line, per the mockup.
            align: Align{x: 0.0, y: 0.5}
            new_batch: true
            deck_a_head := RoundedView{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                align: Align{x: 0.0, y: 0.5}
                // Invisible until a file is dragged over it: this half of
                // the header is deck A's drop target, and the border is
                // how it says so.
                draw_bg +: {
                    color: #x00000000
                    border_color: #x00000000
                    border_size: 1.0
                    border_radius: 8.0
                }
                Label{
                    text: "A"
                    draw_text.color: #xff5c39
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
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 17}
                    }
                    deck_a_pitch_text := MusicLabel{text: "+0.0%"}
                }
                deck_a_key := KeyReadout{text: "—"}
                deck_a_time := MusicLabel{width: 78 text: "0:00 / 0:00"}
            }
            // QUANT, not SNAP: an immediate, phase-preserving jump —
            // Traktor's word for exactly this. SNAP stays reserved for
            // placement rounding, which this deliberately is not. It sits
            // at the console's center line, between the two decks it
            // gates equally.
            View{
                width: Fit
                height: Fit
                flow: Right
                spacing: 4
                margin: Inset{left: 10, right: 10}
                align: Align{x: 0.5, y: 0.5}
                MusicLabel{width: 40 text: "QUANT"}
                music_snap := VjBeatsDrop{width: 34}
            }
            deck_b_head := RoundedView{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                align: Align{x: 0.0, y: 0.5}
                // Invisible until a file is dragged over it: this half of
                // the header is deck B's drop target, and the border is
                // how it says so.
                draw_bg +: {
                    color: #x00000000
                    border_color: #x00000000
                    border_size: 1.0
                    border_radius: 8.0
                }
                deck_b_time := MusicLabel{width: 78 text: "0:00 / 0:00"}
                deck_b_key := KeyReadout{text: "—"}
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
            deck_a_well := DeckWell{
                width: Fill
                deck_a_overview := mod.widgets.VjWaveOverview{height: Fill}
            }
            deck_b_well := DeckWell{
                width: Fill
                deck_b_overview := mod.widgets.VjWaveOverview{height: Fill}
            }
        }


        // The console proper: the decks over the two lists.
        //
        // Down while the window has the height for it. On a WIDE, SHORT
        // window — a console squeezed against the bottom of the screen —
        // `App::sync_page_body_flow` turns this row-wise instead, and the
        // lists stand to the right of deck B. The room a short window is
        // missing is vertical; the room it has going spare is horizontal,
        // so the lists take the room that actually exists.
        page_body := View{
            width: Fill
            height: Fill
            flow: Down
            spacing: 6
            // ---- deck region: knobs | lanes + transport | knobs ----
            // Three columns, each as tall as the region. The MIDDLE one carries
            // the zoomed lanes ABOVE the transport strip, which is what makes the
            // console responsive: when the strip wraps to two or three rows, the
            // lanes give up exactly that height and nothing else does — the knob
            // and karaoke columns either side keep their layout, and the
            // explorer/queue below never moves. Floor: lanes + strip >= 330,
            // the old 300px karaoke floor plus a one-row strip.
            deck_region := View{
                // The decks take whatever the lists column leaves.
                width: Fill
                // The single-field constrained form is the ONLY one this DSL
                // provably applies (multi-field literals parse to nothing;
                // measured).
                height: Fill{min: 330.}
                flow: Right
                spacing: 8

                deck_a_panel := View{
                    width: 316
                    height: Fill
                    flow: Down
                    spacing: 5
                    new_batch: true
                    // The console tabs. On a narrow console the deck panels come one
                    // at a time; on a narrower one still the MIXER joins them, and
                    // then the three take the width in turn.
                    //
                    // One strip per thing a tab can show, because the strip has to be
                    // inside whatever is on screen. They all say the same thing.
                    deck_a_tab_strip := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0, y: 0.5}
                        deck_a_tab_0 := MusicButton{width: 62 height: 22 text: "deck A"}
                        deck_a_tab_1 := MusicButton{width: 62 height: 22 text: "deck B"}
                        // Only once the mixer is a tab as well.
                        deck_a_tab_2 := MusicButton{visible: false width: 56 height: 22 text: "mixer"}
                        View{width: Fill height: 1}
                        Tip{
                            text: "Manual — only you change what is on screen"
                            deck_a_mode_manual := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/hand.svg") }
                            }
                        }
                        Tip{
                            text: "Follow the load target — the tab aims the library too"
                            deck_a_mode_target := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/reticle.svg") }
                            }
                        }
                        Tip{
                            text: "Follow what is audible — moves during a mix"
                            deck_a_mode_audible := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/levels.svg") }
                            }
                        }
                        Tip{
                            text: "Follow the load target, but a tab press holds it"
                            deck_a_mode_pinned := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/pin.svg") }
                            }
                        }
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 4
                        align: Align{x: 0.0, y: 0.5}
                        deck_a_sync := MusicButton{width: Fill height: 22 text: "SYNC"}
                        deck_a_keylock := MusicButton{width: 44 height: 22 text: "KEY"}
                        // The key steps in whole semitones, so it steps: a fader
                        // with twelve detents a side would be a worse way to ask
                        // for the same number. The readout is up in the header,
                        // beside the BPM the key belongs to.
                        deck_a_key_down := MusicButton{width: 22 height: 22 padding: 0 align: Align{x: 0.5, y: 0.5} text: "-"}
                        deck_a_key_up := MusicButton{width: 22 height: 22 padding: 0 align: Align{x: 0.5, y: 0.5} text: "+"}
                        deck_a_range := MusicButton{width: 46 height: 22 text: "±8%"}
                    }
                    View{
                        width: Fill
                        height: Fill
                        flow: Right
                        spacing: 8
                        View{
                            // Fit, not a number: pitch 44, volume 44, the meter
                            // 10 and two 6pt gaps come to 110, and the 104 this
                            // used to claim was paid for by whichever child came
                            // last — deck A's meter squeezed to 4, deck B's pitch
                            // column to 38, clipping the 0 under it. Fit cannot
                            // fall out of step with its own children.
                            width: Fit
                            height: Fill
                            flow: Right
                            spacing: 6
                            View{
                                width: 44
                                height: Fill
                                flow: Down
                                spacing: 2
                                align: Align{x: 0.5, y: 0.0}
                                MusicLabel{text: "TEMPO"}
                                deck_a_pitch := MusicFader{min: -1.0 max: 1.0 default: 0.0}
                                deck_a_pitch_reset := MusicButton{width: Fill height: 14 padding: 0 align: Align{x: 0.5, y: 0.5} text: "0"}
                            }
                            View{
                                width: 44
                                height: Fill
                                flow: Down
                                spacing: 2
                                align: Align{x: 0.5, y: 0.0}
                                MusicLabel{text: "VOL"}
                                deck_a_gain := MusicFader{min: 0.0 max: 1.5 default: 1.0}
                                deck_a_mute := MusicButton{width: Fill height: 14 padding: 0 align: Align{x: 0.5, y: 0.5} text: "M"}
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
                            spacing: 2
                            deck_a_eq_head := View{
                                // Only on a console short enough to fold; a tall
                                // one wears the panel exactly as it always did.
                                visible: false
                                width: Fill
                                height: Fit
                                flow: Right
                                align: Align{x: 0.0, y: 0.0}
                                deck_a_eq_title := KnobLabel{
                                    text: "EQUALIZER"
                                    align: Align{x: 0.0, y: 0.0}
                                    draw_text +: {
                                        text_style: theme.font_bold{font_size: 8}
                                        color_hover: #xffffff
                                        color_down: #xffffff
                                    }
                                }
                                View{width: Fill height: 1}
                                // The chevron, drawn rather than typed: the small triangle
                                // glyphs are not in this font and came out as boxes. Two marks
                                // with one shown, never one mark with its svg swapped — that
                                // drops the loaded document and leaves a white silhouette.
                                deck_a_eq_chev_up := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_up.svg") }
                                }
                                deck_a_eq_chev_down := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_down.svg") }
                                }
                            }
                            deck_a_eq_body := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 3
                                KnobStack{
                                    deck_a_label_eq_high := KnobLabel{text: "HIGH"}
                                    deck_a_eq_high := MusicKnob{}
                                    MSRow{
                                        deck_a_kill_high := MSButton{text: "M"}
                                        deck_a_soloband_high := MSButton{text: "S"}
                                    }
                                }
                                KnobStack{
                                    deck_a_label_eq_mid := KnobLabel{text: "MID"}
                                    deck_a_eq_mid := MusicKnob{}
                                    MSRow{
                                        deck_a_kill_mid := MSButton{text: "M"}
                                        deck_a_soloband_mid := MSButton{text: "S"}
                                    }
                                }
                                KnobStack{
                                    deck_a_label_eq_low := KnobLabel{text: "LOW"}
                                    deck_a_eq_low := MusicKnob{}
                                    MSRow{
                                        deck_a_kill_low := MSButton{text: "M"}
                                        deck_a_soloband_low := MSButton{text: "S"}
                                    }
                                }
                                KnobStack{
                                    deck_a_label_filter := KnobLabel{text: "FILTER"}
                                    deck_a_filter := MusicKnob{min: 0.0 max: 1.0 default: 0.5}
                                }
                            }
                            deck_a_stems_head := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                align: Align{x: 0.0, y: 0.0}
                                deck_a_stem_mix := KnobLabel{
                                    text: "STEM MIX"
                                    // KnobLabel centres for knob legends; this one
                                    // is a section header and reads left, the way
                                    // the plain label it replaced did. The margin
                                    // drops the whole stems block — header, knobs,
                                    // M/S — clear of the EQ row above it.
                                    margin: Inset{left: 0.0 right: 0.0 top: 8.0 bottom: 0.0}
                                    align: Align{x: 0.0, y: 0.0}
                                    draw_text +: {
                                        text_style: theme.font_bold{font_size: 8}
                                        // The resting ink is painted per state —
                                        // green live, red off; the hand always gets
                                        // white, so hover means "this is a switch"
                                        // rather than a second state to read.
                                        color_hover: #xffffff
                                        color_down: #xffffff
                                    }
                                }
                                View{width: Fill height: 1}
                                // The chevron, drawn rather than typed: the small triangle
                                // glyphs are not in this font and came out as boxes. Two marks
                                // with one shown, never one mark with its svg swapped — that
                                // drops the loaded document and leaves a white silhouette.
                                deck_a_stems_chev_up := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_up.svg") }
                                }
                                deck_a_stems_chev_down := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_down.svg") }
                                }
                            }
                            deck_a_stems_body := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 3
                                StemStack{
                                    deck_a_label_drums := KnobLabel{text: "DRUMS"}
                                    deck_a_stem_drums := StemKnob{}
                                    MSRow{
                                        deck_a_kill_drums := MSButton{text: "M"}
                                        deck_a_solo_drums := MSButton{text: "S"}
                                    }
                                }
                                StemStack{
                                    deck_a_label_bass := KnobLabel{text: "BASS"}
                                    deck_a_stem_bass := StemKnob{}
                                    MSRow{
                                        deck_a_kill_bass := MSButton{text: "M"}
                                        deck_a_solo_bass := MSButton{text: "S"}
                                    }
                                }
                                StemStack{
                                    deck_a_label_vocals := KnobLabel{text: "VOCALS"}
                                    deck_a_stem_vocals := StemKnob{}
                                    MSRow{
                                        deck_a_kill_vocals := MSButton{text: "M"}
                                        deck_a_solo_vocals := MSButton{text: "S"}
                                    }
                                }
                                StemStack{
                                    deck_a_label_other := KnobLabel{text: "OTHER"}
                                    deck_a_stem_other := StemKnob{}
                                    MSRow{
                                        deck_a_kill_other := MSButton{text: "M"}
                                        deck_a_solo_other := MSButton{text: "S"}
                                    }
                                }
                            }
                            // StatusLabel, not KnobLabel: these two lines are
                            // set_text targets, and LabelRef::set_text on a
                            // Button is a silent no-op (the autopilot branch
                            // caught it). Empty and collapsed while idle.
                            deck_a_stem_state := StatusLabel{text: ""}
                            deck_a_grid_state := StatusLabel{text: ""}
                            // A section header that is also the switch — the
                            // twin of STEM MIX above: resting ink painted per
                            // state (green live, yellow/grey cached, red off),
                            // white under the hand. Same drop as STEM MIX, a
                            // shade less: the status lines above it collapse
                            // when silent, so this margin IS the resting gap.
                            deck_a_kar_head := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                align: Align{x: 0.0, y: 0.0}
                                deck_a_kar_title := KnobLabel{
                                    text: "KARAOKE"
                                    margin: Inset{left: 0.0 right: 0.0 top: 6.0 bottom: 0.0}
                                    align: Align{x: 0.0, y: 0.0}
                                    draw_text +: {
                                        text_style: theme.font_bold{font_size: 8}
                                        color_hover: #xffffff
                                        color_down: #xffffff
                                    }
                                }
                                View{width: Fill height: 1}
                                // The chevron, drawn rather than typed: the small triangle
                                // glyphs are not in this font and came out as boxes. Two marks
                                // with one shown, never one mark with its svg swapped — that
                                // drops the loaded document and leaves a white silhouette.
                                deck_a_kar_chev_up := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_up.svg") }
                                }
                                deck_a_kar_chev_down := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_down.svg") }
                                }
                            }
                            // The transcript, filling the column down to the
                            // transport: the reading copy AND the timing proof.
                            deck_a_lyrics := mod.widgets.VjLyricReader{height: Fill}
                        }
                    }
                    // The deck's own transport, at the foot of its column. These
                    // rows do not move when the strip beside them wraps: the
                    // extra rows are paid for by the waveform lanes, not by this
                    // column's knobs and karaoke.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 3
                        align: Align{x: 0.0, y: 0.5}
                        deck_a_play := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") }
                        }
                        deck_a_cue := MusicButton{width: 40 height: 24 text: "CUE"}
                        // Headphone cue: latch this deck onto the phones bus.
                        // Green when live — monitoring, never program.
                        deck_a_hp := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/headphones.svg") }
                        }
                        deck_a_loop := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/loop_one.svg") }
                        }
                        deck_a_loop_halve := MusicButton{width: 22 height: 24 text: "<"}
                        deck_a_loop_len := VjBeatsDrop{width: 24 loop_rows: true draw_bg +: {arrow: 0.0}}
                        deck_a_loop_double := MusicButton{width: 22 height: 24 text: ">"}
                        // The CDJ's loop pair, in glyphs that read as the marks
                        // they set: `[` in, `]` out. The loop icon left of the
                        // stepper is RELOOP/EXIT; the sparkle past them opens the
                        // scanner, which is also where marks go to be forgotten.
                        deck_a_loop_in := MusicButton{width: 22 height: 24 text: "["}
                        deck_a_loop_out := MusicButton{width: 22 height: 24 text: "]"}
                        deck_a_loop_scan := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/sparkle.svg") }
                        }
                    }
                }

                // The lanes and the transport strip are ONE column: the strip is
                // Fit and the lanes take everything it leaves, so a strip that
                // wraps to two or three rows shortens the WAVEFORMS. The knob
                // columns either side keep their layout, and the library below
                // never moves.
                deck_lanes := View{
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 6
                    // The console tabs. On a narrow console the deck panels come one
                    // at a time; on a narrower one still the MIXER joins them, and
                    // then the three take the width in turn.
                    //
                    // One strip per thing a tab can show, because the strip has to be
                    // inside whatever is on screen. They all say the same thing.
                    mixer_tab_strip := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0, y: 0.5}
                        mixer_tab_0 := MusicButton{width: 62 height: 22 text: "deck A"}
                        mixer_tab_1 := MusicButton{width: 62 height: 22 text: "deck B"}
                        // Only once the mixer is a tab as well.
                        mixer_tab_2 := MusicButton{visible: false width: 56 height: 22 text: "mixer"}
                        View{width: Fill height: 1}
                        Tip{
                            text: "Manual — only you change what is on screen"
                            mixer_mode_manual := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/hand.svg") }
                            }
                        }
                        Tip{
                            text: "Follow the load target — the tab aims the library too"
                            mixer_mode_target := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/reticle.svg") }
                            }
                        }
                        Tip{
                            text: "Follow what is audible — moves during a mix"
                            mixer_mode_audible := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/levels.svg") }
                            }
                        }
                        Tip{
                            text: "Follow the load target, but a tab press holds it"
                            mixer_mode_pinned := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/pin.svg") }
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
                    // Fade shaping, the crossfader and the automation. The strip
                    // widget decides the ORDER per frame: flanking the sweep when
                    // everything fits one line, sweep-first when it does not. It
                    // MEASURES the groups rather than carrying their widths here,
                    // so a restyled control cannot put the numbers out of date.
                    xfade_strip := mod.widgets.VjWrapStrip{
                        width: Fill
                        height: Fit
                        flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                        spacing: 6
                        wrap_spacing: 6
                        align: Align{x: 0.5, y: 0.5}
                        strip_shaping := View{
                            width: Fit
                            height: Fit
                            flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                            spacing: 6
                            wrap_spacing: 6
                            align: Align{x: 0.5, y: 0.5}
                            // How long a fade takes — the number the two buttons
                            // beside it spend.
                            xfade_secs := Slider{
                                width: 118
                                margin: 0
                                text: "duration"
                                min: 0.05
                                max: 20.0
                                default: 4.0
                            }
                            // FADE walks the console to the other side over that
                            // duration; CUT jumps there. Both land on the
                            // other deck, whichever side that currently is.
                            xfade_now := MusicButton{width: 46 height: 22 text: "FADE"}
                            xfade_switch := MusicButton{width: 40 height: 22 text: "CUT"}
                            // Eight gain laws, each row wearing a plot of itself.
                            xcurve := mod.widgets.VjCurveDrop{}
                            // Tone follows the fader too when this is lit: the deck
                            // on its way out loses its bass, so two kicks never
                            // stack in the middle of a blend.
                            music_eqfade := MusicButton{width: 34 height: 22 text: "EQ"}
                        }
                        // The sweep and its two cue keys are three children, not
                        // one: the strip flanks the sweep with them while there
                        // is room for both, and drops them to a line of their own
                        // when flanking would leave the sweep too short to play.
                        fader_cue_a := View{
                            width: Fit
                            height: Fit
                            flow: Right
                            spacing: 6
                            align: Align{x: 0.0, y: 0.5}
                            fade_to_a := MusicButton{width: 46 height: 22 text: "◀ A"}
                            xfade_label_a := MusicLabel{width: 12 text: "A"}
                        }
                        // Fit here is only a fallback: the strip always draws this
                        // one with an explicit width, and the sweep inside fills
                        // whatever that comes to.
                        fader_sweep := View{
                            width: Fit
                            height: Fit
                            flow: Right
                            align: Align{x: 0.5, y: 0.5}
                            // margin: 0 — the themed Slider's mspace margin is
                            // dead width here, and the sweep should own every
                            // pixel the row does not spend on its cue keys.
                            xfader := CrossFader{margin: 0}
                        }
                        fader_cue_b := View{
                            width: Fit
                            height: Fit
                            flow: Right
                            spacing: 6
                            align: Align{x: 0.0, y: 0.5}
                            xfade_label_b := MusicLabel{width: 12 text: "B"}
                            fade_to_b := MusicButton{width: 46 height: 22 text: "B ▶"}
                        }
                        strip_automation := View{
                            width: Fit
                            height: Fit
                            flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                            spacing: 6
                            wrap_spacing: 6
                            align: Align{x: 0.5, y: 0.5}
                            // Widths are cut close here on purpose: SWAP,
                            // NORMALISE, AUTO SYNC, AUTO DJ and its gear are one
                            // thought and belong on one line. The five together
                            // plus their gaps have to stay inside the strip's
                            // 290pt budget or the row wraps and reads as two
                            // unrelated groups.
                            decks_swap := MusicButton{width: 46 height: 22 text: "SWAP"}
                            // Level-matching: a quiet master comes up, a hot one
                            // comes down, and the faders still read what the hand
                            // set. The ECG waveform says levelling without a word.
                            Tip{
                                text: "NORMALIZER"
                                music_normalise := MusicIconButton{
                                    width: 34
                                    height: 22
                                    draw_icon +: { svg: crate_resource("self:resources/icons/waveform.svg") }
                                }
                            }
                            auto_sync := MusicButton{width: 80 height: 22 text: "AUTO SYNC"}
                            // The AUTO DJ latch wears its own status line, the way
                            // the SYNC button wears its mode — one control, one
                            // home — and the gear that configures it never leaves
                            // its side, so the two wrap together.
                            View{
                                width: Fit
                                height: Fit
                                flow: Right
                                spacing: 6
                                align: Align{x: 0.0, y: 0.5}
                                auto_dj := MusicButton{width: 74 height: 22 text: "AUTO DJ"}
                                auto_cfg := MusicIconButton{
                                    width: 26 height: 22
                                    draw_icon +: { svg: crate_resource("self:resources/icons/gear.svg") }
                                }
                            }
                        }
                    }
                }

                deck_b_panel := View{
                    width: 316
                    height: Fill
                    flow: Down
                    spacing: 5
                    new_batch: true
                    // The console tabs. On a narrow console the deck panels come one
                    // at a time; on a narrower one still the MIXER joins them, and
                    // then the three take the width in turn.
                    //
                    // One strip per thing a tab can show, because the strip has to be
                    // inside whatever is on screen. They all say the same thing.
                    deck_b_tab_strip := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0, y: 0.5}
                        deck_b_tab_0 := MusicButton{width: 62 height: 22 text: "deck A"}
                        deck_b_tab_1 := MusicButton{width: 62 height: 22 text: "deck B"}
                        // Only once the mixer is a tab as well.
                        deck_b_tab_2 := MusicButton{visible: false width: 56 height: 22 text: "mixer"}
                        View{width: Fill height: 1}
                        Tip{
                            text: "Manual — only you change what is on screen"
                            deck_b_mode_manual := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/hand.svg") }
                            }
                        }
                        Tip{
                            text: "Follow the load target — the tab aims the library too"
                            deck_b_mode_target := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/reticle.svg") }
                            }
                        }
                        Tip{
                            text: "Follow what is audible — moves during a mix"
                            deck_b_mode_audible := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/levels.svg") }
                            }
                        }
                        Tip{
                            text: "Follow the load target, but a tab press holds it"
                            deck_b_mode_pinned := ModeIcon{
                                draw_icon +: { svg: crate_resource("self:resources/icons/pin.svg") }
                            }
                        }
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 4
                        align: Align{x: 0.0, y: 0.5}
                        deck_b_range := MusicButton{width: 46 height: 22 text: "±8%"}
                        // Mirrored against deck A: + then −, reading outward from
                        // the console's centre line.
                        deck_b_key_up := MusicButton{width: 22 height: 22 padding: 0 align: Align{x: 0.5, y: 0.5} text: "+"}
                        deck_b_key_down := MusicButton{width: 22 height: 22 padding: 0 align: Align{x: 0.5, y: 0.5} text: "-"}
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
                            spacing: 2
                            deck_b_eq_head := View{
                                // Only on a console short enough to fold; a tall
                                // one wears the panel exactly as it always did.
                                visible: false
                                width: Fill
                                height: Fit
                                flow: Right
                                align: Align{x: 0.0, y: 0.0}
                                deck_b_eq_title := KnobLabel{
                                    text: "EQUALIZER"
                                    align: Align{x: 0.0, y: 0.0}
                                    draw_text +: {
                                        text_style: theme.font_bold{font_size: 8}
                                        color_hover: #xffffff
                                        color_down: #xffffff
                                    }
                                }
                                View{width: Fill height: 1}
                                // The chevron, drawn rather than typed: the small triangle
                                // glyphs are not in this font and came out as boxes. Two marks
                                // with one shown, never one mark with its svg swapped — that
                                // drops the loaded document and leaves a white silhouette.
                                deck_b_eq_chev_up := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_up.svg") }
                                }
                                deck_b_eq_chev_down := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_down.svg") }
                                }
                            }
                            deck_b_eq_body := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 3
                                KnobStack{
                                    deck_b_label_filter := KnobLabel{text: "FILTER"}
                                    deck_b_filter := MusicKnob{min: 0.0 max: 1.0 default: 0.5}
                                }
                                KnobStack{
                                    deck_b_label_eq_low := KnobLabel{text: "LOW"}
                                    deck_b_eq_low := MusicKnob{}
                                    MSRow{
                                        deck_b_kill_low := MSButton{text: "M"}
                                        deck_b_soloband_low := MSButton{text: "S"}
                                    }
                                }
                                KnobStack{
                                    deck_b_label_eq_mid := KnobLabel{text: "MID"}
                                    deck_b_eq_mid := MusicKnob{}
                                    MSRow{
                                        deck_b_kill_mid := MSButton{text: "M"}
                                        deck_b_soloband_mid := MSButton{text: "S"}
                                    }
                                }
                                KnobStack{
                                    deck_b_label_eq_high := KnobLabel{text: "HIGH"}
                                    deck_b_eq_high := MusicKnob{}
                                    MSRow{
                                        deck_b_kill_high := MSButton{text: "M"}
                                        deck_b_soloband_high := MSButton{text: "S"}
                                    }
                                }
                            }
                            deck_b_stems_head := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                align: Align{x: 0.0, y: 0.0}
                                deck_b_stem_mix := KnobLabel{
                                    text: "STEM MIX"
                                    // KnobLabel centres for knob legends; this one
                                    // is a section header and reads left, the way
                                    // the plain label it replaced did. Margin as on
                                    // deck A: the stems block drops clear of the EQ.
                                    margin: Inset{left: 0.0 right: 0.0 top: 8.0 bottom: 0.0}
                                    align: Align{x: 0.0, y: 0.0}
                                    draw_text +: {
                                        text_style: theme.font_bold{font_size: 8}
                                        // The resting ink is painted per state —
                                        // green live, red off; the hand always gets
                                        // white, so hover means "this is a switch"
                                        // rather than a second state to read.
                                        color_hover: #xffffff
                                        color_down: #xffffff
                                    }
                                }
                                View{width: Fill height: 1}
                                // The chevron, drawn rather than typed: the small triangle
                                // glyphs are not in this font and came out as boxes. Two marks
                                // with one shown, never one mark with its svg swapped — that
                                // drops the loaded document and leaves a white silhouette.
                                deck_b_stems_chev_up := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_up.svg") }
                                }
                                deck_b_stems_chev_down := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_down.svg") }
                                }
                            }
                            deck_b_stems_body := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 3
                                StemStack{
                                    deck_b_label_drums := KnobLabel{text: "DRUMS"}
                                    deck_b_stem_drums := StemKnob{}
                                    MSRow{
                                        deck_b_kill_drums := MSButton{text: "M"}
                                        deck_b_solo_drums := MSButton{text: "S"}
                                    }
                                }
                                StemStack{
                                    deck_b_label_bass := KnobLabel{text: "BASS"}
                                    deck_b_stem_bass := StemKnob{}
                                    MSRow{
                                        deck_b_kill_bass := MSButton{text: "M"}
                                        deck_b_solo_bass := MSButton{text: "S"}
                                    }
                                }
                                StemStack{
                                    deck_b_label_vocals := KnobLabel{text: "VOCALS"}
                                    deck_b_stem_vocals := StemKnob{}
                                    MSRow{
                                        deck_b_kill_vocals := MSButton{text: "M"}
                                        deck_b_solo_vocals := MSButton{text: "S"}
                                    }
                                }
                                StemStack{
                                    deck_b_label_other := KnobLabel{text: "OTHER"}
                                    deck_b_stem_other := StemKnob{}
                                    MSRow{
                                        deck_b_kill_other := MSButton{text: "M"}
                                        deck_b_solo_other := MSButton{text: "S"}
                                    }
                                }
                            }
                            deck_b_stem_state := StatusLabel{text: ""}
                            deck_b_grid_state := StatusLabel{text: ""}
                            // The switch header, as on deck A.
                            deck_b_kar_head := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                align: Align{x: 0.0, y: 0.0}
                                deck_b_kar_title := KnobLabel{
                                    text: "KARAOKE"
                                    margin: Inset{left: 0.0 right: 0.0 top: 6.0 bottom: 0.0}
                                    align: Align{x: 0.0, y: 0.0}
                                    draw_text +: {
                                        text_style: theme.font_bold{font_size: 8}
                                        color_hover: #xffffff
                                        color_down: #xffffff
                                    }
                                }
                                View{width: Fill height: 1}
                                // The chevron, drawn rather than typed: the small triangle
                                // glyphs are not in this font and came out as boxes. Two marks
                                // with one shown, never one mark with its svg swapped — that
                                // drops the loaded document and leaves a white silhouette.
                                deck_b_kar_chev_up := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_up.svg") }
                                }
                                deck_b_kar_chev_down := ChevronIcon{
                                    visible: false
                                    draw_icon +: { svg: crate_resource("self:resources/icons/chevron_down.svg") }
                                }
                            }
                            deck_b_lyrics := mod.widgets.VjLyricReader{height: Fill}
                        }
                        View{
                            // Fit, not a number: pitch 44, volume 44, the meter
                            // 10 and two 6pt gaps come to 110, and the 104 this
                            // used to claim was paid for by whichever child came
                            // last — deck A's meter squeezed to 4, deck B's pitch
                            // column to 38, clipping the 0 under it. Fit cannot
                            // fall out of step with its own children.
                            width: Fit
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
                                deck_b_mute := MusicButton{width: Fill height: 14 padding: 0 align: Align{x: 0.5, y: 0.5} text: "M"}
                            }
                            View{
                                width: 44
                                height: Fill
                                flow: Down
                                spacing: 2
                                align: Align{x: 0.5, y: 0.0}
                                MusicLabel{text: "TEMPO"}
                                deck_b_pitch := MusicFader{min: -1.0 max: 1.0 default: 0.0}
                                deck_b_pitch_reset := MusicButton{width: Fill height: 14 padding: 0 align: Align{x: 0.5, y: 0.5} text: "0"}
                            }
                        }
                    }
                    // Deck B's transport, at the foot of ITS column, right-aligned
                    // so the two decks mirror across the waveforms.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 3
                        align: Align{x: 1.0, y: 0.5}
                        // Deck B's row runs right to left, so the pair mirrors:
                        // the sparkle outermost, then out, then in.
                        deck_b_loop_scan := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/sparkle.svg") }
                        }
                        // NOT mirrored like the rest of the row: IN then OUT is the
                        // temporal order of the gesture, and hands read it
                        // left-to-right on every CDJ regardless of deck side.
                        deck_b_loop_in := MusicButton{width: 22 height: 24 text: "["}
                        deck_b_loop_out := MusicButton{width: 22 height: 24 text: "]"}
                        deck_b_loop_halve := MusicButton{width: 22 height: 24 text: "<"}
                        deck_b_loop_len := VjBeatsDrop{width: 24 loop_rows: true draw_bg +: {arrow: 0.0}}
                        deck_b_loop_double := MusicButton{width: 22 height: 24 text: ">"}
                        deck_b_loop := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/loop_one.svg") }
                        }
                        // The mirror of deck A's phones latch: hp then CUE,
                        // reading inward like the rest of the row.
                        deck_b_hp := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/headphones.svg") }
                        }
                        deck_b_cue := MusicButton{width: 40 height: 24 text: "CUE"}
                        deck_b_play := MusicIconButton{
                            draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") }
                        }
                    }
                }
            }

            // The lists and the strip that switches them, as ONE column.
            //
            // The strip has to live in here rather than beside the deck region:
            // as a sibling it became a third column the moment the page body
            // turned row-wise, and took its width straight out of the lanes.
            lists_column := View{
                // The third the decks leave, when the body is row-wise.
                // `App::sync_page_body_flow` sets this to exactly what
                // `console_scale::split_body` allots, so the declared value
                // only ever applies while the body is a column.
                width: Fill
                height: Fill
                flow: Down
                spacing: 6
                // ---- content explorer + queue ----
                // The old fixed height is a FLOOR now, not a ceiling: the row grows
                // into whatever the deck region above does not want, so a tall
                // window shows more of the library instead of empty console.
                // Explorer and queue, one at a time, on a console too narrow to
                // stand them side by side. One strip serves both: unlike the deck
                // tabs there is nothing beside these lists whose width a page-wide
                // row would take — they ARE the page at this point.
                lists_tab_strip := View{
                    visible: false
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.0, y: 0.5}
                    lists_tab_0 := MusicButton{width: 74 height: 22 text: "explorer"}
                    lists_tab_1 := MusicButton{width: 62 height: 22 text: "queue"}
                }
                View{
                    width: Fill
                    height: Fill
                    flow: Right
                    spacing: 8
                    new_batch: true
                    library_drop := RoundedView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 4
                        // Invisible until a file is dragged over it: the border
                        // is how this column says a drop would land here.
                        draw_bg +: {
                            color: #x00000000
                            border_color: #x00000000
                            border_size: 1.0
                            border_radius: 8.0
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 6
                            align: Align{x: 0.0, y: 0.5}
                            // Catalog-only controls: the local listing is neither
                            // searched nor paginated, so these fold away with it.
                            music_catalog := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 6
                                align: Align{x: 0.0, y: 0.5}
                                // Twelve characters of query, eight of category: the
                                // floors below which a field stops being a field. The
                                // search box takes whatever the row does not spend;
                                // the category cell is narrowed by
                                // `App::sync_library_density` when the console does.
                                music_search := TextInput{
                                    // Twelve characters of query at the floor, and a
                                    // ceiling: past ~488 the box is just a long empty
                                    // trough, and the row's other controls can use it.
                                    width: Fill{min: 96. max: 488.}
                                    // One line, always: the themed input wraps its
                                    // text by default, and a long query is not worth
                                    // making the whole row two lines tall.
                                    flow: Flow.Right{wrap: false}
                                    empty_text: "search music…"
                                }
                                music_category_cell := View{
                                    width: 96
                                    height: Fit
                                    music_category := TextInput{
                                        width: Fill
                                        flow: Flow.Right{wrap: false}
                                        empty_text: "category"
                                    }
                                }
                                music_go := MusicChipButton{
                                    text: "Search"
                                    draw_icon +: { svg: crate_resource("self:resources/icons/search.svg") }
                                }
                                music_more := MusicChipButton{
                                    text: "More"
                                    draw_icon +: { svg: crate_resource("self:resources/icons/more.svg") }
                                }
                            }
                            music_local := MusicChipButton{
                                text: "LOCAL FILES"
                                draw_icon +: { svg: crate_resource("self:resources/icons/folder.svg") }
                            }
                            // The same IMPORT CONTENT flow the VJ page has: pick a
                            // folder, and its media publishes into the store no-copy.
                            music_import := MusicChipButton{
                                text: "IMPORT"
                                draw_icon +: { svg: crate_resource("self:resources/icons/import.svg") }
                            }
                            // Fit, not a fixed 90: the count is four characters and a
                            // slash, and the dead width it used to carry pushed the
                            // load target away from it for nothing.
                            music_count := MusicLabel{width: Fit text: ""}
                            MusicLabel{text: "load"}
                            deck_target := DropDown{labels: ["Auto" "Deck A" "Deck B" "Off" "Mix"]}
                            // Latched, the deck a picked track lands on starts as
                            // soon as its decode finishes — "select and it plays".
                            music_autoplay := MusicChipButton{
                                text: "AUTOPLAY"
                                draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") }
                            }
                        }
                        // The music import's whole face, on a line of its own.
                        // It began wedged into the control row above, where the
                        // fixed-width chrome squeezed it to eight pixels — a
                        // refusal nobody could read looks exactly like a drop
                        // that did nothing. A Fill line cannot be squeezed, and
                        // an empty one costs a few pixels of height.
                        music_import_status := MusicLabel{
                            width: Fill
                            text: ""
                            draw_text.color: #xff5c39
                        }
                        // The column heads. Every one of them sorts: a click takes
                        // the order, a second click reverses it, and the arrow in the
                        // label says which column is holding it. STEM and KRK are
                        // heads like the rest — same widget, same box — so they sit
                        // on the line their neighbours sit on.
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 6
                            padding: Inset{left: 6.0 right: 6.0 top: 0.0 bottom: 0.0}
                            align: Align{x: 0.0, y: 0.5}
                            MusicLabel{width: 26 text: ""}
                            th_title := MusicColHead{width: Fill{weight: 400. min: 180.} text: "TITLE"}
                            th_artist := MusicColHead{width: Fill{max: 150.} text: "ARTIST"}
                            th_bpm := MusicColHead{width: 54 text: "BPM"}
                            th_key := MusicColHead{width: 40 text: "KEY"}
                            th_time := MusicColHead{width: 52 text: "TIME"}
                            // The cell carries the column width, not the head: a
                            // Button's walk is private, and on a narrow console
                            // `App::sync_library_density` shrinks these two cells and
                            // swaps the words for S and K. The rows' tick columns
                            // follow the same rule in `VjTrackList::draw_walk`, so
                            // header and rows never fall out of step.
                            music_th_stem_cell := View{
                                width: 36
                                height: Fit
                                music_th_stem := MusicColHead{width: Fill text: "STEM"}
                            }
                            music_th_krk_cell := View{
                                width: 30
                                height: Fit
                                music_th_krk := MusicColHead{width: Fill text: "KRK"}
                            }
                            th_tags := MusicColHead{width: Fill{max: 190.} text: "TAGS"}
                            MusicLabel{width: 26 text: ""}
                        }
                        music_tracks := mod.widgets.VjTrackList{show_queue_button: true}
                    }
                    queue_drop := RoundedView{
                        width: 320
                        height: Fill
                        flow: Down
                        spacing: 4
                        // Invisible until a file is dragged over it: the border
                        // is how this column says a drop would land here.
                        draw_bg +: {
                            color: #x00000000
                            border_color: #x00000000
                            border_size: 1.0
                            border_radius: 8.0
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 6
                            align: Align{x: 0.0, y: 0.5}
                            Label{
                                text: "QUEUE"
                                draw_text.color: #xff5c39
                                draw_text.text_style: theme.font_bold{font_size: 10}
                            }
                            queue_count := MusicLabel{width: Fill text: ""}
                            // Queue policy lives with the queue it governs:
                            // recycling and the pick order. The transition style
                            // moved into the AUTO DJ gear modal.
                            queue_repeat := MusicChipButton{
                                height: 20
                                text: "REPEAT"
                                draw_icon +: { svg: crate_resource("self:resources/icons/loop.svg") }
                            }
                            queue_shuffle := MusicChipButton{
                                height: 20
                                text: "SHUFFLE"
                                draw_icon +: { svg: crate_resource("self:resources/icons/shuffle.svg") }
                            }
                            queue_clear := MusicChipButton{
                                height: 20
                                text: "Clear"
                                draw_icon +: { svg: crate_resource("self:resources/icons/square_x.svg") }
                            }
                        }
                        // Compact: the 320-wide panel cannot seat the explorer's
                        // fixed columns — they squeezed the Fill title to nothing,
                        // which is why the queue used to read as bare numbers.
                        music_queue := mod.widgets.VjTrackList{show_queue_button: false compact: true}
                        // The DOCKED home of the pre-listen player: under the
                        // queue, exactly where the mockup parks it.
                        phones_dock := View{
                            visible: false
                            width: Fill
                            height: Fit
                            phones_dock_player := mod.widgets.VjPhonesPlayer{}
                        }
                    }
                }
            }
        }

        // ---- first-use model install: the row and its license gate ----
        // Hidden on a provisioned machine. On a fresh checkout it invites
        // the operator to install the stem splitter and the whisper
        // transcriber; while the install worker runs it is the progress
        // line. The modal is the license gate: nothing downloads before
        // Accept, mirroring the asset UI's weight-license flow.
        models_row := View{
            visible: false
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            align: Align{x: 0.0, y: 0.5}
            models_state := MusicLabel{width: Fill text: ""}
            models_install := MusicButton{width: 130 height: 20 text: "INSTALL MODELS"}
        }

        // The AUTO DJ settings dialog: mix tier, transition style, and the
        // two orthogonal brains. Opened by the gear beside AUTO DJ. A bare
        // Modal walks 0x0 in-flow and draws on the overlay, so it sits
        // directly in the page like the license modal below.
        auto_dj_modal := Modal{
            can_dismiss: true
            content +: {
                width: 340
                height: Fit
                RoundedView{
                    width: Fill
                    height: Fit
                    padding: 20
                    spacing: 12
                    flow: Down
                    draw_bg +: {
                        color: #x16161b
                        border_color: #xffffff18
                        border_size: 1.0
                        border_radius: 6.0
                    }
                    Label{
                        text: "AUTO DJ"
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 11}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 90 text: "MIX BRAIN"}
                        // RANDOM rolls a fresh brain for every transition.
                        auto_brain := DropDown{labels: ["FADE" "EQ" "STEMS" "RANDOM"]}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 90 text: "STYLE"}
                        // Checked mixes body-to-body; unchecked rides the
                        // outro, the classic hand-off.
                        auto_style := CheckBox{text: "BODY TO BODY"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        auto_vocal := MusicButton{width: 110 height: 22 text: "VOCAL GUARD"}
                        auto_phrase := MusicButton{width: 110 height: 22 text: "PHRASE SNAP"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        align: Align{x: 1.0, y: 0.5}
                        auto_cfg_close := MusicButton{width: 60 height: 22 text: "Close"}
                    }
                }
            }
        }

        // The loop-scan dialog: how long a loop to hunt for (seconds or
        // beats), how many to keep, and where marks go to be forgotten. One
        // modal serves both decks; the host remembers which deck's sparkle
        // opened it, and what to put back if the operator cancels.
        //
        // NOT dismissable by clicking outside: FIND and the two removes act
        // at once, so a third way out that is neither OK nor CANCEL would
        // leave the operator unsure which of the two they got.
        loop_scan_modal := Modal{
            can_dismiss: false
            content +: {
                width: 340
                height: Fit
                RoundedView{
                    width: Fill
                    height: Fit
                    padding: 20
                    spacing: 12
                    flow: Down
                    draw_bg +: {
                        color: #x16161b
                        border_color: #xffffff18
                        border_size: 1.0
                        border_radius: 6.0
                    }
                    Label{
                        text: "FIND LOOPS"
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 11}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 90 text: "UNIT"}
                        scan_unit := DropDown{labels: ["SECONDS" "BEATS"]}
                    }
                    scan_secs_rows := View{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 12
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0, y: 0.5}
                            MusicLabel{width: 90 text: "MIN SECS"}
                            scan_min_secs_dec := MusicButton{width: 22 height: 22 text: "-"}
                            scan_min_secs := TextInput{width: 60 text: "4"}
                            scan_min_secs_inc := MusicButton{width: 22 height: 22 text: "+"}
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0, y: 0.5}
                            MusicLabel{width: 90 text: "MAX SECS"}
                            scan_max_secs_dec := MusicButton{width: 22 height: 22 text: "-"}
                            scan_max_secs := TextInput{width: 60 text: "10"}
                            scan_max_secs_inc := MusicButton{width: 22 height: 22 text: "+"}
                        }
                    }
                    scan_beats_rows := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 12
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0, y: 0.5}
                            MusicLabel{width: 90 text: "MIN BEATS"}
                            scan_min_beats := DropDown{labels: ["8" "16" "32" "64" "128" "256" "512" "1024" "2048" "4096" "8192"]}
                        }
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0, y: 0.5}
                            MusicLabel{width: 90 text: "MAX BEATS"}
                            scan_max_beats := DropDown{labels: ["8" "16" "32" "64" "128" "256" "512" "1024" "2048" "4096" "8192"]}
                        }
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 90 text: "LOOPS"}
                        scan_count_dec := MusicButton{width: 22 height: 22 text: "-"}
                        scan_count := TextInput{width: 60 text: "10"}
                        scan_count_inc := MusicButton{width: 22 height: 22 text: "+"}
                    }
                    // Lit = on, the switch idiom the AUTO DJ dialog next
                    // door already uses for its two brains.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 90 text: "AUTOMATIC"}
                        scan_auto := MusicButton{width: 110 height: 22 text: "AUTO FIND"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        scan_remove_user := MusicButton{width: 146 height: 22 text: "REMOVE USER LOOPS"}
                        scan_remove_ai := MusicButton{width: 146 height: 22 text: "REMOVE AI LOOPS"}
                    }
                    // SCAN NOW sits alone on the left: it is the one button
                    // here that DOES something and leaves the dialog open,
                    // where CANCEL and OK are the two ways out and belong
                    // together on the right.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        scan_find := MusicButton{width: 84 height: 22 text: "FIND NOW"}
                        View{width: Fill height: 1}
                        scan_cancel := MusicButton{width: 70 height: 22 text: "CANCEL"}
                        scan_ok := MusicButton{width: 50 height: 22 text: "OK"}
                    }
                }
            }
        }

        models_license_modal := Modal{
            can_dismiss: true
            content +: {
                width: 560
                height: Fit
                RoundedView{
                    width: Fill
                    height: Fit
                    padding: 20
                    spacing: 10
                    flow: Down
                    draw_bg +: {
                        color: #x16161b
                        border_color: #xffffff18
                        border_size: 1.0
                        border_radius: 6.0
                    }
                    Label{
                        text: "About to download the deck models"
                        draw_text.color: #xe8eef4
                        draw_text.text_style: theme.font_bold{font_size: 12}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0, y: 0.5}
                        MusicValue{width: Fill text: "BS-RoFormer 4-stem splitter — 527 MB — MIT (ZFTurbo)"}
                        LinkLabel{text: "Terms" url: "https://github.com/ZFTurbo/Music-Source-Separation-Training/blob/main/LICENSE"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0, y: 0.5}
                        MusicValue{width: Fill text: "Whisper large-v3-turbo transcriber — 1.6 GB — MIT (OpenAI)"}
                        LinkLabel{text: "Terms" url: "https://github.com/openai/whisper/blob/main/LICENSE"}
                    }
                    Label{
                        width: Fill
                        text: "Both weight sets are MIT-licensed. They download once into local/ inside the checkout — resumable, with size and sha256 pinned — and nothing is uploaded anywhere."
                        draw_text.color: #x8e9aa7
                        draw_text.text_style.font_size: 9
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 1.0, y: 0.5}
                        models_download := MusicButton{width: 100 height: 22 text: "Download"}
                    }
                }
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
    /// 1 for a deck that is playing, less for a parked one.
    #[live(1.0)]
    pub active: f32,
    /// 1 once a stem texture is bound; 0 keeps the band colouring.
    ///
    /// The stem palette lives as four UNIFORMS in the script registration
    /// (`color_vocals`..`color_other`), pushed via `set_uniform` every
    /// draw — as instance inputs they tipped this shader over D3D11's
    /// vs_5_0 limit of 32 vertex inputs (36 > 32: no waveform at all on
    /// Windows). They are per-draw constants, so uniforms are their
    /// honest storage class anyway.
    #[live]
    pub has_stems: f32,
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

/// The band + level pyramid: red = low, green = mid, blue = high, and
/// alpha = the column's level against the whole track, which is the only
/// channel that decides how tall a column draws.
pub fn zoom_texture(cx: &mut Cx, tiles: &WaveTiles) -> Option<WavePyramid> {
    build_pyramid(cx, &tiles.zoom)
}

/// The stem-share pyramid, laid out identically to the band one so the
/// shader can sample both with the same level selection: red = vocals,
/// green = drums, blue = bass, alpha = other.
pub fn stem_texture(cx: &mut Cx, columns: &[[u8; 4]]) -> Option<WavePyramid> {
    build_pyramid(cx, columns)
}

/// What one separated column is MADE of, from the four stems' RMS.
///
/// These are shares, not levels: the shader normalizes them by their sum
/// and uses them only to divide the column's height between the four
/// colours. Nothing here can make a column taller, which is what keeps a
/// separated span the same height as the raw span next to it — and what
/// makes it safe to recompute this as the separator streams in, since a
/// column's colour cannot move when coverage grows.
///
/// The lanes are put on the same perceptual curve the band tiles use, so a
/// quiet stem is still legible beside a loud one, and the loudest lane of
/// the column is stored at full scale to spend the whole byte on the split.
pub fn stem_column_shares(rms: [f64; 4]) -> [u8; 4] {
    let top = rms.iter().fold(0.0f64, |a, b| a.max(*b));
    if top <= 1e-9 {
        // Separated but silent: nothing to divide, and no height to divide
        // it into. A single count keeps the column marked as covered so it
        // does not fall back to the grey colouring mid-song.
        return [1; 4];
    }
    let mut out = [0u8; 4];
    for (lane, value) in out.iter_mut().enumerate() {
        let share = (rms[lane] / top).clamp(0.0, 1.0).powf(crate::wave_analysis::WAVE_CURVE as f64);
        *value = (share * 255.0).round() as u8;
    }
    out
}

/// The height of one column's envelope, as a fraction of the half-lane —
/// the Rust mirror of the height law in `DrawWaveLane::pixel`, and what the
/// tests measure. Keep the two in step: the shader is the picture, this is
/// the proof.
pub fn column_height(tile: [u8; 4]) -> f32 {
    (tile[3] as f32 / 255.0).clamp(0.0, 1.0) * WAVE_ENVELOPE
}

/// The same column drawn as separated stems: the cumulative edges of the
/// bass, drums, vocals and other layers. Mirrors the shader's partition —
/// with every knob up, the last edge is exactly [`column_height`].
pub fn stem_stack(tile: [u8; 4], stems: [u8; 4], gains: [f32; 4]) -> [f32; 4] {
    let height = column_height(tile);
    let present: f32 = stems.iter().map(|s| *s as f32).sum();
    let inverse = 1.0 / present.max(0.0001);
    let share = |lane: usize| height * stems[lane] as f32 * inverse * gains[lane];
    let bass = share(2);
    let drums = share(1);
    let vocals = share(0);
    let other = share(3);
    [bass, bass + drums, bass + drums + vocals, bass + drums + vocals + other]
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
    /// The running loop in source seconds — the tile timebase, so this
    /// converts to columns exactly the way the grid does.
    pub loop_span: Option<(f64, f64)>,
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

    /// The loop's span in tile columns, or `None` when there is nothing
    /// worth drawing. Same timebase as the grid, so the same conversion.
    pub fn loop_columns(&self) -> Option<(f64, f64)> {
        let (start, end) = self.loop_span?;
        if end <= start {
            return None;
        }
        Some((start * ZOOM_COLS_PER_SEC, end * ZOOM_COLS_PER_SEC))
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

    /// The deck's running loop, for the band. Diffed: this comes off the
    /// status pump and hardly ever changes between ticks.
    pub fn set_loop_span(&mut self, cx: &mut Cx, deck: DeckId, span: Option<(f64, f64)>) {
        let lane = &mut self.lanes[deck.index()];
        if lane.loop_span == span {
            return;
        }
        lane.loop_span = span;
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
            set_stem_color_uniforms(&mut self.draw_lane, cx);
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
            set_loop_span_uniform(&mut self.draw_lane, cx, lane.loop_columns());
            set_preview_span_uniform(&mut self.draw_lane, cx, None);
            let (beat_cols, phase) = lane.grid_columns().unwrap_or((0.0, 0.0));
            self.draw_lane.beat_cols = beat_cols as f32;
            self.draw_lane.beat_phase = phase as f32;
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
    /// The green marker was clicked: keep the running loop as a blue one.
    SaveLoop,
    /// A blue marker was clicked: go into that saved loop again.
    RecallLoop { index: usize },
    /// A blue marker was dragged off its spot: forget that saved loop.
    DeleteLoop { index: usize },
    /// The red marker was dragged: CUE now sends the deck here.
    SetCue { secs: f64 },
    /// Click or drag: seek to this fraction of the track.
    Seek { fraction: f64 },
    /// A completed loop drag: put the loop's IN point here. Raw source
    /// seconds — the host owns QUANT, so the policy lives in one place.
    MoveLoop { start_secs: f64 },
    /// A yellow marker was clicked: go into that found loop.
    RecallFound { index: usize },
    /// A yellow marker was dragged off its spot: forget that finding.
    DeleteFound { index: usize },
}

/// What the finger currently on the strip is doing. Seeking scrubs the
/// playhead; moving carries the loop band, remembering where inside it the
/// grab landed so the band does not snap its in point under the cursor.
#[derive(Clone, Copy)]
enum OverviewDrag {
    Seek,
    /// A finger on a marker chip. Resolution waits for release: a short
    /// travel is the click (save or recall), a long one deletes a blue.
    Marker { hit: MarkerHit, origin: DVec2, at: DVec2 },
    /// QUANT is on: the playhead keeps playing where it is while a marker
    /// previews the snapped landing; release commits the jump.
    GhostSeek,
    MoveLoop { grab_offset_secs: f64 },
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
    drag: Option<OverviewDrag>,
    /// The running loop in source seconds, for the band. During a loop
    /// drag this is the GHOST: the live loop, which does not move — the
    /// playhead keeps living in it until the hand commits on release.
    #[rust]
    loop_span: Option<(f64, f64)>,
    /// Saved loops — the blue chips. `(start, end)` in source seconds.
    #[rust]
    loop_slots: Vec<(f64, f64)>,
    /// Scanner-found loops — the yellow chips on the bottom edge.
    #[rust]
    found_loops: Vec<(f64, f64)>,
    /// Where CUE lands — the red chip. Source seconds.
    #[rust]
    cue_secs: f64,
    /// The chip under the cursor, for the hover scale-up. Pressing takes
    /// it back to normal size — the pressed-down feel.
    #[rust]
    hover_marker: Option<MarkerHit>,
    /// The chips themselves: green for the running loop's handle, blue
    /// for a saved one — FCP's marker idiom at strip scale.
    #[live]
    draw_marker_live: DrawQuad,
    #[live]
    draw_marker_saved: DrawQuad,
    /// The yellow chip: a found loop, mirrored to point up from the
    /// bottom edge so the two mark rows never collide.
    #[live]
    draw_marker_found: DrawQuad,
    /// The SPAN lines: hovering a mark draws a hairline at each end of its
    /// loop, in the mark's own colour, so the whole span can be read off
    /// the strip without engaging it. One per colour rather than one
    /// re-coloured, so each line's colour is declared beside the chip it
    /// belongs to and the two cannot drift apart.
    #[live]
    draw_edge_live: DrawColor,
    #[live]
    draw_edge_saved: DrawColor,
    #[live]
    draw_edge_found: DrawColor,
    /// The red chip at CUE's landing — the track start — so the button's
    /// destination is visible at a glance.
    #[live]
    draw_marker_cue: DrawQuad,
    /// The dim twin the red chip sends out while being dragged: the solid
    /// one holds its ground, the ghost shows where CUE would land.
    #[live]
    draw_marker_cue_ghost: DrawQuad,
    /// The red chip's hover face: white, so the hand knows it is live.
    #[live]
    draw_marker_cue_hot: DrawQuad,
    /// The landing a drag-in-progress would commit, shown as a dimmer band
    /// beside the ghost. `None` outside a loop drag.
    #[rust]
    preview: Option<(f64, f64)>,
    /// The last raw (unsnapped) IN the finger asked for, so FingerUp can
    /// hand the host exactly what the hand meant and let the engine's own
    /// snap stay the authority.
    #[rust]
    preview_raw: Option<f64>,
    /// Snap inputs, mirrored from the host so the preview can run the same
    /// arithmetic the commit will.
    #[rust]
    snap_grid: Option<TrackGrid>,
    #[rust]
    snap_beats: u32,
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

    /// The running loop in source seconds, or `None`. Diffed, because this
    /// is pushed from the status pump and almost never changes between
    /// ticks — a redraw a frame for an unchanged band is the whole cost.
    pub fn set_loop_span(&mut self, cx: &mut Cx, span: Option<(f64, f64)>) {
        if self.loop_span == span {
            return;
        }
        self.loop_span = span;
        self.area.redraw(cx);
    }

    /// The marker under an absolute pointer position: blue/green/red in the
    /// strip's top band, yellow in the bottom band. On a strip short enough
    /// that the bands overlap, a top hit still wins the overlap
    /// deterministically — but a top-band MISS falls through to the bottom
    /// band rather than swallowing a yellow click the top band had nothing
    /// to say about. One math for clicks and for hover.
    fn marker_under(&self, rect: Rect, abs: DVec2) -> Option<MarkerHit> {
        if rect.size.x <= 1.0 || self.cols == 0 {
            return None;
        }
        let duration = self.cols as f64 / ZOOM_COLS_PER_SEC;
        let secs = ((abs.x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0) * duration;
        let tol = MARKER_GRAB_PX / rect.size.x * duration;
        let from_top = abs.y - rect.pos.y;
        let from_bottom = rect.pos.y + rect.size.y - abs.y;
        if from_top <= MARKER_STRIP_PX {
            let running_in = self.loop_span.map(|(start, _)| start);
            if let Some(hit) = marker_hit(&self.loop_slots, running_in, self.cue_secs, secs, tol) {
                return Some(hit);
            }
        }
        if from_bottom <= MARKER_STRIP_PX {
            return found_marker_hit(&self.found_loops, secs, tol);
        }
        None
    }

    /// The red chip's home, diffed like the others.
    pub fn set_cue_marker(&mut self, cx: &mut Cx, secs: f64) {
        if (self.cue_secs - secs).abs() < 1e-9 {
            return;
        }
        self.cue_secs = secs;
        self.area.redraw(cx);
    }

    /// The saved-loop chips, diffed like the span push.
    pub fn set_loop_slots(&mut self, cx: &mut Cx, slots: &[(f64, f64)]) {
        if self.loop_slots.as_slice() == slots {
            return;
        }
        self.loop_slots = slots.to_vec();
        self.area.redraw(cx);
    }

    /// The found-loop chips, diffed like the others.
    pub fn set_found_loops(&mut self, cx: &mut Cx, spans: &[(f64, f64)]) {
        if self.found_loops.as_slice() == spans {
            return;
        }
        self.found_loops = spans.to_vec();
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

    /// The snap inputs, so the preview and the commit are the same
    /// arithmetic. Diffed like the other pump pushes.
    pub fn set_snap_grid(&mut self, cx: &mut Cx, grid: Option<TrackGrid>, unit_beats: u32) {
        if self.snap_grid == grid && self.snap_beats == unit_beats {
            return;
        }
        self.snap_grid = grid;
        self.snap_beats = unit_beats;
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

    /// Source seconds under a pointer at `x`, plus how many seconds one
    /// `BAND_GRAB_PX` of screen is worth here. The strip never learns the
    /// duration directly — the tile timebase is fixed, so its column count
    /// already carries it.
    fn secs_at(&self, cx: &mut Cx, x: f64) -> Option<(f64, f64)> {
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 || self.cols == 0 {
            return None;
        }
        let duration = self.cols as f64 / ZOOM_COLS_PER_SEC;
        let secs = ((x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0) * duration;
        Some((secs, BAND_GRAB_PX / rect.size.x * duration))
    }

    /// One step of a loop drag: preview the snapped landing of `x`, ghost
    /// untouched. Called from FingerDown too, so a bare click previews —
    /// and can commit — without ever moving.
    fn preview_move(&mut self, cx: &mut Cx, x: f64, grab_offset_secs: f64) {
        let Some((secs, _)) = self.secs_at(cx, x) else { return };
        let raw = secs - grab_offset_secs;
        let duration = self.cols as f64 / ZOOM_COLS_PER_SEC;
        if let Some(preview) =
            move_preview(self.loop_span, raw, self.snap_grid, self.snap_beats, duration)
        {
            // A candidate off the track keeps the previous preview: the
            // band stops at the wall.
            self.preview = Some(preview);
            self.preview_raw = Some(raw);
            self.area.redraw(cx);
        }
    }

    /// One step of a ghost seek: a MARKER at the snapped landing — a
    /// degenerate preview band the shader's edge rule draws as a line.
    /// The reference is the strip's own playhead; the engine re-snaps on
    /// commit with its sync-aware reference, so the marker is a preview,
    /// not the authority.
    fn preview_ghost_seek(&mut self, cx: &mut Cx, x: f64) {
        let Some((secs, _)) = self.secs_at(cx, x) else { return };
        let duration = self.cols as f64 / ZOOM_COLS_PER_SEC;
        let landing = match self.snap_grid {
            Some(grid) => {
                grid.snap_translate(secs, self.head * duration, self.snap_beats)
            }
            None => secs,
        };
        self.preview = Some((landing, landing + 2.0 / ZOOM_COLS_PER_SEC));
        self.preview_raw = Some(secs);
        self.area.redraw(cx);
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
                // Marker strip first: a chip click is neither a seek nor a
                // loop drag, whatever else is going on below it.
                let rect = self.area.rect(cx);
                if let Some(hit) = self.marker_under(rect, fe.abs) {
                    // Armed, not fired: release decides between the click
                    // and, for a blue chip dragged far enough, the delete.
                    self.drag = Some(OverviewDrag::Marker { hit, origin: fe.abs, at: fe.abs });
                    self.area.redraw(cx);
                    return;
                }
                if matches!(self.loop_span, Some((start, end)) if end > start) {
                    // While a loop runs, the whole strip is the loop's:
                    // a grab inside the band keeps its offset, a press
                    // anywhere else lands IN under the finger. Seeking
                    // waits for the loop to be exited — the loop owns the
                    // deck while it plays.
                    let grab_offset_secs = self
                        .secs_at(cx, fe.abs.x)
                        .and_then(|(secs, tol)| band_grab(self.loop_span, secs, tol))
                        .unwrap_or(0.0);
                    self.drag = Some(OverviewDrag::MoveLoop { grab_offset_secs });
                    self.preview_move(cx, fe.abs.x, grab_offset_secs);
                } else if self.snap_beats > 0
                    && self.snap_grid.is_some_and(|grid| grid.has_grid())
                {
                    // QUANT on: seeks ghost too. The music keeps playing
                    // while a marker previews the snapped landing, and
                    // release commits — a live scrub in whole-unit steps
                    // would be a stutter, not a preview.
                    self.drag = Some(OverviewDrag::GhostSeek);
                    self.preview_ghost_seek(cx, fe.abs.x);
                } else {
                    self.drag = Some(OverviewDrag::Seek);
                    self.seek_at(cx, fe.abs.x);
                }
            }
            Hit::FingerMove(fe) => match self.drag {
                Some(OverviewDrag::Seek) => self.seek_at(cx, fe.abs.x),
                Some(OverviewDrag::Marker { hit, origin, .. }) => {
                    self.drag = Some(OverviewDrag::Marker { hit, origin, at: fe.abs });
                    self.area.redraw(cx);
                }
                Some(OverviewDrag::GhostSeek) => self.preview_ghost_seek(cx, fe.abs.x),
                Some(OverviewDrag::MoveLoop { grab_offset_secs }) => {
                    // The live loop does not move: the playhead keeps
                    // living in the ghost until the hand commits.
                    self.preview_move(cx, fe.abs.x, grab_offset_secs);
                }
                None => {}
            },
            Hit::FingerUp(_) => {
                match (self.drag, self.preview_raw, self.preview) {
                    (Some(OverviewDrag::Marker { hit, origin, at }), _, _) => {
                        let travelled = (at - origin).length();
                        match hit {
                            MarkerHit::Recall(index) if travelled >= MARKER_DELETE_PX => {
                                self.events.push(OverviewEvent::DeleteLoop { index });
                            }
                            MarkerHit::Recall(index) => {
                                self.events.push(OverviewEvent::RecallLoop { index });
                            }
                            MarkerHit::Found(index) if travelled >= MARKER_DELETE_PX => {
                                self.events.push(OverviewEvent::DeleteFound { index });
                            }
                            MarkerHit::Found(index) => {
                                self.events.push(OverviewEvent::RecallFound { index });
                            }
                            MarkerHit::Save if travelled < MARKER_DELETE_PX => {
                                self.events.push(OverviewEvent::SaveLoop);
                            }
                            // A green chip dragged past the threshold is a
                            // cancel: there is nothing saved to delete.
                            MarkerHit::Save => {}
                            MarkerHit::Cue => {
                                // Any travel moves the cue; the engine owns
                                // clamping and the QUANT translation.
                                if self.cols != 0 {
                                    let duration = self.cols as f64 / ZOOM_COLS_PER_SEC;
                                    let rect = self.area.rect(cx);
                                    if rect.size.x > 1.0 {
                                        let secs = ((at.x - rect.pos.x) / rect.size.x)
                                            .clamp(0.0, 1.0)
                                            * duration;
                                        self.events.push(OverviewEvent::SetCue { secs });
                                    }
                                }
                            }
                        }
                    }
                    (Some(OverviewDrag::MoveLoop { .. }), Some(raw), Some(preview)) => {
                        // One event per completed drag — and none for a
                        // drag that came home.
                        if self.loop_span != Some(preview) {
                            self.events.push(OverviewEvent::MoveLoop { start_secs: raw });
                        }
                    }
                    (Some(OverviewDrag::GhostSeek), Some(raw), _) => {
                        // The RAW finger position: the engine's snap is the
                        // authority, with its sync-aware reference.
                        let duration = self.cols.max(1) as f64 / ZOOM_COLS_PER_SEC;
                        self.events.push(OverviewEvent::Seek {
                            fraction: (raw / duration).clamp(0.0, 1.0),
                        });
                    }
                    _ => {}
                }
                self.drag = None;
                self.preview = None;
                self.preview_raw = None;
                self.area.redraw(cx);
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                cx.set_cursor(MouseCursor::Hand);
                let rect = self.area.rect(cx);
                let hover = self.marker_under(rect, fe.abs);
                if hover != self.hover_marker {
                    self.hover_marker = hover;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover_marker.take().is_some() {
                    self.area.redraw(cx);
                }
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
        // The loop DOES belong here: the strip is where you see which part
        // of the song you are stuck in. Same tile timebase as the lanes.
        let columns = self
            .loop_span
            .map(|(start, end)| (start * ZOOM_COLS_PER_SEC, end * ZOOM_COLS_PER_SEC));
        set_loop_span_uniform(&mut self.draw_lane, cx, columns);
        // During a drag the ghost above stays put and this is where release
        // will land — the pair is the whole point of the ghost model.
        let preview_columns = self
            .preview
            .map(|(start, end)| (start * ZOOM_COLS_PER_SEC, end * ZOOM_COLS_PER_SEC));
        set_preview_span_uniform(&mut self.draw_lane, cx, preview_columns);
        self.draw_lane.active = if self.active { 1.0 } else { 0.7 };
        set_stem_color_uniforms(&mut self.draw_lane, cx);
        // The reference picture: every layer at full weight, whatever the
        // knobs are doing to the mix.
        self.draw_lane.gain_vocals = 1.0;
        self.draw_lane.gain_drums = 1.0;
        self.draw_lane.gain_bass = 1.0;
        self.draw_lane.gain_other = 1.0;
        self.draw_lane.head_col = (self.head * self.cols.max(1) as f64) as f32;
        self.draw_lane.head_on = if self.pyramid.is_some() { 1.0 } else { 0.0 };
        self.draw_lane.draw_abs(cx, rect);
        // The marker chips ride the top edge, each over its loop's IN.
        // NO chip ever moves with the pointer: hover scales one up, a
        // press takes it back to normal (the pressed-down feel), and a
        // blue chip being dragged stays home while the drag pulls its
        // invisible soul — past the threshold it dies in place.
        if self.cols != 0 {
            let duration = self.cols as f64 / ZOOM_COLS_PER_SEC;
            // The POINT aims at the position, so the centre is never
            // clamped — a chip at the track edge hangs half off the strip
            // rather than lying about where it points.
            let chip_sized = |centre: f64, grown: bool| {
                let (w, h) = if grown { (14.0, 17.0) } else { (9.0, 11.0) };
                Rect {
                    pos: dvec2(centre - w * 0.5, rect.pos.y),
                    size: dvec2(w, h),
                }
            };
            let centre_of =
                |start: f64| rect.pos.x + (start / duration).clamp(0.0, 1.0) * rect.size.x;
            let held = match self.drag {
                Some(OverviewDrag::Marker { hit, origin, at }) => Some((hit, origin, at)),
                _ => None,
            };
            // Hover grows a chip only while it is not being pressed.
            let grown = |hit: MarkerHit| {
                self.hover_marker == Some(hit) && !matches!(held, Some((h, _, _)) if h == hit)
            };
            // Hovering a mark draws its whole span: a hairline in the
            // mark's own colour at the IN and another at the OUT, so the
            // loop can be read off the strip without engaging it. The IN
            // line runs the full height under its chip — the chip alone
            // marks a point, and a point does not read as an edge the way
            // its partner across the strip does. Drawn UNDER the chips. A
            // bookmark is a zero-length span and the cue is a point —
            // neither has a span to show, and neither draws a line.
            let hovered_span = match self.hover_marker {
                Some(MarkerHit::Recall(index)) => {
                    self.loop_slots.get(index).copied().map(|span| (span, 1u8))
                }
                Some(MarkerHit::Save) => self.loop_span.map(|span| (span, 0u8)),
                Some(MarkerHit::Found(index)) => {
                    self.found_loops.get(index).copied().map(|span| (span, 2u8))
                }
                _ => None,
            };
            if let Some(((start, end), kind)) = hovered_span {
                if end - start > 1e-6 {
                    let edge = match kind {
                        0 => &mut self.draw_edge_live,
                        1 => &mut self.draw_edge_saved,
                        _ => &mut self.draw_edge_found,
                    };
                    for at in [start, end] {
                        edge.draw_abs(
                            cx,
                            Rect {
                                pos: dvec2(centre_of(at) - 0.75, rect.pos.y),
                                size: dvec2(1.5, rect.size.y),
                            },
                        );
                    }
                }
            }
            // CUE's landing, under everything else. While dragged, the
            // solid chip holds its ground and a ghost shows the landing.
            let cue_rect = chip_sized(centre_of(self.cue_secs), grown(MarkerHit::Cue));
            if grown(MarkerHit::Cue) {
                self.draw_marker_cue_hot.draw_abs(cx, cue_rect);
            } else {
                self.draw_marker_cue.draw_abs(cx, cue_rect);
            }
            if let Some((MarkerHit::Cue, _, at)) = held {
                self.draw_marker_cue_ghost.draw_abs(cx, chip_sized(at.x, false));
            }
            for (index, slot) in self.loop_slots.iter().copied().enumerate() {
                if let Some((MarkerHit::Recall(dragged), origin, at)) = held {
                    if dragged == index && (at - origin).length() >= MARKER_DELETE_PX {
                        // Dead where it stood: release will delete it.
                        continue;
                    }
                }
                self.draw_marker_saved
                    .draw_abs(cx, chip_sized(centre_of(slot.0), grown(MarkerHit::Recall(index))));
            }
            // Found loops ride the BOTTOM edge, mirrored: same behaviours
            // as the blue row — hover grows, a drag past the threshold
            // dies in place.
            let chip_bottom = |centre: f64, grown: bool| {
                let (w, h) = if grown { (14.0, 17.0) } else { (9.0, 11.0) };
                Rect {
                    pos: dvec2(centre - w * 0.5, rect.pos.y + rect.size.y - h),
                    size: dvec2(w, h),
                }
            };
            for (index, span) in self.found_loops.iter().copied().enumerate() {
                if let Some((MarkerHit::Found(dragged), origin, at)) = held {
                    if dragged == index && (at - origin).length() >= MARKER_DELETE_PX {
                        continue;
                    }
                }
                self.draw_marker_found
                    .draw_abs(cx, chip_bottom(centre_of(span.0), grown(MarkerHit::Found(index))));
            }
            if let Some((start, _)) = self.loop_span {
                let saved = self.loop_slots.iter().any(|slot| (slot.0 - start).abs() < 1e-6);
                if !saved {
                    self.draw_marker_live
                        .draw_abs(cx, chip_sized(centre_of(start), grown(MarkerHit::Save)));
                }
            }
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// the lyrics reader (shared widget)
// ---------------------------------------------------------------------------

/// The transcript panel moved to the shared widget family so the asset UI's
/// audio preview shows the same reader; the VJ keeps its name as an alias
/// (`mod.widgets.VjLyricReader` in the script_mod above).
pub use makepad_asset_widgets::lyric_reader::{
    lyric_stamp, LyricEvent, LyricReader as VjLyricReader, LyricRow,
};

// ---------------------------------------------------------------------------
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
    /// The store holds this track's four separated stems.
    pub stem: bool,
    /// The store holds this track's word-aligned transcript.
    pub krk: bool,
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
            stem: false,
            krk: false,
            badge: String::new(),
            live: false,
        }
    }
}

/// Below this list width the explorer drops its word headers: STEM and KRK
/// read S and K, and their columns shrink to `MARK_COLUMN_NARROW`. Measured
/// against the column set: badge, artist, bpm, key, time, the two marks,
/// tags and the queue chip cost ~670 points before the title gets its floor
/// of 180, so a list under ~900 is already spending pixels it does not have.
pub const LIBRARY_NARROW_WIDTH: f64 = 900.0;

// ---------------------------------------------------------------------------
// the transport strip: three groups, an order that depends on the width
// ---------------------------------------------------------------------------

/// The longest the sweep is EVER drawn, on any row it lands on. Past this
/// the strip centres it and leaves the rest as air: a fader longer than
/// this cannot be played across in one throw of the hand, so the extra
/// width buys nothing and costs the reach.
///
/// This is a hard ceiling, not a preference the layout may trade away. It
/// used to bind only a FLANKED sweep, on the argument that a row the sweep
/// owns outright has nothing but air to put either side of it — which is
/// true, and beside the point: air is what the operator asked for, and a
/// full-width console stretched the fader to the whole row instead.
///
/// LAYOUT POINTS, not screen pixels. The operator's number was 576 px as
/// measured on a 1.5x display, which is 384 points — and points are the
/// right home for it, since the same 384 keeps the fader the same APPARENT
/// size on a display of any density. It read as 576 for a while without
/// anyone noticing, because until the cap bound on every row (it used to
/// bind only on a flanked one) no ordinary console width ever reached it.
///
/// The hand wants the throw, not the restraint: at 280 a wide console left
/// a visibly short fader with room going spare either side of it.
pub const STRIP_SWEEP_MAX: f64 = 384.0;

/// The shortest sweep worth flanking with cue keys. Under this they stop
/// sharing its line and take one of their own.
pub const STRIP_SWEEP_MIN: f64 = 150.0;

/// Breathing room kept on a row the strip fills deliberately. The group
/// widths are measured from the frame before, so a control that changed
/// size this frame — a cue key losing its bare A, say — can leave the sum a
/// point or two over the row and wrap something the strip meant to keep.
pub const STRIP_ROW_SLACK: f64 = 8.0;

/// Under this STRIP width the bare A and B either side of the sweep go: the
/// cue keys flanking them already read A and B.
pub const STRIP_FADER_LABELS_MIN: f64 = 320.0;
/// The transport strip: fade shaping, the crossfader and the automation.
///
/// Two rules the operator gave, which no single source order can satisfy:
/// the fader is always CENTRED, and whenever the strip needs more than one
/// row the fader is the row on TOP. On one line that means shaping, fader,
/// automation — the fader in the middle, flanked. Wrapped, it means the
/// fader first, alone. A `Flow::Right{wrap}` lays out in source order, so
/// the order is decided here instead, per frame, from the width the strip
/// actually got.
///

/// Everything else is still the turtle's: the children wrap and each row
/// centres itself exactly as it would in a plain wrapping view.

/// A strip child's walk: the width the strip decided for it, and whatever
/// height its own contents come to.
/// What the strip makes of a row `available` points across: which of its
/// three shapes it takes, and how wide the sweep is drawn in that shape.
///
/// Pure, and separate from the drawing, because the sweep's ceiling is a
/// promise about the fader itself — never wider than `STRIP_SWEEP_MAX`, on
/// any row, at any console width — and a promise that holds at whatever
/// width the operator happens to drag the window to is one that has to be
/// checkable at every width, not at the one that is on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripPlan {
    /// The row's usable width: the strip's own, or `STRIP_SWEEP_MAX` while
    /// the turtle cannot say.
    pub row: f64,
    /// One line holds everything: both groups, the cue keys, and a sweep
    /// still worth playing.
    pub flanked: bool,
    /// The cue keys stay beside the sweep. When they cannot they are
    /// hidden, not moved: a row of their own would cost the lanes above it.
    pub cues_inline: bool,
    /// How wide the sweep is drawn. NEVER more than `STRIP_SWEEP_MAX`.
    pub sweep_w: f64,
}

pub fn strip_sweep_plan(
    available: f64,
    shaping_w: f64,
    automation_w: f64,
    cue_w: f64,
    spacing: f64,
) -> StripPlan {
    let row = if available.is_finite() { available } else { STRIP_SWEEP_MAX };
    let cues = 2.0 * cue_w + 2.0 * spacing;
    // One line needs both groups AND a sweep worth playing.
    let flanked = available.is_finite()
        && available >= shaping_w + automation_w + cues + STRIP_SWEEP_MIN + 4.0 * spacing;
    // Can the cue keys flank the sweep and still leave it playable?
    let cues_inline = flanked || row - cues - STRIP_ROW_SLACK >= STRIP_SWEEP_MIN;
    let sweep_w = if flanked {
        available - shaping_w - automation_w - cues - 4.0 * spacing
    } else if cues_inline {
        // The fader takes the top row, cue keys still flanking it.
        row - cues - STRIP_ROW_SLACK
    } else {
        // Too narrow to flank: the sweep takes the whole row on its own.
        row
    };
    StripPlan {
        row,
        flanked,
        cues_inline,
        // The one clamp. It is applied to every shape, not only the flanked
        // one: a row the sweep owns outright has nothing but air to put
        // either side of a capped fader, and air is exactly what was asked
        // for — the row centres what it holds, so the leftover falls away
        // evenly and the fader keeps the throw one hand can cross.
        sweep_w: sweep_w.clamp(0.0, STRIP_SWEEP_MAX),
    }
}

/// What the strip remembers about one group after a pass.
///
/// Only a group the strip let draw FREE has told it anything. A group the
/// strip WALKED reports back the width the strip forced on it — the
/// strip's own number, not news — and filing that away as the group's own
/// is what set the layout oscillating: walk a too-wide group to the row,
/// measure the row, conclude it now fits, draw it free, measure it too wide
/// again, walk it again. Two frames a cycle, for as long as the window
/// stays narrow, with a redraw asked for every one of them.
///
/// So a walked group keeps its last free measurement, and a hidden one
/// (which measures zero) keeps it too: the cue keys go and come back, and
/// the strip has to remember what they are worth while they are away.
fn remembered_width(last: f64, measured: f64, drawn_free: bool) -> f64 {
    if drawn_free && measured > 1.0 {
        measured
    } else {
        last
    }
}

fn strip_walk(width: f64) -> Walk {
    Walk {
        abs_pos: None,
        margin: Inset::default(),
        width: Size::Fixed(width.max(0.0)),
        height: Size::fit(),
        metrics: Metrics::default(),
    }
}
#[derive(Script, ScriptHook, Widget)]
pub struct VjWrapStrip {
    #[deref]
    view: View,
    #[rust]
    area: Area,
    /// What the groups (shaping, automation, one cue key) measured at the
    /// END of the last draw — the only moment their areas are real. `None`
    /// until the strip has drawn once, when the fallbacks stand in.
    #[rust]
    group_w: Option<(f64, f64, f64)>,
}

impl Widget for VjWrapStrip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let shaping = self.view.widget(cx, ids!(strip_shaping));
        let cue_a = self.view.widget(cx, ids!(fader_cue_a));
        let sweep = self.view.widget(cx, ids!(fader_sweep));
        let cue_b = self.view.widget(cx, ids!(fader_cue_b));
        let automation = self.view.widget(cx, ids!(strip_automation));
        let layout = self.view.layout;
        let spacing = layout.spacing;
        let wrap_spacing = layout.wrap_spacing;

        // What the groups came to at the END of the last draw. Their
        // contents are themed widgets — a drop-down's width is the theme's
        // business, not ours — so the strip MEASURES them rather than carry
        // numbers that go stale the moment someone restyles a button.
        //
        // Measured at the end of the pass, never here: a child's area is
        // only readable once it has drawn, and by the top of this pass the
        // areas of the last one are already gone. Asking at this point
        // always answered zero, so the strip ran on its fallbacks for every
        // frame of its life — and those fallbacks over-guessed the groups by
        // some ninety points, which the sweep paid for and the row wore as
        // air at both ends.
        let (shaping_w, automation_w, cue_w) =
            self.group_w.unwrap_or((380.0, 290.0, 72.0));
        // Whether those numbers are measurements or still the guesses.
        let measured = self.group_w.is_some();

        cx.begin_turtle(walk, layout);
        let available = cx.turtle().inner_width();
        let plan = strip_sweep_plan(available, shaping_w, automation_w, cue_w, spacing);
        let row = plan.row;

        // The bare A and B either side of the sweep are a courtesy the strip
        // cannot always afford — the cue keys already read A and B. Keyed to
        // the STRIP's width, never to the sweep's: the labels sit inside the
        // cue keys, so a sweep-width rule would feed back into its own input
        // and flicker. Toggled only on a change, so a draw never asks for a
        // redraw that asks for a draw.
        let labels = row >= STRIP_FADER_LABELS_MIN;
        for (group, id) in [(&cue_a, ids!(xfade_label_a)), (&cue_b, ids!(xfade_label_b))] {
            let label = group.widget(cx, id);
            if label.visible() != labels {
                label.set_visible(cx, labels);
            }
        }
        // When the cue keys cannot flank the sweep they GO — hidden, not
        // moved: a row of their own would cost the lanes above another 28
        // points, and FADE and CUT beside the duration reach the same two
        // decks without asking for the room. A group is measured while it is
        // VISIBLE, so cue_w still knows what they are worth when the width
        // comes back.
        for group in [&cue_a, &cue_b] {
            if group.visible() != plan.cues_inline {
                group.set_visible(cx, plan.cues_inline);
            }
        }

        // A group is drawn at its natural width where it fits, and WALKED to
        // the row where it does not: a Fit-width wrapping row has no bound to
        // wrap against, so an unwalked group overflows the strip and its last
        // control — the curve chip — is clipped away entirely.
        //
        // The very first pass draws free whatever the fallbacks say. The
        // strip has to learn what its groups actually come to, and a walked
        // group never tells it — so a strip that started narrow and walked
        // its groups on the strength of a guess would keep that guess for
        // good. One frame of a group overflowing is the price of never
        // guessing again.
        //
        // On a flanked row every group is drawn free by construction: the
        // arithmetic that chose that shape already found room for all of
        // them side by side.
        let shaping_free = plan.flanked || !measured || shaping_w <= row;
        let automation_free = plan.flanked || !measured || automation_w <= row;
        if plan.flanked {
            shaping.draw_all(cx, scope);
            cue_a.draw_all(cx, scope);
            sweep.draw_walk_all(cx, scope, strip_walk(plan.sweep_w));
            cue_b.draw_all(cx, scope);
            automation.draw_all(cx, scope);
        } else if plan.cues_inline {
            cue_a.draw_all(cx, scope);
            sweep.draw_walk_all(cx, scope, strip_walk(plan.sweep_w));
            cue_b.draw_all(cx, scope);
            cx.turtle_new_line_with_spacing(wrap_spacing);
            if shaping_free {
                shaping.draw_all(cx, scope);
            } else {
                shaping.draw_walk_all(cx, scope, strip_walk(row));
            }
            if automation_free {
                automation.draw_all(cx, scope);
            } else {
                automation.draw_walk_all(cx, scope, strip_walk(row));
            }
        } else {
            // Too narrow to flank: the cue keys are hidden (above) and the
            // sweep takes the whole row on its own.
            sweep.draw_walk_all(cx, scope, strip_walk(plan.sweep_w));
            cx.turtle_new_line_with_spacing(wrap_spacing);
            if shaping_free {
                shaping.draw_all(cx, scope);
            } else {
                shaping.draw_walk_all(cx, scope, strip_walk(row));
            }
            if automation_free {
                automation.draw_all(cx, scope);
            } else {
                automation.draw_walk_all(cx, scope, strip_walk(row));
            }
        }

        cx.end_turtle_with_area(&mut self.area);

        // NOW the groups have drawn, so their areas are real: this is the
        // only point in the frame where the strip can learn what its own
        // children came to — and only from the ones it did not force.
        let fresh = (
            remembered_width(shaping_w, shaping.area().rect(cx).size.x, shaping_free),
            remembered_width(
                automation_w,
                automation.area().rect(cx).size.x,
                automation_free,
            ),
            remembered_width(
                cue_w,
                cue_a.area().rect(cx).size.x.max(cue_b.area().rect(cx).size.x),
                plan.cues_inline,
            ),
        );
        // The sweep was sized from the numbers at the top of this pass. If
        // the groups came to something else, the row is short (or long) by
        // the difference and nothing else would ever ask for the frame that
        // puts it right — so ask here, and only while the two still
        // disagree, which stops the asking as soon as they agree.
        let settled = self.group_w.is_some_and(|was| {
            (was.0 - fresh.0).abs() < 0.5
                && (was.1 - fresh.1).abs() < 0.5
                && (was.2 - fresh.2).abs() < 0.5
        });
        self.group_w = Some(fresh);
        if !settled {
            self.area.redraw(cx);
        }
        DrawStep::done()
    }
}
/// What a mark column costs once its header is one letter.
pub const MARK_COLUMN_NARROW: f64 = 16.0;

/// A click in a track list.
/// Seek gestures out of the pre-listen wave strip, cast from the global
/// actions list. Placement-agnostic on purpose: whichever instance is live
/// (docked, inline, floating) steers the ONE preview player.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum PhonesWaveAction {
    Seek(f64),
    #[default]
    None,
}

/// The pre-listen seek strip: the decoded track's peaks as amber bins, a
/// playhead line, and a press-or-drag that asks the host to jump — a mini
/// VjWaveOverview with everything but the shape and the seek stripped out.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjPhonesWave {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_bin: DrawColor,
    #[live]
    draw_head: DrawColor,
    #[rust]
    peaks: Arc<Vec<f32>>,
    #[rust]
    fraction: f64,
    /// While a hand is on the strip the drag owns the head — the pump's
    /// pushes wait for release.
    #[rust]
    dragging: bool,
    #[rust]
    area: Area,
}

impl VjPhonesWave {
    /// New track: the strip's picture, head rewound.
    pub fn set_peaks(&mut self, cx: &mut Cx, peaks: Arc<Vec<f32>>) {
        self.peaks = peaks;
        self.fraction = 0.0;
        self.area.redraw(cx);
    }

    /// The playhead, pushed by the host's pump.
    pub fn set_fraction(&mut self, cx: &mut Cx, fraction: f64) {
        if self.dragging {
            return;
        }
        if (fraction - self.fraction).abs() > 1e-4 {
            self.fraction = fraction.clamp(0.0, 1.0);
            self.area.redraw(cx);
        }
    }

    fn fraction_at(&self, cx: &mut Cx, x: f64) -> Option<f64> {
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 {
            return None;
        }
        Some(((x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0))
    }
}

impl WidgetNode for VjPhonesWave {
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

impl Widget for VjPhonesWave {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.dragging = true;
                if let Some(fraction) = self.fraction_at(cx, fe.abs.x) {
                    self.fraction = fraction;
                    cx.widget_action(self.uid, PhonesWaveAction::Seek(fraction));
                    self.area.redraw(cx);
                }
            }
            Hit::FingerMove(fe) => {
                if self.dragging {
                    if let Some(fraction) = self.fraction_at(cx, fe.abs.x) {
                        self.fraction = fraction;
                        cx.widget_action(self.uid, PhonesWaveAction::Seek(fraction));
                        self.area.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(_) => {
                self.dragging = false;
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 4.0 || rect.size.y < 4.0 {
            return DrawStep::done();
        }
        self.draw_bg.draw_abs(cx, rect);
        if !self.peaks.is_empty() {
            // Bars are laid out in PIXELS, not one per stored bin: a bin
            // pitch under a pixel lands each bar on a different sub-pixel
            // and the strip shimmers light/dark instead of reading as
            // audio. Fixed 3px pitch (2px bar, 1px gap), each bar taking
            // the loudest bin under it.
            let mid = rect.pos.y + rect.size.y * 0.5;
            // HEADROOM: the loudest bar stops short of the strip's edge.
            // Drawn flush, a peak that exactly meets the boundary reads as
            // a waveform sliced off by its container rather than one that
            // fits inside it — the picture has to look like it has room.
            let half = ((rect.size.y * 0.5 - 1.0) * 0.72).max(1.0);
            let pitch = 3.0f64;
            let bars = ((rect.size.x / pitch).floor() as usize).max(1);
            let peaks = self.peaks.clone();
            let per_bar = peaks.len() as f64 / bars as f64;
            for bar in 0..bars {
                let start = ((bar as f64 * per_bar) as usize).min(peaks.len() - 1);
                let end = (((bar + 1) as f64 * per_bar) as usize)
                    .clamp(start + 1, peaks.len());
                let mut energy = 0.0f64;
                for value in &peaks[start..end] {
                    energy = energy.max((*value as f64).clamp(0.0, 1.0));
                }
                let x = (rect.pos.x + bar as f64 * pitch).round();
                let reach = (energy * half).max(0.5);
                self.draw_bin.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x, (mid - reach).round()),
                        size: dvec2(2.0, (reach * 2.0).max(1.0).round()),
                    },
                );
            }
            let head_x = rect.pos.x + self.fraction * rect.size.x;
            self.draw_head.draw_abs(
                cx,
                Rect {
                    pos: dvec2(head_x - 1.0, rect.pos.y),
                    size: dvec2(2.0, rect.size.y),
                },
            );
        }
        DrawStep::done()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrackListHit {
    /// Row body pressed: pick it. Carries what the keyboard was holding,
    /// which decides whether the press replaces the picks, adds to them, or
    /// extends a range. A press only ever PICKS — the hand that pressed may
    /// still be about to carry the row off to a deck.
    Pick(usize, KeyModifiers),
    /// Row body released where it went down: NOW it is a click, and a deck
    /// target loads it. Carries the modifiers so a set-building release
    /// still loads nothing.
    Load(usize, KeyModifiers),
    /// The row's `+` button: queue it.
    Queue(usize),
    /// The row's headphones button: pre-listen it on the phones bus.
    Preview(usize),
    /// The inline player's play/pause.
    PreviewToggle,
    /// The inline player's ×.
    PreviewClose,
    /// The inline player's A / B: send the pre-listened track to a deck.
    PreviewLoad(DeckId),
    /// The inline player's +: put it at the back of the set.
    PreviewQueue,
    /// A press that has since travelled: the operator is dragging the picked
    /// rows somewhere. Reported once per drag, from the row it started on.
    Drag(usize),
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
    /// A narrow list keeps badge + title and drops every fixed column;
    /// without this the fixed widths squeeze the Fill title to nothing.
    #[live]
    compact: bool,
    /// Pushed by `App::sync_library_density` from the width the list got:
    /// the tick columns shrink in step with the header's S/K.
    #[rust]
    narrow: bool,
    /// Rows the operator has picked, sorted. A pick is not a load: it is
    /// what the hand is about to drag somewhere.
    #[rust]
    selected: Vec<usize>,
    /// Where a shift-range measures from — the last row picked outright.
    #[rust]
    anchor: Option<usize>,
    /// The row riding the pointer during a reorder, outlined so the hand
    /// can see what it holds.
    #[rust]
    carry: Option<usize>,
    /// The track in the phones, if any — keyed by TRACK, so the same song
    /// reads green in the explorer and the queue at once.
    #[rust]
    active_preview: Option<TrackKey>,
    /// PLAYER = INLINE: the previewing row unfolds into the player.
    #[rust]
    inline_on: bool,
    /// What the unfolded player shows, pushed whole by the host's pump.
    #[rust]
    preview_line: PhonesLine,
}

/// The inline player's face, pushed by the host each pump — the list stays
/// a pure view of it.
#[derive(Clone, Default)]
pub struct PhonesLine {
    pub title: String,
    pub time: String,
    pub fraction: f64,
    pub playing: bool,
    pub peaks: Arc<Vec<f32>>,
    /// Already in the set list: the `+` chip stands down.
    pub queued: bool,
}

impl VjTrackList {
    /// Which track is in the phones — `None` unlights every row.
    pub fn set_active_preview(&mut self, cx: &mut Cx, key: Option<TrackKey>) {
        if self.active_preview != key {
            self.active_preview = key;
            self.view.redraw(cx);
        }
    }

    /// Whether the previewing row unfolds into the inline player.
    pub fn set_inline_player(&mut self, cx: &mut Cx, on: bool) {
        if self.inline_on != on {
            self.inline_on = on;
            self.view.redraw(cx);
        }
    }

    /// The inline player's face. Diffed here so the per-frame push only
    /// redraws while something on it actually moves.
    pub fn set_preview_line(&mut self, cx: &mut Cx, line: PhonesLine) {
        let changed = self.preview_line.title != line.title
            || self.preview_line.time != line.time
            || (self.preview_line.fraction - line.fraction).abs() > 1e-4
            || self.preview_line.playing != line.playing
            || self.preview_line.queued != line.queued
            || !Arc::ptr_eq(&self.preview_line.peaks, &line.peaks);
        if changed {
            self.preview_line = line;
            if self.inline_on {
                self.view.redraw(cx);
            }
        }
    }

    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<TrackRowEntry>) {
        if self.entries != entries {
            // A pick belongs to a TRACK, not to a row number. The listing is
            // rebuilt for every badge and status change — a deck loading a
            // track rewrites it — so the picks are carried across by key and
            // only the ones whose track is gone are dropped.
            let picked: Vec<TrackKey> = self
                .selected
                .iter()
                .filter_map(|row| self.entries.get(*row).map(|entry| entry.key.clone()))
                .collect();
            self.entries = entries;
            self.selected = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| picked.contains(&entry.key))
                .map(|(row, _)| row)
                .collect();
            if self.selected.is_empty() {
                self.anchor = None;
            }
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


    /// The picked rows, in list order.
    pub fn selection(&self) -> &[usize] {
        &self.selected
    }

    /// The row currently riding the pointer, or `None` when nothing is
    /// being carried. Drawn as an outline in the carry colour.
    pub fn set_carry(&mut self, cx: &mut Cx, row: Option<usize>) {
        let row = row.filter(|row| *row < self.entries.len());
        if self.carry != row {
            self.carry = row;
            self.view.redraw(cx);
        }
    }

    /// A click on `row`, with whatever the keyboard was holding.
    ///
    /// Plain picks that row alone; ctrl (or cmd) toggles it; shift takes
    /// everything between the anchor and it. The list is the one place that
    /// knows the row order, so the arithmetic lives here rather than in the
    /// caller.
    pub fn click_row(&mut self, cx: &mut Cx, row: usize, modifiers: KeyModifiers) {
        if row >= self.entries.len() {
            return;
        }
        if modifiers.shift {
            let from = self.anchor.unwrap_or(row);
            let (lo, hi) = if from <= row { (from, row) } else { (row, from) };
            self.selected = (lo..=hi).collect();
        } else if modifiers.control || modifiers.logo {
            match self.selected.iter().position(|picked| *picked == row) {
                Some(at) => {
                    self.selected.remove(at);
                }
                None => {
                    self.selected.push(row);
                    self.selected.sort_unstable();
                }
            }
            self.anchor = Some(row);
        } else if self.selected.len() > 1 && self.selected.contains(&row) {
            // A plain press on a row that is ALREADY part of a set keeps the
            // set: the hand is most likely about to carry it somewhere, and
            // a press that collapsed the pick would leave one row in the
            // fist instead of the several the operator chose.
            self.anchor = Some(row);
        } else {
            self.selected = vec![row];
            self.anchor = Some(row);
        }
        self.view.redraw(cx);
    }

    /// Rows that are no longer there cannot stay picked.
    pub fn clear_selection(&mut self, cx: &mut Cx) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.anchor = None;
        self.view.redraw(cx);
    }
    /// The header measured the width; the rows follow it.
    pub fn set_narrow(&mut self, narrow: bool) {
        self.narrow = narrow;
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
            // INLINE placement: the previewing row wears the player
            // template; every other row (and every row in the other two
            // placements) stays the plain one.
            let expanded = if self.inline_on {
                self.entries
                    .iter()
                    .position(|entry| Some(&entry.key) == self.active_preview.as_ref())
            } else {
                None
            };
            while let Some(row_id) = list.next_visible_item(cx) {
                if row_id >= self.entries.len() {
                    continue;
                }
                let template =
                    if expanded == Some(row_id) { id!(TrackRowPlayer) } else { id!(TrackRow) };
                let mut item = list.item(cx, row_id, template);
                if let Some(entry) = self.entries.get(row_id) {
                    item.label(cx, ids!(row_badge)).set_text(cx, &entry.badge);
                    item.label(cx, ids!(row_title)).set_text(cx, &entry.title);
                    item.label(cx, ids!(row_artist)).set_text(cx, &entry.artist);
                    item.label(cx, ids!(row_bpm)).set_text(cx, &entry.bpm);
                    item.label(cx, ids!(row_key)).set_text(cx, &entry.musical_key);
                    item.label(cx, ids!(row_time)).set_text(cx, &entry.duration);
                    item.label(cx, ids!(row_stem))
                        .set_text(cx, if entry.stem { "✓" } else { "" });
                    item.label(cx, ids!(row_krk))
                        .set_text(cx, if entry.krk { "✓" } else { "" });
                    item.label(cx, ids!(row_tags)).set_text(cx, &entry.tags);
                    // The tick columns follow the header's S/K: one
                    // measurement (App::sync_library_density) decides both,
                    // so a narrow console never leaves the marks under a
                    // header that has already shrunk.
                    for (column, wide_width) in
                        [(ids!(row_stem), 36.0), (ids!(row_krk), 30.0)]
                    {
                        let cell = item.widget(cx, column);
                        let mut cell_ref = cell.borrow_mut::<Label>();
                        if let Some(label) = cell_ref.as_mut() {
                            let width =
                                if self.narrow { MARK_COLUMN_NARROW } else { wide_width };
                            label.walk.width = Size::Fixed(width);
                        }
                    }
                    let wide = !self.compact;
                    for column in [
                        ids!(row_artist),
                        ids!(row_bpm),
                        ids!(row_key),
                        ids!(row_time),
                        ids!(row_stem),
                        ids!(row_krk),
                        ids!(row_tags),
                    ] {
                        item.widget(cx, column).set_visible(cx, wide);
                    }
                    item.button(cx, ids!(row_queue))
                        .set_visible(cx, self.show_queue_button);
                    // The phones mark: green while this row's track is the
                    // one being pre-listened. Templated rows are painted
                    // from data here, never through the host's latch cache.
                    let previewing = self.active_preview.as_ref() == Some(&entry.key);
                    let hp_ink: u32 = if previewing { 0x35c05fff } else { 0x9fabb7ff };
                    let hp_edge: u32 = if previewing { 0x35c05f80 } else { 0xffffff26 };
                    let hp_ink = Vec4f::from_u32(hp_ink);
                    let hp_edge = Vec4f::from_u32(hp_edge);
                    let mut hp = item.button(cx, ids!(row_hp));
                    script_apply_eval!(cx, hp, {
                        draw_icon +: { color: #(hp_ink) }
                        draw_bg +: { border_color: #(hp_edge) }
                    });
                    if expanded == Some(row_id) {
                        // The unfolded player: face from the pushed line.
                        // The plain template's bg instances do not exist on
                        // this one, so the stripe apply is skipped whole.
                        item.label(cx, ids!(hp_title)).set_text(cx, &self.preview_line.title);
                        item.label(cx, ids!(hp_time)).set_text(cx, &self.preview_line.time);
                        item.button(cx, ids!(hp_play))
                            .set_visible(cx, !self.preview_line.playing);
                        item.button(cx, ids!(hp_pause))
                            .set_visible(cx, self.preview_line.playing);
                        item.button(cx, ids!(hp_queue))
                            .set_visible(cx, !self.preview_line.queued);
                        let wave = item.widget(cx, ids!(hp_seek));
                        let borrow = wave.borrow_mut::<VjPhonesWave>();
                        if let Some(mut wave) = borrow {
                            if !Arc::ptr_eq(&wave.peaks, &self.preview_line.peaks) {
                                wave.set_peaks(cx, self.preview_line.peaks.clone());
                            }
                            wave.set_fraction(cx, self.preview_line.fraction);
                        }
                    } else {
                        let live = if entry.live { 1.0f32 } else { 0.0 };
                        let odd = if row_id % 2 == 1 { 1.0f32 } else { 0.0 };
                        let sel =
                            if self.selected.contains(&row_id) { 1.0f32 } else { 0.0 };
                        let carry = if self.carry == Some(row_id) { 1.0f32 } else { 0.0 };
                        script_apply_eval!(cx, item, {
                            draw_bg +: { live: #(live) odd: #(odd) sel: #(sel) carry: #(carry) }
                        });
                    }
                }
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

/// How far a press has to travel before it counts as a drag rather than a
/// click. A hand on a trackpad never holds perfectly still.
///
/// This is the ONE threshold: it is where the carry begins and the ghost
/// appears under the cursor, and so it is also where the click dies. What
/// the operator can see — a row now riding the pointer — is exactly what
/// decides whether letting go loads a deck.
pub const TRACK_DRAG_SLOP: f64 = 5.0;

/// Read row clicks out of a frame's actions, the same way the tile grids do.
///
/// `press_travel` is the FARTHEST the live press has been from where it
/// went down, which the host keeps because only it sees the whole gesture.
/// Peak travel, not the release's distance: a carry that goes out and
/// comes back has still been a carry, and letting go over the row it
/// started on must not read as a click on it.
pub fn track_list_hits(
    ui: &WidgetRef,
    cx: &mut Cx,
    path: &[LiveId],
    actions: &Actions,
    press_travel: f64,
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
        let body = item.view(cx, ids!(row_body));
        // A recycled row can surface the same press more than once; one
        // press is one load, whatever the list reports.
        let hit = if item.button(cx, ids!(row_hp)).clicked(actions) {
            TrackListHit::Preview(row_id)
        } else if item.button(cx, ids!(row_queue)).clicked(actions) {
            TrackListHit::Queue(row_id)
        } else if item.button(cx, ids!(hp_play)).clicked(actions)
            || item.button(cx, ids!(hp_pause)).clicked(actions)
        {
            // Only the unfolded (inline player) row has these; on every
            // other row the refs are empty and never click.
            TrackListHit::PreviewToggle
        } else if item.button(cx, ids!(hp_close)).clicked(actions) {
            TrackListHit::PreviewClose
        } else if item.button(cx, ids!(hp_load_a)).clicked(actions) {
            TrackListHit::PreviewLoad(DeckId::A)
        } else if item.button(cx, ids!(hp_load_b)).clicked(actions) {
            TrackListHit::PreviewLoad(DeckId::B)
        } else if item.button(cx, ids!(hp_queue)).clicked(actions) {
            TrackListHit::PreviewQueue
        } else if let Some(down) = body.finger_down(actions) {
            TrackListHit::Pick(row_id, down.modifiers)
        } else if let Some(moved) = body.finger_move(actions) {
            // Travelled far enough from where the finger went down: the
            // operator is carrying the picked rows, not choosing one.
            if (moved.abs - moved.abs_start).length() < TRACK_DRAG_SLOP {
                continue;
            }
            TrackListHit::Drag(row_id)
        } else if let Some(up) = body.finger_up(actions) {
            // The click lands on the RELEASE, and only for a press that
            // never became a carry. Once the ghost is out, the drop decides
            // where those rows go — loading on the way past is how one drag
            // used to cue a deck the operator never aimed at.
            if press_travel >= TRACK_DRAG_SLOP {
                continue;
            }
            TrackListHit::Load(row_id, up.modifiers)
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

/// Tempo slider readout, signed percent.
pub fn format_pitch(pitch: f64) -> String {
    format!("{:+.1}%", pitch * 100.0)
}

/// Key-shift readout, in signed whole semitones. An em-dash at zero: an
/// untransposed deck should read as plainly untouched, not as "+0".
pub fn format_key_shift(semitones: f64) -> String {
    let steps = semitones.round() as i64;
    if steps == 0 {
        "—".to_string()
    } else {
        format!("{steps:+}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The group widths the strip measures, near enough: shaping,
    /// automation and one cue key, with the themed spacing between them.
    fn plan_at(row: f64) -> StripPlan {
        strip_sweep_plan(row, 380.0, 290.0, 72.0, 6.0)
    }

    #[test]
    fn the_sweep_is_never_wider_than_its_cap_at_any_console_width() {
        // Every width the window can be dragged to, a tenth of a point at
        // a time. The cap is a promise about the fader, so it cannot hold
        // only at the width that happens to be on screen.
        let mut over = Vec::new();
        for step in 0..40_000 {
            let row = step as f64 * 0.1;
            let plan = plan_at(row);
            if plan.sweep_w > STRIP_SWEEP_MAX {
                over.push((row, plan.sweep_w));
            }
        }
        assert!(over.is_empty(), "sweep over {STRIP_SWEEP_MAX}: {:?}", &over[..over.len().min(4)]);
        // And with the groups at other sizes, since the strip measures them
        // rather than carrying numbers.
        for shaping in [0.0, 120.0, 380.0, 900.0] {
            for automation in [0.0, 90.0, 290.0, 700.0] {
                for cue in [0.0, 40.0, 72.0, 200.0] {
                    for step in 0..2_000 {
                        let row = step as f64 * 2.0;
                        let plan = strip_sweep_plan(row, shaping, automation, cue, 6.0);
                        assert!(
                            plan.sweep_w <= STRIP_SWEEP_MAX,
                            "row {row} groups {shaping}/{automation}/{cue} gave {}",
                            plan.sweep_w
                        );
                        assert!(plan.sweep_w >= 0.0, "row {row} gave a negative sweep");
                    }
                }
            }
        }
        // A turtle that cannot say how wide the row is falls back to the
        // cap itself, which is still the cap.
        assert!(plan_at(f64::INFINITY).sweep_w <= STRIP_SWEEP_MAX);
    }

    /// One frame of the strip's own feedback loop, played out on paper.
    ///
    /// The strip decides the layout from what it remembers of its groups,
    /// draws them, and learns their widths from that drawing — so the
    /// drawing it chose decides what it learns. A group let draw FREE
    /// reports what it naturally comes to; one the strip WALKED reports the
    /// width it was walked to.
    fn strip_frame(
        remembered: (f64, f64, f64),
        natural: (f64, f64),
        row: f64,
    ) -> ((f64, f64, f64), StripPlan) {
        let (shaping_w, automation_w, cue_w) = remembered;
        let plan = strip_sweep_plan(row, shaping_w, automation_w, cue_w, 6.0);
        let shaping_free = plan.flanked || shaping_w <= plan.row;
        let automation_free = plan.flanked || automation_w <= plan.row;
        let measures = |free: bool, nat: f64| if free { nat } else { plan.row };
        (
            (
                remembered_width(shaping_w, measures(shaping_free, natural.0), shaping_free),
                remembered_width(
                    automation_w,
                    measures(automation_free, natural.1),
                    automation_free,
                ),
                // The cue keys are drawn free whenever they are visible, and
                // measure zero when they are not.
                remembered_width(cue_w, if plan.cues_inline { cue_w } else { 0.0 }, plan.cues_inline),
            ),
            plan,
        )
    }

    #[test]
    fn the_strip_settles_instead_of_flipping_between_two_layouts() {
        // The strip asks for a redraw while its measurements disagree with
        // what it laid out from, so a loop that never reaches a fixed point
        // is not a slow settle — it is a repaint every frame, for as long as
        // the window stays that width. Which is what the narrow console did:
        // walked shaping measures the row, the row looks like it fits, free
        // shaping measures too wide again, two frames a cycle, forever.
        let natural = (380.0, 290.0);
        for step in 0..4_000 {
            let row = 20.0 + step as f64 * 0.5;
            let mut remembered = (380.0, 290.0, 72.0);
            let mut plans = Vec::new();
            for _ in 0..24 {
                let (next, plan) = strip_frame(remembered, natural, row);
                remembered = next;
                plans.push((remembered, plan));
            }
            // Whatever it started from, the last frames must be identical to
            // each other: same remembered widths, same layout, same sweep.
            let (settled_mem, settled_plan) = plans[plans.len() - 1];
            for (mem, plan) in &plans[plans.len() - 6..] {
                assert_eq!(
                    (*mem, *plan),
                    (settled_mem, settled_plan),
                    "row {row} never settled: {:?}",
                    &plans[plans.len() - 6..]
                );
            }
        }
    }

    #[test]
    fn a_walked_group_never_reports_the_strips_own_number_back_to_it() {
        // 300 points of row, a group that wants 380: the width the old rule
        // oscillated at. The group is walked, so what it measures is the
        // strip's own 300 — and the strip must not take that for the
        // group's own width, or next frame it "fits".
        assert_eq!(remembered_width(380.0, 300.0, false), 380.0);
        // Drawn free, the measurement is the group speaking, and it is kept
        // even when it shrank.
        assert_eq!(remembered_width(380.0, 372.0, true), 372.0);
        // A hidden group measures nothing and is not news either.
        assert_eq!(remembered_width(72.0, 0.0, true), 72.0);

        // End to end, at that width: one frame to learn, then still.
        let mut remembered = (380.0, 290.0, 72.0);
        let first = strip_frame(remembered, (380.0, 290.0), 300.0);
        remembered = first.0;
        assert_eq!(remembered.0, 380.0, "the walked group kept its own width");
        for _ in 0..8 {
            let (next, plan) = strip_frame(remembered, (380.0, 290.0), 300.0);
            assert_eq!(next, remembered, "settled, and staying settled");
            assert_eq!(plan, first.1, "and the layout with it");
            remembered = next;
        }
    }

    #[test]
    fn the_cap_binds_the_row_the_sweep_owns_outright() {
        // The window width that used to stretch it: too narrow for one
        // line, wide enough that the sweep's own row ran past the cap.
        let plan = plan_at(900.0);
        assert!(!plan.flanked, "900 points cannot hold both groups and a sweep");
        assert!(plan.cues_inline, "the cue keys still fit beside it");
        assert_eq!(plan.sweep_w, STRIP_SWEEP_MAX, "capped, with the rest left as air");

        // Short of the cap the sweep still takes what the row gives it, so
        // the ceiling never becomes a floor.
        let row = STRIP_SWEEP_MAX + 100.0;
        let plan = plan_at(row);
        assert!(!plan.flanked);
        assert!(plan.sweep_w < STRIP_SWEEP_MAX, "{} should be short of the cap", plan.sweep_w);
        assert_eq!(plan.sweep_w, row - (2.0 * 72.0 + 2.0 * 6.0) - STRIP_ROW_SLACK);
    }

    #[test]
    fn a_narrow_strip_drops_the_cue_keys_and_a_wide_one_flanks() {
        // Wide: one line, groups either side, the sweep at whatever is left
        // up to the cap.
        let wide = plan_at(1650.0);
        assert!(wide.flanked && wide.cues_inline);
        assert!(wide.sweep_w >= STRIP_SWEEP_MIN, "{} is not worth playing", wide.sweep_w);

        // Narrow: nothing flanks, the cue keys go, and the sweep has the
        // whole row.
        let narrow = plan_at(200.0);
        assert!(!narrow.flanked && !narrow.cues_inline);
        assert_eq!(narrow.sweep_w, 200.0);
    }

    use super::*;
    use crate::mixer::TrackPcm;

    #[test]
    fn the_key_readout_signs_its_semitones_and_dashes_at_home() {
        assert_eq!(format_key_shift(0.0), "—");
        assert_eq!(format_key_shift(3.0), "+3");
        assert_eq!(format_key_shift(-2.0), "-2");
        assert_eq!(format_key_shift(12.0), "+12");
    }

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
    fn the_loop_maps_to_columns_on_the_tile_timebase() {
        let mut lane = lane(120.0, 0.0);
        lane.loop_span = Some((1.0, 3.0));
        let (start, end) = lane.loop_columns().expect("a loop");
        assert!((start - 100.0).abs() < 1e-9, "1 s in = column 100");
        assert!((end - 300.0).abs() < 1e-9, "3 s in = column 300");
        // No span, nothing to draw — the shader reads that as off.
        lane.loop_span = None;
        assert!(lane.loop_columns().is_none());
        // A span with no length is off too, rather than a zero-width sliver
        // the edge rules would still draw on top of each other.
        lane.loop_span = Some((2.0, 2.0));
        assert!(lane.loop_columns().is_none());
    }

    #[test]
    fn the_band_is_grabbable_along_its_length_and_not_outside_it() {
        let span = Some((10.0, 14.0));
        assert_eq!(band_grab(span, 10.0, 0.35), Some(0.0), "the in edge grabs at zero");
        assert_eq!(band_grab(span, 12.0, 0.35), Some(2.0), "the middle grabs at its offset");
        assert!(band_grab(span, 30.0, 0.35).is_none(), "well clear of it is a seek");
        assert!(band_grab(span, 0.5, 0.35).is_none());
    }

    #[test]
    fn a_thin_band_is_still_grabbable_through_the_tolerance() {
        // A one-beat loop at 120 BPM is 0.5 s — under two pixels on a whole
        // track strip, so the hit test has to be forgiving or the band is
        // uncatchable at exactly the sizes the loop cutter produces.
        let span = Some((10.0, 10.5));
        assert!(band_grab(span, 10.25, 0.35).is_some(), "dead centre must hit");
        assert!(band_grab(span, 10.7, 0.35).is_some(), "just past OUT is still the band");
        assert!(band_grab(span, 12.0, 0.35).is_none(), "far past it is a seek again");
    }

    #[test]
    fn the_grab_offset_never_exceeds_the_band() {
        // Grabbing in the tolerance margin past OUT must not report an
        // offset longer than the loop, or the drag would place IN beyond
        // where the finger is.
        let span = Some((10.0, 10.5));
        assert_eq!(band_grab(span, 10.8, 0.35), Some(0.5), "clamped to the length");
        assert_eq!(band_grab(span, 9.8, 0.35), Some(0.0), "clamped at the in edge");
    }

    #[test]
    fn no_span_means_every_grab_is_a_seek() {
        assert!(band_grab(None, 12.0, 0.35).is_none());
    }

    #[test]
    fn marker_clicks_resolve_nearest_and_blue_beats_green_beats_red() {
        let saved = [(10.0, 12.0), (30.0, 31.0)];
        // Near a blue marker: recall it, nearest one on a tie of tolerance.
        assert_eq!(marker_hit(&saved, None, 0.0, 10.2, 0.35), Some(MarkerHit::Recall(0)));
        assert_eq!(marker_hit(&saved, None, 0.0, 29.8, 0.35), Some(MarkerHit::Recall(1)));
        // Near the green (running) IN and nothing blue: save.
        assert_eq!(marker_hit(&saved, Some(50.0), 0.0, 50.1, 0.35), Some(MarkerHit::Save));
        // Green sitting on a saved IN: the blue meaning wins.
        assert_eq!(
            marker_hit(&saved, Some(10.0), 0.0, 10.0, 0.35),
            Some(MarkerHit::Recall(0))
        );
        // The red cue chip is the quietest voice: it answers only when
        // nothing louder is in reach.
        assert_eq!(marker_hit(&saved, None, 20.0, 20.1, 0.35), Some(MarkerHit::Cue));
        assert_eq!(marker_hit(&saved, Some(20.2), 20.0, 20.1, 0.35), Some(MarkerHit::Save));
        // Clear of everything: no marker business at all.
        assert_eq!(marker_hit(&saved, Some(50.0), 0.0, 40.0, 0.35), None);
    }

    #[test]
    fn found_marker_clicks_resolve_nearest_in_the_bottom_band() {
        let found = [(12.0, 20.0), (40.0, 48.0)];
        assert_eq!(found_marker_hit(&found, 12.2, 0.35), Some(MarkerHit::Found(0)));
        assert_eq!(found_marker_hit(&found, 39.8, 0.35), Some(MarkerHit::Found(1)));
        assert_eq!(found_marker_hit(&found, 30.0, 0.35), None);
        // A tie resolves to the nearest IN, not the first.
        let tight = [(10.0, 12.0), (10.5, 14.0)];
        assert_eq!(found_marker_hit(&tight, 10.45, 0.35), Some(MarkerHit::Found(1)));
    }

    #[test]
    fn the_preview_steps_in_whole_units_against_the_ghost() {
        let g = grid(120.0, 0.25, 0); // 0.5 s a beat
        let span = Some((10.25, 12.25));
        let p = move_preview(span, 30.4, Some(g), 4, 300.0).expect("a preview");
        let steps = (p.0 - 10.25) / (4.0 * 0.5);
        assert!((steps - steps.round()).abs() < 1e-9, "moved {steps} bars");
        assert!((p.1 - p.0 - 2.0).abs() < 1e-9, "the length must survive the move");
    }

    #[test]
    fn the_preview_is_exact_when_snap_is_off_or_the_grid_is_missing() {
        let g = grid(120.0, 0.25, 0);
        let span = Some((10.0, 12.0));
        assert_eq!(move_preview(span, 30.4, Some(g), 0, 300.0), Some((30.4, 32.4)));
        assert_eq!(move_preview(span, 30.4, None, 4, 300.0), Some((30.4, 32.4)));
    }

    #[test]
    fn the_preview_refuses_to_leave_the_track() {
        let span = Some((10.0, 12.0));
        assert!(move_preview(span, 299.5, None, 0, 300.0).is_none(), "off the end");
        assert!(move_preview(span, -1.0, None, 0, 300.0).is_none(), "off the front");
        assert!(move_preview(None, 30.0, None, 0, 300.0).is_none(), "no span at all");
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

    // ---- the height law ----------------------------------------------------
    //
    // A column is as tall as the music is loud there, measured once against
    // the whole track. Colour — bands, stems, knobs — divides that height
    // up; nothing is allowed to set it.

    /// One stem of the fixture: a steady tone, quiet for the first half of
    /// the track and loud for the second, so the same audio appears at two
    /// levels twenty decibels apart.
    fn stem_tone(rate: u32, secs: f64, hz: f64, gain: f64, quiet_gain: f64) -> Vec<[i16; 2]> {
        let len = (rate as f64 * secs) as usize;
        let half = len / 2;
        (0..len)
            .map(|index| {
                let time = index as f64 / rate as f64;
                let level = if index < half { quiet_gain } else { 1.0 };
                let value = gain * level * (2.0 * std::f64::consts::PI * hz * time).sin();
                let sample = (value * 30_000.0) as i16;
                [sample, sample]
            })
            .collect()
    }

    /// The four stems and the track they add up to: a quiet half at -20 dB
    /// and a loud half, identical in content.
    fn quiet_then_loud(rate: u32, secs: f64) -> (TrackPcm, [Vec<[i16; 2]>; 4]) {
        let quiet = 0.1;
        let stems = [
            stem_tone(rate, secs, 900.0, 0.22, quiet), // vocals
            stem_tone(rate, secs, 3_500.0, 0.30, quiet), // drums
            stem_tone(rate, secs, 60.0, 0.40, quiet),  // bass
            stem_tone(rate, secs, 220.0, 0.14, quiet), // other
        ];
        let len = stems[0].len();
        let mut frames = vec![[0i16; 2]; len];
        for index in 0..len {
            let sum: i32 = stems.iter().map(|stem| stem[index][0] as i32).sum();
            let sample = sum.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            frames[index] = [sample, sample];
        }
        (TrackPcm { frames, sample_rate: rate }, stems)
    }

    /// RMS of one stem over the frames one zoom column covers.
    fn column_rms(stem: &[[i16; 2]], rate: u32, column: usize) -> f64 {
        let frames_per_col = rate as f64 / ZOOM_COLS_PER_SEC;
        let from = (column as f64 * frames_per_col) as usize;
        let to = (((column + 1) as f64 * frames_per_col) as usize).min(stem.len());
        if from >= to {
            return 0.0;
        }
        let sum: f64 = stem[from..to]
            .iter()
            .map(|frame| {
                let mono = (frame[0] as f64 + frame[1] as f64) * 0.5 / 32_768.0;
                mono * mono
            })
            .sum();
        (sum / (to - from) as f64).sqrt()
    }

    /// The tiles of the fixture, plus a column well inside the quiet half
    /// and one well inside the loud half.
    fn fixture() -> (WaveTiles, [Vec<[i16; 2]>; 4], u32, usize, usize) {
        let rate = 44_100u32;
        let secs = 20.0;
        let (pcm, stems) = quiet_then_loud(rate, secs);
        let analysis = crate::wave_analysis::analyze(&pcm);
        let cols = analysis.tiles.zoom.len();
        assert!(cols > 1_000, "{cols} columns");
        // A quarter and three quarters in: the middle of each half.
        (analysis.tiles, stems, rate, cols / 4, cols * 3 / 4)
    }

    #[test]
    fn a_quiet_intro_draws_short_and_the_drop_draws_tall() {
        let (tiles, _stems, _rate, quiet, loud) = fixture();
        let quiet_h = column_height(tiles.zoom[quiet]);
        let loud_h = column_height(tiles.zoom[loud]);
        assert!(loud_h > 0.6, "the drop should nearly fill the lane, got {loud_h}");
        assert!(
            quiet_h < 0.3 * loud_h,
            "a -20 dB intro drew {quiet_h} against a drop of {loud_h}"
        );
        // And nothing draws past the track's own reference level.
        for column in &tiles.zoom {
            assert!(column_height(*column) <= WAVE_ENVELOPE + 1e-6);
        }
    }

    #[test]
    fn the_stems_partition_the_column_and_never_scale_it() {
        let (tiles, stems, rate, quiet, loud) = fixture();
        for column in [quiet, loud] {
            let rms = [
                column_rms(&stems[0], rate, column),
                column_rms(&stems[1], rate, column),
                column_rms(&stems[2], rate, column),
                column_rms(&stems[3], rate, column),
            ];
            let shares = stem_column_shares(rms);
            let tile = tiles.zoom[column];
            let stack = stem_stack(tile, shares, [1.0; 4]);
            // Every knob up: the coloured column is EXACTLY as tall as the
            // grey one. This is the seam.
            let height = column_height(tile);
            assert!(
                (stack[3] - height).abs() <= 1e-5,
                "column {column}: coloured {} vs grey {height}",
                stack[3]
            );
            // The layers stack outward, none of them inverted.
            assert!(stack[0] <= stack[1] && stack[1] <= stack[2] && stack[2] <= stack[3]);
            // Each stem is present in proportion to what it contributes:
            // the bass tone is the loudest lane, so it owns the core.
            assert!(stack[0] > 0.2 * stack[3], "bass core {} of {}", stack[0], stack[3]);
            // Killing a stem takes away its share and nothing else.
            let killed = stem_stack(tile, shares, [1.0, 1.0, 0.0, 1.0]);
            let bass = stack[0];
            assert!(
                (killed[3] - (stack[3] - bass)).abs() <= 1e-5,
                "killing the bass changed the rest: {} vs {}",
                killed[3],
                stack[3] - bass
            );
            // And no knob can make a column taller than its level.
            assert!(stack[3] <= column_height(tile) + 1e-6);
        }
    }

    #[test]
    fn the_seam_holds_and_the_coloured_half_keeps_its_dynamics() {
        let (tiles, stems, rate, quiet, loud) = fixture();
        // A lane is about 120 device pixels of half-height; a pixel is
        // therefore this much of the envelope.
        let pixel = 1.0 / 120.0;
        let mut heights = Vec::new();
        for column in [quiet, loud] {
            let rms = [
                column_rms(&stems[0], rate, column),
                column_rms(&stems[1], rate, column),
                column_rms(&stems[2], rate, column),
                column_rms(&stems[3], rate, column),
            ];
            let tile = tiles.zoom[column];
            let coloured = stem_stack(tile, stem_column_shares(rms), [1.0; 4])[3];
            let grey = column_height(tile);
            assert!(
                (coloured - grey).abs() <= pixel,
                "seam jumps by {} at column {column}",
                (coloured - grey).abs()
            );
            heights.push(coloured);
        }
        // The separated picture has the same dynamics as the raw one: the
        // quiet half is short there too, which is the bug this guards.
        assert!(
            heights[0] < 0.3 * heights[1],
            "separated intro {} against separated drop {}",
            heights[0],
            heights[1]
        );
    }

    #[test]
    fn a_loud_stem_cannot_lift_a_quiet_column() {
        // The old failure: four stems each normalized to their own scale,
        // stacked, and clamped — every busy column filled the lane. A
        // column that is a quarter of the track's level draws a quarter of
        // the height however loud its four stems are relative to each other.
        let quiet_tile = [80u8, 90, 70, 64];
        let full_stems = stem_column_shares([0.30, 0.30, 0.30, 0.30]);
        assert_eq!(full_stems, [255; 4], "an even column is an even split");
        let stack = stem_stack(quiet_tile, full_stems, [1.0; 4]);
        let height = column_height(quiet_tile);
        assert!((stack[3] - height).abs() <= 1e-5, "{} vs {height}", stack[3]);
        assert!(height < 0.26, "a quarter-level column drew {height}");
        // Each of the four owns a quarter of it.
        assert!((stack[0] - height * 0.25).abs() <= 1e-5);
        // Silence that has been separated stays silent rather than falling
        // back to the grey colouring.
        assert_eq!(stem_column_shares([0.0; 4]), [1; 4]);
        let silent = stem_stack([0, 0, 0, 0], [1; 4], [1.0; 4]);
        assert_eq!(silent, [0.0; 4]);
    }
}
