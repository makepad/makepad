use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub DrawCheckBox = {{DrawCheckBox}} {}
    pub CheckBoxBase = {{CheckBox}} {}
    
    pub CheckBox = <CheckBoxBase> {
        width: Fit, height: Fit,
        padding: <THEME_MSPACE_2> {}
        align: { x: 0., y: 0. }
        
        label_walk: {
            width: Fit, height: Fit,
            margin: <THEME_MSPACE_H_1> { left: 12.5 }
        }
        
        draw_check: {
            uniform size: 7.5;
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.check_type {
                    CheckType::Check => {
                        let left = 1;
                        let sz = self.size - 1.0;
                        let offset_x = 0.0;
                        let offset_y = -1.0;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        
                        sdf.box(left, c.y - sz, sz * 2.0, sz * 2.0, 1.5 + offset_y);
                        sdf.fill_keep(mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM, pow(self.pos.y, 1.)))
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pos.y), THEME_BEVELING)
                        
                        let szs = sz * 0.5;
                        let dx = 1.0;
                        sdf.move_to(left + 4.0, c.y);
                        sdf.line_to(c.x, c.y + szs);
                        sdf.line_to(c.x + szs, c.y - szs);
                        sdf.stroke(mix(
                            mix(THEME_COLOR_U_HIDDEN, THEME_COLOR_CTRL_HOVER, self.hover),
                            THEME_COLOR_TEXT_ACTIVE,
                            self.selected), 1.25
                        );
                    }
                    CheckType::Radio => {
                        let sz = self.size;
                        let left = 0.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM, pow(self.pos.y, 1.)))
                        sdf.stroke(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_LIGHT, self.pos.y), THEME_BEVELING)
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(
                            mix(
                                mix(
                                    THEME_COLOR_U_HIDDEN,
                                    THEME_COLOR_CTRL_HOVER,
                                    self.hover
                                ),
                                THEME_COLOR_TEXT_ACTIVE,
                                self.selected
                            )
                        );
                    }
                    CheckType::Toggle => {
                        let sz = self.size;
                        let left = 0.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);
                        
                        sdf.stroke_keep(
                            mix(
                                THEME_COLOR_BEVEL_SHADOW,
                                THEME_COLOR_BEVEL_LIGHT,
                                clamp(self.pos.y - 0.2, 0, 1)
                            ),
                            THEME_BEVELING
                        )
                            
                        sdf.fill(
                            mix(
                                mix(THEME_COLOR_INSET_PIT_TOP, THEME_COLOR_INSET_PIT_BOTTOM * 0.1, pow(self.pos.y, 1.0)),
                                mix(THEME_COLOR_INSET_PIT_TOP_HOVER * 1.75, THEME_COLOR_INSET_PIT_BOTTOM * 0.1, pow(self.pos.y, 1.0)),
                                self.hover
                            )
                        )
                        let isz = sz * 0.65;
                        sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                        sdf.circle(left + sz + self.selected * sz, c.y - 0.5, 0.425 * isz);
                        sdf.subtract();
                        sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                        sdf.blend(self.selected)
                        sdf.fill(mix(THEME_COLOR_TEXT_DEFAULT, THEME_COLOR_TEXT_HOVER, self.hover));
                    }
                    CheckType::None => {
                        sdf.fill(THEME_COLOR_D_HIDDEN);
                    }
                }
                return sdf.result
            }
        }
            
        draw_text: {
            instance focus: 0.0
            instance selected: 0.0
            instance hover: 0.0
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_DEFAULT,
                        THEME_COLOR_TEXT_DEFAULT,
                        self.hover
                    ),
                    THEME_COLOR_TEXT_DEFAULT,
                    self.selected
                )
            }
        }
            
        draw_icon: {
            instance focus: 0.0
            instance hover: 0.0
            instance selected: 0.0
            uniform color: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_active: (THEME_COLOR_TEXT_ACTIVE)
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        self.color * 1.4,
                        self.hover
                    ),
                    mix(
                        self.color_active,
                        self.color_active * 1.4,
                        self.hover
                    ),
                    self.selected
                )
            }
        }
            
        icon_walk: { width: 13.0, height: Fit }
            
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_check: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_check: {selected: 0.0},
                        draw_text: {selected: 0.0},
                        draw_icon: {selected: 0.0},
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_check: {selected: 1.0}
                        draw_text: {selected: 1.0}
                        draw_icon: {selected: 1.0},
                    }
                }
            }
        }
    }
        
    pub CheckBoxToggle = <CheckBox> {
        align: { x: 0., y: 0. }
        draw_check: { check_type: Toggle }
        label_walk: {
            margin: <THEME_MSPACE_H_1> { left: 22.5 }
        }
            
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.25}}
                    apply: {
                        draw_check: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_check: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            selected = {
                default: off
                off = {
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_check: {selected: 0.0},
                        draw_text: {selected: 0.0},
                        draw_icon: {selected: 0.0},
                    }
                }
                on = {
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_check: {selected: 1.0}
                        draw_text: {selected: 1.0}
                        draw_icon: {selected: 1.0},
                    }
                }
            }
        }
    }
        
    pub CheckBoxCustom = <CheckBox> {
        draw_check: { check_type: None }
        align: { x: 0.0, y: 0.5}
        label_walk: { margin: <THEME_MSPACE_H_2> {} }
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawCheckBox {
    #[deref] draw_super: DrawQuad,
    #[live] check_type: CheckType,
    #[live] hover: f32,
    #[live] focus: f32,
    #[live] selected: f32
}

#[derive(Live, LiveHook, LiveRegister)]
#[live_ignore]
#[repr(u32)]
pub enum CheckType {
    #[pick] Check = shader_enum(1),
    Radio = shader_enum(2),
    Toggle = shader_enum(3),
    None = shader_enum(4),
}

#[derive(Live, LiveHook, Widget)]
pub struct CheckBox {
    
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[live] icon_walk: Walk,
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    
    #[redraw] #[live] draw_check: DrawCheckBox,
    #[live] draw_text: DrawText,
    #[live] draw_icon: DrawIcon,
    
    #[live] text: ArcStringMut,
    
    #[live] bind: String,
    #[action_data] #[rust] action_data: WidgetActionData,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum CheckBoxAction {
    Change(bool),
    None
}
/*
#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawLabelText {
    #[deref] draw_super: DrawText,
    #[live] hover: f32,
    #[live] pressed: f32,
}*/

impl CheckBox {
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_check.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_check.end(cx);
    }
}

impl Widget for CheckBox {
    
    fn widget_to_data(&self, _cx: &mut Cx, actions: &Actions, nodes: &mut LiveNodeVec, path: &[LiveId]) -> bool {
        match actions.find_widget_action_cast(self.widget_uid()) {
            CheckBoxAction::Change(v) => {
                nodes.write_field_value(path, LiveValue::Bool(v));
                true
            }
            _ => false
        }
    }
    
    fn data_to_widget(&mut self, cx: &mut Cx, nodes: &[LiveNode], path: &[LiveId]) {
        if let Some(value) = nodes.read_field_value(path) {
            if let Some(value) = value.as_bool() {
                self.animator_toggle(cx, value, Animate::Yes, id!(selected.on), id!(selected.off));
            }
        }
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
                
        match event.hits(cx, self.draw_check.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(_fe) => {
                if self.animator_in_state(cx, id!(selected.on)) {
                    self.animator_play(cx, id!(selected.off));
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, CheckBoxAction::Change(false));
                }
                else {
                    self.animator_play(cx, id!(selected.on));
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, CheckBoxAction::Change(true));
                }
            },
            Hit::FingerUp(_fe) => {
                                
            }
            Hit::FingerMove(_fe) => {
                                
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
    
    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }
    
    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
    }
}

impl CheckBoxRef {
    pub fn changed(&self, actions: &Actions) -> Option<bool> {
        if let CheckBoxAction::Change(b) = actions.find_widget_action_cast(self.widget_uid()) {
            return Some(b)
        }
        None
    }
    
    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            let s = inner.text.as_mut_empty();
            s.push_str(text);
        }
    }
    
    pub fn selected(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.animator_in_state(cx, id!(selected.on))
        }
        else {
            false
        }
    }
    
    pub fn set_selected(&self, cx: &mut Cx, value: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_toggle(cx, value, Animate::Yes, id!(selected.on), id!(selected.off));
        }
    }
}
