use makepad_render::*;
use crate::buttonlogic::*;

live_register!{
    use makepad_render::shader_std::*;
    
    Button: {{Button}} {
        bg: {
            instance color: vec4 = #333
            instance hover: float
            instance pressed: float

            const shadow: float = 3.0
            const border_radius: float = 2.5
            
            fn pixel(self) -> vec4 {
                let cx = Sdf2d::viewport(self.pos * self.rect_size);
                cx.box(
                    shadow,
                    shadow,
                    self.rect_size.x - shadow * (1. + self.pressed),
                    self.rect_size.y - shadow * (1. + self.pressed),
                    border_radius
                );
                cx.blur = 6.0;
                cx.fill(mix(#0007, #0, self.hover));
                cx.blur = 0.001;
                cx.box(
                    shadow,
                    shadow,
                    self.rect_size.x - shadow * 2.,
                    self.rect_size.y - shadow * 2.,
                    border_radius
                );
                return cx.fill(mix(mix(#3, #4, self.hover), #2a, self.pressed));
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
         
        state_default: {
            from: {all: Play::Forward {duration: 0.1}}
            bg: {pressed: 0.0, hover: 0.0}
            text: {color: #9}
        }
        
        state_hover: {
            from: {
                all: Play::Forward {duration: 0.1}
                state_down: Play::Forward {duration: 0.01}
            }
            bg: { 
                pressed: 0.0,
                hover: [{time: 0.0, value: 1.0}],
            } 
            text: {color: [{time: 0.0, value: #f}]}
        }
         
        state_pressed: {
            from: {all: Play::Forward {duration: 0.2}}
            bg: {
                pressed: [{time: 0.0, value: 1.0}],
                hover: 1.0,
            }
            text: {color: [{time: 0.0, value: #c}]}
        }
    }
}

#[derive(LiveComponent, LiveApply, LiveAnimate)]
pub struct Button {
    #[rust] pub button_logic: ButtonLogic,
    #[rust] pub animator: Animator,
    #[live] pub state_default: Option<LivePtr>,
    #[live] pub state_hover: Option<LivePtr>,
    #[live] pub state_pressed: Option<LivePtr>,
    #[live] pub bg: DrawQuad,
    #[live] pub text: DrawText,
    #[live] pub layout: Layout,
    #[live] pub label: String
}

impl LiveTraitCast for Button{
    fn to_frame_component(&mut self)->Option<&mut dyn FrameComponent>{
        return Some(self);
    }
}

impl FrameComponent for Button {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event)->OptionAnyAction{
        self.handle_event(cx, event).into()
    }
    
    fn draw_dyn(&mut self, cx: &mut Cx) {
        self.draw(cx, None);
    }
}

impl Button {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        self.handle_animation(cx, event);
        let res = self.button_logic.handle_event(cx, event, self.bg.draw_vars.area);
        match res.state {
            ButtonState::Pressed => self.animate_to(cx, self.state_pressed.unwrap()),
            ButtonState::Default => self.animate_to(cx, self.state_default.unwrap()),
            ButtonState::Hover => self.animate_to(cx, self.state_hover.unwrap()),
            _ => ()
        };
        res.action
    }
    
    pub fn draw(&mut self, cx: &mut Cx, label: Option<&str>) {
        self.bg.begin_quad(cx, self.layout);
        self.text.draw_text_walk(cx, label.unwrap_or(&self.label));
        self.bg.end_quad(cx);
    }
}
