//! An answer of the profiler's query engine (makepad_prof_stream::ask::Ans) drawn as it reads
//! for an agent: the finding first, then its tables (header once, numbers right-aligned), notes
//! and preformatted timelines, and the drill-down queries as buttons. Clicking a table row whose
//! first cell is an id (F812, T3) asks about it; clicking a drill-down asks that query.

use makepad_prof_stream::ask::{Ans, Block};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ProfAnswerBase = #(ProfAnswer::register_widget(vm))
    mod.widgets.ProfAnswer = set_type_default() do mod.widgets.ProfAnswerBase{
        width: Fill height: Fill
        background: theme.color_bg_app
        surface: mix(theme.color_bg_app, theme.color_text, 0.04)
        edge: mix(theme.color_bg_app, theme.color_text, 0.12)
        ink: theme.color_text
        muted: mix(theme.color_bg_app, theme.color_text, 0.58)
        accent: theme.color_focus
        draw_finding +: {color: theme.color_text text_style: theme.font_bold{font_size: 10}}
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 8.5}}
        draw_head +: {color: theme.color_text text_style: theme.font_bold{font_size: 8}}
        draw_code +: {color: theme.color_text text_style: theme.font_code{font_size: 8}}
    }
}

#[derive(Clone, Debug, Default)]
pub enum ProfAnswerAction {
    /// ask this query
    Query(String),
    #[default]
    None,
}

const PAD: f64 = 12.0;
const LINE: f64 = 15.0;
const ROW: f64 = 17.0;
const CHAR_W: f64 = 5.1;

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct ProfAnswer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_finding: DrawText,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_head: DrawText,
    #[live]
    draw_code: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    accent: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    view: Rect,
    #[rust]
    ans: Option<Ans>,
    #[rust]
    scroll: f64,
    #[rust]
    content_h: f64,
    /// clickable rects of the last draw: (rect, query)
    #[rust]
    links: Vec<(Rect, String)>,
    #[rust]
    hover: Option<usize>,
}

impl ProfAnswer {
    pub fn set_answer(&mut self, cx: &mut Cx, ans: Option<Ans>) {
        self.ans = ans;
        self.scroll = 0.0;
        self.area.redraw(cx);
    }

    /// Replaces the answer keeping the scroll (a live refresh of the same query).
    pub fn refresh(&mut self, cx: &mut Cx, ans: Ans) {
        self.ans = Some(ans);
        self.area.redraw(cx);
    }

    fn rect(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f) {
        self.draw_shape.color = color;
        self.draw_shape.draw_abs(cx, r);
    }

    /// The query a table row asks: by its first cell's id.
    fn row_query(cell: &str) -> Option<String> {
        let id = cell.split_whitespace().next()?;
        let num = |p: char| id.strip_prefix(p).filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())).is_some();
        if num('F') {
            Some(format!("frame {id}"))
        } else if num('T') {
            Some(format!("top cpu lane={id}"))
        } else {
            None
        }
    }

    pub fn wrap(s: &str, width: f64, char_w: f64) -> Vec<String> {
        let max = ((width / char_w) as usize).max(10);
        let mut out = Vec::new();
        for para in s.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max {
                    out.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
            out.push(line);
        }
        out
    }
}

/// The drawn width of `s`.
fn measure(draw: &mut DrawText, cx: &mut Cx2d, s: &str) -> f64 {
    draw.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64
}

impl WidgetNode for ProfAnswer {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    fn children(&self, _visit: &mut dyn FnMut(LiveId, WidgetRef)) {}
    fn find_widgets_from_point(&self, _cx: &Cx, _p: DVec2, _found: &mut dyn FnMut(&WidgetRef)) {}
}

impl Widget for ProfAnswer {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        cx.push_clip_rect(view);
        let bg = self.background;
        self.rect(cx, view, bg);
        self.links.clear();
        let Some(ans) = self.ans.clone() else {
            cx.pop_clip_rect();
            return DrawStep::done();
        };
        let x = view.pos.x + PAD;
        let w = view.size.x - 2.0 * PAD;
        let mut y = view.pos.y + PAD - self.scroll;
        for l in Self::wrap(&ans.finding, w, 6.2) {
            self.draw_finding.draw_abs(cx, dvec2(x, y), &l);
            y += 17.0;
        }
        y += 4.0;
        for b in &ans.blocks {
            match b {
                Block::Title(s) => {
                    y += 4.0;
                    self.draw_head.color = self.muted;
                    self.draw_head.draw_abs(cx, dvec2(x, y), s);
                    y += LINE;
                }
                Block::Note(s) => {
                    self.draw_text.color = self.muted;
                    for l in Self::wrap(s, w, CHAR_W) {
                        self.draw_text.draw_abs(cx, dvec2(x, y), &l);
                        y += LINE;
                    }
                }
                Block::Pre(s) => {
                    let lines: Vec<&str> = s.lines().collect();
                    let r = Rect { pos: dvec2(x - 4.0, y - 2.0), size: dvec2(w + 8.0, lines.len() as f64 * 12.0 + 4.0) };
                    let surface = self.surface;
                    self.rect(cx, r, surface);
                    self.draw_code.color = self.ink;
                    for l in lines {
                        self.draw_code.draw_abs(cx, dvec2(x, y), l);
                        y += 12.0;
                    }
                    y += 6.0;
                }
                Block::Table(t) => {
                    let n = t.head.len();
                    let mut cw = vec![0f64; n];
                    for (i, h) in t.head.iter().enumerate() {
                        cw[i] = measure(&mut self.draw_head, cx, h);
                    }
                    for r in &t.rows {
                        for (i, c) in r.iter().enumerate().take(n) {
                            cw[i] = cw[i].max(measure(&mut self.draw_text, cx, c));
                        }
                    }
                    // the widest column (usually the last, a description) takes what is left
                    let total: f64 = cw.iter().map(|c| c + 14.0).sum();
                    if total > w {
                        let (wi, _) = cw.iter().enumerate().fold((0, 0.0), |a, (i, c)| if *c > a.1 { (i, *c) } else { a });
                        cw[wi] = (cw[wi] - (total - w)).max(40.0);
                    }
                    let numeric: Vec<bool> = (0..n).map(|i| t.rows.iter().all(|r| r.get(i).map_or(true, |c| c.chars().next().map_or(true, |ch| ch.is_ascii_digit() || ch == '-' || ch == '+')))).collect();
                    let edge = self.edge;
                    // header
                    let mut cx0 = x;
                    self.draw_head.color = self.muted;
                    for (i, h) in t.head.iter().enumerate() {
                        let tx = if numeric[i] { cx0 + cw[i] - measure(&mut self.draw_head, cx, h) } else { cx0 };
                        self.draw_head.draw_abs(cx, dvec2(tx, y), h);
                        cx0 += cw[i] + 14.0;
                    }
                    y += ROW;
                    self.rect(cx, Rect { pos: dvec2(x, y - 3.0), size: dvec2(w, 1.0) }, edge);
                    for (ri, r) in t.rows.iter().enumerate() {
                        let rr = Rect { pos: dvec2(x - 4.0, y - 2.0), size: dvec2(w + 8.0, ROW) };
                        let link = t.links.get(ri).cloned().flatten().or_else(|| r.first().and_then(|c| Self::row_query(c)));
                        if let Some(q) = link {
                            let k = self.links.len();
                            if self.hover == Some(k) {
                                let surface = self.surface;
                                self.rect(cx, rr, surface);
                            }
                            self.links.push((rr, q));
                        }
                        let mut cx0 = x;
                        for (i, c) in r.iter().enumerate().take(n) {
                            let mut s = c.clone();
                            while measure(&mut self.draw_text, cx, &s) > cw[i] + 0.5 && s.chars().count() > 1 {
                                if s.ends_with('…') {
                                    s.pop();
                                }
                                s.pop();
                                s.push('…');
                            }
                            let tx = if numeric[i] { cx0 + cw[i] - measure(&mut self.draw_text, cx, &s) } else { cx0 };
                            self.draw_text.color = self.ink;
                            self.draw_text.draw_abs(cx, dvec2(tx, y), &s);
                            cx0 += cw[i] + 14.0;
                        }
                        y += ROW;
                    }
                    y += 6.0;
                }
            }
        }
        // drill-downs
        if !ans.more.is_empty() {
            y += 4.0;
            let mut bx = x;
            for q in &ans.more {
                let bw = measure(&mut self.draw_text, cx, q) + 16.0;
                if bx + bw > x + w {
                    bx = x;
                    y += 24.0;
                }
                let r = Rect { pos: dvec2(bx, y), size: dvec2(bw, 20.0) };
                let k = self.links.len();
                let c = if self.hover == Some(k) { self.accent } else { self.edge };
                self.rect(cx, r, c);
                let inner = Rect { pos: dvec2(bx + 1.0, y + 1.0), size: dvec2(bw - 2.0, 18.0) };
                let surface = self.surface;
                self.rect(cx, inner, surface);
                self.draw_text.color = self.ink;
                self.draw_text.draw_abs(cx, dvec2(bx + 8.0, y + 4.0), q);
                self.links.push((r, q.clone()));
                bx += bw + 6.0;
            }
            y += 24.0;
        }
        self.content_h = y + self.scroll - view.pos.y + PAD;
        cx.pop_clip_rect();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // the rect the turtle gave at draw time can move afterwards (a Fill sibling above is
        // resolved later): the links move with it
        let placed = self.area.rect(cx);
        if placed.size.x > 0.0 && placed.pos != self.view.pos {
            let d = placed.pos - self.view.pos;
            for l in self.links.iter_mut() {
                l.0.pos += d;
            }
            self.view.pos = placed.pos;
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let h = self.links.iter().position(|l| l.0.contains(e.abs));
                if h != self.hover {
                    self.hover = h;
                    self.area.redraw(cx);
                }
                cx.set_cursor(if h.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(e) => {
                self.scroll = (self.scroll + e.scroll.y).clamp(0.0, (self.content_h - self.view.size.y).max(0.0));
                self.area.redraw(cx);
            }
            Hit::FingerUp(e) if e.is_over => {
                if let Some(l) = self.links.iter().find(|l| l.0.contains(e.abs)) {
                    cx.widget_action(self.uid, ProfAnswerAction::Query(l.1.clone()));
                }
            }
            _ => {}
        }
    }
}
