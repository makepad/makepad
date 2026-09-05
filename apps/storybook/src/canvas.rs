//! The canvas: one story at a time, instantiated from its template when it
//! is asked for and dropped when another is.
//!
//! A story template lives under `mod.stories`. The canvas looks it up by name
//! in the script heap, builds a widget from it the way a page flip builds a
//! page, and inserts it into the widget tree under itself so the tree, the
//! remote `/snap` route and every `ids!` lookup see it. A theme reload
//! rebuilds every widget from its template, so the canvas drops its story on
//! `LiveEdit` and instantiates it again on the next draw.
use crate::makepad_widgets::makepad_script::trap::NoTrap;
use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.StoryCanvasBase = #(StoryCanvas::register_widget(vm))
    mod.widgets.StoryCanvas = set_type_default() do mod.widgets.StoryCanvasBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct StoryCanvas {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[rust]
    area: Area,
    /// The template the canvas should be showing.
    #[rust]
    wanted: Option<String>,
    /// The template it is showing, and the widget built from it.
    #[rust]
    shown: Option<(String, WidgetRef)>,
    /// Templates that were asked for and did not exist, so the log line is
    /// written once per name and not once per frame.
    #[rust]
    missing: Vec<String>,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
}

impl WidgetNode for StoryCanvas {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if let Some((dsl, page)) = &self.shown {
            visit(LiveId::from_str(dsl), page.clone());
        }
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }
}

impl StoryCanvas {
    /// Show the template with this name from the next draw on.
    pub fn open(&mut self, cx: &mut Cx, dsl: &str) {
        if self.wanted.as_deref() != Some(dsl) {
            self.wanted = Some(dsl.to_string());
            cx.widget_tree_mark_dirty(self.uid);
            self.redraw(cx);
        }
    }

    /// Drop the shown story so the next draw builds it afresh.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.shown = None;
        cx.widget_tree_mark_dirty(self.uid);
        self.redraw(cx);
    }

    /// The root of the story on screen, for the app's per-story handlers.
    pub fn shown_root(&self) -> Option<WidgetRef> {
        self.shown.as_ref().map(|(_, page)| page.clone())
    }

    pub fn shown_dsl(&self) -> Option<&str> {
        self.shown.as_ref().map(|(dsl, _)| dsl.as_str())
    }

    fn instantiate(&mut self, cx: &mut Cx, dsl: &str) -> Option<WidgetRef> {
        let id = LiveId::from_str(dsl);
        let value = cx.with_vm(|vm| {
            let stories = vm.module(id!(stories));
            vm.bx.heap.value(stories, id.into(), NoTrap)
        });
        if value.is_nil() || value.is_err() || value.as_object().is_none() {
            if !self.missing.iter().any(|m| m == dsl) {
                self.missing.push(dsl.to_string());
                log!("storybook: missing template {}", dsl);
            }
            return None;
        }
        let page = cx.with_vm(|vm| WidgetRef::script_from_value(vm, value));
        if page.is_empty() {
            log!("storybook: template {} did not build a widget", dsl);
            return None;
        }
        cx.widget_tree_insert_child_deep(self.uid, id, page.clone());
        cx.widget_tree_mark_dirty(self.uid);
        Some(page)
    }
}

impl Widget for StoryCanvas {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            // The whole tree is being rebuilt from its templates; the story
            // must be too, or it keeps the pre-reload objects.
            self.reset(cx);
        }
        if let Some((_, page)) = self.shown.clone() {
            let uid = self.uid;
            let page_uid = page.widget_uid();
            cx.group_widget_actions(uid, page_uid, |cx| page.handle_event(cx, event, scope));
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let stale = match (&self.wanted, &self.shown) {
            (Some(w), Some((s, _))) => w != s,
            (Some(_), None) => true,
            (None, Some(_)) => true,
            (None, None) => false,
        };
        if stale {
            self.shown = None;
            cx.widget_tree_mark_dirty(self.uid);
            if let Some(dsl) = self.wanted.clone() {
                if let Some(page) = self.instantiate(cx, &dsl) {
                    self.shown = Some((dsl, page));
                }
            }
        }
        let Some((_, page)) = self.shown.clone() else {
            cx.begin_turtle(walk, self.layout);
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        };
        if self.draw_state.begin_with(cx, &(), |cx, _| page.walk(cx)) {
            cx.begin_turtle(walk, self.layout);
        }
        if let Some(w) = self.draw_state.get() {
            page.draw_walk(cx, scope, w)?;
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl StoryCanvasRef {
    pub fn open(&self, cx: &mut Cx, dsl: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx, dsl);
        }
    }

    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset(cx);
        }
    }

    pub fn shown_root(&self) -> Option<WidgetRef> {
        self.borrow().and_then(|inner| inner.shown_root())
    }
}
