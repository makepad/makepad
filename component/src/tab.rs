use {
    crate::{
        tab_close_button::{TabCloseButtonAction, TabCloseButton},
        makepad_platform::*,
    }
};

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::theme::*;
    
    Tab: {{Tab}} {
        name_text: {
            text_style: FONT_LABEL {}
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        COLOR_TEXT_DEFAULT,
                        COLOR_TEXT_SELECTED,
                        self.selected
                    ),
                    COLOR_TEXT_HOVER,
                    self.hover
                )
            }
        }
        
        bg_quad: {
            instance hover: float
            instance selected: float
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return mix(
                    mix(
                        COLOR_BG_HEADER,
                        COLOR_BG_EDITOR,
                        self.selected
                    ),
                    #f,
                    0.0 //mix(self.hover * 0.05, self.hover * -0.025, self.selected)
                );
                /*sdf.clear(color)
                sdf.move_to(0.0, 0.0)
                sdf.line_to(0.0, self.rect_size.y)
                sdf.move_to(self.rect_size.x, 0.0)
                sdf.line_to(self.rect_size.x, self.rect_size.y)
                return sdf.stroke(BORDER_COLOR, BORDER_WIDTH)*/
            }
        }
        walk:{
            width: Size::Fit,
            height: Size::Fill, //Fixed((DIM_TAB_HEIGHT)),
        }
        
        layout: {
            align: {x: 0.0, y: 0.5},
            padding: {
                left: 10.0,
                top: 2.0,
                right: 15.0,
                bottom: 0.0,
            },
        }
        
        state:{
            hover = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.2}}
                    apply: {
                        hover: 0.0,
                        bg_quad: {hover: (hover)}
                        name_text: {hover: (hover)}
                    }
                }
                
                on = {
                    from: {all: Play::Forward {duration: 0.1}}
                    apply: {
                        hover: [{time: 0.0, value: 1.0}],
                    }
                }
            }
            
            selected = {
                default: off
                off = {
                    from: {all: Play::Forward {duration: 0.3}}
                    apply: {
                        selected: 0.0,
                        close_button: {button_quad: {selected: (selected)}}
                        bg_quad: {selected: (selected)}
                        name_text: {selected: (selected)}
                    }
                }
                
                on = {
                    from: {all: Play::Snap}
                    apply: {
                        selected: 1.0,
                    }
                }
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
    
    state: State,
    
    close_button: TabCloseButton,
    
    // height: f32,
    
    hover: f32,
    selected: f32,

    walk: Walk, 
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
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, animate: Animate) {
        self.is_selected = is_selected;
        self.toggle_state(cx, is_selected, animate, ids!(selected.on), ids!(selected.off));
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, name: &str) {
        //self.bg_quad.color = self.color(self.is_selected);
        self.bg_quad.begin(cx, self.walk, self.layout);
        //self.name_text.color = self.name_color(self.is_selected);
        self.close_button.draw(cx);
        //cx.turtle_align_y();
        self.name_text.draw_walk(cx, Walk::default(), name);
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
        self.state_handle_event(cx, event);
        
        let mut block_hover_out = false;
        match self.close_button.handle_event(cx, event) {
            TabCloseButtonAction::WasPressed => dispatch_action(cx, TabAction::CloseWasPressed),
            TabCloseButtonAction::HoverIn => block_hover_out = true,
            TabCloseButtonAction::HoverOut => self.animate_state(cx, ids!(hover.off)),
            _ => ()
        };
        
        match event.hits(cx, self.bg_quad.draw_vars.area) {
            HitEvent::FingerHover(f) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match f.hover_state {
                    HoverState::In => {
                        self.animate_state(cx, ids!(hover.on));
                    }
                    HoverState::Out => if !block_hover_out {
                        self.animate_state(cx, ids!(hover.off));
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
                    self.bg_quad.draw_vars.redraw(cx);
                    *f.action = DragAction::Copy;
                }
                DragState::Out => {
                    self.is_dragged = false;
                    self.bg_quad.draw_vars.redraw(cx);
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
                self.bg_quad.draw_vars.redraw(cx);
                dispatch_action(cx, TabAction::ReceivedDraggedItem(f.dragged_item.clone()))
            }
            _ => {}
        }
    }
}

