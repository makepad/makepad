use {
    crate::{
        makepad_draw::*,
    },
};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub PopupMenuItemBase = {{PopupMenuItem}} {}
    pub PopupMenuBase = {{PopupMenu}} {}
        
    pub PopupMenuItem = <PopupMenuItemBase> {
        width: Fill, height: Fit,
        align: { y: 0.5 }
        padding: <THEME_MSPACE_1> { left: 15. }
        
        draw_text: {
            instance active: 0.0
            instance hover: 0.0
            instance disabled: 0.0

            uniform color: (THEME_COLOR_LABEL_INNER)
            uniform color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            uniform color_active: (THEME_COLOR_LABEL_INNER_DOWN)
            uniform color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P),
            }

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            self.color,
                            self.color_active,
                            self.active
                        ),
                        self.color_hover,
                        self.hover
                    ),
                    self.color_disabled,
                    self.disabled
                )
            }
        }
        
        draw_bg: {
            instance active: 0.0
            instance hover: 0.0
            instance disabled: 0.0

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color: (THEME_COLOR_U_HIDDEN)
            uniform color_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform color_active: (THEME_COLOR_OUTSET_ACTIVE)
            uniform color_disabled: (THEME_COLOR_OUTSET_ACTIVE)

            uniform border_color_1: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_active: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_disabled: (THEME_COLOR_U_HIDDEN)

            uniform border_color_2: (THEME_COLOR_U_HIDDEN)
            uniform border_color_2_hover: (THEME_COLOR_U_HIDDEN)
            uniform border_color_2_active: (THEME_COLOR_U_HIDDEN)
            uniform border_color_2_disabled: (THEME_COLOR_U_HIDDEN)

            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_active: (THEME_COLOR_MARK_ACTIVE)
            uniform mark_color_disabled: (THEME_COLOR_MARK_DISABLED)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )
                
                // Background
                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.color,
                                self.color_active,
                                self.active
                            ),
                            self.color_hover,
                            self.hover
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                );

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                self.hover
                            ),
                            mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                            self.active
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                );

                // Mark
                let sz = 3.;
                let dx = 2.0;
                let c = vec2(8.0, 0.5 * self.rect_size.y);
                sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx);
                sdf.line_to(c.x, c.y + sz);
                sdf.line_to(c.x + sz, c.y - sz);

                sdf.stroke(
                    mix(
                        mix(
                            self.mark_color,
                            self.mark_color_active,
                            self.active
                        ),
                        self.mark_color_disabled,
                        self.disabled
                    ), 1.
                );
                
                return sdf.result;
            }
        }
        
        animator: {
            disabled = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                        draw_text: {hover: 1.0}
                    }
                }
            }
            
            active = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {active: 0.0,}
                        draw_text: {active: 0.0,}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {active: 1.0,}
                        draw_text: {active: 1.0,}
                    }
                }
            }
        }
        indent_width: 10.0
    }

    PopupMenuItemGradientX = <PopupMenuItem> {
        draw_bg: {
            uniform color_1: (THEME_COLOR_U_HIDDEN)
            uniform color_1_hover: (THEME_COLOR_OUTSET_1_HOVER)
            uniform color_1_active: (THEME_COLOR_OUTSET_1_ACTIVE)
            uniform color_1_disabled: (THEME_COLOR_OUTSET_1_DISABLED)

            uniform color_2: (THEME_COLOR_U_HIDDEN)
            uniform color_2_hover: (THEME_COLOR_OUTSET_2_HOVER)
            uniform color_2_active: (THEME_COLOR_OUTSET_2_ACTIVE)
            uniform color_2_disabled: (THEME_COLOR_OUTSET_2_DISABLED)

            uniform border_color_1: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_active: (THEME_COLOR_U_HIDDEN)
            uniform border_color_1_disabled: (THEME_COLOR_U_HIDDEN)

            uniform border_color_2: (THEME_COLOR_U_HIDDEN)
            uniform border_color_2_hover: (THEME_COLOR_U_HIDDEN)
            uniform border_color_2_active: (THEME_COLOR_U_HIDDEN)
            uniform border_color_2_disabled: (THEME_COLOR_U_HIDDEN)

            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_active: (THEME_COLOR_MARK_ACTIVE)
            uniform mark_color_disabled: (THEME_COLOR_MARK_DISABLED)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                
                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                // Background
                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, self.pos.x),
                                mix(self.color_1_active, self.color_2_active, self.pos.x),
                                self.active
                            ),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x),
                            self.hover
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, self.pos.x),
                        self.disabled
                    )
                );

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ),
                            mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                            self.active
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, self.pos.y + dither),
                        self.disabled
                    ), self.border_size
                );

                // Mark
                let sz = 3.;
                let dx = 2.0;
                let c = vec2(8.0, 0.5 * self.rect_size.y);
                sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx);
                sdf.line_to(c.x, c.y + sz);
                sdf.line_to(c.x + sz, c.y - sz);

                sdf.stroke(
                    mix(
                        mix(
                            self.mark_color,
                            self.mark_color_active,
                            self.active
                        ),
                        self.mark_color_disabled,
                        self.disabled
                    ), 1.);
                
                return sdf.result;
            }
        }
    }

    PopupMenuItemGradientY = <PopupMenuItemGradientX> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                
                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                // Background
                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, gradient_fill.y),
                                mix(self.color_1_active, self.color_2_active, gradient_fill.y),
                                self.active
                            ),
                            mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                            self.hover
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
                        self.disabled
                    )
                );

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                self.hover
                            ),
                            mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                            self.active
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                );

                // Mark
                let sz = 3.;
                let dx = 2.0;
                let c = vec2(8.0, 0.5 * self.rect_size.y);
                sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx);
                sdf.line_to(c.x, c.y + sz);
                sdf.line_to(c.x + sz, c.y - sz);

                sdf.stroke(
                    mix(
                        mix( self.mark_color, self.mark_color_active, self.active),
                        self.mark_color_disabled,
                        self.disabled
                    ), 1.
                );
                
                return sdf.result;
            }
        }
    }

    pub PopupMenu = <PopupMenuBase> {
        width: 150., height: Fit,
        flow: Down,
        padding: <THEME_MSPACE_1> {}
        
        menu_item: <PopupMenuItem> {}
        
        draw_bg: {
            uniform color_dither: 1.0
            uniform color: (THEME_COLOR_FG_APP)

            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform border_size: (THEME_BEVELING)
            uniform border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
            uniform border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(self.color);

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(
                            self.border_color_1,
                            self.border_color_2,
                            gradient_border.y
                        ), self.border_size
                    );
                }

                return sdf.result;
            }
        }
    }
    pub PopupMenuFlat = <PopupMenu> {
        menu_item: <PopupMenuItem> {}

        draw_bg: {
            uniform border_color_1: (THEME_COLOR_BEVEL)
            uniform border_color_2: (THEME_COLOR_BEVEL)
        }
    }

    pub PopupMenuFlatter = <PopupMenuFlat> {
        draw_bg: { border_size: 0. }
    }
    
    pub PopupMenuGradientY = <PopupMenu> {
        menu_item: <PopupMenuItemGradientY> {}
        
        draw_bg: {
            uniform color_1: (THEME_COLOR_FG_APP)
            uniform color_2: (THEME_COLOR_FG_APP * 1.2)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(mix(self.color_1, self.color_2, gradient_fill.y));

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(
                            self.border_color_1,
                            self.border_color_2,
                            gradient_border.y
                        ), self.border_size
                    );
                }

                return sdf.result;
            }
        }
    }

    pub PopupMenuGradientX = <PopupMenuGradientY> {
        menu_item: <PopupMenuItemGradientX> {}
        
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither,
                    self.pos.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(mix(self.color_1, self.color_2, gradient_fill.x));

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(
                            self.border_color_1,
                            self.border_color_2,
                            gradient_border.y
                        ), self.border_size
                    );
                }

                return sdf.result;
            }
        }
    }


}


#[derive(Live, LiveHook, LiveRegister)]
pub struct PopupMenuItem {
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_text: DrawText,
    
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    #[walk] walk: Walk,
    
    #[live] indent_width: f32,
    #[live] icon_walk: Walk,
    
    #[live] opened: f32,
    #[live] hover: f32,
    #[live] active: f32,
}

#[derive(Live, LiveRegister)]
pub struct PopupMenu {
    #[live] draw_list: DrawList2d,
    #[live] menu_item: Option<LivePtr>,
    
    #[live] draw_bg: DrawQuad,
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[live] items: Vec<String>,
    #[rust] first_tap: bool,
    #[rust] menu_items: ComponentMap<PopupMenuItemId, PopupMenuItem>,
    #[rust] init_select_item: Option<PopupMenuItemId>,
    
    #[rust] count: usize,
}

impl LiveHook for PopupMenu {
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(list_node).as_field()) {
            for (_, node) in self.menu_items.iter_mut() {
                node.apply(cx, apply, index, nodes);
            }
        }
        self.draw_list.redraw(cx);
    }
}

pub enum PopupMenuItemAction {
    WasSweeped,
    WasSelected,
    MightBeSelected,
    None
}

#[derive(Clone, DefaultNone)]
pub enum PopupMenuAction {
    WasSweeped(PopupMenuItemId),
    WasSelected(PopupMenuItemId),
    None,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct PopupMenuItemId(pub LiveId);

impl PopupMenuItem {
    
    pub fn draw_item(
        &mut self,
        cx: &mut Cx2d,
        label: &str,
    ) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.draw_text.draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuItemAction),
    ) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.area().redraw(cx);
        }
        
        match event.hits_with_options(
            cx,
            self.draw_bg.area(),
            HitOptions::new().with_sweep_area(sweep_area)
        ) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                dispatch_action(cx, PopupMenuItemAction::WasSweeped);
                self.animator_play(cx, id!(hover.on));
                self.animator_play(cx, id!(active.on));
            }
            Hit::FingerUp(se) if se.is_primary_hit() => {
                if !se.is_sweep {
                    //if se.was_tap() { // ok this only goes for the first time
                    //    dispatch_action(cx, PopupMenuItemAction::MightBeSelected);
                    //    println!("MIGHTBESELECTED");
                    // }
                    //else {
                    dispatch_action(cx, PopupMenuItemAction::WasSelected);
                    //}
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                    self.animator_play(cx, id!(active.off));
                }
            }
            _ => {}
        }
    }
}

impl PopupMenu {
    
    pub fn menu_contains_pos(&self, cx: &mut Cx, pos: DVec2) -> bool {
        self.draw_bg.area().clipped_rect(cx).contains(pos)
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.draw_list.begin_overlay_reuse(cx);
        
        let size = cx.current_pass_size();
        cx.begin_sized_turtle(size, Layout::flow_down());
        
        // ok so. this thing needs a complete position reset
        self.draw_bg.begin(cx, self.walk, self.layout);
        self.count = 0;
    }
    
    pub fn end(&mut self, cx: &mut Cx2d, shift_area: Area, shift: DVec2) {
        // ok so.
        /*
        let menu_rect1 = cx.turtle().padded_rect_used();
        let pass_rect = Rect {pos: dvec2(0.0, 0.0), size: cx.current_pass_size()};
        let menu_rect2 = pass_rect.add_margin(-dvec2(10.0, 10.0)).contain(menu_rect1);
        */
        //cx.turtle_mut().set_shift(shift + (menu_rect2.pos - menu_rect1.pos));
        //let menu_rect1 = cx.turtle().padded_rect_used();
        self.draw_bg.end(cx);
        
        cx.end_pass_sized_turtle_with_shift(shift_area, shift);
        //cx.debug.rect_r(self.draw_bg.area().get_rect(cx));
        self.draw_list.end(cx);
        self.menu_items.retain_visible();
        if let Some(init_select_item) = self.init_select_item.take() {
            self.select_item_state(cx, init_select_item);
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_list.redraw(cx);
    }
    
    pub fn draw_item(
        &mut self,
        cx: &mut Cx2d,
        item_id: PopupMenuItemId,
        label: &str,
    ) {
        self.count += 1;
        
        let menu_item = self.menu_item;
        let menu_item = self.menu_items.get_or_insert(cx, item_id, | cx | {
            PopupMenuItem::new_from_ptr(cx, menu_item)
        });
        menu_item.draw_item(cx, label);
    }
    
    pub fn init_select_item(&mut self, which_id: PopupMenuItemId) {
        self.init_select_item = Some(which_id);
        self.first_tap = true;
    }
    
    fn select_item_state(&mut self, cx: &mut Cx, which_id: PopupMenuItemId) {
        for (id, item) in &mut *self.menu_items {
            if *id == which_id {
                item.animator_cut(cx, id!(active.on));
                item.animator_cut(cx, id!(hover.on));
            }
            else {
                item.animator_cut(cx, id!(active.off));
                item.animator_cut(cx, id!(hover.off));
            }
        }
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuAction),
    ) {
        let mut actions = Vec::new();
        for (item_id, node) in self.menu_items.iter_mut() {
            node.handle_event_with(cx, event, sweep_area, &mut | _, e | actions.push((*item_id, e)));
        }
        
        for (node_id, action) in actions {
            match action {
                PopupMenuItemAction::MightBeSelected => {
                    if self.first_tap {
                        self.first_tap = false;
                    }
                    else {
                        self.select_item_state(cx, node_id);
                        dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                    }
                }
                PopupMenuItemAction::WasSweeped => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                }
                PopupMenuItemAction::WasSelected => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                }
                _ => ()
            }
        }
    }
}

