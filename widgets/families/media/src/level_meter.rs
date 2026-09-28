//! Level meter — a bar for a live value that FALLS, with a high-water mark
//! and a latching out-of-range lamp.
//!
//! The library already had every shape of "how far along is it": bars,
//! rings, arcs and a dial. None of them can do this. The bars and rings in
//! `progress.rs` ease forward only, because a bar that slides backwards
//! looks like a bar that is lying, and a value set below the one on screen
//! snaps there instead. That is the right rule for a download and the wrong
//! one for load, throughput, latency, temperature or headroom, where going
//! down IS the news and the fall is the part being read. The one member of
//! that family allowed to fall is the `Gauge`, and it eases a needle toward
//! a target in both directions at one speed: no instant attack, no
//! high-water mark, no repaint gate. A dial, not a meter. So this is a
//! separate widget rather than a flag on that family: the two disagree about
//! what a falling number means.
//!
//! # What makes it readable
//!
//! A meter fed the raw newest reading is unreadable: it is twenty different
//! numbers a second and the eye takes an average of the flicker rather than
//! the worst thing that happened. So the bar goes up the instant a higher
//! reading arrives — missing the spike is the one thing a peak meter may not
//! do — and comes down on a schedule slow enough to follow. Above it rides a
//! mark holding the highest recent reading, for the glance that was
//! somewhere else. That arithmetic is `MeterBallistics`, a plain type beside
//! the widget: a host that draws its own meter, or wants the numbers without
//! a widget at all, can use it on its own, and it can be held to its own
//! arithmetic in a test because it takes `dt` rather than reading a clock.
//!
//! # The lamp does not clear itself
//!
//! An out-of-range event is over in a millisecond; a meter that showed it
//! for one frame would show it to nobody. So `over` LATCHES: the host says
//! `set_over(cx, true)` when a reading went past whatever its own ceiling
//! is, and the lamp stays lit until something clears it — a press on the
//! meter, or `clear`. Handing `set_over` a false does nothing on purpose.
//! The widget never decides what out of range means; it only remembers.
//!
//! # Why it stops asking to be drawn
//!
//! Between the two obvious ways to drive a meter, one never repaints (the
//! host feeds it and nothing runs the fall) and the other repaints the
//! window forever (a timer ticks whether or not anything moved, including
//! with nothing happening). This one runs frames only while there is
//! something left to animate, and only calls for a redraw when the drawn
//! value has moved far enough to see — the deadband in `take_push`. A
//! settled meter costs nothing at all until the next reading arrives.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// The bar covers about nine tenths of the distance to a lower reading in
/// 1.7 seconds, which as a time constant is 1.7 / ln(10). Slow enough that a
/// fall can be followed by eye, fast enough that the bar is about now.
const RELEASE_SECS: f32 = 0.74;
/// How long the mark stands where it is before it starts to follow the bar
/// down. Long enough to look up at, short enough to still be about now.
const HOLD_SECS: f32 = 1.0;
/// A one-pole never arrives, so under this the meter is simply out. Without
/// it a settled meter would go on asking to be redrawn for the rest of the
/// session, each time by an amount no screen can show.
const SILENCE: f32 = 0.001;
/// Below this much of the scale a move cannot be seen: a meter is at most a
/// couple of hundred device pixels long, so this is a fraction of one.
const DEADBAND: f32 = 1.0 / 256.0;
/// A tick this long is not a tick, it is a gap — a window that was not being
/// painted, a page that was not on screen. Obeyed literally it would dump
/// the meter to zero in one step, which reads as a cut rather than a fall.
const LONGEST_TICK: f32 = 0.25;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawLevelMeterBase = #(DrawLevelMeter::script_component(vm))
    set_type_default() do #(DrawLevelMeter::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.LevelMeterBase = #(LevelMeter::register_widget(vm))
    /** A bar for a live value that falls: instant on the way up, slow on the
     * way down, with a mark holding the highest recent reading and a lamp
     * that latches when the host says the reading went out of range. */
    mod.widgets.LevelMeter = set_type_default() do mod.widgets.LevelMeterBase{
        width: 160
        height: 12
        /** a standing reading for a meter nothing is feeding; writing it again moves the bar 0..1 step 0.01 */
        level: 0.0
        /** run the scale bottom to top instead of left to right 0..1 step 1 */
        vertical: false
        /** show the out-of-range lamp at the high end of the scale 0..1 step 1 */
        lamp: true
        /** the lamp, lit; the widget latches this and only clearing puts it out 0..1 step 1 */
        over: false
        /** the fall's time constant in seconds; nine tenths of a fall takes about 2.3 of them 0.05..3 step 0.05 */
        release_secs: 0.74
        /** seconds the mark stands before it follows the bar down 0..5 step 0.1 */
        hold_secs: 1.0
        /** the exponent the reading is drawn on; 1 is linear, 0.5 lifts the quiet end 0.2..2 step 0.05 */
        taper: 1.0
        /** a press anywhere on the meter puts the lamp out 0..1 step 1 */
        clear_on_press: true
        /** dimmed, for a channel that is not live 0..1 step 1 */
        disabled: false

        // `level`..`opacity` are the instances: they ride in the draw
        // struct, so every other property here has to be a uniform or the
        // instance slots stop lining up with the struct.
        draw_bg +: {
            level: 0.0
            hold: 0.0
            over: 0.0
            lamp: 1.0
            vertical: 0.0
            opacity: 1.0
            /** corner rounding of the trough and the lamp 0..8 step 0.5 */
            border_radius: uniform(3.0)
            /** clear space between the trough wall and the lit bar 0..4 step 0.5 */
            inset: uniform(1.5)
            /** where on the scale the bar starts warming 0..1 step 0.01 */
            hot_at: uniform(0.7)
            /** where on the scale it is fully hot 0..1 step 0.01 */
            hot_to: uniform(0.95)
            /** pitch of the ladder rungs, in points; zero draws the bar solid 0..12 step 0.5 */
            segment: uniform(4.0)
            /** the share of each rung left dark 0..0.9 step 0.05 */
            segment_gap: uniform(0.35)
            /** thickness of the high-water mark, in points 1..6 step 0.5 */
            mark_size: uniform(2.0)
            /** length of the lamp along the scale, in points 4..24 step 0.5 */
            lamp_size: uniform(8.0)
            /** clear space between the trough and the lamp 0..8 step 0.5 */
            lamp_gap: uniform(3.0)
            /** the empty trough */
            track_color: uniform(theme.color_surface_container_highest)
            /** the bar at the quiet end of the scale */
            color_level: uniform(theme.color_success)
            /** the bar at the loud end */
            color_hot: uniform(theme.color_warning)
            /** the high-water mark's core: the theme's own ink */
            color_mark: uniform(theme.color_text)
            /** what the mark is backed with. The far end of the surface
             * ladder from the ink, so whichever of the pair loses its
             * contrast against what the mark has landed on, the other has it */
            color_mark_shadow: uniform(theme.color_surface_container_lowest)
            /** the lamp with nothing to report */
            color_lamp_off: uniform(theme.color_surface_container_high)
            /** the lamp lit. Deliberately not the bar's hot colour: the one
             * thing it must not do is look like a loud reading */
            color_lamp: uniform(theme.color_error)

            // The meter is drawn in SCALE space -- `p` runs along the scale
            // from its quiet end, `q` across it -- and this is the one place
            // that turns a scale rect into a screen rect. It is why the two
            // orientations are one widget and not two: vertical only means
            // the scale runs up, so its quiet end is the bottom.
            rect_of: fn(p: float, q: float, plen: float, qlen: float) -> vec4 {
                let up = self.rect_size.y - p - plen
                return vec4(
                    mix(p, q, self.vertical),
                    mix(q, up, self.vertical),
                    mix(plen, qlen, self.vertical),
                    mix(qlen, plen, self.vertical)
                )
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let along = mix(self.rect_size.x, self.rect_size.y, self.vertical)
                let across = mix(self.rect_size.y, self.rect_size.x, self.vertical)
                // Where this pixel stands on the scale, quiet end at zero.
                let scale_at = mix(
                    self.pos.x * self.rect_size.x,
                    (1.0 - self.pos.y) * self.rect_size.y,
                    self.vertical
                )
                // The lamp takes its room off the loud end, and the reading
                // is drawn against what is left: a meter wearing a lamp must
                // not read a few percent short of one without.
                let lamp_room = self.lamp * (self.lamp_size + self.lamp_gap)
                let bar = max(4.0, along - lamp_room)
                let r = min(self.border_radius, across * 0.5)
                let pad = min(self.inset, across * 0.4)
                let span = max(1.0, bar - pad * 2.0)

                let trough = self.rect_of(0.0, 0.0, bar, across)
                sdf.box(trough.x, trough.y, trough.z, trough.w, r)
                sdf.fill(self.track_color)

                if self.level > 0.0 {
                    let lit = self.rect_of(pad, pad, max(1.0, span * self.level), across - pad * 2.0)
                    // Keyed to the READING at this pixel rather than to the
                    // widget's size, so the hot band sits at the same place
                    // on the scale however long the meter is drawn.
                    let share = clamp((scale_at - pad) / span, 0.0, 1.0)
                    let c = self.color_level.mix(self.color_hot, smoothstep(self.hot_at, self.hot_to, share))
                    // The ladder: a dark rung every `segment` points, which
                    // is what makes a level countable rather than merely
                    // long. A zero pitch draws the bar solid.
                    let rung = mix(
                        1.0,
                        step(self.segment_gap, fract(scale_at / max(2.0, self.segment))),
                        step(0.5, self.segment)
                    )
                    sdf.box(lit.x, lit.y, lit.z, lit.w, max(0.5, r - pad))
                    sdf.fill(vec4(c.xyz, c.w * rung))
                }
                // Nothing held, no mark: parked on the floor it would read
                // as a level that is not there.
                if self.hold > 0.002 {
                    // Held inside the trough rather than centred on the
                    // reading, because at full scale a centred mark loses
                    // half its width off the end -- the one reading it is
                    // there for. On a whole point, or a mark landing across
                    // two device rows at half weight in each comes and goes
                    // as the level nudges it a fraction either way. The
                    // shoulder is kept inside the trough too, hence the
                    // point of room left at each stop.
                    let shoulder = 1.0
                    let at = floor(clamp(
                        pad + span * self.hold - self.mark_size * 0.5,
                        pad + shoulder,
                        bar - pad - self.mark_size - shoulder
                    ))
                    // Drawn twice. The mark has to read against the empty
                    // trough, where it spends most of its life, AND against
                    // the lit bar it lands on at full scale, and no one
                    // colour does both in every theme: an ink that carries
                    // over the trough is washed out by a pale fill, and a
                    // dark one disappears into a dark trough. A core and a
                    // backing from opposite ends of the surface ladder mean
                    // one of the two always has its contrast.
                    let back = self.rect_of(
                        at - shoulder,
                        pad,
                        self.mark_size + shoulder * 2.0,
                        across - pad * 2.0
                    )
                    sdf.box(back.x, back.y, back.z, back.w, 0.5)
                    sdf.fill(self.color_mark_shadow)
                    let mark = self.rect_of(at, pad, self.mark_size, across - pad * 2.0)
                    sdf.box(mark.x, mark.y, mark.z, mark.w, 0.5)
                    sdf.fill(self.color_mark)
                }
                if self.lamp > 0.5 {
                    let lamp = self.rect_of(bar + self.lamp_gap, 0.0, self.lamp_size, across)
                    sdf.box(lamp.x, lamp.y, lamp.z, lamp.w, r)
                    sdf.fill(self.color_lamp_off.mix(self.color_lamp, step(0.5, self.over)))
                }
                return sdf.result * self.opacity
            }
        }
    }

    /** The meter stood on end: a narrow column filling bottom to top, with
     * no lamp, for a rack of channels beside one another. */
    mod.widgets.LevelMeterColumn = mod.widgets.LevelMeter{
        width: 10
        height: 96
        vertical: true
        lamp: false
    }
}

/// A level meter's actions.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum LevelMeterAction {
    /// A press put the latched lamp out.
    Cleared,
    #[default]
    None,
}

/// How a reading READS, as against what it measures.
///
/// Instant on the way up, a dt-correct exponential fall on the way down, and
/// a mark that dwells before following the bar. It holds no clock and no
/// widget: `tick` is handed the seconds since the last one, which is what
/// lets a test hold it to its own arithmetic, and what keeps the fall the
/// same speed on a machine that is servicing the meter late.
///
/// The three parameters are public because they are the host's to choose and
/// a widget copies its own live properties over them between ticks.
#[derive(Clone, Copy, Debug)]
pub struct MeterBallistics {
    /// The fall's TIME CONSTANT in seconds, which is not the time a fall
    /// takes: `tick` covers `1 - e^(-dt/release_secs)` of the distance left,
    /// so one of these is about two thirds of the way down and nine tenths
    /// of a fall is ln(10) — about 2.3 — of them. The default 0.74 is the
    /// 1.7 s nine-tenths fall `RELEASE_SECS` describes.
    pub release_secs: f32,
    /// Seconds the mark stands before it starts to follow the bar down.
    pub hold_secs: f32,
    /// The exponent the reading is DRAWN on. One is linear. Below one lifts
    /// the quiet end of the scale, which is where a linear bar spends almost
    /// nothing; above one does the opposite. It lives here rather than in
    /// the drawing because the deadband below has to be judged on the value
    /// that reaches the screen.
    pub taper: f32,
    level: f32,
    hold: f32,
    /// Seconds the mark has stood where it is.
    held_for: f32,
    /// The bar and mark last handed out, and whether any ever were.
    pushed: Option<(f32, f32)>,
}

impl Default for MeterBallistics {
    fn default() -> Self {
        Self::new(RELEASE_SECS, HOLD_SECS, 1.0)
    }
}

impl MeterBallistics {
    pub fn new(release_secs: f32, hold_secs: f32, taper: f32) -> Self {
        Self {
            release_secs,
            hold_secs,
            taper,
            level: 0.0,
            hold: 0.0,
            held_for: 0.0,
            pushed: None,
        }
    }

    /// One tick. `reading` is the highest value seen since the last tick, as
    /// a share of full scale; `dt` the seconds since it.
    pub fn tick(&mut self, reading: f32, dt: f32) {
        // Both guards live here rather than at the call site, because they
        // belong to the arithmetic they protect: every caller has the same
        // two ways to hand over nonsense, a gap for a tick and a reading
        // from a division that had no denominator.
        let dt = if dt.is_finite() { dt.clamp(0.0, LONGEST_TICK) } else { 0.0 };
        // Pinned rather than remembered: a reading past full scale would
        // otherwise decay from wherever it was and hold the bar at the end
        // for a second and a half. That a ceiling was passed is the lamp's
        // news to carry, not the bar's.
        let reading = if reading.is_finite() { reading.clamp(0.0, 1.0) } else { 0.0 };
        // 1 - e^(-dt/tau): the share of the remaining distance to cover in
        // this tick. Being a function of dt is the whole point — the tick is
        // not evenly spaced, and a per-tick constant would make the speed of
        // the fall a function of how busy the machine is.
        let fall = 1.0 - (-dt / self.release_secs.max(0.001)).exp();

        if reading >= self.level {
            self.level = reading;
        } else {
            self.level += (reading - self.level) * fall;
            if self.level < SILENCE {
                self.level = 0.0;
            }
        }

        if self.level >= self.hold {
            self.hold = self.level;
            self.held_for = 0.0;
        } else {
            self.held_for += dt;
            if self.held_for >= self.hold_secs {
                // It follows the bar down rather than dropping to meet it: a
                // mark that jumps reads as a new event, which is the one
                // thing it is not.
                self.hold += (self.level - self.hold) * fall;
                if self.hold < SILENCE {
                    self.hold = 0.0;
                }
            }
        }
    }

    /// Stand the bar and the mark at `reading` without running anything:
    /// what a meter shows before any host has fed it one, and what a page
    /// laid out in the DSL is asking for when it writes a level.
    pub fn seed(&mut self, reading: f32) {
        let reading = if reading.is_finite() { reading.clamp(0.0, 1.0) } else { 0.0 };
        self.level = reading;
        self.hold = reading;
        self.held_for = 0.0;
        self.pushed = None;
    }

    /// Empty, and with nothing owed to whoever is drawing it.
    pub fn reset(&mut self) {
        self.level = 0.0;
        self.hold = 0.0;
        self.held_for = 0.0;
        self.pushed = None;
    }

    /// The reading as it was measured, ballistics and all, on the scale it
    /// was given in. This is the one to PRINT; the taper is for drawing.
    pub fn value(&self) -> f32 {
        self.level.clamp(0.0, 1.0)
    }

    /// The bar, on the scale it is drawn on.
    pub fn level(&self) -> f32 {
        self.tapered(self.level)
    }

    /// The high-water mark, on the same scale.
    pub fn hold(&self) -> f32 {
        self.tapered(self.hold)
    }

    /// Nothing left to animate: the bar is out and the mark has followed it
    /// down. A host may stop ticking here and lose nothing.
    pub fn is_settled(&self) -> bool {
        self.level == 0.0 && self.hold == 0.0
    }

    /// The pair to draw, or `None` when nothing has moved enough to see.
    ///
    /// This is what makes a meter cheap. Asking for a redraw marks the pass
    /// for repaint, so a meter that reports every tick repaints the window
    /// twenty times a second forever, including with nothing happening. It
    /// reports the DISPLAY values, because that is where being visible is
    /// decided — a threshold on the measured value is blind at the quiet end
    /// of a lifted scale and twitchy at the loud end.
    pub fn take_push(&mut self) -> Option<(f32, f32)> {
        let now = (self.level(), self.hold());
        // Both, because the mark moves on its own: while the bar stands
        // still at a steady reading the mark is still coming down over it,
        // and a deadband watching only the bar would freeze it there.
        let moved = match self.pushed {
            None => true,
            Some((level, hold)) => {
                (now.0 - level).abs() >= DEADBAND || (now.1 - hold).abs() >= DEADBAND
            }
        };
        // The last step to nothing always goes out. A meter left standing a
        // deadband above zero is a meter saying something is still there.
        let landed = self.pushed.is_some_and(|(level, hold)| level > 0.0 || hold > 0.0)
            && now == (0.0, 0.0);
        (moved || landed).then(|| {
            self.pushed = Some(now);
            now
        })
    }

    fn tapered(&self, value: f32) -> f32 {
        let value = value.clamp(0.0, 1.0);
        if self.taper == 1.0 {
            value
        } else {
            value.powf(self.taper.clamp(0.05, 20.0))
        }
    }
}

/// The meter's shader. The instances are the whole of what changes while it
/// runs: where the bar stands, where the mark stands, whether the lamp is
/// lit, whether it wears one, and which way the scale runs.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLevelMeter {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    level: f32,
    #[live]
    hold: f32,
    #[live]
    over: f32,
    #[live]
    lamp: f32,
    #[live]
    vertical: f32,
    #[live]
    opacity: f32,
}

/// A bar for a live value that falls. The host feeds readings in; the widget
/// owns the ballistics, the frames they need and nothing else.
#[derive(Script, ScriptHook, Widget)]
pub struct LevelMeter {
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
    draw_bg: DrawLevelMeter,
    /// A standing reading for a meter nothing is feeding. Writing it again
    /// stands the bar and the mark there; it is not where a fed meter is.
    #[live]
    pub level: f64,
    /// The scale runs bottom to top instead of left to right.
    #[live]
    pub vertical: bool,
    /// Wear the out-of-range lamp at the loud end.
    #[live(true)]
    pub lamp: bool,
    /// The lamp, lit. `set_over` latches this on; only clearing puts it out.
    #[live]
    pub over: bool,
    /// The fall's time constant in seconds, not the time a fall takes: nine
    /// tenths of a fall is about 2.3 of them. `MeterBallistics::release_secs`
    /// has the arithmetic. Nothing acts on it until something feeds the
    /// meter; a standing `level` never ticks.
    #[live(0.74)]
    pub release_secs: f64,
    /// Seconds the mark stands where it is before it follows the bar down.
    #[live(1.0)]
    pub hold_secs: f64,
    /// The exponent the reading is DRAWN on; one is linear, below one lifts
    /// the quiet end. It never touches what `value` reports.
    #[live(1.0)]
    pub taper: f64,
    /// A press anywhere on the meter puts the lamp out.
    #[live(true)]
    pub clear_on_press: bool,
    /// Greys the meter and stops a press from clearing the lamp. The
    /// readings still land: a disabled meter is one nobody may touch, not
    /// one that has stopped listening.
    #[live]
    pub disabled: bool,
    #[rust]
    ballistics: MeterBallistics,
    /// The highest reading fed since the last tick. A host feeding faster
    /// than the screen draws would otherwise lose the very spikes the
    /// instant attack exists to catch.
    #[rust]
    pending: f32,
    /// The `level` the bar was last stood at, so a script apply that changes
    /// it — the controls panel, a page that writes a reading in its DSL — is
    /// noticed at draw time by comparing the two.
    #[rust]
    seeded: f64,
    #[rust]
    last_tick: f64,
    #[rust]
    running: bool,
    #[rust]
    next_frame: NextFrame,
}

impl LevelMeter {
    /// Hand over the highest reading since the last one, as a share of full
    /// scale, and start the fall if it is not already running.
    pub fn feed(&mut self, cx: &mut Cx, reading: f64) {
        self.pending = self.pending.max(reading as f32);
        // Not running means settled — that is the only way the loop stops —
        // so a reading of nothing has nothing to animate and starts no
        // frames. This is the whole of what keeps an idle meter free.
        if !self.running && self.pending > 0.0 {
            self.running = true;
            self.last_tick = cx.seconds_since_app_start();
            self.next_frame = cx.new_next_frame();
        }
    }

    /// Take the host's out-of-range reading. True latches the lamp; false is
    /// ignored, because the event that lit it was over before the frame was.
    pub fn set_over(&mut self, cx: &mut Cx, over: bool) {
        if over && !self.over {
            self.over = true;
            self.draw_bg.redraw(cx);
        }
    }

    /// Put the lamp out. The bar is left alone: what it is showing is still
    /// true, and only the report of the ceiling was being acknowledged.
    pub fn clear(&mut self, cx: &mut Cx) {
        if self.over {
            self.over = false;
            self.draw_bg.redraw(cx);
        }
    }

    /// Empty the meter and put the lamp out: a channel that has been taken
    /// away, rather than one that has gone quiet.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.ballistics.reset();
        self.pending = 0.0;
        self.over = false;
        self.draw_bg.redraw(cx);
    }

    /// The measured reading, ballistics and all — the one to print.
    pub fn value(&self) -> f64 {
        self.ballistics.value() as f64
    }

    /// The parameters are live properties, so a controls panel or a script
    /// apply may move them between ticks; they are copied over rather than
    /// captured once.
    fn sync(&mut self) {
        self.ballistics.release_secs = self.release_secs as f32;
        self.ballistics.hold_secs = self.hold_secs as f32;
        self.ballistics.taper = self.taper as f32;
    }
}

impl Widget for LevelMeter {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync();
        if self.level != self.seeded {
            self.seeded = self.level;
            self.ballistics.seed(self.level as f32);
        }
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.level = self.ballistics.level();
        self.draw_bg.hold = self.ballistics.hold();
        self.draw_bg.over = if self.over { 1.0 } else { 0.0 };
        self.draw_bg.lamp = if self.lamp { 1.0 } else { 0.0 };
        self.draw_bg.vertical = if self.vertical { 1.0 } else { 0.0 };
        // The same dim as the bars, rings and dial in `progress.rs`: a meter
        // beside a disabled bar that was fainter than it would read as a
        // second state rather than the same one.
        self.draw_bg.opacity = if self.disabled { 0.6 } else { 1.0 };
        self.draw_bg.draw_abs(cx, rect);
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.next_frame.is_event(event) {
            let dt = (ne.time - self.last_tick).max(0.0) as f32;
            self.last_tick = ne.time;
            self.sync();
            let reading = std::mem::take(&mut self.pending);
            self.ballistics.tick(reading, dt);
            if let Some((level, hold)) = self.ballistics.take_push() {
                self.draw_bg.level = level;
                self.draw_bg.hold = hold;
                self.draw_bg.redraw(cx);
            }
            // Settled means the bar is out and the mark has followed it
            // down, so there is nothing left to animate and the meter stops
            // asking for frames until the next reading wakes it.
            if self.ballistics.is_settled() {
                self.running = false;
            } else {
                self.next_frame = cx.new_next_frame();
            }
        }
        if self.disabled || !self.clear_on_press {
            return;
        }
        let uid = self.uid;
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                if self.over {
                    self.clear(cx);
                    cx.widget_action(uid, LevelMeterAction::Cleared);
                }
            }
            _ => (),
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!("{:.2}", self.ballistics.value()))
    }
}

impl LevelMeterRef {
    pub fn feed(&self, cx: &mut Cx, reading: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.feed(cx, reading);
        }
    }

    pub fn set_over(&self, cx: &mut Cx, over: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_over(cx, over);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }

    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset(cx);
        }
    }

    /// Whether the lamp is lit.
    pub fn over(&self) -> bool {
        self.borrow().map(|inner| inner.over).unwrap_or(false)
    }

    /// The measured reading — the one to print.
    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }

    /// Where the bar stands, on the scale it is drawn on.
    pub fn level(&self) -> f64 {
        self.borrow()
            .map(|inner| inner.ballistics.level() as f64)
            .unwrap_or(0.0)
    }

    pub fn cleared(&self, actions: &Actions) -> bool {
        matches!(
            actions.find_widget_action(self.widget_uid()).map(|a| a.cast()),
            Some(LevelMeterAction::Cleared)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default parameters, which is what the DSL hands the widget.
    fn meter() -> MeterBallistics {
        MeterBallistics::default()
    }

    /// Missing a spike is the one thing a peak meter may not do, and the
    /// fall it is read by is the other half of the same bargain.
    #[test]
    fn the_bar_takes_a_rise_at_once_and_gives_it_up_on_a_schedule() {
        let mut m = meter();
        m.tick(1.0, 0.05);
        assert_eq!(m.level(), 1.0, "a reading has to be there the tick it arrives");
        // Nine tenths of the way down in 1.7 s, by the constant's definition.
        for _ in 0..34 {
            m.tick(0.0, 0.05);
        }
        assert!(
            (m.level() - 0.1).abs() < 0.005,
            "1.7 s should be nine tenths of the fall, which reads 0.100, got {:.3}",
            m.level()
        );
        // And a rise on the way down is taken at once too, or the meter
        // under-reads exactly when the thing it watches gets busy again.
        m.tick(0.8, 0.05);
        assert_eq!(m.level(), 0.8, "the rise is instant wherever the bar was");
    }

    /// The reason the fall is a time constant and not a per-tick figure. A
    /// meter's tick arrives late, coalesces, and stops entirely while
    /// another page is up; a per-tick constant would make how fast the bar
    /// falls a function of how busy the machine is.
    #[test]
    fn the_fall_is_the_same_however_often_the_meter_is_ticked() {
        let mut often = meter();
        let mut seldom = meter();
        often.tick(1.0, 0.01);
        seldom.tick(1.0, 0.01);
        for _ in 0..100 {
            often.tick(0.0, 0.01);
        }
        for _ in 0..4 {
            seldom.tick(0.0, 0.25);
        }
        assert!(
            (often.level() - seldom.level()).abs() < 1e-3,
            "one second is one second: {:.4} against {:.4}",
            often.level(),
            seldom.level()
        );
        // A gap longer than any real tick is clamped, not obeyed: coming
        // back to a page that was not being painted would otherwise look
        // like a cut rather than a fall.
        let mut returned = meter();
        returned.tick(1.0, 0.05);
        returned.tick(0.0, 30.0);
        assert!(returned.level() > 0.0, "a huge gap is clamped");
        assert!(returned.level() < 1.0, "but it is still a fall");
        // Nor is nonsense obeyed. A reading that is not a number is not a
        // reading: it counts as nothing and the bar falls as it would for a
        // silence, rather than poisoning the bar with a NaN forever.
        let mut nonsense = meter();
        nonsense.tick(1.0, 0.05);
        nonsense.tick(f32::NAN, 0.05);
        assert!(
            nonsense.level() > 0.9 && nonsense.level() < 1.0,
            "one tick of silence, no more and no less: {:.4}",
            nonsense.level()
        );
        // A gap that is not a number is no time at all, so nothing moves.
        let stood_at = nonsense.level();
        nonsense.tick(0.0, f32::NAN);
        assert_eq!(nonsense.level(), stood_at, "an unusable gap moves nothing");
    }

    /// The mark is for the glance that was somewhere else.
    #[test]
    fn the_mark_stands_a_moment_and_then_rides_the_bar_down() {
        let mut m = meter();
        m.tick(1.0, 0.05);
        assert_eq!(m.hold(), 1.0);
        for _ in 0..18 {
            m.tick(0.0, 0.05);
        }
        assert_eq!(m.hold(), 1.0, "still standing at 0.9 s");
        assert!(m.level() < 0.4, "while the bar has gone: {:.3}", m.level());
        for _ in 0..20 {
            m.tick(0.0, 0.05);
        }
        assert!(m.hold() < 1.0, "and after a second it follows");
        assert!(m.hold() >= m.level(), "never below the bar it marks");
        // A higher reading reclaims it at once and restarts the standing.
        m.tick(1.0, 0.05);
        assert_eq!(m.hold(), 1.0);
    }

    /// A reading past full scale is the lamp's news, not the bar's: pinned
    /// rather than remembered, or one spike would hold the bar at the end
    /// for a second and a half of decay nobody can see.
    #[test]
    fn a_reading_past_the_end_of_the_scale_is_pinned_not_remembered() {
        let mut m = meter();
        m.tick(4.0, 0.05);
        assert_eq!(m.value(), 1.0);
        for _ in 0..34 {
            m.tick(0.0, 0.05);
        }
        assert!(
            (m.level() - 0.1).abs() < 0.005,
            "1.7 s from the end, exactly as if it had been a 1.0: {:.3}",
            m.level()
        );
        // And a negative one is nothing, not an absolute value.
        let mut below = meter();
        below.tick(-0.5, 0.05);
        assert_eq!(below.level(), 0.0);
    }

    /// The taper is a drawing decision and must not touch the measurement.
    #[test]
    fn the_taper_moves_where_a_reading_is_drawn_and_not_what_it_is() {
        let mut lifted = MeterBallistics::new(RELEASE_SECS, HOLD_SECS, 0.5);
        lifted.tick(0.25, 0.05);
        assert_eq!(lifted.level(), 0.5, "a quarter is drawn at half height");
        assert_eq!(lifted.value(), 0.25, "and is still a quarter when printed");
        let mut linear = meter();
        linear.tick(0.25, 0.05);
        assert_eq!(linear.level(), 0.25, "linear is the default");
        // Both ends are fixed points of any exponent, so the scale still
        // means the same thing at the places it is read against.
        let mut ends = MeterBallistics::new(RELEASE_SECS, HOLD_SECS, 0.5);
        ends.tick(1.0, 0.05);
        assert_eq!(ends.level(), 1.0);
        ends.reset();
        assert_eq!(ends.level(), 0.0);
    }

    /// Asking for a redraw marks the pass for repaint, so a meter that
    /// reports every tick repaints the window twenty times a second forever.
    #[test]
    fn a_settled_meter_stops_asking_to_be_drawn() {
        let mut m = meter();
        m.tick(1.0, 0.05);
        assert!(m.take_push().is_some(), "a meter that has never reported does");
        // Ten seconds of nothing: long enough for the bar AND the mark,
        // which stands a whole second before it even starts down.
        let mut last = None;
        let mut pushes = 0;
        for _ in 0..200 {
            m.tick(0.0, 0.05);
            if let Some(push) = m.take_push() {
                last = Some(push);
                pushes += 1;
            }
        }
        assert_eq!(last, Some((0.0, 0.0)), "the last step to nothing always goes out");
        assert!(m.is_settled());
        assert!(pushes < 200, "and it stopped reporting well before the end");
        for _ in 0..40 {
            m.tick(0.0, 0.05);
            assert_eq!(m.take_push(), None, "a settled meter reports nothing at all");
        }
    }

    /// The deadband watches both numbers, because they move independently:
    /// a steady reading holds the bar still while the mark is still coming
    /// down over it, and a gate watching only the bar would freeze it there.
    ///
    /// The bar is made to stand EXACTLY still first, which is what makes
    /// this test about the mark. A bar merely asymptoting onto a steady
    /// reading is still crossing the deadband on its own for most of a
    /// minute, so a count taken from the top would come back healthy with
    /// the mark's half of the gate deleted. Dropping the bar BELOW the
    /// reading first puts every later tick through the instant-attack
    /// branch, which stands it on the reading to the bit: from there the
    /// only thing that can move is the mark, and with the hold half of the
    /// gate removed this window reports nothing at all.
    #[test]
    fn the_mark_gets_through_the_deadband_while_the_bar_stands_still() {
        let mut m = meter();
        m.tick(1.0, 0.05);
        for _ in 0..12 {
            m.tick(0.0, 0.05);
        }
        assert!(m.level() < 0.5, "the bar is under the reading: {:.3}", m.level());
        // From here a steady half is a rise every tick, so the bar is stood
        // at exactly it and stays there while the mark comes down over it.
        m.tick(0.5, 0.05);
        let bar = m.level();
        assert_eq!(bar, 0.5);
        m.take_push();
        let mark_before = m.hold();
        let mut moved = 0;
        for _ in 0..60 {
            m.tick(0.5, 0.05);
            assert_eq!(m.level(), bar, "the bar has not moved a bit");
            if m.take_push().is_some() {
                moved += 1;
            }
        }
        assert!(moved > 0, "the mark's journey over a still bar was reported");
        assert!(
            m.hold() < mark_before - DEADBAND,
            "and it was a journey: {:.3} to {:.3}",
            mark_before,
            m.hold()
        );
        assert!(m.hold() > m.level(), "the mark is still above it: {:.3}", m.hold());
    }

    /// A seeded meter is a picture, not a measurement: it stands where it
    /// was put and runs nothing, which is what a page being laid out wants.
    #[test]
    fn a_seeded_meter_stands_where_it_was_put_and_settles_nothing() {
        let mut m = meter();
        m.seed(0.7);
        assert_eq!(m.level(), 0.7);
        assert_eq!(m.hold(), 0.7, "the mark stands with it, not on the floor");
        assert!(!m.is_settled(), "so a widget showing one still has work to do");
        m.seed(2.0);
        assert_eq!(m.level(), 1.0, "and a seed past the end is pinned like a reading");
        m.reset();
        assert!(m.is_settled());
        assert_eq!(m.hold(), 0.0);
    }
}
