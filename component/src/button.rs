#![allow(unused)]
use {
    crate::{
        makepad_platform::*,
        button_logic::*,
        frame_component::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    
    DrawLabelText: {{DrawLabelText}} {
        text_style: {
            font: {
                path: "resources/IBMPlexSans-SemiBold.ttf"
            }
            font_size:11.0
        }
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
            
            const BORDER_RADIUS: 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.0;
                let body = mix(mix(#53, #5c, self.hover), #33, self.pressed);
                //return mix(#ff,#f00,max(0.0,self.rect_size.y - grad_bot - sdf.pos.y)/grad_bot);
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x-2.0,
                    self.rect_size.y-2.0,
                    BORDER_RADIUS
                )
                sdf.fill_keep(body)
                let top_gradient = mix(body,mix(#6d,#1f,self.pressed),max(0.0,grad_top-sdf.pos.y)/grad_top);
                let bot_gradient = mix(mix(#2f,#53,self.pressed),top_gradient,clamp(0.0,1.0,self.rect_size.y - grad_bot - sdf.pos.y)/grad_bot);
                
                sdf.stroke(
                    bot_gradient,
                    1.2
                )
                return sdf.result
            }
        }
        
        walk: {
            width: Size::Fit,
            height: Size::Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: {left: 16.0, top: 10.0, right: 16.0, bottom: 10.0}
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
#[live_register(register_as_frame_component!(Button))]
pub struct Button {
    #[rust] pub button_logic: ButtonLogic,
    #[state(default_state)] pub animator: Animator,
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    pressed_state: Option<LivePtr>,
    bg_quad: DrawQuad,
    label_text: DrawLabelText,
    walk: Walk,
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
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        //cx.debug_draw_tree(true, 0);
    }
}

impl FrameComponent for Button {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, self_id:LiveId) -> FrameComponentActionRef {
        self.handle_event(cx, event).into()
    }

    fn get_walk(&self)->Walk{
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk:Walk)->Result<LiveId,()>{
        self.draw_walk(cx, walk);
        Err(())
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

    pub fn draw_label(&mut self, cx: &mut Cx2d, label: &str) {
        self.bg_quad.begin(cx, self.walk, self.layout);
        self.label_text.draw_walk(cx, label);
        self.bg_quad.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk:Walk) {
        self.bg_quad.begin(cx, walk, self.layout);
        self.label_text.draw_walk(cx, &self.label);
        self.bg_quad.end(cx);
    }
}
