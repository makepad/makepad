#![allow(unused)]
use makepad_render::*;
use crate::button_logic::*;


live_register!{
    use makepad_render::shader_std::*;
    
    Button: {{Button}} {
        bg_quad: {
            instance color: vec4 = #333
            instance hover: float
            instance pressed: float
            
            const shadow: float = 3.0
            const border_radius: float = 2.5
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    shadow,
                    shadow,
                    self.rect_size.x - shadow * (1. + self.pressed),
                    self.rect_size.y - shadow * (1. + self.pressed),
                    border_radius
                );
                sdf.blur = 6.0;
                sdf.fill(mix(#0007, #0, self.hover));
                sdf.blur = 0.001;
                sdf.box(
                    shadow,
                    shadow,
                    self.rect_size.x - shadow * 2.,
                    self.rect_size.y - shadow * 2.,
                    border_radius
                );
                return sdf.fill(mix(mix(#3, #4, self.hover), #2a, self.pressed));
            }
        }
        
        layout: Layout {
            align: Align {fx: 0.5, fy: 0.5},
            walk: Walk {
                width: Width::Computed,
                height: Height::Computed,
                margin: Margin {l: 1.0, r: 1.0, t: 1.0, b: 1.0},
            }
            padding: Padding {l: 16.0, t: 12.0, r: 16.0, b: 12.0}
        }
        
        default_state: {
            from: {all: Play::Forward {duration: 0.1}}
            apply:{
                bg_quad: {pressed: 0.0, hover: 0.0}
                label_text: {color: #9}
            }
        }
        
        hover_state: {
            from: {
                all: Play::Forward {duration: 0.1}
                pressed_state: Play::Forward {duration: 0.01}
            }
            apply: {
                bg_quad: {
                    pressed: 0.0,
                    hover: [{time: 0.0, value: 1.0}],
                }
                label_text: {color: [{time: 0.0, value: #f}]}
            }
        }
        
        pressed_state: {
            from: {all: Play::Forward {duration: 0.2}}
            apply: {
                bg_quad: {
                    pressed: [{time: 0.0, value: 1.0}],
                    hover: 1.0,
                }
                label_text: {color: [{time: 0.0, value: #c}]}
            }
        }
    }
}

#[derive(Live)]
pub struct Button {
    
    #[rust] pub button_logic: ButtonLogic,
    #[default_state(default_state)] pub animator: Animator,
    
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    pressed_state: Option<LivePtr>,
    bg_quad: DrawQuad,
    label_text: DrawText,
    layout: Layout,
    label: String
}

impl LiveHook for Button {
    fn to_frame_component(&mut self) -> Option<&mut dyn FrameComponent> {
        return Some(self);
    }
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if apply_from.is_from_doc() {
            /*
            self.animator2.cut_to_live(cx, id!(hover), self.state_default.unwrap());
            self.animator2.cut_to_live(cx, id!(label), self.state_default_label.unwrap());
            //self.animator2.cut_to_live(cx, id!(hover), self.state_default.unwrap());
            self.animator2.animate_to_live(cx, id!(hover), self.state_hover.unwrap());
            self.animator2.animate_to_live(cx, id!(label), self.state_hover_label.unwrap());
            println!("{}", self.animator2.state.as_ref().unwrap().to_string(0,100));
            */
        }
    }
}

impl FrameComponent for Button {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event) -> OptionAnyAction {
        self.handle_event(cx, event).into()
    }
    
    fn draw_dyn(&mut self, cx: &mut Cx) {
        self.draw(cx, None);
    }
}
/*
impl Button{
    fn animate_to2(&mut self, cx: &mut Cx, track:LiveId, state: LivePtr) {
        if self.animator2.state.is_none() {
            self.animator2.cut_to_live(cx, track, self.state_default.unwrap());
         }
        self.animator2.animate_to_live(cx, track, state);
        //println!("{}", self.animator2.state.as_ref().unwrap().to_string(0,100));
    }
    fn handle_animation2(&mut self, cx: &mut Cx, event: &mut Event) {
        if self.animator2.do_animation(cx, event) {
            let state = self.animator2.swap_out_state();
            self.apply(cx, ApplyFrom::Animate, state.child_by_name(0,id!(state)).unwrap(), &state);
            self.animator2.swap_in_state(state);
        }
    }
}    */

impl Button {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        
        self.animator_handle_event(cx, event);
        let res = self.button_logic.handle_event(cx, event, self.bg_quad.draw_vars.area);
        
        match res.state {
            ButtonState::Pressed => self.animate_to(cx, self.pressed_state.unwrap()),
            ButtonState::Default => self.animate_to(cx, self.default_state.unwrap()),
            ButtonState::Hover => self.animate_to(cx, self.hover_state.unwrap()),
            _ => ()
        };
        res.action
    }
    
    pub fn draw(&mut self, cx: &mut Cx, label: Option<&str>) {
        self.bg_quad.begin(cx, self.layout);
        self.label_text.draw_walk(cx, label.unwrap_or(&self.label));
        self.bg_quad.end(cx);
    }
}
