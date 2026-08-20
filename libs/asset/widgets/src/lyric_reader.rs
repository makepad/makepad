//! The lyrics reader: a whole track's transcript as a scrollable,
//! playhead-following list.
//!
//! Factored out of the VJ's music surface (`apps/vj/src/music_view.rs`) so
//! the asset UI's audio preview shows the same panel over the same data —
//! typed [`LyricRow`]s in, [`LyricEvent`]s out, per the widget-layer policy:
//! the host owns the transport and the transcript, the widget only draws and
//! reports. The current line sits mid-panel with an accent bar, its words
//! fill as the song crosses them (when the row is `confident`), a wheel
//! pauses following for a few seconds ([`LYRIC_FOLLOW_RESUME_SECS`]) and a
//! click on a line asks the host to seek.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LyricReaderBase = #(LyricReader::register_widget(vm))
    mod.widgets.LyricReader = set_type_default() do mod.widgets.LyricReaderBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x0c1116
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 6.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        // The line being sung: a lit plate, and a bar down the left edge in
        // the reader's accent colour.
        draw_row +: {
            color: uniform(#x18242e)
            edge: uniform(#x3ee0b0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 3.0)
                sdf.fill(self.color)
                let px = self.pos.x * self.rect_size.x
                let bar = 1.0 - smoothstep(1.6, 2.6, px)
                return sdf.result + vec4(self.edge.xyz * bar, bar)
            }
        }
        draw_text +: {
            color: #xd6dee6
            text_style: theme.font_bold{font_size: 8}
        }
        draw_time +: {
            color: #x55636f
            text_style: theme.font_regular{font_size: 7}
        }
        // Where the panel is in the transcript, and whether it is still
        // following the song: bright while the operator is browsing, a faint
        // tick while the playhead owns it.
        draw_thumb +: {
            color: uniform(#x3ee0b0)
            browsing: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 1.5)
                sdf.fill(vec4(self.color.xyz, mix(0.18, 0.85, self.browsing)))
                return sdf.result
            }
        }
    }
}


/// One transcribed line, ready to draw: the words, when they are sung, and
/// the `m:ss` stamp shown beside them.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricRow {
    pub start_secs: f64,
    pub end_secs: f64,
    pub text: String,
    pub stamp: String,
    /// One start time per word, so the reader's green fill hops exactly as
    /// the program's subtitle does. Empty on an older cache entry.
    pub words: Vec<f64>,
    /// Whether those times are trusted enough to hop on; false sweeps.
    pub confident: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LyricEvent {
    /// A line was clicked: put the needle at its first word.
    Seek { secs: f64 },
}

/// Height of one line in the reader, in points.
const LYRIC_ROW_H: f64 = 17.0;
/// Width of the timestamp gutter.
const LYRIC_STAMP_W: f64 = 34.0;
/// Depth granted to each text pass, so an overdrawn run is not eaten by the
/// depth test (see `views::VideoProgram::draw_lyric_row` — same law).
const LYRIC_DEPTH_STEP: f32 = 0.01;
/// How long the reader stays where the operator put it after the last wheel
/// notch, before the playhead takes the scroll back. Long enough to read a
/// verse ahead, short enough that a forgotten scroll heals itself.
pub const LYRIC_FOLLOW_RESUME_SECS: f64 = 4.0;

/// The transcript, beside the deck that is playing it.
///
/// This is the karaoke display's debug view and its reading copy at once: the
/// whole track's lines with their timestamps, scrolling continuously against
/// the playhead so the current line sits in the middle of the panel, the line
/// being sung lit with the accent bar the waveform's playhead uses, and its
/// words turning green as the song crosses them. Clicking a line seeks there,
/// which is how a timing question gets answered in one gesture.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct LyricReader {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_row: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_time: DrawText,
    #[live]
    draw_thumb: DrawQuad,
    #[rust]
    area: Area,
    #[rust]
    lines: Vec<LyricRow>,
    #[rust]
    position: f64,
    /// What the panel says when there is no transcript yet.
    #[rust]
    placeholder: String,
    /// Each line word-wrapped to the panel's width, and the width it was
    /// wrapped for. The column is narrow and a lyric line is a phrase, so a
    /// line may take two rows — truncating it would make the reader useless
    /// as a reading copy. Cached because it costs a text layout per word.
    #[rust]
    wrapped: Vec<Vec<String>>,
    #[rust]
    wrapped_for: f64,
    /// Top of each line in list pixels, and the scroll the last draw used,
    /// so a click lands on the line the operator actually sees.
    #[rust]
    tops: Vec<f64>,
    #[rust]
    drawn_scroll: f64,
    /// Where the operator scrolled to, in list pixels, while auto-follow is
    /// paused. `None` = following the playhead.
    #[rust]
    manual_scroll: Option<f64>,
    /// App-clock seconds of the last wheel notch, so following resumes by
    /// itself after [`LYRIC_FOLLOW_RESUME_SECS`].
    #[rust]
    manual_at: f64,
    /// Armed while the reader is browsing, so the resume happens on its own
    /// even if nothing else in the app asks for a frame.
    #[rust]
    follow_frame: NextFrame,
    #[rust]
    events: Vec<LyricEvent>,
}

impl LyricReader {
    pub fn set_lines(&mut self, cx: &mut Cx, lines: Vec<LyricRow>) {
        if self.lines == lines {
            return;
        }
        self.lines = lines;
        self.wrapped.clear();
        self.wrapped_for = 0.0;
        self.area.redraw(cx);
    }

    pub fn set_placeholder(&mut self, cx: &mut Cx, text: &str) {
        if self.placeholder == text {
            return;
        }
        self.placeholder = text.to_string();
        self.area.redraw(cx);
    }

    pub fn set_position(&mut self, cx: &mut Cx, secs: f64) {
        if (self.position - secs).abs() < 1e-4 {
            return;
        }
        self.position = secs;
        self.area.redraw(cx);
    }

    pub fn take_events(&mut self) -> Vec<LyricEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The line being sung at `secs`, if any.
    pub fn current(&self) -> Option<usize> {
        current_line(&self.lines, self.position)
    }

    /// Where the list should sit, in line units, for the playhead.
    ///
    /// Continuous, not stepped: inside a line it advances with the words and
    /// across a gap it glides to the next one, so the panel scrolls the way
    /// the waveform does rather than jumping every few seconds.
    fn float_index(&self) -> f64 {
        float_index(&self.lines, self.position)
    }

    /// Whether the reader is currently BROWSING (the operator scrolled) or
    /// following the playhead.
    pub fn is_following(&self) -> bool {
        self.manual_scroll.is_none()
    }

    /// Give the scroll back to the playhead. Called when the resume timeout
    /// passes and when a click seeks — a seek re-centres by definition.
    fn resume_follow(&mut self) {
        self.manual_scroll = None;
    }

    /// Scroll range the auto-follow itself can produce, so browsing can
    /// never leave the transcript.
    fn scroll_bounds(&self, view_h: f64) -> (f64, f64) {
        lyric_scroll_bounds(self.tops.last().copied().unwrap_or(0.0), view_h)
    }

    /// Wheel/trackpad scrolling: browse freely, and pause auto-follow for
    /// [`LYRIC_FOLLOW_RESUME_SECS`] after the last notch. (Dragging is NOT a
    /// scroll gesture here — VJ law: a drag belongs to a control.)
    fn scroll_by(&mut self, cx: &mut Cx, delta: f64, now: f64) {
        if self.lines.is_empty() || delta.abs() < f64::EPSILON {
            return;
        }
        let view_h = self.area.rect(cx).size.y;
        let (min, max) = self.scroll_bounds(view_h);
        let from = self.manual_scroll.unwrap_or(self.drawn_scroll);
        self.manual_scroll = Some((from + delta).clamp(min, max));
        self.manual_at = now;
        debug_assert!(lyric_still_browsing(self.manual_at, now));
        self.follow_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    /// A hairline on the right edge: how far through the transcript the
    /// panel sits, and — by its brightness — whether the operator or the
    /// playhead is driving it. Nothing to grab: the wheel is the control.
    fn draw_scroll_thumb(&mut self, cx: &mut Cx2d, rect: Rect, scroll: f64) {
        let content = self.tops.last().copied().unwrap_or(0.0);
        if content <= rect.size.y || rect.size.y < LYRIC_ROW_H * 2.0 {
            return;
        }
        let (min, max) = self.scroll_bounds(rect.size.y);
        let span = (max - min).max(1.0);
        let at = ((scroll - min) / span).clamp(0.0, 1.0);
        let height = (rect.size.y * (rect.size.y / content)).max(12.0);
        let top = rect.pos.y + at * (rect.size.y - height);
        let browsing = f32::from(u8::from(!self.is_following()));
        self.draw_thumb.set_uniform(cx, live_id!(browsing), &[browsing]);
        self.draw_thumb.draw_abs(
            cx,
            Rect {
                pos: dvec2(rect.pos.x + rect.size.x - 4.0, top),
                size: dvec2(2.0, height),
            },
        );
    }

    /// Word-wrap every line for the panel's current width, and rebuild the
    /// list geometry. Cached: only a new transcript or a resize pays for it.
    fn rewrap(&mut self, cx: &mut Cx2d, width: f64) {
        if self.wrapped.len() == self.lines.len() && (self.wrapped_for - width).abs() < 0.5 {
            return;
        }
        self.wrapped = self
            .lines
            .iter()
            .map(|line| wrap_to_width(&self.draw_text, cx, &line.text, width))
            .collect();
        self.wrapped_for = width;
        self.tops = Vec::with_capacity(self.wrapped.len() + 1);
        let mut at = 0.0;
        for rows in &self.wrapped {
            self.tops.push(at);
            at += rows.len().max(1) as f64 * LYRIC_ROW_H;
        }
        self.tops.push(at);
    }
}

/// Greedy word wrap against measured text width.
fn wrap_to_width(draw: &DrawText, cx: &mut Cx2d, text: &str, max_width: f64) -> Vec<String> {
    let measure = |cx: &mut Cx2d, text: &str| -> f64 {
        let laid = draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        (laid.size_in_lpxs.width * draw.font_scale) as f64
    };
    if max_width <= 0.0 || measure(cx, text) <= max_width {
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
        if measure(cx, &candidate) > max_width {
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

/// Index of the line covering `secs`, or `None` in a gap.
pub fn current_line(lines: &[LyricRow], secs: f64) -> Option<usize> {
    let after = lines.partition_point(|line| line.start_secs <= secs);
    let index = after.checked_sub(1)?;
    (secs < lines[index].end_secs).then_some(index)
}

/// Continuous list position for a playhead: `i` at the start of line `i`,
/// `i + 1` at the start of line `i + 1`, linear in between.
pub fn float_index(lines: &[LyricRow], secs: f64) -> f64 {
    if lines.is_empty() {
        return 0.0;
    }
    let after = lines.partition_point(|line| line.start_secs <= secs);
    let Some(index) = after.checked_sub(1) else {
        return 0.0;
    };
    if index + 1 >= lines.len() {
        return index as f64;
    }
    let from = lines[index].start_secs;
    let to = lines[index + 1].start_secs;
    if to <= from {
        return index as f64;
    }
    index as f64 + ((secs - from) / (to - from)).clamp(0.0, 1.0)
}

/// Scroll range a transcript `content` pixels tall can occupy in a panel
/// `view_h` tall — exactly the range the playhead's own centring produces
/// (head 0..content, panel centred on it), so browsing can reach every line
/// the song would have scrolled to and no further.
pub fn lyric_scroll_bounds(content: f64, view_h: f64) -> (f64, f64) {
    let half = (view_h - LYRIC_ROW_H) * 0.5;
    (-half, (content - half).max(-half))
}

/// Where a wheel notch of `delta` leaves a panel sitting at `from`.
pub fn lyric_browse_to(from: f64, delta: f64, content: f64, view_h: f64) -> f64 {
    let (min, max) = lyric_scroll_bounds(content, view_h);
    (from + delta).clamp(min, max)
}

/// Whether the panel is still the operator's at `now`, given the last wheel
/// notch at `at`. Auto-follow takes it back after
/// [`LYRIC_FOLLOW_RESUME_SECS`] — a click that seeks gives it back at once.
pub fn lyric_still_browsing(at: f64, now: f64) -> bool {
    now - at < LYRIC_FOLLOW_RESUME_SECS
}

/// `m:ss` for the gutter.
pub fn lyric_stamp(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

impl WidgetNode for LyricReader {
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

impl Widget for LyricReader {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Browsing heals itself: once the operator has stopped scrolling for
        // LYRIC_FOLLOW_RESUME_SECS the playhead takes the panel back. Driven
        // from a frame the reader asks for, so it happens whether or not
        // anything else in the app is animating.
        if self.follow_frame.is_event(event).is_some() {
            if self.manual_scroll.is_some() {
                let now = cx.seconds_since_app_start();
                if !lyric_still_browsing(self.manual_at, now) {
                    self.resume_follow();
                } else {
                    self.follow_frame = cx.new_next_frame();
                }
                self.area.redraw(cx);
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let rect = self.area.rect(cx);
                let row = ((fe.abs.y - rect.pos.y + self.drawn_scroll) / LYRIC_ROW_H).floor();
                if row >= 0.0 {
                    if let Some(line) = self.lines.get(row as usize) {
                        self.events.push(LyricEvent::Seek { secs: line.start_secs });
                        // The seek re-centres the panel, so browsing ends
                        // here rather than fighting the new playhead.
                        self.resume_follow();
                    }
                }
            }
            // Wheel / trackpad: browse the transcript. The panel keeps what
            // the operator chose until the resume timeout; a drag is still
            // not a scroll (VJ law).
            Hit::FingerScroll(fe) => {
                let delta = if fe.scroll.y.abs() > f64::EPSILON {
                    fe.scroll.y
                } else {
                    fe.scroll.x
                };
                let now = cx.seconds_since_app_start();
                self.scroll_by(cx, delta, now);
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) if !self.lines.is_empty() => {
                cx.set_cursor(MouseCursor::Hand);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.draw_bg.draw_abs(cx, rect);
        if rect.size.y < LYRIC_ROW_H || rect.size.x < 40.0 {
            return DrawStep::done();
        }
        if self.lines.is_empty() {
            if !self.placeholder.is_empty() {
                self.draw_time.draw_abs(
                    cx,
                    dvec2(rect.pos.x + 8.0, rect.pos.y + 8.0),
                    &self.placeholder,
                );
            }
            return DrawStep::done();
        }
        let text_x = rect.pos.x + 6.0 + LYRIC_STAMP_W;
        let text_w = (rect.pos.x + rect.size.x - 6.0 - text_x).max(20.0);
        self.rewrap(cx, text_w);

        // Centre the playhead's position IN THE LIST; the list slides
        // continuously behind it, the way the waveform scrolls under its
        // fixed head, rather than jumping a line at a time.
        let float = self.float_index();
        let index = (float.floor() as usize).min(self.lines.len() - 1);
        let head = self.tops[index]
            + (float - index as f64) * (self.tops[index + 1] - self.tops[index]);
        let follow = head - (rect.size.y - LYRIC_ROW_H) * 0.5;
        // Browsing wins over the playhead until it times out; the bounds are
        // re-clamped here because a rewrap can change the list's height
        // under a scroll taken before it.
        let scroll = match self.manual_scroll {
            Some(at) => {
                let (min, max) = self.scroll_bounds(rect.size.y);
                at.clamp(min, max)
            }
            None => follow,
        };
        self.drawn_scroll = scroll;

        let current = self.current();
        // The same word-hopping fill the program subtitle uses, from the same
        // helper — one karaoke contract, two surfaces.
        let progress = current
            .map(|index| {
                let line = &self.lines[index];
                sung_fraction(
                    line.start_secs,
                    line.end_secs,
                    &line.text,
                    &line.words,
                    line.confident,
                    self.position,
                ) as f64
            })
            .unwrap_or(0.0);
        let mut depth = 0.0f32;
        for index in 0..self.lines.len() {
            let top = rect.pos.y + self.tops[index] - scroll;
            let height = self.tops[index + 1] - self.tops[index];
            // Whole lines only: the panel has no scroll clip of its own, and
            // a half-drawn line spilling past the edge reads as a bug.
            if top < rect.pos.y - 0.5 || top + height > rect.pos.y + rect.size.y + 0.5 {
                continue;
            }
            let live = current == Some(index);
            if live {
                self.draw_row.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(rect.pos.x + 2.0, top),
                        size: dvec2(rect.size.x - 4.0, height - 1.0),
                    },
                );
            }
            self.draw_time.draw_abs(
                cx,
                dvec2(rect.pos.x + 6.0, top + 4.0),
                &self.lines[index].stamp,
            );
            // Text first and WHOLE — the reader never reveals a line a letter
            // at a time — then the sung part again over it in the accent, on
            // its own depth slice so the overdraw survives the depth test.
            let ahead = if live {
                vec4(0.90, 0.94, 0.98, 1.0)
            } else {
                vec4(0.55, 0.62, 0.69, 1.0)
            };
            let rows = std::mem::take(&mut self.wrapped[index]);
            let total: usize = rows.iter().map(|row| row.chars().count()).sum();
            let mut sung = if live {
                (progress * total as f64).round() as usize
            } else {
                0
            };
            for (row_index, row) in rows.iter().enumerate() {
                let y = top + row_index as f64 * LYRIC_ROW_H + 3.0;
                self.draw_text.color = ahead;
                self.draw_text.draw_depth = depth;
                depth += LYRIC_DEPTH_STEP;
                self.draw_text.draw_abs(cx, dvec2(text_x, y), row);
                let take = sung.min(row.chars().count());
                sung -= take;
                if take == 0 {
                    continue;
                }
                let split = row
                    .char_indices()
                    .nth(take)
                    .map(|(at, _)| at)
                    .unwrap_or(row.len());
                self.draw_text.color = vec4(0.243, 0.878, 0.690, 1.0);
                self.draw_text.draw_depth = depth;
                depth += LYRIC_DEPTH_STEP;
                self.draw_text.draw_abs(cx, dvec2(text_x, y), &row[..split]);
            }
            self.wrapped[index] = rows;
        }
        self.draw_text.draw_depth = 0.0;
        self.draw_scroll_thumb(cx, rect, scroll);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// the karaoke fill contract
// ---------------------------------------------------------------------------

/// How long a word takes to fill once its moment arrives. Long enough to
/// read as a hop rather than a pop, short enough that the fill is never what
/// the eye is tracking — the WORD is.
pub const WORD_HOP_SECS: f64 = 0.08;

/// How far across a line's characters the fill has reached at `secs` — the
/// ONE karaoke fill used by every surface (the VJ program subtitle, the VJ
/// reader, the asset UI preview). `words` holds a start time per whitespace
/// word of `text`; word times are only hopped on when `confident` and at
/// least two are present, else the fill degrades to a linear sweep.
///
/// With word times this is a staircase: the word whose moment has come fills
/// over [`WORD_HOP_SECS`] and then holds until the next one, so the fill
/// hops the way a karaoke bouncing ball does and STOPS through the breath or
/// the instrumental beat in the middle of a line. A word held well past the
/// line's own pace fills across its whole interval instead — slowly, ending
/// exactly where the hop would have, so nothing jumps.
pub fn sung_fraction(
    start_secs: f64,
    end_secs: f64,
    text: &str,
    words: &[f64],
    confident: bool,
    secs: f64,
) -> f32 {
    if secs <= start_secs {
        return 0.0;
    }
    if !(confident && words.len() >= 2) {
        let span = (end_secs - start_secs).max(1e-3);
        return (((secs - start_secs) / span).clamp(0.0, 1.0)) as f32;
    }
    let at = words.partition_point(|start| *start <= secs);
    let Some(index) = at.checked_sub(1) else {
        return 0.0;
    };
    // Character offsets measured on the REAL string, not on a reconstruction
    // of it: the renderers colour `text[..n]` and split the line by character
    // count, so the fraction has to be in exactly those units or the boundary
    // drifts a character per word.
    let (before, width, total) = word_span(text, index);
    if total == 0 {
        return 0.0;
    }
    let ends = words.get(index + 1).copied().unwrap_or(end_secs).max(words[index]);
    let span = ends - words[index];
    let fill = if span > sustain_threshold(end_secs, words) {
        span
    } else {
        WORD_HOP_SECS.min(span.max(1e-3))
    };
    let hop = ((secs - words[index]) / fill.max(1e-3)).clamp(0.0, 1.0);
    (((before as f64 + hop * width as f64) / total as f64).clamp(0.0, 1.0)) as f32
}

/// How long a word has to last before it counts as held rather than sung at
/// this line's pace. The reference is the line's own lower-quartile
/// interval, so a slow ballad is not treated as one long sustain and a fast
/// verse is not judged against somebody else's tempo.
fn sustain_threshold(end_secs: f64, words: &[f64]) -> f64 {
    if words.len() < 2 {
        return f64::INFINITY;
    }
    let mut spans: Vec<f64> = Vec::with_capacity(words.len());
    for index in 0..words.len() {
        let ends = words.get(index + 1).copied().unwrap_or(end_secs).max(words[index]);
        spans.push(ends - words[index]);
    }
    spans.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pace = spans[spans.len() / 4];
    (pace * 1.2).max(WORD_HOP_SECS * 3.0)
}

/// `(chars before word `index`, chars the word covers, chars in the line)`.
/// The covered width includes the space after the word, so a filled word
/// leaves no unpainted gap before the next one.
fn word_span(text: &str, index: usize) -> (usize, usize, usize) {
    let mut at = 0usize;
    let mut word = 0usize;
    let mut start: Option<usize> = None;
    let mut found: Option<(usize, usize)> = None;
    for character in text.chars() {
        if character.is_whitespace() {
            if let Some(from) = start.take() {
                if word == index {
                    found = Some((from, at - from + 1));
                }
                word += 1;
            }
        } else if start.is_none() {
            start = Some(at);
        }
        at += 1;
    }
    if let Some(from) = start {
        if word == index {
            found = Some((from, at - from));
        }
    }
    let (before, width) = found.unwrap_or((at, 0));
    (before, width, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The reader's browse rule: the wheel moves the panel inside the range
    /// the playhead itself could have scrolled to, and never past the ends
    /// of the transcript.
    fn browsing_the_transcript_stays_inside_it() {
        // 40 lines of one row each in a 200px panel.
        let content = 40.0 * LYRIC_ROW_H;
        let view = 200.0;
        let (min, max) = lyric_scroll_bounds(content, view);
        assert!(min < 0.0, "the first line can sit in the middle: {min}");
        assert!(max > min);
        assert_eq!(lyric_browse_to(0.0, -10_000.0, content, view), min, "top stop");
        assert_eq!(lyric_browse_to(0.0, 10_000.0, content, view), max, "bottom stop");
        let stepped = lyric_browse_to(0.0, 40.0, content, view);
        assert!((stepped - 40.0).abs() < 1e-9, "a notch moves by its own delta");
        // The range is exactly what the playhead's own centring covers: the
        // head runs from the first line to the last, so a short transcript
        // browses over its own height and no further.
        assert!((max - min - content).abs() < 1e-9, "{min}..{max} for {content}px");
        let (short_min, short_max) = lyric_scroll_bounds(2.0 * LYRIC_ROW_H, view);
        assert!((short_max - short_min - 2.0 * LYRIC_ROW_H).abs() < 1e-9);
    }

    #[test]
    /// Auto-follow pauses while the operator scrolls and comes back by
    /// itself — the state machine, without a Cx.
    fn following_pauses_while_browsing_and_resumes_after_the_timeout() {
        let at = 100.0;
        assert!(lyric_still_browsing(at, at), "the notch itself");
        assert!(lyric_still_browsing(at, at + LYRIC_FOLLOW_RESUME_SECS - 0.01));
        assert!(!lyric_still_browsing(at, at + LYRIC_FOLLOW_RESUME_SECS));
        assert!(!lyric_still_browsing(at, at + 60.0));
        // Every further notch extends the pause from ITS own time.
        let later = at + LYRIC_FOLLOW_RESUME_SECS - 0.5;
        assert!(lyric_still_browsing(later, later + 1.0));
    }

    #[test]
    fn the_fill_hops_and_sweeps() {
        // Evenly paced words: each fills in a quick hop, then HOLDS until
        // the next word's moment.
        let words = [1.0, 1.2, 1.4, 1.6];
        let line = |secs| sung_fraction(1.0, 1.8, "a bb cc dd", &words, true, secs);
        assert_eq!(line(0.5), 0.0);
        let after_first = line(1.0 + WORD_HOP_SECS + 1e-3);
        assert!(after_first > 0.0);
        // Holds between the end of the hop and the next word.
        assert_eq!(line(1.0 + WORD_HOP_SECS + 1e-3), line(1.2 - 1e-3));
        // Done at the end.
        assert!((line(1.9) - 1.0).abs() < 1e-6);
        // Unconfident: linear sweep.
        let plain = sung_fraction(1.0, 3.0, "a bb ccc", &[], false, 2.0);
        assert!((plain - 0.5).abs() < 1e-6);
    }
}
