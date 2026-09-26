//! Timeline — a run of events down a line, in the order they happened.
//!
//! A history a reader scans rather than edits: a delivery's progress, a
//! release's milestones, an audit trail. Each event carries a time, a
//! heading, a body and a bullet, and **the line runs between the bullets
//! rather than through them** — a bullet is a stop on the way, not a bead
//! threaded onto a wire that has already been drawn over it. That is the
//! one rule the layout is built around: every segment of the line is a
//! separate quad that starts clear of the bullet above it and stops clear
//! of the bullet below.
//!
//! Alignment is one property. `Trailing` puts every event on one side with
//! the line hugging the other edge, `Leading` mirrors it, `Alternating`
//! puts the line down the middle and swings the events either side of it.
//! `Across` is the same layout turned ninety degrees: the line lies flat and
//! the events hang off it, above or below by the same rule.
//!
//! One event can be marked as where things have got to. It gets a wider
//! bullet in the accent colour, and the line behind it — the part already
//! travelled — is drawn in the accent colour too, so the state of the run is
//! readable without reading a word of it.
//!
//! What it deliberately does NOT do:
//!
//! - **It does not hold widgets.** An event is text this widget measures and
//!   draws itself, which is what lets a bullet sit on the middle of its own
//!   first line and lets a segment stop short of it. An event that needs a
//!   button in it is a row of widgets and not a timeline.
//! - **It does not scroll or recycle rows.** A history taller than the room
//!   goes inside a scrolling parent. One long enough to be worth recycling
//!   wants a list that recycles, which this is not.
//! - **It does not parse or format times.** `time` is whatever string the
//!   caller has already made, so "20 minutes ago" and a full date cost the
//!   same and neither is second-guessed here.
//! - **It does not break words.** A word wider than the room overruns rather
//!   than being cut in half: breaking mid-word needs per-character measuring
//!   and reads worse than the overrun it avoids.

use crate::{badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*};

/// What a bullet looks like.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TimelineBullet {
    /// A solid disc: an event like the rest.
    #[default]
    Filled,
    /// A ring with nothing in it: an event of a lesser kind, or one still
    /// to come. The middle is left clear rather than filled with a
    /// background colour, so the bullet is right on any surface.
    Hollow,
    /// A ring with a glyph inside it. The glyph is drawn from the ordinary
    /// text chain, so a caller has to pick one the chain carries — a plain
    /// letter, a digit, or a mark like `\u{25b2}`.
    Mark(String),
}

/// One event on the line.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TimelineEvent {
    /// Whatever the caller wants read as "when": a clock time, a date, an
    /// age. Empty leaves the line out.
    pub time: String,
    /// The one line that says what happened.
    pub heading: String,
    /// The rest of it, wrapped to the room there is. Empty leaves it out.
    pub body: String,
    pub bullet: TimelineBullet,
}

impl TimelineEvent {
    pub fn new(time: &str, heading: &str, body: &str) -> Self {
        Self {
            time: time.to_string(),
            heading: heading.to_string(),
            body: body.to_string(),
            bullet: TimelineBullet::Filled,
        }
    }

    pub fn with_bullet(mut self, bullet: TimelineBullet) -> Self {
        self.bullet = bullet;
        self
    }
}

/// One event written as a single line: `time | heading | body | bullet`.
///
/// Every field after the first is optional. The bullet word is `dot` for a
/// filled disc, `ring` for a hollow one, or `mark:X` for a ring with the
/// glyph `X` in it; anything else falls back to a filled disc rather than
/// being drawn as a mystery. A `|` cannot appear in the text — it ends the
/// field it is in — which is the price of a list a caller can write in one
/// markup literal instead of four parallel ones.
pub fn parse_event(line: &str) -> TimelineEvent {
    let mut parts = line.split('|');
    let time = parts.next().unwrap_or("").trim().to_string();
    let heading = parts.next().unwrap_or("").trim().to_string();
    let body = parts.next().unwrap_or("").trim().to_string();
    let bullet = parse_bullet(parts.next().unwrap_or("").trim());
    TimelineEvent { time, heading, body, bullet }
}

pub fn parse_events(lines: &[String]) -> Vec<TimelineEvent> {
    lines.iter().map(|line| parse_event(line)).collect()
}

fn parse_bullet(word: &str) -> TimelineBullet {
    match word {
        "ring" => TimelineBullet::Hollow,
        other => match other.strip_prefix("mark:") {
            Some(glyph) if !glyph.is_empty() => TimelineBullet::Mark(glyph.to_string()),
            _ => TimelineBullet::Filled,
        },
    }
}

/// Which event is where things have got to, from the 1-based `at` a caller
/// writes. Zero marks nothing, and so does a number past the end.
///
/// One-based because the alternative is a sentinel: with a 0-based index
/// there is no value that means "nothing is current", and a timeline of a
/// plain history — most of them — has to be able to say that.
pub fn marked_index(at: usize, len: usize) -> Option<usize> {
    (at >= 1 && at <= len).then(|| at - 1)
}

/// Whether event `index` sits on the leading side of the line: the left of
/// a column, the top of a row.
///
/// Alternating starts on the trailing side, so the first event of an
/// alternating timeline sits where every event of a one-sided one does.
pub fn on_leading_side(index: usize, side: TimelineSide) -> bool {
    match side {
        TimelineSide::Trailing => false,
        TimelineSide::Leading => true,
        TimelineSide::Alternating => index % 2 == 1,
    }
}

/// One segment of the line: the run between the bullet at `after` and the
/// one after it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineLink {
    /// The index of the bullet the segment starts below (or right of).
    pub after: usize,
    pub from: f64,
    pub to: f64,
}

/// The segments of the line, given each bullet as `(centre, radius)` along
/// the axis and the clear space to leave either side of one.
///
/// This is the whole of "the line runs between the bullets, not through
/// them": a segment starts past the near bullet's edge and stops before the
/// far one's, and a pair with no room left between them gets no segment at
/// all rather than a backwards one. Radii come in per bullet because the
/// marked event's bullet is wider than the rest and the segments either
/// side of it have to give way by exactly that much.
pub fn link_spans(bullets: &[(f64, f64)], clearance: f64) -> Vec<TimelineLink> {
    let clearance = clearance.max(0.0);
    let mut links = Vec::new();
    for (i, pair) in bullets.windows(2).enumerate() {
        let (near, near_r) = pair[0];
        let (far, far_r) = pair[1];
        let from = near + near_r + clearance;
        let to = far - far_r - clearance;
        if to > from {
            links.push(TimelineLink { after: i, from, to });
        }
    }
    links
}

/// `text` broken into lines no wider than `room`, measured by `width_of`.
///
/// A word wider than the room gets a line to itself and overruns it; see
/// the module doc for why that is preferred to cutting the word. A room of
/// zero or less is taken to mean "not known yet" and yields a single line,
/// because the alternative — one word per line — looks like a bug and is
/// what an unresolved turtle would otherwise produce.
pub fn wrap_lines(text: &str, room: f64, mut width_of: impl FnMut(&str) -> f64) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if !(room > 0.0) {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
            continue;
        }
        let wider = format!("{line} {word}");
        if width_of(&wider) <= room {
            line = wider;
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// How tall a line's box is, as a multiple of the size of the text in it.
/// The text styles set `line_spacing: 1.0` and the lines are stacked here
/// instead, so the box a line is centred in is the same one the next line
/// starts below.
const LINE_BOX: f64 = 1.45;

/// Where a line's ink starts inside its line box, as a fraction of the
/// size: `draw_abs` takes the top of the LINE box, not of the ink.
const INK_TOP: f64 = 0.30;

/// How much wider the marked bullet is than the rest. Wide enough to be
/// seen at a glance, narrow enough that it does not eat the gap between
/// the line and the text beside it.
const HERE_SCALE: f64 = 1.4;

/// The clear space between a bullet's edge and the segment that runs up to
/// it. Fixed rather than scaled: it is the gap the eye reads as "the line
/// stops here", and it reads the same whatever the bullet's size.
const LINK_CLEARANCE: f64 = 3.0;

/// The `draw_abs` y that centres one line of text of `size` in a box
/// `height` tall whose top is at `top`.
fn text_top(top: f64, height: f64, size: f64) -> f64 {
    top + (height - size) * 0.5 - size * INK_TOP
}

/// Which of the three text layers a laid-out line belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ink {
    Time,
    Heading,
    Body,
}

/// One line of an event, measured and ready to place.
struct Line {
    ink: Ink,
    text: String,
    width: f64,
    height: f64,
}

/// One event, measured: the lines it draws and the room they need.
struct Block {
    lines: Vec<Line>,
    /// The height of the first line's box. The bullet is centred on it, so
    /// a bullet lines up with the first thing the event says rather than
    /// with the middle of a paragraph.
    first: f64,
    height: f64,
    width: f64,
    bullet: TimelineBullet,
}

/// What a timeline reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TimelineAction {
    /// An event was pressed, by its index in the list.
    Picked(usize),
    #[default]
    None,
}

/// Which way the line runs.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum TimelineAxis {
    /// Down the page, events stacked.
    #[pick]
    #[default]
    Down,
    /// Across the page, events hanging off the line.
    Across,
}

/// Which side of the line the events sit on.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum TimelineSide {
    /// All of them after the line: to its right going down, below it going
    /// across. The line hugs the near edge.
    #[pick]
    #[default]
    Trailing,
    /// All of them before the line, mirrored. The line hugs the far edge.
    Leading,
    /// Either side in turn, with the line down the middle. The first event
    /// takes the trailing side.
    Alternating,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawTimelineGroundBase = #(DrawTimelineGround::script_component(vm))
    set_type_default() do #(DrawTimelineGround::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // The timeline's own rect, painted as nothing: every bullet,
            // segment and line of text is placed absolutely, so without
            // this the widget has no rect to be redrawn, hit or found by.
            return vec4(0.0 0.0 0.0 0.0)
        }
    }

    mod.widgets.DrawTimelineRailBase = #(DrawTimelineRail::script_component(vm))
    set_type_default() do #(DrawTimelineRail::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.DrawTimelineBulletBase = #(DrawTimelineBullet::script_component(vm))
    set_type_default() do #(DrawTimelineBullet::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.TimelineAxis = #(TimelineAxis::script_api(vm))
    mod.widgets.TimelineSide = #(TimelineSide::script_api(vm))

    mod.widgets.TimelineBase = #(Timeline::register_widget(vm))

    /** A run of events down a line, in the order they happened: a bullet, a
     * time, a heading and a body apiece, with the line running between the
     * bullets rather than through them. */
    mod.widgets.Timeline = set_type_default() do mod.widgets.TimelineBase{
        width: Fill
        height: Fit

        /** the events, one string each: "time | heading | body | bullet" */
        events: []
        /** which event is where things have got to, counting from one; 0 marks none 0..40 step 1 */
        at: 0
        /** which way the line runs: Down or Across */
        axis: mod.widgets.TimelineAxis.Down
        /** which side the events sit on: Trailing, Leading or Alternating */
        side: mod.widgets.TimelineSide.Trailing
        /** how wide a bullet is 6..32 step 1 */
        bullet_size: 13.
        /** how thick the line is 1..6 step 0.5 */
        line_width: 2.
        /** the room between the line and an event 4..48 step 1 */
        gap: 12.
        /** the room between one event and the next 0..64 step 1 */
        event_gap: 14.

        /** The line: a plain strip, one segment per gap between bullets. */
        draw_rail +: {
            // `done` is the instance: it rides in the draw struct, so every
            // other property here has to be a uniform or the instance slot
            // stops lining up with the struct.
            done: 0.0
            /** the line still to come */
            color_line: uniform(theme.color_outline_variant)
            /** the line already travelled, behind the marked event */
            color_done: uniform(theme.color_primary)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let mut ink = self.color_line
                if self.done > 0.5 { ink = self.color_done }
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                sdf.fill(ink)
                return sdf.result
            }
        }

        /** A bullet: a disc, or a ring with the middle left clear. */
        draw_bullet +: {
            filled: 0.0
            here: 0.0
            /** an ordinary bullet */
            color_dot: uniform(theme.color_outline)
            /** the bullet of the event things have got to */
            color_here: uniform(theme.color_primary)
            /** how thick a hollow bullet's ring is 0.5..4 step 0.5 */
            ring_width: uniform(1.5)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let mx = self.rect_size.x * 0.5
                let my = self.rect_size.y * 0.5
                let mut ink = self.color_dot
                if self.here > 0.5 { ink = self.color_here }
                // Half a point off the edge, so the anti-aliased rim has
                // somewhere to be inside the quad it was given.
                let outer = min(mx, my) - 0.5
                if self.filled > 0.5 {
                    sdf.circle(mx, my, outer)
                    sdf.fill(ink)
                } else {
                    // The stroke straddles the circle, so the ring is laid
                    // half a width in to keep it inside the same quad.
                    sdf.circle(mx, my, outer - self.ring_width * 0.5)
                    sdf.stroke(ink, self.ring_width)
                }
                return sdf.result
            }
        }

        /** When it happened. */
        draw_time +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.type_label_s_size line_spacing: 1.0}
        }
        /** What happened. */
        draw_heading +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.type_label_l_size line_spacing: 1.0}
        }
        /** The rest of it. */
        draw_body +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.type_body_s_size line_spacing: 1.0}
        }
        /** The glyph inside a `mark:` bullet. */
        draw_mark +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.type_label_s_size line_spacing: 1.0}
        }
    }

    /** The line down the middle, events either side of it in turn. */
    mod.widgets.TimelineAlternating = mod.widgets.Timeline{
        side: mod.widgets.TimelineSide.Alternating
    }

    /** The line lying flat, events hanging below it. */
    mod.widgets.TimelineAcross = mod.widgets.Timeline{
        axis: mod.widgets.TimelineAxis.Across
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTimelineGround {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTimelineRail {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    done: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTimelineBullet {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    filled: f32,
    #[live]
    here: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Timeline {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The timeline's own rect, drawn as nothing — see the shader's comment.
    #[redraw]
    #[live]
    draw_bg: DrawTimelineGround,
    #[live]
    draw_rail: DrawTimelineRail,
    #[live]
    draw_bullet: DrawTimelineBullet,
    #[live]
    pub draw_time: DrawText,
    #[live]
    pub draw_heading: DrawText,
    #[live]
    pub draw_body: DrawText,
    #[live]
    pub draw_mark: DrawText,

    /// The events as markup writes them, one string apiece.
    #[live]
    pub events: Vec<String>,
    /// Where things have got to, counting from one; 0 marks nothing.
    #[live(0)]
    pub at: usize,
    #[live]
    pub axis: TimelineAxis,
    #[live]
    pub side: TimelineSide,
    #[live(13.0)]
    pub bullet_size: f64,
    #[live(2.0)]
    pub line_width: f64,
    #[live(12.0)]
    pub gap: f64,
    #[live(14.0)]
    pub event_gap: f64,

    #[rust]
    parsed: Vec<TimelineEvent>,
    /// The strings `parsed` was made from, so the widget can tell a list it
    /// has already read from one markup has just handed it again.
    #[rust]
    from_lines: Vec<String>,
    /// Where each event was drawn, in order, for the press.
    #[rust]
    hits: Vec<Rect>,
    #[rust]
    area: Area,
}

impl Timeline {
    /// The events, replacing whatever markup gave. The markup list is
    /// dropped with them: leaving it would let the next draw take it back.
    pub fn set_events(&mut self, cx: &mut Cx, events: Vec<TimelineEvent>) {
        if self.parsed != events {
            self.parsed = events;
            self.events.clear();
            self.from_lines.clear();
            self.redraw(cx);
        }
    }

    pub fn events(&self) -> &[TimelineEvent] {
        &self.parsed
    }

    /// Mark where things have got to, counting from one. Zero marks nothing.
    pub fn set_at(&mut self, cx: &mut Cx, at: usize) {
        if self.at != at {
            self.at = at;
            self.redraw(cx);
        }
    }

    fn ink(&mut self, ink: Ink) -> &mut DrawText {
        match ink {
            Ink::Time => &mut self.draw_time,
            Ink::Heading => &mut self.draw_heading,
            Ink::Body => &mut self.draw_body,
        }
    }

    fn ink_size(&self, ink: Ink) -> f64 {
        let draw_text = match ink {
            Ink::Time => &self.draw_time,
            Ink::Heading => &self.draw_heading,
            Ink::Body => &self.draw_body,
        };
        draw_text.text_style.font_size as f64
    }

    fn bullet_radius(&self, index: usize, marked: Option<usize>) -> f64 {
        let size = if marked == Some(index) {
            self.bullet_size * HERE_SCALE
        } else {
            self.bullet_size
        };
        (size * 0.5).max(1.0)
    }

    /// The diameter of the widest bullet there could be, which is what the
    /// line's own row has to be tall enough for.
    fn widest_bullet(&self, marked: Option<usize>) -> f64 {
        if marked.is_some() {
            self.bullet_size * HERE_SCALE
        } else {
            self.bullet_size
        }
    }

    /// Every event, measured against the room its text has.
    fn blocks(&self, cx: &mut Cx2d, room: f64) -> Vec<Block> {
        let mut blocks = Vec::with_capacity(self.parsed.len());
        for event in &self.parsed {
            let mut lines: Vec<Line> = Vec::new();
            if !event.time.is_empty() {
                let size = self.ink_size(Ink::Time);
                let width = measure(&self.draw_time, cx, &event.time);
                lines.push(Line {
                    ink: Ink::Time,
                    text: event.time.clone(),
                    width,
                    height: size * LINE_BOX,
                });
            }
            if !event.heading.is_empty() {
                let size = self.ink_size(Ink::Heading);
                let width = measure(&self.draw_heading, cx, &event.heading);
                lines.push(Line {
                    ink: Ink::Heading,
                    text: event.heading.clone(),
                    width,
                    height: size * LINE_BOX,
                });
            }
            if !event.body.is_empty() {
                let size = self.ink_size(Ink::Body);
                let wrapped = wrap_lines(&event.body, room, |run: &str| {
                    measure(&self.draw_body, cx, run)
                });
                for text in wrapped {
                    let width = measure(&self.draw_body, cx, &text);
                    lines.push(Line {
                        ink: Ink::Body,
                        text,
                        width,
                        height: size * LINE_BOX,
                    });
                }
            }
            let height: f64 = lines.iter().map(|line| line.height).sum();
            let width = lines.iter().map(|line| line.width).fold(0.0_f64, f64::max);
            // An event with nothing in it still takes its bullet's room,
            // or the line collapses onto itself where the event was.
            let first = lines.first().map(|line| line.height).unwrap_or(self.bullet_size);
            blocks.push(Block {
                lines,
                first,
                height: height.max(first),
                width,
                bullet: event.bullet.clone(),
            });
        }
        blocks
    }

    /// The room above the first event's first line that its bullet needs,
    /// for a bullet taller than the line it is centred on.
    fn top_pad(&self, blocks: &[Block], marked: Option<usize>) -> f64 {
        match blocks.first() {
            Some(block) => (self.bullet_radius(0, marked) - block.first * 0.5).max(0.0),
            None => 0.0,
        }
    }

    /// How tall the two sides of an `Across` line are: the leading side
    /// above it and the trailing side below.
    fn stack_heights(&self, blocks: &[Block]) -> (f64, f64) {
        let mut above = 0.0_f64;
        let mut below = 0.0_f64;
        for (index, block) in blocks.iter().enumerate() {
            if on_leading_side(index, self.side) {
                above = above.max(block.height);
            } else {
                below = below.max(block.height);
            }
        }
        (above, below)
    }

    /// The segments of the line. `cross` is the line's own coordinate on
    /// the other axis: the x it runs down, or the y it runs along.
    fn draw_links(&mut self, cx: &mut Cx2d, bullets: &[(f64, f64)], marked: Option<usize>, cross: f64) {
        for link in link_spans(bullets, LINK_CLEARANCE) {
            // A segment counts as travelled when it arrives at the marked
            // bullet or at one before it; the segments after it are still
            // to come.
            let done = matches!(marked, Some(here) if link.after < here);
            self.draw_rail.done = if done { 1.0 } else { 0.0 };
            let thickness = self.line_width.max(0.5);
            let rect = match self.axis {
                TimelineAxis::Down => Rect {
                    pos: dvec2(cross - thickness * 0.5, link.from),
                    size: dvec2(thickness, link.to - link.from),
                },
                TimelineAxis::Across => Rect {
                    pos: dvec2(link.from, cross - thickness * 0.5),
                    size: dvec2(link.to - link.from, thickness),
                },
            };
            self.draw_rail.draw_abs(cx, rect);
        }
    }

    fn draw_bullets(&mut self, cx: &mut Cx2d, bullets: &[(f64, f64)], blocks: &[Block], marked: Option<usize>, cross: f64) {
        for (index, &(along, radius)) in bullets.iter().enumerate() {
            let pos = match self.axis {
                TimelineAxis::Down => dvec2(cross - radius, along - radius),
                TimelineAxis::Across => dvec2(along - radius, cross - radius),
            };
            let rect = Rect { pos, size: dvec2(radius * 2.0, radius * 2.0) };
            let bullet = &blocks[index].bullet;
            self.draw_bullet.here = if marked == Some(index) { 1.0 } else { 0.0 };
            self.draw_bullet.filled = if *bullet == TimelineBullet::Filled { 1.0 } else { 0.0 };
            self.draw_bullet.draw_abs(cx, rect);
            if let TimelineBullet::Mark(glyph) = bullet {
                let size = self.draw_mark.text_style.font_size as f64;
                let width = measure(&self.draw_mark, cx, glyph);
                let x = rect.pos.x + (rect.size.x - width) * 0.5;
                let y = text_top(rect.pos.y, rect.size.y, size);
                self.draw_mark.draw_abs(cx, dvec2(x, y), glyph);
            }
        }
    }

    fn draw_down(&mut self, cx: &mut Cx2d, rect: Rect, blocks: &[Block]) {
        let marked = marked_index(self.at, blocks.len());
        let half = self.widest_bullet(marked) * 0.5;
        // The line hugs whichever edge the events are not on, and takes the
        // middle when they are on both.
        let rail_x = match self.side {
            TimelineSide::Trailing => rect.pos.x + half,
            TimelineSide::Leading => rect.pos.x + rect.size.x - half,
            TimelineSide::Alternating => rect.pos.x + rect.size.x * 0.5,
        };
        let lead_edge = rail_x - half - self.gap;
        let trail_edge = rail_x + half + self.gap;

        let mut bullets: Vec<(f64, f64)> = Vec::with_capacity(blocks.len());
        let mut y = rect.pos.y + self.top_pad(blocks, marked);
        for (index, block) in blocks.iter().enumerate() {
            bullets.push((y + block.first * 0.5, self.bullet_radius(index, marked)));
            let leading = on_leading_side(index, self.side);
            let mut line_y = y;
            for line in &block.lines {
                let size = self.ink_size(line.ink);
                // The leading side is set against the line, so its text is
                // ranged right: ragged edges on both sides of the line
                // would read as two lists rather than one.
                let x = if leading { lead_edge - line.width } else { trail_edge };
                let pos = dvec2(x, text_top(line_y, line.height, size));
                self.ink(line.ink).draw_abs(cx, pos, &line.text);
                line_y += line.height;
            }
            self.hits.push(Rect {
                pos: dvec2(rect.pos.x, y),
                size: dvec2(rect.size.x, block.height),
            });
            y += block.height + self.event_gap;
        }
        self.draw_links(cx, &bullets, marked, rail_x);
        self.draw_bullets(cx, &bullets, blocks, marked, rail_x);
    }

    fn draw_across(&mut self, cx: &mut Cx2d, rect: Rect, blocks: &[Block]) {
        let count = blocks.len();
        let marked = marked_index(self.at, count);
        let column = rect.size.x / count.max(1) as f64;
        let bullet = self.widest_bullet(marked);
        let (above, _below) = self.stack_heights(blocks);
        let rail_y = rect.pos.y
            + above
            + if above > 0.0 { self.gap } else { 0.0 }
            + bullet * 0.5;

        let mut bullets: Vec<(f64, f64)> = Vec::with_capacity(count);
        for (index, block) in blocks.iter().enumerate() {
            let mid_x = rect.pos.x + column * (index as f64 + 0.5);
            bullets.push((mid_x, self.bullet_radius(index, marked)));
            let leading = on_leading_side(index, self.side);
            let mut line_y = if leading {
                rail_y - bullet * 0.5 - self.gap - block.height
            } else {
                rail_y + bullet * 0.5 + self.gap
            };
            for line in &block.lines {
                let size = self.ink_size(line.ink);
                // Centred under its own bullet: a column of text hanging
                // off a point wants to be read as belonging to that point.
                let pos = dvec2(mid_x - line.width * 0.5, text_top(line_y, line.height, size));
                self.ink(line.ink).draw_abs(cx, pos, &line.text);
                line_y += line.height;
            }
            self.hits.push(Rect {
                pos: dvec2(mid_x - column * 0.5, rect.pos.y),
                size: dvec2(column, rect.size.y),
            });
        }
        self.draw_links(cx, &bullets, marked, rail_y);
        self.draw_bullets(cx, &bullets, blocks, marked, rail_y);
    }
}

impl Widget for Timeline {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Markup wins whenever its list differs from what was last read, so
        // a live reload and a control that rewrites `events` both land
        // without a hook to catch them.
        if self.from_lines != self.events {
            self.from_lines = self.events.clone();
            self.parsed = parse_events(&self.events);
        }
        let count = self.parsed.len();
        let padding = self.layout.padding;
        // The room has to be known before the turtle begins, because a Fit
        // height cannot be answered until the text has been wrapped to it.
        // A Fit width has no room to resolve, so the text is measured
        // unwrapped and the widget asks for exactly what it would draw.
        let resolved = if walk.width.is_fit() {
            None
        } else {
            cx.turtle().max_width(walk)
        };
        let outer = resolved.map(|width| (width - padding.left - padding.right).max(0.0));
        let room = match self.axis {
            TimelineAxis::Down => {
                outer.map(|width| (width - self.bullet_size - self.gap).max(0.0))
            }
            TimelineAxis::Across => {
                outer.map(|width| (width / count.max(1) as f64 - self.gap).max(0.0))
            }
        };
        let room = room.unwrap_or(f64::INFINITY);

        let blocks = self.blocks(cx, room);
        let marked = marked_index(self.at, count);
        let widest = blocks.iter().map(|block| block.width).fold(0.0_f64, f64::max);
        let (natural_w, natural_h) = if count == 0 {
            (0.0, 0.0)
        } else {
            match self.axis {
                TimelineAxis::Down => {
                    let heights: f64 = blocks.iter().map(|block| block.height).sum();
                    (
                        self.widest_bullet(marked) + self.gap + widest,
                        heights + self.event_gap * (count - 1) as f64
                            + self.top_pad(&blocks, marked),
                    )
                }
                TimelineAxis::Across => {
                    let (above, below) = self.stack_heights(&blocks);
                    (
                        (widest + self.gap) * count as f64,
                        above
                            + if above > 0.0 { self.gap } else { 0.0 }
                            + self.widest_bullet(marked)
                            + if below > 0.0 { self.gap } else { 0.0 }
                            + below,
                    )
                }
            }
        };
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => {
                    Size::Fixed(natural_w + padding.left + padding.right)
                }
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => {
                    Size::Fixed(natural_h + padding.top + padding.bottom)
                }
                other => other,
            },
            ..walk
        };

        self.draw_bg.begin(cx, walk, self.layout);
        let inner = cx.turtle().inner_rect();
        self.hits.clear();
        match self.axis {
            TimelineAxis::Down => self.draw_down(cx, inner, &blocks),
            TimelineAxis::Across => self.draw_across(cx, inner, &blocks),
        }
        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // A timeline is read, not operated: there is no hover state and no
        // cursor change. A press is still reported, because a host that
        // wants an event to open something has nowhere else to hear it.
        if let Hit::FingerDown(fe) = event.hits(cx, self.area) {
            if let Some(index) = self.hits.iter().position(|rect| rect.contains(fe.abs)) {
                let uid = self.uid;
                cx.widget_action(uid, TimelineAction::Picked(index));
            }
        }
    }

    /// The headings in order, so a test can read the run in one line.
    fn text(&self) -> String {
        self.parsed
            .iter()
            .map(|event| event.heading.clone())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl TimelineRef {
    pub fn set_events(&self, cx: &mut Cx, events: Vec<TimelineEvent>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_events(cx, events);
        }
    }

    pub fn set_at(&self, cx: &mut Cx, at: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_at(cx, at);
        }
    }

    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.parsed.len()).unwrap_or(0)
    }

    /// Where things have got to, counting from one; 0 is nothing marked.
    pub fn at(&self) -> usize {
        self.borrow().map(|inner| inner.at).unwrap_or(0)
    }

    /// The event pressed this pass, by its index, if one was.
    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<TimelineAction>() {
            TimelineAction::Picked(index) => Some(index),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A width for a fake font: every character the same, which is enough
    /// to test where the breaks land.
    fn ruler(text: &str) -> f64 {
        text.chars().count() as f64 * 10.0
    }

    #[test]
    fn an_event_line_splits_into_its_four_fields() {
        let event = parse_event("09:00 | Booked | Taken and paid for. | ring");
        assert_eq!(event.time, "09:00");
        assert_eq!(event.heading, "Booked");
        assert_eq!(event.body, "Taken and paid for.");
        assert_eq!(event.bullet, TimelineBullet::Hollow);
    }

    /// Everything after the time is optional, and a line with nothing but a
    /// heading is a perfectly good event.
    #[test]
    fn the_fields_after_the_time_are_optional() {
        let event = parse_event("Packed");
        assert_eq!(event.time, "Packed");
        assert_eq!(event.heading, "");
        assert_eq!(event.body, "");
        assert_eq!(event.bullet, TimelineBullet::Filled);
    }

    /// An unknown bullet word falls back to the ordinary disc rather than
    /// drawing itself as a mystery mark.
    #[test]
    fn a_bullet_word_is_a_disc_a_ring_or_a_glyph() {
        assert_eq!(parse_event("a|b|c|dot").bullet, TimelineBullet::Filled);
        assert_eq!(parse_event("a|b|c|ring").bullet, TimelineBullet::Hollow);
        assert_eq!(
            parse_event("a|b|c|mark:2").bullet,
            TimelineBullet::Mark("2".to_string())
        );
        assert_eq!(parse_event("a|b|c|sparkle").bullet, TimelineBullet::Filled);
        assert_eq!(parse_event("a|b|c|mark:").bullet, TimelineBullet::Filled);
    }

    /// Nothing is current until something says so, and a stale index from a
    /// list that has since shrunk marks nothing rather than the wrong event.
    #[test]
    fn zero_marks_nothing_and_so_does_a_number_past_the_end() {
        assert_eq!(marked_index(0, 3), None);
        assert_eq!(marked_index(1, 3), Some(0));
        assert_eq!(marked_index(3, 3), Some(2));
        assert_eq!(marked_index(4, 3), None);
        assert_eq!(marked_index(1, 0), None);
    }

    /// An alternating run starts where a one-sided run puts everything, so
    /// switching a timeline to alternating never moves its first event.
    #[test]
    fn alternating_starts_on_the_trailing_side() {
        assert!(!on_leading_side(0, TimelineSide::Alternating));
        assert!(on_leading_side(1, TimelineSide::Alternating));
        assert!(!on_leading_side(2, TimelineSide::Alternating));
        assert!(!on_leading_side(7, TimelineSide::Trailing));
        assert!(on_leading_side(7, TimelineSide::Leading));
    }

    /// THE RULE: the line runs between the bullets. Every segment starts
    /// past the bullet above it and stops before the one below, so no
    /// segment ever crosses a bullet's edge.
    #[test]
    fn the_line_stops_short_of_every_bullet() {
        let bullets = [(0.0, 6.0), (60.0, 6.0), (140.0, 6.0)];
        let links = link_spans(&bullets, 3.0);
        assert_eq!(links.len(), 2, "one segment per gap, and none past the ends");
        for link in &links {
            let (near, near_r) = bullets[link.after];
            let (far, far_r) = bullets[link.after + 1];
            assert!(link.from >= near + near_r, "starts clear of the bullet above");
            assert!(link.to <= far - far_r, "stops clear of the bullet below");
            assert!(link.to > link.from, "and runs forwards");
        }
    }

    /// One bullet is a timeline too, and it gets no line at all: a line
    /// with nothing to join is a stray mark.
    #[test]
    fn one_bullet_has_no_line() {
        assert!(link_spans(&[(0.0, 6.0)], 3.0).is_empty());
        assert!(link_spans(&[], 3.0).is_empty());
    }

    /// Two bullets closer than their own radii leave no room for a segment,
    /// and get none rather than one drawn backwards through both of them.
    #[test]
    fn bullets_too_close_together_get_no_segment() {
        assert!(link_spans(&[(0.0, 6.0), (10.0, 6.0)], 3.0).is_empty());
    }

    /// The marked event's bullet is wider than the rest, and the segments
    /// either side of it give way by exactly that much.
    #[test]
    fn a_wider_bullet_pushes_its_own_segments_back() {
        let even = link_spans(&[(0.0, 6.0), (60.0, 6.0), (120.0, 6.0)], 3.0);
        let wide = link_spans(&[(0.0, 6.0), (60.0, 9.0), (120.0, 6.0)], 3.0);
        assert_eq!(wide[0].to, even[0].to - 3.0, "the segment above stops sooner");
        assert_eq!(wide[1].from, even[1].from + 3.0, "the one below starts later");
        assert_eq!(wide[0].from, even[0].from, "and the far ends do not move");
    }

    /// Text that fits is one line, whatever the room to spare.
    #[test]
    fn text_that_fits_is_one_line() {
        assert_eq!(wrap_lines("four five", 500.0, ruler), vec!["four five"]);
    }

    /// The break lands on the last word that fits, not the first that does
    /// not.
    #[test]
    fn wrapping_breaks_at_the_last_word_that_fits() {
        // "one two" is 70 wide, "one two six" is 110.
        assert_eq!(
            wrap_lines("one two six", 100.0, ruler),
            vec!["one two", "six"]
        );
    }

    /// A word wider than the room keeps its own line and overruns it, which
    /// is better read than the same word cut in half.
    #[test]
    fn a_word_wider_than_the_room_overruns_alone() {
        assert_eq!(
            wrap_lines("a considerable word", 60.0, ruler),
            vec!["a", "considerable", "word"]
        );
    }

    /// Nothing to say takes no lines, and a room not yet known takes one:
    /// an unresolved width must not turn a sentence into a word per line.
    #[test]
    fn empty_text_and_unknown_room_both_answer() {
        assert!(wrap_lines("   ", 100.0, ruler).is_empty());
        assert_eq!(wrap_lines("one two six", 0.0, ruler), vec!["one two six"]);
        assert_eq!(wrap_lines("one two six", -5.0, ruler), vec!["one two six"]);
    }
}
