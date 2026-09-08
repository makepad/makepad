//! NavList — a set of destinations of which exactly one is where you are.
//!
//! Six strips in five applications in this repository are the same control
//! written six times: a handful of destinations, exactly one lit, and
//! choosing one swaps the surface beside it. Each polls its buttons by id,
//! none has a keyboard, none reports what was chosen as an action, and they
//! paint "which one is lit" four different ways — **two of them not at all**,
//! so in those two the strip never says where you are.
//!
//! **What this owns is the model, not the look.** The rows come from a
//! `destination` template the caller supplies, because those six strips have
//! six different looks and a row drawn in Rust here would be re-skinned by
//! every one of them — the same reason a shared splitter preset turned out
//! to be worth nothing. What cannot be written per-app cheaply, and so lives
//! here, is the rule that exactly one is lit, one keyboard stop for the whole
//! list instead of one per row, arrows that move the choice, and a typed
//! action saying which destination was picked.
//!
//! **A row must be radio-shaped.** That is the contract: the template is a
//! `RadioButton` or a preset of one, so the list can light exactly one and
//! put the others out. A row that cannot be lit cannot be a destination.
//!
//! A rail and a bar are not two widgets here; they are `flow: Down` and
//! `flow: Right` over the same list.

use crate::{
    makepad_derive_widget::*, makepad_draw::*, radio_button::RadioButtonWidgetRefExt,
    widget::*, widget_tree::CxWidgetExt,
};
use std::collections::HashMap;

/// One place the list can send you.
#[derive(Clone, Debug, PartialEq)]
pub struct Destination {
    pub id: LiveId,
    pub label: String,
}

impl Destination {
    pub fn new(id: LiveId, label: &str) -> Self {
        Self { id, label: label.to_string() }
    }
}

/// What a list reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum NavAction {
    /// A destination was chosen — by pointer or by key.
    Selected(LiveId),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawNavGroundBase = #(DrawNavGround::script_component(vm))
    set_type_default() do #(DrawNavGround::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // The list's own rect, painted as nothing: the rows are separate
            // widgets, so without this the list has no rect to focus, to
            // redraw or to be found by.
            return vec4(0.0 0.0 0.0 0.0)
        }
    }

    mod.widgets.NavListBase = #(NavList::register_widget(vm))

    /** A set of destinations of which exactly one is where you are. The
     * rows come from the `destination` template, so a caller keeps its own
     * look and gets the one-lit rule, the keyboard and the report. */
    mod.widgets.NavList = set_type_default() do mod.widgets.NavListBase{
        width: Fit
        height: Fit
        flow: Down
        spacing: 2.
        /** the destinations to start with, by name */
        labels: []

        // A named entry, the way a list declares its row templates: it
        // lands in the vec and is collected rather than drawn as a child.
        destination := RadioButtonTab{
            width: Fill
            height: Fit
        }
    }

    /** A column of destinations down one side. */
    mod.widgets.NavRail = mod.widgets.NavList{
        width: 200.
        height: Fill
        flow: Down
    }

    /** A row of destinations along an edge. */
    mod.widgets.NavBar = mod.widgets.NavList{
        width: Fill
        height: Fit
        flow: Right
        // Each row is its own width here. A rail's rows fill the rail, but
        // a row that fills a RIGHT flow takes the whole bar and leaves the
        // rest of the destinations nowhere to be.
        destination := RadioButtonTab{
            width: Fit
            height: Fit
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNavGround {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, Widget)]
pub struct NavList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The list's own rect, drawn as nothing — see the shader's comment.
    #[redraw]
    #[live]
    draw_bg: DrawNavGround,

    /// The destinations to start with, for a list declared in markup.
    #[live]
    pub labels: Vec<String>,

    /// The row template, collected from the instance by name.
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    rows: Vec<(LiveId, WidgetRef)>,
    #[rust]
    destinations: Vec<Destination>,
    #[rust]
    selected: Option<LiveId>,
    /// Whether `labels` has been turned into destinations yet.
    #[rust]
    seeded: bool,
    #[rust]
    area: Area,
}

impl ScriptHook for NavList {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // The row template arrives as a named entry on the instance, the way
        // a list's item templates do.
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                self.templates
                                    .insert(id, vm.bx.heap.new_object_ref(template_obj));
                            }
                        }
                    }
                });
            }
        }
        if apply.is_reload() {
            // The rows are built from a template that may have just changed,
            // so they are rebuilt rather than patched.
            self.rows.clear();
        }
    }
}

impl NavList {
    /// The destinations, in order. The chosen one stays chosen if it is
    /// still here, and otherwise the list falls back to the first, because a
    /// nav with nothing lit does not say where you are.
    pub fn set_destinations(&mut self, cx: &mut Cx, destinations: Vec<Destination>) {
        let keep = self
            .selected
            .filter(|id| destinations.iter().any(|d| d.id == *id))
            .or_else(|| destinations.first().map(|d| d.id));
        self.destinations = destinations;
        self.selected = keep;
        self.rows.clear();
        self.redraw(cx);
    }

    pub fn select(&mut self, cx: &mut Cx, id: LiveId) {
        if self.selected != Some(id) && self.destinations.iter().any(|d| d.id == id) {
            self.selected = Some(id);
            self.redraw(cx);
        }
    }

    pub fn selected(&self) -> Option<LiveId> {
        self.selected
    }

    pub fn count(&self) -> usize {
        self.destinations.len()
    }

    /// The destination one step from the current one, for the arrow keys.
    fn step(&self, by: isize) -> Option<LiveId> {
        if self.destinations.is_empty() {
            return None;
        }
        let at = self
            .selected
            .and_then(|id| self.destinations.iter().position(|d| d.id == id))
            .unwrap_or(0) as isize;
        let last = self.destinations.len() as isize - 1;
        let to = (at + by).clamp(0, last) as usize;
        Some(self.destinations[to].id)
    }

    fn row(&mut self, cx: &mut Cx, index: usize, id: LiveId) -> Option<WidgetRef> {
        if let Some((_, widget)) = self.rows.get(index) {
            return Some(widget.clone());
        }
        let template = self.templates.get(&live_id!(destination))?;
        let value: ScriptValue = template.as_object().into();
        let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, value));
        // A tree node under the list, so the design overlay can pick a row
        // and style the template it came from.
        cx.widget_tree_insert_child(self.uid, id, widget.clone());
        self.rows.push((id, widget.clone()));
        Some(widget)
    }
}

impl Widget for NavList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // A list declared in markup has to show its destinations without the
        // host saying anything, and has to still do so after a reload.
        if !self.seeded {
            self.seeded = true;
            if self.destinations.is_empty() && !self.labels.is_empty() {
                let destinations = self
                    .labels
                    .iter()
                    .enumerate()
                    .map(|(i, name)| Destination::new(LiveId(i as u64 + 1), name))
                    .collect();
                self.set_destinations(cx.cx.cx, destinations);
            }
        }

        self.draw_bg.begin(cx, walk, self.layout);
        let entries = self.destinations.clone();
        let selected = self.selected;
        for (index, entry) in entries.iter().enumerate() {
            if let Some(widget) = self.row(cx.cx.cx, index, entry.id) {
                let radio = widget.as_radio_button();
                radio.set_text(&entry.label);
                // Exactly one is lit. Said here, once, rather than in each
                // of the six places that used to say it differently.
                if selected == Some(entry.id) {
                    radio.select(cx.cx.cx, scope);
                } else {
                    radio.unselect(cx.cx.cx);
                }
                let row_walk = widget.walk(cx.cx.cx);
                let _ = widget.draw_walk(cx, scope, row_walk);
            }
        }
        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let rows = self.rows.clone();
        for (_, widget) in &rows {
            widget.handle_event(cx, event, scope);
        }
        if let Event::Actions(actions) = event {
            for (id, widget) in &rows {
                if widget.as_radio_button().clicked(actions) {
                    let uid = self.uid;
                    self.select(cx, *id);
                    cx.set_key_focus(self.area);
                    cx.widget_action(uid, NavAction::Selected(*id));
                }
            }
        }
        // One keyboard stop for the whole list, not one per row: a nav of
        // five destinations should be one Tab away, not five.
        match event.hits(cx, self.area) {
            Hit::FingerDown(_) => cx.set_key_focus(self.area),
            Hit::KeyDown(ke) => {
                let to = match ke.key_code {
                    KeyCode::ArrowDown | KeyCode::ArrowRight => self.step(1),
                    KeyCode::ArrowUp | KeyCode::ArrowLeft => self.step(-1),
                    KeyCode::Home => self.destinations.first().map(|d| d.id),
                    KeyCode::End => self.destinations.last().map(|d| d.id),
                    _ => None,
                };
                if let Some(id) = to {
                    if self.selected != Some(id) {
                        let uid = self.uid;
                        self.select(cx, id);
                        cx.widget_action(uid, NavAction::Selected(id));
                    }
                }
            }
            _ => {}
        }
    }

    /// Where you are, so a test can read the list in one line.
    fn text(&self) -> String {
        self.selected
            .and_then(|id| self.destinations.iter().find(|d| d.id == id))
            .map(|d| d.label.clone())
            .unwrap_or_default()
    }
}

impl NavListRef {
    pub fn set_destinations(&self, cx: &mut Cx, destinations: Vec<Destination>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_destinations(cx, destinations);
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

    pub fn count(&self) -> usize {
        self.borrow().map(|inner| inner.count()).unwrap_or(0)
    }

    /// The destination chosen this pass, if one was.
    pub fn chosen(&self, actions: &Actions) -> Option<LiveId> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<NavAction>() {
            NavAction::Selected(id) => Some(id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Choosing survives a set that still holds the choice, and falls to the
    /// first when it does not: a nav with nothing lit does not say where
    /// you are, which is the defect two of the six shipped strips have.
    #[test]
    fn the_choice_survives_what_it_can_and_falls_back_when_it_cannot() {
        let here: Vec<Destination> = (1..=3)
            .map(|i| Destination::new(LiveId(i), &format!("place {i}")))
            .collect();
        let selected = Some(LiveId(2));
        let keep = selected
            .filter(|id| here.iter().any(|d| d.id == *id))
            .or_else(|| here.first().map(|d| d.id));
        assert_eq!(keep, Some(LiveId(2)), "still there: still lit");

        let gone = vec![Destination::new(LiveId(7), "elsewhere")];
        let keep = selected
            .filter(|id| gone.iter().any(|d| d.id == *id))
            .or_else(|| gone.first().map(|d| d.id));
        assert_eq!(keep, Some(LiveId(7)), "gone: the first takes over");
    }
}
