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

use crate::columns::{Column, ColumnWidth};
use crate::decks::DeckId;
use crate::loop_splat_view::{DrawSplatBlock, DrawSplatCell, VjLoopSplat};
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

/// How far a phrase stub stays off each end of the strip: the chip rows
/// are 11 points tall at rest and grow to 17 under the pointer, so this is
/// the grown height and the stub is clear even while a chip is being read.
pub const CHANGE_CLEAR: f64 = 18.0;

/// The closest two turns may be drawn. A build-up can turn every few
/// seconds, which on a whole-track strip is a hatch that hides the drop it
/// is marking; past this they are one turn, which is what they look like.
pub const CHANGE_MIN_PX: f64 = 5.0;

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

const DECK_ACCENTS: [[f32; 4]; 2] = [
    [1.0, 0.361, 0.224, 1.0],
    [0.416, 0.659, 1.0, 1.0],
];

fn deck_accent(deck: DeckId) -> Vec4f {
    let c = DECK_ACCENTS[deck.index()];
    vec4(c[0], c[1], c[2], c[3])
}

fn set_loop_color_uniform(lane: &mut DrawWaveLane, cx: &Cx2d, color: Vec4f) {
    lane.draw_vars
        .set_uniform(cx, live_id!(color_loop), &[color.x, color.y, color.z, 0.18]);
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

/// How hard this lane's end-of-track warning is showing. Pushed EVERY
/// draw like the spans above: a lane that has stopped warning has to say
/// so, or the last value sticks to whatever is drawn next.
fn set_warn_uniform(lane: &mut DrawWaveLane, cx: &Cx2d, warn: f32) {
    lane.draw_vars.set_uniform(cx, live_id!(warn), &[warn.clamp(0.0, 1.0)]);
}

fn set_head_fraction_uniform(lane: &mut DrawWaveLane, cx: &Cx2d, fraction: f32) {
    lane.draw_vars.set_uniform(cx, live_id!(head_fraction), &[fraction]);
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
    /// The saved loop's own NUMBER, not its position in the strip's list:
    /// a swap or a delete moves the positions and the number stays with
    /// the loop. `Found` beside it keeps an index, because a scanner
    /// finding has no identity to keep.
    Recall(u16),
    /// One of the shape's four edges: where the intro ends, where the
    /// outro starts, and the two the recording itself gives. Carried as
    /// its own index rather than a kind, because the strip needs only to
    /// say WHICH of the four the wheel is over.
    Shape(u8),
    /// The red marker: drag to move where CUE lands.
    Cue,
    /// A yellow marker on the BOTTOM edge: a scanner-found loop.
    Found(usize),
}

/// Resolve a click at `secs` against the markers. Blue wins over green on
/// overlap — recalling a saved loop is the deliberate act; saving it again
/// would be a no-op anyway. Nearest within `tol` takes it.
fn marker_hit(
    saved: &[(u16, f64, f64, u32)],
    running_in: Option<f64>,
    cue_secs: f64,
    secs: f64,
    tol: f64,
) -> Option<MarkerHit> {
    let nearest_saved = saved
        .iter()
        .map(|entry| (entry.0, (entry.1 - secs).abs()))
        .filter(|(_, distance)| *distance <= tol)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((slot, _)) = nearest_saved {
        return Some(MarkerHit::Recall(slot));
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

/// The time a hovered mark sits at, so the hover readout can show WHEN
/// as well as WHERE. A shape edge is structure, not a placed mark, and
/// reads nothing here.
fn mark_time(
    hit: MarkerHit,
    loop_span: Option<(f64, f64)>,
    loop_slots: &[(u16, f64, f64, u32)],
    found_loops: &[(f64, f64)],
    cue_secs: f64,
) -> Option<f64> {
    match hit {
        MarkerHit::Save => loop_span.map(|(start, _)| start),
        MarkerHit::Recall(slot) => {
            loop_slots.iter().find(|entry| entry.0 == slot).map(|entry| entry.1)
        }
        MarkerHit::Found(index) => found_loops.get(index).map(|span| span.0),
        MarkerHit::Cue => Some(cue_secs),
        MarkerHit::Shape(_) => None,
    }
}

/// Whether a seek release at `finger_y` has left the strip's own band by
/// more than the abort margin — above or below it, not along it. The
/// strip is thin and sits among other controls, so a hand lifting off to
/// reach for something else should not also relocate the playhead.
fn seek_release_aborted(finger_y: f64, strip_y: f64, strip_h: f64) -> bool {
    finger_y < strip_y - SEEK_ABORT_PX || finger_y > strip_y + strip_h + SEEK_ABORT_PX
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
/// A seek drag released this far off the strip -- above or below it,
/// not along it -- does not commit. The strip is thin and sits among
/// other controls; a hand that lifted off to reach for something else
/// should not also relocate the playhead.
const SEEK_ABORT_PX: f64 = 60.0;
/// How far one notch of the wheel moves a mark it is hovering.
///
/// Ten milliseconds is about a third of the shortest gap a hand can hear
/// as separate, so a notch is a correction rather than a move; shift
/// takes it down to a millisecond, which is under a sample at any rate
/// this tab renders and is there for the last hair.
const MARK_NUDGE_SECS: f64 = 0.010;
const MARK_NUDGE_FINE_SECS: f64 = 0.001;
/// What one detent of a wheel reports. The fader ladder divides by the
/// same number.
const WHEEL_NOTCH: f64 = 120.0;

/// Whole notches only, with the remainder kept for the next event: a
/// trackpad sends a continuous stream and a mark must move in steps a
/// hand can count, not slide under it.
fn nudge_delta(scroll: DVec2, fine: bool, residue: &mut f64) -> f64 {
    let axis = if scroll.y != 0.0 { -scroll.y } else { -scroll.x };
    if !axis.is_finite() {
        return 0.0;
    }
    *residue += axis / WHEEL_NOTCH;
    let notches = residue.trunc();
    *residue -= notches;
    notches * if fine { MARK_NUDGE_FINE_SECS } else { MARK_NUDGE_SECS }
}

/// Where inside the loop band `secs` landed, or `None` if it did not. The
/// offset is what makes a drag feel pinned: the band travels with the
/// finger instead of snapping its in point under the cursor.
/// Which END of the running band a press landed on, if either.
///
/// The interior keeps the whole-band move it has always had; only the
/// last few pixels at each end resize. A band shorter than two grab
/// zones has no interior to move by, so its ends win -- a one-beat loop
/// is exactly the one you want to be able to stretch.
fn band_edge(span: Option<(f64, f64)>, secs: f64, tolerance_secs: f64) -> Option<bool> {
    let (start, end) = span?;
    if secs < start - tolerance_secs || secs > end + tolerance_secs {
        return None;
    }
    let near_in = (secs - start).abs() <= tolerance_secs;
    let near_out = (secs - end).abs() <= tolerance_secs;
    match (near_in, near_out) {
        // Inside a very short band both are in reach: the nearer wins.
        (true, true) => Some((secs - start) > (end - secs)),
        (true, false) => Some(false),
        (false, true) => Some(true),
        (false, false) => None,
    }
}

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

/// How long before the end of a record the lane starts warning, in
/// seconds. Half a minute: long enough to find the next track and get it
/// on a deck without hurrying, short enough that it is not lit through
/// most of an outro. 0.0 is the operator's off switch (see `warn_at`).
pub const WARN_SECS_MIN: f64 = 0.0;
pub const WARN_SECS_MAX: f64 = 90.0;
pub const WARN_SECS_DEFAULT: f64 = 30.0;

/// Zoom limits, seconds of audio across the full lane width.
pub const ZOOM_MIN_SECS: f64 = 1.5;
pub const ZOOM_MAX_SECS: f64 = 10.0;
pub const ZOOM_DEFAULT_SECS: f64 = 8.0;

/// Where the playhead sits across the lane's width. Kept well off both
/// edges -- a head pinned at 0 or 1 would show only history or only
/// lookahead, which is a scroll, not a deck.
pub const HEAD_FRACTION_MIN: f64 = 0.15;
pub const HEAD_FRACTION_MAX: f64 = 0.85;
pub const HEAD_FRACTION_DEFAULT: f64 = 0.5;
/// A pointer that has not moved for this long is holding the record still.
const SCRATCH_IDLE_SECS: f64 = 0.045;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawSplatCell::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_mix: texture_2d(float)
        tex_stems: texture_2d(float)
        time: uniform(0.0)

        mix_level_at: fn(column: float, base_row: float, level_cols: float, scale: float) -> vec4 {
            let c = clamp(floor(column / scale), 0.0, max(level_cols - 1.0, 0.0))
            let wrap = floor(c / self.tex_w)
            let u = (c - wrap * self.tex_w + 0.5) / self.tex_w
            let v = (base_row + wrap + 0.5) / self.tex_h
            return self.tex_mix.sample_as_bgra(vec2(u, v))
        }

        mix_span: fn(column: float) -> vec4 {
            let lo = self.mix_level_at(column, self.lo_row, self.lo_cols, self.lo_scale)
            if self.lod_blend <= 0.0 {
                if self.lo_scale <= 1.0 {
                    let base = floor(column - 0.5)
                    let f = column - 0.5 - base
                    let a = self.mix_level_at(base, self.lo_row, self.lo_cols, 1.0)
                    let b = self.mix_level_at(base + 1.0, self.lo_row, self.lo_cols, 1.0)
                    return a * (1.0 - f) + b * f
                }
                return lo
            }
            let hi = self.mix_level_at(column, self.hi_row, self.hi_cols, self.hi_scale)
            return lo * (1.0 - self.lod_blend) + hi * self.lod_blend
        }

        stem_level_at: fn(column: float, base_row: float, level_cols: float, scale: float) -> vec4 {
            let c = clamp(floor(column / scale), 0.0, max(level_cols - 1.0, 0.0))
            let wrap = floor(c / self.tex_w)
            let u = (c - wrap * self.tex_w + 0.5) / self.tex_w
            let v = (base_row + wrap + 0.5) / self.tex_h
            return self.tex_stems.sample_as_bgra(vec2(u, v))
        }

        stem_span: fn(column: float) -> vec4 {
            let lo = self.stem_level_at(column, self.lo_row, self.lo_cols, self.lo_scale)
            if self.lod_blend <= 0.0 {
                return lo
            }
            let hi = self.stem_level_at(column, self.hi_row, self.hi_cols, self.hi_scale)
            return lo * (1.0 - self.lod_blend) + hi * self.lod_blend
        }

        playing_progress: fn(base: vec4) -> vec4 {
            let playing = step(3.5, self.state)
            let p = self.pos * self.rect_size
            let w = self.rect_size.x
            let h = self.rect_size.y
            let x0 = clamp(self.part.x * w, 0.0, w)
            let x1 = clamp(self.part.y * w, x0, w)
            let y0 = clamp(self.part.z * h, 0.0, h)
            let y1 = clamp(self.part.w * h, y0, h)
            let inside_y = step(y0 + 2.0, p.y) * step(p.y, y1 - 2.0)
            let play_x = x0 + 2.0
                + clamp(self.phase, 0.0, 1.0) * max(x1 - x0 - 4.0, 0.0)
            let played = step(x0 + 2.0, p.x) * step(p.x, play_x) * inside_y * playing
            let filled = base.mix(vec4(1.0, 1.0, 1.0, 1.0), played * 0.18)
            let distance = abs(p.x - play_x)
            let edge = (1.0 - smoothstep(1.5, 2.0, distance)) * inside_y * playing
            let line = (1.0 - smoothstep(0.75, 1.0, distance)) * inside_y * playing
            let edged = filled.mix(vec4(0.0, 0.0, 0.0, 1.0), edge * 0.55)
            return edged.mix(vec4(1.0, 1.0, 1.0, 1.0), line * 0.95)
        }

        countdown: fn(base: vec4) -> vec4 {
            let armed = step(2.5, self.state) - step(3.5, self.state)
            let p = self.pos * self.rect_size
            let w = self.rect_size.x
            let h = self.rect_size.y
            let x0 = clamp(self.part.x * w, 0.0, w)
            let x1 = clamp(self.part.y * w, x0, w)
            let y0 = clamp(self.part.z * h, 0.0, h)
            let y1 = clamp(self.part.w * h, y0, h)
            let fill_x = x0 + 3.0
                + clamp(self.bar_phase, 0.0, 1.0) * max(x1 - x0 - 6.0, 0.0)
            let strip = step(y0 + 3.0, p.y) * step(p.y, min(y0 + 6.0, y1 - 2.0))
                * step(x0 + 3.0, p.x) * step(p.x, x1 - 3.0) * armed
            let filled = strip * step(p.x, fill_x)
            let track = base.mix(vec4(0.0, 0.0, 0.0, 1.0), strip * 0.45)
            return track.mix(vec4(1.0, 1.0, 1.0, 1.0), filled * 0.95)
        }

        pixel: fn() {
            let w = self.rect_size.x
            let h = self.rect_size.y
            let p = self.pos * self.rect_size
            let sdf = Sdf2d.viewport(p)
            let rgb = self.color.xyz
            let part_x0 = clamp(self.part.x * w, 0.0, w)
            let part_x1 = clamp(self.part.y * w, part_x0, w)
            let part_y0 = clamp(self.part.z * h, 0.0, h)
            let part_y1 = clamp(self.part.w * h, part_y0, h)
            sdf.box(1.0, 1.0, w - 2.0, h - 2.0, 3.0)
            if self.state < 0.5 {
                sdf.stroke(vec4(rgb.x, rgb.y, rgb.z, 0.10), 1.0)
                return sdf.result
            }

            let is_mix = step(3.5, self.channel)
            let wave_available = self.has_mix
                * mix(self.has_stems, 1.0, is_mix)
                * step(0.001, self.span_cols)
            if wave_available < 0.5 {
                // Analysis or stems are still pending: retain the flat state fill.
                if self.state < 1.5 {
                    sdf.stroke(vec4(rgb.x, rgb.y, rgb.z, 0.25), 1.0)
                } else if self.state < 2.5 {
                    sdf.fill(vec4(rgb.x, rgb.y, rgb.z, 0.45))
                    sdf.stroke(vec4(rgb.x, rgb.y, rgb.z, 0.68), 1.0)
                } else {
                    sdf.fill(vec4(rgb.x, rgb.y, rgb.z, 0.05))
                    sdf.stroke(vec4(rgb.x, rgb.y, rgb.z, 0.68), 1.0)
                    sdf.box(
                        part_x0 + 1.0,
                        part_y0 + 1.0,
                        max(part_x1 - part_x0 - 2.0, 1.0),
                        max(part_y1 - part_y0 - 2.0, 1.0),
                        1.0
                    )
                    if self.state < 3.5 {
                        let pulse = 0.5 + 0.5 * sin(self.time * 12.5663706)
                        sdf.fill(vec4(rgb.x, rgb.y, rgb.z, 0.34 + pulse * 0.34))
                        sdf.stroke(vec4(1.0, 1.0, 1.0, 0.45 + pulse * 0.5), 2.0)
                    } else {
                        sdf.fill(vec4(rgb.x * 0.82, rgb.y * 0.82, rgb.z * 0.82, 0.92))
                        sdf.stroke(vec4(1.0, 1.0, 1.0, 0.95), 2.0)
                    }
                }
                return self.countdown(self.playing_progress(sdf.result))
            }

            // Keep the pad quiet behind its waveform; state remains visible
            // in the outline, queued pulse and playing progress.
            if self.state < 1.5 {
                sdf.stroke(vec4(rgb.x, rgb.y, rgb.z, 0.25), 1.0)
            } else if self.state < 2.5 {
                sdf.fill(vec4(rgb.x, rgb.y, rgb.z, 0.07))
                sdf.stroke(vec4(rgb.x, rgb.y, rgb.z, 0.68), 1.0)
            } else {
                sdf.fill(vec4(rgb.x, rgb.y, rgb.z, 0.03))
                sdf.stroke(vec4(rgb.x, rgb.y, rgb.z, 0.68), 1.0)
                sdf.box(
                    part_x0 + 1.0,
                    part_y0 + 1.0,
                    max(part_x1 - part_x0 - 2.0, 1.0),
                    max(part_y1 - part_y0 - 2.0, 1.0),
                    1.0
                )
                if self.state < 3.5 {
                    // Up next: a pulsing white frame, and the bar countdown
                    // strip along the active slot's top edge.
                    let pulse = 0.5 + 0.5 * sin(self.time * 12.5663706)
                    sdf.fill(vec4(rgb.x, rgb.y, rgb.z, 0.08))
                    sdf.stroke(vec4(1.0, 1.0, 1.0, 0.45 + pulse * 0.5), 2.0)
                } else {
                    sdf.fill(vec4(rgb.x, rgb.y, rgb.z, 0.10))
                    sdf.stroke(vec4(1.0, 1.0, 1.0, 0.95), 2.0)
                }
            }

            let inner_w = max(w - 8.0, 1.0)
            let inner_x = clamp((p.x - 4.0) / inner_w, 0.0, 1.0)
            let column = self.span_start + inner_x * self.span_cols
            let tile = self.mix_span(column)
            let stems = self.stem_span(column)
            let present = max(stems.x + stems.y + stems.z + stems.w, 0.0001)
            let c0 = 1.0 - step(0.5, self.channel)
            let c1 = step(0.5, self.channel) - step(1.5, self.channel)
            let c2 = step(1.5, self.channel) - step(2.5, self.channel)
            let c3 = step(2.5, self.channel) - step(3.5, self.channel)
            let stem_level = (stems.x * c0 + stems.y * c1 + stems.z * c2 + stems.w * c3) / present
            let level = clamp(mix(tile.w * stem_level, tile.w, is_mix), 0.0, 1.0) * 0.80

            let active = step(2.5, self.state)
            let part_h = max(self.part.w - self.part.z, 0.001)
            let part_y = clamp((self.pos.y - self.part.z) / part_h, 0.0, 1.0)
            let display_y = mix(self.pos.y, part_y, active)
            let display_h = mix(h, part_y1 - part_y0, active)
            let inner_scale = max(display_h - 8.0, 1.0) / max(display_h, 1.0)
            let y = abs(display_y - 0.5) * 2.0
            let feather = 2.0 / max(h, 2.0)
            let envelope = (1.0 - smoothstep(
                level * inner_scale - feather,
                level * inner_scale + feather,
                y
            )) * step(0.002, level)
            let in_x = smoothstep(3.0, 4.0, p.x)
                * (1.0 - smoothstep(w - 4.0, w - 3.0, p.x))
            let part_mask = step(part_x0, p.x) * step(p.x, part_x1)
                * step(part_y0, p.y) * step(p.y, part_y1)
            let cover = envelope * in_x * mix(1.0, part_mask, active)

            let pulse = 0.5 + 0.5 * sin(self.time * 12.5663706)
            let part_w = max(self.part.y - self.part.x, 0.001)
            let part_x = clamp((self.pos.x - self.part.x) / part_w, 0.0, 1.0)
            let played = step(part_x, clamp(self.phase, 0.0, 1.0))
            let silent = 1.0 - step(1.5, self.state)
            let ready = step(1.5, self.state) - step(2.5, self.state)
            let queued = step(2.5, self.state) - step(3.5, self.state)
            let playing = step(3.5, self.state)
            let alpha = (silent * 0.20
                + ready * 0.35
                + queued * (0.45 + pulse * 0.40)
                + playing * (0.55 + played * 0.35))
                * mix(1.0, 0.55, self.has_blocks)
            let gain = 1.0 + playing * played * 0.12
            let wave = vec4(
                min(rgb.x * gain, 1.0),
                min(rgb.y * gain, 1.0),
                min(rgb.z * gain, 1.0),
                alpha
            )
            return self.countdown(self.playing_progress(sdf.result.mix(wave, cover)))
        }
    }

    set_type_default() do #(DrawSplatBlock::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 1.0)
            sdf.fill(vec4(self.color.xyz, self.alpha))
            return sdf.result
        }
    }

    mod.widgets.VjLoopSplatBase = #(VjLoopSplat::register_widget(vm))
    mod.widgets.VjLoopSplat = set_type_default() do mod.widgets.VjLoopSplatBase{
        width: Fill
        height: Fill
        draw_text +: {
            color: #xf4f7fa
            text_style: theme.font_bold{font_size: 10}
        }
        draw_small +: {
            color: #xaab4be
            text_style: theme.font_bold{font_size: 8}
        }
    }

    // ---- one zoomed waveform lane ----------------------------------------
    set_type_default() do #(DrawWaveLane::script_shader(vm)){
        ..mod.draw.DrawQuad
        tiles: texture_2d(float)
        stem_tiles: texture_2d(float)

        color_bg: uniform(#x0a0d12)
        // Before separation a wave is grey peaks and nothing else: colour
        // in this view always means a real separated stem.
        color_grey: uniform(#x8b98a6)
        // How much colour a band reading may carry. Mirrored in Rust as
        // BAND_CHROMA, where the reasoning and the measurements live.
        band_chroma: uniform(0.24)
        color_grid: uniform(#xffffff1e)
        color_grid_bar: uniform(#xffffff6e)
        // The running loop, in the app-wide accent. Low alpha: a wash the
        // waveform stays readable through, not a fill that replaces it.
        color_loop: uniform(#xff5c392e)
        // The loop's span as (start_col, end_col, _, _). A UNIFORM and not
        // two `#[live]` fields: those become per-instance VERTEX INPUTS,
        // and this lane already sits on the vs_5_0 limit of 32 — two more
        // attributes fail the shader compile outright (X4506). Nothing
        // here varies per instance anyway.
        loop_span: uniform(#x00000000)
        // How hard the end-of-track warning is showing, 0..1. A UNIFORM
        // for the same reason `loop_span` is one: this lane sits ON the
        // vs_5_0 limit of 32 vertex inputs, and one more `#[live]` field
        // is one more per-instance attribute and no waveform at all on
        // Windows. It varies per LANE rather than per instance, and each
        // lane is its own draw, so a uniform carries it honestly.
        warn: uniform(0.0)
        color_warn: uniform(#xff3b30)
        // A loop drag's would-be landing, same encoding as `loop_span`.
        // Drawn dimmer beside the ghost so the operator sees both where
        // the loop IS and where release will put it.
        preview_span: uniform(#x00000000)
        color_head: uniform(#xf4f7fa)
        // Where the playhead sits across the lane's width, 0..1. A
        // uniform, not an instance: this shader is already at D3D11's
        // vs_5_0 32-input ceiling (see the stem palette below), and one
        // more instance field is "no waveform at all on Windows" again.
        head_fraction: uniform(0.5)
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

        // The running loop: a translucent wash plus one-pixel bracket
        // edges. The short top/bottom caps make IN and OUT read as a pair,
        // not as two unrelated grid rules.
        loop_at: fn(column: float) -> float {
            if self.loop_span.y <= self.loop_span.x {
                return 0.0
            }
            let inside = step(self.loop_span.x, column) * step(column, self.loop_span.y)
            let scale = max(self.cols_per_px, 0.0001)
            let from_in = (column - self.loop_span.x) / scale
            let from_out = (self.loop_span.y - column) / scale
            let edge_in = 1.0 - smoothstep(0.5, 1.0, abs(from_in))
            let edge_out = 1.0 - smoothstep(0.5, 1.0, abs(from_out))
            let y = min(self.pos.y, 1.0 - self.pos.y) * self.rect_size.y
            let cap_y = 1.0 - smoothstep(0.5, 1.0, y)
            let cap_in = step(0.0, from_in) * step(from_in, 6.0)
            let cap_out = step(0.0, from_out) * step(from_out, 6.0)
            let bracket = max(max(edge_in, edge_out), cap_y * max(cap_in, cap_out))
            return max(inside * self.color_loop.w, bracket)
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
            let column = self.centre_col + (px - self.rect_size.x * self.head_fraction) * self.cols_per_px
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

            // A column the separator has NOT reached is coloured by its
            // three bands, which have been in this texture all along and
            // ignored: red is low, green is mid, blue is high. The balance
            // of the three picks a hue; the hue is all it says.
            //
            // Held at the grey's own brightness, because brightness in this
            // lane already means played-or-coming and active-or-parked, and
            // capped well short of the stem palette's colourfulness, because
            // the one thing this must never do is let an unseparated record
            // pass for a separated one. The other half of that is structural
            // and stronger: a separated column is a STACK of up to four
            // colours with edges, and this is one flat tone from the centre
            // to the tip.
            let top = max(t.x, max(t.y, t.z))
            let unit = vec3(t.x, t.y, t.z) / max(top, 0.0001)
            let mid = unit.x * 0.299 + unit.y * 0.587 + unit.z * 0.114
            let off = unit - vec3(mid, mid, mid)
            let spread = max(off.x, max(off.y, off.z)) - min(off.x, min(off.y, off.z))
            let grey_y = self.color_grey.x * 0.299
                + self.color_grey.y * 0.587
                + self.color_grey.z * 0.114
            let toned = vec3(grey_y, grey_y, grey_y)
                + off * min(1.0, self.band_chroma / max(spread, 0.0001))
            // Nothing measured, nothing to say: the old grey stands.
            let measured = step(0.004, top) * step(0.000001, spread)
            let plain = vec4(
                mix(self.color_grey.x, toned.x, measured),
                mix(self.color_grey.y, toned.y, measured),
                mix(self.color_grey.z, toned.z, measured),
                1.0
            )

            let c0 = plain.mix(self.color_bass, separated)
            let c1 = plain.mix(self.color_drums, separated)
            let c2 = plain.mix(self.color_vocals, separated)
            let c3 = plain.mix(self.color_other, separated)

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
            // The overview and a stationary loop lane carry an in-shader
            // playhead; normally the zoomed lanes share the centre overlay.
            // The loop sits over the picture, under the playhead: you have
            // to be able to see the band through a loud passage.
            let la = max(self.loop_at(column), self.preview_at(column))
            let banded = ruled.mix(vec4(self.color_loop.x, self.color_loop.y, self.color_loop.z, 1.0), la)
            // The end-of-track warning, UNDER the playhead so the head
            // stays crisp, and capped well short of opaque: the last
            // thirty seconds of a record are exactly when the picture
            // most needs reading, so this has to be impossible to miss
            // without painting over the thing being watched.
            let warned = banded.mix(self.color_warn, self.warn * 0.38)
            let hd = abs(column - self.head_col) / max(self.cols_per_px, 0.0001)
            let ha = (1.0 - smoothstep(0.5, 1.8, hd)) * self.head_on
            return warned.mix(self.color_head, ha)
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
        draw_body_edge +: { color: #xb4c0cd }
        draw_mark_top +: {
            color: #xe5484d
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
        draw_mark_bottom +: {
            color: #xf5c542
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
            // Per CHIP rather than per family: the bank hands each number
            // its own hue, so the colour arrives with the draw.
            color: #x3d8bff
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
        // Where the file starts and stops making a sound. Quiet, because
        // it is a fact about the recording rather than a mark anybody put
        // there, and every chip draws over it.
        draw_edge_sound +: { color: #x6b7683 }
        // Where the BODY starts and ends, which is what the automation
        // aims at -- brighter than the two that only say where the
        // recording makes a noise.
        draw_edge_body +: { color: #xb4c0cd }
        // Where the arrangement turns. A stub rather than a rule, because
        // the difference that has to survive a glance at a loud passage is
        // a difference in SHAPE: three greys separated only by lightness
        // all read as "a dark hairline" over a bright waveform.
        draw_change +: { color: #x8e9aa8 }
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
        // The seek target while dragging, and a hovered mark's time.
        draw_text +: {
            color: #xf4f7fa
            text_style: theme.font_bold{font_size: 9}
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
        // TWELVE GENERIC CELLS, not one per column.
        //
        // Which column a cell carries — its words, its width, its ink and
        // whether it shows at all — is decided per draw from the list's
        // column layout, because the operator picks both the set of columns
        // and their ORDER. A cell per named column cannot be reordered: a
        // view draws its children in the order they are declared here, and
        // that order is fixed at build time. The header opposite is built
        // the same way and from the same layout, so the two cannot drift.
        //
        // The bold face is the widest a cell ever needs (BPM wears it); a
        // lighter column overrides the text style along with its colour.
        //
        // They live inside a FILL box of their own, so the two chips after
        // it are fixed-size siblings of that box rather than of the cells.
        // A Fill column with a minimum — the title has one — takes more than
        // its share when the row is narrow, and what it took came out of
        // whatever was laid out last: on a narrow set list the headphone and
        // remove chips were pushed off the end of their own row. The box
        // clips instead, so the columns lose their tail and the chips keep
        // their place.
        row_cells := View{
        width: Fill
        height: Fill
        flow: Right
        spacing: 6
        clip_x: true
        align: Align{x: 0.0, y: 0.5}
        row_col0 := TrackText{width: 0}
        row_col1 := TrackText{width: 0}
        row_col2 := TrackText{width: 0}
        row_col3 := TrackText{width: 0}
        row_col4 := TrackText{width: 0}
        row_col5 := TrackText{width: 0}
        row_col6 := TrackText{width: 0}
        row_col7 := TrackText{width: 0}
        row_col8 := TrackText{width: 0}
        row_col9 := TrackText{width: 0}
        row_col10 := TrackText{width: 0}
        row_col11 := TrackText{width: 0}
        row_col12 := TrackText{width: 0}
        }
        row_key := TrackText{width: 40 draw_text.color: #xc6a0f0}
        row_time := TrackText{width: 52 draw_text.color: #x9fabb7}
        // The processed marks: a green tick under STEM when the
        // store holds this track's four stems, under KRK when it
        // holds the word-aligned transcript.
        row_stem := TrackText{width: 36 draw_text.color: #x35c05f}
        row_krk := TrackText{width: 30 draw_text.color: #x35c05f}
        row_license := TrackText{width: 128 draw_text.color: #x6f7b87}
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
        // The set list's own remove: a row in the QUEUE is a decision, and
        // taking it back should not mean dragging the row out or clearing
        // the lot. Sits beside the phones mark, where the explorer keeps
        // its `+` — the same place means the same kind of job.
        row_unqueue := Button{
            width: 22
            height: 18
            text: "\u{2212}"
            padding: 0
            align: Align{x: 0.5, y: 0.5}
            draw_bg +: {
                color: #x272e38
                color_hover: #x3a2b2f
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
                    // Where the KEYS are standing. Cooler than the carry
                    // accent and quieter than the pick's fill: it says
                    // "an arrow moves from here", not "this is chosen".
                    color_cursor: #x8fb4ff
                    live: instance(0.0)
                    odd: instance(0.0)
                    sel: instance(0.0)
                    carry: instance(0.0)
                    cursor: instance(0.0)
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
                        // ONE outline, two things it can mean. The carried
                        // row wears it because the order rearranges live
                        // under the pointer, so the mark answers both what
                        // is in the hand and where letting go would leave
                        // it; otherwise it marks where the keys are
                        // standing. The carry wins when both are true --
                        // what is in the hand beats where the keys are.
                        sdf.stroke(
                            vec4(
                                self.color_cursor.x,
                                self.color_cursor.y,
                                self.color_cursor.z,
                                self.cursor
                            ).mix(
                                vec4(
                                    self.color_carry.x,
                                    self.color_carry.y,
                                    self.color_carry.z,
                                    self.carry
                                ),
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
                    // This template carries none of the plain row's stripe
                    // instances, so the cursor cannot be drawn the same way
                    // -- and the previewing row is exactly the one an
                    // operator walking the library is most likely standing
                    // on. A plain border, transparent until the keys are
                    // here, says it without a second shader.
                    border_size: 1.5
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

    // The deck transports are the console's primary hand targets. They use
    // the same chrome as the compact utility controls, but at a size that is
    // comfortable to hit; two rows keep that size inside each 316pt deck.
    let MusicTransportButton = MusicButton{
        height: 38
        padding: Inset{left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        align: Align{x: 0.5, y: 0.5}
        draw_bg +: {border_radius: 7.0}
        draw_text +: {text_style: theme.font_bold{font_size: 11}}
    }

    let MusicTransportIconButton = MusicIconButton{
        width: 40
        height: 38
        icon_walk: Walk{width: 16 height: Fit}
        draw_bg +: {border_radius: 7.0}
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
        // Centred HERE rather than at the moment a chip loses its word.
        //
        // A narrow console collapses these to a fixed round key with only
        // the icon in it, and a left-aligned icon in a fixed box sits off to
        // one side with dead width beside it. Setting the alignment from
        // Rust does not work — the alignment type is not in scope inside an
        // applied fragment, so it silently failed and the glyph stayed put.
        // With the word in place the button is Fit-width, so centring the
        // content changes nothing there.
        align: Align{x: 0.5, y: 0.5}
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

    // A gain knob: unity at the top, +6 dB at the right stop, and the cut
    // half a square law so the last few degrees before KILL are where
    // the fine control is. The arc runs from the resting point, so a cut
    // and a boost point different ways.
    let MusicKnob = Rotary{
        width: 42
        height: 42
        min: 0.0
        max: 2.0
        default: 1.0
        scroll_step: 0.025
        taper: Audio
        arc_from_origin: true
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
                // The lit arc runs from the resting point to the pointer
                // when the knob asks for it, else from the stop as a
                // fader would.
                let lo = min(self.origin_pos, self.slide_pos)
                let hi = max(self.origin_pos, self.slide_pos)
                let from = mix(0.0, lo, self.arc_origin)
                let to = mix(max(self.slide_pos, 0.01), max(hi, lo + 0.01), self.arc_origin)
                sdf.arc_round_caps(c.x, c.y, r - 2.5, start + sweep * from, start + sweep * to, 2.5)
                sdf.fill(self.val_color)
                // A tick at the resting point, so a knob at rest -- an arc
                // of no length -- still shows where home is.
                let o = start + sweep * self.origin_pos
                let od = vec2(-sin(o), cos(o))
                let t0 = c + od * (r - 6.0)
                let t1 = c + od * r
                sdf.move_to(t0.x, t0.y)
                sdf.line_to(t1.x, t1.y)
                sdf.stroke(self.rim_color * self.arc_origin, 1.0)
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

    let AttributionLink = LinkLabel{
        height: Fit
        margin: 0
        padding: 0
        draw_bg +: {
            color: #x00000000
            color_focus: #x00000000
            color_hover: #x00000000
            color_down: #x00000000
            border_size: 0.0
        }
        draw_text +: {
            color: #x6f7b87
            color_focus: #x6f7b87
            color_hover: #x9fabb7
            color_down: #x5f6a76
            text_style: theme.font_bold{font_size: 7}
        }
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
    let StemKnob = MusicKnob{width: 40 height: 40 default: 1.0}

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
                // From the bottom, or out of the resting point when the
                // fader asks for it: a pitch fader pulled down should
                // show a bar hanging BELOW centre, not a shorter bar
                // still growing from the floor.
                let lo = min(self.origin_pos, self.slide_pos)
                let hi = max(self.origin_pos, self.slide_pos)
                let from = mix(0., lo, self.arc_origin)
                let to = mix(self.slide_pos, hi, self.arc_origin)
                let fill_h = max(1., h * (to - from))
                let fill_bottom = top + h - h * from
                sdf.box(track_x + 1.5, fill_bottom - fill_h + 1.5, track_w - 3., max(1., fill_h - 3.), 2.)
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
        default: 0.5
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

    // A channel meter: a segmented column, and above it a mark holding the
    // highest recent peak. Both values are uniforms, so the level moves
    // without re-emitting an instance.
    let DeckMeter = SolidView{
        width: 9
        height: Fill
        draw_bg +: {
            level: uniform(0.0)
            hold: uniform(0.0)
            color: uniform(#x1d222a)
            color_lit: uniform(#xff5c39)
            color_hot: uniform(#xff5a4e)
            // Its own colour, and a pale one deliberately: the mark has to
            // read against the dark trough AND against the lit bar it sits
            // on top of, and anything from the bar's own range disappears
            // into the bar exactly when the level is high.
            color_mark: uniform(#xffffffcc)
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
                // Held inside the column rather than centred on the level:
                // at full scale a centred mark loses half its width off the
                // top edge, which is the one reading it exists to report.
                // Two whole pixels, on a whole pixel. At 1.5 it landed
                // across two rows at half weight in each and came and went
                // as the level moved it a fraction up or down -- the same
                // reason the marks on the wave lane snap.
                let mark_h = 2.0
                let mark_y = floor(clamp(
                    (1.0 - self.hold) * self.rect_size.y - mark_h * 0.5,
                    0.0,
                    self.rect_size.y - mark_h
                ))
                sdf.box(1.5, mark_y, self.rect_size.x - 3.0, mark_h, 0.5)
                // Nothing held, no mark -- otherwise it parks on the floor
                // and reads as a level that is not there.
                sdf.fill(vec4(
                    self.color_mark.x,
                    self.color_mark.y,
                    self.color_mark.z,
                    self.color_mark.w * step(0.002, self.hold)
                ))
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
                // Retire the deck. It sits at the head's outer corner
                // beside the letter it clears, mirrored across the centre
                // line the way the rest of this row is. A press on a
                // PLAYING deck is refused and the button greys to say so;
                // a second press within half a second puts the track back.
                //
                // The room for it comes out of the title column's Fill, so
                // nothing below moves.
                deck_a_retire := MusicButton{
                    width: 22 height: 22 padding: 0
                    align: Align{x: 0.5, y: 0.5}
                    text: "×"
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
                    deck_a_credit := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 3
                        deck_a_credit_artist := AttributionLink{text: ""}
                        Label{
                            text: "·"
                            draw_text.color: #x6f7b87
                            draw_text.text_style: theme.font_bold{font_size: 7}
                        }
                        deck_a_credit_license := AttributionLink{text: ""}
                    }
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
                // The count, beside the tempo it is counting. Its own
                // batch: the LED resolves its phase at draw time and
                // redraws every frame, and the head's title, art, tempo
                // and time must not be dragged along at that cadence.
                View{
                    width: Fit
                    height: Fit
                    new_batch: true
                    align: Align{x: 0.5, y: 0.5}
                    deck_a_beat := VjBeatLed{width: 18 height: 22}
                }
                deck_a_key := KeyReadout{text: "—"}
                deck_a_time := MusicLabel{width: 78 text: "0:00 / 0:00"}
            }
            // QUANT, not SNAP: an immediate, phase-preserving jump, which
            // is what the word means here. SNAP stays reserved for
            // placement rounding, which this deliberately is not. It sits
            // at the console's center line, between the two decks it
            // gates equally.
            //
            // One word over two chips: the left is deck A's and the right
            // is deck B's, matching the heads either side. A unit belongs
            // to a deck rather than to the console because the two decks
            // are rarely doing the same job -- one is playing and wants
            // its jumps on the bar, the other is being cued by hand.
            View{
                width: Fit
                height: Fit
                flow: Down
                spacing: 2
                margin: Inset{left: 10, right: 10}
                align: Align{x: 0.5, y: 0.5}
                MusicLabel{text: "QUANT"}
                View{
                    width: Fit
                    height: Fit
                    flow: Right
                    spacing: 4
                    music_snap_a := VjBeatsDrop{width: 34}
                    music_snap_b := VjBeatsDrop{width: 34}
                }
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
                // The count, beside the tempo it is counting. Its own
                // batch: the LED resolves its phase at draw time and
                // redraws every frame, and the head's title, art, tempo
                // and time must not be dragged along at that cadence.
                View{
                    width: Fit
                    height: Fit
                    new_batch: true
                    align: Align{x: 0.5, y: 0.5}
                    deck_b_beat := VjBeatLed{width: 18 height: 22}
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
                    deck_b_credit := View{
                        visible: false
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 3
                        deck_b_credit_artist := AttributionLink{text: ""}
                        Label{
                            text: "·"
                            draw_text.color: #x6f7b87
                            draw_text.text_style: theme.font_bold{font_size: 7}
                        }
                        deck_b_credit_license := AttributionLink{text: ""}
                    }
                    deck_b_artist := MusicLabel{width: Fill text: ""}
                }
                deck_b_art := Image{width: 44 height: 44}
                Label{
                    text: "B"
                    draw_text.color: #x6aa8ff
                    draw_text.text_style: theme.font_bold{font_size: 13}
                }
                deck_b_retire := MusicButton{
                    width: 22 height: 22 padding: 0
                    align: Align{x: 0.5, y: 0.5}
                    text: "×"
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
                deck_a_overview := mod.widgets.VjWaveOverview{
                    height: Fill
                    draw_load +: {color: #xff5c39}
                }
            }
            deck_b_well := DeckWell{
                width: Fill
                deck_b_overview := mod.widgets.VjWaveOverview{
                    height: Fill
                    draw_load +: {color: #x6aa8ff}
                }
            }
        }


        // The console proper and its floating score card share an overlay.
        // The page body remains the responsive in-flow surface; the card is
        // its later sibling, so it neither takes deck/list space nor yields
        // pointer hits to the waveform underneath.
        View{
            width: Fill
            height: Fill
            flow: Overlay

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
                        // SLIP: the track keeps running where you left it
                        // while the record goes somewhere else. The room
                        // comes out of SYNC's own Fill, which is the only
                        // elastic thing in this row and where the headphone
                        // button's width came from too.
                        deck_a_slip := MusicButton{width: 34 height: 22 text: "SLIP"}
                        // The analyser's grid can sit on the off pulse: same tempo,
                        // sync exactly half a beat out. This flips it.
                        deck_a_phase_flip := MusicButton{width: 26 height: 22 padding: 0 align: Align{x: 0.5, y: 0.5} text: "½"}
                        // Headphone cue: latch this deck onto the phones bus.
                        // Green when live — monitoring, never program.
                        //
                        // Up here with SYNC and KEY rather than down in the
                        // transport: pre-listen changes what the OPERATOR
                        // hears, not what the room does, and so do the two it
                        // now sits between. The height is the row's 22, not the
                        // icon button's own 24 — two points proud of the
                        // buttons either side reads as a mistake.
                        deck_a_hp := MusicIconButton{
                            width: 30
                            height: 22
                            draw_icon +: { svg: crate_resource("self:resources/icons/headphones.svg") }
                        }
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
                                deck_a_pitch := MusicFader{min: -1.0 max: 1.0 default: 0.0 arc_from_origin: true}
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
                                    deck_a_filter := MusicKnob{min: 0.0 max: 1.0 default: 0.5 taper: Linear}
                                    // The slot the three bands spend on
                                    // kill and solo: the sweep has no
                                    // bands to kill, so its row carries
                                    // resonance, the echo and freeze
                                    // instead. Three now, not two, so
                                    // each chip is a single letter, the
                                    // way M and S already are.
                                    MSRow{
                                        deck_a_resonance := MSButton{text: "R"}
                                        deck_a_freeze := MSButton{text: "F"}
                                    }
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
                            //
                            // Wrapped in a plain View because the reader is a
                            // raw-Area widget with no `visible` of its own:
                            // set_visible on it does nothing, so folded it
                            // went on being laid out and — being Fill — ate
                            // whatever room the blocks above it gave up. The
                            // accordion folds THIS, which does honour it.
                            deck_a_kar_body := View{
                                width: Fill
                                height: Fill
                                deck_a_lyrics := mod.widgets.VjLyricReader{height: Fill}
                            }
                        }
                    }
                    // The deck's own transport, at the foot of its column. One
                    // row, sized to hold every primary control without
                    // escaping the fixed deck panel -- see MusicTransportButton
                    // and MusicTransportIconButton's own overrides below for
                    // the sizing this row specifically needs.
                    View{
                        width: Fill
                        height: Fit
                        flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                        spacing: 3
                        wrap_spacing: 3
                        align: Align{x: 0.0, y: 0.5}
                        deck_a_play := MusicTransportIconButton{
                            width: 26
                            icon_walk: Walk{width: 13 height: Fit}
                            draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") }
                        }
                        deck_a_cue := MusicTransportIconButton{
                            width: 38
                            icon_walk: Walk{width: 14 height: Fit}
                            draw_icon +: { svg: crate_resource("self:resources/icons/cue.svg") }
                        }
                        // One beat either way -- the nudge a hand makes
                        // when the drop lands a hair off -- and held, they
                        // BEND the record rather than stepping it. A beat
                        // is a measured one where the analysis found beats
                        // and a second where it did not, so they always
                        // step something.
                        //
                        // They point at the TRACK, not at the playhead: <
                        // sends the track a beat FORWARD past the head, >
                        // a beat back, which is the same convention as a
                        // hand on the platter. NOT mirrored on deck B: the
                        // sense is the same whichever deck it is.
                        deck_a_beat_fwd := MusicTransportButton{width: 22 text: "<"}
                        deck_a_beat_back := MusicTransportButton{width: 22 text: ">"}
                        deck_a_loop := MusicTransportIconButton{
                            width: 26
                            icon_walk: Walk{width: 13 height: Fit}
                            draw_icon +: { svg: crate_resource("self:resources/icons/loop_one.svg") }
                        }
                        deck_a_loop_halve := MusicTransportButton{width: 22 text: "-"}
                        deck_a_loop_len := VjBeatsDrop{
                            width: 30 height: 34 loop_rows: true
                            draw_bg +: {arrow: 0.0}
                            draw_text +: {text_style: theme.font_bold{font_size: 10}}
                        }
                        deck_a_loop_double := MusicTransportButton{width: 22 text: "+"}
                        // The loop pair, in glyphs that read as the marks
                        // they set: `[` in, `]` out. The loop icon left of the
                        // stepper is RELOOP/EXIT; the sparkle past them opens the
                        // scanner, which is also where marks go to be forgotten.
                        deck_a_loop_in := MusicTransportButton{width: 22 text: "["}
                        deck_a_loop_out := MusicTransportButton{width: 22 text: "]"}
                        deck_a_loop_scan := MusicTransportIconButton{
                            width: 26
                            icon_walk: Walk{width: 13 height: Fit}
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
                                scroll_step: 0.025
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
                            // The travel turned round: the left end answers
                            // to the deck that was on the right, and the
                            // letters beside the sweep follow it.
                            xfade_rev := MusicButton{width: 40 height: 22 text: "REV"}
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
                        deck_b_phase_flip := MusicButton{width: 26 height: 22 padding: 0 align: Align{x: 0.5, y: 0.5} text: "½"}
                        // Deck A's phones latch, mirrored: KEY then hp then
                        // SYNC, reading outward from the console's centre.
                        deck_b_hp := MusicIconButton{
                            width: 30
                            height: 22
                            draw_icon +: { svg: crate_resource("self:resources/icons/headphones.svg") }
                        }
                        // Mirrored, so SLIP sits inboard of SYNC on this
                        // side the way the rest of the row is mirrored.
                        deck_b_slip := MusicButton{width: 34 height: 22 text: "SLIP"}
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
                                    deck_b_filter := MusicKnob{min: 0.0 max: 1.0 default: 0.5 taper: Linear}
                                    // The slot the three bands spend on
                                    // kill and solo: the sweep has no
                                    // bands to kill, so its row carries
                                    // resonance, the echo and freeze
                                    // instead. Three now, not two, so
                                    // each chip is a single letter, the
                                    // way M and S already are.
                                    MSRow{
                                        deck_b_resonance := MSButton{text: "R"}
                                        deck_b_freeze := MSButton{text: "F"}
                                    }
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
                            deck_b_kar_body := View{
                                width: Fill
                                height: Fill
                                deck_b_lyrics := mod.widgets.VjLyricReader{height: Fill}
                            }
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
                                deck_b_pitch := MusicFader{min: -1.0 max: 1.0 default: 0.0 arc_from_origin: true}
                                deck_b_pitch_reset := MusicButton{width: Fill height: 14 padding: 0 align: Align{x: 0.5, y: 0.5} text: "0"}
                            }
                        }
                    }
                    // Deck B mirrors the transport row across the waveforms.
                    View{
                        width: Fill
                        height: Fit
                        flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                        spacing: 3
                        wrap_spacing: 3
                        align: Align{x: 1.0, y: 0.5}
                        // NOT mirrored, exactly as the loop marks are not:
                        // the chevrons read the same on both decks.
                        deck_b_beat_fwd := MusicTransportButton{width: 22 text: "<"}
                        deck_b_beat_back := MusicTransportButton{width: 22 text: ">"}
                        deck_b_cue := MusicTransportIconButton{
                            width: 38
                            icon_walk: Walk{width: 14 height: Fit}
                            draw_icon +: { svg: crate_resource("self:resources/icons/cue.svg") }
                        }
                        deck_b_play := MusicTransportIconButton{
                            width: 26
                            icon_walk: Walk{width: 13 height: Fit}
                            draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") }
                        }
                        // The sparkle stays outermost. IN then OUT keeps the
                        // gesture's temporal order on either deck.
                        deck_b_loop_scan := MusicTransportIconButton{
                            width: 26
                            icon_walk: Walk{width: 13 height: Fit}
                            draw_icon +: { svg: crate_resource("self:resources/icons/sparkle.svg") }
                        }
                        deck_b_loop_in := MusicTransportButton{width: 22 text: "["}
                        deck_b_loop_out := MusicTransportButton{width: 22 text: "]"}
                        deck_b_loop_halve := MusicTransportButton{width: 22 text: "-"}
                        deck_b_loop_len := VjBeatsDrop{
                            width: 30 height: 34 loop_rows: true
                            draw_bg +: {arrow: 0.0}
                            draw_text +: {text_style: theme.font_bold{font_size: 10}}
                        }
                        deck_b_loop_double := MusicTransportButton{width: 22 text: "+"}
                        deck_b_loop := MusicTransportIconButton{
                            width: 26
                            icon_walk: Walk{width: 13 height: Fit}
                            draw_icon +: { svg: crate_resource("self:resources/icons/loop_one.svg") }
                        }
                    }
                }
            }

            // The grip between the decks and the lists.
            //
            // It lies across the flow, whichever way the body runs: a bar under
            // the decks while they are stacked, and a bar beside them once they
            // are not. `App::sync_page_body_flow` turns it, and dragging it sets
            // the lists their size — overriding the automatic allotment, but
            // never past the point where the mixer would starve.
            page_splitter := RoundedView{
                width: Fill
                height: 7
                cursor: MouseCursor.RowResize
                show_bg: true
                draw_bg +: {
                    // Visible at rest, not only under the pointer: a
                    // splitter nobody can see is a splitter nobody drags.
                    color: #xffffff1f
                    color_hover: #xffffff5c
                    border_radius: 3.0
                    hover: instance(0.0)
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        // A short bar in the middle, not the whole width: it
                        // reads as a handle rather than a rule across the page.
                        let w = min(self.rect_size.x, self.rect_size.y) * 0.5 + 22.0
                        let along = max(self.rect_size.x, self.rect_size.y)
                        let horizontal = step(self.rect_size.y, self.rect_size.x)
                        let bar_x = mix(self.rect_size.x * 0.5 - 1.5, along * 0.5 - w * 0.5, horizontal)
                        let bar_y = mix(along * 0.5 - w * 0.5, self.rect_size.y * 0.5 - 1.5, horizontal)
                        let bar_w = mix(3.0, w, horizontal)
                        let bar_h = mix(w, 3.0, horizontal)
                        sdf.box(bar_x, bar_y, bar_w, bar_h, 1.5)
                        sdf.fill(self.color.mix(self.color_hover, self.hover))
                        return sdf.result
                    }
                }
                // `Animator{ state: { default: @off ... } }` — the form the
                // widgets use. The looser one this first carried parsed to
                // nothing, so the grip never lit and only the cursor said it
                // could be dragged.
                animator: Animator{
                    hover: {
                        default: @off
                        off: AnimatorState{
                            from: {all: Forward {duration: 0.1}}
                            apply: { draw_bg: {hover: 0.0} }
                        }
                        on: AnimatorState{
                            from: {all: Forward {duration: 0.08}}
                            apply: { draw_bg: {hover: 1.0} }
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
                    lists_tab_2 := MusicButton{width: 62 height: 22 text: "loops"}
                }
                // A GRIP between the listing and the set list.
                //
                // The queue used to be a fixed 320 points, which is the right
                // width for a title and nothing else — and the set list now
                // carries tempo and key, and whatever else the operator has
                // asked it to carry. FromB keeps that 320 as the starting
                // place rather than as the law, so widening the set list
                // costs the listing exactly what it gains.
                // The pair the loops page replaces: hidden as one, so the loops
                // page gets the whole column rather than a seam and two blanks.
                lists_pair := View{
                    width: Fill
                    height: Fill
                    lists_split := Splitter{
                        width: Fill
                        height: Fill
                        axis: SplitterAxis.Horizontal
                        align: SplitterAlign.FromB(320.0)
                        // The seam the rest of the console uses: near-invisible
                        // at rest, accent under the pointer.
                        size: 6.0
                        draw_bg +: {
                            color_bg: #x14171c
                            color: #x222830
                            color_hover: #x46312b
                            color_drag: #xff5c39
                            splitter_pad: 2.0
                            bar_size: 72.0
                        }
                        a: View{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 6
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
                                // OUTSIDE the catalog-only view on purpose. It used to
                                // live inside it, so the LOCAL FILES switch hid the box
                                // along with the paging controls — and an operator who
                                // had dropped four hundred files on the window got four
                                // hundred rows and no way to type a name at all. The
                                // night the store cannot be reached is exactly the night
                                // that matters. The category and paging controls stay
                                // catalog-only below, because a local listing has
                                // neither.
                                music_search := TextInput{
                                    // Twelve characters of query at the floor, and a
                                    // ceiling: past ~488 the box is just a long empty
                                    // trough, and the row's other controls can use it.
                                    width: Fill{min: 96. max: 488.}
                                    // One line, always: the themed input wraps its text
                                    // by default, and a long query is not worth making
                                    // the whole row two lines tall.
                                    flow: Flow.Right{wrap: false}
                                    empty_text: "search music…"
                                }
                                music_catalog := View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    align: Align{x: 0.0, y: 0.5}
                                    // Eight characters of category: the floor below which
                                    // a field stops being a field. The category cell is
                                    // narrowed by `App::sync_library_density` when the
                                    // console is.
                                    music_category_cell := View{
                                        width: 96
                                        height: Fit
                                        music_category := TextInput{
                                            width: Fill
                                            flow: Flow.Right{wrap: false}
                                            empty_text: "category"
                                            // The explorer opens filtered to
                                            // music (the model's default);
                                            // showing the word keeps the box
                                            // honest — clear it for all audio.
                                            text: "music"
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
                                // Narrow the listing to what has already been worked
                                // out. Several at once AND together: "a key and a
                                // tempo" is the harmonic-mixing question, and it is
                                // not answerable one column at a time.
                                //
                                // One chip that drops a list of ticks, not four chips
                                // in a row: four of them cost most of a narrow
                                // console's line to say something the operator reads
                                // once a set. Closed, the chip carries the count, so
                                // a filter that is narrowing the listing still says
                                // so without being opened.
                                //
                                // OUTSIDE `music_catalog`, which folds away with the
                                // local listing: STEMS/KARAOKE/KEY/BPM is work that
                                // has been done or not, and a local file answers that
                                // question exactly as a catalog row does. Search and
                                // More are catalog-only; this is not.
                                //
                                // Height PINNED to the chips it now sits between —
                                // its own line let it stand at 18, and four points
                                // short in this row reads as a mistake.
                                music_has_filter := DropToggles{
                                    height: 22
                                    text: "FILTER"
                                    labels: ["STEMS" "KARAOKE" "KEY" "BPM"]
                                    draw_icon +: { svg: crate_resource("self:resources/icons/filter.svg") }
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
                                // The explorer's menu: what may be worked out
                                // ahead of the set, which columns each list
                                // carries, and where the cache lives. A MENU
                                // mark rather than a gear — a gear promises
                                // settings for the thing beside it, and this
                                // opens the lists' own menu. Icon-only, so it
                                // takes the house icon button rather than a bare
                                // Button, which would leave the glyph off-centre.
                                music_prep_cfg := MusicIconButton{
                                    width: 24
                                    height: 20
                                    draw_icon +: { svg: crate_resource("self:resources/icons/menu.svg") }
                                }
                                // Fit, not a fixed 90: the count is four characters and a
                                // slash, and the dead width it used to carry pushed the
                                // load target away from it for nothing.
                                music_count := MusicLabel{width: Fit text: ""}
                                // Each target wears a mark, so a narrow console
                                // can drop the words and still be read: the
                                // decks are A and B on every other surface here,
                                // OFF is the power sign, and the mix target is a
                                // fader. The height is PINNED because the face
                                // is `height: Fit` — collapsed to its icon it
                                // would otherwise stand six points shorter than
                                // the chips it sits between.
                                deck_target := DropDown{
                                    height: 22
                                    labels: ["Auto" "Deck A" "Deck B" "Off" "Mix"]
                                    icons: [
                                        crate_resource("self:resources/icons/auto.svg")
                                        crate_resource("self:resources/icons/deck_a.svg")
                                        crate_resource("self:resources/icons/deck_b.svg")
                                        crate_resource("self:resources/icons/off.svg")
                                        crate_resource("self:resources/icons/mix.svg")
                                    ]
                                }
                                // What a pick may take over, read straight on
                                // from where it goes: the two are one thought,
                                // and in this order they are one sentence. The
                                // word
                                // "load" used to stand here introducing a
                                // wordless control, which was the emptiest
                                // thing on the line.
                                //
                                // The room comes from `music_search`, whose
                                // Fill yields down to 96 points before anything
                                // else in this row gives an inch — the same
                                // slack the FILTER chip and the count spend.
                                // Three short words rather than marks, because
                                // no icon says "refuse" without being learned,
                                // and this is the one control here whose
                                // default changes what a click does.
                                deck_over_playing := DropDown{
                                    height: 22
                                    labels: ["Refuse" "Stop" "Play in"]
                                }
                                // Which end of the crossfader each deck answers
                                // to, or THRU: out from under it, at full
                                // wherever the fader stands.
                                xf_side_a := DropDown{
                                    height: 22
                                    labels: ["A left" "A thru" "A right"]
                                }
                                xf_side_b := DropDown{
                                    height: 22
                                    labels: ["B left" "B thru" "B right"]
                                }
                                // What a plain press on a deck's LOCK means.
                                // Tap locks is what it has always been. Hold
                                // locks puts all four of the lock's meanings
                                // on the button itself — a tap matches once,
                                // a finger that stays hands the lock over —
                                // for a surface with no modifier keys to
                                // hold down.
                                deck_sync_gesture := DropDown{
                                    height: 22
                                    labels: ["Tap locks" "Hold locks"]
                                }
                                // How much of a record the zoomed lanes show,
                                // in seconds across their width. A wheel over
                                // a lane still does it; this is the same value
                                // where a hand can see it, a controller can
                                // reach it, and shift puts it back.
                                wave_zoom_learn := Learn{
                                    wave_zoom_knob := DropSlider{
                                        min: 1.5
                                        max: 10.0
                                        default: 8.0
                                        suffix: "s"
                                        draw_icon +: { svg: crate_resource("self:resources/icons/waveform.svg") }
                                    }
                                }
                                // Latched, the deck a picked track lands on starts as
                                // soon as its decode finishes — "select and it plays".
                                // An EJECT turned a quarter turn: the bar leads,
                                // the triangle follows. A plain play triangle
                                // here reads as "play", which is a different
                                // button on every deck in the room — and it has
                                // to survive losing its word, because this chip
                                // collapses to its icon on a narrow console like
                                // the rest of the row.
                                music_autoplay := MusicChipButton{
                                    text: "AUTOPLAY"
                                    draw_icon +: { svg: crate_resource("self:resources/icons/autoplay.svg") }
                                }
                            }
                            // The music import's whole face, on a line of its own.
                            // It began wedged into the control row above, where the
                            // fixed-width chrome squeezed it to eight pixels — a
                            // refusal nobody could read looks exactly like a drop
                            // that did nothing. A Fill line cannot be squeezed, and
                            // an empty one costs a few pixels of height.
                            // Only on screen while it has something to say: an empty
                            // label still costs a row between the search and the list.
                            music_import_status := MusicLabel{
                                visible: false
                                width: Fill
                                text: ""
                                draw_text.color: #xff5c39
                            }
                            // The column heads. Every one of them sorts: a click takes
                            // the order, a second click reverses it, and the arrow in the
                            // label says which column is holding it.
                            //
                            // Twelve generic cells, matching the row's twelve — see the
                            // note on `row_col0`. Which column each carries comes from
                            // the operator's layout at sync time, so the heads and the
                            // cells under them are reordered by one decision rather than
                            // by two that could disagree.
                            //
                            // The CELL carries the width, not the head: a Button's walk
                            // is private, so the head fills a box the host can size.
                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 6
                                padding: Inset{left: 6.0 right: 6.0 top: 0.0 bottom: 0.0}
                                align: Align{x: 0.0, y: 0.5}
                                MusicLabel{width: 26 text: ""}
                                // The same FILL box the rows put their cells in,
                                // so the heads narrow exactly as the cells under
                                // them do. See `row_cells`.
                                th_cells := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 6
                                clip_x: true
                                align: Align{x: 0.0, y: 0.5}
                                th_cell0 := View{width: 0 height: Fit th_head0 := MusicColHead{width: Fill text: ""}}
                                th_cell1 := View{width: 0 height: Fit th_head1 := MusicColHead{width: Fill text: ""}}
                                th_cell2 := View{width: 0 height: Fit th_head2 := MusicColHead{width: Fill text: ""}}
                                th_cell3 := View{width: 0 height: Fit th_head3 := MusicColHead{width: Fill text: ""}}
                                th_cell4 := View{width: 0 height: Fit th_head4 := MusicColHead{width: Fill text: ""}}
                                th_cell5 := View{width: 0 height: Fit th_head5 := MusicColHead{width: Fill text: ""}}
                                th_cell6 := View{width: 0 height: Fit th_head6 := MusicColHead{width: Fill text: ""}}
                                th_cell7 := View{width: 0 height: Fit th_head7 := MusicColHead{width: Fill text: ""}}
                                th_cell8 := View{width: 0 height: Fit th_head8 := MusicColHead{width: Fill text: ""}}
                                th_cell9 := View{width: 0 height: Fit th_head9 := MusicColHead{width: Fill text: ""}}
                                th_cell10 := View{width: 0 height: Fit th_head10 := MusicColHead{width: Fill text: ""}}
                                th_cell11 := View{width: 0 height: Fit th_head11 := MusicColHead{width: Fill text: ""}}
                                th_cell12 := View{width: 0 height: Fit th_head12 := MusicColHead{width: Fill text: ""}}
                                }
                                // Stands in for the row's headphone + queue
                                // chips, so a head sits over its own column
                                // rather than 24 points to the right of it.
                                MusicLabel{width: 50 text: ""}
                                // What the filters left, and what the background
                                // passes are doing about the rest — at the
                                // explorer's right edge, level with the heads.
                                //
                                // It used to hold a whole line for one short
                                // string. Up here it costs the width of the string
                                // and nothing when there is no string: FIT, never
                                // Fill, because a Fill would claim the right end of
                                // the row while empty and stand every head off its
                                // own column for nothing.
                                //
                                // While a count IS showing the heads do sit that
                                // much to the left of their cells. That is the
                                // trade this placement makes, and the count is
                                // short and comes and goes.
                                music_prep_status := MusicLabel{width: Fit text: ""}
                            }
                            music_tracks := mod.widgets.VjTrackList{show_queue_button: true}
                        }
                            // The console strip: always one line of live numbers, and the
                            // app's own log when it is opened. It sits under the
                            // explorer, where the list it reports on is.
                            console_strip := View{
                                width: Fill
                                height: Fit
                                flow: Down
                                console_grip := RoundedView{
                                    visible: false
                                    width: Fill
                                    height: 7
                                    cursor: MouseCursor.RowResize
                                    show_bg: true
                                    draw_bg +: {
                                        color: #xffffff1f
                                        color_hover: #xffffff5c
                                        border_radius: 3.0
                                        hover: instance(0.0)
                                    }
                                }
                                View{
                                    width: Fill
                                    height: 24
                                    flow: Right
                                    spacing: 6
                                    align: Align{x: 0.0, y: 0.5}
                                    console_chevron_up := ChevronIcon{
                                        visible: false
                                        draw_icon +: { svg: crate_resource("self:resources/icons/chevron_up.svg") }
                                    }
                                    console_chevron_down := ChevronIcon{
                                        draw_icon +: { svg: crate_resource("self:resources/icons/chevron_down.svg") }
                                    }
                                    // One line, and it says so: the row is
                                    // twenty-four points and a second line
                                    // would be drawn where there is nothing
                                    // to draw it in. What does not fit ends
                                    // in an ellipsis rather than vanishing.
                                    console_line := MusicLabel{
                                        width: Fill
                                        max_lines: 1
                                        text_overflow: TextOverflow.Ellipsis
                                        text: ""
                                    }
                                    console_view_0 := MusicChipButton{height: 20 text: "numbers"}
                                    console_view_1 := MusicChipButton{height: 20 text: "log"}
                                    console_view_2 := MusicChipButton{height: 20 text: "both"}
                                }
                                console_body := View{
                                    visible: false
                                    width: Fill
                                    height: 160
                                    flow: Down
                                    spacing: 4
                                    console_numbers := MusicLabel{width: Fill text: ""}
                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Right
                                        spacing: 6
                                        align: Align{x: 0.0, y: 0.5}
                                        console_filter := TextInput{
                                            width: Fill{min: 96. max: 320.}
                                            flow: Flow.Right{wrap: false}
                                            empty_text: "filter…"
                                        }
                                        console_clear := MusicChipButton{height: 20 text: "clear"}
                                    }
                                    console_log := MusicLabel{width: Fill text: ""}
                                }
                            }
                        }
                        b: View{
                            width: Fill
                            height: Fill
                            queue_drop := RoundedView{
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
                                // The list's own name, where the word QUEUE
                                // used to be: a console with several set
                                // lists has to say which one is on the decks,
                                // and this costs the crowded header nothing.
                                queue_name := Label{
                                    width: Fit
                                    text: "SET LIST"
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
                                queue_lists := MusicChipButton{
                                    height: 20
                                    text: "LISTS"
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
                            music_queue := mod.widgets.VjTrackList{
                                show_queue_button: false
                                show_unqueue_button: true
                            }
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
                loops_drop := RoundedView{
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 4
                    draw_bg +: {
                        color: #x00000000
                        border_color: #x00000000
                        border_size: 1.0
                        border_radius: 8.0
                    }
                    // The loop page's own row, where the explorer keeps its
                    // search: which deck the grid shows, the engine switch,
                    // and the score popup.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0, y: 0.5}
                        splat_deck_a := MusicButton{width: 26 height: 22 text: "A"}
                        splat_deck_b := MusicButton{width: 26 height: 22 text: "B"}
                        splat_on := MusicButton{width: 36 height: 22 text: "ON"}
                        View{width: Fill height: Fit}
                        // Correcting the grid of the deck named to the
                        // left. Here rather than in the deck head because
                        // the head has no width left and this page already
                        // says which deck it is about: ONE makes the beat
                        // under the playhead the first of the bar, HERE
                        // pulls the nearest ruling onto the playhead, and
                        // the four ratios are the octave and the two
                        // musical thirds the detector confuses.
                        MusicLabel{text: "GRID"}
                        grid_one := MusicButton{width: 34 height: 22 text: "1"}
                        grid_here := MusicButton{width: 44 height: 22 text: "here"}
                        // The same correction by a hair, for a grid whose
                        // tempo is right and whose beats sit early or late:
                        // the ear is the judge and the record keeps
                        // playing. Shift walks five hairs at once.
                        grid_earlier := MusicButton{width: 24 height: 22 text: "◂"}
                        grid_later := MusicButton{width: 24 height: 22 text: "▸"}
                        grid_double := MusicButton{width: 34 height: 22 text: "×2"}
                        grid_halve := MusicButton{width: 34 height: 22 text: "÷2"}
                        grid_two_thirds := MusicButton{width: 38 height: 22 text: "×⅔"}
                        grid_three_quarters := MusicButton{width: 38 height: 22 text: "×¾"}
                        grid_tap := MusicButton{width: 40 height: 22 text: "tap"}
                        grid_undo := MusicButton{width: 44 height: 22 text: "undo"}
                        grid_lock := MusicButton{width: 40 height: 22 text: "lock"}
                        // Measure this record again from nothing: for the
                        // case where the analysis is simply wrong and a
                        // model has been installed, or the stems have
                        // arrived, since the last look at it.
                        grid_rescan := MusicButton{width: 56 height: 22 text: "re-scan"}
                        View{width: Fill height: Fit}
                        splat_score := MusicButton{width: 52 height: 22 text: "score"}
                    }
                    loop_splat := mod.widgets.VjLoopSplat{}
                }
            }
        }

            loop_score_panel := RoundedView{
                visible: false
                width: 700
                height: 240
                flow: Down
                padding: Inset{left: 6.0 right: 6.0 bottom: 6.0}
                cursor: MouseCursor.Default
                capture_overload: true
                draw_bg +: {
                    color: #x171c22
                    border_color: #x38424d
                    border_size: 1.0
                    border_radius: 8.0
                }
                View{
                    width: Fill
                    height: 24
                    flow: Right
                    spacing: 4
                    align: Align{x: 0.0 y: 0.5}
                    loop_score_title := Label{
                        width: Fill
                        height: 18
                        text: "select a loop cell"
                        draw_text.color: #xf4f7fa
                        draw_text.text_style: theme.font_bold{font_size: 10}
                    }
                    loop_score_play := MusicButton{
                        width: 28
                        height: 22
                        padding: 0
                        text: "▶"
                    }
                    loop_score_stop := MusicButton{
                        width: 28
                        height: 22
                        padding: 0
                        text: "■"
                    }
                    loop_score_loop := MusicChipButton{
                        height: 22
                        text: "LOOP"
                    }
                    loop_score_close := MusicButton{
                        width: 24
                        height: 22
                        text: "×"
                    }
                }
                loop_score := mod.widgets.ScoreView{
                    width: Fill
                    height: Fill
                    fit: ScoreFit.Content
                    hide_labels: true
                    // The app is dark; the score uses its designed dark
                    // palette (charcoal paper, warm ink), never an inversion.
                    dark: true
                    draw_bg +: {color: #x20211f}
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
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "Stem separation"}
                        stem_separation := DropDown{
                            labels: ["Off" "AI hub" "Local"]
                            selected_item: 1
                        }
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        auto_vocal := MusicButton{width: 110 height: 22 text: "VOCAL GUARD"}
                        auto_phrase := MusicButton{width: 110 height: 22 text: "PHRASE SNAP"}
                    }
                    // Its own line: three of these do not fit the panel's
                    // width, and a clipped label is worse than a short row.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        auto_choose := MusicButton{width: 110 height: 22 text: "CHOOSE NEXT"}
                        auto_exit := MusicButton{width: 110 height: 22 text: "PICK EXIT"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        auto_route := MusicButton{width: 110 height: 22 text: "PICK SHAPE"}
                        auto_suggest := MusicButton{width: 110 height: 22 text: "ASK FIRST"}
                    }
                    // The arc: how long the night is and what shape it
                    // takes. Read only while CHOOSE NEXT is doing the
                    // choosing — a dial nothing consults is worse than no
                    // dial at all.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}
                        MusicLabel{width: 62 text: "SET"}
                        auto_length := MusicButton{width: 62 height: 22 text: "\u{2014}"}
                        auto_curve := MusicButton{width: 78 height: 22 text: "BUILD"}
                    }
                    // Why the last record was chosen, in the scorer's own
                    // words so the label and the plan cannot disagree.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}
                        auto_why := MusicLabel{width: Fill text: ""}
                        auto_good := MusicButton{width: 30 height: 22 text: "+"}
                        auto_bad := MusicButton{width: 30 height: 22 text: "-"}
                        auto_go := MusicButton{width: 42 height: 22 text: "GO"}
                        auto_veto := MusicButton{width: 78 height: 22 text: "NOT THAT"}
                    }
                    // Three verbs that act on the queue and the pending
                    // transition directly, rather than configuring how
                    // future ones get planned.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        auto_fade_now := MusicButton{width: 100 height: 22 text: "FADE NOW"}
                        auto_skip := MusicButton{width: 78 height: 22 text: "SKIP"}
                        auto_add_random := MusicButton{width: 100 height: 22 text: "+ RANDOM"}
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

        // The preprocessing dialog: what the app may work out about a track
        // before anyone asks to play it, and where it keeps the answers.
        //
        // Four passes down the left, two source columns across — the listing
        // and the set list — because those are genuinely different appetites.
        // Separating six hundred records is hours of device time nobody
        // asked for; separating the next three in the set list is exactly
        // what should be running while the current one plays.
        prep_modal := Modal{
            can_dismiss: true
            content +: {
                width: 460
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
                        text: "PREPROCESSING"
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 11}
                    }
                    Label{
                        width: Fill
                        text: "Work done ahead of the set, so the columns are filled before you need them."
                        draw_text.color: #x8e9aa7
                        draw_text.text_style.font_size: 9
                    }
                    // The header for the two tick columns. Fixed widths that
                    // match the rows below, so the ticks line up under their
                    // own words instead of drifting with the label lengths.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: Fill text: ""}
                        MusicLabel{width: 90 text: "EXPLORER"}
                        MusicLabel{width: 70 text: "SET LIST"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: Fill text: "STEMS"}
                        prep_stems_explorer := CheckBox{width: 90 text: ""}
                        prep_stems_queue := CheckBox{width: 70 text: ""}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: Fill text: "KARAOKE"}
                        prep_karaoke_explorer := CheckBox{width: 90 text: ""}
                        prep_karaoke_queue := CheckBox{width: 70 text: ""}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: Fill text: "KEY"}
                        prep_key_explorer := CheckBox{width: 90 text: ""}
                        prep_key_queue := CheckBox{width: 70 text: ""}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: Fill text: "BPM"}
                        prep_bpm_explorer := CheckBox{width: 90 text: ""}
                        prep_bpm_queue := CheckBox{width: 70 text: ""}
                    }
                    // Tempo and key fall out of ONE pass over the samples, so
                    // asking for either buys both. Said here rather than
                    // discovered by an operator wondering why unticking BPM
                    // changed nothing.
                    Label{
                        width: Fill
                        text: "Key and BPM come from one pass — either one fills both columns."
                        draw_text.color: #x6f7b87
                        draw_text.text_style.font_size: 9
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "TRACKS AHEAD"}
                        prep_ahead := ValueInput{
                            width: 70
                            min: 1.0
                            max: 500.0
                            step: 1.0
                            precision: 0
                        }
                        MusicLabel{width: Fill text: "per source"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "FIRST MINUTE"}
                        prep_fast := MusicChipButton{height: 20 text: "FAST"}
                        MusicLabel{width: Fill text: "columns sooner; a deck still measures the whole record"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "AT ONCE"}
                        prep_concurrency := ValueInput{
                            width: 70
                            min: 1.0
                            max: 8.0
                            step: 1.0
                            precision: 0
                        }
                        // Separation and transcription hold the one device the
                        // show is drawing on; they stay serial whatever this
                        // says, and saying so here is cheaper than an operator
                        // discovering it from a stuttering output.
                        MusicLabel{width: Fill text: "key/bpm only — stems stay serial"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "CACHE FOLDER"}
                        prep_cache_path := MusicLabel{width: Fill text: ""}
                        prep_cache_browse := MusicButton{width: 70 height: 22 text: "Browse"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_progress := MusicLabel{width: Fill text: ""}
                    }
                    // What a fresh load clears. Off is what the tab has
                    // always done: the channel strip an operator has set
                    // stands across a load, because the tone and the trim
                    // describe the ROOM rather than the record. They are
                    // separable because they answer to different hands.
                    //
                    // Here rather than on the deck: this is how somebody
                    // likes to work, set once and left, not a control
                    // reached for during a set.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "A LOAD CLEARS"}
                        reset_speed := CheckBox{width: 86 text: "tempo"}
                        reset_key := CheckBox{width: 86 text: "key"}
                        reset_eq := CheckBox{width: 86 text: "EQ"}
                        MusicLabel{width: Fill text: ""}
                    }
                    // Three and three: the dialog is four hundred points
                    // wide and six words do not fit on one line of it.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: ""}
                        reset_filter := CheckBox{width: 86 text: "filter"}
                        reset_gain := CheckBox{width: 86 text: "trim"}
                        reset_stems := CheckBox{width: 86 text: "lanes"}
                        MusicLabel{width: Fill text: ""}
                    }
                    // What a LAUNCH clears. Off is what the tab has always
                    // done: a channel muted on the mix page comes back
                    // muted, whatever the deck on it is about to do. Beside
                    // the row above because it is the same thought a launch
                    // later -- set once and left, not reached for mid-set.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "A LAUNCH CLEARS"}
                        launch_clears_dj_mutes := CheckBox{width: 170 text: "the DJ channel mutes"}
                        MusicLabel{width: Fill text: "a fader is not a mute"}
                    }
                    // Where the equalizer's three bands meet. The equalizer's
                    // own setting, so it lives here with the equalizer rather
                    // than on the effects rack -- and one setting for every
                    // chain, because the split is the desk's EQ character,
                    // not a record's and not a channel's. Set once and left,
                    // like the row above it.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "EQ SPLIT"}
                        sfx_eq_low_hz := Slider{
                            width: 170
                            text: "low | mid"
                            min: 80.0
                            max: 800.0
                            default: 250.0
                            taper: Log
                            unit: "Hz"
                            precision: 0
                        }
                        MusicLabel{width: Fill text: ""}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: ""}
                        sfx_eq_high_hz := Slider{
                            width: 170
                            text: "mid | high"
                            min: 1000.0
                            max: 8000.0
                            default: 2500.0
                            taper: Log
                            unit: "Hz"
                            precision: 0
                        }
                        MusicLabel{width: Fill text: "every chain, and it survives a restart"}
                    }
                    // The one line under the lists, and whether it names
                    // what most recently went wrong. Off is the line the
                    // strip has always shown; a fault is otherwise only in
                    // the log, which is shut in a booth. Set once and left,
                    // like the rows above it.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "THE ONE LINE"}
                        console_faults := CheckBox{width: 170 text: "names the newest fault"}
                        MusicLabel{width: Fill text: "cleared by opening the log"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 110 text: "COLUMNS"}
                        prep_columns_open := MusicButton{
                            width: 150
                            height: 22
                            text: "CHOOSE COLUMNS"
                        }
                        // Artist, album, genre, year and bitrate come out of
                        // the file's own tags: no pass here fills them, and
                        // none needs to.
                        MusicLabel{width: Fill text: "tags need no pass"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 1.0, y: 0.5}
                        // The surgical answer, next to the blunt one. A
                        // wrong tempo on one record is a reason to measure
                        // that record again, not to throw away a library
                        // that took a night to work out -- and until this
                        // was here, CLEAR ALL DATA was the only way to ask.
                        // No confirmation: what it forgets it immediately
                        // sets about replacing, which is the opposite of
                        // the button beside it.
                        prep_rescan_picked := MusicButton{
                            width: 150
                            height: 22
                            text: "RE-SCAN PICKED"
                        }
                        prep_clear := MusicButton{
                            width: 130
                            height: 22
                            text: "CLEAR ALL DATA"
                            draw_icon +: { svg: crate_resource("self:resources/icons/trash.svg") }
                            draw_bg +: {
                                color: #x3a1f1f
                                color_focus: #x3a1f1f
                                color_hover: #x4a2626
                                color_down: #x2c1717
                                border_color: #xff5c3966
                            }
                            draw_text +: { color: #xff8a6a }
                        }
                        prep_close := MusicButton{width: 60 height: 22 text: "Close"}
                    }
                }
            }
        }

        // One Yes/No for both destructive answers this page can need: moving
        // the cache and emptying it. The host writes the words and remembers
        // which question it asked, so there is one dialog rather than two
        // that drift apart.
        prep_confirm_modal := Modal{
            can_dismiss: true
            content +: {
                width: 420
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
                        border_radius: 5.0
                    }
                    prep_confirm_title := Label{
                        text: ""
                        draw_text.color: #xe8eef4
                        draw_text.text_style: theme.font_bold{font_size: 11}
                    }
                    prep_confirm_body := Label{
                        width: Fill
                        text: ""
                        draw_text.color: #x8e9aa7
                        draw_text.text_style.font_size: 9
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 1.0 y: 0.5}
                        prep_confirm_no := MusicButton{width: 60 height: 22 text: "No"}
                        prep_confirm_yes := MusicButton{width: 90 height: 22 text: "Yes"}
                    }
                }
            }
        }


        // The columns dialog: which columns a list shows, and in what order.
        //
        // One dialog serves both lists, with a switch at the top saying which
        // one is being edited, because the two want genuinely different sets
        // — the set list is a 320-point column — and two dialogs would be two
        // places for the same twelve rows to drift apart.
        //
        // Twelve rows, one per column, in the list's CURRENT order: the row
        // order IS the answer, so moving a row is the whole gesture. A hidden
        // column keeps its place in the order rather than falling to the
        // bottom, so unticking something and ticking it back puts it where it
        // was.
        prep_columns_modal := Modal{
            can_dismiss: true
            content +: {
                width: 420
                height: Fit
                RoundedView{
                    width: Fill
                    height: Fit
                    padding: 20
                    spacing: 6
                    flow: Down
                    draw_bg +: {
                        color: #x16161b
                        border_color: #xffffff18
                        border_size: 1.0
                        border_radius: 6.0
                    }
                    Label{
                        text: "COLUMNS"
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 11}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: Fit text: "EDITING"}
                        prep_cols_explorer := MusicChipButton{height: 20 text: "EXPLORER"}
                        prep_cols_queue := MusicChipButton{height: 20 text: "SET LIST"}
                        prep_cols_note := MusicLabel{width: Fill text: ""}
                    }
                    // How the KEY column is written. A reading habit, not
                    // three different facts: the estimate and the wheel
                    // underneath are the same whichever is lit, and the
                    // column's ORDER never changes with it.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: Fit text: "KEY AS"}
                        prep_key_wheel := MusicChipButton{height: 20 text: "8A"}
                        prep_key_open := MusicChipButton{height: 20 text: "1m"}
                        prep_key_names := MusicChipButton{height: 20 text: "Am"}
                        MusicLabel{width: Fill text: ""}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show0 := CheckBox{width: 26 text: ""}
                        prep_col_label0 := MusicLabel{width: Fill text: ""}
                        prep_col_up0 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down0 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show1 := CheckBox{width: 26 text: ""}
                        prep_col_label1 := MusicLabel{width: Fill text: ""}
                        prep_col_up1 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down1 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show2 := CheckBox{width: 26 text: ""}
                        prep_col_label2 := MusicLabel{width: Fill text: ""}
                        prep_col_up2 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down2 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show3 := CheckBox{width: 26 text: ""}
                        prep_col_label3 := MusicLabel{width: Fill text: ""}
                        prep_col_up3 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down3 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show4 := CheckBox{width: 26 text: ""}
                        prep_col_label4 := MusicLabel{width: Fill text: ""}
                        prep_col_up4 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down4 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show5 := CheckBox{width: 26 text: ""}
                        prep_col_label5 := MusicLabel{width: Fill text: ""}
                        prep_col_up5 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down5 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show6 := CheckBox{width: 26 text: ""}
                        prep_col_label6 := MusicLabel{width: Fill text: ""}
                        prep_col_up6 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down6 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show7 := CheckBox{width: 26 text: ""}
                        prep_col_label7 := MusicLabel{width: Fill text: ""}
                        prep_col_up7 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down7 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show8 := CheckBox{width: 26 text: ""}
                        prep_col_label8 := MusicLabel{width: Fill text: ""}
                        prep_col_up8 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down8 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show9 := CheckBox{width: 26 text: ""}
                        prep_col_label9 := MusicLabel{width: Fill text: ""}
                        prep_col_up9 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down9 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show10 := CheckBox{width: 26 text: ""}
                        prep_col_label10 := MusicLabel{width: Fill text: ""}
                        prep_col_up10 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down10 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show11 := CheckBox{width: 26 text: ""}
                        prep_col_label11 := MusicLabel{width: Fill text: ""}
                        prep_col_up11 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down11 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        prep_col_show12 := CheckBox{width: 26 text: ""}
                        prep_col_label12 := MusicLabel{width: Fill text: ""}
                        prep_col_up12 := MusicButton{width: 26 height: 20 text: "UP"}
                        prep_col_down12 := MusicButton{width: 26 height: 20 text: "DN"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 1.0, y: 0.5}
                        prep_cols_reset := MusicButton{width: 80 height: 22 text: "Reset"}
                        prep_cols_close := MusicButton{width: 60 height: 22 text: "Close"}
                    }
                }
            }
        }


        // The shelf. One list is on the decks; the rest wait here with their
        // own names, locks and pick orders. Eight fixed rows, filled per
        // open from the shelf itself -- the same fixed-slot idiom the
        // columns dialog and the row menu use, because a DSL cannot grow a
        // list at runtime.
        set_lists_modal := Modal{
            can_dismiss: true
            content +: {
                width: 460
                height: Fit
                RoundedView{
                    width: Fill
                    height: Fit
                    padding: 20
                    spacing: 6
                    flow: Down
                    draw_bg +: {
                        color: #x16161b
                        border_color: #xffffff18
                        border_size: 1.0
                        border_radius: 6.0
                    }
                    Label{
                        text: "SET LISTS"
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 10}
                    }
                    sl_note := MusicLabel{width: Fill text: ""}
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick0 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name0 := MusicLabel{width: Fill text: ""}
                        sl_lock0 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop0 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick1 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name1 := MusicLabel{width: Fill text: ""}
                        sl_lock1 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop1 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick2 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name2 := MusicLabel{width: Fill text: ""}
                        sl_lock2 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop2 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick3 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name3 := MusicLabel{width: Fill text: ""}
                        sl_lock3 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop3 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick4 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name4 := MusicLabel{width: Fill text: ""}
                        sl_lock4 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop4 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick5 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name5 := MusicLabel{width: Fill text: ""}
                        sl_lock5 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop5 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick6 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name6 := MusicLabel{width: Fill text: ""}
                        sl_lock6 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop6 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_pick7 := MusicChipButton{width: 60 height: 20 text: "PLAY"}
                        sl_name7 := MusicLabel{width: Fill text: ""}
                        sl_lock7 := MusicChipButton{width: 56 height: 20 text: "LOCK"}
                        sl_drop7 := MusicButton{width: 26 height: 20 text: "X"}
                    }
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        sl_rename := TextInput{
                            width: Fill
                            height: 22
                            empty_text: "name this list&"
                        }
                        sl_new := MusicButton{width: 70 height: 22 text: "+ NEW"}
                        sl_close := MusicButton{width: 60 height: 22 text: "Close"}
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
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0, y: 0.5}
                            MusicLabel{width: 90 text: "MIN GAP"}
                            scan_gap_secs_dec := MusicButton{width: 22 height: 22 text: "-"}
                            scan_gap_secs := TextInput{width: 60 text: "2"}
                            scan_gap_secs_inc := MusicButton{width: 22 height: 22 text: "+"}
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
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0, y: 0.5}
                            MusicLabel{width: 90 text: "MIN GAP"}
                            scan_gap_beats_dec := MusicButton{width: 22 height: 22 text: "-"}
                            scan_gap_beats := TextInput{width: 60 text: "4"}
                            scan_gap_beats_inc := MusicButton{width: 22 height: 22 text: "+"}
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
                    // door already uses for its two brains. Lit OVERLAP
                    // lets the finds lie over each other, and MIN GAP then
                    // reads IN to IN; unlit, no two finds may touch and the
                    // gap is the clear air between them.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        MusicLabel{width: 90 text: "OVERLAP"}
                        scan_overlap := MusicButton{width: 110 height: 22 text: "ALLOWED"}
                    }
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
                    // Filing, not sound. SORT puts the row in playing order
                    // on the numbers it already holds -- the gaps stay where
                    // they are; PACK closes them and renumbers from the
                    // first pad. A sibling row rather than the one above:
                    // 146 and 146 with the gap between them already fill
                    // the dialog's three hundred points exactly.
                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 0.0, y: 0.5}
                        scan_sort_loops := MusicButton{width: 146 height: 22 text: "SORT BY TIME"}
                        scan_pack_loops := MusicButton{width: 146 height: 22 text: "PACK NUMBERS"}
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
                    Label{
                        width: Fill
                        text: "MORE MODELS"
                        draw_text.color: #xff5c39
                        draw_text.text_style: theme.font_bold{font_size: 10}
                    }
                    hub_model_install_panel := mod.widgets.ModelInstallPanel{
                        width: Fill
                        height: 220
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
    /// The tile column under the playhead.
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
#[derive(Clone, PartialEq)]
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

/// How a pair of columns becomes one, a level up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reduce {
    /// Every channel takes the larger of the two.
    PerChannel,
    /// The alpha still takes the larger — heights must not change — but the
    /// other three come from whichever column was LOUDER.
    ///
    /// For channels that describe what a column SOUNDED LIKE, a per-channel
    /// maximum is a lie: it takes the bass from one instant and the treble
    /// from another and reports a moment that never happened. Pulled back far
    /// enough every band reaches its own ceiling somewhere in the span, so
    /// they all converge and the description dissolves into grey. Measured
    /// over the analysed library, half the columns lose their colouring by
    /// four reductions and two thirds by eight — which is the whole-track
    /// strip, the view most often glanced at.
    BandsOfTheLouder,
}

/// One level up: pairs of columns become one, by the given rule.
pub fn reduce_once(below: &[[u8; 4]], reduce: Reduce) -> Vec<[u8; 4]> {
    let mut level = Vec::with_capacity(below.len().div_ceil(2));
    for pair in below.chunks(2) {
        let mut column = pair[0];
        if let Some(second) = pair.get(1) {
            match reduce {
                Reduce::PerChannel => {
                    for channel in 0..4 {
                        column[channel] = column[channel].max(second[channel]);
                    }
                }
                Reduce::BandsOfTheLouder => {
                    if second[3] > column[3] {
                        column = *second;
                    }
                    column[3] = pair[0][3].max(second[3]);
                }
            }
        }
        level.push(column);
    }
    level
}

/// Build a pyramid from four-channel columns.
///
/// Level 0 is the source resolution; each level above reduces the pair below
/// it — the alpha always by MAX, so peaks survive all the way up, which is
/// what makes a pulled-back view read as music instead of mush and what stops
/// a zoomed-out waveform aliasing.
pub fn build_pyramid_with(
    cx: &mut Cx,
    columns: &[[u8; 4]],
    reduce: Reduce,
) -> Option<WavePyramid> {
    if columns.is_empty() {
        return None;
    }
    let width = TILE_TEX_WIDTH.min(columns.len().max(1));
    let mut pyramid: Vec<Vec<[u8; 4]>> = vec![columns.to_vec()];
    while pyramid.last().map(|level| level.len()).unwrap_or(0) > 1
        && pyramid.len() < MAX_WAVE_LEVELS
    {
        let below = pyramid.last().expect("checked");
        pyramid.push(reduce_once(below, reduce));
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
    build_pyramid_with(cx, &tiles.zoom, Reduce::BandsOfTheLouder)
}

/// The stem-share pyramid, laid out identically to the band one so the
/// shader can sample both with the same level selection: red = vocals,
/// green = drums, blue = bass, alpha = other.
pub fn stem_texture(cx: &mut Cx, columns: &[[u8; 4]]) -> Option<WavePyramid> {
    // Per channel here, because these four are shares of one column rather
    // than a description plus a level: there is no "louder of the two" to
    // take the rest from.
    build_pyramid_with(cx, columns, Reduce::PerChannel)
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

/// The unseparated wave's grey, as the shader declares it.
pub const WAVE_GREY: [f32; 3] = [0.545, 0.596, 0.651];

/// The most colour a band reading is allowed to carry.
///
/// This is the number that keeps a band-coloured column from being mistaken
/// for a separated one, and it is a CAP rather than a hope: whatever the
/// three bands do, the drawn colour cannot pass it. Measured over the
/// analysed library it holds every column at or under 0.39 saturation, while
/// the least colourful of the four stem colours is 0.62 and the other three
/// are above 0.82. The closest any real column ever comes to a stem it could
/// be confused with is 0.27 of saturation away.
pub const BAND_CHROMA: f32 = 0.24;

/// What one unseparated column is COLOURED like, from its three bands.
///
/// The bands are already on the GPU — the level channel beside them is what
/// decides height — and until now the colour ignored them, so a record the
/// separator had not reached drew as one flat grey and said nothing about
/// where its bass was.
///
/// Three properties, in the order they matter:
///
/// * **Brightness never moves.** Every colour this returns has the grey's
///   own luma. Brightness in this lane already says two other things — what
///   has been played, and whether the deck is the active one — and a third
///   meaning would collide with both.
/// * **Colourfulness is capped**, at [`BAND_CHROMA`], so the reading can
///   never climb into the range the stem colours live in.
/// * **Hue carries the whole message**: which band is loudest, on the
///   texture's own red-green-blue = low-mid-high axis. A column with its
///   three bands level has nothing to say and draws neutral.
pub fn band_tint(tile: [u8; 4]) -> [f32; 3] {
    let luma = |c: [f32; 3]| c[0] * 0.299 + c[1] * 0.587 + c[2] * 0.114;
    let grey_y = luma(WAVE_GREY);
    let band = [tile[0] as f32 / 255.0, tile[1] as f32 / 255.0, tile[2] as f32 / 255.0];
    let top = band[0].max(band[1]).max(band[2]);
    // Nothing measured here at all: keep the grey rather than invent a hue
    // for a column that has no content to describe.
    if top < 0.004 {
        return WAVE_GREY;
    }
    // Against the column's own loudest band, so the hue is the BALANCE of
    // the three and not their loudness — loudness is the level channel's
    // job, and this must not say it twice.
    let unit = [band[0] / top, band[1] / top, band[2] / top];
    let mid = luma(unit);
    let off = [unit[0] - mid, unit[1] - mid, unit[2] - mid];
    let spread = off[0].max(off[1]).max(off[2]) - off[0].min(off[1]).min(off[2]);
    if spread < 1e-6 {
        return [grey_y, grey_y, grey_y];
    }
    let scale = (BAND_CHROMA / spread).min(1.0);
    [grey_y + off[0] * scale, grey_y + off[1] * scale, grey_y + off[2] * scale]
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
    /// How long the record is, in source seconds. Only the warning uses
    /// it; zero means "not known yet", which never warns.
    pub duration_secs: f64,
    pub grid: Option<TrackGrid>,
    /// The running loop in source seconds — the tile timebase, so this
    /// converts to columns exactly the way the grid does.
    pub loop_span: Option<(f64, f64)>,
    /// One-based loop slot shown at the overlay's top-left.
    pub loop_slot: Option<u8>,
    /// Where the body of the record begins and ends, in source seconds --
    /// the two edges the automation aims at. `None` when the analysis did
    /// not actually find them: the fallback it hands back in that case is a
    /// fraction of the duration, and a rule drawn across the lane saying
    /// "the intro ends here" would be a measurement that never happened.
    /// The strip still draws the guess, where it sits among the whole
    /// record and reads as one.
    pub body: Option<(f64, f64)>,
    /// The operator's own marks, in source seconds — the tile timebase,
    /// like `loop_span`. The cue, the saved slots with the colour each
    /// is wearing, and the loops the finder offered. Named apart from
    /// `loop_slot` above, which is a different thing: that one is which
    /// pad the RUNNING loop came from.
    pub cue_secs: f64,
    pub saved_slots: Vec<(u16, f64, f64, u32)>,
    pub found_loops: Vec<(f64, f64)>,
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
    /// forward at the deck's rate, and wrapped through a running loop.
    ///
    /// Without the wrap the drawn head walks out past the loop's end and
    /// is yanked back on the next host sample -- once every time round,
    /// which at a one-beat loop is several times a second and reads as
    /// the picture tearing rather than as a loop.
    ///
    /// The wrap is gated on the LAST SAMPLED position being inside the
    /// span, never the predicted one. A head still on its way into a loop
    /// has not been captured by it yet, and folding it there would
    /// teleport it forward into a lap it has not run.
    pub fn position_at(&self, now: f64) -> f64 {
        if !self.playing || self.scratching {
            return self.position_secs;
        }
        let elapsed = (now - self.stamp).clamp(0.0, 0.5);
        let ahead = (self.position_secs + elapsed * self.rate.max(0.0)).max(0.0);
        match self.loop_span {
            Some((start, end))
                if end > start && self.position_secs >= start && self.position_secs < end =>
            {
                start + (ahead - start).rem_euclid(end - start)
            }
            _ => ahead,
        }
    }

    /// How hard the end-of-track warning should show, 0..1.
    ///
    /// A ramp over the last `warn_secs`, times a one-per-second pulse
    /// that never quite reaches nothing -- a warning that blinked fully
    /// out would be invisible exactly half the time, and the point is to
    /// be caught out of the corner of an eye while looking at the other
    /// deck. `warn_secs <= 0.0` is the operator's off switch: the ramp's
    /// own window is then empty and never contains anything, so this
    /// needs no separate enabled flag.
    ///
    /// Computed HERE, at draw time, from the same `now` the playhead
    /// uses. Worked out by the host at pump cadence instead, the pulse
    /// would judder against a scroll that is smooth.
    pub fn warn_at(&self, now: f64, warn_secs: f64) -> f32 {
        if !self.playing || self.duration_secs <= 0.0 {
            return 0.0;
        }
        let left = self.duration_secs - self.position_at(now);
        if !(0.0..warn_secs).contains(&left) {
            return 0.0;
        }
        let ramp = (warn_secs - left) / warn_secs;
        let phase = now.rem_euclid(1.0);
        let pulse = 0.45 + 0.55 * (1.0 - (phase * 2.0 - 1.0).abs());
        (ramp * pulse).clamp(0.0, 1.0) as f32
    }

    /// The tile column under the playhead.
    pub fn head_column(&self) -> f64 {
        self.position_secs * ZOOM_COLS_PER_SEC
    }

    /// The tile column under the playhead at `now`.
    pub fn head_column_at(&self, now: f64) -> f64 {
        self.position_at(now) * ZOOM_COLS_PER_SEC
    }

    /// The nearest mark strictly ahead of `at_secs` -- the cue, every
    /// saved loop's start, every found loop's start -- or `None` when
    /// nothing left in the record is marked. Source seconds, the same
    /// space `position_secs` lives in.
    pub fn next_mark_secs(&self, at_secs: f64) -> Option<f64> {
        std::iter::once(self.cue_secs)
            .chain(self.saved_slots.iter().map(|entry| entry.1))
            .chain(self.found_loops.iter().map(|span| span.0))
            .filter(|&secs| secs > at_secs)
            .fold(None, |best: Option<f64>, secs| Some(best.map_or(secs, |b| b.min(secs))))
    }

    /// What to show next to the playhead for the nearest mark ahead of
    /// it: beats when the grid can count them (what a DJ actually plans
    /// around -- "two bars to the cue"), seconds when it cannot.
    pub fn next_mark_label(&self, now: f64) -> Option<String> {
        let at = self.position_at(now);
        let next = self.next_mark_secs(at)?;
        match self.grid.filter(|grid| grid.has_grid()) {
            Some(grid) => {
                let beats = (grid.beat_at(next) - grid.beat_at(at)).max(0.0);
                Some(format!("{beats:.0} beats"))
            }
            None => Some(crate::clock::countdown(next - at)),
        }
    }

    /// The zoom for ONE lane, from the shared one.
    ///
    /// The tiles are in SOURCE columns, so a record running fast crosses
    /// more of them per second than a slow one. At a shared zoom that puts
    /// two tempo-matched decks on screen at different beat widths, sliding
    /// past each other at different speeds -- which is precisely what two
    /// decks side by side are being looked at to compare. Scaling each
    /// lane by what its own platter is turning at cancels the source
    /// timebase out: a beat is the same number of pixels on both, and they
    /// scroll together.
    fn lane_zoom(cols_per_px: f32, rate: f64) -> f32 {
        // A stopped or reversed deck keeps a readable zoom rather than
        // collapsing the lane to a single column.
        (cols_per_px as f64 * rate.abs().max(0.01)) as f32
    }

    /// Where a mark at `secs` of SOURCE time lands on this lane, in
    /// screen x: the tile column it sits on, measured from the column
    /// under the middle of the lane, over this lane's own zoom.
    ///
    /// The whole-track strip answers the same question by a fraction of
    /// the record's length. That is the other surface's space and cannot
    /// be borrowed: this one scrolls, and what is under the middle
    /// changes every frame.
    pub fn mark_x(secs: f64, centre_col: f64, lane_cols: f64, middle_x: f64) -> f64 {
        middle_x + (secs * ZOOM_COLS_PER_SEC - centre_col) / lane_cols.max(1e-4)
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

/// The eight directions a text outline is stamped in. Same geometry the
/// karaoke reader's line outline uses (`views.rs`), kept as its own copy
/// here rather than shared: the two are unrelated widgets that happen to
/// need the same ring, not one feature split across two files.
const WAVE_TEXT_OUTLINE_RING: [(f64, f64); 8] = [
    (-1.0, 0.0),
    (1.0, 0.0),
    (0.0, -1.0),
    (0.0, 1.0),
    (-0.7, -0.7),
    (0.7, -0.7),
    (-0.7, 0.7),
    (0.7, 0.7),
];

/// A bar number, loop-slot number or seek readout drawn directly on the
/// wave has to survive whatever colour the waveform happens to be under
/// it -- a bright peak reads as no number at all for a flat-coloured
/// glyph. A dark ring stamped under the fill, no depth step needed: this
/// is plain 2D overlay drawing, so draw ORDER already puts the ring
/// under the fill, unlike the karaoke line's own outline which shares a
/// depth-tested pass with video and needs one.
fn draw_outlined_text(draw_text: &mut DrawText, cx: &mut Cx2d, pos: DVec2, text: &str, fill: Vec4f) {
    let ring = (draw_text.text_style.font_size as f64 * 0.09).max(1.0);
    draw_text.color = Vec4f { x: 0.0, y: 0.0, z: 0.0, w: 0.85 };
    for (dx, dy) in WAVE_TEXT_OUTLINE_RING {
        draw_text.draw_abs(cx, dvec2(pos.x + dx * ring, pos.y + dy * ring), text);
    }
    draw_text.color = fill;
    draw_text.draw_abs(cx, pos, text);
}

/// What the surface reports back to the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaveEvent {
    /// A pointer landed on a lane.
    ScratchStart { deck: DeckId },
    /// The finger is at `secs` on the record and travelling at `rate`.
    /// A PLACE as well as a speed: the place is what closes the loop so
    /// nothing lost on the way is lost for good, and the speed is measured
    /// here, on the side that has the pointer timestamps.
    ScratchRate { deck: DeckId, secs: f64, rate: f32 },
    ScratchEnd { deck: DeckId },
    /// The reverse hold, on the same surface and the same hand: the record
    /// runs backwards while it is down and lands where it would have been.
    CensorStart { deck: DeckId },
    CensorEnd { deck: DeckId },
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
    /// The mark chips. Two shapes and not four: a chip hanging from the
    /// top edge and a chip standing on the bottom one. The colour is a
    /// per-instance field, so one drawer serves the cue and every saved
    /// slot's own hue without a uniform lingering between draws.
    /// The body edges. A rule the full height of the lane rather than a
    /// chip, because nobody placed it and nothing can be done to it: it is
    /// a fact about the record, like the ruling it crosses.
    #[live]
    draw_body_edge: DrawColor,
    #[live]
    draw_mark_top: DrawColor,
    #[live]
    draw_mark_bottom: DrawColor,
    #[live]
    draw_text: DrawText,
    #[rust]
    area: Area,
    #[rust]
    lanes: [WaveLane; 2],
    #[rust(ZOOM_DEFAULT_SECS)]
    zoom_secs: f64,
    #[rust(HEAD_FRACTION_DEFAULT)]
    head_fraction: f64,
    #[rust(WARN_SECS_DEFAULT)]
    warn_secs: f64,
    #[rust]
    lane_rects: [Rect; 2],
    /// The viewport centre captured when a loop becomes active. The wave
    /// stays put at the user's current zoom while its head crosses the band.
    #[rust]
    loop_centres: [Option<f64>; 2],
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
    /// Where the record was when the finger landed, and where the finger
    /// landed. Everything the drag publishes is measured from this pair,
    /// so a pointer that goes back where it started puts the record back
    /// where it started.
    anchor_secs: f64,
    anchor_x: f64,
    /// The last place published, so an idle pointer can say "still here"
    /// rather than having to say it in speed.
    last_secs: f64,
    /// This drag is a reverse HOLD, not a scrub. Its rate is fixed, so no
    /// pointer motion and no idle decay may retune it.
    censor: bool,
}

impl VjWaveScroll {
    pub fn set_lane(&mut self, cx: &mut Cx, deck: DeckId, lane: WaveLane) {
        let stamp = cx.seconds_since_app_start();
        let index = deck.index();
        if self.lanes[index].loop_span != lane.loop_span {
            self.loop_centres[index] = lane.loop_span.map(|(start, end)| {
                if end > start {
                    lane.position_secs.clamp(start, end) * ZOOM_COLS_PER_SEC
                } else {
                    lane.position_secs * ZOOM_COLS_PER_SEC
                }
            });
        }
        self.lanes[index] = WaveLane { stamp, ..lane };
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
    pub fn set_loop_span(
        &mut self,
        cx: &mut Cx,
        deck: DeckId,
        span: Option<(f64, f64)>,
        slot: Option<u8>,
    ) {
        let index = deck.index();
        let lane = &mut self.lanes[index];
        if lane.loop_span == span && lane.loop_slot == slot {
            return;
        }
        if lane.loop_span != span {
            self.loop_centres[index] = span.map(|(start, end)| {
                if end > start {
                    lane.position_secs.clamp(start, end) * ZOOM_COLS_PER_SEC
                } else {
                    lane.position_secs * ZOOM_COLS_PER_SEC
                }
            });
        }
        lane.loop_span = span;
        lane.loop_slot = slot;
        self.area.redraw(cx);
    }

    /// Push the deck's stem knobs into the lane: a layer shrinks as its
    /// knob comes down and vanishes when it is killed.
    /// The marks for one deck. Named as the strip's are, so the two
    /// surfaces read the same at the call site.
    pub fn set_body(&mut self, cx: &mut Cx, deck: DeckId, body: Option<(f64, f64)>) {
        let lane = &mut self.lanes[deck.index()];
        if lane.body == body {
            return;
        }
        lane.body = body;
        self.area.redraw(cx);
    }

    pub fn set_cue_marker(&mut self, cx: &mut Cx, deck: DeckId, secs: f64) {
        let lane = &mut self.lanes[deck.index()];
        if (lane.cue_secs - secs).abs() < 1e-9 {
            return;
        }
        lane.cue_secs = secs;
        self.area.redraw(cx);
    }

    pub fn set_loop_slots(&mut self, cx: &mut Cx, deck: DeckId, slots: &[(u16, f64, f64, u32)]) {
        let lane = &mut self.lanes[deck.index()];
        if lane.saved_slots == slots {
            return;
        }
        lane.saved_slots = slots.to_vec();
        self.area.redraw(cx);
    }

    pub fn set_found_loops(&mut self, cx: &mut Cx, deck: DeckId, spans: &[(f64, f64)]) {
        let lane = &mut self.lanes[deck.index()];
        if lane.found_loops == spans {
            return;
        }
        lane.found_loops = spans.to_vec();
        self.area.redraw(cx);
    }

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

    /// Set once from the settings file at startup, not a live gesture: an
    /// operator's preferred balance of history against lookahead is a
    /// thing decided once, not dragged mid-set the way zoom is.
    pub fn set_head_fraction(&mut self, cx: &mut Cx, fraction: f64) {
        let fraction = fraction.clamp(HEAD_FRACTION_MIN, HEAD_FRACTION_MAX);
        if (fraction - self.head_fraction).abs() > 1e-9 {
            self.head_fraction = fraction;
            self.area.redraw(cx);
        }
    }

    /// Set once from the settings file at startup, same as the head
    /// position: how long before the end of a record the lane starts
    /// warning, or 0.0 to turn the warning off entirely.
    pub fn set_warn_secs(&mut self, cx: &mut Cx, secs: f64) {
        let secs = secs.clamp(WARN_SECS_MIN, WARN_SECS_MAX);
        if (secs - self.warn_secs).abs() > 1e-9 {
            self.warn_secs = secs;
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
        (-delta_x * self.secs_per_px(width) / delta_secs) as f32
    }

    /// How far along the record a pointer travel of `delta_x` moves it,
    /// in the same direction convention as the rate above.
    fn drag_offset(&self, width: f64, delta_x: f64) -> f64 {
        -delta_x * self.secs_per_px(width)
    }

    fn secs_per_px(&self, width: f64) -> f64 {
        if width <= 1.0 {
            0.0
        } else {
            self.zoom_secs / width
        }
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
            if let Some(drag) = self.drag.filter(|drag| !drag.censor) {
                let now = cx.seconds_since_app_start();
                if now - drag.idle_since > SCRATCH_IDLE_SECS {
                    // Still here, and no longer moving. The place stands;
                    // only the speed goes to zero.
                    self.events.push(WaveEvent::ScratchRate {
                        deck: drag.deck,
                        secs: drag.last_secs,
                        rate: 0.0,
                    });
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
                // Shift on the lane is the reverse hold rather than a
                // scrub: the same hand, on the same surface, where the
                // reversal is visible.
                let censor = fe.mod_shift();
                // Where the record is, as this surface last heard it. It
                // is a display frame old at worst, and the loop closes
                // that much of a disagreement inside a tenth of a second.
                let anchor_secs = self.lanes[deck.index()].position_secs;
                self.drag = Some(DragState {
                    deck,
                    last_x: fe.abs.x,
                    last_time: now,
                    idle_since: now,
                    anchor_secs,
                    anchor_x: fe.abs.x,
                    last_secs: anchor_secs,
                    censor,
                });
                self.events.push(if censor {
                    WaveEvent::CensorStart { deck }
                } else {
                    WaveEvent::ScratchStart { deck }
                });
                self.next_frame = cx.new_next_frame();
            }
            Hit::FingerMove(fe) => {
                let Some(mut drag) = self.drag else { return };
                // A reverse hold is a fixed rate. A twitch of the pointer
                // must not turn it into a scrub.
                if drag.censor {
                    return;
                }
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
                // Measured from the anchor, not accumulated from the last
                // event: a sum of hops loses whatever a coalesced or
                // dropped event carried, and this is the number the whole
                // change exists to make exact.
                let secs = drag.anchor_secs + self.drag_offset(width, fe.abs.x - drag.anchor_x);
                drag.last_x = fe.abs.x;
                drag.last_time = now;
                drag.idle_since = now;
                drag.last_secs = secs;
                self.drag = Some(drag);
                self.events.push(WaveEvent::ScratchRate { deck: drag.deck, secs, rate });
            }
            Hit::FingerUp(_) => {
                if let Some(drag) = self.drag.take() {
                    self.events.push(if drag.censor {
                        WaveEvent::CensorEnd { deck: drag.deck }
                    } else {
                        WaveEvent::ScratchEnd { deck: drag.deck }
                    });
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
        // A playing deck redraws every frame: the normal waveform scrolls,
        // while a loop keeps the waveform still and advances only its head.
        if self.lanes.iter().any(|lane| lane.playing) {
            self.next_frame = cx.new_next_frame();
        }
        self.note_frame(cx.cx, now);
        // Two lanes with a ruler gutter between them.
        let gutter = 14.0f64;
        let lane_h = ((rect.size.y - gutter) * 0.5).max(8.0);
        let cols_per_px = (self.zoom_secs * ZOOM_COLS_PER_SEC / rect.size.x) as f32;
        let mut moving_heads = [false; 2];
        for index in 0..2 {
            let y = if index == 0 {
                rect.pos.y
            } else {
                rect.pos.y + lane_h + gutter
            };
            let lane_rect = Rect { pos: dvec2(rect.pos.x, y), size: dvec2(rect.size.x, lane_h) };
            self.lane_rects[index] = lane_rect;
            let lane = &self.lanes[index];
            let lane_cols = WaveLane::lane_zoom(cols_per_px, lane.rate);
            match lane.pyramid.as_ref() {
                Some(pyramid) => {
                    self.draw_lane.draw_vars.set_texture(0, &pyramid.texture);
                    self.draw_lane.tex_w = pyramid.width.max(1) as f32;
                    self.draw_lane.tex_h = pyramid.height.max(1) as f32;
                    // Pick the pyramid levels this zoom needs and hand the
                    // shader their row offsets: the whole zoom is a uniform.
                    let (lo, lo_scale, hi, hi_scale, blend) =
                        pyramid.levels_for(lane_cols as f64);
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
            self.draw_lane.cols = lane.cols as f32;
            // Snap the scroll to whole device pixels. A sub-pixel offset
            // makes every column's sample point crawl between neighbours
            // frame to frame, which reads as a shimmer over the whole wave.
            let head = lane.head_column_at(now);
            let loop_columns = lane.loop_columns();
            moving_heads[index] = loop_columns.is_some();
            // A loop captures the viewport where it engaged. The waveform
            // and user zoom stay untouched while the in-shader head moves
            // through (and wraps inside) the highlighted source span.
            let raw_centre = self.loop_centres[index].filter(|_| moving_heads[index]).unwrap_or(head);
            let columns_per_pixel = lane_cols as f64;
            let centre = if columns_per_pixel > 0.0 {
                (raw_centre / columns_per_pixel).round() * columns_per_pixel
            } else {
                raw_centre
            };
            self.draw_lane.centre_col = centre as f32;
            self.draw_lane.cols_per_px = lane_cols;
            set_head_fraction_uniform(&mut self.draw_lane, cx, self.head_fraction as f32);
            self.draw_lane.head_col = head as f32;
            self.draw_lane.head_on = if moving_heads[index] { 1.0 } else { 0.0 };
            set_loop_color_uniform(&mut self.draw_lane, cx, deck_accent(if index == 0 { DeckId::A } else { DeckId::B }));
            set_loop_span_uniform(&mut self.draw_lane, cx, loop_columns);
            set_preview_span_uniform(&mut self.draw_lane, cx, None);
            let (beat_cols, phase) = lane.grid_columns().unwrap_or((0.0, 0.0));
            self.draw_lane.beat_cols = beat_cols as f32;
            self.draw_lane.beat_phase = phase as f32;
            self.draw_lane.active = if lane.playing { 1.0 } else { 0.55 };
            set_warn_uniform(&mut self.draw_lane, cx, lane.warn_at(now, self.warn_secs));
            self.draw_lane.draw_abs(cx, lane_rect);
        }

        // The operator's own marks, over the wave. The same shapes and
        // colours the whole-track strip uses -- the two surfaces are one
        // visual language -- but never its coordinate space: the strip
        // maps a fraction of the whole record, this maps tile columns
        // around the head. Found loops ride the bottom edge and everything
        // else the top, which is what keeps the two rows from colliding.
        //
        // Drawn in the strip's own order, so a coincidence resolves the
        // same way on both: the cue under the saved slots, because the
        // numbered pad is the one that has to be read correctly to be
        // played. All of it under the head, which is the only thing here
        // that is happening now.
        for index in 0..2 {
            if !self.lanes[index].loaded {
                continue;
            }
            let lane_cols =
                WaveLane::lane_zoom(cols_per_px, self.lanes[index].rate).max(1e-4) as f64;
            // The captured centre only while a loop is really running --
            // the same filter the waveform itself takes. A span that never
            // opened can leave a centre latched, and marks standing still
            // against a scrolling wave are worse than no marks.
            let centre = self.loop_centres[index]
                .filter(|_| moving_heads[index])
                .unwrap_or_else(|| self.lanes[index].head_column_at(now));
            let lane_rect = self.lane_rects[index];
            let (chip_w, chip_h) = (9.0f64, 11.0f64);
            let middle_x = rect.pos.x + rect.size.x * self.head_fraction;
            let x_of = |secs: f64| WaveLane::mark_x(secs, centre, lane_cols, middle_x);
            // Off the lane is SKIPPED, never clamped: a chip parked at the
            // edge would claim a mark is there when it is seconds away.
            let visible = |x: f64| {
                x + chip_w * 0.5 >= lane_rect.pos.x
                    && x - chip_w * 0.5 <= lane_rect.pos.x + lane_rect.size.x
            };
            // Snapped to whole pixels. The chip is nine wide with a
            // one-pixel stroke: at a fractional x that stroke straddles
            // two pixels and halves its weight in each, and since the
            // centre moves every frame the blur crawls as the wave
            // scrolls. The same reason the waveform snaps its own centre.
            let top_at = |x: f64| Rect {
                pos: dvec2((x - chip_w * 0.5).round(), lane_rect.pos.y),
                size: dvec2(chip_w, chip_h),
            };
            let bottom_at = |x: f64| Rect {
                pos: dvec2(
                    (x - chip_w * 0.5).round(),
                    lane_rect.pos.y + lane_rect.size.y - chip_h,
                ),
                size: dvec2(chip_w, chip_h),
            };

            // Where the body begins and ends, under everything a hand
            // placed: this is a fact the analysis noticed, and it must not
            // sit on top of a mark somebody put there on purpose.
            if let Some((intro_end, outro_start)) = self.lanes[index].body {
                for at in [intro_end, outro_start] {
                    let x = x_of(at);
                    if x >= lane_rect.pos.x && x <= lane_rect.pos.x + lane_rect.size.x {
                        self.draw_body_edge.draw_abs(
                            cx,
                            Rect {
                                pos: dvec2(x.round(), lane_rect.pos.y),
                                size: dvec2(1.0, lane_rect.size.y.max(1.0)),
                            },
                        );
                    }
                }
            }

            for k in 0..self.lanes[index].found_loops.len() {
                let (start, _) = self.lanes[index].found_loops[k];
                let x = x_of(start);
                if visible(x) {
                    self.draw_mark_bottom.color = Vec4f::from_u32(0xf5c542ff);
                    self.draw_mark_bottom.draw_abs(cx, bottom_at(x));
                }
            }
            let cue_x = x_of(self.lanes[index].cue_secs);
            if visible(cue_x) {
                self.draw_mark_top.color = Vec4f::from_u32(0xe5484dff);
                self.draw_mark_top.draw_abs(cx, top_at(cue_x));
            }
            for k in 0..self.lanes[index].saved_slots.len() {
                let (_, start, _, colour) = self.lanes[index].saved_slots[k];
                let x = x_of(start);
                if visible(x) {
                    self.draw_mark_top.color = Vec4f::from_u32(colour);
                    self.draw_mark_top.draw_abs(cx, top_at(x));
                }
            }
        }

        // Slot number at the visible top-left of the active band. Text is
        // direct-drawn over the same lane; the overlay remains one widget.
        for index in 0..2 {
            let lane = &self.lanes[index];
            let lane_cols = WaveLane::lane_zoom(cols_per_px, lane.rate);
            let Some(slot) = lane.loop_slot.filter(|_| moving_heads[index]) else { continue };
            let Some((start, end)) = lane.loop_columns() else { continue };
            let centre = self.loop_centres[index].unwrap_or_else(|| lane.head_column_at(now));
            let start_x = rect.pos.x + rect.size.x * self.head_fraction
                + (start - centre) / lane_cols.max(1e-4) as f64;
            let end_x = rect.pos.x + rect.size.x * self.head_fraction
                + (end - centre) / lane_cols.max(1e-4) as f64;
            let lane_rect = self.lane_rects[index];
            if end_x < lane_rect.pos.x || start_x > lane_rect.pos.x + lane_rect.size.x {
                continue;
            }
            self.draw_text.text_style.font_size = 9.0;
            draw_outlined_text(
                &mut self.draw_text,
                cx,
                dvec2(start_x.max(lane_rect.pos.x) + 3.0, lane_rect.pos.y + 2.0),
                &slot.to_string(),
                Vec4f::from_u32(0xf4f7faff),
            );
        }

        // Bar numbers, ruled off whichever deck is leading the view.
        let bar_number_color = Vec4f::from_u32(0x8e9aa7ff);
        let ruler = if self.lanes[0].grid.is_some() { 0 } else { 1 };
        let lane = &self.lanes[ruler];
        let lane_cols = WaveLane::lane_zoom(cols_per_px, lane.rate);
        if let Some((beat_cols, phase)) = lane.grid_columns() {
            let bar_cols = beat_cols * 4.0;
            if bar_cols > 1.0 {
                let centre = self.loop_centres[ruler]
                    .filter(|_| moving_heads[ruler])
                    .unwrap_or_else(|| lane.head_column_at(now));
                // The columns this lane actually shows, which is its own
                // zoom across the width -- not the shared one.
                let half_cols = rect.size.x * lane_cols as f64 * 0.5;
                let first = ((centre - half_cols - phase) / bar_cols).floor();
                let last = ((centre + half_cols - phase) / bar_cols).ceil();
                // Thin the labels out when the bars crowd together.
                let px_per_bar = bar_cols / lane_cols.max(1e-4) as f64;
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
                        let x = rect.pos.x + rect.size.x * self.head_fraction
                            + (col - centre) / lane_cols.max(1e-4) as f64;
                        if x >= rect.pos.x && x <= rect.pos.x + rect.size.x - 12.0 {
                            draw_outlined_text(
                                &mut self.draw_text,
                                cx,
                                dvec2(x + 2.0, rect.pos.y + lane_h + 1.0),
                                &format!("{}", bar + 1),
                                bar_number_color,
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

        // Unlooped lanes keep the familiar fixed centre head. A looping
        // lane draws its moving head in the waveform shader instead.
        if !moving_heads[0] && !moving_heads[1] {
            self.draw_head.draw_abs(
                cx,
                Rect {
                    pos: dvec2(rect.pos.x + rect.size.x * self.head_fraction - 6.0, rect.pos.y),
                    size: dvec2(12.0, rect.size.y),
                },
            );
        } else {
            for index in 0..2 {
                if moving_heads[index] {
                    continue;
                }
                let lane_rect = self.lane_rects[index];
                self.draw_head.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(lane_rect.pos.x + lane_rect.size.x * self.head_fraction - 6.0, lane_rect.pos.y),
                        size: dvec2(12.0, lane_rect.size.y),
                    },
                );
            }
        }
        // What's coming: beats or time to the nearest mark ahead of the
        // playhead, right beside the head line rather than back at the
        // strip -- the strip answers WHERE in the whole record, this
        // answers HOW SOON, which is the question actually asked while
        // playing.
        for index in 0..2 {
            let Some(label) = self.lanes[index].next_mark_label(now) else { continue };
            let lane_rect = self.lane_rects[index];
            let head_x = lane_rect.pos.x + lane_rect.size.x * self.head_fraction;
            self.draw_text.text_style.font_size = 9.0;
            draw_outlined_text(
                &mut self.draw_text,
                cx,
                dvec2(head_x + 9.0, lane_rect.pos.y + 2.0),
                &label,
                Vec4f::from_u32(0xf4f7faff),
            );
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// whole-track overview
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
struct WaveLoadView {
    phase: f32,
    progress: Option<f32>,
    /// The deck is already playing what has arrived: the waveform stays
    /// in view and the progress is a thin line under it, not a curtain.
    playable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverviewEvent {
    /// The green marker was clicked: keep the running loop as a blue one.
    SaveLoop,
    /// A blue marker was clicked: go into that saved loop again.
    RecallLoop { slot: u16 },
    /// The wheel over a mark: move it by a hair, without the grid's say.
    NudgeMark { hit: MarkerHit, delta_secs: f64 },
    /// A blue marker was dragged off its spot: forget that saved loop.
    DeleteLoop { slot: u16 },
    /// One saved loop dragged onto another: exchange what the two numbers
    /// hold. The gesture costs no screen room — it refines the drag that
    /// already deletes when it lands on nothing.
    SwapLoop { from: u16, onto: u16 },
    /// The red marker was dragged: CUE now sends the deck here.
    SetCue { secs: f64 },
    /// Click or drag: seek to this fraction of the track.
    Seek { fraction: f64 },
    /// A completed loop drag: put the loop's IN point here. Raw source
    /// seconds — the host owns QUANT, so the policy lives in one place.
    MoveLoop { start_secs: f64 },
    /// One end of the running loop dragged, the other left where it is.
    MoveLoopEdge { out: bool, secs: f64 },
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
    /// Dragging one end of the band. `out` says which.
    MoveLoopEdge { out: bool },
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
    /// Direct-drawn over the same strip: no extra layout and no
    /// platform-specific path. Each deck gives it its header accent.
    #[live]
    draw_load: DrawColor,
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
    /// Number, in, out, colour. The colour comes from the deck, which
    /// resolves the palette -- the strip paints what it is handed.
    loop_slots: Vec<(u16, f64, f64, u32)>,
    /// Scanner-found loops — the yellow chips on the bottom edge.
    #[rust]
    found_loops: Vec<(f64, f64)>,
    /// Where CUE lands — the red chip. Source seconds.
    #[rust]
    cue_secs: f64,
    /// Where the recording itself begins and ends, in source seconds. Not
    /// a mark: a measurement, and the reason CUE lands where it does on a
    /// track that opens with silence.
    #[rust]
    sound: Option<(f64, f64)>,
    /// Where the arrangement turns, in seconds. The analysis has always
    /// worked these out and only the automation ever saw them.
    #[rust]
    changes: Vec<f64>,
    /// The record's shape: the four edges, in order. Drawn under every
    /// chip and moved by the wheel over the strip's middle band, which
    /// nothing else claims.
    #[rust]
    shape: Option<[f64; 4]>,
    /// The chip under the cursor, for the hover scale-up. Pressing takes
    /// it back to normal size — the pressed-down feel.
    #[rust]
    hover_marker: Option<MarkerHit>,
    /// Fractions of a wheel notch not yet spent. See `nudge_delta`.
    #[rust]
    scroll_residue: f64,
    /// The chips themselves: green for the running loop's handle, blue
    /// for a saved one — FCP's marker idiom at strip scale.
    #[live]
    draw_marker_live: DrawQuad,
    #[live]
    draw_marker_saved: DrawColor,
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
    #[live]
    draw_edge_sound: DrawColor,
    #[live]
    draw_edge_body: DrawColor,
    #[live]
    draw_change: DrawColor,
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
    /// The seek target while dragging, and a hovered mark's time. Never
    /// both at once -- a drag owns the readout while it runs.
    #[live]
    draw_text: DrawText,
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
    #[rust]
    load: Option<WaveLoadView>,
    #[rust]
    load_frame: NextFrame,
}

impl VjWaveOverview {
    pub fn set_load(&mut self, cx: &mut Cx, visual: Option<(f32, f32, f32, bool)>) {
        let load = visual.map(|(phase, progress, indeterminate, playable)| WaveLoadView {
            phase,
            progress: (indeterminate < 0.5).then_some(progress),
            playable,
        });
        if self.load == load {
            return;
        }
        self.load = load;
        self.area.redraw(cx);
        if self.load.is_some_and(|load| load.progress.is_none()) {
            self.load_frame = cx.new_next_frame();
        }
    }

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
        // The band between the two marker rows, which nothing else
        // claims: the shape's edges answer there, so the wheel can move
        // them without competing with a chip.
        if let Some(edges) = self.shape {
            let nearest = edges
                .iter()
                .enumerate()
                .map(|(index, at)| (index, (at - secs).abs()))
                .filter(|(_, distance)| *distance <= tol)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((index, _)) = nearest {
                return Some(MarkerHit::Shape(index as u8));
            }
        }
        None
    }

    /// The record's four edges, diffed like the rest.
    pub fn set_changes(&mut self, cx: &mut Cx, changes: &[f64]) {
        if self.changes == changes {
            return;
        }
        self.changes = changes.to_vec();
        self.area.redraw(cx);
    }

    pub fn set_shape(&mut self, cx: &mut Cx, shape: Option<[f64; 4]>) {
        if self.shape == shape {
            return;
        }
        self.shape = shape;
        self.area.redraw(cx);
    }

    /// Where the recording begins and ends, diffed like the rest.
    pub fn set_sound(&mut self, cx: &mut Cx, span: Option<(f64, f64)>) {
        if self.sound == span {
            return;
        }
        self.sound = span;
        self.area.redraw(cx);
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
    pub fn set_loop_slots(&mut self, cx: &mut Cx, slots: &[(u16, f64, f64, u32)]) {
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

    /// One step of a plain seek: preview where the finger is pointing.
    /// Commits on release, same as the snapped ghost seek below -- a
    /// plain drag used to commit on every `FingerMove`, which on a strip
    /// this thin meant one jittery pixel could retarget the deck before
    /// the hand had aimed.
    fn preview_seek(&mut self, cx: &mut Cx, x: f64) {
        let Some((secs, _)) = self.secs_at(cx, x) else { return };
        self.preview = Some((secs, secs + 2.0 / ZOOM_COLS_PER_SEC));
        self.preview_raw = Some(secs);
        self.area.redraw(cx);
    }

    /// The time a hovered mark sits at, for the hover readout.
    fn hover_mark_secs(&self) -> Option<f64> {
        mark_time(
            self.hover_marker?,
            self.loop_span,
            &self.loop_slots,
            &self.found_loops,
            self.cue_secs,
        )
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
        if self.load_frame.is_event(event).is_some()
            && self.load.is_some_and(|load| load.progress.is_none())
        {
            self.area.redraw(cx);
            self.load_frame = cx.new_next_frame();
        }
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
                    // The last few pixels at each end resize; everything
                    // between them still moves the whole band.
                    let edge = self
                        .secs_at(cx, fe.abs.x)
                        .and_then(|(secs, tol)| band_edge(self.loop_span, secs, tol));
                    if let Some(out) = edge {
                        self.drag = Some(OverviewDrag::MoveLoopEdge { out });
                        self.area.redraw(cx);
                        return;
                    }
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
                    self.preview_seek(cx, fe.abs.x);
                }
            }
            Hit::FingerMove(fe) => match self.drag {
                Some(OverviewDrag::Seek) => self.preview_seek(cx, fe.abs.x),
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
                Some(OverviewDrag::MoveLoopEdge { out }) => {
                    // An edge moves LIVE, unlike the whole-band drag: the
                    // loop goes on sounding while it is stretched, which
                    // is the point of being able to stretch it.
                    if let Some((secs, _)) = self.secs_at(cx, fe.abs.x) {
                        self.events.push(OverviewEvent::MoveLoopEdge { out, secs });
                    }
                }
                None => {}
            },
            Hit::FingerUp(fe) => {
                // A release far off the strip's own band -- above or
                // below it, not along it -- abandons a seek rather than
                // committing wherever the finger happened to end up.
                let rect = self.area.rect(cx);
                let seek_aborted = seek_release_aborted(fe.abs.y, rect.pos.y, rect.size.y);
                match (self.drag, self.preview_raw, self.preview) {
                    (Some(OverviewDrag::Marker { hit, origin, at }), _, _) => {
                        let travelled = (at - origin).length();
                        match hit {
                            // The shape's edges are the wheel's, not a
                            // drag's: a chip can be picked up and moved,
                            // an edge of the record cannot.
                            MarkerHit::Shape(_) => {}
                            MarkerHit::Recall(slot) if travelled >= MARKER_DELETE_PX => {
                                // Dropped ON another saved loop, this is a
                                // swap; dropped anywhere else it is the
                                // delete it always was.
                                match self.marker_under(self.area.rect(cx), at) {
                                    Some(MarkerHit::Recall(onto)) if onto != slot => self
                                        .events
                                        .push(OverviewEvent::SwapLoop { from: slot, onto }),
                                    _ => self.events.push(OverviewEvent::DeleteLoop { slot }),
                                }
                            }
                            MarkerHit::Recall(slot) => {
                                self.events.push(OverviewEvent::RecallLoop { slot });
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
                    (Some(OverviewDrag::MoveLoopEdge { .. }), _, _) => {}
                    (Some(OverviewDrag::MoveLoop { .. }), Some(raw), Some(preview)) => {
                        // One event per completed drag — and none for a
                        // drag that came home.
                        if self.loop_span != Some(preview) {
                            self.events.push(OverviewEvent::MoveLoop { start_secs: raw });
                        }
                    }
                    (Some(OverviewDrag::GhostSeek), Some(raw), _) if !seek_aborted => {
                        // The RAW finger position: the engine's snap is the
                        // authority, with its sync-aware reference.
                        let duration = self.cols.max(1) as f64 / ZOOM_COLS_PER_SEC;
                        self.events.push(OverviewEvent::Seek {
                            fraction: (raw / duration).clamp(0.0, 1.0),
                        });
                    }
                    (Some(OverviewDrag::Seek), Some(raw), _) if !seek_aborted => {
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
            // The wheel over a mark moves it by a hair the grid cannot
            // express -- a cue a few milliseconds behind the transient, a
            // loop whose IN sits just inside the kick. Nothing else in
            // this tab reads a scroll here, and no scrolling container
            // surrounds the strip, so the gesture costs nothing.
            Hit::FingerScroll(fe) => {
                let Some(hit) = self.marker_under(self.area.rect(cx), fe.abs) else {
                    self.scroll_residue = 0.0;
                    return;
                };
                let fine = fe.modifiers.shift;
                let delta_secs = nudge_delta(fe.scroll, fine, &mut self.scroll_residue);
                if delta_secs != 0.0 {
                    self.events.push(OverviewEvent::NudgeMark { hit, delta_secs });
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
        set_loop_color_uniform(&mut self.draw_lane, cx, self.draw_load.color);
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
        if let Some(load) = self.load {
            let accent = self.draw_load.color;
            // A deck that is already playing keeps its picture: the bar is
            // a thin line along the bottom edge, where the decoded region
            // grows under the waveform drawing in above it. A deck still
            // waiting gets the curtain and the bar across the middle.
            let track = if load.playable {
                Rect {
                    pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - 3.0),
                    size: dvec2(rect.size.x.max(1.0), 3.0),
                }
            } else {
                self.draw_load.color = Vec4f::from_u32(0x090c10d9);
                self.draw_load.draw_abs(cx, rect);
                Rect {
                    pos: dvec2(rect.pos.x + 12.0, rect.pos.y + rect.size.y * 0.5 - 4.0),
                    size: dvec2((rect.size.x - 24.0).max(1.0), 8.0),
                }
            };
            self.draw_load.color = Vec4f::from_u32(0x2a323cff);
            self.draw_load.draw_abs(cx, track);
            self.draw_load.color = match load.phase as u32 {
                2 => Vec4f::from_u32(0xf5c542ff),
                3 => Vec4f::from_u32(0x35c05fff),
                4 => Vec4f::from_u32(0xe5484dff),
                _ => accent,
            };
            let fill = match load.progress {
                Some(progress) => Rect {
                    pos: track.pos,
                    size: dvec2(
                        track.size.x * progress.clamp(0.0, 1.0) as f64,
                        track.size.y,
                    ),
                },
                None => {
                    let t = (cx.seconds_since_app_start() * 0.8).rem_euclid(2.0);
                    let sweep = if t < 1.0 { t } else { 2.0 - t };
                    let width = (track.size.x * 0.22).clamp(28.0, 84.0).min(track.size.x);
                    self.load_frame = cx.new_next_frame();
                    Rect {
                        pos: dvec2(track.pos.x + (track.size.x - width) * sweep, track.pos.y),
                        size: dvec2(width, track.size.y),
                    }
                }
            };
            if fill.size.x > 0.0 {
                self.draw_load.draw_abs(cx, fill);
            }
            self.draw_load.color = accent;
        }
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
                Some(MarkerHit::Recall(slot)) => self
                    .loop_slots
                    .iter()
                    .find(|entry| entry.0 == slot)
                    .map(|entry| ((entry.1, entry.2), 1u8)),
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
            // The record's shape: four hairlines under every chip. The
            // outer two are where the recording itself starts and stops,
            // so a long silent head reads as one rather than as a track
            // that begins late; the inner two are where the body arrives
            // and leaves, which is what the automation aims at.
            let edges: Vec<f64> = match self.shape {
                Some(edges) => edges.to_vec(),
                None => self.sound.map(|(a, b)| vec![a, b]).unwrap_or_default(),
            };
            for (index, at) in edges.iter().copied().enumerate() {
                // With four edges the middle two are the body's; with two
                // there is only the recording's own extent to show.
                let body = edges.len() == 4 && (index == 1 || index == 2);
                let edge = if body { &mut self.draw_edge_body } else { &mut self.draw_edge_sound };
                edge.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(centre_of(at) - 0.75, rect.pos.y),
                        size: dvec2(1.5, rect.size.y),
                    },
                );
            }
            // The turns, in the strip's middle band so they stay clear of
            // both chip rows, and thinned: a build-up can put changes a few
            // seconds apart, which on a whole-track strip is a picket fence
            // that hides the drop it is marking rather than showing it.
            let band = (rect.size.y - CHANGE_CLEAR * 2.0).max(2.0);
            let mut last_x = f64::NEG_INFINITY;
            for at in self.changes.iter().copied() {
                let x = centre_of(at).round();
                if x - last_x < CHANGE_MIN_PX {
                    continue;
                }
                last_x = x;
                self.draw_change.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x, rect.pos.y + CHANGE_CLEAR),
                        size: dvec2(1.0, band),
                    },
                );
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
            for entry in self.loop_slots.iter().copied() {
                if let Some((MarkerHit::Recall(dragged), origin, at)) = held {
                    if dragged == entry.0 && (at - origin).length() >= MARKER_DELETE_PX {
                        // Dead where it stood: release will delete it, or
                        // put it on whatever number it landed on.
                        continue;
                    }
                }
                self.draw_marker_saved.color = Vec4f::from_u32(entry.3);
                self.draw_marker_saved.draw_abs(
                    cx,
                    chip_sized(centre_of(entry.1), grown(MarkerHit::Recall(entry.0))),
                );
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
                let saved = self.loop_slots.iter().any(|entry| (entry.1 - start).abs() < 1e-6);
                if !saved {
                    self.draw_marker_live
                        .draw_abs(cx, chip_sized(centre_of(start), grown(MarkerHit::Save)));
                }
            }
            // The seek target while a plain or ghost seek is being
            // dragged: the absolute time it would land on and how far
            // that is from where the deck is now, neither of which a
            // strip this thin can make obvious by eye alone.
            if let (Some(OverviewDrag::Seek | OverviewDrag::GhostSeek), Some((target_secs, _))) =
                (self.drag, self.preview)
            {
                let head_secs = self.head * duration;
                self.draw_text.text_style.font_size = 9.0;
                let label = format!(
                    "{} ({})",
                    crate::clock::playhead(target_secs),
                    crate::clock::offset(target_secs - head_secs)
                );
                let x = centre_of(target_secs).clamp(rect.pos.x, rect.pos.x + rect.size.x - 84.0);
                draw_outlined_text(
                    &mut self.draw_text,
                    cx,
                    dvec2(x, rect.pos.y + rect.size.y * 0.5 - 5.0),
                    &label,
                    Vec4f::from_u32(0xf4f7faff),
                );
            } else if self.drag.is_none() {
                // Hovering (not dragging) a mark answers WHEN: the strip
                // already shows WHERE with its hairlines, but a chip's
                // own x position is a few pixels of precision at best.
                if let Some(secs) = self.hover_mark_secs() {
                    self.draw_text.text_style.font_size = 9.0;
                    let x = centre_of(secs).clamp(rect.pos.x, rect.pos.x + rect.size.x - 48.0);
                    draw_outlined_text(
                        &mut self.draw_text,
                        cx,
                        dvec2(x, rect.pos.y + rect.size.y * 0.5 - 5.0),
                        &crate::clock::playhead(secs),
                        Vec4f::from_u32(0xf4f7faff),
                    );
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

/// The seat a row with no key takes: past every real one, so unjudged
/// tracks gather at one end whichever way the column runs.
pub const NO_KEY_ORDER: u8 = u8::MAX;

#[derive(Clone, Debug, PartialEq)]
pub struct TrackRowEntry {
    pub key: TrackKey,
    pub title: String,
    /// Straight out of the file's own tags — no analysis stands behind
    /// these four, which is why they fill in whether or not the
    /// preprocessing lane has ever looked at the track.
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub year: String,
    /// Nominal container bitrate, already carrying its unit.
    pub bitrate: String,
    /// Pre-formatted so the list stays a pure view.
    pub bpm: String,
    pub musical_key: String,
    /// Where that key sits on the wheel, so the column can be ordered
    /// musically without the list having to know what the text means.
    /// [`NO_KEY_ORDER`] for a track nothing has judged.
    pub key_order: u8,
    /// How this record's key sits with what the room is hearing, 0..=1.
    /// `None` when nothing is playing, or when either key is unknown --
    /// which is not the same as a clash and must not be painted like one.
    pub key_fit: Option<f32>,
    pub duration: String,
    pub license: String,
    pub tags: String,
    /// `YYYY-MM-DD`, blank for a track this machine has no timestamp for.
    /// A calendar-day string sorts chronologically as plain text, so the
    /// ADDED column needs no separate machine-sortable field the way KEY
    /// does.
    pub added: String,
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
            album: String::new(),
            genre: String::new(),
            year: String::new(),
            bitrate: String::new(),
            bpm: String::new(),
            musical_key: String::new(),
            key_order: NO_KEY_ORDER,
            key_fit: None,
            duration: String::new(),
            license: String::new(),
            tags: String::new(),
            added: String::new(),
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
/// licence, tags and the queue chip cost ~800 points before the title gets
/// its floor of 180, so a list under ~1040 is already spending pixels it
/// does not have.
pub const LIBRARY_NARROW_WIDTH: f64 = 1040.0;

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
        ..Default::default()
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
    /// Row body pressed with the SECONDARY button: the operator is asking
    /// what this record can do, not moving it. Carries where the press
    /// landed, because that is where the menu belongs.
    Menu(usize, DVec2),
    /// Row body released where it went down: NOW it is a click, and a deck
    /// target loads it. Carries the modifiers so a set-building release
    /// still loads nothing.
    Load(usize, KeyModifiers),
    /// The row's `+` button: queue it. Carries the modifiers so Shift
    /// (play next) and Control (replace the queue) reach the host --
    /// same treatment as the row body's own `Pick`/`Load`.
    Queue(usize, KeyModifiers),
    /// The queue row's minus button: take it back off the set list.
    Unqueue(usize),
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
    /// The queue's remove chip. Explicit rather than inferred from the
    /// absence of `+`: the two lists differ in more than one way, and a
    /// flag that says what it means survives the next one.
    #[live]
    show_unqueue_button: bool,
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
    /// Where the KEYS are standing. A pick says what the hand is about to
    /// drag; the cursor says which row an arrow moves from and which row a
    /// named load acts on. They travel together for a plain move — one idea
    /// on screen, not two — and part only under shift, which walks the
    /// cursor while dragging the pick out behind it.
    #[rust]
    cursor: Option<usize>,
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
    /// Which columns this list shows and in what order, pushed by the host
    /// from the operator's layout. Empty until the first push, which draws
    /// as a title-only list rather than as nothing.
    #[rust]
    columns: Vec<Column>,
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

/// The most columns a row or a header can carry. The templates declare this
/// many generic cells; a layout is never longer, because it is a permutation
/// of the thirteen that exist.
pub const MAX_COLUMNS: usize = 13;

/// The cell ids in the row template, in declaration order. A column's place
/// in the operator's layout picks the cell it draws into, which is what makes
/// the order theirs rather than the template's.
pub const ROW_CELLS: [&[LiveId]; MAX_COLUMNS] = [
    ids!(row_col0),
    ids!(row_col1),
    ids!(row_col2),
    ids!(row_col3),
    ids!(row_col4),
    ids!(row_col5),
    ids!(row_col6),
    ids!(row_col7),
    ids!(row_col8),
    ids!(row_col9),
    ids!(row_col10),
    ids!(row_col11),
    ids!(row_col12),
];

/// The header's boxes and the heads inside them, same order as [`ROW_CELLS`].
pub const HEAD_CELLS: [&[LiveId]; MAX_COLUMNS] = [
    ids!(th_cell0),
    ids!(th_cell1),
    ids!(th_cell2),
    ids!(th_cell3),
    ids!(th_cell4),
    ids!(th_cell5),
    ids!(th_cell6),
    ids!(th_cell7),
    ids!(th_cell8),
    ids!(th_cell9),
    ids!(th_cell10),
    ids!(th_cell11),
    ids!(th_cell12),
];

pub const HEAD_BUTTONS: [&[LiveId]; MAX_COLUMNS] = [
    ids!(th_head0),
    ids!(th_head1),
    ids!(th_head2),
    ids!(th_head3),
    ids!(th_head4),
    ids!(th_head5),
    ids!(th_head6),
    ids!(th_head7),
    ids!(th_head8),
    ids!(th_head9),
    ids!(th_head10),
    ids!(th_head11),
    ids!(th_head12),
];

/// What one column reads for one row. The tick columns are a mark rather
/// than a word — a row either carries that work or it does not.
pub fn column_text(column: Column, entry: &TrackRowEntry) -> String {
    match column {
        Column::Title => entry.title.clone(),
        Column::Artist => entry.artist.clone(),
        Column::Album => entry.album.clone(),
        Column::Genre => entry.genre.clone(),
        Column::Year => entry.year.clone(),
        Column::Bitrate => entry.bitrate.clone(),
        Column::Bpm => entry.bpm.clone(),
        Column::Key => entry.musical_key.clone(),
        Column::Time => entry.duration.clone(),
        Column::Stem => if entry.stem { "✓" } else { "" }.to_string(),
        Column::Krk => if entry.krk { "✓" } else { "" }.to_string(),
        Column::Tags => entry.tags.clone(),
        Column::Added => entry.added.clone(),
    }
}

/// A column's ink. Tempo and key wear their own colours because they are
/// what the eye hunts for while beatmatching; the rest are quieter than the
/// title so a full row still reads title-first.
/// A key that shares most of its notes with what the room is hearing.
/// Below this the two records are far enough round the wheel to be heard
/// arguing.
pub const KEY_FIT_GOOD: f32 = 0.85;

/// The colour a KEY cell is drawn in, given how that key sits with the
/// room. Deliberately only three answers: it is a glance, not a reading.
pub fn key_cell_color(fit: Option<f32>) -> u32 {
    match fit {
        // Nothing to compare against: the column's own colour, exactly as
        // every other cell gets.
        None => column_color(Column::Key),
        Some(fit) if fit >= KEY_FIT_GOOD => 0x9be8b0ff,
        Some(_) => 0xd8776bff,
    }
}

pub fn column_color(column: Column) -> u32 {
    match column {
        Column::Title => 0xd6dee6ff,
        Column::Bpm => 0xff5c39ff,
        Column::Key => 0xc6a0f0ff,
        Column::Stem | Column::Krk => 0x35c05fff,
        Column::Tags => 0x6f7b87ff,
        _ => 0x9fabb7ff,
    }
}

/// The layout size for a column, honouring the narrow console's shrunken
/// tick columns exactly as the header does.
pub fn column_size(column: Column, narrow: bool) -> Size {
    if narrow && matches!(column, Column::Stem | Column::Krk) {
        return Size::Fixed(MARK_COLUMN_NARROW);
    }
    match column.width() {
        ColumnWidth::Fixed(width) => Size::Fixed(width),
        // The title carries the weight so it takes the slack a row has left
        // after every fixed column has been paid.
        //
        // Its MINIMUM is dropped on a narrow list, and that is the whole
        // reason the set list can carry a tempo and a key at all. A floor of
        // 180 points is right for a listing, where the title is what the eye
        // reads; in a 330-point set list it is more than the whole row has
        // to give, and every column after it was clipped away to pay for it.
        // A truncated title beside a tempo is worth more there than a whole
        // title beside nothing.
        ColumnWidth::Fill { min, max } => Size::Fill {
            weight: if matches!(column, Column::Title) { 400.0 } else { 100.0 },
            // A column neither grows from a basis nor gives ground: the
            // weights above are the whole of how the row shares its width.
            basis: FitBound::Abs(0.0),
            shrink: 0.0,
            min: min.filter(|_| !narrow),
            max,
        },
    }
}

impl VjTrackList {
    /// Which columns this list shows, in the operator's order.
    pub fn set_columns(&mut self, cx: &mut Cx, columns: Vec<Column>) {
        if self.columns != columns {
            self.columns = columns;
            self.view.redraw(cx);
        }
    }

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
            // The shift anchor is a row number too, and it was only ever
            // dropped when the picks emptied -- so a rebuild that kept the
            // picks left the anchor measuring from whichever record had
            // landed on that number, and the next shift-click selected a
            // range nobody asked for.
            let anchored =
                self.anchor.and_then(|row| self.entries.get(row).map(|entry| entry.key.clone()));
            // And so is the cursor, for the same reason.
            let stood_on =
                self.cursor.and_then(|row| self.entries.get(row).map(|entry| entry.key.clone()));
            // And so is the viewport's own top row: see `carried_top_row`.
            let list = self.view.portal_list(cx, ids!(list));
            let top = list.first_id();
            let scroll = list.borrow().map(|held| held.first_scroll()).unwrap_or(0.0);
            let carried = carried_top_row(&self.entries, top, &entries);

            self.entries = entries;
            self.selected = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| picked.contains(&entry.key))
                .map(|(row, _)| row)
                .collect();
            self.anchor = anchored
                .and_then(|key| self.entries.iter().position(|entry| entry.key == key));
            self.cursor = stood_on
                .and_then(|key| self.entries.iter().position(|entry| entry.key == key));
            if self.selected.is_empty() {
                self.anchor = None;
            }
            // Only when it actually moved. The listing is rebuilt constantly
            // for reasons that change no order at all, and re-seating the
            // viewport on every one of those would throw away the fraction
            // of a row the operator had scrolled to.
            if let Some(row) = carried.filter(|row| *row != top) {
                list.set_first_id_and_scroll(row, scroll);
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
        // The keys carry on from wherever the hand last was, so a click and
        // then an arrow is one continuous gesture rather than two.
        self.cursor = Some(row);
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
        self.cursor = None;
        self.view.redraw(cx);
    }

    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Move where the keys are standing, and carry the view to it.
    ///
    /// A plain move IS the pick, so an arrow walk leaves exactly one idea on
    /// screen rather than a cursor and a selection disagreeing about which
    /// record the operator means. Under `extend` the cursor walks on its own
    /// and drags the range out behind it from the anchor, which is the same
    /// thing a shift-click does with the mouse.
    pub fn move_cursor(&mut self, cx: &mut Cx, step: crate::library_nav::CursorMove, extend: bool) {
        let list = self.view.portal_list(cx, ids!(list));
        // Measured from the rows the list actually drew, because they are
        // not a uniform height: the previewing row wears a taller template.
        let drawn = list.borrow().map(|held| held.visible_items()).unwrap_or(0);
        let page = crate::library_nav::page_rows(drawn);
        let Some(row) =
            crate::library_nav::step_cursor(self.cursor, self.entries.len(), page, step)
        else {
            return;
        };
        self.cursor = Some(row);
        match extend {
            true => {
                let from = self.anchor.unwrap_or(row);
                let (lo, hi) = if from <= row { (from, row) } else { (row, from) };
                self.selected = (lo..=hi).collect();
            }
            false => {
                self.selected = vec![row];
                self.anchor = Some(row);
            }
        }
        // No-ops when the row is already on screen, so a row-by-row walk in
        // the middle of the listing never animates and only a step off the
        // edge moves the view.
        list.smooth_scroll_to(cx, row, TRACK_SCROLL_SPEED, None, 0.0);
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
                // Without this the listing that emptied under a scrolled
                // viewport hides its own "no tracks" line too, so a search
                // that found nothing and a search that broke look alike.
                if let Some(top) = stranded_viewport_lands_at(list.first_id(), 1) {
                    list.set_first_id_and_scroll(top, 0.0);
                }
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
            if let Some(top) = stranded_viewport_lands_at(list.first_id(), self.entries.len()) {
                list.set_first_id_and_scroll(top, 0.0);
            }
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
                    // Every cell is filled from the layout, never from a
                    // fixed idea of which cell is which column. A cell past
                    // the end of the layout is hidden rather than left
                    // wearing whatever the last row that used it put there.
                    //
                    // The layout is the whole answer. This used to drop every
                    // fixed column on a narrow list to protect the title, so
                    // the set list could show nothing but a name — which made
                    // the tempo and key of the record about to play the two
                    // things the operator could not see. A list too narrow
                    // for what it was given is now the operator's own call,
                    // and one they can take back in the columns dialog.
                    for (slot, cell) in ROW_CELLS.iter().enumerate() {
                        let column = self.columns.get(slot).copied();
                        let shown = column.is_some();
                        let widget = item.widget(cx, cell);
                        widget.set_visible(cx, shown);
                        let Some(column) = column.filter(|_| shown) else { continue };
                        let mut cell_ref = widget.borrow_mut::<Label>();
                        let Some(label) = cell_ref.as_mut() else { continue };
                        label.walk.width = column_size(column, self.narrow);
                        label.draw_text.color = Vec4f::from_u32(match column {
                            Column::Key => key_cell_color(entry.key_fit),
                            other => column_color(other),
                        });
                        drop(cell_ref);
                        item.label(cx, cell).set_text(cx, &column_text(column, entry));
                    }
                    item.button(cx, ids!(row_queue))
                        .set_visible(cx, self.show_queue_button);
                    item.button(cx, ids!(row_unqueue))
                        .set_visible(cx, self.show_unqueue_button);
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
                        // this one, so the stripe apply is skipped whole --
                        // all except where the keys are standing, which this
                        // row says with its border instead.
                        let edge: u32 =
                            if self.cursor == Some(row_id) { 0x8fb4ffff } else { 0x00000000 };
                        let edge = Vec4f::from_u32(edge);
                        script_apply_eval!(cx, item, {
                            draw_bg +: { border_color: #(edge) }
                        });
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
                        let cursor = if self.cursor == Some(row_id) { 1.0f32 } else { 0.0 };
                        script_apply_eval!(cx, item, {
                            draw_bg +: {
                                live: #(live) odd: #(odd) sel: #(sel)
                                carry: #(carry) cursor: #(cursor)
                            }
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

/// How fast the listing travels when a key walks the cursor off the edge of
/// it. Only an edge step scrolls at all -- a walk through rows that are
/// already on screen does not move the view -- so this is the speed of
/// following the cursor, not of browsing.
pub const TRACK_SCROLL_SPEED: f64 = 90.0;

/// Read row clicks out of a frame's actions, the same way the tile grids do.
///
/// `press_travel` is the FARTHEST the live press has been from where it
/// went down, which the host keeps because only it sees the whole gesture.
/// Peak travel, not the release's distance: a carry that goes out and
/// comes back has still been a carry, and letting go over the row it
/// started on must not read as a click on it.
/// Whether a local file's row answers what the operator typed.
///
/// The catalog's search is a posting index reached over HTTP; a local
/// listing has none of that and never had any search at all, so an operator
/// who dropped four hundred files on the window got four hundred rows and no
/// way to narrow them. This is the whole of the local answer: every word of
/// the query has to appear somewhere in the row's own text.
///
/// SUBSTRING, not whole-word, because that is what a filename is like —
/// "opener-final-2" should answer "final". Every word ANDed, so a second
/// word narrows rather than widens. And folded through the same table the
/// catalog index uses, so "cafe" finds "Café" on both sides of the LOCAL
/// FILES switch rather than only one.
pub fn local_row_matches(query: &str, haystack: &str) -> bool {
    let folded = makepad_asset_data::fold::fold_to_ascii(haystack);
    makepad_asset_data::fold::fold_to_ascii(query)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .all(|word| folded.contains(word))
}

/// Which row the viewport should sit on after a re-listing, given the row it
/// sat on before it, or `None` when nothing it was showing survived.
///
/// A viewport is a ROW NUMBER, and a row number means a different record
/// every time the listing is rebuilt -- which the explorer does for every
/// badge, every status change and every keystroke in the search box. Holding
/// the number therefore holds the operator's PLACE only by accident; what
/// they were actually looking at is the record.
///
/// When the record at the top is gone, the walk carries on DOWN the old
/// order and takes the first one that did survive. Down rather than up
/// because a narrowing listing is what usually causes this, and the records
/// after the top one are the ones the operator had not scrolled past yet.
pub fn carried_top_row(
    was: &[TrackRowEntry],
    top: usize,
    now: &[TrackRowEntry],
) -> Option<usize> {
    was.get(top..)?
        .iter()
        .find_map(|entry| now.iter().position(|held| held.key == entry.key))
}

/// Where a list's viewport should land when the listing under it has
/// changed size, or `None` to leave it exactly where the operator put it.
///
/// The list widget re-seats itself only while drawing rows that EXIST, and
/// a row id past the end of the listing draws nothing at all. So a viewport
/// scrolled deep into a long listing that then shrank under it -- the
/// ordinary result of typing into the search box, or of a filter chip --
/// asked for rows that were not there, got none, and left the panel BLANK
/// until the operator thought to scroll back up. During a set, on the
/// surface they are looking at to find the next record.
///
/// Two decisions, both deliberate. It fires ONLY when the viewport is
/// genuinely past the end, so a listing that merely lost a row does not
/// yank the operator somewhere they did not ask to be. And it lands on the
/// TOP rather than on the last row: a stranded position refers to records
/// that are gone, so there is nothing to be near, and a narrowed listing
/// wants to be read from its first row.
pub fn stranded_viewport_lands_at(first_id: usize, rows: usize) -> Option<usize> {
    (first_id >= rows).then_some(0)
}

/// Whether a press on a row body carries that row: picks it, drags it, or
/// loads a deck from it on release.
///
/// The primary button only -- and any touch, which has no second button to
/// offer. A `View` reports its finger events whatever button made them, so
/// without this a press of the SECONDARY button picked the row, a drag with
/// it carried the selection, and the release LOADED A DECK. The worst thing
/// a console can do with a gesture it does not understand is guess, and
/// guessing "put this record on air" during a set is the worst guess
/// available. Everything that is not the primary button means nothing here.
pub fn press_carries_a_row(device: &DigitDevice) -> bool {
    device.is_primary_hit()
}

/// Whether a press on a row body is asking for that record's own menu.
///
/// The secondary button, and only it. A touch has no second button to offer
/// and must keep carrying rows -- the long-press twin that would give a
/// touch console the same menu competes with the row carry, which is how
/// records reach a deck by hand, so it is a separate decision and not made
/// here.
pub fn press_asks_for_a_menu(device: &DigitDevice) -> bool {
    device.mouse_button().is_some_and(|button| button.contains(MouseButton::SECONDARY))
}

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
        } else if let Some(modifiers) = item.button(cx, ids!(row_queue)).clicked_modifiers(actions) {
            TrackListHit::Queue(row_id, modifiers)
        } else if item.button(cx, ids!(row_unqueue)).clicked(actions) {
            TrackListHit::Unqueue(row_id)
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
            if press_asks_for_a_menu(&down.device) {
                TrackListHit::Menu(row_id, down.abs)
            } else if !press_carries_a_row(&down.device) {
                continue;
            } else {
                TrackListHit::Pick(row_id, down.modifiers)
            }
        } else if let Some(moved) = body.finger_move(actions) {
            if !press_carries_a_row(&moved.device) {
                continue;
            }
            // Travelled far enough from where the finger went down: the
            // operator is carrying the picked rows, not choosing one.
            if (moved.abs - moved.abs_start).length() < TRACK_DRAG_SLOP {
                continue;
            }
            TrackListHit::Drag(row_id)
        } else if let Some(up) = body.finger_up(actions) {
            if !press_carries_a_row(&up.device) {
                continue;
            }
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

/// `YYYY-MM-DD` for a millisecond Unix timestamp, blank for `0` or earlier
/// (the sentinel for "this machine has no timestamp for this track").
///
/// No calendar crate in this workspace, so this is the whole of one:
/// days-since-epoch to a civil year/month/day, Howard Hinnant's
/// `civil_from_days` (public-domain algorithm, not tied to any particular
/// implementation), good over any date this column will ever show.
pub fn format_added_date(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}")
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

/// A band or stem knob's readout while a hand is on it: the gain in
/// decibels, or KILL. It is the HAND's position, not what the strip is
/// doing about it -- a killed band's knob still reads what it will come
/// back to. Unity reads "0.0 dB" without a sign, and so does a gain
/// that only rounds to it; an untouched knob should look untouched.
pub fn format_band_gain(gain: f32) -> String {
    if gain < crate::music_dsp::EQ_KILL_EPSILON {
        return "KILL".to_string();
    }
    let db = format!("{:+.1}", crate::dsp_math::ratio_to_db(gain));
    let db = if db == "+0.0" || db == "-0.0" { "0.0".to_string() } else { db };
    format!("{db} dB")
}

/// The sweep knob's readout: OFF about centre, else which filter it is
/// and where its corner sits, from the same function the coefficients
/// come from. Whole hertz below a thousand, tenths of a kilohertz
/// above. ASCII only: the font drops glyphs it lacks.
pub fn format_filter(position: f32) -> String {
    match crate::music_dsp::filter_corner_hz(position) {
        None => "OFF".to_string(),
        Some((highpass, hz)) => {
            let side = if highpass { "HP" } else { "LP" };
            if hz < 1000.0 {
                format!("{side} {}", hz.round() as i64)
            } else {
                format!("{side} {:.1}k", hz / 1000.0)
            }
        }
    }
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

    /// The key cell says at a glance whether a record will sit with the
    /// room, and says nothing at all when there is nothing to sit with.
    #[test]
    fn the_key_cell_is_coloured_by_how_it_sits_with_the_room() {
        let plain = column_color(Column::Key);
        assert_eq!(key_cell_color(None), plain, "no reference is not a clash");
        assert_ne!(key_cell_color(Some(1.0)), plain, "a match reads as one");
        assert_ne!(key_cell_color(Some(0.4)), key_cell_color(Some(1.0)));
        // The threshold is a share of the notes two keys have in common,
        // so a neighbour on the wheel still reads as agreeable.
        assert_eq!(key_cell_color(Some(KEY_FIT_GOOD)), key_cell_color(Some(1.0)));
    }


    fn filled_row() -> TrackRowEntry {
        TrackRowEntry {
            license: String::new(),
            key: TrackKey::Local(PathBuf::from("a.mp3")),
            title: "Title".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            genre: "Genre".into(),
            year: "1998".into(),
            bitrate: "320k".into(),
            bpm: "123.0".into(),
            musical_key: "8A".into(),
            key_order: 14,
            key_fit: None,
            duration: "3:16".into(),
            tags: "Tags".into(),
            added: "2026-09-08".into(),
            stem: true,
            krk: false,
            badge: String::new(),
            live: false,
        }
    }

    #[test]
    fn every_column_reads_its_own_field_and_no_other() {
        let row = filled_row();
        // Each column must pull a DIFFERENT field. A copy-paste slip in the
        // match would show one field under two headings, which reads as data
        // rather than as a bug.
        for (column, expect) in [
            (Column::Title, "Title"),
            (Column::Artist, "Artist"),
            (Column::Album, "Album"),
            (Column::Genre, "Genre"),
            (Column::Year, "1998"),
            (Column::Bitrate, "320k"),
            (Column::Bpm, "123.0"),
            (Column::Key, "8A"),
            (Column::Time, "3:16"),
            (Column::Tags, "Tags"),
            (Column::Added, "2026-09-08"),
        ] {
            assert_eq!(column_text(column, &row), expect, "{column:?}");
        }
        // The two mark columns are a tick or nothing, never a word.
        assert_eq!(column_text(Column::Stem, &row), "✓");
        assert_eq!(column_text(Column::Krk, &row), "");
    }

    fn listing(names: &[&str]) -> Vec<TrackRowEntry> {
        names
            .iter()
            .map(|name| {
                TrackRowEntry::blank(TrackKey::Local(PathBuf::from(*name)), name.to_string())
            })
            .collect()
    }

    #[test]
    fn the_viewport_follows_the_record_it_was_showing() {
        let was = listing(&["a", "b", "c", "d", "e"]);
        // The same records, reordered under it (a sort, or a badge change
        // that re-sorted): the viewport was on "c" and stays on "c".
        let now = listing(&["e", "d", "c", "b", "a"]);
        assert_eq!(carried_top_row(&was, 2, &now), Some(2));
        let now = listing(&["c", "a", "b"]);
        assert_eq!(carried_top_row(&was, 2, &now), Some(0), "narrowed, and 'c' leads it");
    }

    #[test]
    fn a_top_record_that_did_not_survive_hands_the_viewport_to_the_first_one_below_it_that_did() {
        let was = listing(&["a", "b", "c", "d", "e"]);
        // "c" is gone; "d" is the next thing the operator had not yet
        // scrolled past, so the viewport lands on it rather than jumping.
        let now = listing(&["a", "d", "e"]);
        assert_eq!(carried_top_row(&was, 2, &now), Some(1));
    }

    #[test]
    fn a_viewport_showing_nothing_that_survived_asks_for_nothing() {
        let was = listing(&["a", "b", "c"]);
        assert_eq!(carried_top_row(&was, 1, &listing(&["a"])), None, "b and c both gone");
        assert_eq!(carried_top_row(&was, 9, &listing(&["a"])), None, "already past the end");
        assert_eq!(carried_top_row(&was, 0, &[]), None, "nothing to land on at all");
    }

    #[test]
    fn a_local_row_answers_a_word_from_anywhere_in_its_name() {
        let row = "F:/music/crates/opener-final-2.mp3";
        // A filename is not words with spaces between them, so whole-word
        // matching would answer almost nothing an operator types.
        assert!(local_row_matches("final", row));
        assert!(local_row_matches("opener", row));
        assert!(local_row_matches("crates", row), "the folder it sits in counts too");
        assert!(!local_row_matches("closer", row));
    }

    #[test]
    fn every_word_narrows_rather_than_widens() {
        let row = "F:/music/deep house/opener.mp3";
        assert!(local_row_matches("deep opener", row), "both present, in any order");
        assert!(!local_row_matches("deep closer", row), "one missing is no match");
    }

    #[test]
    fn a_local_row_folds_the_same_way_the_catalog_index_does() {
        let row = "F:/music/Café del Mar.mp3";
        assert!(local_row_matches("cafe", row), "the spelling somebody types");
        assert!(local_row_matches("café", row), "and the one on the file");
        // Both sides of the LOCAL FILES switch answer the same question.
        assert!(local_row_matches("CAFE", row));
    }

    #[test]
    fn an_empty_or_punctuation_only_query_matches_everything() {
        // The caller skips the filter for a blank query; this is the second
        // line of that defence, for a query that is all separators.
        for query in ["", "   ", "---", "..."] {
            assert!(local_row_matches(query, "anything at all"), "{query:?}");
        }
    }

    #[test]
    fn a_viewport_still_inside_its_listing_is_left_where_the_operator_put_it() {
        assert_eq!(stranded_viewport_lands_at(0, 10), None);
        assert_eq!(stranded_viewport_lands_at(5, 10), None);
        assert_eq!(
            stranded_viewport_lands_at(9, 10),
            None,
            "the last row is still a row; a listing that lost one must not yank the viewport",
        );
    }

    #[test]
    fn a_viewport_past_the_end_of_a_shrunken_listing_lands_on_the_top() {
        assert_eq!(
            stranded_viewport_lands_at(10, 10),
            Some(0),
            "one past the last row is already asking for a row that is not there",
        );
        assert_eq!(stranded_viewport_lands_at(40, 3), Some(0));
        // The listing that emptied: its placeholder is the one row of a
        // range of one, and it has to be reachable too.
        assert_eq!(stranded_viewport_lands_at(40, 1), Some(0));
    }

    #[test]
    fn only_the_primary_button_carries_a_row() {
        use makepad_widgets::makepad_platform::event::finger::DigitDevice;
        assert!(press_carries_a_row(&DigitDevice::Mouse { button: MouseButton::PRIMARY }));
        // The one that used to load a deck.
        assert!(!press_carries_a_row(&DigitDevice::Mouse { button: MouseButton::SECONDARY }));
        for other in [MouseButton::MIDDLE, MouseButton::BACK, MouseButton::FORWARD] {
            assert!(
                !press_carries_a_row(&DigitDevice::Mouse { button: other }),
                "{other:?} should mean nothing on a row",
            );
        }
        assert!(
            press_carries_a_row(&DigitDevice::Touch { uid: 1 }),
            "a touch has no second button to offer, so it must still carry",
        );
    }

    #[test]
    fn format_added_date_reads_a_millisecond_stamp_as_a_calendar_day() {
        // Known-good dates, computed independently (`date -u -d ... +%s`),
        // including a leap day and a leap CENTURY (2000 is divisible by
        // 400, so it is one) to exercise the era correction.
        assert_eq!(format_added_date(1_788_825_600_000), "2026-09-08");
        assert_eq!(format_added_date(86_400_000), "1970-01-02");
        assert_eq!(format_added_date(1_835_395_200_000), "2028-02-29");
        assert_eq!(format_added_date(951_782_400_000), "2000-02-29");
        // A time within the day still floors to that day.
        assert_eq!(format_added_date(1_788_825_600_000 + 12 * 3_600_000), "2026-09-08");
    }

    #[test]
    fn format_added_date_is_blank_for_the_zero_and_negative_sentinel() {
        assert_eq!(format_added_date(0), "", "no timestamp reads as no timestamp, not epoch");
        assert_eq!(format_added_date(-86_400_000), "");
    }

    #[test]
    fn a_blank_field_reads_as_an_empty_cell_rather_than_a_zero() {
        let row = TrackRowEntry::blank(TrackKey::Local(PathBuf::from("a.mp3")), "T".into());
        for column in crate::columns::ALL {
            if matches!(column, Column::Title) {
                continue;
            }
            assert_eq!(column_text(column, &row), "", "{column:?} invented a value");
        }
    }

    #[test]
    fn the_mark_columns_shrink_with_a_narrow_console_and_nothing_else_does() {
        // The narrow rule is the tick columns' alone: a title that shrank
        // with them would be squeezed twice.
        let fixed = |size: Size| match size {
            Size::Fixed(points) => points,
            other => panic!("expected a fixed width, got {other:?}"),
        };
        assert_eq!(fixed(column_size(Column::Stem, true)), MARK_COLUMN_NARROW);
        assert_eq!(fixed(column_size(Column::Krk, true)), MARK_COLUMN_NARROW);
        assert_eq!(fixed(column_size(Column::Stem, false)), 36.0);
        assert_eq!(
            fixed(column_size(Column::Bpm, true)),
            fixed(column_size(Column::Bpm, false)),
            "only the tick columns answer to a narrow console"
        );
        // The title takes the slack, so it must carry more weight than any
        // other Fill column or a long tag list would starve it.
        let (Size::Fill { weight: title, .. }, Size::Fill { weight: tags, .. }) =
            (column_size(Column::Title, false), column_size(Column::Tags, false))
        else {
            panic!("title and tags are both Fill columns");
        };
        assert!(title > tags, "the title must outweigh the columns beside it");
    }

    #[test]
    fn a_narrow_list_lets_the_title_shrink_so_the_columns_after_it_survive() {
        // The set list is ~330 points wide. A 180-point floor under the
        // title is more than that row has to give once the badge and the two
        // chips are paid, and everything after the title was clipped away —
        // which is exactly the tempo and key the set list exists to show.
        let Size::Fill { min: wide, .. } = column_size(Column::Title, false) else {
            panic!("the title is a Fill column");
        };
        let Size::Fill { min: narrow, .. } = column_size(Column::Title, true) else {
            panic!("the title is a Fill column");
        };
        assert_eq!(wide, Some(180.0), "a listing keeps its floor");
        assert_eq!(narrow, None, "a narrow list gives it up");
        // The fixed columns are unmoved: they are what the floor was
        // crowding out, so shrinking them too would defeat the exercise.
        let fixed = |size: Size| match size {
            Size::Fixed(points) => points,
            other => panic!("expected a fixed width, got {other:?}"),
        };
        assert_eq!(fixed(column_size(Column::Bpm, true)), 54.0);
        assert_eq!(fixed(column_size(Column::Key, true)), 40.0);
    }

    #[test]
    fn the_templates_have_a_cell_for_every_column_there_is() {
        // The row and the header are reordered by index into these arrays,
        // so a column with no cell would silently never draw.
        assert_eq!(ROW_CELLS.len(), MAX_COLUMNS);
        assert_eq!(HEAD_CELLS.len(), MAX_COLUMNS);
        assert_eq!(HEAD_BUTTONS.len(), MAX_COLUMNS);
        assert!(crate::columns::ALL.len() <= MAX_COLUMNS);
    }

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
    fn the_loop_playhead_wraps_without_moving_the_waveform_timebase() {
        let mut lane = lane(120.0, 3.9);
        lane.playing = true;
        lane.stamp = 10.0;
        lane.loop_span = Some((2.0, 4.0));
        assert!((lane.position_at(10.2) - 2.1).abs() < 1e-9);
        assert_eq!(lane.loop_columns(), Some((200.0, 400.0)));
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
    fn the_ends_of_a_band_resize_and_its_middle_still_moves() {
        let span = Some((10.0, 20.0));
        assert_eq!(band_edge(span, 10.1, 0.35), Some(false), "near IN");
        assert_eq!(band_edge(span, 19.9, 0.35), Some(true), "near OUT");
        assert_eq!(band_edge(span, 15.0, 0.35), None, "the middle still moves");
        assert_eq!(band_edge(span, 30.0, 0.35), None, "and outside is neither");
        // A band shorter than two grab zones has no middle: the nearer end
        // wins, so a one-beat loop can still be stretched.
        let tiny = Some((10.0, 10.4));
        assert_eq!(band_edge(tiny, 10.05, 0.35), Some(false));
        assert_eq!(band_edge(tiny, 10.35, 0.35), Some(true));
        assert_eq!(band_edge(None, 10.0, 0.35), None);
    }
    #[test]
    fn no_span_means_every_grab_is_a_seek() {
        assert!(band_grab(None, 12.0, 0.35).is_none());
    }


    #[test]
    fn a_wheel_notch_moves_a_mark_and_a_fraction_of_one_waits() {
        let mut residue = 0.0;
        // A trackpad's continuous stream: nothing moves until a whole
        // notch has arrived, and then exactly one notch does.
        for _ in 0..3 {
            assert_eq!(nudge_delta(dvec2(0.0, -30.0), false, &mut residue), 0.0);
        }
        let step = nudge_delta(dvec2(0.0, -30.0), false, &mut residue);
        assert!((step - MARK_NUDGE_SECS).abs() < 1e-12, "one notch, got {step}");
        // A mouse sends a whole detent at once, and the wheel's sign is
        // the same as the fader ladder's.
        let mut residue = 0.0;
        let step = nudge_delta(dvec2(0.0, 120.0), false, &mut residue);
        assert!((step + MARK_NUDGE_SECS).abs() < 1e-12, "the other way, got {step}");
        // Shift is the fine step.
        let mut residue = 0.0;
        let step = nudge_delta(dvec2(0.0, -120.0), true, &mut residue);
        assert!((step - MARK_NUDGE_FINE_SECS).abs() < 1e-12);
        // A horizontal wheel counts too, and a number that is not one
        // moves nothing.
        let mut residue = 0.0;
        assert!(nudge_delta(dvec2(-120.0, 0.0), false, &mut residue) > 0.0);
        let mut residue = 0.0;
        assert_eq!(nudge_delta(dvec2(0.0, f64::NAN), false, &mut residue), 0.0);
    }
    #[test]
    fn marker_clicks_resolve_nearest_and_blue_beats_green_beats_red() {
        let saved = [(0u16, 10.0, 12.0, 0u32), (1u16, 30.0, 31.0, 0u32)];
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
    fn a_hovered_mark_answers_when_and_a_shape_edge_answers_nothing() {
        let saved = [(3_u16, 40.0, 44.0, 0xff0000ffu32)];
        let found = [(60.0, 64.0)];
        let running = Some((10.0, 12.0));
        assert_eq!(mark_time(MarkerHit::Save, running, &saved, &found, 5.0), Some(10.0));
        assert_eq!(mark_time(MarkerHit::Recall(3), running, &saved, &found, 5.0), Some(40.0));
        // A slot number that is not on the strip has nothing to answer.
        assert_eq!(mark_time(MarkerHit::Recall(9), running, &saved, &found, 5.0), None);
        assert_eq!(mark_time(MarkerHit::Found(0), running, &saved, &found, 5.0), Some(60.0));
        assert_eq!(mark_time(MarkerHit::Cue, running, &saved, &found, 5.0), Some(5.0));
        // The record's own shape is structure, not a mark someone placed.
        assert_eq!(mark_time(MarkerHit::Shape(1), running, &saved, &found, 5.0), None);
    }

    #[test]
    fn a_seek_release_only_aborts_off_the_strips_own_band() {
        // Squarely inside the band: commits.
        assert!(!seek_release_aborted(50.0, 40.0, 20.0));
        // Just past the top and bottom edges, still within the margin:
        // a hand lifting slightly off the strip is still aiming at it.
        assert!(!seek_release_aborted(40.0 - SEEK_ABORT_PX + 1.0, 40.0, 20.0));
        assert!(!seek_release_aborted(60.0 + SEEK_ABORT_PX - 1.0, 40.0, 20.0));
        // Well past either edge: the hand let go of the idea, not just the pixel.
        assert!(seek_release_aborted(40.0 - SEEK_ABORT_PX - 1.0, 40.0, 20.0));
        assert!(seek_release_aborted(60.0 + SEEK_ABORT_PX + 1.0, 40.0, 20.0));
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

    /// The reason the zoom is per lane: two decks matched by ear must
    /// match on screen too, or the picture contradicts the room.
    #[test]
    fn two_synced_decks_draw_their_beats_the_same_width() {
        // A at 120 played straight; B at 100 played 1.2x, which is 120 to
        // the ear. B's beat spans 1.2x more SOURCE columns, so only a zoom
        // scaled by the rate puts them on the same pixels.
        let a = lane(120.0, 0.0);
        let b = WaveLane { grid: Some(grid(100.0, 0.0, 0)), rate: 1.2, ..lane(100.0, 0.0) };
        let shared = 4.0f32;
        let width = |lane: &WaveLane| {
            lane.grid_columns().unwrap().0 / WaveLane::lane_zoom(shared, lane.rate) as f64
        };
        // A ten-thousandth of a pixel: the zoom reaches the shader as an
        // f32, so exact equality is not a claim the type can keep, and
        // anything this small is well under a screen pixel.
        assert!(
            (width(&a) - width(&b)).abs() < 1e-4,
            "a beat is {} px on A and {} px on B",
            width(&a),
            width(&b)
        );
        // And the same rule makes them scroll together: pixels per second
        // of AUDIBLE time is the same on both lanes.
        let scroll = |lane: &WaveLane| {
            ZOOM_COLS_PER_SEC * lane.rate / WaveLane::lane_zoom(shared, lane.rate) as f64
        };
        assert!((scroll(&a) - scroll(&b)).abs() < 1e-4, "{} vs {}", scroll(&a), scroll(&b));
        // And the control: on ONE shared zoom they do not match at all, so
        // the agreement above belongs to the scaling and not to the fixture.
        let unscaled = |lane: &WaveLane| lane.grid_columns().unwrap().0 / shared as f64;
        assert!(
            (unscaled(&a) - unscaled(&b)).abs() > 1.0,
            "shared zoom would draw {} px against {} px",
            unscaled(&a),
            unscaled(&b)
        );
    }

    /// A mark is drawn where the record says it is, in the lane's own
    /// scrolling space rather than the strip's whole-track one.
    #[test]
    fn a_mark_lands_where_the_record_says_it_is() {
        let centre = 12.0 * ZOOM_COLS_PER_SEC;
        let middle = 500.0;
        let cols = 4.0;
        // The second the head is on sits under the middle of the lane.
        assert!((WaveLane::mark_x(12.0, centre, cols, middle) - middle).abs() < 1e-9);
        // One second later is one second's worth of columns to the right,
        // at this lane's own zoom...
        let ahead = WaveLane::mark_x(13.0, centre, cols, middle);
        assert!((ahead - middle - ZOOM_COLS_PER_SEC / cols).abs() < 1e-9, "{ahead}");
        // ...and one second earlier is the same distance the other way.
        let behind = WaveLane::mark_x(11.0, centre, cols, middle);
        assert!((middle - behind - ZOOM_COLS_PER_SEC / cols).abs() < 1e-9, "{behind}");
        // A lane showing more seconds in the same pixels draws the same
        // mark nearer the middle, which is what keeps a mark agreeing
        // with the waveform under it.
        let wider = WaveLane::mark_x(13.0, centre, cols * 2.0, middle);
        assert!(wider < ahead && wider > middle, "{wider} against {ahead}");
    }

    /// The default has to be the exact value that used to be hardcoded
    /// (`rect.size.x * 0.5`) -- this is a config knob added to existing,
    /// working behaviour, not a change to it, and every session before
    /// this one gets the identical picture it always had.
    #[test]
    fn the_head_fraction_default_is_dead_centre() {
        assert_eq!(HEAD_FRACTION_DEFAULT, 0.5);
        assert!(HEAD_FRACTION_MIN < HEAD_FRACTION_DEFAULT);
        assert!(HEAD_FRACTION_DEFAULT < HEAD_FRACTION_MAX);
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
    fn a_playhead_inside_a_loop_wraps_instead_of_walking_out_of_it() {
        let mut lane = lane(120.0, 10.0);
        lane.stamp = 100.0;
        lane.playing = true;
        // A SUB-BEAT loop, which is where this bites: the prediction only
        // ever runs half a second ahead, so a loop longer than that could
        // never have walked out of itself in the first place.
        lane.loop_span = Some((10.0, 10.3));
        // Inside the lap, nothing to do.
        assert!((lane.position_at(100.1) - 10.1).abs() < 1e-9);
        // Past the end: back round, not out the far side. Two-thirds of a
        // second of prediction is one whole lap and change.
        assert!((lane.position_at(100.5) - 10.2).abs() < 1e-9);
        // However stale the stamp, the drawn head stays in the span.
        let far = lane.position_at(1_000.0);
        assert!((10.0..10.3).contains(&far), "{far} is inside the loop");

        // A head that has NOT reached the loop yet is left alone: it is on
        // its way in and the loop has not captured it.
        lane.position_secs = 9.5;
        assert!((lane.position_at(100.25) - 9.75).abs() < 1e-9);

        // A degenerate span is not a division: it simply does not fold.
        lane.position_secs = 10.0;
        lane.loop_span = Some((10.0, 10.0));
        assert!((lane.position_at(100.4) - 10.4).abs() < 1e-9);
    }

    #[test]
    fn a_lane_warns_over_the_last_half_minute_and_never_blinks_fully_out() {
        let mut lane = lane(120.0, 0.0);
        lane.stamp = 100.0;
        lane.playing = true;
        lane.duration_secs = 300.0;

        // Nothing to say in the middle of a record.
        lane.position_secs = 100.0;
        assert_eq!(lane.warn_at(100.0, WARN_SECS_DEFAULT), 0.0);
        // Nor with no length known, nor stopped.
        lane.position_secs = 290.0;
        lane.duration_secs = 0.0;
        assert_eq!(lane.warn_at(100.0, WARN_SECS_DEFAULT), 0.0);
        lane.duration_secs = 300.0;
        lane.playing = false;
        assert_eq!(lane.warn_at(100.0, WARN_SECS_DEFAULT), 0.0);
        lane.playing = true;

        // Inside the window it shows, and it shows harder as the end
        // comes -- sampled at the same point of the pulse both times, or
        // the pulse rather than the ramp would be under test.
        let far = lane.warn_at(100.0, WARN_SECS_DEFAULT);
        lane.position_secs = 299.0;
        let near = lane.warn_at(100.0, WARN_SECS_DEFAULT);
        assert!(far > 0.0, "twenty seconds out is already warning");
        assert!(near > far, "{near} at one second out beats {far} at ten");

        // Armed, it pulses -- but never all the way to nothing, or it
        // would be invisible half of every second.
        let over_a_second: Vec<f32> =
            (0..10).map(|i| lane.warn_at(100.0 + i as f64 * 0.1, WARN_SECS_DEFAULT)).collect();
        let low = over_a_second.iter().cloned().fold(f32::MAX, f32::min);
        let high = over_a_second.iter().cloned().fold(0.0f32, f32::max);
        assert!(low > 0.0, "never fully out: {low}");
        assert!(high > low * 1.5, "and visibly moving: {low} to {high}");

        // Past the end there is nothing left to warn about.
        lane.position_secs = 301.0;
        assert_eq!(lane.warn_at(100.0, WARN_SECS_DEFAULT), 0.0);
    }

    /// `warn_secs` of 0.0 is the operator's off switch, not a special
    /// case in `warn_at` -- the ramp's own window (`0.0..0.0`) is empty
    /// and never contains a real "seconds left", at any point in the
    /// record, playing or not.
    #[test]
    fn a_warn_window_of_zero_never_warns() {
        let mut lane = lane(120.0, 0.0);
        lane.stamp = 100.0;
        lane.playing = true;
        lane.duration_secs = 300.0;
        lane.position_secs = 299.999;
        assert_eq!(lane.warn_at(100.0, 0.0), 0.0);

        // A smaller, non-zero window still works, just over its own
        // shorter span -- the value is a real threshold, not a toggle
        // that only understands its default.
        assert_eq!(lane.warn_at(100.0, 0.0005), 0.0, "outside a half-second window");
        lane.position_secs = 299.9999;
        assert!(lane.warn_at(100.0, 0.0005) > 0.0, "inside it");
    }

    #[test]
    fn the_next_mark_is_the_nearest_one_still_ahead() {
        let mut lane = lane(120.0, 10.0);
        lane.cue_secs = 5.0; // behind the playhead: not a candidate
        lane.saved_slots = vec![(1, 40.0, 44.0, 0), (2, 15.0, 16.0, 0)];
        lane.found_loops = vec![(12.0, 12.5)];
        // Nearest of the three ahead of 10.0 is the found loop at 12.0,
        // not the saved slot that is merely first in the list.
        assert_eq!(lane.next_mark_secs(10.0), Some(12.0));
        // Once the playhead passes it, the next saved slot takes over.
        assert_eq!(lane.next_mark_secs(13.0), Some(15.0));
        // Past everything, there is nothing left to point at.
        assert_eq!(lane.next_mark_secs(41.0), None);
    }

    #[test]
    fn the_next_mark_label_counts_beats_with_a_grid_and_seconds_without() {
        let mut with_grid = lane(120.0, 10.0); // 0.5s a beat
        with_grid.saved_slots = vec![(1, 12.0, 13.0, 0)];
        // Two seconds at 120bpm is exactly four beats.
        assert_eq!(with_grid.next_mark_label(10.0), Some("4 beats".to_string()));

        let mut no_grid = with_grid.clone();
        no_grid.grid = None;
        assert_eq!(no_grid.next_mark_label(10.0), Some(crate::clock::countdown(2.0)));

        // Nothing ahead: nothing to say next to the playhead at all.
        let mut nothing_ahead = with_grid.clone();
        nothing_ahead.saved_slots.clear();
        assert_eq!(nothing_ahead.next_mark_label(10.0), None);
    }

    #[test]
    fn an_empty_lane_has_nothing_to_rule_or_scroll() {
        let lane = WaveLane::default();
        assert_eq!(lane.cols, 0);
        assert!(lane.grid_columns().is_none());
        assert!((lane.head_column_at(123.0)).abs() < 1e-9);
    }

    #[test]
    fn band_gain_reads_in_decibels_and_a_kill_says_so() {
        assert_eq!(format_band_gain(1.0), "0.0 dB");
        assert_eq!(format_band_gain(2.0), "+6.0 dB");
        assert_eq!(format_band_gain(0.5), "-6.0 dB");
        assert_eq!(format_band_gain(0.0), "KILL");
        assert_eq!(format_band_gain(5e-5), "KILL");
        assert_eq!(format_band_gain(0.999), "0.0 dB", "no signed zero");
        assert_eq!(format_band_gain(0.25), "-12.0 dB");
    }

    #[test]
    fn the_sweep_reads_off_or_its_corner() {
        assert_eq!(format_filter(0.5), "OFF");
        assert_eq!(format_filter(0.51), "OFF");
        assert_eq!(format_filter(0.0), "LP 40");
        assert_eq!(format_filter(1.0), "HP 9.0k");
        let (_, hz) = crate::music_dsp::filter_corner_hz(0.25).unwrap();
        assert!(hz < 1000.0, "{hz}");
        assert_eq!(format_filter(0.25), format!("LP {}", hz.round() as i64));
        let (_, hz) = crate::music_dsp::filter_corner_hz(0.4).unwrap();
        assert!(hz > 1000.0, "{hz}");
        assert_eq!(format_filter(0.4), format!("LP {:.1}k", hz / 1000.0));
        assert!(format_filter(0.9).starts_with("HP "));
        for i in 0..=100 {
            assert!(format_filter(i as f32 / 100.0).len() <= 8, "fits the legend");
        }
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

    fn luma(c: [f32; 3]) -> f32 {
        c[0] * 0.299 + c[1] * 0.587 + c[2] * 0.114
    }

    fn saturation(c: [f32; 3]) -> f32 {
        let top = c[0].max(c[1]).max(c[2]);
        let low = c[0].min(c[1]).min(c[2]);
        if top <= 1e-6 { 0.0 } else { (top - low) / top }
    }

    /// A build-up can turn every few seconds. Across a whole record that is
    /// a hatch that hides the drop it is marking rather than showing it.
    #[test]
    fn turns_too_close_together_are_drawn_as_one() {
        // The rule the strip draws by, in the same order.
        let thin = |xs: &[f64]| {
            let mut out = Vec::new();
            let mut last = f64::NEG_INFINITY;
            for x in xs.iter().copied() {
                if x - last >= CHANGE_MIN_PX {
                    out.push(x);
                    last = x;
                }
            }
            out
        };
        // A build-up: eight turns inside twelve pixels.
        let fence: Vec<f64> = (0..8).map(|i| 100.0 + i as f64 * 1.5).collect();
        let drawn = thin(&fence);
        assert!(drawn.len() <= 3, "a picket fence survived: {drawn:?}");
        assert_eq!(drawn[0], 100.0, "and the first turn is always kept");
        // An ordinary arrangement is untouched: nothing is thinned away
        // that the operator could have seen.
        let phrases: Vec<f64> = (0..12).map(|i| i as f64 * 21.0).collect();
        assert_eq!(thin(&phrases), phrases, "a real arrangement must survive whole");
    }

    /// The stub has to clear both chip rows even while one is being read:
    /// a chip grows under the pointer.
    #[test]
    fn a_turn_stays_clear_of_both_chip_rows() {
        // The grown chip height, from the strip's own chip_bottom.
        let grown_chip = 17.0;
        assert!(
            CHANGE_CLEAR >= grown_chip,
            "a turn would run under a chip being hovered: {CHANGE_CLEAR} vs {grown_chip}"
        );
        // And on a strip too short to hold the band, the stub still has a
        // height rather than a negative one.
        for height in [2.0f64, 8.0, 24.0, 40.0, 120.0] {
            let band = (height - CHANGE_CLEAR * 2.0).max(2.0);
            assert!(band >= 2.0, "height {height} gave a band of {band}");
        }
    }

    /// When the analysis cannot find a body it still hands back numbers, and
    /// on a long record that guess looks exactly like a real intro.
    #[test]
    fn a_guessed_shape_never_reaches_the_mixing_lane() {
        let measured = crate::track_shape::TrackShape {
            intro_start_secs: 0.0,
            intro_end_secs: 30.0,
            outro_start_secs: 240.0,
            outro_end_secs: 300.0,
            detected: true,
        };
        let guessed = crate::track_shape::TrackShape { detected: false, ..measured };
        // The lane takes the pair only from a shape that was really found.
        let body = |shape: crate::track_shape::TrackShape| {
            Some(shape)
                .filter(|s| s.detected)
                .map(|s| (s.intro_end_secs, s.outro_start_secs))
        };
        assert_eq!(body(measured), Some((30.0, 240.0)));
        assert_eq!(body(guessed), None, "a guess must not draw as a measurement");
    }

    /// Brightness in this lane already says what has been played and which
    /// deck is active. A colouring that moved it would be a third meaning on
    /// a channel that has two.
    #[test]
    fn colouring_a_column_never_changes_how_bright_it_is() {
        let grey_y = luma(WAVE_GREY);
        for tile in [
            [255, 83, 60, 255],
            [94, 255, 255, 234],
            [227, 198, 168, 244],
            [150, 150, 150, 200],
            [255, 40, 20, 220],
            [30, 60, 255, 180],
            [40, 255, 40, 200],
            [1, 0, 0, 12],
        ] {
            let got = luma(band_tint(tile));
            assert!(
                (got - grey_y).abs() < 1e-4,
                "{tile:?} drew at brightness {got}, the grey is {grey_y}"
            );
        }
    }

    /// The one outcome that would make this colouring a net loss: an
    /// operator believing a record is separated when it is not.
    #[test]
    fn a_band_colour_can_never_be_as_strong_as_a_stem_colour() {
        // The least colourful of the four, which is the one to clear.
        let weakest_stem = STEM_COLORS
            .iter()
            .map(|c| saturation([c[0], c[1], c[2]]))
            .fold(f32::INFINITY, f32::min);
        assert!(weakest_stem > 0.6, "the stem palette moved: {weakest_stem}");
        // Every corner and edge of the band cube, not a sample of it: the
        // cap has to hold for anything the analysis can produce.
        let mut worst = 0.0f32;
        for low in (0..=255).step_by(15) {
            for mid in (0..=255).step_by(15) {
                for high in (0..=255).step_by(15) {
                    let s = saturation(band_tint([low, mid, high, 255]));
                    worst = worst.max(s);
                }
            }
        }
        assert!(
            worst < weakest_stem - 0.2,
            "a band colour reached {worst} against the palest stem at {weakest_stem}"
        );
    }

    /// And it has to be worth drawing: a cap that made everything grey would
    /// pass the test above and deliver nothing.
    #[test]
    fn a_column_that_leans_on_one_band_is_visibly_coloured() {
        let flat = saturation(WAVE_GREY);
        for (name, tile) in [
            ("bass", [255, 40, 20, 220]),
            ("air", [30, 60, 255, 180]),
            ("mid", [40, 255, 40, 200]),
        ] {
            let s = saturation(band_tint(tile));
            assert!(
                s > flat * 1.5,
                "a {name}-heavy column drew at {s}, barely past the plain grey {flat}"
            );
        }
        // Three bands level is a column with nothing to say, and it says so.
        let level = saturation(band_tint([150, 150, 150, 200]));
        assert!(level < 0.02, "a balanced column should be neutral, drew {level}");
        // And the hues really are different readings, not one tint.
        let bass = band_tint([255, 40, 20, 220]);
        let air = band_tint([30, 60, 255, 180]);
        assert!(bass[0] > bass[2] + 0.15, "bass must read warm: {bass:?}");
        assert!(air[2] > air[0] + 0.15, "air must read cool: {air:?}");
    }

    /// Nothing about a colouring may change the shape of the wave.
    #[test]
    fn keeping_a_columns_bands_together_leaves_every_height_alone() {
        let columns: Vec<[u8; 4]> = (0..1000)
            .map(|i| {
                let n = i as u8;
                [n.wrapping_mul(7), n.wrapping_mul(13), n.wrapping_mul(29), n.wrapping_mul(3)]
            })
            .collect();
        let mut per_channel = vec![columns.clone()];
        let mut louder = vec![columns.clone()];
        for _ in 0..8 {
            per_channel.push(reduce_once(per_channel.last().unwrap(), Reduce::PerChannel));
            louder.push(reduce_once(louder.last().unwrap(), Reduce::BandsOfTheLouder));
        }
        for (level, (a, b)) in per_channel.iter().zip(louder.iter()).enumerate() {
            let heights_a: Vec<u8> = a.iter().map(|c| c[3]).collect();
            let heights_b: Vec<u8> = b.iter().map(|c| c[3]).collect();
            assert_eq!(heights_a, heights_b, "level {level} changed a height");
        }
        // And it does what it is for: the bands of a reduced column are a
        // real column's bands, not three maxima from three different moments.
        let pair = [[255u8, 0, 0, 10], [0, 0, 255, 200]];
        let one = reduce_once(&pair, Reduce::BandsOfTheLouder);
        assert_eq!(one[0], [0, 0, 255, 200], "the louder column's own bands");
        let both = reduce_once(&pair, Reduce::PerChannel);
        assert_eq!(both[0], [255, 0, 255, 200], "which the old rule never was");
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
