//! A module: a rack unit with a header that holds a set of controls and
//! acts on all of them at once. Every bound control drawn inside it
//! (however deep) is the module's ([`super::host::register`]), so a module
//! needs no list of what it holds:
//! - four preset slots (a click on an empty one stores the controls'
//!   values, on a stored one recalls them; shift-click stores again);
//! - A/B morph: the slider in the header moves every value between slot 1
//!   and slot 2 (numbers in a line, colours through OKLab, toggles and
//!   choices at the middle), live while dragged, one undo step at the end;
//! - randomise within each control's range (a few clicks in, the dice
//!   keep the ones it lands on: the lock in the header keeps the colours);
//! - reset: every control back to what the panel found;
//! - bypass: `bypass: "SPARKS.on"`, a toggle in the header bound to a bool.
//!
//! The mood pad (`PanelMood` in a module) morphs between all four slots at
//! once: each corner a slot, the puck's place weighting them.

use super::control::{fit_text, DrawPanelShape, R_CARD, R_CONTROL, R_GROUP};
use super::host::{self, Bound, BoundKind, PanelValue};
use super::palette_gen;
use super::tree;
use crate::color_picker::{format_code_color, parse_code_color};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PanelModuleBase = #(PanelModule::register_widget(vm))
    /** A rack unit: a titled set of controls with presets, A/B morph,
     * randomise, reset and bypass. */
    mod.widgets.PanelModule = set_type_default() do mod.widgets.PanelModuleBase{
        width: Fill height: Fit flow: Down spacing: 6 margin: Inset{left: 6 right: 6} padding: Inset{left: 10 right: 10 top: 2 bottom: 10}
        draw_hit +: {color: #x00000000}
        draw_title +: {text_style: theme.font_bold{font_size: 8.5}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }

    mod.widgets.PanelMoodBase = #(PanelMood::register_widget(vm))
    /** A pad between the module's four preset slots: each corner a slot. */
    mod.widgets.PanelMood = set_type_default() do mod.widgets.PanelMoodBase{
        width: Fill height: 150
        draw_bg +: {color: #x00000000}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

/// Code for a value (a number in its unit form, else as written).
fn value_of(code: &str) -> PanelValue {
    match code.trim() {
        "true" => PanelValue::Bool(true),
        "false" => PanelValue::Bool(false),
        _ => PanelValue::Code(code.to_string()),
    }
}

/// Values weighted together: each path's value from `sets` (each a
/// snapshot and its weight; weights sum to 1). Numbers in a line, colours
/// through OKLab, the rest the heaviest set's.
pub fn blend(bounds: &[Bound], sets: &[(&[(String, String)], f64)]) -> Vec<(String, PanelValue)> {
    let mut out = Vec::new();
    for b in bounds {
        let codes: Vec<(&str, f64)> = sets.iter().filter_map(|(s, w)| s.iter().find(|(p, _)| *p == b.path).map(|(_, c)| (c.as_str(), *w))).collect();
        if codes.is_empty() {
            continue;
        }
        let heaviest = codes.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|c| c.0).unwrap_or(codes[0].0);
        let code = match &b.kind {
            BoundKind::Numbers { step, .. } => {
                let lists: Vec<(Vec<f64>, f64)> = codes.iter().map(|(c, w)| (host::numbers_in(c), *w)).collect();
                let n = lists.iter().map(|l| l.0.len()).min().unwrap_or(0);
                if n == 0 {
                    heaviest.to_string()
                } else {
                    let total: f64 = lists.iter().map(|l| l.1).sum::<f64>().max(1e-9);
                    let values: Vec<f64> = (0..n).map(|k| lists.iter().map(|(l, w)| l[k] * w).sum::<f64>() / total).collect();
                    let decimals = if *step >= 1.0 { 0 } else { (-step.log10()).ceil().max(0.0) as usize };
                    host::with_numbers(heaviest, &values, decimals)
                }
            }
            BoundKind::Colour => {
                let colours: Vec<([f64; 4], f64)> = codes.iter().filter_map(|(c, w)| Some((parse_code_color(c)?, *w))).collect();
                if colours.is_empty() {
                    heaviest.to_string()
                } else {
                    // Pairwise in OKLab, weight by weight.
                    let mut acc = colours[0].0;
                    let mut wsum = colours[0].1;
                    for (c, w) in &colours[1..] {
                        let t = if wsum + w > 0.0 { w / (wsum + w) } else { 0.0 };
                        acc = palette_gen::mix(&[acc], &[*c], t)[0];
                        wsum += w;
                    }
                    format_code_color(acc)
                }
            }
            _ => heaviest.to_string(),
        };
        out.push((b.path.clone(), value_of(&code)));
    }
    out
}

/// A random value for each control within its range (`seed`: the same
/// dice, the same values); colours kept when `keep_colours`.
pub fn randomised(bounds: &[Bound], seed: u64, keep_colours: bool) -> Vec<(String, PanelValue)> {
    let rnd = |i: u64, k: u64| {
        let mut x = seed.wrapping_mul(0x9e3779b97f4a7c15) ^ (i * 31 + k).wrapping_mul(0xbf58476d1ce4e5b9);
        x ^= x >> 31;
        x = x.wrapping_mul(0x94d049bb133111eb);
        x ^= x >> 29;
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = Vec::new();
    for (i, b) in bounds.iter().enumerate() {
        let Some(now) = host::read(&b.path).map(|v| v.code()) else { continue };
        let i = i as u64;
        let code = match &b.kind {
            BoundKind::Numbers { lo, hi, step } => {
                let n = host::numbers_in(&now).len();
                if n == 0 {
                    continue;
                }
                let values: Vec<f64> = (0..n as u64).map(|k| {
                    let v = lo + (hi - lo) * rnd(i, k);
                    if *step > 0.0 { (v / step).round() * step } else { v }
                }).collect();
                let decimals = if *step >= 1.0 { 0 } else { (-step.log10()).ceil().max(0.0) as usize };
                host::with_numbers(&now, &values, decimals)
            }
            BoundKind::Colour if !keep_colours => {
                let Some(c) = parse_code_color(&now) else { continue };
                palette_gen::random(&[c], seed ^ i, palette_gen::Keep { lightness: true, accent: false, paper: false }).first().map(|c| format_code_color(*c)).unwrap_or(now)
            }
            BoundKind::Bool => (rnd(i, 0) > 0.5).to_string(),
            BoundKind::Choice(list) if !list.is_empty() => {
                let pick = &list[(rnd(i, 0) * list.len() as f64) as usize % list.len()];
                if now.trim().starts_with('"') { format!("{pick:?}") } else { format!("@{pick}") }
            }
            _ => continue,
        };
        out.push((b.path.clone(), value_of(&code)));
    }
    out
}

/// A chip at (x, y) in the module's header rows: its width.
fn chip_at(s: &mut PanelModule, cx: &mut Cx2d, x: f64, y: f64, label: &str, spot: Spot, p: &makepad_look::palette::Palette, tint: Vec4f) -> f64 {
    let w = makepad_widgets::badge::advance(&s.draw_small, cx, label) + 16.0;
    let r = Rect { pos: dvec2(x, y), size: dvec2(w, 20.0) };
    let fill = if s.hot == Some(spot) { tint } else { color(p.raised, 1.0) };
    s.draw_frame.fill_round(cx, r, fill, R_CONTROL);
    s.draw_small.color = color(if s.hot == Some(spot) { p.on_accent } else { p.text }, 1.0);
    s.draw_small.draw_abs(cx, dvec2(r.pos.x + 8.0, r.pos.y + 5.0), label);
    s.spots.push((r, spot));
    w
}

fn set_all(values: Vec<(String, PanelValue)>) {
    for (path, value) in values {
        host::set(&path, value);
    }
    host::changed();
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Slot(usize),
    Morph,
    Dice,
    Lock,
    Reset,
    Bypass,
    /// The AI button (opens the asks), and ask `k` of [`ASKS`].
    Ai,
    Ask(usize),
    /// "···": the module's actions shown or hidden.
    Menu,
}

impl Spot {
    /// What it does, said while the pointer is on it.
    fn tip(self, stored: bool, keep_colours: bool) -> String {
        match self {
            Spot::Slot(k) if stored => format!("Recall snapshot {} (shift-click: store the values now)", k + 1),
            Spot::Slot(k) => format!("Store this module's values as snapshot {}", k + 1),
            Spot::Morph => "Blend every value between snapshot 1 and snapshot 2".into(),
            Spot::Dice => "Set every control to a random value within its range".into(),
            Spot::Lock if keep_colours => "Colours stay when randomising (click: randomise them too)".into(),
            Spot::Lock => "Colours are randomised too (click: keep them)".into(),
            Spot::Reset => "Put back the values the panel opened with".into(),
            Spot::Bypass => "Turn this module on or off".into(),
            Spot::Ai => "Ask the AI to rework this module's values".into(),
            Spot::Ask(k) if ASKS[k].2 == "variants" => "Ask the AI for five takes on this module, to pick from".into(),
            Spot::Ask(k) => format!("Ask the AI for {} (one undo step)", ASKS[k].1),
            Spot::Menu => "Snapshots, morph, randomise, reset and AI for this module".into(),
        }
    }
}

/// What the AI button offers: a label, the prompt, and the mode.
const ASKS: [(&str, &str, &str); 4] = [
    ("Something different", "something completely different here", "replace"),
    ("Reinvent", "reinvent this", "replace"),
    ("Randomise creatively", "randomise this creatively", "replace"),
    ("5 variants", "five different takes on this", "variants"),
];

#[derive(Script, ScriptHook, Widget)]
pub struct PanelModule {
    #[deref]
    view: View,
    #[live]
    pub title: String,
    /// A bool path the header's switch turns on and off.
    #[live]
    pub bypass: String,
    /// The module's colour (its header's edge), as `#rrggbb`.
    #[live]
    pub tint: String,
    /// When in the film it acts ("beats 9-16", "the lyric 'AGI'"): shown
    /// under the title; the host reads it from the panel's code.
    #[live]
    pub when: String,
    /// Open when the panel opens (else folded to its heading and the
    /// controls before its first group).
    #[live]
    pub open: bool,
    /// The module's card: under its heading and everything it holds.
    #[live]
    draw_card: DrawPanelShape,
    /// The heading's marks, chips and bars.
    #[live]
    draw_frame: DrawPanelShape,
    /// The actions' rows (under "···").
    #[live]
    draw_head: DrawPanelShape,
    /// Over what does not matter now (dimmed), the activity marks and the
    /// notes: a draw call of its own, after the controls.
    #[live]
    draw_dim: DrawPanelShape,
    /// Over the heading and its rows (what the pointer hits).
    #[live]
    draw_hit: DrawColor,
    #[rust]
    place: Area,
    #[rust]
    lit_seen: u64,
    /// Open or folded (from `open` until clicked or revealed).
    #[rust]
    unfolded: Option<bool>,
    /// Opened by hand: brought into view once it has drawn open.
    #[rust]
    reveal_open: bool,
    /// The picture's selection it last acted on.
    #[rust]
    reveal_seen: Option<u64>,
    /// Every path in it (drawn or folded away), as last drawn.
    #[rust]
    paths: Vec<String>,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_small: DrawText,
    #[rust]
    spots: Vec<(Rect, Spot)>,
    #[rust]
    hot: Option<Spot>,
    #[rust]
    morph: f64,
    #[rust]
    morphing: bool,
    #[rust]
    dice: u64,
    #[rust(true)]
    keep_colours: bool,
    /// The asks are shown (under the tools).
    #[rust]
    asking_open: bool,
    /// The actions row is shown ("···").
    #[rust]
    menu_open: bool,
    /// The ask in flight and what it said last.
    #[rust]
    ask: Option<u64>,
    #[rust]
    ask_said: String,
}

impl PanelModule {
    fn id(&self) -> u64 {
        self.widget_uid().0
    }

    /// Every control in it as the module acts on them: as each said when it
    /// drew, and what the value says for those folded away.
    fn bounds(&self) -> Vec<Bound> {
        let mut bounds = host::module(self.id()).bounds;
        for path in &self.paths {
            if bounds.iter().any(|b| b.path == *path) {
                continue;
            }
            let Some(code) = host::read(path).map(|v| v.code()) else { continue };
            let numbers = host::numbers_in(&code);
            let kind = if parse_code_color(&code).is_some() {
                BoundKind::Colour
            } else if matches!(code.trim(), "true" | "false") {
                BoundKind::Bool
            } else if let Some(v) = numbers.iter().map(|v| v.abs()).reduce(f64::max) {
                BoundKind::Numbers { lo: 0.0, hi: (2.0 * v).max(1.0), step: if numbers.iter().all(|n| n.fract() == 0.0) { 1.0 } else { 0.01 } }
            } else {
                BoundKind::Other
            };
            bounds.push(Bound { path: path.clone(), kind });
        }
        bounds
    }

    fn click(&mut self, cx: &mut Cx, spot: Spot, shift: bool) {
        let id = self.id();
        let mut state = host::module(id);
        state.bounds = self.bounds();
        match spot {
            Spot::Slot(k) => {
                let stored = state.slots.get(k).cloned().flatten();
                match stored {
                    Some(values) if !shift => set_all(values.iter().map(|(p, c)| (p.clone(), value_of(c))).collect()),
                    _ => {
                        let snap = host::snapshot(&state.bounds);
                        host::module_update(id, |m| {
                            if m.slots.len() < 4 {
                                m.slots.resize(4, None);
                            }
                            m.slots[k] = Some(snap);
                        });
                    }
                }
            }
            Spot::Dice => {
                self.dice += 1;
                set_all(randomised(&state.bounds, self.dice, self.keep_colours));
            }
            Spot::Lock => self.keep_colours = !self.keep_colours,
            Spot::Reset => set_all(state.original.iter().map(|(p, c)| (p.clone(), value_of(c))).collect()),
            Spot::Bypass => {
                let on = host::read(&self.bypass).is_some_and(|v| v.code().trim() == "true");
                host::set(&self.bypass, PanelValue::Bool(!on));
                host::changed();
            }
            Spot::Ai => self.asking_open = !self.asking_open,
            Spot::Ask(k) => {
                let (_, prompt, mode) = ASKS[k];
                let targets = self.paths.clone();
                self.ask = host::ask(prompt, &targets, mode);
                self.ask_said = if self.ask.is_some() { "working".into() } else { "failed: the host takes no asks".into() };
                self.asking_open = false;
            }
            // Shown with the module (a folded one opens).
            Spot::Menu => {
                self.menu_open = !self.menu_open;
                if self.menu_open && self.unfolded != Some(true) {
                    self.unfolded = Some(true);
                    host::set_fold(&host::fold_key(&self.title, &self.paths), true);
                    self.reveal_open = true;
                }
                if !self.menu_open {
                    self.asking_open = false;
                }
            }
            Spot::Morph => {}
        }
        self.draw_head.redraw(cx);
    }

    fn morph_to(&mut self, cx: &mut Cx, rect: Rect, x: f64) {
        let t = ((x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0);
        self.morph = t;
        let mut state = host::module(self.id());
        state.bounds = self.bounds();
        let (Some(Some(a)), Some(Some(b))) = (state.slots.first(), state.slots.get(1)) else { return };
        for (path, value) in blend(&state.bounds, &[(a.as_slice(), 1.0 - t), (b.as_slice(), t)]) {
            host::set(&path, value);
        }
        host::changed();
        self.draw_head.redraw(cx);
    }
}

/// A module dropped (its panel made again or closed) leaves the index.
impl Drop for PanelModule {
    fn drop(&mut self) {
        host::tree_forget(self.widget_uid().0);
    }
}

impl Widget for PanelModule {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let at = |s: &Self, abs: DVec2| s.spots.iter().rev().find(|(r, _)| r.contains(abs)).copied();
        match event {
            Event::MouseMove(e) => host::pointer(cx, e.abs),
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape && host::selected_module().is_some() => {
                host::clear_selection();
                self.draw_head.redraw(cx);
            }
            _ => {}
        }
        // What matters now changed (the playhead, the picture's selection).
        let lit = host::lit_rev();
        if lit != self.lit_seen {
            self.lit_seen = lit;
            self.draw_head.redraw(cx);
        }
        match event.hits(cx, self.draw_hit.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = at(self, fe.abs).map(|(_, s)| s);
                cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_head.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hot = None;
                self.draw_head.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some((rect, Spot::Morph)) = at(self, fe.abs) {
                    self.morphing = true;
                    self.morph_to(cx, rect, fe.abs.x);
                }
            }
            Hit::FingerMove(fe) => {
                if self.morphing {
                    if let Some((rect, _)) = self.spots.iter().find(|(_, s)| *s == Spot::Morph).copied() {
                        self.morph_to(cx, rect, fe.abs.x);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                if std::mem::take(&mut self.morphing) {
                    return;
                }
                if fe.is_over {
                    match at(self, fe.abs) {
                        Some((_, spot)) => self.click(cx, spot, fe.modifiers.shift),
                        // The header elsewhere: the module selected (its
                        // paths stay the focus), or no longer.
                        // The heading elsewhere: the module opens (and is
                        // selected: its paths stay the focus) or folds.
                        None => {
                            let open = self.unfolded != Some(true);
                            self.unfolded = Some(open);
                            host::set_fold(&host::fold_key(&self.title, &self.paths), open);
                            self.reveal_open = open;
                            // Opened: its groups start folded, each time.
                            if open {
                                for (_, kid) in self.view.children.iter() {
                                    if let Some(mut group) = kid.borrow_mut::<super::rich::PanelGroup>() {
                                        group.fold();
                                    }
                                }
                            }
                            if !open {
                                self.menu_open = false;
                                self.asking_open = false;
                            }
                            if open != (host::selected_module() == Some(self.id())) {
                                host::select_module(self.id(), self.paths.clone());
                            }
                            self.view.redraw(cx);
                        }
                    }
                }
            }
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let p = makepad_look::palette::get(cx);
        let id = self.id();
        let state = host::module(id);
        // What it holds, however deep, drawn or folded away.
        let kids: Vec<WidgetRef> = self.view.children.iter().map(|(_, w)| w.clone()).collect();
        let mut paths: Vec<String> = Vec::new();
        let mut controls = 0;
        let mut kid_paths = Vec::new();
        for k in &kids {
            let (ps, n) = tree::paths_of(k);
            controls += n;
            for q in &ps {
                if !paths.contains(q) {
                    paths.push(q.clone());
                }
            }
            kid_paths.push(ps);
        }
        self.paths = paths.clone();
        let lit = host::activity(&paths);
        // Not drawn: with "only what's active" when nothing in it matters
        // now, and when the search finds neither its title nor any of its
        // controls. (The picture's selection hides and dims nothing: the
        // modules holding it are marked, open and come into view.)
        let title_hit = host::title_found(&self.title);
        if (lit == Some(false) && host::only_active()) || (!host::found(&paths) && !title_hit) {
            host::tree_hidden(id, &self.title);
            return DrawStep::done();
        }
        // Open: as written until clicked, when the index asks, when the
        // picture's selection changes to something it holds, and while a
        // search finds something in it.
        // (As the person left it, when they did: a panel made again keeps it.)
        if self.unfolded.is_none() {
            self.unfolded = Some(host::folded_by_hand(&host::fold_key(&self.title, &paths)).unwrap_or(self.open));
        }
        if host::take_open(id) {
            self.unfolded = Some(true);
            host::set_fold(&host::fold_key(&self.title, &paths), true);
            self.reveal_open = true;
        }
        let reveal = host::reveal_rev();
        let picked = host::picked(&paths);
        if self.reveal_seen.is_some_and(|r| r != reveal) && picked {
            self.unfolded = Some(true);
        }
        self.reveal_seen = Some(reveal);
        let open = self.unfolded == Some(true) || !host::search().is_empty();
        // While the film plays what matters moves with the playhead.
        if lit.is_some() && host::playhead().is_some_and(|p| p.2) {
            cx.redraw_area_in_draw(self.draw_head.area());
        }
        let tint = parse_code_color(&self.tint).map(|c| vec4(c[0] as f32, c[1] as f32, c[2] as f32, 1.0)).unwrap_or(color(p.accent, 1.0));
        let faded = |c: Vec4f, a: f32| vec4(c.x, c.y, c.z, c.w * a);
        let selected = host::selected_module() == Some(id);
        // The card: a rounded panel under the heading and what it holds,
        // edged in the module's colour while it is the selected one.
        self.draw_card.shape = 0.0;
        self.draw_card.radius = R_CARD;
        self.draw_card.stroke = 1.0;
        self.draw_card.fill = color(p.row, 1.0);
        // (Holding the picture's selection: edged in its colour too.)
        self.draw_card.edge = if selected || picked { faded(tint, 0.75) } else { color(p.separator, 1.0) };
        self.draw_card.begin(cx, walk, Layout::flow_down());
        // One line: the fold chevron, the module's colour as a dot (the
        // bypass switch when it has one; faint while it does not act at the
        // playhead), the title, when it acts and how many controls it holds,
        // its spans in the film, and "···" (its actions).
        let head = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(30.0)));
        self.spots.clear();
        let cy = head.pos.y + head.size.y * 0.5;
        let mut x = head.pos.x + 8.0;
        self.draw_frame.chevron(cx, Rect { pos: dvec2(x, cy - 6.0), size: dvec2(12.0, 12.0) }, color(p.muted, 1.0), open);
        x += 16.0;
        let lit_a = if lit == Some(false) { 0.4 } else { 1.0 };
        if !self.bypass.is_empty() {
            let on = host::read(&self.bypass).is_some_and(|v| v.code().trim() == "true");
            let r = Rect { pos: dvec2(x, cy - 5.0), size: dvec2(10.0, 10.0) };
            if on {
                self.draw_frame.fill_round(cx, r, faded(tint, lit_a), 5.0);
            } else {
                self.draw_frame.edged_round(cx, r, color(0, 0.0), color(p.muted, 1.0), 1.2, 5.0);
            }
            self.spots.push((Rect { pos: r.pos - dvec2(4.0, 4.0), size: r.size + dvec2(8.0, 8.0) }, Spot::Bypass));
        } else {
            self.draw_frame.fill_round(cx, Rect { pos: dvec2(x + 1.0, cy - 4.0), size: dvec2(8.0, 8.0) }, faded(tint, lit_a), 4.0);
        }
        x += 18.0;
        // The right end: "···", and the spans bar before it.
        let more = Rect { pos: dvec2(head.pos.x + head.size.x - 32.0, cy - 10.0), size: dvec2(26.0, 20.0) };
        let menu_on = self.menu_open && open;
        let hot = self.hot == Some(Spot::Menu);
        let look = p.pick(menu_on, hot, 0);
        if menu_on {
            self.draw_frame.edged_round(cx, more, color(look.fill, 1.0), color(look.edge, 1.0), 1.0, R_CONTROL);
        } else if hot {
            self.draw_frame.fill_round(cx, more, color(look.fill, 1.0), R_CONTROL);
        }
        self.draw_small.color = color(if menu_on { look.ink } else { p.muted }, 1.0);
        let dots_w = makepad_widgets::badge::advance(&self.draw_small, cx, "···");
        self.draw_small.draw_abs(cx, dvec2(more.pos.x + (more.size.x - dots_w) * 0.5, more.pos.y + 4.0), "···");
        self.spots.push((more, Spot::Menu));
        let spans = host::spans(&paths);
        let mut right = more.pos.x - 8.0;
        if let (false, Some((t, length, _))) = (spans.is_empty(), host::playhead()) {
            let length = length.max(0.001);
            let bar = Rect { pos: dvec2(right - 56.0, cy - 2.0), size: dvec2(56.0, 4.0) };
            self.draw_frame.fill_round(cx, bar, color(p.well, 1.0), 2.0);
            for (a, b) in spans {
                let x0 = bar.pos.x + bar.size.x * (a / length).clamp(0.0, 1.0);
                let x1 = bar.pos.x + bar.size.x * (b / length).clamp(0.0, 1.0);
                self.draw_frame.fill_round(cx, Rect { pos: dvec2(x0, bar.pos.y), size: dvec2((x1 - x0).max(2.0), bar.size.y) }, faded(tint, 0.85), 2.0);
            }
            let ph = bar.pos.x + bar.size.x * (t / length).clamp(0.0, 1.0);
            self.draw_frame.fill_round(cx, Rect { pos: dvec2(ph - 1.0, bar.pos.y - 3.0), size: dvec2(2.0, bar.size.y + 6.0) }, color(p.text_hi, 1.0), 1.0);
            right = bar.pos.x - 10.0;
        }
        // The title, cut to its room; when it acts and how many, after it
        // while there is room.
        let room = (right - x).max(20.0);
        let (title, _) = fit_text(&self.draw_title, cx, &self.title, room);
        self.draw_title.color = color(p.text_hi, if lit == Some(false) { 0.7 } else { 1.0 });
        self.draw_title.draw_abs(cx, dvec2(x, cy - 6.0), &title);
        let tx = x + makepad_widgets::badge::advance(&self.draw_title, cx, &title) + 8.0;
        let said = match (self.when.is_empty(), controls) {
            (true, 1) => "1 control".to_string(),
            (true, n) => format!("{n} controls"),
            (false, n) => format!("{} · {n}", self.when),
        };
        if right - tx > 30.0 {
            let (said, _) = fit_text(&self.draw_small, cx, &said, right - tx);
            self.draw_small.color = color(p.muted, 1.0);
            self.draw_small.draw_abs(cx, dvec2(tx, cy - 5.0), &said);
        }
        let chip = |s: &mut Self, cx: &mut Cx2d, x: f64, y: f64, label: &str, spot: Spot, on: bool| -> f64 {
            let w = makepad_widgets::badge::advance(&s.draw_small, cx, label) + 16.0;
            let r = Rect { pos: dvec2(x, y), size: dvec2(w, 20.0) };
            let fill = if on { faded(tint, 1.0) } else if s.hot == Some(spot) { color(p.control_hover, 1.0) } else { color(p.raised, 1.0) };
            s.draw_frame.fill_round(cx, r, fill, R_CONTROL);
            s.draw_small.color = color(if on { p.on_accent } else { p.text }, 1.0);
            s.draw_small.draw_abs(cx, dvec2(r.pos.x + 8.0, r.pos.y + 5.0), label);
            s.spots.push((r, spot));
            w
        };
        // An ask in flight: polled each frame until it is done.
        if let Some(id) = self.ask {
            self.ask_said = host::ask_status(id);
            if self.ask_said == "working" {
                cx.redraw_area_in_draw(self.draw_head.area());
            } else {
                self.ask = None;
                host::changed();
            }
        }
        // The actions ("···"), each named, in a rounded well of two rows: the
        // snapshots and the morph between 1 and 2; then randomise, the
        // colours' lock (where it holds colours), reset and AI (where the
        // host takes asks).
        if open && self.menu_open {
            let rows = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(58.0)));
            let well = Rect { pos: dvec2(rows.pos.x + 8.0, rows.pos.y), size: dvec2(rows.size.x - 16.0, 54.0) };
            self.draw_head.fill_round(cx, well, color(p.well, 0.7), R_GROUP);
            let (y1, y2) = (well.pos.y + 4.0, well.pos.y + 30.0);
            let mut x = well.pos.x + 8.0;
            self.draw_small.color = color(p.muted, 1.0);
            self.draw_small.draw_abs(cx, dvec2(x, y1 + 5.0), "Snapshots");
            x += makepad_widgets::badge::advance(&self.draw_small, cx, "Snapshots") + 8.0;
            for k in 0..4 {
                let stored = state.slots.get(k).is_some_and(|s| s.is_some());
                x += chip(self, cx, x, y1, ["1", "2", "3", "4"][k], Spot::Slot(k), stored) + 4.0;
            }
            let has_ab = state.slots.first().is_some_and(|s| s.is_some()) && state.slots.get(1).is_some_and(|s| s.is_some());
            if has_ab {
                x += 10.0;
                self.draw_small.color = color(p.muted, 1.0);
                self.draw_small.draw_abs(cx, dvec2(x, y1 + 5.0), "Morph 1–2");
                x += makepad_widgets::badge::advance(&self.draw_small, cx, "Morph 1–2") + 8.0;
                let room = (well.pos.x + well.size.x - 12.0 - x).min(140.0);
                if room >= 30.0 {
                    let track = Rect { pos: dvec2(x, y1 + 8.0), size: dvec2(room, 4.0) };
                    self.draw_frame.fill_round(cx, track, color(p.track, 1.0), 2.0);
                    self.draw_frame.edged_round(cx, Rect { pos: dvec2(track.pos.x + track.size.x * self.morph - 6.0, y1 + 4.0), size: dvec2(12.0, 12.0) }, tint, color(p.row, 1.0), 1.0, 6.0);
                    self.spots.push((Rect { pos: dvec2(track.pos.x - 4.0, y1 - 2.0), size: dvec2(room + 8.0, 24.0) }, Spot::Morph));
                }
            }
            let mut x = well.pos.x + 8.0;
            x += chip(self, cx, x, y2, "Randomise", Spot::Dice, false) + 4.0;
            if self.bounds().iter().any(|b| b.kind == BoundKind::Colour) {
                let keep = self.keep_colours;
                x += chip(self, cx, x, y2, if keep { "Colours kept" } else { "Colours too" }, Spot::Lock, keep) + 4.0;
            }
            x += chip(self, cx, x, y2, "Reset", Spot::Reset, false) + 4.0;
            if host::can_ask() {
                let on = self.asking_open || self.ask_said == "working";
                chip(self, cx, x, y2, "AI…", Spot::Ai, on);
            }
        }
        if open && ((self.menu_open && self.asking_open) || !self.ask_said.is_empty()) {
            let row = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(28.0)));
            let well = Rect { pos: dvec2(row.pos.x + 8.0, row.pos.y), size: dvec2(row.size.x - 16.0, 26.0) };
            self.draw_head.fill_round(cx, well, color(p.well, 0.7), R_GROUP);
            if self.menu_open && self.asking_open {
                let mut x = well.pos.x + 4.0;
                for (k, (label, _, _)) in ASKS.iter().enumerate() {
                    x += chip_at(self, cx, x, well.pos.y + 3.0, label, Spot::Ask(k), &p, tint) + 4.0;
                }
            } else {
                let said = match self.ask_said.as_str() {
                    "working" => "AI: working…".to_string(),
                    s => format!("AI: {s}"),
                };
                self.draw_small.color = color(if self.ask_said.starts_with("failed") { p.danger } else { p.text }, 1.0);
                self.draw_small.draw_abs(cx, dvec2(well.pos.x + 8.0, well.pos.y + 8.0), &said);
            }
        }
        let below = cx.turtle().pos().y;
        self.draw_hit.color = color(0, 0.0);
        self.draw_hit.draw_abs(cx, Rect { pos: head.pos, size: dvec2(head.size.x, below - head.pos.y) });
        // What the pointer is on says what it does.
        let header_tip = self.hot.and_then(|spot| {
            let r = self.spots.iter().find(|(_, s)| *s == spot)?.0;
            let stored = matches!(spot, Spot::Slot(k) if state.slots.get(k).is_some_and(|s| s.is_some()));
            Some((r, spot.tip(stored, self.keep_colours)))
        });
        // The controls: each registers with this module as it draws.
        // Folded, nothing below the heading.
        host::module_begin(id);
        let upto = if open { kids.len() } else { 0 };
        if upto > 0 {
            cx.begin_turtle(Walk::new(Size::fill(), Size::fit()), self.view.layout.clone());
            for (k, ps) in kids.iter().zip(&kid_paths).take(upto) {
                if title_hit || host::found(ps) {
                    host::set_title_found(title_hit);
                    k.draw_all(cx, scope);
                    host::set_title_found(false);
                }
            }
            cx.end_turtle();
        }
        host::module_end();
        self.draw_card.end(cx);
        let rect = self.draw_card.area().rect(cx);
        if std::mem::take(&mut self.reveal_open) && open {
            host::opened(rect);
        }
        host::placed_module(cx, &mut self.place, id, rect, paths);
        host::tree_module(id, &self.title, rect);
        // What does not matter now, dimmed (the module, or each control of
        // it); what acts now is simply not dimmed (no marks: they clashed).
        let veil = color(p.row, 0.6);
        self.draw_dim.new_draw_call(cx);
        match lit {
            Some(false) => self.draw_dim.fill_round(cx, rect, veil, R_CARD),
            Some(true) if open => {
                for (r, paths) in host::module_places(id) {
                    if host::activity(&paths) == Some(false) {
                        self.draw_dim.fill_round(cx, r, veil, R_CONTROL);
                    }
                }
            }
            _ => {}
        }
        // A control's note for the pointer, over everything in the module
        // (else what the heading's button under it does, under it).
        let tip = host::take_tip().or_else(|| header_tip.map(|(r, note)| (Rect { pos: dvec2(r.pos.x, r.pos.y + r.size.y + 2.0), size: dvec2(makepad_widgets::badge::advance(&self.draw_small, cx, &note) + 12.0, 18.0) }, note, false)));
        if let Some((r, note, offer)) = tip {
            // A note stays inside the module; the offer is where it is clicked.
            let x = if offer { r.pos.x } else { r.pos.x.min(rect.pos.x + rect.size.x - r.size.x).max(rect.pos.x) };
            let r = Rect { pos: dvec2(x, r.pos.y), size: r.size };
            self.draw_dim.edged_round(cx, r, color(if offer { p.raised } else { p.menu }, 1.0), color(p.border, 1.0), 1.0, R_CONTROL);
            self.draw_small.new_draw_call(cx);
            self.draw_small.color = color(if offer { p.text_hi } else { p.text }, 1.0);
            self.draw_small.draw_abs(cx, dvec2(r.pos.x + 6.0, r.pos.y + 4.0), &note);
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------- mood pad

#[derive(Script, ScriptHook, Widget)]
pub struct PanelMood {
    /// When in the film it acts: the host reads it from the panel's code.
    #[live]
    pub when: String,
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_cell: DrawPanelShape,
    #[live]
    draw_small: DrawText,
    /// The module it morphs (the one it is drawn in).
    #[rust]
    module: Option<u64>,
    #[rust]
    puck: DVec2,
    #[rust]
    rect: Rect,
    #[rust]
    dragging: bool,
}

impl PanelMood {
    fn move_to(&mut self, cx: &mut Cx, abs: DVec2) {
        let Some(id) = self.module else { return };
        let t = dvec2(((abs.x - self.rect.pos.x) / self.rect.size.x).clamp(0.0, 1.0), ((abs.y - self.rect.pos.y) / self.rect.size.y).clamp(0.0, 1.0));
        self.puck = t;
        let state = host::module(id);
        // Corners: 1 top left, 2 top right, 3 bottom left, 4 bottom right.
        let w = [(1.0 - t.x) * (1.0 - t.y), t.x * (1.0 - t.y), (1.0 - t.x) * t.y, t.x * t.y];
        let sets: Vec<(&[(String, String)], f64)> = (0..4).filter_map(|k| state.slots.get(k).and_then(|s| s.as_deref()).map(|s| (s, w[k]))).collect();
        if sets.is_empty() {
            return;
        }
        for (path, value) in blend(&state.bounds, &sets) {
            host::set(&path, value);
        }
        host::changed();
        self.draw_bg.redraw(cx);
    }
}

impl Widget for PanelMood {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Crosshair),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.dragging = true;
                self.move_to(cx, fe.abs);
            }
            Hit::FingerMove(fe) if self.dragging => self.move_to(cx, fe.abs),
            Hit::FingerUp(fe) if self.dragging => {
                self.dragging = false;
                self.move_to(cx, fe.abs);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.module = host::module_current();
        let p = makepad_look::palette::get(cx);
        let rect = cx.walk_turtle(walk);
        self.rect = rect;
        self.draw_bg.color = color(0, 0.0);
        self.draw_bg.draw_abs(cx, rect);
        self.draw_cell.edged_round(cx, rect, color(p.well, 1.0), color(p.line_faint, 1.0), 1.0, R_GROUP);
        let state = self.module.map(host::module).unwrap_or_default();
        // The corners: each slot, said by its number; an empty one dim.
        let corners = [rect.pos, dvec2(rect.pos.x + rect.size.x - 18.0, rect.pos.y), dvec2(rect.pos.x, rect.pos.y + rect.size.y - 16.0), rect.pos + rect.size - dvec2(18.0, 16.0)];
        for (k, c) in corners.iter().enumerate() {
            let stored = state.slots.get(k).is_some_and(|s| s.is_some());
            self.draw_small.color = color(if stored { p.text_hi } else { p.muted }, 1.0);
            self.draw_small.draw_abs(cx, *c + dvec2(5.0, 3.0), &(k + 1).to_string());
        }
        if state.slots.iter().filter(|s| s.is_some()).count() < 2 {
            self.draw_small.color = color(p.muted, 1.0);
            self.draw_small.draw_abs(cx, rect.pos + dvec2(28.0, rect.size.y * 0.5 - 6.0), "store presets in the slots, then move between them");
        }
        let c = rect.pos + dvec2(rect.size.x * self.puck.x, rect.size.y * self.puck.y);
        self.draw_cell.edged_round(cx, Rect { pos: c - dvec2(7.0, 7.0), size: dvec2(14.0, 14.0) }, color(p.accent, 1.0), color(p.text_hi, 1.0), 1.5, 7.0);
        DrawStep::done()
    }
}
