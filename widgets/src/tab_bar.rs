use crate::{
    animator::Animate,
    makepad_derive_widget::*,
    makepad_draw::*,
    scroll_bars::ScrollBars,
    tab::{Tab, TabAction},
    widget::*,
};
use std::collections::HashMap;

/// A sample of finger position and time, used for flick velocity calculation.
#[derive(Copy, Clone)]
struct FingerScrollSample {
    abs: f64,
    time: f64,
}

/// Tracks the state of a finger-based drag-to-scroll gesture on the tab bar.
enum FingerScrollState {
    Idle,
    Dragging { samples: Vec<FingerScrollSample> },
    Flicking { delta: f64, next_frame: NextFrame },
}

impl Default for FingerScrollState {
    fn default() -> Self { Self::Idle }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TabBarBase = #(TabBar::register_widget(vm))

    mod.widgets.TabBar = set_type_default() do mod.widgets.TabBarBase{
        CloseableTab := mod.widgets.Tab{closeable: true}
        PermanentTab := mod.widgets.Tab{closeable: false}

        width: Fill
        height: max(theme.tab_height, 25.)
        margin: 0.

        draw_drag +: {
            draw_depth: 10
            color: theme.color_bg_container
        }

        draw_fill +: {
            color_dither: uniform(0.0)
            border_radius: uniform(theme.corner_radius)
            border_size: uniform(theme.beveling)
            gradient_fill_horizontal: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)
            color_2: uniform(#0000)
            border_color: uniform(#fff0)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut color_fill = self.color
                let mut color_stroke = self.border_color

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_squeeze = 20.
                    let dir = if self.gradient_fill_horizontal > 0.5
                        pow(self.pos.x, gradient_squeeze) + dither
                    else
                        pow(self.pos.y, gradient_squeeze) + dither
                    color_fill = mix(self.color, self.color_2, dir)
                }

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_squeeze = 20.
                    let dir = if self.gradient_border_horizontal > 0.5
                        pow(self.pos.x, gradient_squeeze) + dither
                    else
                        pow(self.pos.y, gradient_squeeze) + dither
                    color_stroke = mix(self.border_color, self.border_color_2, dir)
                }

                sdf.box_all(
                    1.
                    1.
                    self.rect_size.x - 2.
                    self.rect_size.y - 2.
                    0.5
                    self.border_radius
                    0.5
                    0.5
                )

                sdf.fill(color_fill)
                sdf.stroke(color_stroke, self.border_size)

                return sdf.result
            }
        }

        draw_bg +: {
            color_dither: uniform(1.0)
            border_radius: uniform(0.)
            border_size: uniform(theme.beveling)
            color: theme.color_bg_app * 0.875
            gradient_fill_horizontal: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)
            color_2: instance(vec4(-1.0, -1.0, -1.0, -1.0));
            border_color: uniform(#fff0)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            pixel: fn() {
               let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut color_fill = self.color
                let mut color_stroke = self.border_color

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_squeeze = 20.
                    let dir = if self.gradient_fill_horizontal > 0.5
                        pow(self.pos.x, gradient_squeeze)
                    else
                        pow(self.pos.y, gradient_squeeze)
                    color_fill = mix(self.color, self.color_2, dir)
                }

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_squeeze = 20.
                    let dir = if self.gradient_border_horizontal > 0.5
                        pow(self.pos.x, gradient_squeeze) + dither
                    else
                        pow(self.pos.y, gradient_squeeze) + dither
                    color_stroke = mix(self.border_color, self.border_color_2, dir)
                }

                sdf.rect(
                    1.
                    1.
                    self.rect_size.x - 1.5
                    self.rect_size.y - 1.5
                )

                sdf.fill_keep(color_fill)
                sdf.stroke(color_stroke, self.border_size)
                return sdf.result
            }
        }

        scroll_bars: ScrollBarsTabs{
            show_scroll_x: true
            show_scroll_y: false
            scroll_bar_x +: {
                draw_bg +: {
                    color_hover: #fff6
                    size: 5.0
                }
                bar_size: 7.5
                use_vertical_finger_scroll: true
            }
        }
    }

    mod.widgets.TabBarFlat = mod.widgets.TabBar{
        height: max(theme.tab_flat_height, 25.)

        CloseableTab := mod.widgets.TabFlat{closeable: true}
        PermanentTab := mod.widgets.TabFlat{closeable: false}
    }

    mod.widgets.TabBarGradientX = mod.widgets.TabBar{
        CloseableTab := mod.widgets.TabGradientX{closeable: true}
        PermanentTab := mod.widgets.TabGradientX{closeable: false}

        draw_bg +: {
            gradient_fill_horizontal: 1.0
            gradient_border_horizontal: 1.0
            color_dither: 1.0
            border_radius: theme.corner_radius
            color: theme.color_bg_app * 0.8
            color_2: theme.color_bg_app * 1.2
        }
    }

    mod.widgets.TabBarGradientY = mod.widgets.TabBar{
        CloseableTab := mod.widgets.TabGradientY{closeable: true}
        PermanentTab := mod.widgets.TabGradientY{closeable: false}
        draw_bg +: {
            gradient_fill_horizontal: 0.0
            gradient_border_horizontal: 0.0
            color_dither: 1.0
            border_radius: 0.
            border_size: theme.beveling
            color: theme.color_bg_app * 0.875
            color_2: theme.color_shadow
        }

        draw_fill +: {
            color_dither: 1.0
            border_radius: theme.corner_radius
            color: theme.color_bg_app * 0.9
            color_2: #282828
        }
    }
}

#[derive(Script, Widget)]
pub struct TabBar {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

    #[redraw]
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    draw_drag: DrawColor,

    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_fill: DrawColor,
    #[walk]
    walk: Walk,

    #[rust]
    draw_state: DrawStateWrap<()>,
    #[rust]
    view_area: Area,

    #[rust]
    tab_order: Vec<LiveId>,

    #[rust]
    is_dragged: bool,

    // Templates stored as rooted ScriptObjectRef - populated in on_after_apply
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    tabs: ComponentMap<LiveId, (Tab, LiveId)>,

    #[rust]
    active_tab: Option<usize>,

    #[rust]
    active_tab_id: Option<LiveId>,
    #[rust]
    prev_active_tab_id: Option<LiveId>,
    #[rust]
    next_active_tab_id: Option<LiveId>,

    /// State for finger-based drag-to-scroll on the tab bar.
    #[rust]
    finger_scroll: FingerScrollState,

    /// Smooth scroll animation state for scrolling a selected tab into view.
    #[rust]
    scroll_into_view_anim: Option<ScrollIntoViewAnim>,
}

struct ScrollIntoViewAnim {
    target_scroll_x: f64,
    next_frame: NextFrame,
}

impl ScriptHook for TabBar {
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
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Collect templates from the object's vec - templates use prefixed ids (CloseableTab, PermanentTab)
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        // Templates defined in the DSL end up in the vec
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

        // Update existing tabs if templates changed
        if apply.is_reload() {
            for (_, (tab, templ_id)) in self.tabs.iter_mut() {
                if let Some(template_ref) = self.templates.get(templ_id) {
                    let template_value: ScriptValue = template_ref.as_object().into();
                    tab.script_apply(vm, apply, scope, template_value);
                }
            }
        }
    }
}

impl Widget for TabBar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.scroll_bars.handle_event(cx, event, scope).len() > 0 {
            self.view_area.redraw(cx);
        };

        self.handle_finger_scroll_flick(cx, event);
        self.handle_scroll_into_view_anim(cx, event);

        if let Some(tab_id) = self.next_active_tab_id.take() {
            cx.widget_action(uid, TabBarAction::TabWasPressed(tab_id));
        }
        for (tab_id, (tab, _)) in self.tabs.iter_mut() {
            tab.handle_event_with(cx, event, &mut |cx, action| match action {
                TabAction::WasPressed => {
                    cx.widget_action(uid, TabBarAction::TabWasPressed(*tab_id));
                }
                TabAction::CloseWasPressed => {
                    cx.widget_action(uid, TabBarAction::TabCloseWasPressed(*tab_id));
                }
                TabAction::ShouldTabStartDrag => {
                    cx.widget_action(uid, TabBarAction::ShouldTabStartDrag(*tab_id));
                }
                TabAction::ShouldTabStopDrag => {}
                TabAction::TouchDown { abs, time } => {
                    self.finger_scroll = FingerScrollState::Dragging {
                        samples: vec![FingerScrollSample { abs: abs.x, time }],
                    };
                }
                TabAction::TouchScroll { abs, time } => {
                    if let FingerScrollState::Dragging { samples } = &mut self.finger_scroll {
                        let Some(old_abs) = samples.last().map(|s| s.abs) else { return };
                        samples.push(FingerScrollSample { abs: abs.x, time });
                        if samples.len() > 4 {
                            samples.remove(0);
                        }
                        let delta = abs.x - old_abs;
                        let scroll_pos = self.scroll_bars.get_scroll_pos();
                        if self.scroll_bars.set_scroll_pos(cx, Vec2d { x: scroll_pos.x - delta, y: scroll_pos.y }) {
                            self.view_area.redraw(cx);
                        }
                    }
                }
                TabAction::TouchUp { abs: _, time: _, } => {
                    if let FingerScrollState::Dragging { samples } = &self.finger_scroll {
                        // Calculate flick velocity from recent samples.
                        let mut last: Option<&FingerScrollSample> = None;
                        let mut scaled_delta = 0.0;
                        let mut total_delta = 0.0;
                        for sample in samples.iter().rev() {
                            if let Some(prev) = last {
                                let time_delta = prev.time - sample.time;
                                if time_delta > 0.0 {
                                    let abs_delta = prev.abs - sample.abs;
                                    total_delta += abs_delta;
                                    scaled_delta += abs_delta / time_delta;
                                }
                            }
                            last = Some(sample);
                        }
                        const FLICK_SCALING: f64 = 0.005;
                        const FLICK_MINIMUM: f64 = 0.2;
                        const FLICK_MAXIMUM: f64 = 80.0;
                        scaled_delta *= FLICK_SCALING;
                        if total_delta.abs() > 10.0 && scaled_delta.abs() > FLICK_MINIMUM {
                            let delta = scaled_delta.min(FLICK_MAXIMUM).max(-FLICK_MAXIMUM);
                            self.finger_scroll = FingerScrollState::Flicking {
                                delta,
                                next_frame: cx.new_next_frame(),
                            };
                        } else {
                            self.finger_scroll = FingerScrollState::Idle;
                        }
                    }
                }
            });
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            return DrawStep::make_step();
        }
        if let Some(()) = self.draw_state.get() {
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl TabBar {
    /// Drives the smooth scroll animation for scrolling a selected tab into view.
    fn handle_scroll_into_view_anim(&mut self, cx: &mut Cx, event: &Event) {
        const SMOOTHING: f64 = 0.12;

        let target = if let Some(anim) = &self.scroll_into_view_anim {
            if anim.next_frame.is_event(event).is_some() {
                Some(anim.target_scroll_x)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(target) = target {
            let current = self.scroll_bars.get_scroll_pos().x;
            let remaining = target - current;
            if remaining.abs() < 1.0 {
                // Close enough — snap to target and stop.
                self.scroll_bars.set_scroll_pos(cx, Vec2d { x: target, y: 0.0 });
                self.scroll_into_view_anim = None;
            } else {
                // Ease toward target.
                let new_x = current + remaining * SMOOTHING;
                self.scroll_bars.set_scroll_pos(cx, Vec2d { x: new_x, y: 0.0 });
                self.scroll_into_view_anim = Some(ScrollIntoViewAnim {
                    target_scroll_x: target,
                    next_frame: cx.new_next_frame(),
                });
            }
            self.view_area.redraw(cx);
        }
    }

    /// Drives the flick animation for finger-based scroll on each frame.
    fn handle_finger_scroll_flick(&mut self, cx: &mut Cx, event: &Event) {
        const FLICK_DECAY: f64 = 0.97;
        const FLICK_MINIMUM: f64 = 0.2;

        let flick_delta = if let FingerScrollState::Flicking { delta, next_frame } = &self.finger_scroll {
            if next_frame.is_event(event).is_some() {
                Some(*delta)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(mut delta) = flick_delta {
            delta *= FLICK_DECAY;
            if delta.abs() > FLICK_MINIMUM {
                let scroll_pos = self.scroll_bars.get_scroll_pos();
                if self.scroll_bars.set_scroll_pos(cx, Vec2d { x: scroll_pos.x - delta, y: scroll_pos.y }) {
                    self.view_area.redraw(cx);
                }
                self.finger_scroll = FingerScrollState::Flicking {
                    delta,
                    next_frame: cx.new_next_frame(),
                };
            } else {
                self.finger_scroll = FingerScrollState::Idle;
            }
        }
    }

    pub fn begin(&mut self, cx: &mut Cx2d, active_tab: Option<usize>, walk: Walk) {
        self.active_tab = active_tab;
        self.scroll_bars.begin(cx, walk, Layout::flow_right());
        self.draw_bg.draw_abs(cx, cx.turtle().rect_unscrolled());
        self.tab_order.clear();
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        if self.is_dragged {
            self.draw_drag.draw_walk(
                cx,
                Walk {
                    width: Size::fill(),
                    height: Size::fill(),
                    ..Walk::default()
                },
            );
        }
        self.tabs.retain_visible();
        self.draw_fill
            .draw_walk(cx, Walk::new(Size::fill(), Size::fill()));

        self.scroll_bars.end(cx);

        // After scroll_bars.end() so that view_visible/view_total are up to date.
        if self.active_tab_id != self.prev_active_tab_id {
            self.prev_active_tab_id = self.active_tab_id;
            if let Some(tab_id) = self.active_tab_id {
                if let Some((tab, _)) = self.tabs.get(&tab_id) {
                    // Convert the tab's absolute rect to content-relative coordinates.
                    let abs_rect = tab.area().rect(cx);
                    let container_pos = self.scroll_bars.area().rect(cx).pos;
                    let scroll_pos = self.scroll_bars.get_scroll_pos();
                    let content_x = abs_rect.pos.x - container_pos.x + scroll_pos.x;
                    let view_visible = self.scroll_bars.get_scroll_view_visible().x;

                    // Calculate the minimal target scroll position.
                    let target = if content_x < scroll_pos.x {
                        // Tab is off the left edge — scroll left.
                        content_x
                    } else if content_x + abs_rect.size.x > scroll_pos.x + view_visible {
                        // Tab is off the right edge — scroll right.
                        (content_x + abs_rect.size.x) - view_visible
                    } else {
                        // Already fully visible.
                        return;
                    };

                    // Clamp to valid range.
                    let view_total = self.scroll_bars.get_scroll_view_total().x;
                    let target = target.max(0.0).min((view_total - view_visible).max(0.0));

                    self.scroll_into_view_anim = Some(ScrollIntoViewAnim {
                        target_scroll_x: target,
                        next_frame: cx.new_next_frame(),
                    });
                }
            }
        }
    }

    pub fn draw_tab(&mut self, cx: &mut Cx2d, tab_id: LiveId, name: &str, template: LiveId) {
        if let Some(active_tab) = self.active_tab {
            let tab_order_len = self.tab_order.len();
            let tab = self.get_or_create_tab(cx, tab_id, template);
            if tab_order_len == active_tab {
                tab.set_is_active(cx, true, Animate::No);
            } else {
                tab.set_is_active(cx, false, Animate::No);
            }
            tab.draw(cx, name);
            if tab_order_len == active_tab {
                self.active_tab_id = Some(tab_id);
            }
            self.tab_order.push(tab_id);
        } else {
            self.tab_order.push(tab_id);
            let tab = self.get_or_create_tab(cx, tab_id, template);
            tab.draw(cx, name);
        }
    }

    fn get_or_create_tab(&mut self, cx: &mut Cx, tab_id: LiveId, template: LiveId) -> &mut Tab {
        let template_value: Option<ScriptValue> =
            self.templates.get(&template).map(|r| r.as_object().into());
        let (tab, _) = self.tabs.get_or_insert(cx, tab_id, |cx| {
            let tab = if let Some(value) = template_value {
                cx.with_vm(|vm| Tab::script_from_value(vm, value))
            } else {
                cx.with_vm(|vm| Tab::script_new(vm))
            };
            (tab, template)
        });
        tab
    }

    /// Creates a new Tab from the same template as the given tab, with the same active state.
    /// Returns `None` if the tab_id isn't found.
    pub fn create_ghost_tab(&self, cx: &mut Cx, tab_id: LiveId) -> Option<Tab> {
        let (tab, template_id) = self.tabs.get(&tab_id)?;
        let is_active = tab.is_active();
        let template_value: ScriptValue = self.templates.get(template_id)?.as_object().into();
        let mut ghost = cx.with_vm(|vm| Tab::script_from_value(vm, template_value));
        ghost.set_is_active(cx, is_active, Animate::No);
        Some(ghost)
    }

    pub fn active_tab_id(&self) -> Option<LiveId> {
        self.active_tab_id
    }

    pub fn set_active_tab_id(&mut self, cx: &mut Cx, tab_id: Option<LiveId>, animate: Animate) {
        if self.active_tab_id == tab_id {
            return;
        }
        if let Some(tab_id) = self.active_tab_id {
            let (tab, _) = &mut self.tabs[tab_id];
            tab.set_is_active(cx, false, animate);
        }
        self.active_tab_id = tab_id;
        if let Some(tab_id) = self.active_tab_id {
            let (tab, _) = &mut self.tabs[tab_id];
            tab.set_is_active(cx, true, animate);
        }
        self.view_area.redraw(cx);
    }

    pub fn set_next_active_tab(&mut self, cx: &mut Cx, tab_id: LiveId, animate: Animate) {
        if let Some(index) = self.tab_order.iter().position(|id| *id == tab_id) {
            if self.active_tab_id != Some(tab_id) {
                self.next_active_tab_id = self.active_tab_id;
            } else if index > 0 {
                self.next_active_tab_id = Some(self.tab_order[index - 1]);
                self.set_active_tab_id(cx, self.next_active_tab_id, animate);
            } else if index + 1 < self.tab_order.len() {
                self.next_active_tab_id = Some(self.tab_order[index + 1]);
                self.set_active_tab_id(cx, self.next_active_tab_id, animate);
            } else {
                self.set_active_tab_id(cx, None, animate);
            }
            cx.new_next_frame();
        }
    }
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.view_area.redraw(cx)
    }

    pub fn is_over_tab(&self, cx: &Cx, abs: Vec2d) -> Option<(LiveId, Rect)> {
        for (tab_id, (tab, _)) in self.tabs.iter() {
            let rect = tab.area().rect(cx);
            if rect.contains(abs) {
                return Some((*tab_id, rect));
            }
        }
        None
    }

    pub fn tab_rect(&self, cx: &Cx, tab_id: LiveId) -> Option<Rect> {
        self.tabs.get(&tab_id).map(|(tab, _)| tab.area().rect(cx))
    }

    pub fn bar_rect(&self, cx: &Cx) -> Rect {
        self.scroll_bars.area().rect(cx)
    }

    pub fn is_over_tab_bar(&self, cx: &Cx, abs: Vec2d) -> Option<Rect> {
        let rect = self.bar_rect(cx);
        if rect.contains(abs) {
            return Some(rect);
        }
        None
    }
}

#[derive(Clone, Debug, Default)]
pub enum TabBarAction {
    TabWasPressed(LiveId),
    ShouldTabStartDrag(LiveId),
    TabCloseWasPressed(LiveId),
    #[default]
    None,
}
