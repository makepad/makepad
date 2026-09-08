//! Marquee — a line that travels, and comes round again without a seam.
//!
//! A strip too narrow for what it has to say has three honest answers: cut
//! the words with an ellipsis, wrap them onto another line, or move them.
//! The first two need room the strip does not have. This is the third, and
//! the whole difficulty is the coming round: a line that reaches its end
//! and jumps back to the start reads as a glitch rather than as a loop.
//!
//! **The loop has no seam because there is no single copy.** The line is
//! drawn as many times as it takes to cover the strip at every phase of the
//! travel, each copy one `period` further along, where a period is the line
//! plus the gap after it. Nothing ever "restarts": the offset walks from 0
//! to one period and wraps, and because a copy is always already in place
//! where the previous one is leaving, the wrap is invisible.
//!
//! **How many copies is arithmetic, not two.** Two is only enough when the
//! line is at least as wide as the strip. A short line in a wide strip
//! needs as many as fit plus one, or the strip shows a hole for part of
//! every cycle — which is exactly the bug a two-copy marquee has and a
//! reader notices immediately.
//!
//! It carries text and not arbitrary children on purpose. Drawing the same
//! child widget several times in one frame has no precedent in this library
//! and would give every copy but the last a hit rect that lies; a moving
//! target is not something to click at anyway. If a caller ever needs a row
//! of things to travel, that is a different widget and it can say so.

use crate::{badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*};

/// Which way the line travels.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum MarqueeDirection {
    /// Enters at the right, leaves at the left: the way reading goes, so
    /// the next words arrive where the eye is already going.
    #[pick]
    Left = 0,
    Right = 1,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.MarqueeDirection = set_type_default() do #(MarqueeDirection::script_api(vm))

    use mod.widgets.*

    mod.widgets.DrawMarqueeGroundBase = #(DrawMarqueeGround::script_component(vm))
    set_type_default() do #(DrawMarqueeGround::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // Nothing: the ground exists to own the strip's rect, not to
            // paint it. A widget whose only drawn thing is text placed at
            // absolute positions has no rect of its own to report, to
            // redraw, or to be hovered on.
            return vec4(0.0 0.0 0.0 0.0)
        }
    }

    mod.widgets.MarqueeBase = #(Marquee::register_widget(vm))

    /** A line of text that travels across its strip and loops without a
     * seam: for a name too long for the room, or a status line that has
     * more to say than it can show at once. */
    mod.widgets.Marquee = set_type_default() do mod.widgets.MarqueeBase{
        width: Fill
        // A definite height rather than Fit: every copy is placed at an
        // absolute position, which contributes nothing for a turtle to
        // measure, so a Fit strip would collapse to nothing.
        height: 24.
        /** the words that travel */
        text: ""
        /** how far it travels in a second, in points; 0 holds it still 0..400 step 5 */
        speed: 40.0
        /** which way it goes */
        direction: MarqueeDirection.Left
        /** the room after the line before it comes round again 0..400 step 4 */
        gap: 64.0
        /** hold it still while the pointer is on it, so it can be read 0..1 step 1 */
        pause_on_hover: true

        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMarqueeGround {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Marquee {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The strip's own rect, drawn as nothing. Every copy of the line is
    /// placed absolutely and so contributes no rect for a turtle to
    /// measure; without this the widget would report the last text run as
    /// its area, which is the wrong thing to hover, to redraw and to find
    /// in a snapshot.
    #[redraw]
    #[live]
    draw_bg: DrawMarqueeGround,
    #[live]
    pub draw_text: DrawText,

    #[live]
    pub text: String,
    #[live(40.0)]
    pub speed: f64,
    #[live(MarqueeDirection::Left)]
    pub direction: MarqueeDirection,
    #[live(64.0)]
    pub gap: f64,
    #[live(true)]
    pub pause_on_hover: bool,

    /// How far along the current period the line has travelled.
    #[rust]
    offset: f64,
    /// The clock at the last draw, so travel is measured in seconds and not
    /// in frames — a marquee that moves by a fixed step per frame runs at
    /// whatever speed the machine happens to draw at.
    #[rust]
    last_time: Option<f64>,
    #[rust]
    paused: bool,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
}

/// A period shorter than this is treated as this: a line measured at
/// almost nothing with no gap after it would otherwise ask for an
/// unbounded number of copies.
const PERIOD_MIN: f64 = 4.0;

/// No strip needs more copies than this. Only reachable by asking for a
/// very wide strip with almost nothing in it, and a hole in that case is a
/// better outcome than a frame that never ends.
const COPIES_MAX: usize = 128;

/// How many copies of the line it takes to cover a strip at every phase of
/// the travel.
///
/// The leftmost copy sits between one period behind the strip's edge and
/// the edge itself, so the copies must reach a whole period further than
/// the strip is wide — hence the `+ 1` after the division rather than a
/// bare `ceil`. Two copies is the floor, because one cannot cover a strip
/// while it is leaving it.
pub fn copies_needed(strip: f64, period: f64) -> usize {
    if period <= 0.0 {
        return 2;
    }
    // Capped before the cast, not after: a very wide strip divided by a
    // very short period saturates the cast at the top of the type, and one
    // more than that is an overflow rather than a big number.
    let fit = (strip.max(0.0) / period).ceil().min(COPIES_MAX as f64) as usize;
    (fit + 1).clamp(2, COPIES_MAX)
}

/// Where in the period the travel has got to, kept inside it from either
/// direction.
pub fn wrapped_offset(offset: f64, period: f64) -> f64 {
    if period <= 0.0 {
        return 0.0;
    }
    offset.rem_euclid(period)
}

impl Marquee {
    /// Whether the line is moving right now, which is what a test can ask
    /// rather than watching pixels.
    pub fn travelling(&self) -> bool {
        !self.paused && self.speed != 0.0 && !self.text.is_empty()
    }
}

impl Widget for Marquee {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Clipped, always: the whole point is that the copies run past both
        // edges and only the strip is seen.
        self.draw_bg.begin(
            cx,
            walk,
            Layout {
                clip_x: true,
                clip_y: true,
                ..self.layout
            },
        );
        let strip = cx.turtle().rect();

        let width = if self.text.is_empty() {
            0.0
        } else {
            measure(&self.draw_text, cx, &self.text)
        };
        if width > 0.0 {
            let period = (width + self.gap.max(0.0)).max(PERIOD_MIN);

            let now = cx.seconds_since_app_start();
            // Clamped: a window that was asleep or a pass that took a long
            // time must not teleport the line a screen and a half along.
            let dt = self.last_time.map_or(0.0, |t| (now - t).clamp(0.0, 0.25));
            self.last_time = Some(now);
            if !self.paused && self.speed != 0.0 {
                self.offset = wrapped_offset(self.offset + self.speed * dt, period);
            }

            // Both directions keep the leading copy within one period of
            // the strip's edge; only which way the offset carries it
            // differs, so the covering arithmetic below is the same for
            // both.
            let lead = match self.direction {
                MarqueeDirection::Left => strip.pos.x - self.offset,
                MarqueeDirection::Right => strip.pos.x - period + self.offset,
            };
            for i in 0..copies_needed(strip.size.x, period) {
                self.draw_text.draw_walk(
                    cx,
                    Walk {
                        abs_pos: Some(dvec2(lead + i as f64 * period, strip.pos.y)),
                        width: Size::Fixed(width),
                        height: Size::Fixed(strip.size.y),
                        ..Walk::default()
                    },
                    Align { x: 0.0, y: 0.5 },
                    &self.text,
                );
            }

            if self.travelling() {
                self.next_frame = cx.new_next_frame();
            }
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.redraw(cx);
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                if self.pause_on_hover && !self.paused {
                    self.paused = true;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.paused {
                    self.paused = false;
                    // The clock is dropped rather than kept: the pause was
                    // real time passing, and carrying it across would jump
                    // the line by however long the pointer rested there.
                    self.last_time = None;
                    self.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.offset = 0.0;
            self.redraw(cx);
        }
    }

    /// What the strip is doing, so a test can prove it moves rather than
    /// comparing two pictures of a moving thing.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(format!(
            "{} {:.1}",
            if self.travelling() { "travelling" } else { "held" },
            self.offset
        ))
    }
}

impl MarqueeRef {
    /// Whether the line is moving right now.
    pub fn travelling(&self) -> bool {
        self.borrow().map(|inner| inner.travelling()).unwrap_or(false)
    }

    pub fn set_speed(&self, cx: &mut Cx, speed: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.speed = speed;
            inner.redraw(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line at least as wide as the strip needs the two copies everyone
    /// writes: the one leaving and the one arriving.
    #[test]
    fn a_line_wider_than_its_strip_needs_two_copies() {
        assert_eq!(copies_needed(300.0, 400.0), 2);
        assert_eq!(copies_needed(300.0, 300.0), 2);
    }

    /// A short line in a wide strip needs as many as fit and one more.
    /// This is the case a two-copy marquee gets wrong, and it shows as a
    /// hole crossing the strip once a cycle.
    #[test]
    fn a_short_line_in_a_wide_strip_needs_as_many_as_fit_and_one_more() {
        // Four periods fit across the strip exactly, so five cover it at
        // every phase of the travel.
        assert_eq!(copies_needed(400.0, 100.0), 5);
        // And a strip that is not a whole number of periods rounds up
        // before the spare one is added.
        assert_eq!(copies_needed(450.0, 100.0), 6);
    }

    /// Nothing to say and nowhere to say it still answers, rather than
    /// dividing by zero or asking for every copy there is.
    #[test]
    fn a_degenerate_strip_still_answers() {
        assert_eq!(copies_needed(0.0, 100.0), 2);
        assert_eq!(copies_needed(400.0, 0.0), 2);
        assert_eq!(copies_needed(f64::MAX, 1.0), COPIES_MAX);
    }

    /// The travel walks one period and comes round, from either direction.
    #[test]
    fn the_travel_comes_round_within_one_period() {
        assert_eq!(wrapped_offset(30.0, 100.0), 30.0);
        assert_eq!(wrapped_offset(100.0, 100.0), 0.0, "a whole period is the start again");
        assert_eq!(wrapped_offset(250.0, 100.0), 50.0, "several periods along");
        assert_eq!(
            wrapped_offset(-10.0, 100.0),
            90.0,
            "backwards past the start comes round the other way, not to a negative"
        );
        assert_eq!(wrapped_offset(10.0, 0.0), 0.0, "no period, no travel");
    }
}
