use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    animator::{Animator, AnimatorImpl, Animate, AnimatorAction},
};

#[cfg(feature = "svg")]
use crate::makepad_draw::DrawSvg;

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.RadioButtonBase = #(RadioButton::register_widget(vm))
    
    mod.widgets.RadioButtonFlat = set_type_default() do mod.widgets.RadioButtonBase{
        width: Fit
        height: Fit
        align: Align{x: 0., y: 0.}
        padding: theme.mspace_v_2{left: theme.space_2}
        
        icon_walk: Walk{margin: Inset{left: 20.}}
        
        label_walk: Walk{
            width: Fit
            height: Fit
            margin: theme.mspace_h_1{left: 13.}
        }
        label_align: Align{y: 0.0}
        
        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            down: instance(0.0)
            active: instance(0.0)
            disabled: instance(0.0)

            size: uniform(15.0)
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_down: uniform(theme.color_inset_down)
            color_active: uniform(theme.color_inset_active)
            color_focus: uniform(theme.color_inset_focus)
            color_disabled: uniform(theme.color_inset_disabled)

            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_down: uniform(theme.color_bevel_down)
            border_color_active: uniform(theme.color_bevel_active)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_disabled: uniform(theme.color_bevel_disabled)

            mark_color: uniform(theme.color_mark_off)
            mark_color_active: uniform(theme.color_mark_active)
            mark_color_disabled: uniform(theme.color_mark_disabled)
            mark_offset: uniform(0.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let sz_px = self.size
                let radius_px = sz_px * 0.5
                let center_px = vec2(radius_px, self.rect_size.y * 0.5)

                // Draw background circle
                sdf.circle(center_px.x, center_px.y, radius_px - self.border_size)

                let color_fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)

                let color_stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_active, self.active)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_down, self.down)
                    .mix(self.border_color_disabled, self.disabled)

                sdf.fill_keep(color_fill)
                sdf.stroke(color_stroke, self.border_size)

                // Draw mark (inner dot)
                sdf.circle(center_px.x, center_px.y + self.mark_offset, radius_px * 0.5 - self.border_size * 0.75)

                let mark_color = self.mark_color
                    .mix(self.mark_color_active, self.active)
                    .mix(self.mark_color_disabled, self.disabled)

                sdf.fill(mark_color)
                return sdf.result
            }
        }
            
        draw_text +: {
            focus: instance(0.0)
            hover: instance(0.0)
            down: instance(0.0)
            active: instance(0.0)
            disabled: instance(0.0)

            color: theme.color_label_outer
            color_hover: uniform(theme.color_label_outer_hover)
            color_down: uniform(theme.color_label_outer_down)
            color_focus: uniform(theme.color_label_outer_focus)
            color_active: uniform(theme.color_label_outer_active)
            color_disabled: uniform(theme.color_label_outer_disabled)

            get_color: fn() {
                return self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)
            }
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
        }
            
        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.RadioButton = mod.widgets.RadioButtonFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_down: theme.color_bevel_inset_1_down
            border_color_active: theme.color_bevel_inset_1_active
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }
    }

    mod.widgets.RadioButtonFlatter = mod.widgets.RadioButton{
        draw_text +: {
            color: theme.color_label_outer_off
            color_hover: theme.color_label_outer_hover
            color_down: theme.color_label_outer_down
            color_active: theme.color_label_outer_active
            color_disabled: theme.color_label_outer_disabled
        }

        label_walk +: {margin: 0.}

        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
    }

    mod.widgets.RadioButtonTabFlat = mod.widgets.RadioButton{
        height: Fit
        label_walk +: {
            margin: Inset{left: 12., right: 4.}
        }
        padding: theme.mspace_2{left: -2.}

        draw_bg +: {
            color: theme.color_inset
            color_active: theme.color_outset_active
            color_disabled: theme.color_inset_disabled

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )

                let color_fill = self.color
                    .mix(self.color_active, self.active)
                    .mix(self.color_disabled, self.disabled)

                let color_stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_active, self.active)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_down, self.down)
                    .mix(self.border_color_disabled, self.disabled)

                sdf.fill_keep(color_fill)
                sdf.stroke(color_stroke, self.border_size)
                return sdf.result
            }
        }

        draw_text +: {
            color: theme.color_label_inner
            color_active: theme.color_label_inner_active
            color_disabled: theme.color_label_inner_disabled
        }
    }

    mod.widgets.RadioButtonTab = mod.widgets.RadioButtonTabFlat{
        draw_bg +: {
            border_color: theme.color_bevel_outset_1
            border_color_hover: theme.color_bevel_outset_1_hover
            border_color_down: theme.color_bevel_outset_1_down
            border_color_active: theme.color_bevel_outset_1_active
            border_color_focus: theme.color_bevel_outset_1_focus
            border_color_disabled: theme.color_bevel_outset_1_disabled
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum RadioButtonAction {
    Clicked,
    #[default]
    None
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct RadioButton {
    #[source] source: ScriptObjectRef,
    
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[apply_default]
    animator: Animator,
    
    #[live] icon_walk: Walk,
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] draw_text: DrawText,
    #[cfg(feature = "svg")]
    #[live] draw_icon: DrawSvg,
    
    #[live] text: ArcStringMut,

    #[visible] #[live(true)]
    pub visible: bool,
    
    #[live] bind: String,
    #[action_data] #[rust] action_data: WidgetActionData,
}

impl RadioButton {
    
    pub fn draw_radio_button(&mut self, cx: &mut Cx2d, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        #[cfg(feature = "svg")]
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done() 
    }
    
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            matches!(item.cast(), RadioButtonAction::Clicked)
        } else {
            false
        }
    }
    
    pub fn active(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(active.on))
    }
    
    pub fn set_active(&mut self, cx: &mut Cx, value: bool) {
        self.animator_toggle(cx, value, Animate::Yes, ids!(active.on), ids!(active.off));
    }
}

impl Widget for RadioButton {

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }
                
    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, ids!(hover.off));
            },
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.animator_in_state(cx, ids!(active.off)) {
                    self.animator_play(cx, ids!(hover.down));
                }
                self.set_key_focus(cx);
            },
            Hit::FingerUp(_fe) => {
                self.animator_play(cx, ids!(hover.on));
                if self.animator_in_state(cx, ids!(active.off)) {
                    self.animator_play(cx, ids!(active.on));
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, RadioButtonAction::Clicked);
                }
                // Radio buttons don't toggle off when clicked again
            }
            Hit::FingerMove(_fe) => {
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_radio_button(cx, walk)
    }
    
    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl RadioButtonRef {
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.borrow().is_some_and(|inner| inner.clicked(actions))
    }
    
    pub fn unselect(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_play(cx, ids!(active.off));
        }
    }

    pub fn select(&self, cx: &mut Cx, scope: &mut Scope) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.animator_in_state(cx, ids!(active.off)) {
                inner.animator_play(cx, ids!(active.on));
                cx.widget_action_with_data(&inner.action_data, inner.widget_uid(), &scope.path, RadioButtonAction::Clicked);
            }
        }
    }
    
    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            let s = inner.text.as_mut_empty();
            s.push_str(text);
        }
    }
    
    pub fn active(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.active(cx)
        } else {
            false
        }
    }
    
    pub fn set_active(&self, cx: &mut Cx, value: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_active(cx, value);
        }
    }
}

impl RadioButtonSet {
    pub fn selected(&self, cx: &mut Cx, actions: &Actions) -> Option<usize> {
        for (index, item) in self.iter().enumerate() {
            if item.clicked(actions) {
                // Unselect all other radio buttons
                for (i, other) in self.iter().enumerate() {
                    if i != index {
                        other.unselect(cx);
                    }
                }
                return Some(index);
            }
        }
        None
    }
}
