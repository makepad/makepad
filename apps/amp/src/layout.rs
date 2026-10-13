//! Layouts: where the device's components sit, independent of the style.
//!
//! A layout is a Splash file in `resources/layouts/<name>.splash` that sets
//! `mod.widgets.HwLayout`: a tree of Views and splitters whose leaves are
//! `HwSlot{component: "…"}` markers. The components themselves (the screen,
//! the transport keys, the wings, the lists, …) live once, in `HwStage`, and
//! are drawn into their slots' rects. Switching layout therefore swaps only
//! the slots: every component keeps its state (playback, the MilkDrop
//! picture, scroll positions), and moves to its new rect with a short tween.
//!
//! A layout's first lines say what it is and what room it needs:
//! `// title: Name`, `// order: N`, `// min: WxH`, `// pref: WxH`. The
//! person's pick is used while the window is at least its minimum; below
//! it the largest layout that fits takes over (with some hysteresis), and
//! the pick comes back when the window grows again.

use makepad_widgets::*;
use std::collections::HashMap;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.HwSlotBase = #(HwSlot::register_widget(vm))
    mod.widgets.HwSlot = set_type_default() do mod.widgets.HwSlotBase{
        width: Fill
        height: Fill
    }

    mod.widgets.HwStageBase = #(HwStage::register_widget(vm))
    mod.widgets.HwStage = set_type_default() do mod.widgets.HwStageBase{
        width: Fill
        height: Fill
        flow: Overlay
    }

    // Until a layout file is read: everything in one column.
    mod.widgets.HwLayout = View{
        width: Fill
        height: Fill
        flow: Down
        mod.widgets.HwSlot{component: "screen"}
        mod.widgets.HwSlot{component: "transport" height: 80}
        mod.widgets.HwSlot{component: "tracks"}
    }
}

/// The components a layout can place, by slot name; each is `c_<name>` in
/// the stage.
pub const COMPONENTS: [&str; 20] = ["info", "spectrum", "plhead", "plist", "toggles", "volume", "screen", "transport", "display", "now", "progress", "queue", "library", "tracks", "upnext", "chat", "clock", "marquee", "eq", "deck"];

/// The compact forms Amp declares to a window manager that parks it
/// (`Cx::declare_compact_forms`): a layout of the same name is shown for
/// each (`resources/layouts/strip.splash`, `tile.splash`).
pub fn compact_forms() -> Vec<makepad_widgets::makepad_platform::ime::CompactForm> {
    use makepad_widgets::makepad_platform::ime::CompactForm;
    vec![CompactForm::flexible("strip", 320.0, 130.0, 100.0), CompactForm::flexible("tile", 220.0, 130.0, 220.0)]
}

/// How long a layout change moves the components, seconds.
const TWEEN: f64 = 0.32;

/// The size a component is drawn at decides how it shows itself
/// (breakpoints on its own rect, the same for every layout and style).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Fit {
    #[default]
    Full,
    Compact,
    Tiny,
    /// Pocket size: small keys, thin wells (the Winamp-sized layouts).
    Micro,
}

/// The breakpoints, per component: (compact below, tiny below), on the
/// component's width (w) or height (h).
pub fn fit_of(name: &str, rect: Rect) -> Fit {
    let (w, h) = (rect.size.x, rect.size.y);
    let by = |v: f64, compact: f64, tiny: f64| {
        if v < tiny {
            Fit::Tiny
        } else if v < compact {
            Fit::Compact
        } else {
            Fit::Full
        }
    };
    match name {
        // Six keys and their spacing take 476 pt; previous, play and next
        // 260; the lit dashes under the keys need 76 pt of height.
        "transport" if h < 44.0 || w < 260.0 => Fit::Micro,
        "transport" if w < 476.0 => Fit::Tiny,
        "transport" if h < 76.0 => Fit::Compact,
        "transport" => Fit::Full,
        "progress" if h < 30.0 => Fit::Micro,
        // The deck's words need 580 pt; under that its keys are glyphs.
        // Under 400 the name window goes too (the mode and the keys stay).
        "deck" => by(w, 580.0, 400.0),
        "screen" if h < 90.0 || w < 160.0 => Fit::Micro,
        "now" => by(h, 50.0, 30.0),
        // Measured (walnut, 09-24): the volume wing's knob, meter, output
        // and title need 335 pt; without the title and meter 220. The
        // display wing's keys, choosers, EQ/colours need 380;
        // without the choosers and preset name 280.
        // Too short for the wing's knob: a slider (and the meter under it
        // from 44 pt).
        "volume" if h < 150.0 => Fit::Micro,
        "volume" => by(h, 335.0, 220.0),
        "display" => by(h, 380.0, 280.0),
        "progress" => by(w, 260.0, 140.0),
        "tracks" => by(w, 640.0, 380.0),
        "eq" => by(w.min(h * 2.6), 600.0, 380.0),
        _ => Fit::Full,
    }
}

// ---- the slot ----

/// Where a component goes: it takes its room in the layout and reports it.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct HwSlot {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    component: String,
    /// Drawing order among the components: higher is drawn later, over
    /// the others (0 = the stage's own order).
    #[live]
    z: f64,
    /// A backing drawn under the component (a scrim over a picture for
    /// legibility); alpha 0 = none.
    #[live]
    scrim: Vec4f,
    #[rust]
    area: Area,
}

impl WidgetNode for HwSlot {
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

impl Widget for HwSlot {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.walk_turtle_with_area(&mut self.area, walk);
        // The area, not the rect: the layout may still move it once the
        // views round it finish (a Fill before a fixed sibling).
        cx.global::<Slots>().rects.push((self.component.clone(), self.area));
        cx.global::<Slots>().looks.insert(self.component.clone(), (self.z, self.scrim));
        DrawStep::done()
    }
}

#[derive(Default)]
struct Slots {
    rects: Vec<(String, Area)>,
    /// Per component: its slot's `z` and `scrim`.
    looks: HashMap<String, (f64, Vec4f)>,
}

// ---- the stage ----

/// The components, drawn into the rects the layout's slots report.
#[derive(Script, ScriptHook, Widget)]
pub struct HwStage {
    #[deref]
    view: View,
    /// A slot's backing (`scrim`).
    #[live]
    draw_scrim: DrawColor,
    /// Where each component was drawn last, by name.
    #[rust]
    shown: HashMap<String, Rect>,
    /// A layout change: where the components were when it started.
    #[rust]
    from: HashMap<String, Rect>,
    #[rust]
    tween_at: Option<f64>,
    #[rust]
    next_frame: NextFrame,
    /// How each component fits its rect.
    #[rust]
    pub fits: HashMap<String, Fit>,
    /// The EQ panel is open and the layout has no `eq` slot: it is drawn
    /// as a drawer rising over the main list, or as a popover on a device
    /// with no list (`eq.rs`); since when, for its rise.
    #[rust]
    eq_open: bool,
    #[rust]
    eq_opened_at: Option<f64>,
    /// The panels host (the `tracks` slot): the pages its tab keys offer
    /// and the page it shows (`tracks`, `eq`), and since when,
    /// for the page's slide in.
    #[rust]
    pub tab_pages: Vec<String>,
    #[rust]
    pub page: String,
    #[rust]
    page_at: Option<f64>,
    /// The AI stylist is open: a layout without a `chat` slot shows it as a
    /// drawer over the stage.
    #[rust]
    chat_open: bool,
}

/// Height of the panels host's tab keys (and the gap under them).
const TABS_H: f64 = 38.0;

impl HwStage {
    /// The next layout the stage draws moves the components there from
    /// where they are now.
    pub fn begin_tween(&mut self, cx: &mut Cx) {
        self.from = self.shown.clone();
        self.tween_at = Some(cx.seconds_since_app_start());
        self.view.redraw(cx);
    }

    /// Where the EQ drawer goes, `rise` (0..1) of the way up: over the
    /// lower part of the main list, or centred on a device without one.
    fn eq_drawer_rect(stage: Rect, tracks: Option<Rect>, rise: f64) -> Rect {
        match tracks {
            Some(t) if t.size.x >= 360.0 && t.size.y >= 200.0 => {
                let h = (t.size.y * 0.85).clamp(240.0, 460.0).min(t.size.y);
                let y = t.pos.y + t.size.y - h + (1.0 - rise) * h.min(48.0);
                Rect { pos: dvec2(t.pos.x, y), size: dvec2(t.size.x, h) }
            }
            _ => {
                let w = (stage.size.x - 16.0).min(620.0).max(1.0);
                let h = (stage.size.y - 16.0).min(300.0).max(1.0);
                let y = stage.pos.y + (stage.size.y - h) * 0.5 + (1.0 - rise) * 24.0;
                Rect { pos: dvec2(stage.pos.x + (stage.size.x - w) * 0.5, y), size: dvec2(w, h) }
            }
        }
    }
}

impl Widget for HwStage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.view.redraw(cx);
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::flow_overlay());
        cx.global::<Slots>().rects.clear();
        let slots = self.view.widget(cx, ids!(slots));
        slots.draw_walk_all(cx, scope, Walk::fill());
        cx.global::<Slots>().looks.clear();
        let slot_areas = std::mem::take(&mut cx.global::<Slots>().rects);
        let looks = std::mem::take(&mut cx.global::<Slots>().looks);
        let targets: Vec<(String, Rect)> = slot_areas.into_iter().map(|(name, area)| (name, area.rect(cx))).collect();
        let now = cx.seconds_since_app_start();
        let t = self.tween_at.map(|t0| ((now - t0) / TWEEN).clamp(0.0, 1.0)).unwrap_or(1.0);
        // Ease out: quick to leave, settling into place.
        let e = 1.0 - (1.0 - t).powi(3);
        let mut shown = HashMap::new();
        // The panels host: the `tracks` slot shows one page at a time (the
        // track list, the equalizer) under tab keys; a panel with a
        // slot of its own is not a page; `eq:open` places
        // an open panel in its own slot (a band over the picture).
        let has = |n: &str| targets.iter().any(|(t, _)| t == n);
        let host = has("tracks");
        let mut tab_pages = vec!["tracks"];
        for (panel, open, at) in [("eq", self.eq_open, self.eq_opened_at)] {
            let _ = (open, at);
            if !has(panel) && !has(&format!("{panel}:open")) {
                tab_pages.push(panel);
            }
        }
        // The page: the panel opened last, else the track list.
        let mut page = "tracks";
        let mut latest = f64::MIN;
        for (panel, open, at) in [("eq", self.eq_open, self.eq_opened_at)] {
            if open && tab_pages.contains(&panel) && at.unwrap_or(0.0) >= latest {
                latest = at.unwrap_or(0.0);
                page = panel;
            }
        }
        if !host {
            tab_pages.clear();
        }
        if page != self.page {
            self.page = page.to_string();
            self.page_at = Some(now);
        }
        self.tab_pages = tab_pages.iter().map(|p| p.to_string()).collect();
        let page_rise = self.page_at.map(|t0| ((now - t0) / 0.2).clamp(0.0, 1.0)).unwrap_or(1.0);
        // In the stage's order, re-sorted by the slots' `z` (stable).
        let mut order: Vec<&str> = COMPONENTS.to_vec();
        order.sort_by(|a, b| {
            let za = looks.get(*a).map(|l| l.0).unwrap_or(0.0);
            let zb = looks.get(*b).map(|l| l.0).unwrap_or(0.0);
            za.partial_cmp(&zb).unwrap_or(std::cmp::Ordering::Equal)
        });
        for name in order {
            let Some((_, target)) = targets.iter().find(|(n, _)| n == name) else { continue };
            // The host's rect goes to its tab keys and its page.
            if name == "tracks" && tab_pages.len() > 1 {
                continue;
            }
            let rect = match self.from.get(name) {
                Some(from) if t < 1.0 => Rect {
                    pos: from.pos + (target.pos - from.pos) * e,
                    size: from.size + (target.size - from.size) * e,
                },
                _ => *target,
            };
            if rect.size.x < 1.0 || rect.size.y < 1.0 {
                continue;
            }
            self.fits.insert(name.to_string(), fit_of(name, rect));
            shown.insert(name.to_string(), rect);
            if let Some((_, scrim)) = looks.get(name) {
                if scrim.w > 0.0 {
                    self.draw_scrim.color = *scrim;
                    self.draw_scrim.draw_abs(cx, rect);
                }
            }
            let comp = self.view.widget(cx, &[live_id!(components), LiveId::from_str(&format!("c_{name}"))]);
            comp.draw_walk_all(cx, scope, Walk::abs_rect(rect));
        }
        let mut rising = false;
        if host && tab_pages.len() > 1 {
            let (_, target) = targets.iter().find(|(n, _)| n == "tracks").unwrap();
            let rect = match self.from.get("tracks") {
                Some(from) if t < 1.0 => Rect { pos: from.pos + (target.pos - from.pos) * e, size: from.size + (target.size - from.size) * e },
                _ => *target,
            };
            if rect.size.x >= 1.0 && rect.size.y > TABS_H + 20.0 {
                let tabs = Rect { pos: rect.pos, size: dvec2(rect.size.x, TABS_H - 4.0) };
                let body = Rect { pos: dvec2(rect.pos.x, rect.pos.y + TABS_H), size: dvec2(rect.size.x, rect.size.y - TABS_H) };
                self.view.widget(cx, &[live_id!(components), live_id!(c_tabs)]).draw_walk_all(cx, scope, Walk::abs_rect(tabs));
                // The page slides up into place.
                rising = page_rise < 1.0;
                let lift = 1.0 - (1.0 - page_rise).powi(3);
                let shown_rect = Rect { pos: dvec2(body.pos.x, body.pos.y + (1.0 - lift) * 18.0), size: body.size };
                self.fits.insert(page.to_string(), fit_of(page, body));
                shown.insert(page.to_string(), body);
                shown.insert("tracks_host".to_string(), rect);
                let comp = self.view.widget(cx, &[live_id!(components), LiveId::from_str(&format!("c_{page}"))]);
                comp.draw_walk_all(cx, scope, Walk::abs_rect(shown_rect));
            }
        }
        // An open panel with an `…:open` slot of its own.
        for (panel, open) in [("eq", self.eq_open)] {
            if !open {
                continue;
            }
            if let Some((_, rect)) = targets.iter().find(|(n, _)| *n == format!("{panel}:open")) {
                if rect.size.x >= 1.0 && rect.size.y >= 1.0 {
                    self.fits.insert(panel.to_string(), fit_of(panel, *rect));
                    shown.insert(panel.to_string(), *rect);
                    let comp = self.view.widget(cx, &[live_id!(components), LiveId::from_str(&format!("c_{panel}"))]);
                    comp.draw_walk_all(cx, scope, Walk::abs_rect(*rect));
                }
            }
        }
        // No host and no slot: the EQ panel, while open, over everything
        // else (a pocket device's popover).
        if self.eq_open && !host && !has("eq") && !has("eq:open") {
            let tracks = shown.get("tracks").copied();
            let rise = self.eq_opened_at.map(|t0| ((now - t0) / 0.22).clamp(0.0, 1.0)).unwrap_or(1.0);
            rising = rise < 1.0;
            let rect = Self::eq_drawer_rect(cx.turtle().rect(), tracks, 1.0 - (1.0 - rise).powi(3));
            self.fits.insert("eq".to_string(), fit_of("eq", rect));
            shown.insert("eq".to_string(), rect);
            let comp = self.view.widget(cx, &[live_id!(components), live_id!(c_eq)]);
            comp.draw_walk_all(cx, scope, Walk::abs_rect(rect));
        }
        // No `chat` slot: the open stylist as a drawer, over the right of a
        // wide stage, over all of a narrow one.
        if self.chat_open && !has("chat") {
            let stage = cx.turtle().rect();
            let rect = if stage.size.x >= 700.0 {
                let w = (stage.size.x * 0.42).clamp(340.0, 440.0);
                Rect { pos: dvec2(stage.pos.x + stage.size.x - w, stage.pos.y), size: dvec2(w, stage.size.y) }
            } else {
                stage
            };
            shown.insert("chat".to_string(), rect);
            let comp = self.view.widget(cx, &[live_id!(components), live_id!(c_chat)]);
            comp.draw_walk_all(cx, scope, Walk::abs_rect(rect));
        }
        self.shown = shown;
        if t < 1.0 || rising {
            self.next_frame = cx.new_next_frame();
        } else {
            self.tween_at = None;
            self.from.clear();
        }
        cx.end_turtle();
        DrawStep::done()
    }
}

impl HwStageRef {
    /// Open or close the EQ drawer (where the layout has no `eq` slot).
    pub fn set_eq_open(&self, cx: &mut Cx, open: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.eq_open != open {
                inner.eq_open = open;
                inner.eq_opened_at = open.then(|| cx.seconds_since_app_start());
                inner.view.redraw(cx);
            }
        }
    }

    pub fn begin_tween(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.begin_tween(cx);
        }
    }

    pub fn fits(&self) -> HashMap<String, Fit> {
        self.borrow().map(|inner| inner.fits.clone()).unwrap_or_default()
    }

    /// The panels host's tab pages and the page shown (panels.rs).
    pub fn panels(&self) -> (Vec<String>, String) {
        self.borrow().map(|inner| (inner.tab_pages.clone(), inner.page.clone())).unwrap_or_default()
    }

    /// The AI stylist open or shut (drawn as a drawer where the layout has
    /// no `chat` slot).
    pub fn set_chat_open(&self, cx: &mut Cx, open: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.chat_open != open {
                inner.chat_open = open;
                inner.view.redraw(cx);
            }
        }
    }

    /// Where a component was last drawn (its slot's rect, mid-tween too).
    pub fn shown(&self, name: &str) -> Option<Rect> {
        self.borrow().and_then(|inner| inner.shown.get(name).copied())
    }
}

// ---- layout files ----

use crate::hw::EMBEDDED_LAYOUTS;

pub const DEFAULT_LAYOUT: &str = "classic";

/// The window's size class. Tiny, small and medium each have one layout;
/// only a large window has a choice of layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SizeClass {
    /// The compact forms a host asks for when it parks the window small
    /// (`set_compact`): never picked by the window's size.
    Pocket,
    Tiny,
    Small,
    Medium,
    Large,
}

impl SizeClass {
    fn parse(text: &str) -> SizeClass {
        match text.trim() {
            "pocket" => SizeClass::Pocket,
            "tiny" => SizeClass::Tiny,
            "small" => SizeClass::Small,
            "medium" => SizeClass::Medium,
            _ => SizeClass::Large,
        }
    }
}

/// One layout as the picker lists it.
#[derive(Clone, Debug)]
pub struct LayoutEntry {
    pub name: String,
    pub title: String,
    pub min: Vec2d,
    /// `// class: tiny|small|medium|large` (large when missing).
    pub class: SizeClass,
    /// `// pref: WxH`: the window size it is made for.
    pub pref: Option<Vec2d>,
    order: f64,
}

/// Which layout the person picked, which is on screen, and the splitters'
/// positions per layout.
#[derive(Default)]
pub struct HwLayoutState {
    /// The person's pick (`layout.txt`), or `--layout`.
    pub chosen: String,
    /// The layout on screen: the pick, or the largest that fits the window.
    pub active: String,
    pub pinned: bool,
    pub stamp: Option<std::time::SystemTime>,
    /// Cycle layouts (and styles) on a timer, for demos.
    pub showcase: bool,
    pub showcase_styles: bool,
    /// A layout switch being timed (`MUSIC_SWITCH_TRACE`): when it began.
    pub switch_t0: Option<f64>,
    /// Layout changes are applied in place (`apply_in_place`), and one is
    /// waiting.
    pub in_place: bool,
    pub pending_in_place: bool,
    /// The host parked the window and granted this compact form's layout:
    /// it stays on screen whatever the window's size, until unparked.
    pub compact: Option<String>,
    /// The layout that was on screen when the compact form began.
    before_compact: Option<String>,
}

fn layout_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("layouts")
}

fn layout_path(name: &str) -> PathBuf {
    layout_dir().join(format!("{name}.splash"))
}

pub fn stamp(name: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(layout_path(name)).and_then(|m| m.modified()).ok()
}

pub fn layout_source(name: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(text) = std::fs::read_to_string(layout_path(name)) {
        return Some(text);
    }
    EMBEDDED_LAYOUTS.iter().find(|(n, _)| *n == name).map(|(_, s)| s.to_string())
}

fn header(source: &str, key: &str) -> Option<String> {
    source.lines().take(24).find_map(|line| {
        let rest = line.trim().strip_prefix("//")?.trim();
        let value = rest.strip_prefix(key)?.trim_start().strip_prefix(':')?;
        Some(value.trim().to_string())
    })
}

fn size_of(text: &str) -> Option<Vec2d> {
    let (w, h) = text.split_once(['x', 'X'])?;
    Some(dvec2(w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Every layout there is, in picker order.
pub fn layout_list() -> Vec<LayoutEntry> {
    let mut names: Vec<String> = EMBEDDED_LAYOUTS.iter().map(|(n, _)| n.to_string()).collect();
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(entries) = std::fs::read_dir(layout_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) == Some("splash") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if !names.iter().any(|n| n == stem) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    let mut list: Vec<LayoutEntry> = names
        .into_iter()
        .map(|name| {
            let source = layout_source(&name).unwrap_or_default();
            let title = header(&source, "title").unwrap_or_else(|| {
                let mut c = name.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            });
            let order = if name == DEFAULT_LAYOUT { -1.0 } else { header(&source, "order").and_then(|o| o.parse().ok()).unwrap_or(1000.0) };
            let min = header(&source, "min").and_then(|t| size_of(&t)).unwrap_or(dvec2(820.0, 560.0));
            let class = header(&source, "class").map(|c| SizeClass::parse(&c)).unwrap_or(SizeClass::Large);
            let pref = header(&source, "pref").and_then(|t| size_of(&t));
            LayoutEntry { name, title, min, class, pref, order }
        })
        .collect();
    list.sort_by(|a, b| a.order.partial_cmp(&b.order).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.title.cmp(&b.title)));
    list
}

/// How much a class's window must hold: the largest minimum among its
/// layouts, so every layout of the class fits from there up.
pub fn class_min(list: &[LayoutEntry], class: SizeClass) -> Option<Vec2d> {
    list.iter().filter(|e| e.class == class).map(|e| e.min).reduce(|a, b| dvec2(a.x.max(b.x), a.y.max(b.y)))
}

/// The class a window of `size` is in. A layout is never shown under its
/// minimum; a larger class than the current one is entered only
/// `HYSTERESIS` points past its minimum (no flip-flopping at a boundary).
pub fn class_of_size(list: &[LayoutEntry], size: Vec2d, current: SizeClass) -> SizeClass {
    for class in [SizeClass::Large, SizeClass::Medium, SizeClass::Small] {
        let Some(min) = class_min(list, class) else { continue };
        let slack = if class > current { HYSTERESIS } else { 0.0 };
        if size.x >= min.x + slack && size.y >= min.y + slack {
            return class;
        }
    }
    SizeClass::Tiny
}

/// Points under a class's minimum its layout is kept at.
pub const HYSTERESIS: f64 = 24.0;

/// The layout to show at `size`: the person's pick among the large
/// layouts when the window is large, else its class's one layout.
pub fn fit_layout(chosen: &str, current: &str, size: Vec2d) -> String {
    let list = layout_list();
    let current_class = list.iter().find(|e| e.name == current).map(|e| e.class).unwrap_or(SizeClass::Large);
    let class = class_of_size(&list, size, current_class);
    if class == SizeClass::Large {
        if let Some(pick) = list.iter().find(|e| e.name == chosen && e.class == SizeClass::Large) {
            return pick.name.clone();
        }
    }
    // The class's layout (the first in picker order), or the nearest
    // smaller class that has one.
    let mut at = class;
    loop {
        if let Some(entry) = list.iter().find(|e| e.class == at) {
            return entry.name.clone();
        }
        at = match at {
            SizeClass::Large => SizeClass::Medium,
            SizeClass::Medium => SizeClass::Small,
            SizeClass::Small => SizeClass::Tiny,
            SizeClass::Tiny | SizeClass::Pocket => return DEFAULT_LAYOUT.to_string(),
        };
    }
}

/// The class of the layout on screen.
pub fn active_class(cx: &mut Cx) -> SizeClass {
    let name = active(cx);
    layout_list().into_iter().find(|e| e.name == name).map(|e| e.class).unwrap_or(SizeClass::Large)
}

/// `--layout <name>`, `--layout=<name>` or `MUSIC_LAYOUT`.
fn pinned_layout() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for (at, arg) in args.iter().enumerate() {
        if let Some(name) = arg.strip_prefix("--layout=") {
            return Some(name.to_string());
        }
        if arg == "--layout" {
            if let Some(name) = args.get(at + 1) {
                return Some(name.clone());
            }
        }
    }
    std::env::var("MUSIC_LAYOUT").ok().filter(|s| !s.is_empty())
}

fn saved_path() -> Option<PathBuf> {
    Some(crate::worker::data_dir()?.join("layout.txt"))
}

/// Evaluate the layout on screen. Called from `App::script_mod` after the
/// style, before the app's tree.
pub fn apply_layout(vm: &mut ScriptVm) {
    if !vm.cx_mut().has_global::<HwLayoutState>() {
        let (chosen, pinned) = match pinned_layout() {
            Some(name) => (name, true),
            None => (
                saved_path()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| layout_source(s).is_some())
                    .unwrap_or_else(|| DEFAULT_LAYOUT.to_string()),
                false,
            ),
        };
        vm.cx_mut().set_global(HwLayoutState { active: chosen.clone(), chosen, pinned, ..Default::default() });
    }
    let name = vm.cx_mut().global::<HwLayoutState>().active.clone();
    let (name, code) = match layout_source(&name) {
        Some(code) => (name, code),
        None => {
            error!("amp: no layout named {name}; using {DEFAULT_LAYOUT}");
            match layout_source(DEFAULT_LAYOUT) {
                Some(code) => (DEFAULT_LAYOUT.to_string(), code),
                None => return,
            }
        }
    };
    vm.cx_mut().global::<HwLayoutState>().stamp = stamp(&name);
    vm.eval(ScriptMod::new(env!("CARGO_MANIFEST_DIR"), &format!("music_layout_{name}"), &format!("resources/layouts/{name}.splash"), 0, 0, &code, &[]));
}

pub fn active(cx: &mut Cx) -> String {
    if !cx.has_global::<HwLayoutState>() {
        return DEFAULT_LAYOUT.to_string();
    }
    cx.global::<HwLayoutState>().active.clone()
}

pub fn chosen(cx: &mut Cx) -> String {
    if !cx.has_global::<HwLayoutState>() {
        return DEFAULT_LAYOUT.to_string();
    }
    cx.global::<HwLayoutState>().chosen.clone()
}

/// Whether the layout on screen changes by itself, in place (the app
/// applies it to the stage's slots, `apply_in_place`), rather than by a
/// style reload of the whole app.
pub fn set_in_place(cx: &mut Cx, on: bool) {
    if cx.has_global::<HwLayoutState>() {
        cx.global::<HwLayoutState>().in_place = on;
    }
}

/// A layout change, made in place or by a style reload.
fn reload(cx: &mut Cx) {
    if cx.global::<HwLayoutState>().in_place {
        cx.global::<HwLayoutState>().pending_in_place = true;
    } else {
        cx.request_style_reload();
    }
}

/// The layout file on screen, evaluated again and applied to the stage's
/// slots (`slots`): only the arrangement changes; the components, the
/// style and their shaders stay as they are. For a layout switch that
/// draws in the frame it is made.
pub fn apply_in_place(cx: &mut Cx, slots: &mut WidgetRef) -> bool {
    if !std::mem::take(&mut cx.global::<HwLayoutState>().pending_in_place) {
        return false;
    }
    switch_mark(cx, "in place: begin");
    cx.with_vm(|vm| {
        apply_layout(vm);
        let widgets = vm.module(id!(widgets));
        let value = vm.bx.heap_ref().value(widgets, id!(HwLayout).into(), NoTrap);
        slots.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
    });
    switch_mark(cx, "in place: layout evaluated and applied to the slots");
    true
}

/// The person picked `name`: keep it, and show it if it fits.
pub fn choose(cx: &mut Cx, name: &str, size: Vec2d) -> bool {
    let state = cx.global::<HwLayoutState>();
    state.chosen = name.to_string();
    if !state.pinned {
        if let Some(path) = saved_path() {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
            let _ = std::fs::write(path, name);
        }
    }
    refit(cx, size)
}

/// The host parked the window and granted a compact form (`Some(layout
/// name)`), or the window is back at full size (`None`): show the form's
/// layout, or the one that was on screen before it. True when the layout
/// changed (a layout reload is asked for). No tween: the compact surface is
/// a fresh one at its own size.
pub fn set_compact(cx: &mut Cx, form: Option<&str>) -> bool {
    let state = cx.global::<HwLayoutState>();
    match form {
        Some(name) => {
            if state.compact.is_none() {
                state.before_compact = Some(state.active.clone());
            }
            state.compact = Some(name.to_string());
            if state.active == name {
                return false;
            }
            state.active = name.to_string();
        }
        None => {
            if state.compact.take().is_none() {
                return false;
            }
            let back = state.before_compact.take().unwrap_or_else(|| state.chosen.clone());
            if state.active == back {
                return false;
            }
            state.active = back;
        }
    }
    reload(cx);
    true
}

/// Show the layout that fits `size` for the pick; `true` when it changed
/// (a layout reload is asked for).
pub fn refit(cx: &mut Cx, size: Vec2d) -> bool {
    let state = cx.global::<HwLayoutState>();
    // `--layout`: that layout, whatever the size (a layout author's run);
    // a granted compact form: its layout, until the host unparks the window.
    if state.pinned || state.compact.is_some() {
        return false;
    }
    let next = fit_layout(&state.chosen.clone(), &state.active.clone(), size);
    if next == state.active {
        return false;
    }
    state.active = next;
    let now = cx.seconds_since_app_start();
    cx.global::<HwLayoutState>().switch_t0 = Some(now);
    reload(cx);
    switch_mark(cx, "layout chosen");
    true
}

/// `MUSIC_SWITCH_TRACE`: log a phase of the layout switch under way, in ms
/// since it began.
pub fn switch_mark(cx: &mut Cx, phase: &str) {
    if std::env::var_os("MUSIC_SWITCH_TRACE").is_none() || !cx.has_global::<HwLayoutState>() {
        return;
    }
    let now = cx.seconds_since_app_start();
    if let Some(t0) = cx.global::<HwLayoutState>().switch_t0 {
        log!("switch +{:.1} ms: {phase}", (now - t0) * 1000.0);
    }
}

pub fn switch_done(cx: &mut Cx) {
    if cx.has_global::<HwLayoutState>() {
        cx.global::<HwLayoutState>().switch_t0 = None;
    }
}

/// The layout file on screen changed on disk: reload it.
pub fn check_file(cx: &mut Cx) {
    let state = cx.global::<HwLayoutState>();
    let now = stamp(&state.active);
    if now.is_some() && now != state.stamp {
        state.stamp = now;
        // The author is changing the layout: its splitters take the file's
        // positions again (the kept ones would hide a new default).
        let name = state.active.clone();
        let kept: Vec<_> = saved_splits().into_iter().filter(|(l, _, _, _)| *l != name).collect();
        save_splits(&kept);
        cx.request_style_reload();
    }
}

pub fn pinned(cx: &mut Cx) -> bool {
    cx.has_global::<HwLayoutState>() && cx.global::<HwLayoutState>().pinned
}

/// The splitters a layout names (`id := mod.widgets.Hw…Splitter{`).
pub fn splitter_ids(name: &str) -> Vec<LiveId> {
    splitter_names(&layout_source(name).unwrap_or_default()).iter().map(|id| LiveId::from_str(id)).collect()
}

/// Each splitter's own `align:` in a layout's source (the first `align:`
/// after its opening brace: a splitter's properties come before its panes),
/// as (kind a/b/w, value).
pub fn splitter_defaults(name: &str) -> Vec<(String, String, f64)> {
    let source = layout_source(name).unwrap_or_default();
    let code: String = source.lines().map(|line| line.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let mut out = Vec::new();
    for id in splitter_names(&code) {
        // Its own `id :=`, not one that ends in it (`split` in `side_split`).
        let whole = |at: &usize| *at == 0 || !code[..*at].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        let Some(at) = code.match_indices(&format!("{id} :=")).map(|(at, _)| at).find(whole)
            .or_else(|| code.match_indices(&format!("{id}:=")).map(|(at, _)| at).find(whole)) else { continue };
        let Some(open) = code[at..].find('{') else { continue };
        let rest = &code[at + open..];
        let Some(align) = rest.find("align:") else { continue };
        let text = rest[align + 6..].trim_start();
        let kind = if text.starts_with("SplitterAlign.FromA") {
            "a"
        } else if text.starts_with("SplitterAlign.FromB") {
            "b"
        } else if text.starts_with("SplitterAlign.Weighted") {
            "w"
        } else {
            continue;
        };
        let Some(l) = text.find('(') else { continue };
        let Some(r) = text.find(')') else { continue };
        if let Ok(value) = text[l + 1..r].trim().parse::<f64>() {
            out.push((id, kind.to_string(), value));
        }
    }
    out
}

/// Every `name := …Splitter{` in a layout's source, wherever it sits (a
/// line of its own, after `a:` or `b:`, several on one line), comments
/// skipped.
fn splitter_names(source: &str) -> Vec<String> {
    let code: String = source.lines().map(|line| line.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let mut names = Vec::new();
    let mut rest = code.as_str();
    while let Some(at) = rest.find(":=") {
        let before = rest[..at].trim_end();
        let start = before.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map(|i| i + 1).unwrap_or(0);
        let name = &before[start..];
        let after = rest[at + 2..].trim_start();
        let head = after.split('{').next().unwrap_or("");
        if !name.is_empty() && head.trim_end().ends_with("Splitter") && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        rest = &rest[at + 2..];
    }
    names
}

fn splits_path() -> Option<PathBuf> {
    Some(crate::worker::data_dir()?.join("layout_splits.txt"))
}

/// Splitter positions kept per layout: layout, splitter id, kind (a/b/w),
/// value.
pub fn saved_splits() -> Vec<(String, String, String, f64)> {
    let Some(text) = splits_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.to_string(), parts.next()?.to_string(), parts.next()?.to_string(), parts.next()?.parse().ok()?))
        })
        .collect()
}

pub fn save_splits(entries: &[(String, String, String, f64)]) {
    let text: String = entries.iter().map(|(l, s, k, v)| format!("{l} {s} {k} {v}\n")).collect();
    if let Some(path) = splits_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(path, text);
    }
}

/// Show `name` without making it the person's pick (the showcase, or
/// going back to the pick).
pub fn show(cx: &mut Cx, name: &str) {
    let state = cx.global::<HwLayoutState>();
    if state.active == name {
        return;
    }
    state.active = name.to_string();
    reload(cx);
}

pub fn showcase(cx: &mut Cx) -> (bool, bool) {
    if !cx.has_global::<HwLayoutState>() {
        return (false, false);
    }
    let state = cx.global::<HwLayoutState>();
    (state.showcase, state.showcase_styles)
}

pub fn set_showcase(cx: &mut Cx, on: bool, styles: bool) {
    let state = cx.global::<HwLayoutState>();
    state.showcase = on;
    state.showcase_styles = styles;
}

#[cfg(test)]
mod tests {
    use super::splitter_names;

    #[test]
    fn splitters_are_found_however_the_file_is_formatted() {
        let source = "a: top := mod.widgets.HwSplitter{\n  b: View{} } // old := mod.widgets.HwSplitter{\nx := mod.widgets.HwDeckSplitter {a: y := mod.widgets.HwSplitter{}}\nz := View{}";
        assert_eq!(splitter_names(source), vec!["top", "x", "y"]);
    }
}

#[cfg(test)]
mod default_tests {
    #[test]
    fn a_splitter_name_inside_another_is_not_it() {
        let code = "side_split := mod.widgets.HwDeckSplitter{ align: SplitterAlign.FromA(236.0) a: X }\nsplit := mod.widgets.HwSplitter{ align: SplitterAlign.FromB(260.0) }";
        let whole = |at: &usize| *at == 0 || !code[..*at].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        let at = code.match_indices("split :=").map(|(at, _)| at).find(whole).unwrap();
        assert!(code[at..].starts_with("split := mod.widgets.HwSplitter"));
    }
}
