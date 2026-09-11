//! The canvas: one story at a time, instantiated from its template when it
//! is asked for and dropped when another is.
//!
//! A story template lives under `mod.stories`. The canvas looks it up by name
//! in the script heap, builds a widget from it the way a page flip builds a
//! page, and inserts it into the widget tree under itself so the tree, the
//! remote `/snap` route and every `ids!` lookup see it. A theme reload
//! rebuilds every widget from its template, so the canvas drops its story on
//! `LiveEdit` and instantiates it again on the next draw.
//!
//! Two things ride along with the story. The controls panel's edits are
//! script chunks applied to a widget inside the story; the canvas applies
//! them at once and remembers them, so a story rebuilt after a reload comes
//! back the way the user left it. And every action a widget inside the story
//! raises is written to a ring the actions panel reads.
use crate::makepad_widgets::makepad_script::trap::NoTrap;
use crate::makepad_widgets::makepad_script::ScriptMod;
use crate::makepad_widgets::*;
use std::collections::VecDeque;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.storybook.StoryCanvasBase = #(StoryCanvas::register_widget(vm))
    mod.storybook.StoryCanvas = set_type_default() do mod.storybook.StoryCanvasBase{
        width: Fill
        height: Fill
    }
}

/// How many raised actions the log keeps.
pub const LOG_CAPACITY: usize = 200;

/// Apply a script chunk (`{ prop: value ... }`) to a widget the way the
/// design overlay does: an eval-applied module whose errors come back to the
/// caller instead of only landing in the log.
pub fn apply_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str) -> Result<(), String> {
    let chunk = chunk.trim();
    let body = if chunk.starts_with('{') {
        chunk.to_string()
    } else {
        format!("{{{chunk}}}")
    };
    let code = format!("use mod.prelude.widgets.*\n__script_source__{body};");
    // The call site is keyed by the chunk's own text: the shader cache hashes
    // each fn's script address, so two different chunks must not share one.
    let mut hash: u32 = 2166136261;
    for b in code.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    let line = (hash % 1_000_000) as usize + 2;
    let errors = cx.with_vm(|vm| {
        vm.bx.captured_errors = Some(Vec::new());
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: "storybook".to_string(),
            file: "story://control".to_string(),
            line,
            column: 1,
            code,
            values: Vec::new(),
        };
        let mut target = widget.clone();
        use crate::makepad_widgets::makepad_script::traits::ScriptApply;
        target.script_apply_eval(vm, script_mod);
        vm.take_errors()
    });
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Turn a dotted id path ("subject.inner") into the ids a widget lookup takes.
pub fn id_path(path: &str) -> Vec<LiveId> {
    path.split('.')
        .filter(|s| !s.is_empty())
        .map(LiveId::from_str)
        .collect()
}

#[derive(Clone)]
struct Chunk {
    target: String,
    prop: String,
    chunk: String,
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
    /// The controls' edits, one per (target, prop), re-applied on rebuild.
    #[rust]
    chunks: Vec<Chunk>,
    /// What the story raised, newest last.
    #[rust]
    log: VecDeque<String>,
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
    /// Show the template with this name. Switching stories forgets the
    /// previous story's edits and log.
    pub fn open(&mut self, cx: &mut Cx, dsl: &str) {
        if self.wanted.as_deref() != Some(dsl) {
            self.wanted = Some(dsl.to_string());
            self.chunks.clear();
            self.log.clear();
            self.build(cx);
        }
    }

    /// Drop the shown story and build it afresh, forgetting the edits
    /// made to it.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.chunks.clear();
        self.rebuild(cx);
    }

    /// Drop the shown story and build it afresh, keeping the edits to
    /// re-apply.
    pub fn rebuild(&mut self, cx: &mut Cx) {
        self.shown = None;
        self.build(cx);
    }

    /// Build the wanted story, NOW rather than on the next draw.
    ///
    /// It used to be built inside `draw_walk`, which is one line too
    /// late for anything that has to announce itself before the frame
    /// starts. A glass surface is the case: the window decides whether
    /// to capture the scene behind the glass before any widget draws,
    /// and its only evidence is which surfaces asked. Building the
    /// story mid-draw meant the surface asked after the decision had
    /// been taken, so its first painted frame showed either a flat
    /// fallback slab or a photograph of the story before it, and then
    /// a forced redraw of the whole catalogue snapped it into place.
    /// That is the flicker; it is not the blur being slow.
    ///
    /// Every caller is at event time, so the script machine is free.
    /// `draw_walk` keeps the same build behind its staleness check as a
    /// net, for a `wanted` that arrives by some path this does not know
    /// about.
    fn build(&mut self, cx: &mut Cx) {
        self.shown = None;
        cx.widget_tree_mark_dirty(self.uid);
        if let Some(dsl) = self.wanted.clone() {
            if let Some(page) = self.instantiate(cx, &dsl) {
                self.shown = Some((dsl, page));
            }
        }
        self.redraw(cx);
    }

    /// The root of the story on screen, for the app's per-story handlers.
    pub fn shown_root(&self) -> Option<WidgetRef> {
        self.shown.as_ref().map(|(_, page)| page.clone())
    }

    pub fn shown_dsl(&self) -> Option<&str> {
        self.shown.as_ref().map(|(dsl, _)| dsl.as_str())
    }

    /// Apply one property edit to a widget inside the story, now and after
    /// every rebuild. `target` is a dotted id path from the story root; empty
    /// means the root. Returns the script error, if the chunk had one.
    pub fn apply(&mut self, cx: &mut Cx, target: &str, prop: &str, chunk: &str) -> Result<(), String> {
        self.chunks.retain(|c| !(c.target == target && c.prop == prop));
        self.chunks.push(Chunk {
            target: target.to_string(),
            prop: prop.to_string(),
            chunk: chunk.to_string(),
        });
        let Some((_, root)) = self.shown.clone() else {
            return Ok(());
        };
        let result = Self::apply_to(cx, &root, target, chunk);
        root.redraw(cx);
        result
    }

    fn apply_to(cx: &mut Cx, root: &WidgetRef, target: &str, chunk: &str) -> Result<(), String> {
        let widget = if target.is_empty() {
            root.clone()
        } else {
            let w = root.widget(cx, &id_path(target));
            if w.is_empty() {
                return Err(format!("no widget at {target}"));
            }
            w
        };
        apply_chunk(cx, &widget, chunk)
    }

    /// Whether the story's widget at this path can be reached.
    pub fn has_target(&self, cx: &Cx, target: &str) -> bool {
        match &self.shown {
            Some((_, root)) => target.is_empty() || !root.widget(cx, &id_path(target)).is_empty(),
            None => false,
        }
    }

    /// Drain the raised-action log.
    pub fn take_log(&mut self) -> Vec<String> {
        self.log.drain(..).collect()
    }

    /// The same build, reached from inside a draw. Named apart so a
    /// reader can see which of the two paths they are on.
    fn instantiate_in_draw(&mut self, cx: &mut Cx2d, dsl: &str) -> Option<WidgetRef> {
        self.instantiate(cx, dsl)
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
        for chunk in self.chunks.clone() {
            if let Err(e) = Self::apply_to(cx, &page, &chunk.target, &chunk.chunk) {
                log!("storybook: re-applying {} on {}: {}", chunk.prop, chunk.target, e);
            }
        }
        Some(page)
    }
}

impl Widget for StoryCanvas {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            // The whole tree is being rebuilt from its templates; the story
            // must be too, or it keeps the pre-reload objects.
            self.rebuild(cx);
        }
        if let Some((_, page)) = self.shown.clone() {
            let uid = self.uid;
            let page_uid = page.widget_uid();
            let mut raised: Vec<String> = Vec::new();
            cx.map_actions(
                |cx| {
                    cx.group_widget_actions(uid, page_uid, |cx| page.handle_event(cx, event, scope))
                },
                |_cx, buf| {
                    for a in buf.iter() {
                        if let Some(wa) = a.downcast_ref::<WidgetAction>() {
                            raised.push(format!("{:?}", wa.action));
                        }
                    }
                    buf
                },
            );
            for line in raised {
                if self.log.len() >= LOG_CAPACITY {
                    self.log.pop_front();
                }
                self.log.push_back(line);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let stale = match (&self.wanted, &self.shown) {
            (Some(w), Some((s, _))) => w != s,
            (Some(_), None) => true,
            (None, Some(_)) => true,
            (None, None) => false,
        };
        // The net under `build`. A story built here announces itself
        // too late to be captured behind, so anything glass shows one
        // wrong frame — see `build`. Every path this app takes builds
        // at event time; this is for the one that does not.
        if stale {
            self.shown = None;
            cx.widget_tree_mark_dirty(self.uid);
            if let Some(dsl) = self.wanted.clone() {
                if let Some(page) = self.instantiate_in_draw(cx, &dsl) {
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

    pub fn apply(&self, cx: &mut Cx, target: &str, prop: &str, chunk: &str) -> Result<(), String> {
        match self.borrow_mut() {
            Some(mut inner) => inner.apply(cx, target, prop, chunk),
            None => Err("no canvas".to_string()),
        }
    }

    pub fn take_log(&self) -> Vec<String> {
        self.borrow_mut().map(|mut inner| inner.take_log()).unwrap_or_default()
    }
}
