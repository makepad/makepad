#![allow(unused)]
use {
    crate::{
        makepad_platform::*,
        button_logic::*,
        frame_registry::*,
        register_as_frame_component
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    DrawLabelText: {{DrawLabelText}} {
        fn get_color(self) -> vec4 {
            return mix(
                mix(
                    #9,
                    #f,
                    self.hover
                ),
                #9,
                self.pressed
            )
        }
    }
    
    Button: {{Button}} {
        bg_quad: {
            instance hover: 0.0
            instance pressed: 0.0
            
            const SHADOW: 3.0
            const BORDER_RADIUS: 2.5
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    SHADOW,
                    SHADOW,
                    self.rect_size.x - SHADOW * (1. + self.pressed),
                    self.rect_size.y - SHADOW * (1. + self.pressed),
                    BORDER_RADIUS
                );
                sdf.blur = 6.0;
                sdf.fill(mix(#0007, #0, self.hover));
                sdf.blur = 0.001;
                sdf.box(
                    SHADOW,
                    SHADOW,
                    self.rect_size.x - SHADOW * 2.,
                    self.rect_size.y - SHADOW * 2.,
                    BORDER_RADIUS
                );
                return sdf.fill(mix(mix(#3, #4, self.hover), #2a, self.pressed));
            }
        }
        
        
        layout: {
            align: {fx: 0.5, fy: 0.5},
            walk: {
                width: Width::Computed,
                height: Height::Computed,
                margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
            }
            padding: {left: 16.0, top: 12.0, right: 16.0, bottom: 12.0}
        }
        
        default_state: {
            duration: 0.1,
            apply: {
                bg_quad: {pressed: 0.0, hover: 0.0}
                label_text: {pressed: 0.0, hover: 0.0}
            }
        }
        
        hover_state: {
            from: {
                all: Play::Forward {duration: 0.1}
                pressed_state: Play::Forward {duration: 0.01}
            }
            apply: {
                bg_quad: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                label_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
            }
        }
        
        pressed_state: {
            duration: 0.2,
            apply: {
                bg_quad: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                label_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
            }
        }
    }
}

#[derive(Live)]
#[live_register( | cx: &mut Cx | {
    register_as_frame_component!(cx, Button);
})]
pub struct Button {
    #[rust] pub button_logic: ButtonLogic,
    #[state(default_state)] pub animator: Animator,
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    pressed_state: Option<LivePtr>,
    bg_quad: DrawQuad,
    label_text: DrawLabelText,
    layout: Layout,
    label: String
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawLabelText {
    deref_target: DrawText,
    hover: f32,
    pressed: f32,
}

impl LiveHook for Button {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        //cx.debug_draw_tree(true, 0);
    }
}

impl FrameComponent for Button {
    fn type_id(&self) -> LiveType {LiveType::of::<Self>()}
    
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> OptionFrameComponentAction {
        self.handle_event(cx, event).into()
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d) {
        self.draw(cx, None);
    }
}

impl Button {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        
        self.animator_handle_event(cx, event);
        let res = self.button_logic.handle_event(cx, event, self.bg_quad.draw_vars.area);
        
        match res.state {
            ButtonState::Pressed => self.animate_to(cx, self.pressed_state),
            ButtonState::Default => self.animate_to(cx, self.default_state),
            ButtonState::Hover => self.animate_to(cx, self.hover_state),
            _ => ()
        };
        res.action
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, label: Option<&str>) {
        self.bg_quad.begin(cx, self.layout);
        self.label_text.draw_walk(cx, label.unwrap_or(&self.label));
        self.bg_quad.end(cx);
    }
}
