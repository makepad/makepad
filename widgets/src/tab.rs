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
        margin: {right: 0., top: 5.}
        
        close_button: <TabCloseButton> {}

        draw_text: {
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            instance hover: 0.0
            instance active: 0.0

            uniform color: (THEME_COLOR_TEXT_INACTIVE)
            uniform color_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_active: (THEME_COLOR_TEXT_ACTIVE)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        self.color_active,
                        self.active
                    ),
                    self.color_hover,
                    self.hover
                )
            }
        }
        
        draw_bg: {
            instance hover: float
            instance active: float

            uniform border_size: 1.
            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform color_dither: 1.

            uniform color: (THEME_COLOR_D_HIDDEN)
            uniform color_hover: (THEME_COLOR_D_2)
            uniform color_active: (THEME_COLOR_BG_APP)

            uniform border_color_1: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_LIGHT)

            uniform border_color_2: (THEME_COLOR_D_HIDDEN)
            uniform border_color_2_hover: (THEME_COLOR_D_HIDDEN)
            uniform border_color_2_active: (THEME_COLOR_D_HIDDEN)

            uniform overlap_fix: 1.
              
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                sdf.box_y(
                    0.,
                    1.,
                    self.rect_size.x,
                    self.rect_size.y - self.overlap_fix,
                    self.border_radius,
                    0.5
                )

                sdf.fill_keep(mix(
                        mix(self.color, self.color_hover, self.hover),
                        self.color_active,
                        self.active
                    )
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                        self.active
                    ), self.border_size
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
                        draw_text: {hover: 0.0}
                    }
                }
                
                on = {
                    cursor: Hand,
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: [{time: 0.0, value: 1.0}]}
                        draw_text: {hover: [{time: 0.0, value: 1.0}]}
                    }
                }
            }
            
            active = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.3}}
                    apply: {
                        close_button: {draw_button: {active: 0.0}}
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                
                on = {
                    from: {all: Snap}
                    apply: {
                        close_button: {draw_button: {active: 1.0}}
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
    }

    pub TabFlat = <Tab> {
        margin: 0.

        draw_bg: {
            border_size: 1.
            border_radius: 0.5
            color_dither: 1.

            color: (THEME_COLOR_D_HIDDEN)
            color_hover: (THEME_COLOR_D_HIDDEN)
            color_active: (THEME_COLOR_FG_APP)

            border_color_1: (THEME_COLOR_U_HIDDEN)
            border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            border_color_1_active: (THEME_COLOR_U_HIDDEN)

            border_color_2: (THEME_COLOR_D_HIDDEN)
            border_color_2_hover: (THEME_COLOR_D_HIDDEN)
            border_color_2_active: (THEME_COLOR_D_HIDDEN)
            
            overlap_fix: 0.
        }
    }

    pub TabGradientX = <Tab> {
        draw_bg: {
            border_size: 1.
            border_radius: (THEME_CORNER_RADIUS)
            color_dither: 1.

            uniform color_1: (THEME_COLOR_D_HIDDEN)
            uniform color_1_hover: (THEME_COLOR_D_2)
            uniform color_1_active: (THEME_COLOR_BG_APP)

            uniform color_2: (THEME_COLOR_D_HIDDEN)
            uniform color_2_hover: (THEME_COLOR_D_2)
            uniform color_2_active: (THEME_COLOR_BG_APP)

            border_color_1: (THEME_COLOR_U_HIDDEN)
            border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            border_color_1_active: (THEME_COLOR_BEVEL_LIGHT)

            border_color_2: (THEME_COLOR_D_HIDDEN)
            border_color_2_hover: (THEME_COLOR_D_HIDDEN)
            border_color_2_active: (THEME_COLOR_D_HIDDEN)
              
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                sdf.box_y(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y - 1.,
                    self.border_radius,
                    0.5
                )

                sdf.fill_keep(mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.x + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x + dither),
                            self.hover
                        ),
                        mix(self.color_1_active, self.color_2_active, self.pos.x + dither),
                        self.active
                    )
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                        self.active
                    ), self.border_size
                )

                return sdf.result
            }
        }
    }

    pub TabGradientY = <TabGradientX> {
        draw_bg: {
            border_size: (THEME_BEVELING)
            border_radius: (THEME_CORNER_RADIUS)
            color_dither: 1.

            color_1: (THEME_COLOR_D_HIDDEN)
            color_1_hover: (THEME_COLOR_D_2)
            color_1_active: (THEME_COLOR_BG_APP)

            color_2: (THEME_COLOR_D_HIDDEN)
            color_2_hover: (THEME_COLOR_D_2)
            color_2_active: (THEME_COLOR_BG_APP)

            border_color_1: (THEME_COLOR_U_HIDDEN)
            border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            border_color_1_active: (THEME_COLOR_BEVEL_LIGHT)

            border_color_2: (THEME_COLOR_D_HIDDEN)
            border_color_2_hover: (THEME_COLOR_D_HIDDEN)
            border_color_2_active: (THEME_COLOR_D_HIDDEN)
              
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                sdf.box_y(
                    0.,
                    0.,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius,
                    0.5
                )

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                        self.active
                    )
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                        self.active
                    ), self.border_size
                )

                return sdf.result
            }
        }

    }
    
}

#[derive(Live, LiveHook, LiveRegister)]
pub struct Tab {
    #[rust] is_active: bool,
    #[rust] is_dragging: bool,
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_icon: DrawIcon,
    #[live] draw_text: DrawText2,
    #[live] icon_walk: Walk,
    //#[live] draw_drag: DrawColor,
    
    #[animator] animator: Animator,
    
    #[live] close_button: TabCloseButton,
    
    // height: f32,
    #[live] closeable: bool,
    #[live] hover: f32,
    #[live] active: f32,
    
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
    
    pub fn is_active(&self) -> bool {
        self.is_active
    }
    
    pub fn set_is_active(&mut self, cx: &mut Cx, is_active: bool, animate: Animate) {
        self.is_active = is_active;
        self.animator_toggle(cx, is_active, animate, id!(active.on), id!(active.off));
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, name: &str) {
        //self.bg_quad.color = self.color(self.is_active);
        self.draw_bg.begin(cx, self.walk, self.layout);
        //self.name_text.color = self.name_color(self.is_active);
        if self.closeable{
            self.close_button.draw(cx);
        }
        
        self.draw_icon.draw_walk(cx, self.icon_walk);
        //cx.turtle_align_y();
        self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), name);
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
            TabCloseButtonAction::WasPressed if self.closeable => dispatch_action(cx, TabAction::CloseWasPressed),
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
            Hit::FingerDown(fde) => {
                // A primary click/touch selects the tab, but a middle click closes it.
                if fde.is_primary_hit() {
                    dispatch_action(cx, TabAction::WasPressed);
                } else if self.closeable && fde.mouse_button().is_some_and(|b| b.is_middle()) {
                    dispatch_action(cx, TabAction::CloseWasPressed);
                }
            }
            _ => {}
        }
    }
}

