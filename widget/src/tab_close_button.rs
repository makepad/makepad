use makepad_render::*;

live_register!{
    use makepad_render::shader::std::*;
    
    TabCloseButton: {{TabCloseButton}} {
        button_quad: {
            
            instance hover: float;
            instance selected: float;
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let mid = self.rect_size / 2.0;
                let size = (self.hover * 0.5 + 0.5) * 0.5 * length(self.rect_size) / 2.0;
                let min = mid - vec2(size);
                let max = mid + vec2(size);
                sdf.move_to(min.x, min.y);
                sdf.line_to(max.x, max.y);
                sdf.move_to(min.x, max.y);
                sdf.line_to(max.x, min.y);
                return sdf.stroke(vec4(1.0) * (0.5 * self.hover + 0.5), 1.0);
            }
        }
        
        default_state: {
            from: {all: Play::Forward {duration: 0.2}}
            apply: {
                button_quad: {hover: 0.0}
            }
        }
        
        hover_state: {
            from: {all: Play::Forward {duration: 0.1}}
            apply: {
                button_quad: {hover: [{time: 0.0, value: 1.0}]},
            }
        }
        
        walk: {
            height: Height::Fixed(10.0),
            width: Width::Fixed(10.0),
            margin: Margin {
                l: 0.0,
                t: 0.0,
                r: 5.0,
                b: 0.0,
            },
        },
    }
}

#[derive(Live, LiveHook)]
pub struct TabCloseButton {
    button_quad: DrawQuad,
    #[default_state(default_state)]
    animator: Animator,
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    walk: Walk
}

impl TabCloseButton {
    
    pub fn draw(&mut self, cx: &mut Cx) {
        self.button_quad.draw_walk(
            cx,
            self.walk
        );
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
    ) -> TabCloseButtonAction {
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.button_quad.draw_vars.area, HitOpt::default()) {
            Event::FingerHover(event) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match event.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, self.hover_state.unwrap());
                        return TabCloseButtonAction::HoverIn;
                    }
                    HoverState::Out => {
                        self.animate_to(cx, self.default_state.unwrap());
                        return TabCloseButtonAction::HoverOut;
                    }
                    _ => {}
                }
            }
            Event::FingerDown(_) => return TabCloseButtonAction::WasPressed,
            _ => {}
        }
        TabCloseButtonAction::None
    }
}

pub enum TabCloseButtonAction {
    None,
    WasPressed,
    HoverIn,
    HoverOut,
}
