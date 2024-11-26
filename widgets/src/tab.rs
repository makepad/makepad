use {
    crate::{
        tab_close_button::{TabCloseButtonAction, TabCloseButton},
        makepad_draw::*,
    }
};

live_design!{
    link widgets;
    use link::theme::*;
    use link::widgets::*;
    use makepad_draw::shader::std::*;
    
    pub TabBase = {{Tab}} {}
    pub Tab = <TabBase> {
        width: Fit, height: Fill, //Fixed((THEME_TAB_HEIGHT)),
        
        align: {x: 0.0, y: 0.5}
        padding: <THEME_MSPACE_3> { }
        
        close_button: <TabCloseButton> {}
        draw_name: {
            text_style: <THEME_FONT_REGULAR> {}
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_INACTIVE,
                        THEME_COLOR_TEXT_SELECTED,
                        self.selected
                    ),
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                )
            }
        }
        
        draw_bg: {
            instance hover: float
            instance selected: float
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    -1.,
                    -1.,
                    self.rect_size.x + 2,
                    self.rect_size.y + 2,
                    1.
                )
                sdf.fill_keep(
                    mix(
                        THEME_COLOR_D_2 * 0.64,
                        THEME_COLOR_DOCK_TAB_SELECTED,
                        self.selected
                    )
                )
                return sdf.result
            }
        }
        
        animator: {
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
    
    pub TabMinimal = <TabBase> {
        width: Fit, height: Fill, //Fixed((THEME_TAB_HEIGHT)),
        align: {x: 0.0, y: 0.5}
        padding: <THEME_MSPACE_3> { }
        
        close_button: <TabCloseButton> {}
        draw_name: {
            text_style: <THEME_FONT_REGULAR> {}
            instance hover: 0.0
            instance selected: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_TEXT_INACTIVE,
                        THEME_COLOR_TEXT_SELECTED,
                        self.selected
                    ),
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                )
            }
        }
        
        draw_bg: {
            instance hover: float
            instance selected: float
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let marker_height = 2.5
                
                sdf.rect(0, self.rect_size.y - marker_height, self.rect_size.x, marker_height)
                sdf.fill(mix((THEME_COLOR_U_HIDDEN), (THEME_COLOR_DOCK_TAB_SELECTED_MINIMAL), self.selected));
                return sdf.result
            }
        }
        
        animator: {
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

#[derive(Live, LiveHook, LiveRegister)]
pub struct Tab {
    #[rust] is_selected: bool,
    #[rust] is_dragging: bool,
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_icon: DrawIcon,
    #[live] draw_name: DrawText,
    #[live] icon_walk: Walk,
    //#[live] draw_drag: DrawColor,
    
    #[animator] animator: Animator,
    
    #[live] close_button: TabCloseButton,
    
    // height: f32,
    #[live] closeable: bool,
    #[live] hover: f32,
    #[live] selected: f32,
    
    #[live(10.0)] min_drag_dist: f64,
    
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    
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
        self.animator_toggle(cx, is_selected, animate, id!(selected.on), id!(selected.off));
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, name: &str) {
        //self.bg_quad.color = self.color(self.is_selected);
        self.draw_bg.begin(cx, self.walk, self.layout);
        //self.name_text.color = self.name_color(self.is_selected);
        if self.closeable{
            self.close_button.draw(cx);
        }
        
        self.draw_icon.draw_walk(cx, self.icon_walk);
        //cx.turtle_align_y();
        self.draw_name.draw_walk(cx, Walk::fit(), Align::default(), name);
        //cx.turtle_align_y();
        self.draw_bg.end(cx);
        
        //if self.is_dragged {
        //    self.draw_drag.draw_abs(cx, self.draw_bg.area().get_clipped_rect(cx));
        //}
    }
    
    pub fn area(&self) -> Area {
        self.draw_bg.area()
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabAction),
    ) {
        self.animator_handle_event(cx, event);
        
        let mut block_hover_out = false;
        match self.close_button.handle_event(cx, event) {
            TabCloseButtonAction::WasPressed => dispatch_action(cx, TabAction::CloseWasPressed),
            TabCloseButtonAction::HoverIn => block_hover_out = true,
            TabCloseButtonAction::HoverOut => self.animator_play(cx, id!(hover.off)),
            _ => ()
        };
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => if !block_hover_out {
                self.animator_play(cx, id!(hover.off));
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
    }
}

