//! The AI song's lyrics, in time with the music: section tags as small
//! headings, each line with its translation under it, the line being sung
//! marked and its sung words lit, scrolled so it stays in view. A click on
//! a line plays from there; a scroll by hand holds the view for a moment.
//!
//! It draws only when something it shows changes: the next word's start
//! is a timeout away, the scroll eases in frames only while it moves.

use crate::lyrics::{tokens, TimedLyrics};
use crate::widgets::text_width;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // `ink`, `muted`, `accent`: the style's panel text, its secondary text and
    // its accent (set with the palette, hw.rs).
    mod.widgets.HwLyricsBase = #(HwLyrics::register_widget(vm))
    mod.widgets.HwLyrics = set_type_default() do mod.widgets.HwLyricsBase{
        width: Fill
        height: Fill
        ink: #x2b2e33
        muted: #x5a5e66
        accent: #x3399ff
        draw_line +: {text_style: theme.font_regular{font_size: 11.5}}
        draw_now +: {text_style: theme.font_bold{font_size: 11.5}}
        draw_trans +: {text_style: theme.font_regular{font_size: 9.5}}
        draw_section +: {text_style: theme.font_bold{font_size: 8.0}}
    }
}

/// How long a scroll by hand holds the view before it follows again (s).
const HOLD_S: f64 = 4.0;
/// Where the sung line sits, as a fraction of the view's height.
const FOCUS: f64 = 0.36;

#[derive(Clone, Debug, Default)]
pub enum HwLyricsAction {
    #[default]
    None,
    /// Play from this time (s): a line was clicked.
    Seek(f64),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Section,
    /// A row of a line's words (the line's index).
    Words(usize),
    Translation,
}

/// One row laid out: what it is, where (relative to the content top), and
/// for a words row the (word index, x) of each word.
struct Row {
    kind: Kind,
    y: f64,
    h: f64,
    text: String,
    words: Vec<(usize, f64)>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct HwLyrics {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_line: DrawText,
    #[live]
    draw_now: DrawText,
    #[live]
    draw_trans: DrawText,
    #[live]
    draw_section: DrawText,
    #[live]
    draw_mark: DrawColor,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    accent: Vec4f,
    #[rust]
    lyrics: Option<TimedLyrics>,
    /// The song shown (its file), to notice a new one.
    #[rust]
    key: String,
    /// Playback: (position s, at app time s), whether it moves, and whether
    /// the song shown is the one playing (only then is a line sung).
    #[rust]
    anchor: (f64, f64),
    #[rust]
    playing: bool,
    #[rust]
    live: bool,
    #[rust]
    rows: Vec<Row>,
    /// The width the rows were laid out for.
    #[rust]
    laid: Option<f64>,
    /// Each line's top and bottom in the content.
    #[rust]
    line_spans: Vec<(f64, f64)>,
    #[rust]
    content_h: f64,
    #[rust]
    scroll: f64,
    #[rust]
    held_until: f64,
    #[rust]
    last_draw: f64,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    timer: Timer,
}

impl HwLyrics {
    fn time(&self, now: f64) -> f64 {
        self.anchor.0 + if self.playing { (now - self.anchor.1).max(0.0) } else { 0.0 }
    }

    fn sung(&self, now: f64) -> Option<(usize, usize)> {
        if !self.live {
            return None;
        }
        self.lyrics.as_ref()?.at(self.time(now))
    }

    fn layout(&mut self, cx: &mut Cx2d, width: f64) {
        self.rows.clear();
        self.line_spans.clear();
        let Some(lyrics) = self.lyrics.clone() else {
            self.content_h = 0.0;
            return;
        };
        let pad = 10.0;
        let line_h = 19.0;
        let trans_h = 15.0;
        let space = text_width(&self.draw_now, cx, " ").max(3.0);
        let mut y = 4.0;
        let mut n = 0;
        for section in &lyrics.sections {
            if !section.name.is_empty() {
                if y > 4.0 {
                    y += 8.0;
                }
                self.rows.push(Row { kind: Kind::Section, y, h: 16.0, text: section.name.to_uppercase(), words: Vec::new() });
                y += 18.0;
            }
            for line in &section.lines {
                let top = y;
                // Wrapped by the bold widths, so the sung line never reflows.
                let mut row: Vec<(usize, f64)> = Vec::new();
                let mut x = 0.0;
                for (w, word) in line.words.iter().enumerate() {
                    let ww = text_width(&self.draw_now, cx, &word.text);
                    if x > 0.0 && x + ww > width - pad {
                        self.rows.push(Row { kind: Kind::Words(n), y, h: line_h, text: String::new(), words: std::mem::take(&mut row) });
                        y += line_h;
                        x = 0.0;
                    }
                    row.push((w, x));
                    x += ww + if word.space { space } else { 0.0 };
                }
                self.rows.push(Row { kind: Kind::Words(n), y, h: line_h, text: String::new(), words: row });
                y += line_h;
                if !line.translation.is_empty() {
                    let space = text_width(&self.draw_trans, cx, " ").max(2.0);
                    let mut text = String::new();
                    let mut x = 0.0;
                    for (word, gap) in tokens(&line.translation) {
                        let ww = text_width(&self.draw_trans, cx, &word);
                        if x > 0.0 && x + ww > width - pad {
                            self.rows.push(Row { kind: Kind::Translation, y, h: trans_h, text: std::mem::take(&mut text), words: Vec::new() });
                            y += trans_h;
                            x = 0.0;
                        }
                        text.push_str(&word);
                        x += ww;
                        if gap {
                            text.push(' ');
                            x += space;
                        }
                    }
                    self.rows.push(Row { kind: Kind::Translation, y, h: trans_h, text, words: Vec::new() });
                    y += trans_h;
                }
                y += 5.0;
                self.line_spans.push((top, y - 5.0));
                n += 1;
            }
        }
        self.content_h = y + 4.0;
    }
}

impl Widget for HwLyrics {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() || self.timer.is_event(event).is_some() {
            self.draw_line.redraw(cx);
        }
        match event.hits(cx, self.area) {
            Hit::FingerScroll(e) => {
                self.scroll += e.scroll.y;
                self.held_until = cx.seconds_since_app_start() + HOLD_S;
                self.draw_line.redraw(cx);
            }
            Hit::FingerUp(e) if e.is_over && e.was_tap() => {
                let y = e.abs.y - e.rect.pos.y + self.scroll;
                let at = self.line_spans.iter().position(|(top, bottom)| y >= *top && y < *bottom);
                if let (Some(at), Some(lyrics)) = (at, &self.lyrics) {
                    if let Some((_, line)) = lyrics.lines().nth(at) {
                        cx.widget_action(self.uid, HwLyricsAction::Seek(line.start));
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout { clip_x: true, clip_y: true, ..Layout::default() });
        let rect = cx.turtle().rect();
        let now = cx.seconds_since_app_start();
        if self.laid != Some(rect.size.x) {
            self.laid = Some(rect.size.x);
            self.layout(cx, rect.size.x);
        }
        let (fg, muted, accent) = (self.ink, self.muted, self.accent);
        let sung = self.sung(now);
        let t = self.time(now);
        // Follow the sung line unless the person scrolled a moment ago.
        let max_scroll = (self.content_h - rect.size.y).max(0.0);
        let mut moving = false;
        if now >= self.held_until {
            if let Some((top, bottom)) = sung.and_then(|(line, _)| self.line_spans.get(line).copied()) {
                let target = ((top + bottom) * 0.5 - rect.size.y * FOCUS).clamp(0.0, max_scroll);
                let dt = (now - self.last_draw).clamp(0.0, 0.1);
                let step = (target - self.scroll) * (1.0 - (-dt * 7.0).exp());
                if (target - self.scroll).abs() > 0.5 && dt > 0.0 {
                    self.scroll += step;
                    moving = true;
                } else {
                    self.scroll = target;
                }
            }
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        self.last_draw = now;
        let lyrics = self.lyrics.clone();
        if let Some(lyrics) = &lyrics {
            let lines: Vec<&crate::lyrics::Line> = lyrics.lines().map(|(_, l)| l).collect();
            let top = rect.pos.y - self.scroll;
            if let Some((top_y, bottom)) = sung.and_then(|(line, _)| self.line_spans.get(line).copied()) {
                self.draw_mark.color = vec4(accent.x, accent.y, accent.z, 0.10);
                self.draw_mark.draw_abs(cx, Rect { pos: dvec2(rect.pos.x, top + top_y - 2.0), size: dvec2(rect.size.x, bottom - top_y + 4.0) });
                self.draw_mark.color = accent;
                self.draw_mark.draw_abs(cx, Rect { pos: dvec2(rect.pos.x, top + top_y - 2.0), size: dvec2(3.0, bottom - top_y + 4.0) });
            }
            let indent = 9.0;
            for row in &self.rows {
                let y = top + row.y;
                if y + row.h < rect.pos.y || y > rect.pos.y + rect.size.y {
                    continue;
                }
                match row.kind {
                    Kind::Section => {
                        self.draw_section.color = accent;
                        self.draw_section.draw_abs(cx, dvec2(rect.pos.x + indent, y + 2.0), &row.text);
                    }
                    Kind::Translation => {
                        self.draw_trans.color = vec4(muted.x, muted.y, muted.z, 0.9);
                        self.draw_trans.draw_abs(cx, dvec2(rect.pos.x + indent, y), &row.text);
                    }
                    Kind::Words(n) => {
                        let Some(line) = lines.get(n) else { continue };
                        let current = sung.is_some_and(|(at, _)| at == n);
                        let past = sung.is_some_and(|(at, _)| n < at);
                        for &(w, x) in &row.words {
                            let Some(word) = line.words.get(w) else { continue };
                            let pos = dvec2(rect.pos.x + indent + x, y);
                            if current {
                                // Sung words in the text colour, the rest of
                                // the line still to come in the secondary one.
                                self.draw_now.color = if word.start <= t { fg } else { muted };
                                self.draw_now.draw_abs(cx, pos, &word.text);
                            } else {
                                self.draw_line.color = if past { muted } else { fg };
                                self.draw_line.draw_abs(cx, pos, &word.text);
                            }
                        }
                    }
                }
            }
            // The next change: the next word's start while it plays.
            if self.live && self.playing {
                let next = lines.iter().flat_map(|l| l.words.iter().map(|w| w.start)).filter(|s| *s > t + 0.01).fold(f64::MAX, f64::min);
                if next < f64::MAX && !moving {
                    cx.stop_timer(self.timer);
                    self.timer = cx.start_timeout((next - t).clamp(0.02, 30.0));
                }
            }
        }
        if moving {
            self.next_frame = cx.new_next_frame();
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl HwLyricsRef {
    /// The song's lyrics (`key`: its file; a new key scrolls to the top).
    pub fn set_lyrics(&self, cx: &mut Cx, key: &str, lyrics: Option<TimedLyrics>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.key == key && inner.lyrics == lyrics {
            return;
        }
        if inner.key != key {
            inner.scroll = 0.0;
            inner.held_until = 0.0;
        }
        inner.key = key.to_string();
        inner.lyrics = lyrics;
        inner.laid = None;
        inner.draw_line.redraw(cx);
    }

    /// Where playback is (`live`: the song shown is the one playing).
    pub fn set_clock(&self, cx: &mut Cx, live: bool, position: f64, playing: bool) {
        let Some(mut inner) = self.borrow_mut() else { return };
        let now = cx.seconds_since_app_start();
        // A small drift of the estimate is not worth a redraw.
        let drift = (inner.time(now) - position).abs();
        if inner.live != live || inner.playing != playing || drift > 0.25 {
            inner.live = live;
            inner.playing = playing;
            inner.anchor = (position, now);
            inner.draw_line.redraw(cx);
        }
    }

    /// A line was clicked: play from this time.
    pub fn seek(&self, actions: &Actions) -> Option<f64> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            HwLyricsAction::Seek(at) => Some(at),
            HwLyricsAction::None => None,
        }
    }
}
