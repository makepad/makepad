//! PlaybackBar — how far into something timed you are, and a press that
//! moves you.
//!
//! Six copies of this bar were written by hand in this repository before it
//! became a widget, and each one re-derived the same three things. They are
//! what the widget is for:
//!
//! * **The hit band is the whole row, not the track.** A track is four
//!   pixels; a pointer on the move cannot land on four pixels, and a finger
//!   never can. Everything below the clock is a press target, so the thing
//!   you aim at is twenty-odd pixels tall while the thing you see stays
//!   thin.
//! * **The host is ignored while the finger is down.** A player keeps
//!   reporting where it is actually playing, which during a drag is
//!   somewhere behind the finger. Letting those reports through makes the
//!   playhead flick between the two. The gesture owns the bar until it
//!   lets go, and for a moment after, because a seek takes time to land and
//!   the host reports the OLD position until it does.
//! * **A seek is coalesced to its newest target.** A drag produces a
//!   position per pointer move; a host that honours every one of them
//!   decodes hundreds of frames nobody will ever see. Only the latest target
//!   is worth anything, so that is the only one kept, and it goes out at
//!   most once per `seek_interval`.
//!
//! # Why not the slider
//!
//! A slider's drag is RELATIVE — a press at seven tenths of the track moves
//! by seven tenths of nothing, it does not go to seven tenths — and its
//! double tap resets to whatever the source said, which on a playhead means
//! jumping wherever the script happened to write. A playhead is absolute in
//! both gestures: where you press is where you go. The progress bar is the
//! other half of the family and refuses input on purpose; this is the one
//! that takes it.
//!
//! # The clock
//!
//! `format_clock` is a free function beside the widget because a caption, a
//! list row and a tooltip all want the same string and none of them own a
//! bar. It floors rather than rounds: a clock reading 1:00 while fifty-nine
//! and a half seconds have played is claiming time that has not passed.
//! `fraction_at` is the other free function, and it is the line every copy
//! wrote out again.
//!
//! # What it will not do
//!
//! It does not hide itself, it does not play anything, and it does not
//! answer the wheel — a bar inside a scrolling page that seeked on scroll
//! would throw the playhead across the room every time the page moved.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PlaybackBarBase = #(PlaybackBar::register_widget(vm))

    set_type_default() do #(DrawPlaybackBar::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A track showing how far into something timed you are; a press or a drag anywhere on it moves you to that point. */
    mod.widgets.PlaybackBarFlat = set_type_default() do mod.widgets.PlaybackBarBase{
        width: Fill
        height: 40
        margin: theme.mspace_1

        /** the whole length in seconds; 0 means unknown, and then nothing seeks */
        duration: 0.0
        /** where the playhead is, in seconds */
        position: 0.0
        /** an arrow key moves this many seconds 0.5..60 step 0.5 */
        step_secs: 5.0
        /** least seconds between two seeks while a drag runs; 0 reports every move 0..1 step 0.01 */
        seek_interval: 0.12
        /** how long the bar keeps the finger's answer after a release, in seconds 0..5 step 0.1 */
        settle_secs: 1.0
        /** how near the host must land to count as having honoured the seek, in seconds 0..5 step 0.05 */
        settle_tolerance: 0.75
        /** room above the track for the clock, in pixels 0..40 step 1 */
        label_height: 16.
        /** track inset from the left and right edges, in pixels 0..40 step 1 */
        track_inset: 10.
        /** draw the clock */
        show_clock: true
        /** the right-hand clock counts down instead of naming the whole length */
        show_remaining: false

        draw_text +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            color: theme.color_label_outer
            color_hover: uniform(theme.color_label_outer_hover)
            color_drag: uniform(theme.color_label_outer_drag)
            color_disabled: uniform(theme.color_label_outer_disabled)

            text_style: theme.font_regular{font_size: theme.font_size_p}

            // DrawText's own get_color hands back `self.color` and nothing
            // else, so without this override the three instances above and
            // the animator applies that drive them are inert and the clock
            // stays one colour through every state, disabled included.
            get_color: fn() {
                return self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_drag, self.drag)
                    .mix(self.color_disabled, self.disabled)
            }
        }

        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** track thickness in pixels 1..24 step 0.5 */
            thickness: uniform(4.0)
            /** track thickness once the pointer is on it 1..24 step 0.5 */
            thickness_hover: uniform(7.0)
            /** knob diameter in pixels 0..32 step 0.5 */
            knob_size: uniform(11.0)
            /** knob diameter once the pointer is on it 0..32 step 0.5 */
            knob_size_hover: uniform(15.0)
            /** clear ring around the knob in pixels 0..6 step 0.5 */
            knob_ring: uniform(2.0)
            /** corner rounding; the full radius makes a pill 0..24 step 0.5 */
            border_radius: uniform(theme.radius_full)
            /** bevel thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)

            color_track: uniform(theme.color_surface_container_highest)
            color_elapsed: uniform(theme.color_primary)
            color_knob: uniform(theme.color_text)
            color_knob_ring: uniform(theme.color_surface)
            /** bevel stroke on the track; a zero alpha draws none */
            border_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let top = self.label_px
                let band = max(self.rect_size.y - top, 1.0)
                let mid = top + band * 0.5
                // Focus lifts the bar the same way the pointer does, so a
                // keyboard user can see which control the arrows will move.
                let lift = max(max(self.hover, self.drag), self.focus)

                let h = min(mix(self.thickness, self.thickness_hover, lift), band)
                let r = min(self.border_radius, h * 0.5)
                let x0 = self.inset_px
                let w = max(self.rect_size.x - self.inset_px * 2.0, 1.0)
                let y0 = mid - h * 0.5

                sdf.box(x0, y0, w, h, r)
                if self.border_color.w > 0.0 {
                    // fill_KEEP only where a stroke follows to consume the
                    // shape; a kept shape unions with the next box.
                    sdf.fill_keep(self.color_track)
                    sdf.stroke(self.border_color, self.border_size)
                } else {
                    sdf.fill(self.color_track)
                }

                // Disabled, the fill walks halfway to the track so the bar
                // reads as muted. It is NOT what carries the answer: this
                // pair is the theme's own progress pairing (primary on the
                // highest surface container) and even at full strength it
                // is 1.76:1 in the dark theme, so no amount of undimming
                // would reach the 3:1 a graphical object wants. The knob
                // below is the mark that has to survive, and does.
                let played = mix(self.color_elapsed, self.color_track, 0.5 * self.disabled)
                let fw = w * clamp(self.pos_frac, 0.0, 1.0)
                if fw > 0.25 {
                    // Never thinner than it is tall, or the first second
                    // of an hour reads as a smear instead of a dot.
                    sdf.box(x0, y0, min(max(fw, h), w), h, r)
                    sdf.fill(played)
                }

                let grip = mix(self.knob_size, self.knob_size_hover, lift) * 0.5
                // Disabled it stops being a handle and becomes a mark: it
                // keeps both colours — knob on ring is 4.7:1 in the dark
                // theme, 5.1:1 in the light and 6.6:1 in the skeleton, the
                // one pair on this bar that clears 3:1 everywhere — and
                // shrinks to the track's own thickness, so a bar nobody can
                // move still says where the playhead is without offering a
                // grip that would not answer. Three points across is the
                // floor, or a hairline track would leave no mark at all.
                let kr = mix(grip, min(grip, max(h, 3.0) * 0.5), self.disabled)
                if kr > 0.5 {
                    // The knob rides on a ring of the page's own ground.
                    // Without it the knob would have to out-contrast both
                    // halves of the track at once, and no single theme
                    // token manages that in all three themes.
                    //
                    // Its centre is the fill's leading edge and nothing
                    // else: `fw` is already clamped to 0..w, so the centre
                    // is on the track by construction and there is no
                    // second clamp to hold it in — one that pinned the knob
                    // half its width inside each end would draw it where
                    // `fraction_at` does not map it back, and the bar would
                    // not move at all through the first half-knob of the
                    // piece. `track_inset` is what buys the room instead:
                    // it is sized so the ring clears the widget's edge at
                    // both ends of the travel.
                    let kx = x0 + fw
                    sdf.circle(kx, mid, kr + self.knob_ring)
                    sdf.fill(self.color_knob_ring)
                    sdf.circle(kx, mid, kr)
                    sdf.fill(self.color_knob)
                }
                return sdf.result
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {hover: 0.0} draw_text: {hover: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
                }
            }
            drag: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {drag: 0.0} draw_text: {drag: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {drag: 1.0} draw_text: {drag: 1.0}}
                }
            }
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {disabled: 0.0} draw_text: {disabled: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {disabled: 1.0} draw_text: {disabled: 1.0}}
                }
            }
        }
    }

    /** The standard playback bar: the flat face plus the theme's inset bevel on the track. */
    mod.widgets.PlaybackBar = set_type_default() do mod.widgets.PlaybackBarFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
        }
    }
}

/// A duration as a clock: `M:SS`, growing an hours field once it reaches one.
///
/// Seconds are FLOORED. A clock that reads 1:00 while fifty-nine and a half
/// seconds have played is claiming time that has not passed, and a person
/// reading a position off a bar is usually trying to write it down. Anything
/// negative or not a number reads as the start, because a playhead has to
/// draw something.
pub fn format_clock(seconds: f64) -> String {
    clock(seconds, seconds >= 3600.0)
}

/// The clock, told whether to carry an hours field whatever its own value
/// says. A position inside an hour-long piece keeps the shape of the length
/// beside it, so the two do not change width against each other as it plays.
fn clock(seconds: f64, with_hours: bool) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 { seconds.floor() as u64 } else { 0 };
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if with_hours || h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Where a press at `x` falls along a track starting at `track_x` and
/// `track_w` wide, as a fraction 0..1.
///
/// Absolute, not relative: the answer is where the finger IS, not how far it
/// has travelled since it went down. That one difference is why a slider
/// cannot serve as a playhead, and this line is the one every hand-written
/// copy wrote out again. A track with no width answers the start rather than
/// dividing by it.
pub fn fraction_at(x: f64, track_x: f64, track_w: f64) -> f64 {
    if !track_w.is_finite() || track_w <= 0.0 {
        return 0.0;
    }
    ((x - track_x) / track_w).clamp(0.0, 1.0)
}

/// How far through `duration` a position of `position` seconds is. A piece
/// whose length nobody has said is at its start: there is no fraction of an
/// unknown quantity to show.
fn fraction_of(position: f64, duration: f64) -> f64 {
    if !duration.is_finite() || duration <= 0.0 || !position.is_finite() {
        return 0.0;
    }
    (position / duration).clamp(0.0, 1.0)
}

/// Whether a press `y` points down from the top of the widget is a seek.
///
/// Everything below the clock row is, and that is the point: the track is a
/// few pixels thick and nobody hits a few pixels while moving. The clock row
/// itself is not, so pressing the total does not throw the playhead to the
/// end of the piece.
fn seeks_at(y: f64, label_room: f64) -> bool {
    y >= label_room.max(0.0)
}

/// The room actually kept above the track.
///
/// A bar drawing no clock keeps none. `label_height` reserves the strip AND
/// cuts it out of the press band, so a bar that only stopped DRAWING the
/// clock would carry a dead strip along its top that refused presses with
/// nothing on it to explain why — and turning the clock off would have to be
/// done twice, once for the text and once for the room, to get the bar the
/// one toggle plainly asks for.
fn label_room(show_clock: bool, label_height: f64) -> f64 {
    if show_clock {
        label_height.max(0.0)
    } else {
        0.0
    }
}

/// Holds the newest seek target and decides when it may go out.
///
/// A drag produces a target per pointer move. Every one but the last is
/// already overtaken by the time a decoder could act on it, so only the last
/// is kept, and it is released at most once per interval — the cost of a
/// drag is then bounded by how long it lasts rather than by how fast the
/// pointer moves.
#[derive(Default)]
struct SeekGate {
    /// A target nobody has been told about yet.
    pending: Option<f64>,
    /// The last target that went out, so an unchanged one is not repeated.
    sent: Option<f64>,
    /// When it went out, on the frame clock.
    sent_at: f64,
}

impl SeekGate {
    fn want(&mut self, fraction: f64) {
        self.pending = Some(fraction);
    }

    /// The target to report now, if one is due.
    ///
    /// The first target of a gesture goes out at once: a press that waited
    /// out the interval before anything moved would read as a dropped click.
    fn due(&mut self, now: f64, interval: f64) -> Option<f64> {
        let target = self.pending?;
        if self.sent.is_some() && now - self.sent_at < interval {
            return None;
        }
        self.pending = None;
        if self.sent == Some(target) {
            return None;
        }
        self.sent = Some(target);
        self.sent_at = now;
        Some(target)
    }

    /// The target the gesture settled on, whatever the clock says. Nothing
    /// when the host has already been sent exactly this, which is the common
    /// case for a drag that ends where its last reported move was.
    fn flush(&mut self) -> Option<f64> {
        let target = self.pending.take()?;
        if self.sent == Some(target) {
            return None;
        }
        self.sent = Some(target);
        Some(target)
    }

    /// Forget the last gesture, so the next one's first target is not held
    /// back by an interval that belongs to a press from a minute ago.
    fn rest(&mut self) {
        self.pending = None;
        self.sent = None;
    }
}

/// The gesture's claim on what the bar shows.
///
/// While a finger is down the host's reported position is worthless — it is
/// still playing from where it was. After the finger lifts the claim lasts a
/// little longer, because a seek takes time to land and until it does the
/// host keeps reporting the old position; without the wait the playhead
/// snaps back to where it came from and then jumps forward again.
struct Hold {
    /// What the gesture put on screen, along the track.
    fraction: f64,
    /// The same point in seconds, so the host's reports can be compared
    /// against it in the units the host speaks.
    seconds: f64,
    /// The finger is still down.
    down: bool,
    /// When it lifted, on the frame clock.
    released: f64,
}

impl Hold {
    /// The host has arrived where it was sent, so it may have the bar back.
    fn caught_up(&self, host_seconds: f64, tolerance: f64) -> bool {
        !self.down && (host_seconds - self.seconds).abs() <= tolerance
    }

    /// Patience has run out. A host that never honours the seek — a stream
    /// that cannot seek, a file that has ended — must not be able to freeze
    /// the playhead for the rest of the session.
    fn expired(&self, now: f64, patience: f64) -> bool {
        !self.down && now - self.released >= patience
    }
}

/// Which part of a gesture a point on the bar arrived from.
///
/// The bar cannot infer this from the hold it is carrying, because the hold
/// outlives the gesture that made it: for `settle_secs` after a release
/// there is still one sitting there with its finger up. Every caller says
/// which end it is instead.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stroke {
    /// The first touch of a gesture.
    Down,
    /// A move inside a gesture that already owns the bar.
    Move,
    /// The last point of a gesture that began with a `Down`.
    Up,
    /// A key press: both ends at once. One whole intent rather than a
    /// gesture with two of them.
    Whole,
}

impl Stroke {
    /// A stroke that begins a gesture takes the bar fresh.
    fn begins(self) -> bool {
        matches!(self, Stroke::Down | Stroke::Whole)
    }

    /// A stroke that ends one sends its target whatever the interval says,
    /// because nothing follows it to carry the target out.
    fn ends(self) -> bool {
        matches!(self, Stroke::Up | Stroke::Whole)
    }
}

/// The hold the bar is left carrying after a stroke puts the playhead at
/// `fraction`.
///
/// A stroke that BEGINS a gesture always builds a new one. Carrying the old
/// hold over would leave `down` false for a drag that has only just started
/// — every move and the release after it would be dropped as belonging to
/// no gesture, `drag.off` would never play, and the previous release time
/// riding along inside it would expire the hold with the finger still on
/// the bar and hand the playhead back to the host mid-drag. All of which is
/// reachable by pressing twice inside `settle_secs`, which is a second.
fn hold_after(held: Option<Hold>, stroke: Stroke, fraction: f64, seconds: f64, now: f64) -> Hold {
    match held {
        Some(mut hold) if !stroke.begins() => {
            hold.fraction = fraction;
            hold.seconds = seconds;
            hold
        }
        _ => Hold { fraction, seconds, down: true, released: now },
    }
}

/// A playback bar's actions.
#[derive(Clone, Debug, Default)]
pub enum PlaybackBarAction {
    /// A drag started. A host that pauses while the playhead is being moved
    /// pauses here.
    Grabbed,
    /// The bar is showing this many seconds. Every pointer move raises one,
    /// so drive only cheap things from it — a caption, a preview thumbnail.
    Scrubbed(f64),
    /// Go here. Coalesced: at most one per `seek_interval` while a drag
    /// runs, plus the point the gesture settled on.
    Seek(f64),
    /// The drag ended at this many seconds. Keyboard seeks raise no
    /// `Grabbed` and no `Released`, since a key press is one whole intent
    /// rather than a gesture with two ends.
    Released(f64),
    #[default]
    None,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPlaybackBar {
    #[deref]
    draw_super: DrawQuad,
    /// Where the playhead sits along the track, 0..1.
    #[live]
    pos_frac: f32,
    /// The geometry Rust owns, handed to the shader every draw so the hit
    /// band and the drawn track cannot drift apart.
    #[live]
    label_px: f32,
    #[live]
    inset_px: f32,
}

#[derive(Script, Widget, Animator)]
pub struct PlaybackBar {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawPlaybackBar,
    #[live]
    draw_text: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// The whole length, in seconds. Zero means nobody has said, and then
    /// the bar draws an empty track and refuses every gesture — seeking into
    /// something of unknown length has no defined destination.
    #[live]
    pub duration: f64,
    /// Where the playhead is, in seconds, as the host last said.
    #[live]
    pub position: f64,

    /// How far an arrow key moves the playhead.
    #[live(5.0)]
    pub step_secs: f64,
    /// The least time between two seeks while a drag runs. Zero reports
    /// every move, which is what a host with a free seek wants.
    #[live(0.12)]
    pub seek_interval: f64,
    /// How long the bar keeps showing the finger's answer after the finger
    /// lifts, waiting for the host to catch up.
    #[live(1.0)]
    pub settle_secs: f64,
    /// How near the host has to land to count as having honoured the seek.
    #[live(0.75)]
    pub settle_tolerance: f64,

    /// Room above the track for the clock, and the track's inset from the
    /// widget's edges. Rust owns both because the hit band is measured from
    /// them; the shader is handed the same numbers every draw.
    ///
    /// The inset is the knob's room, not decoration. The knob's centre goes
    /// all the way to both ends of the track — that is what makes the drawn
    /// mark and the hit mapping one number — so its outer edge, 9.5 points
    /// out at the hover size and the default ring, would be cut by the
    /// widget's edge at 0:00 and again at the end with any less than this.
    #[live(16.0)]
    label_height: f64,
    #[live(10.0)]
    track_inset: f64,

    /// Draw the clock. Off for a bar whose host already prints the times.
    #[live(true)]
    pub show_clock: bool,
    /// The right-hand clock counts down rather than naming the length.
    #[live]
    pub show_remaining: bool,

    #[rust]
    hold: Option<Hold>,
    #[rust]
    gate: SeekGate,
    #[rust]
    next_frame: NextFrame,
}

impl ScriptHook for PlaybackBar {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if !apply.is_eval() {
            return;
        }
        // A host or the design tool writing `position` outranks a hold that
        // is only waiting for the host to catch up: otherwise the bar would
        // ignore the very value it was just handed. A hold with the finger
        // still on it wins, because that finger is a person.
        if self.hold.as_ref().map(|h| h.down) != Some(true) {
            self.hold = None;
        }
        vm.with_cx_mut(|cx| self.draw_bg.redraw(cx));
    }
}

impl PlaybackBar {
    /// A length nobody has said is not a track anybody can seek into.
    fn seekable(&self) -> bool {
        self.duration.is_finite() && self.duration > 0.0
    }

    fn seconds_at(&self, fraction: f64) -> f64 {
        if self.seekable() {
            fraction.clamp(0.0, 1.0) * self.duration
        } else {
            0.0
        }
    }

    /// What is drawn: the gesture's answer while a gesture has one, the
    /// host's the rest of the time.
    fn shown_fraction(&self) -> f64 {
        match &self.hold {
            Some(hold) => hold.fraction,
            None => fraction_of(self.position, self.duration),
        }
    }

    fn shown_seconds(&self) -> f64 {
        match &self.hold {
            Some(hold) => hold.seconds,
            None if self.seekable() => self.position.clamp(0.0, self.duration),
            // Unknown length still has an elapsed time worth printing.
            None => self.position.max(0.0),
        }
    }

    /// The two halves of the clock. The right half is empty when the length
    /// is unknown, since a total nobody has said cannot be printed and a
    /// zero there would be a lie.
    fn clocks(&self) -> (String, String) {
        let here = self.shown_seconds();
        let hours = self.duration >= 3600.0 || here >= 3600.0;
        let left = clock(here, hours);
        if !self.seekable() {
            return (left, String::new());
        }
        let right = if self.show_remaining {
            format!("-{}", clock((self.duration - here).max(0.0), hours))
        } else {
            clock(self.duration, hours)
        };
        (left, right)
    }

    /// The strip kept above the track for the clock, which is nothing at all
    /// on a bar that is not drawing one.
    fn label_room(&self) -> f64 {
        label_room(self.show_clock, self.label_height)
    }

    fn fraction_at_x(&self, abs_x: f64, rect: Rect) -> f64 {
        fraction_at(abs_x, rect.pos.x + self.track_inset, rect.size.x - self.track_inset * 2.0)
    }

    /// Put the playhead at `fraction` and say so.
    fn drive(&mut self, cx: &mut Cx, fraction: f64, now: f64, stroke: Stroke) {
        let seconds = self.seconds_at(fraction);
        self.hold = Some(hold_after(self.hold.take(), stroke, fraction, seconds, now));
        cx.widget_action(self.uid, PlaybackBarAction::Scrubbed(seconds));
        self.gate.want(fraction);
        let target = if stroke.ends() {
            self.gate.flush()
        } else {
            self.gate.due(now, self.seek_interval)
        };
        if let Some(target) = target {
            let at = self.seconds_at(target);
            cx.widget_action(self.uid, PlaybackBarAction::Seek(at));
        }
        self.next_frame = cx.new_next_frame();
        self.draw_bg.redraw(cx);
    }

    /// The frame clock: it releases a target the interval held back, and
    /// ends a hold the host never came to claim.
    fn tick(&mut self, cx: &mut Cx, now: f64) {
        if let Some(target) = self.gate.due(now, self.seek_interval) {
            let at = self.seconds_at(target);
            cx.widget_action(self.uid, PlaybackBarAction::Seek(at));
        }
        let mut again = self.gate.pending.is_some();
        if let Some(hold) = &self.hold {
            if hold.expired(now, self.settle_secs) {
                self.hold = None;
                self.draw_bg.redraw(cx);
            } else {
                again = true;
            }
        }
        if again {
            self.next_frame = cx.new_next_frame();
        }
    }

    /// Where the host says the playhead is. Ignored outright while a gesture
    /// owns the bar; a report that arrives at the seek's destination ends
    /// the hold there and then, without waiting out `settle_secs`.
    pub fn set_position(&mut self, cx: &mut Cx, seconds: f64) {
        self.position = seconds;
        if let Some(hold) = &self.hold {
            if !hold.caught_up(seconds, self.settle_tolerance) {
                return;
            }
            self.hold = None;
        }
        self.draw_bg.redraw(cx);
    }

    pub fn set_duration(&mut self, cx: &mut Cx, seconds: f64) {
        self.duration = seconds;
        // A new length is normally a new piece, and a hold left over from
        // the last one is keeping a place in something that has stopped
        // playing. A finger still on the bar keeps its hold: it is aiming
        // at the bar in front of it, whatever the host loaded behind it.
        if self.hold.as_ref().map(|h| h.down) != Some(true) {
            self.hold = None;
            self.gate.rest();
        }
        self.draw_bg.redraw(cx);
    }
}

impl Widget for PlaybackBar {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_position) || method == live_id!(set_duration) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                if let Some(seconds) = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64() {
                    if method == live_id!(set_position) {
                        vm.with_cx_mut(|cx| self.set_position(cx, seconds));
                    } else {
                        vm.with_cx_mut(|cx| self.set_duration(cx, seconds));
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(position) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.position));
        }
        if method == live_id!(duration) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.duration));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        if let Some(ne) = self.next_frame.is_event(event) {
            self.tick(cx, ne.time);
        }
        if self.animator_in_state(cx, ids!(disabled.on)) {
            // A bar disabled under a finger would otherwise keep a hold
            // with its finger down for the rest of the session: nothing
            // could expire it, every `set_position` would be ignored and
            // `tick` would re-arm a frame forever. A disabled bar has no
            // gesture, so the hold goes with it.
            if self.hold.as_ref().map(|h| h.down) == Some(true) {
                self.hold = None;
                self.gate.rest();
                self.animator_play(cx, ids!(drag.off));
                self.draw_bg.redraw(cx);
            }
            return;
        }
        let uid = self.uid;
        let dragging = self.hold.as_ref().map(|h| h.down) == Some(true);

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerHoverOver(fe) => {
                // The cursor changes only over the band that answers, so
                // the pointer says where the target is before it is tried.
                if self.seekable() && seeks_at(fe.abs.y - fe.rect.pos.y, self.label_room()) {
                    cx.set_cursor(MouseCursor::Hand);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                if !self.seekable() || !seeks_at(fe.abs.y - fe.rect.pos.y, self.label_room()) {
                    return;
                }
                cx.set_key_focus(self.draw_bg.area());
                // A fresh gesture: what the last one sent must not hold this
                // one's first target back.
                self.gate.rest();
                self.animator_play(cx, ids!(drag.on));
                cx.widget_action(uid, PlaybackBarAction::Grabbed);
                let fraction = self.fraction_at_x(fe.abs.x, fe.rect);
                self.drive(cx, fraction, fe.time, Stroke::Down);
            }
            Hit::FingerMove(fe) => {
                if !dragging {
                    return;
                }
                let fraction = self.fraction_at_x(fe.abs.x, fe.rect);
                self.drive(cx, fraction, fe.time, Stroke::Move);
            }
            Hit::FingerUp(fe) => {
                if !dragging {
                    return;
                }
                // A press taken away ends where the gesture last put the bar,
                // not wherever the finger was when it was taken.
                let fraction = match (&self.hold, fe.cancelled) {
                    (Some(hold), true) => hold.fraction,
                    _ => self.fraction_at_x(fe.abs.x, fe.rect),
                };
                self.drive(cx, fraction, fe.time, Stroke::Up);
                if let Some(hold) = &mut self.hold {
                    hold.down = false;
                    hold.released = fe.time;
                }
                self.animator_play(cx, ids!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                let at = self.seconds_at(fraction);
                cx.widget_action(uid, PlaybackBarAction::Released(at));
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
            }
            Hit::KeyDown(ke) => {
                // The bar takes key focus on the press that starts a drag,
                // so a key can arrive with the finger still down. `Whole`
                // would throw that drag's hold away and leave the finger
                // with nothing to move; the finger keeps the bar.
                if !self.seekable() || dragging {
                    return;
                }
                let here = self.shown_seconds();
                let to = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => here - self.step_secs,
                    KeyCode::ArrowRight | KeyCode::ArrowUp => here + self.step_secs,
                    KeyCode::Home => 0.0,
                    KeyCode::End => self.duration,
                    _ => return,
                };
                // A key press is one whole intent, not a stream: it is not
                // held back by the drag interval, and it needs no hold of
                // its own beyond the settling one `drive` leaves behind.
                self.gate.rest();
                self.drive(cx, fraction_of(to, self.duration), ke.time, Stroke::Whole);
                if let Some(hold) = &mut self.hold {
                    hold.down = false;
                    hold.released = ke.time;
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // The one number for the strip: the shader draws its track below it,
        // the hit band starts at it and the clock sits in it, so a bar with
        // the clock off loses the strip in all three at once.
        let room = self.label_room();
        self.draw_bg.pos_frac = self.shown_fraction() as f32;
        self.draw_bg.label_px = room as f32;
        self.draw_bg.inset_px = self.track_inset as f32;

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        if room > 0.0 {
            let (left, right) = self.clocks();
            let size = self.draw_text.text_style.font_size as f64;
            let baseline = rect.pos.y + (room - size) * 0.5;
            self.draw_text
                .draw_abs(cx, dvec2(rect.pos.x + self.track_inset, baseline), &left);
            if !right.is_empty() {
                let width = measure(&self.draw_text, cx, &right);
                let x = rect.pos.x + rect.size.x - self.track_inset - width;
                self.draw_text.draw_abs(cx, dvec2(x, baseline), &right);
            }
        }
        self.draw_bg.end(cx);

        if !self.animator_in_state(cx, ids!(disabled.on)) {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::Slider, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        let (left, right) = self.clocks();
        if right.is_empty() {
            left
        } else {
            format!("{left} / {right}")
        }
    }
}

/// The first action of the wanted shape in one pass, whatever else that pass
/// is carrying from the same bar.
///
/// Every reader below goes through this rather than `find_widget_action`
/// (widget.rs:1715-1724), which answers with the FIRST action from a uid and
/// stops, whatever type it is. This bar puts THREE in the pass a press
/// lands — `Grabbed`, then `Scrubbed`, then `Seek` — so a reader built on
/// that helper would cast the `Grabbed`, fail its own match and report
/// nothing: a click would never deliver a seek, a release would never be
/// seen behind the `Scrubbed` that precedes it, and a keyboard seek would be
/// swallowed the same way. The library says so directly above the helper
/// (widget.rs:1595-1601) and ships `filter_widget_actions_cast` for it.
///
/// Reordering the pushes is not the fix: four readers want four different
/// firsts.
fn scan<T>(
    actions: &Actions,
    uid: WidgetUid,
    mut pick: impl FnMut(PlaybackBarAction) -> Option<T>,
) -> Option<T> {
    for action in actions.filter_widget_actions_cast::<PlaybackBarAction>(uid) {
        if let Some(found) = pick(action) {
            return Some(found);
        }
    }
    None
}

impl PlaybackBarRef {
    /// Where the host says the playhead is; ignored while a gesture owns
    /// the bar, so it is safe to call on every position report a player
    /// makes.
    pub fn set_position(&self, cx: &mut Cx, seconds: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_position(cx, seconds);
        }
    }

    pub fn set_duration(&self, cx: &mut Cx, seconds: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_duration(cx, seconds);
        }
    }

    /// What the bar is SHOWING, which during a gesture is the finger's
    /// answer rather than the host's.
    pub fn shown_seconds(&self) -> f64 {
        self.borrow().map(|inner| inner.shown_seconds()).unwrap_or(0.0)
    }

    pub fn duration(&self) -> f64 {
        self.borrow().map(|inner| inner.duration).unwrap_or(0.0)
    }

    /// The seek to act on. Already coalesced, so a player may take every one
    /// of these at face value.
    pub fn seek(&self, actions: &Actions) -> Option<f64> {
        scan(actions, self.widget_uid(), |action| match action {
            PlaybackBarAction::Seek(seconds) => Some(seconds),
            _ => None,
        })
    }

    /// Where the bar is showing, on every move of the gesture. For captions
    /// and previews, not for seeking.
    pub fn scrubbed(&self, actions: &Actions) -> Option<f64> {
        scan(actions, self.widget_uid(), |action| match action {
            PlaybackBarAction::Scrubbed(seconds) => Some(seconds),
            _ => None,
        })
    }

    /// A drag began. A host that pauses while the playhead moves pauses on
    /// this and resumes on `released`.
    pub fn grabbed(&self, actions: &Actions) -> bool {
        scan(actions, self.widget_uid(), |action| {
            matches!(action, PlaybackBarAction::Grabbed).then_some(())
        })
        .is_some()
    }

    pub fn released(&self, actions: &Actions) -> Option<f64> {
        scan(actions, self.widget_uid(), |action| match action {
            PlaybackBarAction::Released(seconds) => Some(seconds),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_under_an_hour_is_minutes_and_seconds_with_no_leading_zero_on_the_minutes() {
        assert_eq!(format_clock(0.0), "0:00");
        assert_eq!(format_clock(7.0), "0:07");
        assert_eq!(format_clock(60.0), "1:00");
        assert_eq!(format_clock(599.0), "9:59");
        assert_eq!(format_clock(3599.0), "59:59");
    }

    #[test]
    fn a_clock_grows_an_hours_field_only_once_there_is_an_hour_in_it() {
        assert_eq!(format_clock(3600.0), "1:00:00");
        assert_eq!(format_clock(3661.0), "1:01:01");
        assert_eq!(format_clock(7384.0), "2:03:04");
        // One second short of the hour is still minutes and seconds, so the
        // field appears exactly when the hour does.
        assert_eq!(format_clock(3599.999), "59:59");
    }

    #[test]
    fn a_clock_floors_so_it_never_reads_a_second_that_has_not_played() {
        assert_eq!(format_clock(0.999), "0:00");
        assert_eq!(format_clock(59.6), "0:59");
        assert_eq!(format_clock(119.9), "1:59");
    }

    #[test]
    fn a_clock_of_a_negative_or_nonsense_number_reads_the_start() {
        assert_eq!(format_clock(-1.0), "0:00");
        assert_eq!(format_clock(f64::NAN), "0:00");
        // An endless piece is not a length; the clock reads the start
        // rather than printing whatever an infinity casts to.
        assert_eq!(format_clock(f64::INFINITY), "0:00:00");
    }

    #[test]
    fn a_position_keeps_the_shape_of_the_length_beside_it() {
        // Told to carry hours, a position inside the first minute still
        // prints all three fields, so the two clocks do not change width
        // against each other as the piece plays.
        assert_eq!(clock(42.0, true), "0:00:42");
        assert_eq!(clock(42.0, false), "0:42");
    }

    #[test]
    fn a_fraction_is_measured_from_the_track_and_not_from_the_widget_edge() {
        // A widget at x=100, 6 points of inset either side of a 200-wide
        // track. The middle of the track is the halfway point, not the
        // middle of the widget.
        assert_eq!(fraction_at(206.0, 106.0, 200.0), 0.5);
        assert_eq!(fraction_at(106.0, 106.0, 200.0), 0.0);
        assert_eq!(fraction_at(306.0, 106.0, 200.0), 1.0);
    }

    #[test]
    fn a_press_past_either_end_of_the_track_is_the_end_it_is_past() {
        // The inset is reachable: pressing it is a press on the end, not a
        // press on nothing.
        assert_eq!(fraction_at(100.0, 106.0, 200.0), 0.0);
        assert_eq!(fraction_at(400.0, 106.0, 200.0), 1.0);
    }

    #[test]
    fn a_track_with_no_width_reports_the_start_rather_than_dividing_by_it() {
        assert_eq!(fraction_at(50.0, 0.0, 0.0), 0.0);
        assert_eq!(fraction_at(50.0, 0.0, -10.0), 0.0);
        assert_eq!(fraction_at(50.0, 0.0, f64::NAN), 0.0);
    }

    #[test]
    fn a_position_in_a_piece_of_unknown_length_is_the_start() {
        assert_eq!(fraction_of(30.0, 0.0), 0.0);
        assert_eq!(fraction_of(30.0, f64::NAN), 0.0);
        assert_eq!(fraction_of(f64::NAN, 100.0), 0.0);
        assert_eq!(fraction_of(30.0, 120.0), 0.25);
        // A host that reports past the end does not push the fill past it.
        assert_eq!(fraction_of(150.0, 120.0), 1.0);
    }

    #[test]
    fn the_press_band_is_every_row_below_the_clock_and_not_the_thin_track() {
        // A forty-point bar with a sixteen-point clock row: the track is
        // four points through the middle of the remaining twenty-four, and
        // every one of those twenty-four answers a press.
        for y in [16.0, 20.0, 26.0, 30.0, 39.9] {
            assert!(seeks_at(y, 16.0), "{y} is in the band");
        }
        // The clock row is not, so pressing the total does not throw the
        // playhead to the end of the piece.
        assert!(!seeks_at(0.0, 16.0));
        assert!(!seeks_at(15.9, 16.0));
        // A bar with no clock is a band from top to bottom.
        assert!(seeks_at(0.0, 0.0));
    }

    #[test]
    fn the_first_seek_of_a_gesture_goes_out_at_once() {
        let mut gate = SeekGate::default();
        gate.want(0.4);
        assert_eq!(gate.due(10.0, 0.12), Some(0.4), "a press seeks immediately");
    }

    #[test]
    fn a_burst_of_moves_inside_the_interval_yields_one_seek_at_the_newest_target() {
        let mut gate = SeekGate::default();
        gate.want(0.1);
        assert_eq!(gate.due(0.0, 0.12), Some(0.1));
        // Five moves within the interval. Every one is overtaken but the
        // last, and only the last is worth reporting.
        for (t, f) in [(0.02, 0.2), (0.04, 0.3), (0.06, 0.4), (0.08, 0.5), (0.10, 0.6)] {
            gate.want(f);
            assert_eq!(gate.due(t, 0.12), None, "held back at {t}");
        }
        assert_eq!(gate.due(0.13, 0.12), Some(0.6), "the newest target, not the oldest");
    }

    #[test]
    fn a_target_held_back_by_the_interval_is_still_waiting_to_go_out() {
        let mut gate = SeekGate::default();
        gate.want(0.1);
        gate.due(0.0, 0.12);
        gate.want(0.9);
        assert_eq!(gate.due(0.05, 0.12), None);
        assert!(gate.pending.is_some(), "the frame clock has something to come back for");
    }

    #[test]
    fn the_target_the_gesture_settled_on_goes_out_whatever_the_clock_says() {
        let mut gate = SeekGate::default();
        gate.want(0.2);
        gate.due(0.0, 0.12);
        gate.want(0.8);
        assert_eq!(gate.due(0.01, 0.12), None, "too soon for a moving drag");
        assert_eq!(gate.flush(), Some(0.8), "but a release is not a move");
    }

    #[test]
    fn a_release_on_the_target_already_sent_asks_for_no_second_seek() {
        let mut gate = SeekGate::default();
        gate.want(0.35);
        assert_eq!(gate.due(0.0, 0.12), Some(0.35));
        gate.want(0.35);
        assert_eq!(gate.flush(), None, "the host is already going there");
    }

    #[test]
    fn a_new_gesture_is_not_held_back_by_the_last_ones_clock() {
        let mut gate = SeekGate::default();
        gate.want(0.3);
        gate.due(100.0, 0.12);
        // A second press a hair later would otherwise be swallowed by the
        // interval left over from the first.
        gate.rest();
        gate.want(0.7);
        assert_eq!(gate.due(100.01, 0.12), Some(0.7));
    }

    #[test]
    fn the_host_is_ignored_while_the_finger_is_down() {
        let hold = Hold { fraction: 0.5, seconds: 60.0, down: true, released: 0.0 };
        // Even a report that lands exactly on the target: the finger has
        // not finished, and the next move will contradict it.
        assert!(!hold.caught_up(60.0, 0.75));
        assert!(!hold.expired(1000.0, 1.0), "a hold does not expire under a finger");
    }

    #[test]
    fn a_released_hold_ends_when_the_host_arrives_where_it_was_sent() {
        let hold = Hold { fraction: 0.5, seconds: 60.0, down: false, released: 4.0 };
        assert!(!hold.caught_up(12.0, 0.75), "still reporting where it was playing");
        assert!(hold.caught_up(60.4, 0.75), "close enough is arrived");
        assert!(!hold.caught_up(61.0, 0.75), "a second out is not arrived");
    }

    #[test]
    fn a_host_that_never_honours_the_seek_does_not_freeze_the_playhead() {
        let hold = Hold { fraction: 0.5, seconds: 60.0, down: false, released: 4.0 };
        assert!(!hold.expired(4.5, 1.0), "give it a moment");
        assert!(hold.expired(5.0, 1.0), "then give the bar back");
    }

    #[test]
    fn a_press_landing_on_a_hold_that_is_still_settling_starts_a_gesture_of_its_own() {
        // The bar a second after a release: the hold is still there, finger
        // up, holding the last gesture's release time.
        let settling = Hold { fraction: 0.5, seconds: 60.0, down: false, released: 4.0 };
        let pressed = hold_after(Some(settling), Stroke::Down, 0.25, 30.0, 4.4);
        // A press is a new gesture whatever was left lying there. Reuse
        // would leave `down` false, and then the moves and the release that
        // follow belong to no gesture and are dropped.
        assert!(pressed.down, "the finger on the bar is down");
        assert_eq!(pressed.released, 4.4, "and the old release time is gone");
        assert_eq!(pressed.fraction, 0.25);
        assert_eq!(pressed.seconds, 30.0);
        // The stale release time is not a detail: kept, it expires the hold
        // 0.6s into a press that is still going on.
        assert!(
            !pressed.expired(5.0, 1.0),
            "a hold under a finger cannot expire, and its clock starts now"
        );
    }

    #[test]
    fn a_move_inside_a_gesture_keeps_the_hold_the_press_made() {
        let pressed = hold_after(None, Stroke::Down, 0.25, 30.0, 4.0);
        let moved = hold_after(Some(pressed), Stroke::Move, 0.6, 72.0, 4.2);
        // Only the point moves. The finger is the same finger, so `down`
        // and the release clock are the press's.
        assert_eq!((moved.fraction, moved.seconds), (0.6, 72.0));
        assert!(moved.down);
        assert_eq!(moved.released, 4.0);
    }

    #[test]
    fn a_key_press_is_one_whole_intent_and_takes_the_bar_fresh() {
        assert!(Stroke::Whole.begins(), "it owns the bar from this point");
        assert!(Stroke::Whole.ends(), "and nothing follows to carry its target out");
        // A key struck while the last one is still settling restarts the
        // wait rather than inheriting a clock that is part run down.
        let settling = Hold { fraction: 0.1, seconds: 12.0, down: false, released: 9.0 };
        let struck = hold_after(Some(settling), Stroke::Whole, 0.2, 24.0, 9.5);
        assert_eq!(struck.released, 9.5);
        // The two mid-gesture strokes are the other way round on both counts.
        assert!(!Stroke::Move.begins() && !Stroke::Move.ends());
        assert!(!Stroke::Down.ends(), "a press has a release coming to carry it");
        assert!(!Stroke::Up.begins(), "a release joins the gesture it ends");
    }

    #[test]
    fn a_bar_that_draws_no_clock_keeps_no_room_for_one() {
        // The strip is reserved and cut out of the press band by the same
        // number, so turning the clock off has to reclaim it or the bar
        // grows a dead top that refuses presses.
        assert_eq!(label_room(true, 16.0), 16.0);
        assert_eq!(label_room(false, 16.0), 0.0);
        assert!(seeks_at(0.0, label_room(false, 16.0)), "top to bottom answers");
        assert!(!seeks_at(0.0, label_room(true, 16.0)), "the clock row does not");
        // A negative height is no room rather than a band starting above
        // the widget.
        assert_eq!(label_room(true, -4.0), 0.0);
    }

    /// One widget action, as `cx.widget_action` builds it.
    fn action(uid: WidgetUid, what: PlaybackBarAction) -> Action {
        Box::new(WidgetAction { widget_uid: uid, data: None, action: Box::new(what), group: None })
    }

    #[test]
    fn every_reader_finds_its_own_action_in_the_pass_a_press_puts_out() {
        // The pass a press on the bar lands, in the order it lands it.
        let uid = WidgetUid(9_001);
        let pass: ActionsBuf = vec![
            action(uid, PlaybackBarAction::Grabbed),
            action(uid, PlaybackBarAction::Scrubbed(60.0)),
            action(uid, PlaybackBarAction::Seek(60.0)),
        ];
        let seek = scan(&pass, uid, |a| match a {
            PlaybackBarAction::Seek(s) => Some(s),
            _ => None,
        });
        let scrubbed = scan(&pass, uid, |a| match a {
            PlaybackBarAction::Scrubbed(s) => Some(s),
            _ => None,
        });
        let grabbed = scan(&pass, uid, |a| {
            matches!(a, PlaybackBarAction::Grabbed).then_some(())
        });
        // A reader that took the pass's first action and stopped would
        // answer None to two of these three, and the seek is the one the
        // whole widget exists to deliver.
        assert_eq!(seek, Some(60.0), "a click delivers its seek");
        assert_eq!(scrubbed, Some(60.0));
        assert_eq!(grabbed, Some(()));
        let first = pass[0].downcast_ref::<WidgetAction>().expect("a widget action");
        assert!(
            matches!(
                first.action.downcast_ref::<PlaybackBarAction>(),
                Some(PlaybackBarAction::Grabbed)
            ),
            "and the first of the three is neither of the two"
        );
    }

    #[test]
    fn a_release_is_found_behind_the_scrub_and_the_seek_that_precede_it() {
        // The pass a finger lifting lands: `Released` is last of four.
        let uid = WidgetUid(9_002);
        let pass: ActionsBuf = vec![
            action(uid, PlaybackBarAction::Scrubbed(90.0)),
            action(uid, PlaybackBarAction::Seek(90.0)),
            action(uid, PlaybackBarAction::Released(90.0)),
        ];
        let released = scan(&pass, uid, |a| match a {
            PlaybackBarAction::Released(s) => Some(s),
            _ => None,
        });
        assert_eq!(released, Some(90.0));
    }

    #[test]
    fn another_bars_actions_in_the_same_pass_are_not_this_bars() {
        // Two bars on a page report in one pass. A reader answers for its
        // own uid and is blind to the other's, however early the other's
        // action sits.
        let mine = WidgetUid(9_003);
        let theirs = WidgetUid(9_004);
        let pass: ActionsBuf = vec![
            action(theirs, PlaybackBarAction::Seek(11.0)),
            action(mine, PlaybackBarAction::Grabbed),
            action(mine, PlaybackBarAction::Seek(22.0)),
        ];
        let pick = |uid| {
            scan(&pass, uid, |a| match a {
                PlaybackBarAction::Seek(s) => Some(s),
                _ => None,
            })
        };
        assert_eq!(pick(mine), Some(22.0));
        assert_eq!(pick(theirs), Some(11.0));
        assert_eq!(pick(WidgetUid(9_005)), None, "a bar that said nothing said nothing");
    }
}
