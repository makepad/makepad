use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

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
        $title: H1{
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
        $title: H1{
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

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct SlidesView {
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
        // Handle $prop children from the object's vec (these are our slide templates)
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if kv.key.is_prefixed_id() {
                            // $prop children are our slides
                            if let Some(id) = kv.key.as_id() {
                                if let Some(template_obj) = kv.value.as_object() {
                                    self.templates.insert(id, vm.bx.heap.new_object_ref(template_obj));
                                    self.draw_order.push(id);
                                }

                                // If we already have this slide instantiated, apply updates to it
                                if let Some(slide) = self.slides.get_mut(&id) {
                                    slide.script_apply(vm, apply, scope, kv.value);
                                }
                            }
                        }
                    }
                });
            }
        }

        // Create all slides upfront (slides need to be available for navigation)
        if apply.is_new() || apply.is_reload() {
            for (slide_id, template_ref) in self.templates.iter() {
                if !self.slides.contains_key(slide_id) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    let slide = WidgetRef::script_from_value_scoped(vm, scope, template_value);
                    self.slides.insert(*slide_id, slide);
                }
            }
        }
    }
}

impl WidgetNode for SlidesView {
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }

    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        for child in self.slides.values() {
            child.find_widgets(path, cached, results);
        }
    }

    fn widget_tree_walk(&self, nodes: &mut Vec<WidgetTreeNode>) {
        for (id, child) in self.slides.iter() {
            child.widget_tree_walk_named(*id, nodes);
        }
    }

    fn uid_to_widget(&self, uid: WidgetUid) -> WidgetRef {
        for child in self.slides.values() {
            let x = child.uid_to_widget(uid);
            if !x.is_empty() {
                return x;
            }
        }
        WidgetRef::empty()
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
                scope.with_id(*current_id, |scope| {
                    current.handle_event(cx, event, scope);
                })
            }
        }
        if self.current_slide.fract() > 0.0 {
            let next = current + 1;
            if let Some(next_id) = self.draw_order.get(next) {
                if let Some(next) = self.slides.get(&next_id) {
                    scope.with_id(*next_id, |scope| {
                        next.handle_event(cx, event, scope);
                    })
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
                cx.widget_action(
                    uid,
                    &scope.path,
                    SlidesViewAction::Flipped(self.goal_slide as usize),
                );
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ArrowLeft,
                ..
            }) => {
                self.prev_slide(cx);
                let uid = self.widget_uid();
                cx.widget_action(
                    uid,
                    &scope.path,
                    SlidesViewAction::Flipped(self.goal_slide as usize),
                );
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
                    metrics: Metrics::default(),
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
                    scope.with_id(*first_id, |scope| slide.draw_walk(cx, scope, walk))?;
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
                    metrics: Metrics::default(),
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
                        scope.with_id(*second_id, |scope| slide.draw_walk(cx, scope, walk))?;
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
        self.goal_slide += 1.0;
        let max_goal_slide = (self.draw_order.len().max(1) - 1) as f64;
        if self.goal_slide > max_goal_slide {
            self.goal_slide = max_goal_slide
        }
        self.next_frame(cx);
    }

    pub fn prev_slide(&mut self, cx: &mut Cx) {
        self.goal_slide -= 1.0;
        if self.goal_slide < 0.0 {
            self.goal_slide = 0.0;
        }
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
