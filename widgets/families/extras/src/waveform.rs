//! Waveform — the peaks of a recording across a lane, and the regions and
//! marks somebody put on it.
//!
//! The host hands it three lists and two numbers: the peaks, the regions,
//! the marks, how long the recording is, and how much of that the peaks
//! cover. It decodes nothing, analyses nothing, and has no opinion about
//! where the peaks came from — a file, a stream, a synthesiser, a made-up
//! series on a catalogue page. What it knows how to do is draw a lot of
//! numbers across a lane cheaply, put the other three kinds of thing on it
//! in rows that cannot fight, and tell the host what a finger did.
//!
//! # The lane has two rows
//!
//! The top `marker_strip` points belong to the marks: their chips and their
//! names, and nothing else is drawn or grabbed there. Everything below it is
//! the body: the peaks, the region washes and their brackets, and the region
//! names along the bottom. The playhead crosses both. All three shaders and
//! the hit test take that split from one expression — `clamp(marker_strip,
//! 0, height)` — so none of them can disagree about it at any value, not
//! even a band taller than the lane.
//!
//! That split is the whole reason the gestures are unambiguous. A press in
//! the strip is a mark or it is nothing; a press in the body is a region
//! edge or it is a seek. No press has to be guessed at, and a finger that
//! slips off a chip does not move the playhead as a parting gift. Set
//! `marker_strip` to 0 and there are no marks at all — not drawn, not
//! hit-tested — because a mark with nowhere to be grabbed is a mark with
//! nowhere to be.
//!
//! # Where the peaks sit on the axis
//!
//! The peaks are the fixed picture. They are spread across the first
//! `peaks_span` of the axis — the whole of it by default — and the regions,
//! the marks and the playhead slide against them as `duration` changes.
//!
//! So the invariant is: **the peak array covers exactly `0..peaks_span`, and
//! `peaks_span` of 0 means the whole `duration`.** A host that hands over
//! peaks for the first thirty seconds of a five-minute file, and leaves
//! `peaks_span` alone, will get thirty seconds of audio stretched across
//! five minutes of lane with every mark against the wrong part of it and no
//! error anywhere. Set `peaks_span` to 30 and the same array draws across
//! the first tenth of the lane, which is where it belongs, and the rest of
//! the lane stays empty until the peaks for it arrive.
//!
//! # The gestures
//!
//! * **The body**: moves the playhead, live, and reports where it settled.
//! * **A region's edge**, within `edge_grab` points: resizes that end. The
//!   end being dragged stops `min_span` short of the other rather than
//!   shoving it along. That rule and `min_span` are the range slider's,
//!   settled there and copied here; the arithmetic is a second, private
//!   `Lane`, because that widget's `Range` is private to its own module.
//! * **A mark's chip**, within `marker_grab` points: moves it. Let go within
//!   [`PICK_SLOP`] points of where it was pressed and it is a pick, not a
//!   move — a mark you meant to click should not end up a frame to the left
//!   of where it was.
//!
//! The arrow keys move the playhead and nothing else; Home and End send it
//! to the two ends. The keyboard has one selection and this lane has three
//! kinds of thing on it, so giving the keyboard the regions and the marks
//! would need a selection model, and a selection model is a bigger widget
//! than this one. The wheel is left alone, so a page with a waveform on it
//! still scrolls when the pointer is over the waveform.
//!
//! A lane holding key focus draws a RING inside its own edge, and not a
//! tint of its ground: `color_inset_hover`, `color_inset_focus` and
//! `color_inset_drag` are aliases of `color_inset` in both shipped desktop
//! themes, so a ground tint would leave a focused lane pixel-identical to
//! an unfocused one — on a tab stop whose arrow keys edit a value. Hover,
//! focus and drag go on the PEAKS instead, through the `color_val` ladder,
//! which does step per state in all three themes.
//!
//! # What it costs
//!
//! One quad for the lane, one per region, one per mark, one for the
//! playhead, and one text draw per name that fits — so a frame costs what
//! the regions and marks cost and nothing at all for the peaks. The peaks
//! are paid for once per change of the peaks, of `duration` or `peaks_span`,
//! or of the lane's width: they are reduced to one texel per device pixel of
//! covered lane and uploaded as a single row of texture, and at no other
//! time. See [`MAX_COLUMNS`] for the one hard number.
use crate::{
    // Animate is set_disabled's; the other four are what the Animator derive
    // and animator_handle_event need. The same set as range_slider.rs:30.
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    // The intent palette a region or a mark is coloured through, and the
    // one-line text measurer the skip-not-clip rule needs.
    badge::{BadgeIntent, BadgePalette},
    family_api::measure,
    // The escape hatch's parser. There is no second hex parser here.
    color::parse_hex_color,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    // What `script_call` returns. range_slider.rs:36 imports it for the same
    // reason, and uses it at :605.
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.WaveformBase = #(Waveform::register_widget(vm))

    // The four draw types register bare, with nothing on them but the quad
    // they inherit. Everything a caller might want to reach is on the widget
    // preset below, so `draw_lane +: {...}` on an instance has something to
    // merge into; a type default carrying the whole shader would leave a
    // caller with nothing to override.
    set_type_default() do #(DrawWaveformLane::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawWaveformRegion::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawWaveformMarker::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    set_type_default() do #(DrawWaveformHead::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** The peaks of a recording across a lane, with the regions and marks on it. */
    mod.widgets.Waveform = set_type_default() do mod.widgets.WaveformBase{
        width: Fill
        // A fixed height and never a Fit one: a lane has no content to size
        // itself to, so `height: Fit` resolves to nothing and the lane is
        // laid out with no body and never painted. A row that sizes to fit
        // is fine — it sizes to this number.
        height: 96.
        margin: theme.mspace_v_1

        /** how far the lane's axis runs, in whatever the host counts in */
        duration: 1.0
        /** how much of the axis the peaks cover; 0 is all of it */
        peaks_span: 0.0
        /** where the playhead is, in the same unit */
        playhead: 0.0
        /** draw the playhead, and shade what is behind it as played */
        show_playhead: true
        /** draw region and mark names */
        show_names: true
        /** regions, one string each: "start | end | name | intent-or-#rrggbb" */
        regions: []
        /** marks, one string each: "at | name | intent-or-#rrggbb" */
        markers: []
        /** the least a region may be squeezed to; 0 lets its ends meet */
        min_span: 0.0
        /** drag quantization in the axis unit; 0 is continuous */
        step: 0.0
        /** the band at the top the marks own, in points; 0 removes the marks 0..48 step 1 */
        marker_strip: 16.
        /** how close a press must come to a region edge, in points 2..24 step 1 */
        edge_grab: 6.
        /** how close a press must come to a mark chip, in points 2..24 step 1 */
        marker_grab: 7.
        /** a mark chip's width in points 4..24 step 1 */
        chip_width: 9.
        /** the room a name leaves the thing it names, in points 0..24 step 1 */
        name_inset: 4.

        /** The colour of every intent, inherited from the shared palette so
         * a region and a badge that mean the same thing look the same. */
        palette: mod.widgets.BadgePalette{}

        /** The ground and the peaks: one quad, one texture fetch a pixel. */
        draw_lane +: {
            // The reduced picture: one row, one texel a device pixel of the
            // lane the peaks cover. Bound by the widget with
            // draw_vars.set_texture(0, ..) immediately before the draw; the
            // handle lives on the widget, never on this struct.
            wave_tex: texture_2d(float)

            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            // Plain literals, not instance() or uniform(): each one has a
            // field on the draw struct behind it, and instance() in the DSL
            // never binds to a Rust field. The four Rust writes every draw —
            // cols, covered, head, head_on — and strip_px are fields too,
            // and are left at their Rust defaults here because nothing a
            // caller writes would survive the next draw.
            /** how much of the half-body a full-scale peak fills 0.2..1 step 0.02 */
            envelope: 0.88
            /** how far a played column fades toward the ground 0..1 step 0.05 */
            played_fade: 0.55
            /** corner rounding in points 0..16 step 0.5 */
            border_radius: 3.

            // Uniforms, so they cost no instance slot: nothing here varies
            // between the draws of one lane.
            /** how far a disabled lane's peaks fade toward the quiet ink 0..1 step 0.05 */
            disabled_fade: uniform(0.7)
            /** the keyboard ring's thickness, in points 0..6 step 0.5 */
            ring_size: uniform(theme.size_focus_ring)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_drag: uniform(theme.color_inset_drag)
            color_disabled: uniform(theme.color_inset_disabled)

            /** the peaks */
            color_wave: uniform(theme.color_val)
            color_wave_hover: uniform(theme.color_val_hover)
            color_wave_focus: uniform(theme.color_val_focus)
            color_wave_drag: uniform(theme.color_val_drag)
            /** the opaque ink a played or a disabled column fades toward */
            color_wave_quiet: uniform(theme.color_bg_app)
            /** the line the peaks are measured from */
            color_centre: uniform(theme.color_outline_variant)
            /** the keyboard ring, and what it is when nothing has focus */
            ring_color: uniform(theme.color_primary)
            ring_color_off: uniform(theme.color_u_hidden)

            pixel: fn() {
                let py = self.pos.y * self.rect_size.y

                // The library's settled state chain — slider.rs:204-208 and
                // range_slider.rs:137-140 — with drag nested inside hover, so
                // a drag at half a hover cannot throw the focus tint away.
                // The four inset tokens it reads are aliases of ONE colour in
                // both desktop themes (theme_desktop_dark.rs:299-304,
                // theme_desktop_light.rs:302-307) and differ by 2/255 of
                // alpha in the skeleton, so the ground does not in fact move
                // on hover, focus or drag today. What moves is the peaks
                // below, whose ladder does step; the chain stays here so a
                // theme that gives the inset states their own values gets
                // them without a code change.
                let ground = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                    .mix(self.color_disabled, self.disabled)

                // Where the marks' band ends and the body starts. The region
                // shader, the marker shader and the hit test all derive it
                // from this same expression, so a band of any height — zero,
                // or taller than the lane — means one thing to all four.
                let body_top = clamp(self.strip_px, 0.0, self.rect_size.y)
                let body_h = max(self.rect_size.y - body_top, 2.0)
                // +1 at the top of the body and -1 at the bottom, so a
                // column is drawn about the level it was measured from.
                let v = 1.0 - (py - body_top) / body_h * 2.0
                let feather = 2.0 / body_h
                let in_body = step(body_top, py)

                // The line the peaks are measured from, drawn whether or not
                // there are any and right across the lane whether or not the
                // peaks reach the end of it. A lane that draws nothing at
                // all reads as one that failed to lay out, and the one thing
                // this must not do is look broken while it waits for a file.
                let rule = (1.0 - smoothstep(0.0, feather * 1.2, abs(v))) * in_body
                let base = ground.mix(self.color_centre, rule * 0.8)

                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // sdf.box's effective radius is twice what it is handed, so
                // the number on the property means points.
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.border_radius * 0.5)

                // The peaks carry the states the ground cannot: `color_val`
                // and its hover, focus and drag rungs are three different
                // colours in all three themes (dark 375-378, light 378-381,
                // skeleton 329-332), so these are the mixes a pointer and the
                // keyboard actually show.
                let ink = self.color_wave
                    .mix(self.color_wave_focus, self.focus)
                    .mix(self.color_wave_hover.mix(self.color_wave_drag, self.drag), self.hover)

                // Behind the playhead the recording has been played, and a
                // disabled lane is not to be operated: both read quieter.
                // Quieter means NEARER WHAT THE LANE IS STANDING ON, and that
                // has to be an opaque colour. The disabled value token is
                // alpha 0 in both desktop themes and would punch the peaks
                // out as holes; the lane's own ground is not a colour either
                // but 5-to-15% black over the app, so fading toward it takes
                // the peaks' alpha down with it and makes the same hole more
                // slowly. `theme.color_bg_app` is opaque in all three themes
                // and is the ground the lane's own wash is laid over, so a
                // peak mixed toward it approaches invisible from whichever
                // side the theme is on — downward in the dark one, upward in
                // the light one — and never through transparency.
                let played = step(self.pos.x, self.head) * self.head_on
                let quiet = clamp(played * self.played_fade + self.disabled * self.disabled_fade, 0.0, 1.0)
                let peak = ink.mix(self.color_wave_quiet, quiet)

                // Where this pixel falls inside the stretch of lane the
                // peaks cover. Past it the host has not sent peaks for this
                // part of the recording, and the honest picture of that is
                // empty lane rather than stretched audio. Written as a
                // guarded block and not an early return so the ring at the
                // bottom is drawn from one place.
                let u = self.pos.x / max(self.covered, 0.0001)
                let mut cover = 0.0
                if self.cols >= 1.0 && u <= 1.0 {
                    // One texel a pixel by construction, so the column is
                    // picked and never filtered: a filtered fetch would blend
                    // the high byte of one column with the low byte of the
                    // next, which is not a wrong value but a meaningless one.
                    let col = clamp(floor(u * self.cols), 0.0, self.cols - 1.0)
                    let t = self.wave_tex.sample_nearest(vec2((col + 0.5) / self.cols, 0.5))
                    // Sixteen bits a value, two channels each: the high of
                    // the column in alpha and red, the low in green and blue.
                    // That is the layout voice_wave packs and reads back
                    // against this same texture format. Mirrored in Rust by
                    // `unpack_column`; keep the two in step.
                    let hi_u = t.w + t.x / 256.0
                    let lo_u = t.y + t.z / 256.0
                    let top = clamp(hi_u * 2.0 - 1.0, -1.0, 1.0) * self.envelope
                    let bot = clamp(lo_u * 2.0 - 1.0, -1.0, 1.0) * self.envelope

                    // Every column gets at least half a pixel either side of
                    // ITS OWN midpoint. Silence in a recording is still
                    // recording, and a column that draws as nothing reads as
                    // a hole in the file — but widening toward the centre
                    // line would drag a signal that sits off centre back onto
                    // it, and showing that is the whole reason the low and
                    // the high are kept apart.
                    let mid = (top + bot) * 0.5
                    let half = max((top - bot) * 0.5, feather * 0.5)
                    let hi = mid + half
                    let lo = mid - half

                    cover = (1.0 - smoothstep(hi - feather, hi + feather, v))
                        * smoothstep(lo - feather, lo + feather, v)
                        * in_body
                }

                sdf.fill(base.mix(peak, cover))

                // The keyboard ring, and not a tint of the ground: this is a
                // nav stop whose arrows edit a value, and the ground's own
                // focus token is an alias of its base in both desktop themes,
                // so a lane holding key focus would otherwise be pixel-exact
                // with one that does not. The form is svg_select.rs:122-155's
                // and rating.rs:119-136's — `color_primary` over
                // `color_u_hidden`, gated so a disabled lane never rings.
                // The box is inset half the ring and the stroke is given half
                // its width, because `sdf.stroke` paints that far EITHER side
                // of the shape: the ring is then `ring_size` points thick and
                // sits inside the lane rather than over its neighbour.
                sdf.box(
                    self.ring_size * 0.5,
                    self.ring_size * 0.5,
                    max(self.rect_size.x - self.ring_size, 1.0),
                    max(self.rect_size.y - self.ring_size, 1.0),
                    self.border_radius * 0.5
                )
                sdf.stroke(
                    self.ring_color_off.mix(self.ring_color, self.focus * (1.0 - self.disabled)),
                    self.ring_size * 0.5
                )
                return sdf.result
            }
        }

        /** A region: a wash the peaks stay readable through, and a bracket. */
        draw_region +: {
            // `color`, `hot_start`, `hot_end` and `strip_px` are written by
            // Rust once per region and this quad is drawn many times a
            // frame, so all four must be per-instance. Everything below them
            // is the same for every region in a frame, but stays a plain
            // literal for the same reason the ones above do: each has a
            // field on the draw struct behind it.
            color: theme.color_primary
            /** the pointer is on the opening edge 0..1 step 1 */
            hot_start: 0.0
            /** the pointer is on the closing edge 0..1 step 1 */
            hot_end: 0.0
            /** where the marks' band ends; nothing is painted above it — written per draw */
            strip_px: 16.
            /** how much colour the body of a region carries 0..0.6 step 0.02 */
            wash: 0.16
            /** how thick a region's edge is, in points 0.5..4 step 0.5 */
            edge_size: 1.5
            /** how far the caps run in from each edge, in points 0..32 step 1 */
            cap_length: 7.
            /** how much the ink lifts under the pointer 0..0.3 step 0.01 */
            hot_lift: 0.10

            pixel: fn() {
                let px = self.pos.x * self.rect_size.x
                let py = self.pos.y * self.rect_size.y

                // The band at the top belongs to the marks. This quad spans
                // the whole lane so all three shaders take the split from
                // one expression, and this is where the region honours it.
                let body_top = clamp(self.strip_px, 0.0, self.rect_size.y)
                let in_body = step(body_top, py)

                let from_in = px
                let from_out = self.rect_size.x - px
                let edge_in = 1.0 - smoothstep(self.edge_size - 0.5, self.edge_size + 0.5, from_in)
                let edge_out = 1.0 - smoothstep(self.edge_size - 0.5, self.edge_size + 0.5, from_out)

                // A bracket, not two rules. The short caps along the top and
                // bottom of the BODY tie the opening edge to the closing
                // one, so a region reads as one thing rather than as two
                // unrelated lines through the lane.
                let near_y = min(py - body_top, self.rect_size.y - py)
                let cap = (1.0 - smoothstep(self.edge_size - 0.5, self.edge_size + 0.5, near_y))
                    * step(min(from_in, from_out), self.cap_length)

                // Coverage is the wash or the bracket, whichever is more,
                // and it is multiplied by nothing: the edge is already
                // opaque, so a hot factor here would move the antialias ramp
                // half a pixel and change no colour at all.
                let lit = max(max(edge_in, edge_out), cap)
                let a = clamp(max(self.wash, lit), 0.0, 1.0) * self.color.a * in_body

                // The edge under the pointer says so before it is pressed,
                // by getting brighter. Both edges look alike and the middle
                // is not grabbable, so nothing else would say where to put
                // the finger; lifting the ink rather than the coverage is
                // how the range slider does the same job.
                let lift = self.hot_lift * max(self.hot_start * edge_in, self.hot_end * edge_out)
                let ink = self.color.rgb + vec3(lift, lift, lift)
                return vec4(ink * a, a)
            }
        }

        /** A mark: a chip in the strip, and a stem down the body under it. */
        draw_marker +: {
            color: theme.color_secondary
            /** the pointer is on this mark 0..1 step 1 */
            hot: 0.0
            /** the band the chip fills, in points — written per draw */
            strip_px: 16.
            /** how thick the stem under a chip is, in points 0..4 step 0.5 */
            stem_size: 1.
            /** how far down the body the stem reaches 0..1 step 0.05 */
            stem_reach: 1.
            /** how much the ink lifts under the pointer 0..0.3 step 0.01 */
            hot_lift: 0.10

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                // The same expression the lane and the region take their
                // body top from. No floor under it: the widget draws no mark
                // at all when the band is nothing, and any floor would put
                // the chip over the peaks by the difference for every band
                // shorter than it.
                let strip = clamp(self.strip_px, 0.0, self.rect_size.y)
                let lift = self.hot * self.hot_lift
                let ink = vec4(self.color.rgb + vec3(lift, lift, lift), self.color.a)

                // A flag with a point at the bottom, out of two shapes that
                // always paint: a rounded box, and a square standing on one
                // corner. NOT one tapered path — two mirrored identical SDF
                // paths in this repository paint only the second, so the
                // first fill of a pair is the one that disappears, and the
                // chip is both the mark's only presence and its only grab
                // target. A square on its corner is an axis-aligned box in a
                // frame rotated a quarter turn, which is how the delivery
                // tick in chat.rs is drawn for the same reason.
                let point = min(w * 0.5, strip * 0.34)
                let body_h = max(strip - point, 1.0)
                sdf.box(0.5, 0.0, w - 1.0, body_h, 1.0)
                let mid_x = w * 0.5
                let tip_y = body_h
                let side = point * 1.4142136
                sdf.rotate(0.7853982, mid_x, tip_y)
                sdf.rect(mid_x - side * 0.5, tip_y - side * 0.5, side, side)
                sdf.fill(ink)
                sdf.rotate(-0.7853982, mid_x, tip_y)

                // The chip says where the mark is to half a chip; the stem
                // says it to a point. A mark that cannot be lined up against
                // the peaks is a mark in roughly the wrong place.
                sdf.rect(
                    (w - self.stem_size) * 0.5,
                    strip,
                    self.stem_size,
                    max(self.rect_size.y - strip, 0.0) * self.stem_reach
                )
                sdf.fill(vec4(ink.rgb, ink.a * 0.55))
                return sdf.result
            }
        }

        /** The playhead, drawn over everything else. */
        draw_head +: {
            color: theme.color_on_surface
            /** the hard core's half-width, in points 0.5..4 step 0.25 */
            core: 1.
            /** how far the halo reaches, in points 0..24 step 0.5 */
            halo: 6.

            pixel: fn() {
                // A hard core with a soft halo either side: findable over a
                // loud passage inside a coloured region, without being a bar
                // painted across the picture. The widget makes this quad
                // max(2 * halo, 2) points wide, so `d` really does run out
                // to `halo` and both properties mean something.
                let d = abs((self.pos.x - 0.5) * self.rect_size.x)
                let hard = 1.0 - smoothstep(self.core - 0.5, self.core + 0.5, d)
                let soft = (1.0 - smoothstep(self.core, max(self.halo, self.core + 0.5), d)) * 0.28
                let a = clamp(hard + soft, 0.0, 1.0) * self.color.a
                return vec4(self.color.rgb * a, a)
            }
        }

        /** Region and mark names alike, in one colour and never the item's own. */
        draw_name +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_lane: {hover: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_lane: {hover: 1.0}}
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {draw_lane: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_lane: {focus: 1.0}}
                }
            }
            drag: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_lane: {drag: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_lane: {drag: 1.0}}
                }
            }
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_lane: {disabled: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_lane: {disabled: 1.0}}
                }
            }
        }
    }

    // A plain derivation, with NO second set_type_default(). One type
    // default is stored per Rust type and a second call replaces the first,
    // so declaring the cut-down transport preset that way would make it the
    // registered default for every Waveform anywhere. Every size variant in
    // this crate is written like this — ButtonFlatter, ButtonIcon,
    // AvatarSmall, Badge.
    //
    // `marker_strip` alone moves the band: the widget writes strip_px into
    // all three shaders every draw from that one number, so repeating it
    // under draw_region and draw_marker would be a value nothing reads.
    /** A short lane for a transport bar: chips, no names, a thin strip. */
    mod.widgets.WaveformStrip = mod.widgets.Waveform{
        height: 34.
        show_names: false
        marker_strip: 8.
        chip_width: 7.
        draw_lane +: {
            border_radius: 2.
            // Less height to work with, so the peaks use more of it.
            envelope: 0.94
        }
        draw_region +: {
            cap_length: 4.
        }
        draw_head +: {
            halo: 4.
        }
    }
}

/// How far a press may wander, in layout points, and still be a pick rather
/// than a move. The same three points `value_input.rs`'s `CLICK_SLOP` uses,
/// and for the same reason: a finger that meant to click travels a pixel or
/// two on the way up.
pub const PICK_SLOP: f64 = 3.0;

/// How tall a line's box is, as a multiple of the size of the text in it,
/// and where the ink starts inside it. Both are `timeline.rs:214-232`'s
/// numbers, because `draw_abs` takes the top of the LINE box and not of the
/// ink, and a name placed by the ink sits a third of a line too low.
const LINE_BOX: f64 = 1.45;
const INK_TOP: f64 = 0.30;

/// The `draw_abs` y that centres one line of text of `size` in a box
/// `height` tall whose top is at `top`.
fn text_top(top: f64, height: f64, size: f64) -> f64 {
    top + (height - size) * 0.5 - size * INK_TOP
}

// ---- the data -------------------------------------------------------------

/// A span of the recording with a name on it.
///
/// `start` and `end` are in whatever unit the lane's `duration` is counted
/// in — seconds for most callers, samples or bars for some. The widget never
/// converts and never formats.
///
/// No `Default`: `BadgeIntent` has none, and there is no honest neutral to
/// derive around it. `Toast` (toast.rs) and `TipRequest` (tip.rs) hold the
/// same enum the same way, and both name `BadgeIntent::Neutral` by hand.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveformRegion {
    pub start: f64,
    pub end: f64,
    /// What it is called. Empty draws no name, which is what a region that
    /// only marks a loop wants.
    pub name: String,
    /// What this region MEANS, in the same eight words every small mark in
    /// the library speaks. Resolved through the shared `BadgePalette`, so a
    /// region that means "error" is the same red as the badge that does, and
    /// stays right when the theme flips.
    pub intent: BadgeIntent,
    /// The escape hatch: a colour the host owns outright, used instead of
    /// the intent when it is set.
    ///
    /// For a host with a palette of its own that already carries meaning —
    /// a mixer whose channels are colour-coded, a score whose parts are.
    /// Anything else should use `intent`: a colour written here is a colour
    /// nobody re-checks against the light theme, and this widget argues
    /// elsewhere that the two ladders do not step the same way.
    pub color: Option<Vec4f>,
}

impl WaveformRegion {
    /// A region between two times. They are put in order here rather than
    /// checked: a span written backwards is the same span.
    pub fn new(start: f64, end: f64, name: &str) -> Self {
        Self {
            start: start.min(end),
            end: start.max(end),
            name: name.to_string(),
            intent: BadgeIntent::Neutral,
            color: None,
        }
    }

    pub fn with_intent(mut self, intent: BadgeIntent) -> Self {
        self.intent = intent;
        self
    }

    /// A colour of the host's own, instead of the intent's. See `color`.
    pub fn with_color(mut self, color: Vec4f) -> Self {
        self.color = Some(color);
        self
    }

    pub fn span(&self) -> f64 {
        self.end - self.start
    }
}

/// One instant with a name on it. No `Default`, for [`WaveformRegion`]'s
/// reason.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveformMarker {
    pub at: f64,
    pub name: String,
    pub intent: BadgeIntent,
    /// The escape hatch, as on [`WaveformRegion`].
    pub color: Option<Vec4f>,
}

impl WaveformMarker {
    pub fn new(at: f64, name: &str) -> Self {
        Self {
            at,
            name: name.to_string(),
            intent: BadgeIntent::Neutral,
            color: None,
        }
    }

    pub fn with_intent(mut self, intent: BadgeIntent) -> Self {
        self.intent = intent;
        self
    }

    pub fn with_color(mut self, color: Vec4f) -> Self {
        self.color = Some(color);
        self
    }
}

/// Read one line of markup as a region: `"start | end | name | tint"`.
///
/// The two numbers are plain, in the unit `duration` is counted in. This
/// widget does not read clock times, for the same reason `Timeline` does not
/// format them: "1:30" is a minute and a half to one caller and a bar and
/// three beats to another, and a widget that guessed would be wrong for half
/// of them. A field that is not a number reads as zero.
///
/// The tint field is one of the eight intent words — `neutral`, `primary`,
/// `secondary`, `tertiary`, `error`, `warning`, `success`, `info` — and that
/// is the form to use: it is the theme's, and it survives a theme change.
/// A `#rrggbb` or `#rrggbbaa` is also accepted, as the escape hatch for a
/// host that owns its palette; a word that is neither leaves the region
/// neutral rather than throwing the line away. The `#` is not optional
/// there: `ace`, `decade` and `beaded` are all runs of hex digits, and a lane
/// that read one of them as a colour would be answering a typo.
///
/// A colour in a line is written WITHOUT the `x` that a colour needs in
/// `script_mod!` source. That `x` is there to stop the Rust tokenizer
/// reading `1e2` as a float, and there is no tokenizer inside a string;
/// `#x4f9d69` here would parse as neither a word nor a colour.
///
/// A name may not contain `|`. The split takes the first three bars, so a
/// fourth and everything after it is read as part of the tint field.
pub fn parse_region(line: &str) -> WaveformRegion {
    // Read in the order the fields are written, one statement each. The
    // tint has to be read AFTER the name and not inside the struct literal
    // beside it: `read_tint` returns a pair, so it cannot sit in the
    // literal, and hoisting it above one silently makes it eat the name.
    let mut parts = line.splitn(4, '|');
    let start = read_number(parts.next());
    let end = read_number(parts.next());
    let name = parts.next().unwrap_or("").trim().to_string();
    let (intent, color) = read_tint(parts.next());
    WaveformRegion {
        start: start.min(end),
        end: start.max(end),
        name,
        intent,
        color,
    }
}

pub fn parse_regions(lines: &[String]) -> Vec<WaveformRegion> {
    lines.iter().map(|line| parse_region(line)).collect()
}

/// Read one line of markup as a mark: `"at | name | tint"`. The tint field
/// is the same one [`parse_region`] documents.
pub fn parse_marker(line: &str) -> WaveformMarker {
    let mut parts = line.splitn(3, '|');
    let at = read_number(parts.next());
    let name = parts.next().unwrap_or("").trim().to_string();
    let (intent, color) = read_tint(parts.next());
    WaveformMarker { at, name, intent, color }
}

pub fn parse_markers(lines: &[String]) -> Vec<WaveformMarker> {
    lines.iter().map(|line| parse_marker(line)).collect()
}

/// A field that is missing or is not a number counts as zero rather than
/// throwing the whole line away: a region with one end unwritten is at least
/// visible, and a region that vanished is a bug the caller cannot see.
fn read_number(field: Option<&str>) -> f64 {
    field.and_then(|t| t.trim().parse::<f64>().ok()).unwrap_or(0.0)
}

/// An intent word first, a `#rrggbb` second, neutral for anything else.
/// The word is tried first because it is the form callers should be
/// reaching for; the colour is the escape hatch.
///
/// The `#` is REQUIRED here even though `parse_hex_color` treats it as
/// optional (color.rs:139). Without that guard the fall-through accepts any
/// bare run of three, six or eight hex digits, so a tint field of `bad`,
/// `ace`, `decade` or `beaded` would quietly become a colour instead of
/// leaving the item neutral — and a word that is not an intent word is
/// almost always a typo, not a request for a shade of green.
fn read_tint(field: Option<&str>) -> (BadgeIntent, Option<Vec4f>) {
    let text = field.unwrap_or("").trim();
    if text.is_empty() {
        return (BadgeIntent::Neutral, None);
    }
    if let Some(intent) = read_intent(text) {
        return (intent, None);
    }
    if text.starts_with('#') {
        if let Some((c, _had_alpha)) = parse_hex_color(text) {
            return (BadgeIntent::Neutral, Some(vec4(c[0], c[1], c[2], c[3])));
        }
    }
    (BadgeIntent::Neutral, None)
}

/// The eight intent words, lower-cased. Written out rather than derived:
/// `BadgeIntent`'s script names are the DSL's business, and a line of
/// markup is text a person typed.
fn read_intent(word: &str) -> Option<BadgeIntent> {
    Some(match word.to_ascii_lowercase().as_str() {
        "neutral" => BadgeIntent::Neutral,
        "primary" => BadgeIntent::Primary,
        "secondary" => BadgeIntent::Secondary,
        "tertiary" => BadgeIntent::Tertiary,
        "error" => BadgeIntent::Error,
        "warning" => BadgeIntent::Warning,
        "success" => BadgeIntent::Success,
        "info" => BadgeIntent::Info,
        _ => return None,
    })
}

// ---- what a finger has hold of -------------------------------------------

/// The thing under the press, by kind and by index into the list it came
/// from.
///
/// Public because it rides in [`WaveformAction::Grabbed`], which is the only
/// way a host can know what a drag is about to be before the first frame of
/// it moves anything.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum WaveformPart {
    Playhead,
    /// The opening end of the region at this index.
    RegionStart(usize),
    /// The closing end of the region at this index.
    RegionEnd(usize),
    Marker(usize),
}

// ---- what it reports ------------------------------------------------------

/// What a waveform says happened.
///
/// # The order, which is contract
///
/// A pass carries at most two of these, and when it carries two, `Grabbed`
/// is the first:
///
/// | pass | what is emitted |
/// |---|---|
/// | press on the body | `Grabbed(Playhead)`, then `Seeking(at)` |
/// | press on a region edge | `Grabbed(RegionStart/End(i))` |
/// | press on a chip | `Grabbed(Marker(i))` |
/// | drag | one of `Seeking` / `RegionMoving` / `MarkerMoving` |
/// | release | one of `Seeked` / `RegionMoved` / `MarkerMoved` / `MarkerPicked` |
/// | an arrow, Home, End | `Seeked(at)` |
///
/// A live report and its settled one never share a pass, so nothing has to
/// win over anything. A key press is settled the moment it happens, so it
/// reports `Seeked` and nothing else — a host that scrubs on `Seeking` and
/// plays on `Seeked` behaves correctly on the keyboard without a special
/// case.
///
/// The body press is the pass that carries two, and that is why every
/// reader below scans the pass instead of taking the first action in it.
///
/// Use the live reports for anything that must keep up with the finger and
/// the settled ones for anything expensive, because a drag across a
/// five-minute lane passes through several hundred values on its way.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum WaveformAction {
    /// A press landed on this part. Nothing has moved yet.
    Grabbed(WaveformPart),
    /// The playhead, on every frame of a press or drag on the lane body.
    Seeking(f64),
    /// Where the playhead was let go, or where a key put it.
    Seeked(f64),
    /// A region's two ends while one of them is being dragged: which
    /// region, then start and end.
    RegionMoving(usize, f64, f64),
    /// The ends the region was let go at.
    RegionMoved(usize, f64, f64),
    /// A mark while it is being dragged: which mark, and where it is now.
    MarkerMoving(usize, f64),
    /// Where the mark was let go.
    MarkerMoved(usize, f64),
    /// A mark that was pressed and released without being moved. A pick and
    /// a move are told apart by how far the finger travelled, not by
    /// whether the number changed: on a long recording one point is a real
    /// number of seconds, so every click would otherwise be a tiny edit.
    MarkerPicked(usize),
    #[default]
    None,
}

/// Scan one widget's actions in a pass and answer with the first one the
/// picker accepts.
///
/// Every reader on [`WaveformRef`] goes through this rather than
/// `find_widget_action` (widget.rs:1715-1724), which returns the FIRST
/// action from a uid and stops: this widget emits two on the frame a body
/// press lands — `Grabbed(Playhead)` and then `Seeking(at)` — so a reader
/// built on that helper would see one of the pair and report None for the
/// other. The library says so directly above it (widget.rs:1595-1601) and
/// ships `filter_widget_actions_cast` for the case.
///
/// It takes an ITERATOR rather than the `Actions`, so the press-frame
/// contract can be tested without a script heap.
fn scan<T>(
    actions: impl Iterator<Item = WaveformAction>,
    mut pick: impl FnMut(WaveformAction) -> Option<T>,
) -> Option<T> {
    for action in actions {
        if let Some(found) = pick(action) {
            return Some(found);
        }
    }
    None
}

// ---- the peaks it is holding ---------------------------------------------

/// What the widget was given, in the shape it was given it.
///
/// Two variants and not one stored pair array: in the magnitude case the low
/// of every column is exactly minus the high, and storing it would be a
/// million floats the widget computed itself. `reduce` reads either.
#[derive(Clone, Debug, Default, PartialEq)]
enum Peaks {
    #[default]
    None,
    /// One magnitude per column, drawn mirrored about the centre line.
    Magnitudes(Vec<f32>),
    /// The lowest and the highest sample in each column, in that order.
    Pairs(Vec<(f32, f32)>),
}

impl Peaks {
    fn len(&self) -> usize {
        match self {
            Peaks::None => 0,
            Peaks::Magnitudes(v) => v.len(),
            Peaks::Pairs(v) => v.len(),
        }
    }

    /// Column `i` as a (low, high) pair, low first.
    fn at(&self, i: usize) -> (f32, f32) {
        match self {
            Peaks::None => (0.0, 0.0),
            Peaks::Magnitudes(v) => {
                let m = v[i].abs();
                (-m, m)
            }
            Peaks::Pairs(v) => v[i],
        }
    }
}

/// Put every column's two numbers in order and say whether the result
/// differs from what is held; `None` means nothing changed.
///
/// Cx-free, so the guard the setters are built on can be tested without a
/// script heap. The ordering happens here rather than being checked, so
/// everything downstream — the packing, the shader, the silence guard — can
/// rely on the low being the low.
fn settle_pairs(held: &Peaks, mut peaks: Vec<(f32, f32)>) -> Option<Peaks> {
    for pair in peaks.iter_mut() {
        *pair = (pair.0.min(pair.1), pair.0.max(pair.1));
    }
    if matches!(held, Peaks::Pairs(h) if *h == peaks) {
        return None;
    }
    Some(Peaks::Pairs(peaks))
}

/// The same guard for the magnitude case.
fn settle_magnitudes(held: &Peaks, peaks: Vec<f32>) -> Option<Peaks> {
    if matches!(held, Peaks::Magnitudes(h) if *h == peaks) {
        return None;
    }
    Some(Peaks::Magnitudes(peaks))
}

// ---- the picture the GPU reads -------------------------------------------

/// The scale a value is packed against: 255 * 256, and not 65535.
///
/// The shader gets a value back as `high_channel + low_channel / 256`, and
/// each channel arrives as its byte over 255 — so that sum is
/// `(high * 256 + low) / (255 * 256)`. Packing against the same number makes
/// the decode an exact inverse of the scale, which is what lets the
/// round-trip test below be exact on the grid instead of "within four parts
/// in a thousand". voice_wave's `signed_to_u16` (voice_wave.rs:218-220)
/// scales to 65535 and lets its `clamp` absorb the overshoot; the texel
/// layout here is that widget's, this one constant is not.
const TEXEL_FULL: f32 = 65280.0;

/// One value as sixteen bits, -1..1 mapped over 0..TEXEL_FULL.
fn pack_value(v: f32) -> u32 {
    ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * TEXEL_FULL).round() as u32
}

/// One column as the texture holds it: the high of the column in the top
/// sixteen bits and the low in the bottom sixteen.
///
/// This is the layout voice_wave.rs:248-252 already ships against this exact
/// format — `(hi << 16) | lo` — and its shader reassembles each value out of
/// two channels at voice_wave.rs:60-63. In a BGRA u32 that puts the high
/// byte of `hi` in the alpha channel and its low byte in red, and the high
/// byte of `lo` in green with its low byte in blue, which is why the decode
/// reads `t.w + t.x / 256.0` and `t.y + t.z / 256.0`. Nothing is left at
/// zero: all four channels carry data.
///
/// Sixteen bits a side is a step of 2/65280 of the half-body. On a lane 96
/// points tall with the default 16-point strip the half-body is 40 points,
/// so one step is 0.0012 of a point — about a four-hundredth of a device
/// pixel on a two-times screen, and there is no argument to have about it.
fn pack_column((lo, hi): (f32, f32)) -> u32 {
    (pack_value(hi) << 16) | pack_value(lo)
}

/// The Rust mirror of the decode in the lane's shader. Keep the two in step:
/// the shader is the picture and this is what the tests measure. Nothing in
/// the widget reads a texel back, so it exists only under `cfg(test)` — the
/// proof, not the path.
#[cfg(test)]
fn unpack_column(texel: u32) -> (f32, f32) {
    let value = |u: u32| (u & 0xffff) as f32 / TEXEL_FULL * 2.0 - 1.0;
    (value(texel), value(texel >> 16))
}

/// The most texels a lane's picture is ever reduced into: 8192, one row,
/// 32 KB.
///
/// That is past the 7680 device pixels of an 8K panel and short of what a
/// window stretched across two of them would want. A lane wider than this is
/// drawn at this resolution, so each texel covers a little more than a pixel
/// and the picture goes very slightly soft. Nothing is lost and nothing
/// reads past its data; it is worth naming the number rather than claiming
/// no display gets there.
pub const MAX_COLUMNS: usize = 8192;

/// One texel per output column: the lowest of the lows and the highest of
/// the highs in the source columns it covers.
///
/// A MAX reduction and not an average. An average of a hundred columns of
/// music is the same grey smear whatever the music was; taking the extremes
/// keeps every transient visible however far the recording is squeezed, and
/// that is the whole difference between a picture that reads as music and
/// one that reads as a hedge.
///
/// When there are fewer peaks than columns each column takes the one peak
/// its span lands in — a stair, not a curve. A smooth curve drawn between
/// two peaks is audio that was never measured, and it would be the part of
/// the picture a reader trusts most.
fn reduce(peaks: &Peaks, cols: usize) -> Vec<u32> {
    let n = peaks.len();
    if n == 0 || cols == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(cols);
    for c in 0..cols {
        // In u64 on purpose: a day of audio times 8192 columns overflows a
        // 32-bit usize, and wasm32's usize is 32 bits.
        let from = (c as u64 * n as u64 / cols as u64) as usize;
        let to = (((c as u64 + 1) * n as u64 / cols as u64) as usize).max(from + 1).min(n);
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for i in from..to {
            let (l, h) = peaks.at(i);
            if l < lo {
                lo = l;
            }
            if h > hi {
                hi = h;
            }
        }
        out.push(pack_column((lo, hi)));
    }
    out
}

// ---- the draw structs -----------------------------------------------------
//
// Some backends cap a vertex shader at 32 inputs, and a draw shader that
// goes over does not fail loudly — it fails to compile and the widget draws
// nothing at all on that platform. `DrawQuad` contributes 12 floats
// (draw_quad.rs:80-98), every `#[live]` scalar after the `#[deref]` is one
// more, and every `instance()` the DSL declares is one more again. So the
// count is written down here and should be re-added when a field is:
//
//   DrawWaveformLane    8 fields + 4 animator instances + 12 = 24
//   DrawWaveformRegion  11 fields + 12                       = 23
//   DrawWaveformMarker  9 fields + 12                        = 21
//   DrawWaveformHead    6 fields + 12                        = 18
//
// Every colour and every number the widget does not write per draw is a
// `uniform(...)` in the DSL and costs nothing here.

/// The ground and the peaks. Only `#[live]` instance fields after the
/// `#[deref]`, and `#[repr(C)]`: the instance buffer is laid out by
/// declaration order, and putting anything else there corrupts it silently.
/// The texture handle is NOT here — it lives on the widget.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWaveformLane {
    #[deref]
    pub draw_super: DrawQuad,
    /// How many texels the row holds, which is also how many columns of
    /// picture there are: the texture is resized to fit exactly. Zero says
    /// there are no peaks, and the lane draws its ground and its rule.
    #[live]
    pub cols: f32,
    /// What fraction of the lane's width those columns cover, 0..1 —
    /// `peaks_span / duration`. Past it there are no peaks.
    #[live(1.0)]
    pub covered: f32,
    /// Where the playhead sits across the width, 0..1, and whether there is
    /// one. `head_on` does double duty: it draws nothing itself, but with
    /// no playhead there is no played half either.
    #[live]
    pub head: f32,
    #[live]
    pub head_on: f32,
    /// The band at the top the marks own, in points. The peaks are drawn in
    /// what is left under it.
    #[live(16.0)]
    pub strip_px: f32,
    /// How much of the half-body a full-scale peak fills. Short of 1 on
    /// purpose: a column that touches the edge cannot be told from one that
    /// would have gone past it.
    #[live(0.88)]
    pub envelope: f32,
    /// How far a played column fades toward the lane's own ground.
    #[live(0.55)]
    pub played_fade: f32,
    /// Corner rounding in points. `sdf.box` draws twice the radius it is
    /// given (sdf.rs:407-415), so the shader halves this.
    #[live(3.0)]
    pub border_radius: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWaveformRegion {
    #[deref]
    pub draw_super: DrawQuad,
    /// This region's colour: the host's if it gave one, otherwise the
    /// intent's, resolved through the shared palette.
    #[live]
    pub color: Vec4f,
    /// Which edge the pointer is on, so a press can be anticipated rather
    /// than discovered. The two edges look alike and the middle is not
    /// grabbable at all, so nothing else would say where to put the finger.
    #[live]
    pub hot_start: f32,
    #[live]
    pub hot_end: f32,
    /// Where the marks' band ends. Nothing is painted above it.
    #[live(16.0)]
    pub strip_px: f32,
    #[live(0.16)]
    pub wash: f32,
    #[live(1.5)]
    pub edge_size: f32,
    #[live(7.0)]
    pub cap_length: f32,
    /// How much the ink lifts under the pointer, as a straight addition to
    /// rgb — range_slider.rs:183's units, not a coverage multiplier.
    #[live(0.10)]
    pub hot_lift: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWaveformMarker {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub hot: f32,
    /// The band the chip fills, in points, taken raw and never floored.
    #[live(16.0)]
    pub strip_px: f32,
    #[live(1.0)]
    pub stem_size: f32,
    /// How far down the body the stem reaches, 0..1.
    #[live(1.0)]
    pub stem_reach: f32,
    #[live(0.10)]
    pub hot_lift: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWaveformHead {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    /// The hard core's half-width and the halo's reach, in points. The
    /// widget sizes the quad `max(2 * halo, 2)` wide so both are real
    /// distances inside it.
    #[live(1.0)]
    pub core: f32,
    #[live(6.0)]
    pub halo: f32,
}

// ---- the arithmetic, apart from the widget --------------------------------

/// Where everything on the lane sits, and what a press at a point takes hold
/// of.
///
/// A separate, cx-free type for the reason `RangeSlider`'s `Range` is one:
/// it can be tested without a script heap, and the hit test and the drawing
/// are handed the same numbers from the same place, so what is drawn is what
/// is grabbable. It is a second implementation of the same stopping rule and
/// not a reuse — `Range` is private at range_slider.rs:289 — and what is
/// copied is the rule, `min_span` and the quantization, settled there.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Lane {
    duration: f64,
    step: f64,
    min_span: f64,
    /// The lane's own size, in points.
    width: f64,
    height: f64,
    /// The band at the top the marks own, before clamping.
    strip: f64,
    edge_grab: f64,
    marker_grab: f64,
}

impl Lane {
    /// Where the marks' band ends and the body starts, in points from the
    /// top. The three shaders compute `clamp(strip_px, 0, rect_size.y)` and
    /// this is the same expression, so what is painted above the body is
    /// exactly what is grabbed above it — at a band of nothing, at the
    /// default, and at a band taller than the lane.
    fn body_top(&self) -> f64 {
        self.strip.clamp(0.0, self.height)
    }

    fn quantize(&self, v: f64) -> f64 {
        if self.step > 0.0 {
            (v / self.step).round() * self.step
        } else {
            v
        }
    }

    /// A time as 0..1 across the lane.
    fn travel(&self, at: f64) -> f64 {
        if self.duration <= 0.0 {
            0.0
        } else {
            (at / self.duration).clamp(0.0, 1.0)
        }
    }

    /// And as an x, in points from the lane's left edge.
    fn x_of(&self, at: f64) -> f64 {
        self.travel(at) * self.width
    }

    /// And back, quantized by `step` and held inside the axis.
    fn time_at(&self, x: f64) -> f64 {
        if self.width <= 0.0 {
            return 0.0;
        }
        let t = (x / self.width).clamp(0.0, 1.0);
        self.quantize(t * self.duration).clamp(0.0, self.duration.max(0.0))
    }

    /// A tolerance in points, as a length of time. The hit test compares
    /// points to points, so this is what the tests use to say "two chips
    /// two reaches apart" in the unit the marks are written in.
    #[cfg(test)]
    fn reach(&self, px: f64) -> f64 {
        if self.width <= 0.0 {
            0.0
        } else {
            px / self.width * self.duration
        }
    }

    /// Whether a time is on the lane at all. A region or a mark outside the
    /// axis is skipped and never clamped: a chip parked at the edge claims a
    /// mark is there when it is minutes away.
    fn holds(&self, at: f64) -> bool {
        at >= 0.0 && at <= self.duration
    }

    /// Move a region's opening end, the closing one staying where it is.
    ///
    /// The moving end stops `min_span` short of the other rather than
    /// shoving it along. During a drag the finger is on ONE end, so that end
    /// is the one that gives way; moving the other would destroy a number
    /// nobody is touching and one that was probably set on purpose. The rule
    /// is the range slider's, settled there.
    fn move_start(&self, region: &mut WaveformRegion, to: f64) {
        let stop = (region.end - self.min_span.max(0.0)).max(0.0);
        region.start = to.clamp(0.0, stop);
    }

    /// Move a region's closing end, the opening one staying where it is.
    fn move_end(&self, region: &mut WaveformRegion, to: f64) {
        let stop = (region.start + self.min_span.max(0.0)).min(self.duration);
        region.end = to.clamp(stop, self.duration);
    }

    /// What a press at this point takes hold of.
    ///
    /// `None` for a press in the strip that landed on no chip, which is what
    /// makes "a press in the strip is a mark or it is nothing" true, and the
    /// marks are skipped entirely when the band is nothing.
    fn grab_at(
        &self,
        x: f64,
        y: f64,
        regions: &[WaveformRegion],
        markers: &[WaveformMarker],
    ) -> Option<WaveformPart> {
        let body_top = self.body_top();
        if body_top > 0.0 && y < body_top {
            let mut best: Option<(usize, f64)> = None;
            for (i, marker) in markers.iter().enumerate() {
                if !self.holds(marker.at) {
                    continue;
                }
                let d = (x - self.x_of(marker.at)).abs();
                if d > self.marker_grab {
                    continue;
                }
                // The nearer chip, and the earlier one on a tie. Below twice
                // `marker_grab` the two reaches overlap, so a press in the
                // band between the marks is ambiguous and goes to the nearer
                // — but a press ON either chip still takes that chip, because
                // nearest wins. What actually stops a finger aiming is
                // `chip_width`: below that the chips themselves overlap and
                // there is no separate thing left to point at.
                if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                    best = Some((i, d));
                }
            }
            return best.map(|(i, _)| WaveformPart::Marker(i));
        }
        let mut best: Option<(WaveformPart, f64)> = None;
        for (i, region) in regions.iter().enumerate() {
            if region.end < 0.0 || region.start > self.duration {
                continue;
            }
            let ends = [
                (WaveformPart::RegionStart(i), region.start),
                (WaveformPart::RegionEnd(i), region.end),
            ];
            for (part, at) in ends {
                let d = (x - self.x_of(at)).abs();
                if d > self.edge_grab {
                    continue;
                }
                if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                    best = Some((part, d));
                }
            }
        }
        match best {
            Some((part, _)) => Some(part),
            // Everything else in the body is a seek. There is no third
            // answer: a press inside a region moves the playhead, because a
            // region is a span with a name and moving one whole is an edit
            // with a meaning this widget cannot know.
            None => Some(WaveformPart::Playhead),
        }
    }
}

// ---- the widget -----------------------------------------------------------

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct Waveform {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[apply_default]
    animator: Animator,
    /// The whole lane, quads and text together. Everything is drawn with
    /// `draw_abs` into one walked rect, so the rect is what hits are tested
    /// against and what a redraw asks for.
    #[redraw]
    #[area]
    area: Area,

    /// The ground and the peaks: one quad and one texture fetch a pixel.
    #[live]
    pub draw_lane: DrawWaveformLane,
    /// One per region.
    #[live]
    pub draw_region: DrawWaveformRegion,
    /// One per mark: the chip in the strip and the stem down the body.
    #[live]
    pub draw_marker: DrawWaveformMarker,
    /// One, over everything else.
    #[live]
    pub draw_head: DrawWaveformHead,
    /// Region and mark names alike. One colour for both, and never the
    /// item's own: the colour is already carried by the chip and the wash,
    /// and a name repeated in a pale region's own colour is a name nobody
    /// can read.
    #[live]
    pub draw_name: DrawText,
    /// The colour of every intent, inherited from the shared palette so a
    /// region and a badge that mean the same thing look the same.
    #[live]
    pub palette: BadgePalette,

    /// How far the lane's axis runs. Every number this widget takes and
    /// reports is in this unit; it never converts and never formats one.
    /// The default of 1 makes the axis a fraction of the whole, which is
    /// the honest thing for a caller that has peaks and no duration.
    #[live(1.0)]
    pub duration: f64,
    /// How much of the axis the peak array covers, from 0. Zero means all
    /// of `duration`, which is the ordinary case.
    ///
    /// This exists for the file that is still decoding: hand over the peaks
    /// for the first thirty seconds with `peaks_span: 30` and they draw
    /// across the first thirty seconds of lane, with the rest of the lane
    /// empty, instead of being stretched over the whole of it.
    #[live]
    pub peaks_span: f64,
    /// Where the playhead is, in the same unit.
    #[live]
    pub playhead: f64,
    /// Draw the playhead at all. Off also takes the played/coming shading
    /// off the peaks, because with no playhead there is no "played".
    #[live(true)]
    pub show_playhead: bool,
    /// Draw region and mark names.
    #[live(true)]
    pub show_names: bool,

    /// The regions as markup writes them, one string apiece.
    #[live]
    pub regions: Vec<String>,
    /// The marks as markup writes them, one string apiece.
    #[live]
    pub markers: Vec<String>,

    /// The least a region may be squeezed to by a drag. Zero lets its two
    /// ends meet.
    #[live]
    pub min_span: f64,
    /// Quantization for every drag, in the axis unit; 0 is continuous. A
    /// caller with a beat grid sets the beat here and gets a lane that
    /// cannot be dragged off it.
    #[live]
    pub step: f64,

    /// The band at the top of the lane the marks own, in points. Nothing
    /// else is drawn or grabbed there, and at 0 there are no marks at all.
    #[live(16.0)]
    pub marker_strip: f64,
    /// How close a press must come to a region's edge to take it, in
    /// points. Points rather than time on purpose: a two-second region on a
    /// two-hour recording has to stay catchable.
    #[live(6.0)]
    pub edge_grab: f64,
    /// How close a press must come to a mark's chip to take it, in points.
    /// Below twice this two marks' reaches overlap, so a press in the band
    /// between them is ambiguous and goes to the nearer, the earlier one on
    /// a tie; a press on either chip still takes that chip. It is
    /// [`chip_width`](Self::chip_width) that decides whether there are two
    /// things to aim at in the first place.
    #[live(7.0)]
    pub marker_grab: f64,
    /// A mark chip's width, in points.
    #[live(9.0)]
    pub chip_width: f64,
    /// The room a name leaves the thing it names, in points.
    #[live(4.0)]
    pub name_inset: f64,

    /// What the host handed over, in the shape it handed it over in.
    #[rust]
    peaks: Peaks,
    #[rust]
    parsed_regions: Vec<WaveformRegion>,
    /// The strings `parsed_regions` was made from, so the widget can tell a
    /// list it has already read from one markup has just handed it again.
    #[rust]
    region_lines: Vec<String>,
    #[rust]
    parsed_markers: Vec<WaveformMarker>,
    #[rust]
    marker_lines: Vec<String>,

    /// The reduced picture. One row, one texel a device pixel of covered
    /// lane. `#[new]` and not `#[rust]`, because `Texture::new` hands back
    /// the process's shared null texture and `ensure_texture` has to replace
    /// it with one of this widget's own before anything is written —
    /// voice_wave.rs:156 and 199-210 hold theirs exactly this way.
    #[new]
    texture: Texture,
    #[rust]
    texture_built: bool,
    /// How many texels the last build wrote, so a resize that changes
    /// nothing does not rebuild.
    #[rust]
    drawn_cols: usize,
    /// The peaks changed and the picture has to be built again whatever the
    /// width is doing.
    #[rust]
    stale: bool,

    #[rust]
    grab: Option<WaveformPart>,
    /// What a press would take, so the lane can say so before it is
    /// pressed.
    #[rust]
    hot: Option<WaveformPart>,
    /// Where the press landed, for telling a pick from a move.
    #[rust]
    press_x: f64,
}

impl Waveform {
    // ---- the peaks --------------------------------------------------------

    /// The peaks as magnitudes 0..1, drawn mirrored about the centre line.
    ///
    /// This is the setter for a host that has one number per column — an RMS
    /// envelope, a level meter's history, a series that is not audio at all.
    /// Prefer [`set_peak_pairs`](Self::set_peak_pairs) for real audio.
    ///
    /// Taken by value, and compared before it is stored: a host that can
    /// give its array away may call this on every pass and pay a slice
    /// comparison rather than a reduce and a texture upload, and when the
    /// array does differ it is moved in rather than copied. A host that
    /// keeps its own copy would have to clone to call this at all, and
    /// should ask [`peak_count`](Self::peak_count) first.
    pub fn set_peaks(&mut self, cx: &mut Cx, peaks: Vec<f32>) {
        if let Some(peaks) = settle_magnitudes(&self.peaks, peaks) {
            self.peaks = peaks;
            self.stale = true;
            self.area.redraw(cx);
        }
    }

    /// The peaks as the lowest and highest sample in each column, each
    /// -1..1.
    ///
    /// This is the one to use when the numbers came off real audio. A
    /// mirrored magnitude throws away the half of the picture that shows a
    /// signal sitting off the centre line and a transient that goes one way
    /// harder than the other, and those are exactly the two things somebody
    /// opens a waveform to look at.
    ///
    /// The two are put in order per column here rather than checked, so
    /// everything downstream — the packing, the shader, the silence guard —
    /// can rely on the low being the low. Guarded and taken by value like
    /// [`set_peaks`](Self::set_peaks).
    pub fn set_peak_pairs(&mut self, cx: &mut Cx, peaks: Vec<(f32, f32)>) {
        if let Some(peaks) = settle_pairs(&self.peaks, peaks) {
            self.peaks = peaks;
            self.stale = true;
            self.area.redraw(cx);
        }
    }

    /// Forget the peaks. The lane keeps its regions, marks and playhead and
    /// draws its centre rule, which is what a lane between two recordings
    /// should look like — not an empty box.
    pub fn clear_peaks(&mut self, cx: &mut Cx) {
        if matches!(self.peaks, Peaks::None) {
            return;
        }
        self.peaks = Peaks::None;
        self.stale = true;
        self.area.redraw(cx);
    }

    /// How many peaks it is holding. A length and not a copy, so a host that
    /// keeps its own array can ask whether this lane has been seeded without
    /// paying for the answer.
    pub fn peak_count(&self) -> usize {
        self.peaks.len()
    }

    /// How much of the axis those peaks cover; 0 means all of `duration`.
    pub fn peaks_span(&self) -> f64 {
        self.peaks_span
    }

    /// Say what stretch of the axis the peak array covers. See the field.
    pub fn set_peaks_span(&mut self, cx: &mut Cx, span: f64) {
        if self.peaks_span != span {
            self.peaks_span = span;
            self.stale = true;
            self.area.redraw(cx);
        }
    }

    // ---- the lists --------------------------------------------------------

    /// The regions, replacing whatever markup gave. The markup list is
    /// dropped with them: leaving it would let the next draw take it back.
    pub fn set_regions(&mut self, cx: &mut Cx, regions: Vec<WaveformRegion>) {
        if self.parsed_regions != regions {
            self.parsed_regions = regions;
            self.regions.clear();
            self.region_lines.clear();
            self.area.redraw(cx);
        }
    }

    pub fn regions(&self) -> &[WaveformRegion] {
        &self.parsed_regions
    }

    /// How many regions are on the lane, without copying them.
    pub fn region_count(&self) -> usize {
        self.parsed_regions.len()
    }

    pub fn set_markers(&mut self, cx: &mut Cx, markers: Vec<WaveformMarker>) {
        if self.parsed_markers != markers {
            self.parsed_markers = markers;
            self.markers.clear();
            self.marker_lines.clear();
            self.area.redraw(cx);
        }
    }

    pub fn markers(&self) -> &[WaveformMarker] {
        &self.parsed_markers
    }

    /// How many marks are on the lane, without copying them.
    pub fn marker_count(&self) -> usize {
        self.parsed_markers.len()
    }

    // ---- the numbers ------------------------------------------------------

    pub fn playhead(&self) -> f64 {
        self.playhead
    }

    /// Put the playhead somewhere with no gesture to watch. Setting a
    /// position is not a drag, so nothing is reported: a host that moved the
    /// playhead already knows where it put it, and a widget that told it
    /// back would put every transport into a loop.
    pub fn set_playhead(&mut self, cx: &mut Cx, at: f64) {
        let at = at.clamp(0.0, self.duration.max(0.0));
        if self.playhead != at {
            self.playhead = at;
            self.area.redraw(cx);
        }
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// How far the axis runs.
    ///
    /// The peaks do NOT stretch with it. They are the fixed picture, spread
    /// across the first `peaks_span` of the axis — which is all of it unless
    /// the host said otherwise — and it is the regions, the marks and the
    /// playhead that slide against them, because each of those is a number
    /// on the axis and the axis just changed length.
    ///
    /// So a host that lengthens `duration` without lengthening `peaks_span`
    /// is saying "there is more recording than I have peaks for", and gets a
    /// lane whose right-hand part is empty. A host that lengthens it and
    /// leaves `peaks_span` at 0 is saying "the same peaks now cover more
    /// time", which is a resample it did not do.
    ///
    /// The playhead comes back onto the axis with it. Every other path holds
    /// it inside 0..duration — [`set_playhead`](Self::set_playhead) clamps,
    /// and so does `Lane::time_at` under a drag — so a shortened axis that
    /// left it outside would be the one way to make `playhead()` answer with
    /// a number the lane cannot draw, and the first arrow key after that
    /// would jump the value from wherever it was to the end of the new axis.
    pub fn set_duration(&mut self, cx: &mut Cx, secs: f64) {
        if self.duration != secs {
            self.duration = secs;
            self.playhead = self.playhead.clamp(0.0, self.duration.max(0.0));
            self.stale = true;
            self.area.redraw(cx);
        }
    }

    // ---- the internals ----------------------------------------------------

    /// The arithmetic for a lane of this size, built fresh each draw and
    /// each hit: every number in it is a live property and the tweaker may
    /// have moved any of them since the last frame.
    fn lane_of(&self, size: DVec2) -> Lane {
        Lane {
            duration: self.duration,
            step: self.step,
            min_span: self.min_span,
            width: size.x,
            height: size.y,
            strip: self.marker_strip,
            edge_grab: self.edge_grab,
            marker_grab: self.marker_grab,
        }
    }

    /// What fraction of the lane's width the peaks occupy.
    fn covered(&self) -> f64 {
        if self.peaks_span <= 0.0 || self.duration <= 0.0 {
            1.0
        } else {
            (self.peaks_span / self.duration).clamp(0.0, 1.0)
        }
    }

    /// A region's or a mark's colour: the host's if it gave one, otherwise
    /// the intent's, out of the shared palette.
    fn tint(&self, intent: BadgeIntent, own: Option<Vec4f>) -> Vec4f {
        own.unwrap_or_else(|| self.palette.family(intent).base)
    }

    fn ensure_texture(&mut self, cx: &mut Cx) {
        if !self.texture_built {
            self.texture = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    data: Some(vec![pack_column((0.0, 0.0)); 1]),
                    width: 1,
                    height: 1,
                    updated: TextureUpdated::Full,
                },
            );
            self.texture_built = true;
        }
    }

    /// Reduce the peaks into one row of texture as wide as the pixels they
    /// will occupy, and answer how many texels that is.
    ///
    /// Done when the peaks change, when `duration` or `peaks_span` change,
    /// or when the width of the covered lane changes — and at no other time.
    /// `set_data_u32` writes the row and the new width together
    /// (platform/src/texture.rs:975-996), so there is no second width to
    /// keep in step and the shader needs no `tex_w`.
    fn refresh_picture(&mut self, cx: &mut Cx, width_px: f64) -> usize {
        self.ensure_texture(cx);
        if self.peaks.len() == 0 {
            self.drawn_cols = 0;
            self.stale = false;
            return 0;
        }
        let cols = ((width_px * self.covered()).round().max(1.0) as usize).min(MAX_COLUMNS);
        if self.stale || cols != self.drawn_cols {
            let data = reduce(&self.peaks, cols);
            self.texture.set_data_u32(cx, cols, 1, data);
            self.drawn_cols = cols;
            self.stale = false;
        }
        cols
    }

    /// The marks in time order, as indices into the list the host wrote.
    /// The room a mark's name has is the distance to the next mark IN TIME,
    /// not in the order it was written, so the host does not have to sort.
    fn marker_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.parsed_markers.len()).collect();
        order.sort_by(|a, b| {
            self.parsed_markers[*a]
                .at
                .partial_cmp(&self.parsed_markers[*b].at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        order
    }

    fn key_step(&self) -> f64 {
        if self.step > 0.0 {
            self.step
        } else {
            self.duration * 0.01
        }
    }

    /// Say what a press would take, before it is pressed.
    fn set_hot(&mut self, cx: &mut Cx, hot: Option<WaveformPart>) {
        if self.hot != hot {
            self.hot = hot;
            self.area.redraw(cx);
        }
    }

    fn draw_names_of_regions(&mut self, cx: &mut Cx2d, rect: Rect, lane: &Lane) {
        let size = self.draw_name.text_style.font_size as f64;
        let box_h = size * LINE_BOX;
        let top = rect.pos.y + rect.size.y - self.name_inset - box_h;
        let regions = std::mem::take(&mut self.parsed_regions);
        for region in regions.iter() {
            if region.name.is_empty() || region.end < 0.0 || region.start > lane.duration {
                continue;
            }
            let x0 = rect.pos.x + lane.x_of(region.start);
            let x1 = rect.pos.x + lane.x_of(region.end);
            let room = x1 - x0 - self.name_inset * 2.0;
            let width = measure(&self.draw_name, cx, &region.name);
            // Skipped, never clipped and never shortened. Half a word says
            // less than no word and costs the reader a second working out
            // that it is half a word; the host has the whole name anyway.
            if width > room {
                continue;
            }
            self.draw_name.draw_abs(
                cx,
                dvec2(x0 + self.name_inset, text_top(top, box_h, size)),
                &region.name,
            );
        }
        self.parsed_regions = regions;
    }

    fn draw_names_of_markers(&mut self, cx: &mut Cx2d, rect: Rect, lane: &Lane) {
        let size = self.draw_name.text_style.font_size as f64;
        let order = self.marker_order();
        let markers = std::mem::take(&mut self.parsed_markers);
        let right = rect.pos.x + rect.size.x;
        for (slot, index) in order.iter().enumerate() {
            let marker = &markers[*index];
            if marker.name.is_empty() || !lane.holds(marker.at) {
                continue;
            }
            let x = rect.pos.x + lane.x_of(marker.at) + self.chip_width * 0.5 + self.name_inset;
            // The next mark in TIME is what the name has to stop short of.
            let stop = order
                .get(slot + 1)
                .map(|next| rect.pos.x + lane.x_of(markers[*next].at) - self.chip_width * 0.5)
                .unwrap_or(right);
            let width = measure(&self.draw_name, cx, &marker.name);
            if width > stop.min(right) - x {
                continue;
            }
            self.draw_name.draw_abs(
                cx,
                dvec2(x, text_top(rect.pos.y, lane.body_top(), size)),
                &marker.name,
            );
        }
        self.parsed_markers = markers;
    }
}

// ---- the handle -----------------------------------------------------------

impl WaveformRef {
    pub fn set_peaks(&self, cx: &mut Cx, peaks: Vec<f32>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_peaks(cx, peaks);
        }
    }

    pub fn set_peak_pairs(&self, cx: &mut Cx, peaks: Vec<(f32, f32)>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_peak_pairs(cx, peaks);
        }
    }

    pub fn clear_peaks(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_peaks(cx);
        }
    }

    pub fn peak_count(&self) -> usize {
        self.borrow().map(|inner| inner.peak_count()).unwrap_or(0)
    }

    pub fn peaks_span(&self) -> f64 {
        self.borrow().map(|inner| inner.peaks_span()).unwrap_or(0.0)
    }

    pub fn set_peaks_span(&self, cx: &mut Cx, span: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_peaks_span(cx, span);
        }
    }

    pub fn set_regions(&self, cx: &mut Cx, regions: Vec<WaveformRegion>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_regions(cx, regions);
        }
    }

    /// The regions as they stand — including a change a drag has just made
    /// to one, which is how a host reads back what it did not write. This
    /// one copies; [`region_count`](Self::region_count) does not.
    pub fn regions(&self) -> Vec<WaveformRegion> {
        self.borrow().map(|inner| inner.regions().to_vec()).unwrap_or_default()
    }

    pub fn region_count(&self) -> usize {
        self.borrow().map(|inner| inner.region_count()).unwrap_or(0)
    }

    pub fn set_markers(&self, cx: &mut Cx, markers: Vec<WaveformMarker>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_markers(cx, markers);
        }
    }

    pub fn markers(&self) -> Vec<WaveformMarker> {
        self.borrow().map(|inner| inner.markers().to_vec()).unwrap_or_default()
    }

    pub fn marker_count(&self) -> usize {
        self.borrow().map(|inner| inner.marker_count()).unwrap_or(0)
    }

    pub fn playhead(&self) -> f64 {
        self.borrow().map(|inner| inner.playhead()).unwrap_or(0.0)
    }

    pub fn set_playhead(&self, cx: &mut Cx, at: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_playhead(cx, at);
        }
    }

    pub fn duration(&self) -> f64 {
        self.borrow().map(|inner| inner.duration()).unwrap_or(0.0)
    }

    pub fn set_duration(&self, cx: &mut Cx, secs: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_duration(cx, secs);
        }
    }

    // ---- reading a pass ---------------------------------------------------
    //
    // Every reader below SCANS the pass through `scan`, for the reason
    // written above that function: a body press puts two actions in one
    // pass and a reader that took the first would be blind to the other.

    /// What a press just took hold of, if one did.
    pub fn grabbed(&self, actions: &Actions) -> Option<WaveformPart> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::Grabbed(part) => Some(part),
                _ => None,
            },
        )
    }

    /// The playhead while a press or drag is moving it.
    pub fn seeking(&self, actions: &Actions) -> Option<f64> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::Seeking(at) => Some(at),
                _ => None,
            },
        )
    }

    /// Where the playhead settled. This is the one to start playing from.
    pub fn seeked(&self, actions: &Actions) -> Option<f64> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::Seeked(at) => Some(at),
                _ => None,
            },
        )
    }

    /// A region's ends while an edge of it is being dragged: which region,
    /// then start and end.
    pub fn region_moving(&self, actions: &Actions) -> Option<(usize, f64, f64)> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::RegionMoving(i, s, e) => Some((i, s, e)),
                _ => None,
            },
        )
    }

    /// The ends a region drag settled on.
    pub fn region_moved(&self, actions: &Actions) -> Option<(usize, f64, f64)> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::RegionMoved(i, s, e) => Some((i, s, e)),
                _ => None,
            },
        )
    }

    pub fn marker_moving(&self, actions: &Actions) -> Option<(usize, f64)> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::MarkerMoving(i, at) => Some((i, at)),
                _ => None,
            },
        )
    }

    pub fn marker_moved(&self, actions: &Actions) -> Option<(usize, f64)> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::MarkerMoved(i, at) => Some((i, at)),
                _ => None,
            },
        )
    }

    /// The mark that was pressed and let go without moving.
    pub fn marker_picked(&self, actions: &Actions) -> Option<usize> {
        scan(
            actions.filter_widget_actions_cast::<WaveformAction>(self.widget_uid()),
            |action| match action {
                WaveformAction::MarkerPicked(i) => Some(i),
                _ => None,
            },
        )
    }
}

// ---- the Widget impl ------------------------------------------------------

impl Widget for Waveform {
    /// The catalogue's Disabled control calls this and returns before the
    /// branch that would log an error (app.rs:219-230), and the trait
    /// defaults do nothing at all (widget.rs:338) — so without this pair the
    /// whole `disabled` animator track and every disabled mix in the shader
    /// would be dead and nothing would say so. Copied from
    /// range_slider.rs:597-603.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    /// The three a host reaches for first, reachable from the DSL.
    ///
    /// The lists are deliberately not callable: a region is five things, and
    /// a call taking five positional arguments would be a worse way of
    /// writing the line format markup already has.
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_playhead) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let at = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64();
                if let Some(at) = at {
                    vm.with_cx_mut(|cx| self.set_playhead(cx, at));
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(playhead) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.playhead));
        }
        if method == live_id!(duration) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.duration));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        // A disabled lane looks disabled and cannot be dragged. Same order
        // as range_slider.rs:631-635.
        if self.animator_in_state(cx, ids!(disabled.on)) {
            return;
        }
        let uid = self.uid;

        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.set_hot(cx, None);
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerHoverOver(fe) => {
                let lane = self.lane_of(fe.rect.size);
                let x = fe.abs.x - fe.rect.pos.x;
                let y = fe.abs.y - fe.rect.pos.y;
                let hot = lane.grab_at(x, y, &self.parsed_regions, &self.parsed_markers);
                self.set_hot(cx, hot);
                cx.set_cursor(match hot {
                    Some(WaveformPart::RegionStart(_)) | Some(WaveformPart::RegionEnd(_)) => {
                        MouseCursor::EwResize
                    }
                    Some(WaveformPart::Marker(_)) => MouseCursor::Grab,
                    _ => MouseCursor::Arrow,
                });
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                let lane = self.lane_of(fe.rect.size);
                let x = fe.abs.x - fe.rect.pos.x;
                let y = fe.abs.y - fe.rect.pos.y;
                let Some(part) = lane.grab_at(x, y, &self.parsed_regions, &self.parsed_markers)
                else {
                    // A press in the strip that landed on no chip. Nothing
                    // is grabbed and nothing is reported, which is what
                    // makes the band the marks' own.
                    return;
                };
                // Key focus is taken AFTER that guard and not before it.
                // Taking it first would make the press that the widget
                // documents as doing nothing pull focus off whatever had it
                // and play the focus track — which is a visible edit to the
                // rest of the window, and "nothing" has to mean nothing.
                cx.set_key_focus(self.area);
                self.grab = Some(part);
                self.press_x = fe.abs.x;
                self.animator_play(cx, ids!(drag.on));
                cx.widget_action(uid, WaveformAction::Grabbed(part));
                // The one pass that carries two actions: Grabbed first, then
                // where the press put the playhead.
                if let WaveformPart::Playhead = part {
                    self.playhead = lane.time_at(x);
                    cx.widget_action(uid, WaveformAction::Seeking(self.playhead));
                }
                self.area.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(part) = self.grab else {
                    return;
                };
                let lane = self.lane_of(fe.rect.size);
                let at = lane.time_at(fe.abs.x - fe.rect.pos.x);
                // The widget moves its own copy while the drag is on, so the
                // picture keeps up with the finger, and reports every step.
                // Waiting for the host to answer would be a lane a frame
                // behind the pointer.
                match part {
                    WaveformPart::Playhead => {
                        self.playhead = at;
                        cx.widget_action(uid, WaveformAction::Seeking(at));
                    }
                    WaveformPart::RegionStart(i) | WaveformPart::RegionEnd(i) => {
                        let Some(region) = self.parsed_regions.get_mut(i) else {
                            return;
                        };
                        if matches!(part, WaveformPart::RegionStart(_)) {
                            lane.move_start(region, at);
                        } else {
                            lane.move_end(region, at);
                        }
                        let (start, end) = (region.start, region.end);
                        cx.widget_action(uid, WaveformAction::RegionMoving(i, start, end));
                    }
                    WaveformPart::Marker(i) => {
                        let Some(marker) = self.parsed_markers.get_mut(i) else {
                            return;
                        };
                        marker.at = at;
                        cx.widget_action(uid, WaveformAction::MarkerMoving(i, at));
                    }
                }
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                let Some(part) = self.grab.take() else {
                    return;
                };
                self.animator_play(cx, ids!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                match part {
                    WaveformPart::Playhead => {
                        cx.widget_action(uid, WaveformAction::Seeked(self.playhead));
                    }
                    WaveformPart::RegionStart(i) | WaveformPart::RegionEnd(i) => {
                        if let Some(region) = self.parsed_regions.get(i) {
                            let (start, end) = (region.start, region.end);
                            cx.widget_action(uid, WaveformAction::RegionMoved(i, start, end));
                        }
                    }
                    WaveformPart::Marker(i) => {
                        // A pick and a move are told apart by how far the
                        // finger travelled, not by whether the number
                        // changed: on a long recording one point is a real
                        // number of seconds, so every click would otherwise
                        // be a tiny silent edit.
                        // A press taken away picks nothing.
                        if (fe.abs.x - self.press_x).abs() <= PICK_SLOP {
                            if !fe.cancelled {
                                cx.widget_action(uid, WaveformAction::MarkerPicked(i));
                            }
                        } else if let Some(marker) = self.parsed_markers.get(i) {
                            let at = marker.at;
                            cx.widget_action(uid, WaveformAction::MarkerMoved(i, at));
                        }
                    }
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
            }
            // The keyboard gets the playhead and nothing else, and a key
            // press is settled the moment it happens — so it reports
            // `Seeked` and never `Seeking`.
            Hit::KeyDown(ke) => {
                let d = self.key_step();
                let moved = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => Some(self.playhead - d),
                    KeyCode::ArrowRight | KeyCode::ArrowUp => Some(self.playhead + d),
                    KeyCode::Home => Some(0.0),
                    KeyCode::End => Some(self.duration),
                    _ => None,
                };
                if let Some(at) = moved {
                    self.set_playhead(cx, at);
                    cx.widget_action(uid, WaveformAction::Seeked(self.playhead));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Markup wins whenever its list differs from what was last read, so
        // a live reload and a control that rewrites `regions` both land
        // without a hook to catch them. `set_regions` clears both lists
        // together, which is what stops the next draw taking them back.
        if self.region_lines != self.regions {
            self.region_lines = self.regions.clone();
            self.parsed_regions = parse_regions(&self.regions);
        }
        if self.marker_lines != self.markers {
            self.marker_lines = self.markers.clone();
            self.parsed_markers = parse_markers(&self.markers);
        }

        // One walked rect, five things drawn absolutely into it. The rect
        // has to be known before anything is drawn, because how many texture
        // columns the picture reduces into depends on its device width.
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let lane = self.lane_of(rect.size);
        let body_top = lane.body_top();

        // Everything below is clipped into the lane's own rect. Two things
        // are drawn centred on a point rather than inside a box — a chip on
        // its mark, and the playhead quad, which is `max(2 * halo, 2)` points
        // wide so the shader's halo is a real distance inside it — so a mark
        // at 0 and a playhead at 0 both reach several points to the left of
        // the lane. Unclipped that is a halo painted over whatever the lane is
        // standing next to. Clipping rather than clamping, because clamping
        // would move the chip and the head off the times they name, and where
        // the head is is the one thing this widget must not round.
        cx.push_clip_rect(rect);

        let width_px = rect.size.x * cx.current_dpi_factor();
        let cols = self.refresh_picture(cx, width_px);
        self.draw_lane.cols = cols as f32;
        self.draw_lane.covered = self.covered() as f32;
        self.draw_lane.head = lane.travel(self.playhead) as f32;
        self.draw_lane.head_on = if self.show_playhead { 1.0 } else { 0.0 };
        self.draw_lane.strip_px = self.marker_strip as f32;
        self.draw_lane.draw_vars.set_texture(0, &self.texture);
        self.draw_lane.draw_abs(cx, rect);

        // The regions: a wash and a bracket apiece, in the order they
        // arrived with the last one on top. Overlapping regions are a real
        // thing — a verse inside a section — and a widget that refused them
        // would be wrong more often than it was right.
        let regions = std::mem::take(&mut self.parsed_regions);
        for (i, region) in regions.iter().enumerate() {
            if region.end < 0.0 || region.start > lane.duration {
                continue;
            }
            let x0 = rect.pos.x + lane.x_of(region.start);
            let x1 = rect.pos.x + lane.x_of(region.end);
            let least = self.draw_region.edge_size as f64 * 2.0;
            self.draw_region.color = self.tint(region.intent, region.color);
            self.draw_region.hot_start = match self.hot {
                Some(WaveformPart::RegionStart(h)) if h == i => 1.0,
                _ => 0.0,
            };
            self.draw_region.hot_end = match self.hot {
                Some(WaveformPart::RegionEnd(h)) if h == i => 1.0,
                _ => 0.0,
            };
            self.draw_region.strip_px = self.marker_strip as f32;
            self.draw_region.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x0, rect.pos.y),
                    size: dvec2((x1 - x0).max(least), rect.size.y),
                },
            );
        }
        self.parsed_regions = regions;

        // The marks. A band of nothing means no marks at all: not drawn,
        // not hit-tested, because a mark with nowhere to be grabbed is a
        // mark with nowhere to be.
        if body_top > 0.0 {
            let markers = std::mem::take(&mut self.parsed_markers);
            for (i, marker) in markers.iter().enumerate() {
                if !lane.holds(marker.at) {
                    continue;
                }
                let x = rect.pos.x + lane.x_of(marker.at);
                self.draw_marker.color = self.tint(marker.intent, marker.color);
                self.draw_marker.hot = match self.hot {
                    Some(WaveformPart::Marker(h)) if h == i => 1.0,
                    _ => 0.0,
                };
                self.draw_marker.strip_px = self.marker_strip as f32;
                self.draw_marker.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x - self.chip_width * 0.5, rect.pos.y),
                        size: dvec2(self.chip_width, rect.size.y),
                    },
                );
            }
            self.parsed_markers = markers;
        }

        if self.show_names {
            self.draw_names_of_regions(cx, rect, &lane);
            if body_top > 0.0 {
                self.draw_names_of_markers(cx, rect, &lane);
            }
        }

        // Last, because the playhead has to be findable over a loud passage
        // inside a coloured region. The quad is wide enough for the shader's
        // halo to be a real distance inside it: in a one-point quad the
        // distance from the centre never passes half a point and both terms
        // saturate into a flat bar.
        if self.show_playhead {
            let w = (self.draw_head.halo as f64 * 2.0).max(2.0);
            let x = rect.pos.x + lane.x_of(self.playhead);
            self.draw_head.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x - w * 0.5, rect.pos.y),
                    size: dvec2(w, rect.size.y),
                },
            );
        }

        cx.pop_clip_rect();

        if !self.animator_in_state(cx, ids!(disabled.on)) {
            cx.add_nav_stop(self.area, NavRole::Slider, Inset::default());
        }
        DrawStep::done()
    }

    /// What is on the lane, in one line, for a test that wants to read it
    /// without a screenshot: the region names then the mark names.
    fn text(&self) -> String {
        self.parsed_regions
            .iter()
            .map(|r| r.name.clone())
            .chain(self.parsed_markers.iter().map(|m| m.name.clone()))
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lane with the geometry the preset gives it, ninety seconds long
    /// and a thousand points wide.
    fn lane() -> Lane {
        Lane {
            duration: 90.0,
            step: 0.0,
            min_span: 0.0,
            width: 1000.0,
            height: 96.0,
            strip: 16.0,
            edge_grab: 6.0,
            marker_grab: 7.0,
        }
    }

    /// The `script_mod!` block is invisible to the Rust compiler: a mistake
    /// in it shows up only in a running app's log, and a shader that fails
    /// to compile is not an error anywhere — the draw is simply skipped and
    /// the widget paints nothing. Evaluating the crate's whole script
    /// module here, building one lane from the preset and reading both the
    /// numbers and the shader-error slot back is what turns either into a
    /// failed build.
    #[test]
    fn the_preset_reaches_the_widget_and_its_four_shaders_compile() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let wave = cx.with_vm(|vm| {
            crate::script_mod(vm);
            // Registering type defaults compiles nothing; making an
            // instance out of one does. Clearing here keeps any other
            // module's complaint out of this test's answer.
            let _ = crate::makepad_draw::makepad_platform::shader_error::take();
            Waveform::script_new_with_default(vm)
        });
        assert_eq!(
            crate::makepad_draw::makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        // Numbers only the DSL sets: the Rust defaults are Fit and nothing.
        assert_eq!(wave.walk.height.to_fixed(), Some(96.0), "the preset's height");
        assert_eq!(wave.marker_strip, 16.0);
        assert_eq!(wave.chip_width, 9.0);
        assert!(wave.show_playhead && wave.show_names);
        // And the shared palette came through, so an intent resolves to a
        // colour rather than to nothing at all.
        let palette = &wave.palette;
        assert_ne!(
            palette.family(BadgeIntent::Error).base,
            palette.family(BadgeIntent::Success).base,
            "the intents came out of the theme, not out of zero"
        );
    }

    #[test]
    fn shortening_the_axis_brings_the_playhead_back_onto_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut wave = cx.with_vm(|vm| {
            crate::script_mod(vm);
            Waveform::script_new_with_default(vm)
        });
        wave.set_duration(&mut cx, 90.0);
        wave.set_playhead(&mut cx, 80.0);
        assert_eq!(wave.playhead(), 80.0);
        // Ten seconds of axis cannot hold a playhead at eighty. Every other
        // path holds it inside the axis, so this one has to as well, or
        // `playhead()` answers with a number the lane cannot draw and the
        // first arrow key jumps the value from eighty to ten.
        wave.set_duration(&mut cx, 10.0);
        assert_eq!(wave.playhead(), 10.0);
        // A longer axis leaves it alone: nothing is off the lane.
        wave.set_duration(&mut cx, 120.0);
        assert_eq!(wave.playhead(), 10.0);
    }

    #[test]
    fn a_region_line_reads_its_fields_in_the_order_they_are_written() {
        // The name and the tint are the pair that was swapped once: the
        // tint has to be read after the name, so a line carrying both comes
        // back with both.
        let region = parse_region("12 | 30 | Build | primary");
        assert_eq!(region.start, 12.0);
        assert_eq!(region.end, 30.0);
        assert_eq!(region.name, "Build");
        assert_eq!(region.intent, BadgeIntent::Primary);
        assert_eq!(region.color, None);

        // And a name with no tint after it is still a name.
        let plain = parse_region("1 | 2 | Intro");
        assert_eq!(plain.name, "Intro");
        assert_eq!(plain.intent, BadgeIntent::Neutral);

        // The escape hatch, written without the `x` a colour needs in DSL
        // source, because there is no tokenizer inside a string.
        let hexed = parse_region("3 | 4 | Tail | #4f9d69");
        assert_eq!(hexed.name, "Tail");
        assert!(hexed.color.is_some());

        // A span written backwards is the same span.
        let backwards = parse_region("30 | 12 | Build");
        assert_eq!((backwards.start, backwards.end), (12.0, 30.0));
    }

    #[test]
    fn a_tint_word_that_is_not_an_intent_stays_neutral_even_when_it_is_hex() {
        // `parse_hex_color` treats the '#' as optional (color.rs:139) and
        // takes any run of three, six or eight hex digits, so without the
        // guard in `read_tint` every word below came back as a colour
        // instead of as the typo it is. Each of these is a legal length.
        for word in ["bad", "ace", "beaded", "decade", "facade", "deadbeef"] {
            let region = parse_region(&format!("1 | 2 | Name | {word}"));
            assert_eq!(region.intent, BadgeIntent::Neutral, "{word} is not an intent word");
            assert_eq!(region.color, None, "and it is not a colour either");
        }
        // The documented form still is one, and the DSL spelling still is
        // not — the `x` that a colour needs in source has no tokenizer to
        // protect it from inside a string.
        assert!(parse_region("1 | 2 | Name | #4f9d69").color.is_some());
        assert_eq!(parse_region("1 | 2 | Name | #x4f9d69").color, None);
    }

    #[test]
    fn a_marker_line_reads_its_three_fields_in_order() {
        let marker = parse_marker("30 | Drop | error");
        assert_eq!(marker.at, 30.0);
        assert_eq!(marker.name, "Drop");
        assert_eq!(marker.intent, BadgeIntent::Error);
        // A clock time is not a number, and reads as zero rather than
        // throwing the line away.
        assert_eq!(parse_marker("1:30 | Late").at, 0.0);
    }

    #[test]
    fn pack_unpack_is_exact_on_the_grid_and_within_half_a_step_off_it() {
        // The packing quantizes to 65281 levels, so only a value already on
        // that grid survives a round trip unchanged. The ends and the
        // halves are on it; an arbitrary value is not, and comes back
        // within half a step.
        for v in [-1.0f32, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0] {
            let (lo, hi) = unpack_column(pack_column((v, v)));
            assert_eq!(lo, v, "the low of {v} came back as {lo}");
            assert_eq!(hi, v, "the high of {v} came back as {hi}");
        }
        // A step in value space is 2/TEXEL_FULL, so half a step — what the
        // round in `pack_value` can be out by — is 1/TEXEL_FULL. That is the
        // number this asserts, not twice it: the name says half a step and
        // the slack has to mean it, or the test would still pass with the
        // round replaced by a truncation.
        let half_step = 1.0 / TEXEL_FULL;
        for v in [-0.9137f32, -0.123, 0.3, 0.6667, 0.98765] {
            let (lo, hi) = unpack_column(pack_column((v, v)));
            assert!((lo - v).abs() <= half_step, "{v} came back as {lo}");
            assert!((hi - v).abs() <= half_step, "{v} came back as {hi}");
        }
    }

    /// A PACKING test, and named as one. The silence floor itself lives in
    /// the lane's shader and no Rust here can execute it; what pins that is
    /// `test_the_silence_floor_widens_about_the_columns_own_midpoint` below,
    /// which reads the expression out of the source.
    #[test]
    fn an_off_centre_column_packs_with_both_ends_above_the_line() {
        // Both ends above the centre line: the picture the mirrored setter
        // cannot show, and the reason the low and the high are packed
        // separately.
        let (lo, hi) = unpack_column(pack_column((0.2, 0.6)));
        assert!(lo > 0.0, "the low is still above the centre line, at {lo}");
        assert!(hi > lo);
        assert!((lo - 0.2).abs() <= 1.0 / TEXEL_FULL);
        assert!((hi - 0.6).abs() <= 1.0 / TEXEL_FULL);
    }

    #[test]
    fn the_reduction_keeps_the_extremes_rather_than_averaging_them() {
        // One loud column in a hundred quiet ones survives into whichever
        // texel covers it: an average would bury it, and the transient is
        // the part somebody opened this to look at.
        let mut pairs = vec![(-0.02f32, 0.02f32); 400];
        pairs[137] = (-0.9, 0.8);
        let row = reduce(&Peaks::Pairs(pairs), 40);
        assert_eq!(row.len(), 40);
        let (lo, hi) = unpack_column(row[13]);
        assert!(lo < -0.85, "the low of the loud texel is {lo}");
        assert!(hi > 0.75, "the high of the loud texel is {hi}");
        // And a quiet texel is still quiet.
        let (qlo, qhi) = unpack_column(row[0]);
        assert!(qlo > -0.05 && qhi < 0.05);
    }

    #[test]
    fn fewer_peaks_than_columns_draws_a_stair_and_reads_nothing_past_the_data() {
        let row = reduce(&Peaks::Magnitudes(vec![0.25, 0.5, 1.0]), 9);
        assert_eq!(row.len(), 9);
        let mags: Vec<f32> = row.iter().map(|t| unpack_column(*t).1).collect();
        assert_eq!(mags[0], mags[1]);
        assert_eq!(mags[2], mags[1]);
        assert!(mags[8] > mags[0]);
    }

    #[test]
    fn a_press_in_the_strip_on_nothing_is_nothing_and_below_it_is_a_seek() {
        let lane = lane();
        let marks = vec![WaveformMarker::new(10.0, "In")];
        // In the band, nowhere near the one chip.
        assert_eq!(lane.grab_at(600.0, 4.0, &[], &marks), None);
        // On the chip.
        assert_eq!(
            lane.grab_at(lane.x_of(10.0), 4.0, &[], &marks),
            Some(WaveformPart::Marker(0))
        );
        // Under the band, on nothing.
        assert_eq!(lane.grab_at(600.0, 50.0, &[], &marks), Some(WaveformPart::Playhead));
        // And a chip is not grabbable from the body, however close.
        assert_eq!(
            lane.grab_at(lane.x_of(10.0), 50.0, &[], &marks),
            Some(WaveformPart::Playhead)
        );
    }

    #[test]
    fn a_band_of_nothing_takes_the_marks_with_it() {
        let mut lane = lane();
        lane.strip = 0.0;
        let marks = vec![WaveformMarker::new(10.0, "In")];
        assert_eq!(lane.body_top(), 0.0);
        // Every y is body now, chip or no chip.
        assert_eq!(
            lane.grab_at(lane.x_of(10.0), 0.0, &[], &marks),
            Some(WaveformPart::Playhead)
        );
    }

    #[test]
    fn a_band_taller_than_the_lane_stops_at_the_lane() {
        // The three shaders clamp `strip_px` into the lane's own height, and
        // the hit test has to land in the same place at every value or a
        // chip is painted where nothing can be grabbed.
        let mut lane = lane();
        lane.strip = 400.0;
        assert_eq!(lane.body_top(), lane.height);
        let marks = vec![WaveformMarker::new(10.0, "In")];
        assert_eq!(
            lane.grab_at(lane.x_of(10.0), 95.0, &[], &marks),
            Some(WaveformPart::Marker(0))
        );
    }

    #[test]
    fn two_chips_two_reaches_apart_each_take_their_own_press() {
        let lane = lane();
        // Two reaches is fourteen points, which at this width is about one
        // and a quarter seconds.
        let apart = lane.reach(lane.marker_grab * 2.0);
        let marks = vec![
            WaveformMarker::new(30.0, "Drop"),
            WaveformMarker::new(30.0 + apart, "Fill"),
        ];
        assert_eq!(
            lane.grab_at(lane.x_of(30.0), 4.0, &[], &marks),
            Some(WaveformPart::Marker(0))
        );
        assert_eq!(
            lane.grab_at(lane.x_of(30.0 + apart), 4.0, &[], &marks),
            Some(WaveformPart::Marker(1))
        );
    }

    #[test]
    fn a_dragged_edge_stops_short_rather_than_shoving_the_other() {
        let mut lane = lane();
        lane.min_span = 5.0;
        let mut region = WaveformRegion::new(12.0, 30.0, "Build");
        lane.move_start(&mut region, 80.0);
        assert_eq!((region.start, region.end), (25.0, 30.0), "the end stayed put");
        lane.move_end(&mut region, 0.0);
        assert_eq!((region.start, region.end), (25.0, 30.0), "and so did the start");
    }

    #[test]
    fn a_step_quantizes_a_drag_and_the_axis_holds_it() {
        let mut lane = lane();
        lane.step = 5.0;
        assert_eq!(lane.time_at(lane.x_of(12.0)), 10.0);
        assert_eq!(lane.time_at(lane.x_of(13.0)), 15.0);
        // Off either end of the lane is still on the axis.
        assert_eq!(lane.time_at(-40.0), 0.0);
        assert_eq!(lane.time_at(4000.0), 90.0);
    }

    #[test]
    fn a_zero_length_axis_reports_travel_zero_rather_than_dividing_by_it() {
        let mut lane = lane();
        lane.duration = 0.0;
        assert_eq!(lane.travel(5.0), 0.0);
        assert_eq!(lane.x_of(5.0), 0.0);
    }

    /// What `scan` does with a two-action pass, and nothing more than that:
    /// the pass here is built by hand, so this cannot say whether
    /// `handle_event` still emits both. That claim is pinned by
    /// `test_a_body_press_emits_the_grab_and_then_the_seek` below, which
    /// reads the arm out of the source.
    #[test]
    fn scan_finds_either_action_in_a_two_action_pass() {
        // The pass a press on the body puts out, in the order it puts it
        // out. A reader built on `find_widget_action` takes the first and
        // stops, which is how every reader here was once blind to one of
        // the pair.
        let pass = [
            WaveformAction::Grabbed(WaveformPart::Playhead),
            WaveformAction::Seeking(12.5),
        ];
        let grabbed = scan(pass.iter().cloned(), |a| match a {
            WaveformAction::Grabbed(p) => Some(p),
            _ => None,
        });
        let seeking = scan(pass.iter().cloned(), |a| match a {
            WaveformAction::Seeking(at) => Some(at),
            _ => None,
        });
        assert_eq!(grabbed, Some(WaveformPart::Playhead));
        assert_eq!(seeking, Some(12.5));
        // The first action alone answers only one of the two questions.
        assert!(!matches!(pass[0], WaveformAction::Seeking(_)));
    }

    #[test]
    fn equal_peaks_handed_over_twice_change_nothing() {
        let first = settle_pairs(&Peaks::None, vec![(-0.2, 0.4), (-0.1, 0.1)]);
        let held = first.expect("the first array is a change");
        assert_eq!(settle_pairs(&held, vec![(-0.2, 0.4), (-0.1, 0.1)]), None);
        // A column written high-then-low is the same column.
        assert_eq!(settle_pairs(&held, vec![(0.4, -0.2), (0.1, -0.1)]), None);
        // A different array is a change, and it is moved in.
        assert!(settle_pairs(&held, vec![(-0.2, 0.5), (-0.1, 0.1)]).is_some());

        let mags = settle_magnitudes(&Peaks::None, vec![0.1, 0.2]).expect("a change");
        assert_eq!(settle_magnitudes(&mags, vec![0.1, 0.2]), None);
        // The two shapes are different holdings, even with the same numbers.
        assert!(settle_pairs(&mags, vec![(-0.1, 0.1)]).is_some());
    }
}

#[cfg(test)]
mod waveform_registration_tests {
    // This file reads ITSELF, so a needle written as one literal would match
    // its own source and the assertion would be vacuous — or, for a count,
    // wrong by one. Every needle below is joined at runtime out of pieces
    // that never sit next to each other in the text being searched.

    /// The lane registers after the bases it draws with and after the badge
    /// module whose palette it resolves intents through, with one type
    /// default on the canonical preset and none on the variant.
    #[test]
    fn test_waveform_is_registered_after_its_bases() {
        // The family registers after every widget of the core, so its bases
        // only have to be the core's; the prelude re-export is the front's.
        let lib = include_str!("lib.rs");
        let core = include_str!("../../../core/src/lib.rs");
        let front = include_str!("../../../src/lib.rs");
        assert!(lib.contains("pub mod waveform;"));
        assert!(front.contains("waveform::*"));
        assert!(lib.contains("crate::waveform::script_mod(vm);"), "registered");
        let core_calls = &core[core.find("pub fn widgets_mod_with(").expect("widgets_mod_with")..];
        for base in [
            "crate::view::script_mod(vm);",
            "crate::label::script_mod(vm);",
            "crate::badge::script_mod(vm);",
        ] {
            assert!(core_calls.contains(base), "{base} must register before waveform");
        }
    }

    #[test]
    fn test_the_preset_is_the_type_default_and_the_variant_is_not() {
        let wave = include_str!("waveform.rs");
        let base = format!("mod.widgets.{} = #({}::register_widget(vm))", "WaveformBase", "Waveform");
        assert!(wave.contains(&base));
        let preset = format!("mod.widgets.{} = set_type_default() do mod.widgets.{}{{", "Waveform", "WaveformBase");
        assert!(wave.contains(&preset));
        let default_on_base = format!("set_type_default() do mod.widgets.{}", "WaveformBase");
        assert_eq!(wave.matches(default_on_base.as_str()).count(), 1);
        // The variant is a plain derivation. A second set_type_default on
        // the same Rust type replaces the first, so the transport preset
        // would become the default for every Waveform.
        let variant = format!("mod.widgets.{} = mod.widgets.{}{{", "WaveformStrip", "Waveform");
        assert!(wave.contains(&variant));
        let default_on_preset = format!("set_type_default() do mod.widgets.{}{{", "Waveform");
        assert_eq!(wave.matches(default_on_preset.as_str()).count(), 0);
    }

    #[test]
    fn test_all_three_shaders_take_the_body_top_from_one_expression() {
        // The lane, the region and the marker must agree about where the
        // marks' band ends at EVERY value of it, including one taller than
        // the lane, or a region paints through the chips or a chip paints
        // over the peaks. The only way to keep three shaders honest from a
        // test is to check they are written the same.
        let wave = include_str!("waveform.rs");
        let split = format!("clamp(self.strip_px, 0.0, self.{}.y)", "rect_size");
        assert_eq!(
            wave.matches(split.as_str()).count(),
            3,
            "the lane, the region and the marker each derive the body top once, the same way"
        );
        // And Rust's hit test clamps the same number into the same range.
        let rust_split = format!("self.strip.clamp(0.0, self.{})", "height");
        assert!(wave.contains(&rust_split));
    }

    /// The one pass that carries two actions, read out of the arm that
    /// emits them.
    ///
    /// The pair is contract — the table above `WaveformAction` says so, and
    /// every reader on `WaveformRef` scans a pass instead of taking the
    /// first action because of it. A test that builds the pair by hand
    /// cannot tell whether the arm still emits it, so this reads the arm.
    /// Delete either `widget_action` line and it goes red.
    #[test]
    fn test_a_body_press_emits_the_grab_and_then_the_seek() {
        let wave = include_str!("waveform.rs");
        let arm = format!("{}{}", "Hit::FingerDown", "(fe) if fe.device.is_primary_hit()");
        let from = wave.find(&arm).expect("the press arm");
        let next = format!("{}{}", "Hit::FingerMove", "(fe) =>");
        let to = wave[from..].find(&next).expect("the arm after it") + from;
        let body = &wave[from..to];

        let grabbed = format!("{}{}", "WaveformAction::Grabbed", "(part)");
        let seeking = format!("{}{}", "WaveformAction::Seeking", "(self.playhead)");
        let g = body.find(&grabbed).expect("the press says what it took");
        let s = body.find(&seeking).expect("and where it put the playhead");
        assert!(g < s, "Grabbed is the first of the pair");

        // And key focus is taken AFTER the "took hold of nothing" guard, or
        // a press in the strip on no chip would pull focus off whatever had
        // it — which the module doc and the story's gesture table both say
        // is nothing.
        let asked = format!("{}{}", "lane.grab_at", "(x, y, &self.parsed_regions");
        let focus = format!("{}{}", "cx.set_key", "_focus(self.area)");
        let a = body.find(&asked).expect("the press asks what is under it");
        let f = body.find(&focus).expect("the press takes key focus");
        assert!(a < f, "focus is taken after the grab, not before it");
    }

    /// Every reader on the handle scans its pass. `find_widget_action`
    /// (widget.rs:1715-1724) answers with the FIRST action from a uid and
    /// stops, so a reader built on it would be blind to one half of the
    /// press pass above; the library ships the filtering iterator for
    /// exactly this and says so at widget.rs:1595-1601.
    #[test]
    fn test_every_reader_scans_the_pass_rather_than_taking_the_first() {
        let wave = include_str!("waveform.rs");
        let scanning = format!("{}{}", "filter_widget_actions", "_cast::<WaveformAction>");
        assert_eq!(
            wave.matches(scanning.as_str()).count(),
            8,
            "one scan apiece for the eight readers on WaveformRef"
        );
        let first_only = format!("{}{}", "find_widget_action", "(");
        assert_eq!(wave.matches(first_only.as_str()).count(), 0, "and none of the first-only helper");
    }

    /// The silence floor is widened about the column's OWN midpoint.
    ///
    /// Nothing in Rust runs the lane's shader, so the only way to hold this
    /// line is to read it. Widening toward the centre line instead would
    /// drag a signal that sits off centre back onto it, and showing that a
    /// signal sits off centre is the whole reason the low and the high are
    /// carried separately.
    #[test]
    fn test_the_silence_floor_widens_about_the_columns_own_midpoint() {
        let wave = include_str!("waveform.rs");
        let mid = format!("{}{}", "let mid = (top + bot)", " * 0.5");
        let half = format!("{}{}", "let half = max((top - bot) * 0.5,", " feather * 0.5)");
        let hi = format!("{}{}", "let hi = mid", " + half");
        let lo = format!("{}{}", "let lo = mid", " - half");
        assert!(wave.contains(&mid), "the midpoint is the column's own");
        assert!(wave.contains(&half), "and the floor is half a pixel of it");
        assert!(wave.contains(&hi) && wave.contains(&lo), "both ends move off that midpoint");
    }

    /// Key focus is shown with a ring, not with a tint of the ground.
    ///
    /// `color_inset_hover`, `color_inset_focus` and `color_inset_drag` are
    /// aliases of `color_inset` in both shipped desktop themes
    /// (theme_desktop_dark.rs:299-304, theme_desktop_light.rs:302-307), so a
    /// lane that showed focus by tinting its ground would be pixel-identical
    /// to one that has none — on a nav stop whose arrow keys edit a value.
    /// The ring is `color_primary` and the peaks take the `color_val`
    /// ladder, and both of those are three different colours in all three
    /// themes.
    #[test]
    fn test_focus_is_a_ring_and_the_peaks_take_the_ladder_that_steps() {
        let wave = include_str!("waveform.rs");
        let ring = format!("{}{}", "ring_color: uniform(theme.", "color_primary)");
        assert!(wave.contains(&ring), "the ring is the theme's primary");
        let stroked = format!(
            "{}{}",
            "self.ring_color_off.mix(self.ring_color,", " self.focus * (1.0 - self.disabled))"
        );
        assert!(wave.contains(&stroked), "and it is gated on focus, off when disabled");
        for token in ["color_val_hover)", "color_val_focus)", "color_val_drag)"] {
            let needle = format!("{}{}", "uniform(theme.", token);
            assert!(wave.contains(&needle), "the peaks need theme.{token}");
        }
    }
}
