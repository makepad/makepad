//! The instrument choices of a node as a list (the instrument switch's,
//! in `SynthPicker` and on the voice tree's instrument rows): every
//! instrument there is, grouped (the person's own, the synths, the shader
//! instruments, the kits, the General MIDI set by family, the SoundFonts,
//! samples and voices), each folded to its header (how many it holds; the
//! node's own first and marked with what it is now) until opened; typing
//! searches all of them; the group of what it is now starts open. Resting on a row plays it in
//! place, a click switches, the current one is marked. Drawn by the widget
//! that holds it, which gives it the keys while it is open.

use crate::host as synth;
use makepad_score_live::instruments::{Choice, GROUPS};
use makepad_value_pickers::panel::control::{DrawPanelShape, R_CONTROL};
use makepad_widgets::*;

pub const ROW_H: f64 = 20.0;
/// Seconds a row is rested on before it is heard.
const REST: f64 = 0.15;

#[derive(Clone, Debug)]
enum Line {
    /// A group's header: its name, how many it holds (that match).
    Group(&'static str, usize),
    Choice(usize),
}

/// What a click on the list did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clicked {
    /// Not on a row.
    Nothing,
    /// A group folded or unfolded, or the search cleared.
    Folded,
    /// The node switched to the row (one undo step).
    Switched,
}

#[derive(Default)]
pub struct ChoiceList {
    pub node: String,
    choices: Vec<Choice>,
    pub current: Option<String>,
    lines: Vec<Line>,
    /// The groups shown unfolded (the node's own, and those opened).
    unfolded: Vec<&'static str>,
    /// What was typed: the rows of every group that match all its words.
    pub query: String,
    scroll: f64,
    rows: Vec<(Rect, usize)>,
    headers: Vec<(Rect, &'static str)>,
    search: Rect,
    list: Rect,
    /// The row under the pointer and since when; the one heard.
    hover: Option<(usize, f64)>,
    heard: Option<usize>,
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

impl ChoiceList {
    /// Every choice for the node.
    pub fn load(&mut self, node: &str) {
        self.node = node.to_string();
        let (choices, current) = synth::choices(node);
        self.unfolded.clear();
        self.choices = choices;
        self.current = current;
        // What it is now shows among its neighbours, the other groups folded.
        if let Some(own) = self.current.as_ref().and_then(|key| self.choices.iter().find(|c| c.key() == *key)).map(|c| c.group) {
            self.unfolded.push(own);
        }
        self.query.clear();
        self.scroll = 0.0;
        self.relayout();
    }

    /// The lines from the choices, the folds and the search.
    fn relayout(&mut self) {
        let own = self.current.as_ref().and_then(|key| self.choices.iter().find(|c| c.key() == *key)).map(|c| c.group);
        let mut order: Vec<&'static str> = own.into_iter().chain(GROUPS.iter().copied().filter(|g| Some(*g) != own)).collect();
        for c in &self.choices {
            if !order.contains(&c.group) {
                order.push(c.group);
            }
        }
        let words: Vec<String> = self.query.split_whitespace().map(|w| w.to_lowercase()).collect();
        let matches = |c: &Choice| {
            let text = format!("{} {} {}", c.label, c.group, c.detail).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        };
        self.lines.clear();
        for group in order {
            let rows: Vec<usize> = self.choices.iter().enumerate().filter(|(_, c)| c.group == group && matches(c)).map(|(i, _)| i).collect();
            if rows.is_empty() {
                continue;
            }
            self.lines.push(Line::Group(group, rows.len()));
            if !words.is_empty() || self.unfolded.contains(&group) {
                self.lines.extend(rows.into_iter().map(Line::Choice));
            }
        }
        let total = self.lines.len() as f64 * ROW_H;
        self.scroll = self.scroll.clamp(0.0, (total - self.list.size.y + ROW_H).max(0.0));
    }

    /// A key typed while the list is open (the search).
    pub fn type_text(&mut self, text: &str) {
        self.query.push_str(text);
        self.scroll = 0.0;
        self.relayout();
    }

    /// Backspace: the last letter of the search off; true when there was one.
    pub fn backspace(&mut self) -> bool {
        let had = self.query.pop().is_some();
        if had {
            self.relayout();
        }
        had
    }

    pub fn loaded(&self) -> bool {
        !self.choices.is_empty()
    }

    /// What the node is now, in words.
    pub fn current_label(&self) -> String {
        self.current.as_ref().and_then(|key| self.choices.iter().find(|c| c.key() == *key)).map_or_else(|| "(its own)".into(), |c| c.label.clone())
    }

    pub fn stop_preview(&mut self) {
        if self.heard.take().is_some() {
            synth::preview(&self.node, None);
        }
        self.hover = None;
    }

    /// The pointer over the list: true when the row under it changed (a
    /// redraw; `wake` a frame to time the rest).
    pub fn hover_at(&mut self, abs: DVec2) -> (bool, bool) {
        let row = self.rows.iter().find(|(r, _)| r.contains(abs) && self.list.contains(abs)).map(|(_, i)| *i);
        if row == self.hover.map(|(i, _)| i) {
            return (false, false);
        }
        match row {
            Some(i) => {
                self.hover = Some((i, Cx::time_now()));
                (true, true)
            }
            None => {
                self.stop_preview();
                (true, false)
            }
        }
    }

    /// Each frame while hovering: a row rested on long enough is heard.
    /// True while still waiting.
    pub fn tick(&mut self) -> bool {
        let Some((index, since)) = self.hover else { return false };
        if self.heard == Some(index) {
            return false;
        }
        if Cx::time_now() - since >= REST {
            self.heard = Some(index);
            if let Some(choice) = self.choices.get(index) {
                synth::preview(&self.node, Some(choice));
            }
            return false;
        }
        true
    }

    pub fn scroll_by(&mut self, dy: f64) {
        let total = self.lines.len() as f64 * ROW_H;
        self.scroll = (self.scroll + dy).clamp(0.0, (total - self.list.size.y + ROW_H).max(0.0));
    }

    pub fn contains(&self, abs: DVec2) -> bool {
        self.list.contains(abs) || self.search.contains(abs)
    }

    /// A click: a group folded or unfolded, the search cleared, or the
    /// choice switched to (one undo step).
    pub fn click(&mut self, abs: DVec2) -> Clicked {
        if self.search.contains(abs) {
            if !self.query.is_empty() {
                self.query.clear();
                self.relayout();
            }
            return Clicked::Folded;
        }
        if let Some(group) = self.headers.iter().find(|(r, _)| r.contains(abs) && self.list.contains(abs)).map(|(_, g)| *g) {
            if let Some(at) = self.unfolded.iter().position(|g| *g == group) {
                self.unfolded.remove(at);
            } else {
                self.unfolded.push(group);
            }
            self.relayout();
            return Clicked::Folded;
        }
        let Some(index) = self.rows.iter().find(|(r, _)| r.contains(abs) && self.list.contains(abs)).map(|(_, i)| *i) else { return Clicked::Nothing };
        self.stop_preview();
        if let Some(choice) = self.choices.get(index).cloned() {
            synth::switch(&self.node, &choice);
            self.current = Some(choice.key());
        }
        Clicked::Switched
    }

    /// Draw the list in `list`.
    pub fn draw(&mut self, cx: &mut Cx2d, list: Rect, draw_row: &mut DrawPanelShape, draw_text: &mut DrawText, draw_small: &mut DrawText) {
        let p = makepad_look::palette::get(cx);
        // The search row on top, then the rows under it.
        self.search = Rect { pos: list.pos, size: dvec2(list.size.x, ROW_H) };
        draw_row.edged_round(cx, Rect { pos: self.search.pos + dvec2(4.0, 3.0), size: self.search.size - dvec2(8.0, 4.0) }, color(p.row, 1.0), color(p.line_faint, 1.0), 1.0, R_CONTROL);
        let count = self.choices.len();
        let (words, tone) = if self.query.is_empty() { (format!("type to search all {count} instruments"), p.muted) } else { (format!("{}|", self.query), p.text_hi) };
        draw_text.color = color(tone, 1.0);
        draw_text.draw_abs(cx, dvec2(list.pos.x + 12.0, list.pos.y + 4.0), &words);
        if !self.query.is_empty() {
            draw_small.color = color(p.muted, 1.0);
            let w = makepad_widgets::badge::advance(draw_small, cx, "clear");
            draw_small.draw_abs(cx, dvec2(list.pos.x + list.size.x - 12.0 - w, list.pos.y + 5.0), "clear");
        }
        let list = Rect { pos: list.pos + dvec2(0.0, ROW_H), size: list.size - dvec2(0.0, ROW_H) };
        self.list = list;
        self.rows.clear();
        self.headers.clear();
        let searching = !self.query.is_empty();
        let mut y = list.pos.y - self.scroll;
        for line in &self.lines {
            // Only whole rows inside the list are drawn (and hit).
            if y < list.pos.y - 0.5 || y + ROW_H > list.pos.y + list.size.y + 0.5 {
                y += ROW_H;
                continue;
            }
            let r = Rect { pos: dvec2(list.pos.x, y), size: dvec2(list.size.x, ROW_H) };
            match line {
                Line::Group(group, n) => {
                    let open = searching || self.unfolded.contains(group);
                    let now = self.current.as_ref().and_then(|key| self.choices.iter().find(|c| c.key() == *key)).filter(|c| c.group == *group);
                    draw_small.color = color(if now.is_some() { p.accent } else if open { p.secondary } else { p.label }, 1.0);
                    let words = match now {
                        Some(c) => format!("{}  {n}   now: {}", group.to_uppercase(), c.label),
                        None => format!("{}  {n}", group.to_uppercase()),
                    };
                    draw_row.chevron(cx, Rect { pos: dvec2(r.pos.x + 6.0, r.pos.y + 4.0), size: dvec2(12.0, 12.0) }, color(p.muted, 1.0), open);
                    draw_small.draw_abs(cx, dvec2(r.pos.x + 22.0, r.pos.y + 6.0), &words);
                    self.headers.push((r, *group));
                }
                Line::Choice(i) => {
                    let c = &self.choices[*i];
                    let current = self.current.as_deref() == Some(c.key().as_str());
                    let hovered = self.hover.is_some_and(|(h, _)| h == *i);
                    if current || hovered {
                        draw_row.fill_round(cx, Rect { pos: r.pos + dvec2(4.0, 1.0), size: r.size - dvec2(8.0, 2.0) }, color(if current { p.accent_wash } else { p.control_hover }, 1.0), R_CONTROL);
                    }
                    draw_text.color = color(if current { p.accent } else { p.text }, 1.0);
                    draw_text.draw_abs(cx, dvec2(r.pos.x + 22.0, r.pos.y + 4.0), &c.label);
                    draw_small.color = color(if hovered { p.muted } else { p.dim }, 1.0);
                    let label_w = makepad_widgets::badge::advance(draw_text, cx, &c.label);
                    // Short and quiet ("GM 40"); the whole of it on the row
                    // under the pointer.
                    let said = if hovered { c.detail.clone() } else { short_detail(&c.detail) };
                    let (detail, _) = makepad_value_pickers::panel::control::fit_text(draw_small, cx, &said, (r.size.x - label_w - 52.0).max(0.0));
                    let w = makepad_widgets::badge::advance(draw_small, cx, &detail);
                    draw_small.draw_abs(cx, dvec2(r.pos.x + r.size.x - 8.0 - w, r.pos.y + 5.0), &detail);
                    self.rows.push((r, *i));
                }
            }
            y += ROW_H;
        }
    }
}

/// An instrument's detail as a list row says it: "General MIDI program 40
/// of the GM SoundFont (FluidR3 GM)" is "GM 40", a drum kit "GM kit 8",
/// anything else its first clause.
fn short_detail(detail: &str) -> String {
    let number = |rest: &str| rest.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("").to_string();
    if let Some(rest) = detail.strip_prefix("General MIDI drum kit ") {
        return format!("GM kit {}", number(rest));
    }
    if let Some(rest) = detail.strip_prefix("General MIDI program ") {
        return format!("GM {}", number(rest));
    }
    detail.split([':', '(', ';']).next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod short_detail_tests {
    #[test]
    fn short() {
        assert_eq!(super::short_detail("General MIDI program 40 of the GM SoundFont (FluidR3 GM)"), "GM 40");
        assert_eq!(super::short_detail("General MIDI drum kit 8 of the GM SoundFont: room"), "GM kit 8");
        assert_eq!(super::short_detail("a plucked bass: warm"), "a plucked bass");
    }
}
