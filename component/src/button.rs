use {
    crate::{
        makepad_derive_frame::*,
        makepad_platform::*,
        button_logic::*,
        frame_traits::*,
    }
};

live_register!{
    import makepad_platform::shader::std::*;
    
    DrawLabelText: {{DrawLabelText}} {
        text_style: {
            font: {
                //path: d"resources/IBMPlexSans-SemiBold.ttf"
            }
            font_size: 11.0
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
        bg: {
            instance hover: 0.0
            instance pressed: 0.0
            
            const BORDER_RADIUS: 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.0;
                let body = mix(mix(#53, #5c, self.hover), #33, self.pressed);
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(body_transp, mix(#6d, #1f, self.pressed), max(0.0, grad_top - sdf.pos.y) / grad_top);
                let bot_gradient = mix(
                    mix(body_transp, #5c, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );
                
                // the little drop shadow at the bottom
                let shift_inward = BORDER_RADIUS + 4.0;
                sdf.move_to(shift_inward, self.rect_size.y - BORDER_RADIUS);
                sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y - BORDER_RADIUS);
                sdf.stroke(
                    mix(mix(#2f, #1f, self.hover), #0000, self.pressed),
                    BORDER_RADIUS
                )
                
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    BORDER_RADIUS
                )
                sdf.fill_keep(body)
                
                sdf.stroke(
                    bot_gradient,
                    1.0
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
            padding: {left: 14.0, top: 10.0, right: 14.0, bottom: 10.0}
        }
        
        state: {
            hover = {
                default: off,
                off = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        bg: {pressed: 0.0, hover: 0.0}
                        label: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    from: {
                        all: Play::Forward {duration: 0.1}
                        pressed: Play::Forward {duration: 0.01}
                    }
                    apply: {
                        bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        label: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }
                
                pressed = {
                    from: {all: Play::Forward {duration: 0.2}}
                    apply: {
                        bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        label: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, FrameComponent)]
#[live_register(frame_component!(Button))]
pub struct Button {
    state: State,
    
    bg: DrawQuad,
    label: DrawLabelText,
    
    #[alias(width, walk.width)]
    #[alias(height, walk.height)]
    #[alias(margin, walk.margin)]
    walk: Walk,
    
    layout: Layout,
    text: String
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawLabelText {
    draw_super: DrawText,
    hover: f32,
    pressed: f32,
}

impl Button {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)) {
        self.state_handle_event(cx, event);
        let state = button_logic_handle_event(cx, event, self.bg.area(), dispatch_action);
        if let Some(state) = state {
            match state {
                ButtonState::Pressed => self.animate_state(cx, ids!(hover.pressed)),
                ButtonState::Default => self.animate_state(cx, ids!(hover.off)),
                ButtonState::Hover => self.animate_state(cx, ids!(hover.on)),
            }
        };
    }
    
    pub fn draw_label(&mut self, cx: &mut Cx2d, label: &str) {
        self.bg.begin(cx, self.walk, self.layout);
        self.label.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.bg.end(cx);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.bg.begin(cx, walk, self.layout);
        self.label.draw_walk(cx, Walk::fit(), Align::default(), &self.text);
        self.bg.end(cx);
    }
}
