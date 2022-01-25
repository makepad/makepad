use {
    makepad_platform::*,
};

live_register!{
    use makepad_platform::shader::std::*;
    
    FrameTemplate: {{FrameTemplate}} {
        bg_quad: {
            instance hover: float
            instance selected: float
            fn pixel(self) -> vec4 {
            }
        }
        
        value: 40.0
        
        layout: Layout {
            align: Align {fx: 0.0, fy: 0.5},
            walk: Walk {
                width: Width::Computed,
                height: Height::Fixed(40.0),
            },
            padding: Padding {
                l: 10.0,
                t: 2.0,
                r: 15.0,
                b: 0.0,
            },
        }
        
        default_state: {
            from: {all: Play::Forward {duration: 0.2}}
            apply: {
                hover: 0.0,
                bg_quad: {hover: (hover)}
            }
        }
        
        hover_state: {
            from: {all: Play::Forward {duration: 0.1}}
            apply: {
                hover: [{time: 0.0, value: 1.0}],
            }
        }
        
        unselected_state: {
            track: select,
            from: {all: Play::Forward {duration: 0.3}}
            apply: {
                selected: 0.0,
                bg_quad: {selected: (selected)}
            }
        }
        
        selected_state: {
            track: select,
            from: {all: Play::Forward {duration: 0.1}}
            apply: {
                selected: [{time: 0.0, value: 1.0}],
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct FrameTemplate {
    bg_quad: DrawQuad,
    
    #[state(default_state, unselected_state)]
    animator: Animator,
    
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    selected_state: Option<LivePtr>,
    unselected_state: Option<LivePtr>,
    
    layout: Layout,
}

pub enum FrameTemplateAction {
    WasPressed,
    CloseWasPressed,
}

impl FrameTemplate {
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.bg_quad.begin(cx, self.layout);
        self.bg_quad.end(cx);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FrameTemplateAction),
    ) {
        self.animator_handle_event(cx, event);
        
        match event.hits(cx, self.bg_quad.draw_vars.area) {
            HitEvent::FingerHover(f) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match f.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, self.hover_state);
                    }
                    HoverState::Out =>  {
                        self.animate_to(cx, self.default_state);
                    }
                    _ => {}
                }
            }
            HitEvent::FingerDown(_) => {
                dispatch_action(cx, FrameTemplateAction::WasPressed);
            }
            _ => {}
        }
    }
}


