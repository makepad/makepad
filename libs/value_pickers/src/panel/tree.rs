//! The panel as a tree: what each widget shows (its paths, however deep,
//! drawn or folded away), and the header over the tree: the filter the
//! picture's selection sets, a search over the controls, and an index of
//! the modules that opens and scrolls to one.

use super::control::{DrawPanelShape, PanelControl, R_CONTROL};
use super::host;
use super::instruments::{PanelAnchor, PanelCurve, PanelEnvelope, PanelGradient, PanelKeys, PanelMacro, PanelMeter, PanelMod, PanelPoints, PanelSteps};
use super::rich::{PanelColor, PanelFn, PanelText};
use makepad_widgets::badge::advance;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    mod.widgets.PanelHeaderBase = #(PanelHeader::register_widget(vm))
    /** The panel's one header: a search box over the controls and the
     * modules (filters as it is typed; a module it finds can be jumped
     * to), with an "active" token in it (only what acts at the playhead). */
    mod.widgets.PanelHeader = set_type_default() do mod.widgets.PanelHeaderBase{
        width: Fill height: Fit flow: Down margin: Inset{left: 6 right: 6}
        draw_bg +: {color: #x00000000}
        draw_text +: {text_style: theme.font_regular{font_size: 7.5}}
        search := TextInput{width: Fill height: 24 margin: 0 padding: Inset{left: 26 right: 8 top: 4 bottom: 4} empty_text: "Find a control or module"
            draw_bg +: {color: #x00000000 color_hover: #x00000000 color_focus: #x00000000 border_color: #x00000000 border_size: 0.0}
            draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

/// Controls a host adds to the panel (widgets of its own): each says the
/// paths of a widget it knows, or nothing.
pub type OwnPaths = fn(&WidgetRef) -> Option<Vec<String>>;

thread_local! {
    static HOST_OWN: std::cell::RefCell<Vec<OwnPaths>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Make the paths of a host's own control known to the tree (once each).
pub fn register_own(probe: OwnPaths) {
    HOST_OWN.with(|p| {
        let mut p = p.borrow_mut();
        if !p.iter().any(|q| *q as usize == probe as usize) {
            p.push(probe);
        }
    });
}

/// A control's own paths, when it is one.
fn own(w: &WidgetRef) -> Option<Vec<String>> {
    macro_rules! path_of {
        ($($t:ty),*) => {
            $(if let Some(x) = w.borrow::<$t>() {
                return Some(vec![x.path.clone()]);
            })*
        };
    }
    path_of!(PanelControl, PanelColor, PanelText, PanelFn, PanelSteps, PanelPoints, PanelCurve, PanelGradient, PanelMeter, PanelAnchor, PanelKeys, PanelMod);
    if let Some(x) = w.borrow::<super::scene::PanelScene>() {
        return Some(vec![format!("edit.{}.span", x.node)]);
    }
    if let Some(x) = w.borrow::<PanelEnvelope>() {
        return Some(x.paths().iter().map(|s| s.to_string()).collect());
    }
    if let Some(x) = w.borrow::<PanelMacro>() {
        return Some(x.parsed().into_iter().map(|t| t.0).collect());
    }
    HOST_OWN.with(|p| p.borrow().iter().find_map(|probe| probe(w)))
}

fn collect(w: &WidgetRef, out: &mut Vec<String>, controls: &mut usize) {
    if let Some(paths) = own(w) {
        *controls += 1;
        for p in paths {
            if !p.is_empty() && !out.contains(&p) {
                out.push(p);
            }
        }
        return;
    }
    w.children(&mut |_, c| collect(&c, out, controls));
}

/// Every path a widget shows, and how many controls show them, however
/// deep and whether drawn or folded away.
pub fn paths_of(w: &WidgetRef) -> (Vec<String>, usize) {
    let (mut out, mut n) = (Vec::new(), 0);
    collect(w, &mut out, &mut n);
    (out, n)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    /// The "active" token: only what acts at the playhead.
    Active,
    /// Module `k` the search finds: opened and gone to.
    Element(usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct PanelHeader {
    #[deref]
    view: View,
    /// Under the chips (what the pointer hits).
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_chip: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    #[rust]
    spots: Vec<(Rect, Spot)>,
    #[rust]
    hot: Option<Spot>,
    /// The modules the search finds by title, as the jumps show them.
    #[rust]
    elements: Vec<Vec<u64>>,
}

impl Widget for PanelHeader {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let at = |s: &Self, abs: DVec2| s.spots.iter().find(|(r, _)| r.contains(abs)).map(|(_, x)| *x);
        // The field first: a press on it is the field's (the box under it
        // takes only the token and the jumps).
        self.view.handle_event(cx, event, scope);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = at(self, fe.abs);
                cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_chip.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hot = None;
                self.draw_chip.redraw(cx);
            }
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() => match at(self, fe.abs) {
                Some(Spot::Active) => {
                    host::set_only_active(!host::only_active());
                    self.view.redraw(cx);
                }
                Some(Spot::Element(k)) => {
                    if let Some(ids) = self.elements.get(k) {
                        host::open_modules(ids);
                    }
                    self.view.redraw(cx);
                }
                None => {}
            },
            _ => {}
        }
        if let Event::Actions(actions) = event {
            if let Some(text) = self.view.widget(cx, ids!(search)).as_text_input().changed(actions) {
                host::set_search(&text);
                self.view.redraw(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        cx.begin_turtle(walk, Layout::flow_down());
        let width = cx.turtle().inner_width().max(80.0);
        // The modules the search finds by title: jumps under the box.
        let found = host::tree_titles_found();
        self.elements = found.iter().map(|e| e.1.clone()).collect();
        let w_of = |s: &Self, cx: &mut Cx2d, label: &str| advance(&s.draw_text, cx, label) + 16.0;
        let mut layout = Vec::new();
        let (mut x, mut y) = (0.0, 30.0);
        for (title, _) in &found {
            let label = format!("→ {title}");
            let w = w_of(self, cx, &label).min(width);
            if x + w > width && x > 0.0 {
                x = 0.0;
                y += 24.0;
            }
            layout.push((label, x, y, w));
            x += w + 4.0;
        }
        let height = if layout.is_empty() { 28.0 } else { y + 22.0 };
        let rect = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(height)));
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.spots.clear();
        // The box: a rounded well holding the field, a lens, and the token.
        let frame = Rect { pos: rect.pos, size: dvec2(rect.size.x, 28.0) };
        self.draw_chip.edged_round(cx, frame, color(p.well, 1.0), color(p.line_faint, 1.0), 1.0, R_CONTROL + 2.0);
        // The lens: a ring and its handle.
        let lc = frame.pos + dvec2(14.0, 13.0);
        self.draw_chip.edged_round(cx, Rect { pos: lc - dvec2(4.5, 4.5), size: dvec2(9.0, 9.0) }, color(0, 0.0), color(p.muted, 1.0), 1.4, 4.5);
        self.draw_chip.fill_round(cx, Rect { pos: lc + dvec2(3.5, 3.5), size: dvec2(4.0, 1.6) }, color(p.muted, 1.0), 0.8);
        let on = host::only_active();
        let token = "active";
        let tw = advance(&self.draw_text, cx, token) + 14.0;
        let tr = Rect { pos: dvec2(frame.pos.x + frame.size.x - tw - 5.0, frame.pos.y + 5.0), size: dvec2(tw, 18.0) };
        let hot = self.hot == Some(Spot::Active);
        let look = p.pick(on, hot, p.raised);
        self.draw_chip.edged_round(cx, tr, color(look.fill, 1.0), color(look.edge, 1.0), 1.0, 9.0);
        self.draw_text.color = color(look.ink, 1.0);
        self.draw_text.draw_abs(cx, dvec2(tr.pos.x + 7.0, tr.pos.y + 4.0), token);
        self.spots.push((tr, Spot::Active));
        for (k, (label, x, y, w)) in layout.into_iter().enumerate() {
            let r = Rect { pos: dvec2(rect.pos.x + x, rect.pos.y + y), size: dvec2(w, 20.0) };
            let hot = self.hot == Some(Spot::Element(k));
            self.draw_chip.fill_round(cx, r, color(if hot { p.control_hover } else { p.raised }, 1.0), R_CONTROL);
            self.draw_text.color = color(p.text, 1.0);
            let (label, _) = super::control::fit_text(&self.draw_text, cx, &label, w - 16.0);
            self.draw_text.draw_abs(cx, dvec2(r.pos.x + 8.0, r.pos.y + 5.0), &label);
            self.spots.push((r, Spot::Element(k)));
        }
        // The field over the box, left of the token.
        let field = Walk::new(Size::Fixed((frame.size.x - tw - 14.0).max(40.0)), Size::Fixed(24.0)).with_abs_pos(frame.pos + dvec2(0.0, 2.0));
        let _ = self.view.widget(cx, ids!(search)).draw_walk(cx, scope, field);
        cx.end_turtle();
        DrawStep::done()
    }
}
