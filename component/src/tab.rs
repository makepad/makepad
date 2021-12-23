use {
    crate::tab_close_button::{TabCloseButtonAction, TabCloseButton},
    makepad_render::*,
};

live_register!{
    use makepad_render::shader::std::*;
    
    Tab: {{Tab}} {
        name_text: {
            instance hover: float
            instance selected: float
            fn get_color(self) -> vec4 {
                return mix(#82, #ff, self.selected)
            }
        }
        
        bg_quad: {
            const border_width: float = 1.0
            const border_color: vec4 = #28
            
            instance hover: float
            instance selected: float
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let color = mix(mix(#34, #28, self.selected), #f, mix(self.hover * 0.05, self.hover * -0.025, self.selected));
                sdf.clear(color)
                sdf.move_to(0.0, 0.0)
                sdf.line_to(0.0, self.rect_size.y)
                sdf.move_to(self.rect_size.x, 0.0)
                sdf.line_to(self.rect_size.x, self.rect_size.y)
                return sdf.stroke(border_color, border_width)
            }
        }
        
        height: 40.0
        
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
                name_text: {hover: (hover)}
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
                close_button: {button_quad: {selected: (selected)}}
                bg_quad: {selected: (selected)}
                name_text: {selected: (selected)}
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
pub struct Tab {
    #[rust] is_selected: bool,
    #[rust] is_dragged: bool,
    
    bg_quad: DrawQuad,
    name_text: DrawText,
    drag_quad: DrawColor,
    
    #[default_state(default_state, unselected_state)]
    animator: Animator,
    
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    selected_state: Option<LivePtr>,
    unselected_state: Option<LivePtr>,
    
    close_button: TabCloseButton,
    
    height: f32,
    
    hover: f32,
    selected: f32,
    
    layout: Layout,
}

pub enum TabAction {
    WasPressed,
    CloseWasPressed,
    ReceivedDraggedItem(DraggedItem),
}

impl Tab {
    
    pub fn is_selected(&self) -> bool {
        self.is_selected
    }
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, should_animate: bool) {
        self.is_selected = is_selected;
        self.toggle_animator(
            cx,
            is_selected,
            should_animate,
            self.selected_state.unwrap(),
            self.unselected_state.unwrap()
        );
    }
    
    pub fn draw(&mut self, cx: &mut Cx, name: &str) {
        //self.bg_quad.color = self.color(self.is_selected);
        self.bg_quad.begin(cx, self.layout);
        //self.name_text.color = self.name_color(self.is_selected);
        self.close_button.draw(cx);
        //cx.turtle_align_y();
        self.name_text.draw_walk(cx, name);
        //cx.turtle_align_y();
        self.bg_quad.end(cx);
        
        if self.is_dragged {
            self.drag_quad.draw_abs(cx, self.bg_quad.draw_vars.area.get_rect(cx));
        }
    }
    
    /*
    fn color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.color_selected
        } else {
            self.color
        }
    }
    
    fn name_color(&self, is_selected: bool) -> Vec4 {
        if is_selected {
            self.name_color_selected
        } else {
            self.name_color
        }
    }*/
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabAction),
    ) {
        self.animator_handle_event(cx, event);
        
        let mut block_hover_out = false;
        match self.close_button.handle_event(cx, event) {
            TabCloseButtonAction::WasPressed => dispatch_action(cx, TabAction::CloseWasPressed),
            TabCloseButtonAction::HoverIn => block_hover_out = true,
            TabCloseButtonAction::HoverOut => self.animate_to(cx, self.default_state.unwrap()),
            _ => ()
        };
        
        match event.hits(cx, self.bg_quad.draw_vars.area) {
            HitEvent::FingerHover(f) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match f.hover_state {
                    HoverState::In => {
                        self.animate_to(cx, self.hover_state.unwrap());
                    }
                    HoverState::Out => if !block_hover_out {
                        self.animate_to(cx, self.default_state.unwrap());
                    }
                    _ => {}
                }
            }
            HitEvent::FingerDown(_) => {
                dispatch_action(cx, TabAction::WasPressed);
            }
            _ => {}
        }
        match event.drag_hits(cx, self.bg_quad.draw_vars.area) {
            DragEvent::FingerDrag(f) => match f.state {
                DragState::In => {
                    self.is_dragged = true;
                    self.bg_quad.draw_vars.redraw_view(cx);
                    *f.action = DragAction::Copy;
                }
                DragState::Out => {
                    self.is_dragged = false;
                    self.bg_quad.draw_vars.redraw_view(cx);
                }
                DragState::Over => match event {
                    Event::FingerDrag(event) => {
                        event.action = DragAction::Copy;
                    }
                    _ => panic!(),
                },
            },
            DragEvent::FingerDrop(f) => {
                self.is_dragged = false;
                self.bg_quad.draw_vars.redraw_view(cx);
                dispatch_action(cx, TabAction::ReceivedDraggedItem(f.dragged_item.clone()))
            }
            _ => {}
        }
    }
}

