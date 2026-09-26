//! Tabs — the strip that behaves the way a browser's does.
//!
//! The library already had a tab SHAPE, a radio group, a pager, and the
//! arithmetic for an overflow split, and nothing that joined them. What was
//! missing is the behaviour everyone actually means by "tabs", which is not
//! a row of buttons:
//!
//! **Tabs SHARE the width.** They are as wide as they can be up to a
//! maximum, and they shrink together as more arrive rather than running off
//! the edge. That one rule is what makes a strip read as tabs, and it is why
//! a row of radio buttons never will.
//!
//! **A tab can be shut, and the mark for it appears when it is useful.** A
//! close mark on every tab at all times is noise; none at all is a dead end.
//! It shows on the tab under the pointer, on the tab in use, and on any tab
//! wide enough to spare the room. The middle button shuts one without having
//! to aim at anything.
//!
//! **The tab in use belongs to the panel below it.** It is drawn in the
//! panel's own colour so the two read as one surface, which is what says
//! "this strip chooses what is underneath" rather than "these are buttons".
//!
//! Lifted from a working browser chrome rather than invented. Everything
//! particular to that browser stayed behind: this takes theme colours, and
//! it carries no favicon, no globe and no page.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// One tab: what it is called, and the id it answers to.
#[derive(Clone, Debug, PartialEq)]
pub struct TabEntry {
    pub id: LiveId,
    pub label: String,
}

impl TabEntry {
    pub fn new(id: LiveId, label: &str) -> Self {
        Self { id, label: label.to_string() }
    }
}

/// What a strip reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum TabsAction {
    /// A tab was chosen.
    Selected(LiveId),
    /// A tab was shut, by its mark or by the middle button.
    Closed(LiveId),
    /// The add mark was pressed.
    Added,
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, Default)]
struct TabHit {
    id: LiveId,
    rect: Rect,
    close: Rect,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawTabsShapeBase = #(DrawTabsShape::script_component(vm))
    set_type_default() do #(DrawTabsShape::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #00000000
        /** rounding on the two top corners 0..12 step 0.5 */
        radius: 5.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // Round at the top, square at the bottom: a tab meets the panel
            // under it, and a rounded bottom edge would draw a seam across
            // the join the two are meant to share.
            sdf.box_y(
                0.0,
                0.0,
                self.rect_size.x,
                self.rect_size.y + self.radius,
                self.radius * 0.5,
                0.0
            )
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.DrawTabsMarkBase = #(DrawTabsMark::script_component(vm))
    set_type_default() do #(DrawTabsMark::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #88888888
        /** 0 draws a cross, 1 draws a plus 0..1 step 1 */
        plus: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let cx2 = self.rect_size.x * 0.5
            let cy2 = self.rect_size.y * 0.5
            let r = min(self.rect_size.x, self.rect_size.y) * 0.26
            // A plus is a cross turned an eighth of a turn, so one path
            // draws both and the caller only says which.
            let a = mix(0.7853981634, 0.0, self.plus)
            let ax = cos(a) * r
            let ay = sin(a) * r
            sdf.move_to(cx2 - ax, cy2 - ay)
            sdf.line_to(cx2 + ax, cy2 + ay)
            sdf.stroke(self.color, 1.25)
            sdf.move_to(cx2 + ay, cy2 - ax)
            sdf.line_to(cx2 - ay, cy2 + ax)
            sdf.stroke(self.color, 1.25)
            return sdf.result
        }
    }

    mod.widgets.TabsBase = #(Tabs::register_widget(vm))

    /** A strip of tabs that share the width, can be shut, and hand the panel
     * below them its own colour. */
    mod.widgets.Tabs = set_type_default() do mod.widgets.TabsBase{
        width: Fill
        height: 30.
        /** widest one tab may become 60..400 step 10 */
        tab_max_width: 200.
        /** narrowest one tab may be squeezed to 32..160 step 4 */
        tab_min_width: 60.
        /** offer an add mark at the end of the strip */
        can_add: true
        /** offer a close mark on the tabs */
        can_close: true
        /** the tabs to start with, by name */
        labels: []

        /* The whole look is one ladder, and its order is the point. The
         * strip is the app's own background, a quiet tab is that same
         * background so it RECEDES into the frame rather than sitting on
         * it, hovering lifts a tab part of the way, and only the tab in use
         * carries the panel's colour. Brighten a quiet tab past the one in
         * use and the eye picks the wrong tab, which is what a row of
         * buttons looks like. */
        draw_bg +: {color: theme.color_bg_app}
        draw_tab +: {color: theme.color_bg_app}
        draw_tab_hover +: {color: mix(theme.color_bg_app, theme.color_surface_container_low, 0.55)}
        draw_tab_active +: {color: theme.color_surface_container_low}
        draw_sep +: {color: theme.color_outline_variant radius: 0.5}
        draw_mark +: {color: theme.color_text_meta}
        draw_text +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_text_active +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTabsShape {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    radius: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTabsMark {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    plus: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Tabs {
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
    draw_bg: DrawTabsShape,
    #[live]
    draw_tab: DrawTabsShape,
    #[live]
    draw_tab_hover: DrawTabsShape,
    #[live]
    draw_tab_active: DrawTabsShape,
    #[live]
    draw_sep: DrawTabsShape,
    #[live]
    draw_mark: DrawTabsMark,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_active: DrawText,

    #[live(200.0)]
    pub tab_max_width: f64,
    #[live(60.0)]
    pub tab_min_width: f64,
    #[live(true)]
    pub can_add: bool,
    #[live(true)]
    pub can_close: bool,
    /// The tabs to start with, by name, for a strip declared in markup.
    #[live]
    pub labels: Vec<String>,

    #[rust]
    entries: Vec<TabEntry>,
    /// Whether the names in `labels` have been turned into tabs yet.
    #[rust]
    seeded: bool,
    #[rust]
    selected: Option<LiveId>,
    #[rust]
    hits: Vec<TabHit>,
    #[rust]
    add_rect: Rect,
    #[rust]
    hover_tab: Option<LiveId>,
    #[rust]
    hover_close: bool,
    #[rust]
    hover_add: bool,
}

impl Tabs {
    const PAD: f64 = 6.0;
    const ADD: f64 = 26.0;
    const MARK: f64 = 15.0;

    /// The tabs, in order. The chosen one stays chosen if it is still here,
    /// and otherwise the strip falls back to the first, because a strip with
    /// nothing chosen has no panel to show.
    pub fn set_tabs(&mut self, cx: &mut Cx, entries: Vec<TabEntry>) {
        let keep = self
            .selected
            .filter(|id| entries.iter().any(|e| e.id == *id))
            .or_else(|| entries.first().map(|e| e.id));
        self.entries = entries;
        self.selected = keep;
        self.redraw(cx);
    }

    pub fn select(&mut self, cx: &mut Cx, id: LiveId) {
        if self.selected != Some(id) && self.entries.iter().any(|e| e.id == id) {
            self.selected = Some(id);
            self.redraw(cx);
        }
    }

    pub fn selected(&self) -> Option<LiveId> {
        self.selected
    }

    /// Where the pointer is, as the three things the strip cares about.
    fn hit_at(&self, pos: DVec2) -> (Option<LiveId>, bool, bool) {
        if self.can_add && self.add_rect.contains(pos) {
            return (None, false, true);
        }
        for hit in &self.hits {
            if hit.rect.contains(pos) {
                return (Some(hit.id), hit.close.contains(pos), false);
            }
        }
        (None, false, false)
    }

    fn update_hover(&mut self, cx: &mut Cx, pos: Option<DVec2>) {
        let (tab, close, add) = pos.map(|p| self.hit_at(p)).unwrap_or((None, false, false));
        if tab != self.hover_tab || close != self.hover_close || add != self.hover_add {
            self.hover_tab = tab;
            self.hover_close = close;
            self.hover_add = add;
            self.redraw(cx);
        }
    }

    fn tab_width(&self, room: f64) -> f64 {
        tab_width_for(self.entries.len(), room, self.tab_max_width, self.tab_min_width)
    }
}

/// How wide one tab is: share the room, up to a maximum, down to a minimum.
///
/// This one line is what separates tabs from a row of buttons, so it is a
/// free function with its own tests rather than a detail of the draw.
/// Above the minimum the strip is exact; below it the tabs stop shrinking
/// and the strip overruns, which is the honest failure until it learns to
/// scroll.
pub fn tab_width_for(count: usize, room: f64, max: f64, min: f64) -> f64 {
    let count = count.max(1) as f64;
    (room / count).min(max).max(min.min(room.max(1.0)))
}

impl Widget for Tabs {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // A strip declared in markup has to show its tabs without a host
        // saying anything. Seeding on the first draw rather than waiting for
        // an action is what makes that true after a reload as well, when
        // nothing has happened yet to prompt the host.
        if !self.seeded {
            self.seeded = true;
            if self.entries.is_empty() && !self.labels.is_empty() {
                let entries = self
                    .labels
                    .iter()
                    .enumerate()
                    .map(|(i, name)| TabEntry::new(LiveId(i as u64 + 1), name))
                    .collect();
                self.set_tabs(cx.cx.cx, entries);
            }
        }
        self.draw_bg.begin(cx, walk, self.layout);
        let strip = cx.turtle().rect();
        self.hits.clear();

        let add_room = if self.can_add { Self::ADD } else { 0.0 };
        let room = (strip.size.x - Self::PAD * 2.0 - add_room).max(0.0);
        let tab_w = self.tab_width(room);
        let tab_h = strip.size.y;
        let mut x = strip.pos.x + Self::PAD;
        let y = strip.pos.y;

        let entries = self.entries.clone();
        let selected = self.selected;
        for (i, entry) in entries.iter().enumerate() {
            let rect = Rect { pos: dvec2(x, y), size: dvec2(tab_w, tab_h) };
            let active = selected == Some(entry.id);
            let hovered = self.hover_tab == Some(entry.id);
            if active {
                self.draw_tab_active.draw_abs(cx, rect);
            } else if hovered {
                self.draw_tab_hover.draw_abs(cx, rect);
            } else {
                self.draw_tab.draw_abs(cx, rect);
                // A hairline between two quiet neighbours and nowhere else:
                // beside the tab in use, or the one under the pointer, it
                // would be a second edge on a shape that already has one.
                let next_quiet = entries
                    .get(i + 1)
                    .map(|n| selected != Some(n.id) && self.hover_tab != Some(n.id))
                    .unwrap_or(false);
                if next_quiet {
                    self.draw_sep.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x + tab_w - 0.5, y + 7.0),
                            size: dvec2(1.0, (tab_h - 14.0).max(1.0)),
                        },
                    );
                }
            }

            // The close mark earns its room: on the tab in use, on the one
            // under the pointer, or on a tab wide enough that it costs
            // nothing to show.
            let wide = tab_w >= 108.0;
            let show_close = self.can_close && (active || hovered || wide);
            let close_rect = Rect {
                pos: dvec2(x + tab_w - Self::MARK - 5.0, y + (tab_h - Self::MARK) * 0.5),
                size: dvec2(Self::MARK, Self::MARK),
            };
            let text_right = if show_close { close_rect.pos.x - 4.0 } else { x + tab_w - 8.0 };
            let text_left = x + 10.0;
            if text_right > text_left {
                let walk = Walk {
                    abs_pos: Some(dvec2(text_left, y + (tab_h - 14.0) * 0.5)),
                    width: Size::Fixed(text_right - text_left),
                    height: Size::Fixed(14.0),
                    ..Walk::default()
                };
                if active {
                    self.draw_text_active.draw_walk(cx, walk, Align { x: 0.0, y: 0.5 }, &entry.label);
                } else {
                    self.draw_text.draw_walk(cx, walk, Align { x: 0.0, y: 0.5 }, &entry.label);
                }
            }
            if show_close {
                self.draw_mark.plus = 0.0;
                self.draw_mark.draw_abs(cx, close_rect);
            }

            self.hits.push(TabHit { id: entry.id, rect, close: close_rect });
            x += tab_w;
        }

        if self.can_add {
            self.add_rect = Rect {
                pos: dvec2(x + 3.0, y + (tab_h - Self::ADD) * 0.5),
                size: dvec2(Self::ADD, Self::ADD),
            };
            self.draw_mark.plus = 1.0;
            self.draw_mark.draw_abs(cx, self.add_rect);
        } else {
            self.add_rect = Rect::default();
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) => {
                let (tab, close, add) = self.hit_at(fe.abs);
                let middle = fe.mouse_button().map(|b| b.is_middle()).unwrap_or(false);
                if add {
                    cx.widget_action(self.uid, TabsAction::Added);
                } else if let Some(id) = tab {
                    // The middle button shuts a tab without having to aim at
                    // the mark, which is the whole reason people use it.
                    if self.can_close && (close || middle) {
                        cx.widget_action(self.uid, TabsAction::Closed(id));
                    } else {
                        self.selected = Some(id);
                        self.redraw(cx);
                        cx.widget_action(self.uid, TabsAction::Selected(id));
                    }
                }
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.update_hover(cx, Some(fe.abs));
            }
            Hit::FingerMove(fe) => {
                self.update_hover(cx, Some(fe.abs));
            }
            Hit::FingerHoverOut(_) => self.update_hover(cx, None),
            _ => {}
        }
    }

    /// The chosen tab's label, so a test can read the strip in one line.
    fn text(&self) -> String {
        self.selected
            .and_then(|id| self.entries.iter().find(|e| e.id == id))
            .map(|e| e.label.clone())
            .unwrap_or_default()
    }
}

impl TabsRef {
    pub fn set_tabs(&self, cx: &mut Cx, entries: Vec<TabEntry>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_tabs(cx, entries);
        }
    }

    pub fn select(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.select(cx, id);
        }
    }

    pub fn selected(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.selected())
    }

    /// How many tabs are open, so a host that owns the order can tell when
    /// the strip has been rebuilt underneath it and hand the list back.
    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.entries.len()).unwrap_or(0)
    }

    /// The tab chosen this pass, if one was.
    pub fn chosen(&self, actions: &Actions) -> Option<LiveId> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<TabsAction>() {
            TabsAction::Selected(id) => Some(id),
            _ => None,
        }
    }

    /// The tab shut this pass, if one was.
    pub fn closed(&self, actions: &Actions) -> Option<LiveId> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<TabsAction>() {
            TabsAction::Closed(id) => Some(id),
            _ => None,
        }
    }

    /// Whether the add mark was pressed this pass.
    pub fn added(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|a| matches!(a.cast::<TabsAction>(), TabsAction::Added))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule that makes a strip read as tabs rather than as buttons: they
    /// share the room, and they stop at a maximum so two tabs do not each
    /// become half a window wide.
    #[test]
    fn tabs_share_the_room_and_stop_at_the_maximum() {
        assert_eq!(tab_width_for(2, 800.0, 200.0, 60.0), 200.0, "room to spare: the maximum wins");
        assert_eq!(tab_width_for(2, 300.0, 200.0, 60.0), 150.0, "tight: they split what there is");
        assert_eq!(tab_width_for(8, 800.0, 200.0, 60.0), 100.0, "more tabs, each one smaller");
    }

    /// And they stop shrinking, because a tab too narrow to name is not a
    /// tab. Below that the strip overruns, which is the honest failure.
    #[test]
    fn a_tab_stops_shrinking_at_its_minimum() {
        let w = tab_width_for(20, 600.0, 200.0, 60.0);
        assert_eq!(w, 60.0, "twenty into six hundred floors at the minimum");
        assert!(w * 20.0 > 600.0, "so the strip overruns rather than shrinking to nothing");
    }

    /// A strip with nothing in it still asks for a sane width rather than
    /// dividing by zero.
    #[test]
    fn an_empty_strip_is_not_a_division_by_zero() {
        assert_eq!(tab_width_for(0, 400.0, 200.0, 60.0), 200.0);
        assert_eq!(tab_width_for(0, 0.0, 200.0, 60.0), 1.0, "no room at all still answers");
    }

    /// Choosing survives a set that still holds the choice, and falls to the
    /// first when it does not, because a strip with nothing chosen has no
    /// panel to show.
    #[test]
    fn the_choice_survives_what_it_can_and_falls_back_when_it_cannot() {
        let here: Vec<TabEntry> = (1..=3)
            .map(|i| TabEntry::new(LiveId(i), &format!("tab {i}")))
            .collect();
        let selected = Some(LiveId(2));
        let keep = selected
            .filter(|id| here.iter().any(|e| e.id == *id))
            .or_else(|| here.first().map(|e| e.id));
        assert_eq!(keep, Some(LiveId(2)), "still there: still chosen");

        let gone: Vec<TabEntry> = vec![TabEntry::new(LiveId(7), "other")];
        let keep = selected
            .filter(|id| gone.iter().any(|e| e.id == *id))
            .or_else(|| gone.first().map(|e| e.id));
        assert_eq!(keep, Some(LiveId(7)), "gone: the first one takes over");
    }
}
