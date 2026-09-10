//! SlidesView — a deck: one slide at a time, filling what it is given.
//!
//! The slides are the children written under it, in the order they are
//! written; the arrow keys move between them and the deck slides sideways
//! from one to the next rather than cutting.
//!
//! It deliberately does NOT paginate, scroll, or show two slides at once
//! except while it is moving between them: anything on screen beside the
//! slide is the thing a slide exists to keep off it. Nor does it size
//! itself to its content — a slide is handed the deck's rect and is
//! expected to fit in it, so the deck needs a real height of its own.
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, widget_tree::CxWidgetExt};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SlidesViewBase = #(SlidesView::register_widget(vm))

    mod.widgets.SlidesView = set_type_default() do mod.widgets.SlidesViewBase{
        anim_speed: 0.9
    }

    mod.widgets.Slide = mod.widgets.RoundedView{
        width: Fill
        height: Fill
        flow: Down
        spacing: 10
        align: Align{x: 0.0 y: 0.5}
        padding: 50.
        draw_bg +: {
            color: theme.color_inset_1
            color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            radius: theme.container_corner_radius
        }
        title := H1{
            text: "SlideTitle"
            draw_text +: {
                color: theme.color_text
            }
        }
    }

    mod.widgets.SlideChapter = mod.widgets.Slide{
        width: Fill
        height: Fill
        flow: Down
        align: Align{x: 0.0 y: 0.5}
        spacing: 10
        padding: 50
        draw_bg +: {
            color: theme.color_makepad
            color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            radius: theme.container_corner_radius
        }
        title := H1{
            text: "SlideTitle"
            draw_text +: {
                color: theme.color_text
            }
        }
    }

    mod.widgets.SlideBody = mod.widgets.H2{
        text: "Body of the slide"
        draw_text +: {
            color: theme.color_text
        }
    }
}

#[derive(Clone)]
enum DrawState {
    DrawFirst,
    DrawSecond,
}

#[derive(Clone, Debug, Default)]
pub enum SlidesViewAction {
    Flipped(usize),
    #[default]
    None,
}

/// Where an entry written under the deck is filed.
///
/// `named` is the id it was written with (`intro := Slide{}`); `nil_key`
/// says it carried no key at all, which is how a slide is normally
/// written. The unnamed ones are numbered in the order they appear, and
/// that numbering is the deck's order — dropping them, as this once did,
/// is what left a deck of plain `Slide{}` children with no slides in it.
/// An entry that is neither is not a child and is passed over.
fn entry_id(named: Option<LiveId>, nil_key: bool, anonymous: &mut usize) -> Option<LiveId> {
    if named.is_some() {
        return named;
    }
    if nil_key {
        let id = LiveId(*anonymous as u64);
        *anonymous += 1;
        return Some(id);
    }
    None
}

/// The slide a goal lands on. There is nothing past either end of a deck,
/// and an empty deck rests on the first slide rather than on the one
/// before it.
fn clamp_goal(goal: f64, count: usize) -> f64 {
    goal.clamp(0.0, (count.max(1) - 1) as f64)
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct SlidesView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[layout]
    layout: Layout,
    #[rust]
    area: Area,
    #[walk]
    walk: Walk,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    current_slide: f64,
    #[rust]
    goal_slide: f64,
    #[live]
    anim_speed: f64,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust]
    templates: ComponentMap<LiveId, ScriptObjectRef>,
    #[rust]
    slides: ComponentMap<LiveId, WidgetRef>,
    #[rust]
    draw_order: Vec<LiveId>,
}

impl ScriptHook for SlidesView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.next_frame(cx);
        });
    }

    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
            self.draw_order.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // The children written under the deck are its slides. Only collect
        // during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                let mut anonymous = 0usize;
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        let Some(id) = entry_id(kv.key.as_id(), kv.key.is_nil(), &mut anonymous)
                        else {
                            continue;
                        };
                        // A deck may hold anything a script can write; only
                        // the things that can become widgets are slides.
                        if !WidgetRef::value_is_newable_widget(vm, kv.value) {
                            continue;
                        }
                        if let Some(template_obj) = kv.value.as_object() {
                            self.templates.insert(id, vm.bx.heap.new_object_ref(template_obj));
                            // The written order is the deck's order. An id
                            // already in it is this slide being applied
                            // again, not another slide behind it.
                            if !self.draw_order.contains(&id) {
                                self.draw_order.push(id);
                            }
                        }

                        // If we already have this slide instantiated, apply updates to it
                        if let Some(slide) = self.slides.get_mut(&id) {
                            slide.script_apply(vm, apply, scope, kv.value);
                        }
                    }
                });
            }
        }

        // Create all slides upfront (slides need to be available for navigation)
        if apply.is_new() || apply.is_reload() {
            let mut new_slides = Vec::new();
            for (slide_id, template_ref) in self.templates.iter() {
                if !self.slides.contains_key(slide_id) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    let slide = WidgetRef::script_from_value_scoped(vm, scope, template_value);
                    self.slides.insert(*slide_id, slide.clone());
                    new_slides.push((*slide_id, slide));
                }
            }
            let cx = vm.cx_mut();
            for (slide_id, slide) in new_slides {
                cx.widget_tree_insert_child_deep(self.uid, slide_id, slide);
            }
        } else {
            vm.cx_mut().widget_tree_mark_dirty(self.uid);
        }
    }
}

impl WidgetNode for SlidesView {
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
        for (id, child) in self.slides.iter() {
            visit(*id, child.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }
}

impl Widget for SlidesView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::NextFrame(ne) if ne.set.contains(&self.next_frame) => {
                self.current_slide = self.current_slide * self.anim_speed
                    + self.goal_slide * (1.0 - self.anim_speed);
                if (self.current_slide - self.goal_slide).abs() > 0.00001 {
                    self.next_frame(cx);
                    self.area.redraw(cx);
                } else {
                    self.current_slide = self.current_slide.round();
                }
            }
            _ => (),
        }

        let current = self.current_slide.floor() as usize;
        if let Some(current_id) = self.draw_order.get(current) {
            if let Some(current) = self.slides.get(&current_id) {
                current.handle_event(cx, event, scope);
            }
        }
        if self.current_slide.fract() > 0.0 {
            let next = current + 1;
            if let Some(next_id) = self.draw_order.get(next) {
                if let Some(next) = self.slides.get(&next_id) {
                    next.handle_event(cx, event, scope);
                }
            }
        }
        match event.hits(cx, self.area) {
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowRight,
                ..
            }) => {
                self.next_slide(cx);
                let uid = self.widget_uid();
                cx.widget_action(uid, SlidesViewAction::Flipped(self.goal_slide as usize));
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                ..
            }) => {
                self.prev_slide(cx);
                let uid = self.widget_uid();
                cx.widget_action(uid, SlidesViewAction::Flipped(self.goal_slide as usize));
            }
            Hit::FingerDown(_fe) => {
                cx.set_key_focus(self.area);
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, DrawState::DrawFirst) {
            cx.begin_turtle(walk, Layout::flow_overlay());
            let rect = cx.turtle().rect();
            cx.begin_turtle(
                Walk {
                    abs_pos: None,
                    margin: Default::default(),
                    width: Size::fill(),
                    height: Size::fill(),
                    ..Default::default()
                },
                Layout::flow_down()
                    .with_scroll(dvec2(rect.size.x * self.current_slide.fract(), 0.0)),
            );
        }
        if let Some(DrawState::DrawFirst) = self.draw_state.get() {
            let first = self.current_slide.floor() as usize;
            if let Some(first_id) = self.draw_order.get(first) {
                if let Some(slide) = self.slides.get(&first_id) {
                    let walk = slide.walk(cx);
                    slide.draw_walk(cx, scope, walk)?;
                }
            }
            cx.end_turtle();
            let rect = cx.turtle().rect();
            cx.begin_turtle(
                Walk {
                    abs_pos: None,
                    margin: Default::default(),
                    width: Size::fill(),
                    height: Size::fill(),
                    ..Default::default()
                },
                Layout::flow_down().with_scroll(dvec2(
                    -rect.size.x * (1.0 - self.current_slide.fract()),
                    0.0,
                )),
            );
            self.draw_state.set(DrawState::DrawSecond);
        }
        if let Some(DrawState::DrawSecond) = self.draw_state.get() {
            if self.current_slide.fract() > 0.0 {
                let second = self.current_slide.floor() as usize + 1;
                if let Some(second_id) = self.draw_order.get(second) {
                    if let Some(slide) = self.slides.get(&second_id) {
                        let walk = slide.walk(cx);
                        slide.draw_walk(cx, scope, walk)?;
                    }
                }
            }
        }
        cx.end_turtle();
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl SlidesView {
    fn next_frame(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
    }

    pub fn next_slide(&mut self, cx: &mut Cx) {
        self.goal_slide = clamp_goal(self.goal_slide + 1.0, self.draw_order.len());
        self.next_frame(cx);
    }

    pub fn prev_slide(&mut self, cx: &mut Cx) {
        self.goal_slide = clamp_goal(self.goal_slide - 1.0, self.draw_order.len());
        self.next_frame(cx);
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl SlidesViewRef {
    pub fn flipped(&self, actions: &Actions) -> Option<usize> {
        if let SlidesViewAction::Flipped(m) = actions.find_widget_action(self.widget_uid()).cast() {
            Some(m)
        } else {
            None
        }
    }

    pub fn set_current_slide(&self, cx: &mut Cx, slide: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.goal_slide = slide as f64;
            inner.current_slide = slide as f64;
            inner.redraw(cx);
        }
    }

    pub fn set_goal_slide(&self, cx: &mut Cx, slide: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.goal_slide = slide as f64;
            inner.next_frame(cx);
        }
    }

    pub fn get_slide(&self) -> usize {
        if let Some(inner) = self.borrow() {
            return inner.current_slide as usize;
        }
        0
    }

    pub fn next_slide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.next_slide(cx);
        }
    }

    pub fn prev_slide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.prev_slide(cx);
        }
    }
}

impl SlidesViewSet {
    pub fn next_slide(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.next_slide(cx);
        }
    }

    pub fn prev_slide(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.prev_slide(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids a run of children are filed under, in written order; `None`
    /// is a child written without a name, which is how slides are written.
    fn order(entries: &[Option<LiveId>]) -> Vec<LiveId> {
        let mut anonymous = 0usize;
        entries
            .iter()
            .filter_map(|named| entry_id(*named, named.is_none(), &mut anonymous))
            .collect()
    }

    #[test]
    fn a_deck_written_without_names_still_has_slides() {
        // The bug: a slide is written `Slide{...}` and carries no key, and
        // only entries with a key were collected — so none of them were,
        // and the deck drew nothing at all.
        assert_eq!(order(&[None, None, None]), vec![LiveId(0), LiveId(1), LiveId(2)]);
    }

    #[test]
    fn the_written_order_is_the_deck_order() {
        assert_eq!(
            order(&[None, Some(live_id!(outro)), None]),
            vec![LiveId(0), live_id!(outro), LiveId(1)],
            "a named slide keeps its name and does not spend a number"
        );
    }

    #[test]
    fn an_entry_that_is_neither_named_nor_bare_is_not_a_slide() {
        let mut anonymous = 0usize;
        assert_eq!(entry_id(None, false, &mut anonymous), None);
        assert_eq!(anonymous, 0, "and it does not spend a slide's number");
    }

    #[test]
    fn the_goal_stops_at_either_end_of_the_deck() {
        assert_eq!(clamp_goal(-1.0, 3), 0.0, "there is nothing before the first");
        assert_eq!(clamp_goal(3.0, 3), 2.0, "nor anything after the last");
        assert_eq!(clamp_goal(2.0, 3), 2.0);
    }

    #[test]
    fn an_empty_deck_rests_on_the_first_slide() {
        // A deck with no children still answers `next`, and 0 - 1 slides is
        // not a slide to sit on.
        assert_eq!(clamp_goal(1.0, 0), 0.0);
    }
}
