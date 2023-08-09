use {
    crate::{
        tab_close_button::{TabCloseButtonAction, TabCloseButton},
        makepad_draw::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    Tab = {{Tab}} {
        draw_name: {
            text_style: <FONT_LABEL> {}
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
        
        draw_bg: {
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
        walk: {
            width: Fit,
            height: Fill, //Fixed((DIM_TAB_HEIGHT)),
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
        
        state: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_name: {hover: 0.0}
                    }
                }
                
                on = {
                    cursor: Hand,
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: [{time: 0.0, value: 1.0}]}
                        draw_name: {hover: [{time: 0.0, value: 1.0}]}
                    }
                }
            }
            
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.3}}
                    apply: {
                        close_button: {draw_button: {selected: 0.0}}
                        draw_bg: {selected: 0.0}
                        draw_name: {selected: 0.0}
                    }
                }
                
                on = {
                    from: {all: Snap}
                    apply: {
                        close_button: {draw_button: {selected: 1.0}}
                        draw_bg: {selected: 1.0}
                        draw_name: {selected: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct Tab {
    #[rust] is_selected: bool,
    #[rust] is_dragging: bool,
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_name: DrawText,
    //#[live] draw_drag: DrawColor,
    
    #[state] state: LiveState,
    
    #[live] close_button: TabCloseButton,
    
    // height: f32,
    
    #[live] hover: f32,
    #[live] selected: f32,
    
    #[live(10.0)] min_drag_dist: f64,
    
    #[live] walk: Walk,
    #[live] layout: Layout,
    
}

pub enum TabAction {
    WasPressed,
    CloseWasPressed,
    ShouldTabStartDrag,
    ShouldTabStopDrag
    //DragHit(DragHit)
}

impl Tab {
    
    pub fn is_selected(&self) -> bool {
        self.is_selected
    }
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, animate: Animate) {
        self.is_selected = is_selected;
        self.toggle_state(cx, is_selected, animate, id!(selected.on), id!(selected.off));
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, name: &str) {
        //self.bg_quad.color = self.color(self.is_selected);
        self.draw_bg.begin(cx, self.walk, self.layout);
        //self.name_text.color = self.name_color(self.is_selected);
        self.close_button.draw(cx);
        //cx.turtle_align_y();
        self.draw_name.draw_walk(cx, Walk::fit(), Align::default(), name);
        //cx.turtle_align_y();
        self.draw_bg.end(cx);
        
        //if self.is_dragged {
        //    self.draw_drag.draw_abs(cx, self.draw_bg.area().get_clipped_rect(cx));
        //}
    }
    
    pub fn area(&self)->Area{
        self.draw_bg.area()
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabAction),
    ) {
        self.state_handle_event(cx, event);
        
        let mut block_hover_out = false;
        match self.close_button.handle_event(cx, event) {
            TabCloseButtonAction::WasPressed => dispatch_action(cx, TabAction::CloseWasPressed),
            TabCloseButtonAction::HoverIn => block_hover_out = true,
            TabCloseButtonAction::HoverOut => self.animate_state(cx, id!(hover.off)),
            _ => ()
        };
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => if !block_hover_out {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerMove(e) => {
                if !self.is_dragging && (e.abs - e.abs_start).length() > self.min_drag_dist {
                    self.is_dragging = true;
                    dispatch_action(cx, TabAction::ShouldTabStartDrag);
                }
            }
            Hit::FingerUp(_) => {
                if self.is_dragging {
                    dispatch_action(cx, TabAction::ShouldTabStopDrag);
                    self.is_dragging = false;
                }
            }
            Hit::FingerDown(_) => {
                dispatch_action(cx, TabAction::WasPressed);
            }
            _ => {}
        }
        /*
        match event.drag_hits(cx, self.draw_bg.area()) {
            DragHit::NoHit => (),
            hit => dispatch_action(cx, TabAction::DragHit(hit))
            /*
            DragHit::Drag(f) => match f.state {
                DragState::In => {
                    log!("DRAGSTATE IN");
                    //self.is_dragged = true;
                    //self.draw_bg.redraw(cx);
                    //f.response.set(DragResponse::Copy);
                }
                DragState::Out => {
                    //self.is_dragged = false;
                    //self.draw_bg.redraw(cx);
                }
                DragState::Over => {
                    //Event::Drag(event) => {
                    //    event.response.set(DragResponse::Copy);
                    //}
                    //_ => panic!(),
                },
            },
            DragHit::Drop(_f) => {
                //self.is_dragged = false;
                //self.draw_bg.area().redraw(cx);
                //dispatch_action(cx, TabAction::ReceivedDraggedItem(f.dragged_item.clone()))
            }
            _ => {}*/
        }*/
    }
}

