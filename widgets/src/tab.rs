use {
    crate::{
        tab_close_button::{TabCloseButtonAction, TabCloseButton},
        makepad_draw_2d::*,
    }
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    Tab= {{Tab}} {
        name: {
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
        
        bg: {
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
        
        state:{
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        hover: 0.0,
                        bg: {hover: (hover)}
                        name: {hover: (hover)}
                    }
                }
                
                on = {
                    cursor: Hand,
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        hover: [{time: 0.0, value: 1.0}],
                    }
                }
            }
            
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.3}}
                    apply: {
                        selected: 0.0,
                        close_button: {button: {selected: (selected)}}
                        bg: {selected: (selected)}
                        name: {selected: (selected)}
                    }
                }
                
                on = {
                    from: {all: Snap}
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
    
    bg: DrawQuad,
    name: DrawText,
    drag: DrawColor,
    
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
        self.toggle_state(cx, is_selected, animate, id!(selected.on), id!(selected.off));
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, name: &str) {
        //self.bg_quad.color = self.color(self.is_selected);
        self.bg.begin(cx, self.walk, self.layout);
        //self.name_text.color = self.name_color(self.is_selected);
        self.close_button.draw(cx);
        //cx.turtle_align_y();
        self.name.draw_walk(cx, Walk::fit(), Align::default(),  name);
        //cx.turtle_align_y();
        self.bg.end(cx);
        
        if self.is_dragged {
            self.drag.draw_abs(cx, self.bg.area().get_clipped_rect(cx));
        }
    }
    
    
    pub fn handle_event_fn(
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
        
        match event.hits(cx, self.bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => if !block_hover_out {
                self.animate_state(cx, id!(hover.off));
            }
            Hit::FingerDown(_) => {
                dispatch_action(cx, TabAction::WasPressed);
            }
            _ => {}
        }
        match event.drag_hits(cx, self.bg.area()) {
            DragHit::Drag(f) => match f.state {
                DragState::In => {
                    self.is_dragged = true;
                    self.bg.redraw(cx);
                    f.action.set(DragAction::Copy);
                }
                DragState::Out => {
                    self.is_dragged = false;
                    self.bg.redraw(cx);
                }
                DragState::Over => match event {
                    Event::Drag(event) => {
                        event.action.set(DragAction::Copy);
                    }
                    _ => panic!(),
                },
            },
            DragHit::Drop(f) => {
                self.is_dragged = false;
                self.bg.area().redraw(cx);
                dispatch_action(cx, TabAction::ReceivedDraggedItem(f.dragged_item.clone()))
            }
            _ => {}
        }
    }
}

