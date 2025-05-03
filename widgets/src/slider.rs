use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        text_input::{TextInput, TextInputAction}
    }
};

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    DrawSlider = {{DrawSlider}} {}
    
    pub SliderBase = {{Slider}} {}
    
    pub SLIDER_ALT1_DATA_FONTSIZE = (THEME_FONT_SIZE_BASE);

    pub SliderMinimal = <SliderBase> {
        min: 0.0, max: 1.0,
        step: 0.0,
        label_align: { x: 0., y: 0. }
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        precision: 2,
        height: Fit,
        hover_actions_enabled: false,
        
        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float
            instance disabled: float

            uniform border_size: (THEME_BEVELING)

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1_HOVER)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)
            uniform color_1_disabled: (THEME_COLOR_INSET_1_DISABLED)
            uniform color_1_drag: (THEME_COLOR_INSET_1_DRAG)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2_HOVER)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
            uniform color_2_disabled: (THEME_COLOR_INSET_2_DISABLED)
            uniform color_2_drag: (THEME_COLOR_INSET_2_DRAG)
            
            uniform border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_1)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_OUTSET_1)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)

            uniform border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_OUTSET_2)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)

            uniform val_color: (THEME_COLOR_VAL)
            uniform val_color_hover: (THEME_COLOR_VAL_HOVER)
            uniform val_color_focus: (THEME_COLOR_VAL_FOCUS)
            uniform val_color_drag: (THEME_COLOR_VAL_DRAG)
            uniform val_color_disabled: (THEME_COLOR_VAL_DISABLED)

            uniform handle_color: (THEME_COLOR_HANDLE)
            uniform handle_color_hover: (THEME_COLOR_HANDLE_HOVER)
            uniform handle_color_focus: (THEME_COLOR_HANDLE_FOCUS)
            uniform handle_color_drag: (THEME_COLOR_HANDLE_DRAG)
            uniform handle_color_disabled: (THEME_COLOR_HANDLE_DISABLED)

            fn pixel(self) -> vec4 {
                let slider_height = self.border_size * 2.5;
                let handle_size = mix(3, 5, self.hover);
                let handle_bg_size = mix(0, 10, self.hover)

                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
 
                // Track shadow
                sdf.rect(
                    0.,
                    self.rect_size.y - slider_height * 2,
                    self.rect_size.x,
                    slider_height + 1
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.border_color_2,
                                self.border_color_2_focus,
                                self.focus
                            ),
                            mix(
                                self.border_color_2_hover,
                                self.border_color_2_drag,
                                self.drag
                            ),
                            self.hover
                        ),
                        self.border_color_2_disabled,
                        self.disabled
                    )
                );
                    
                // Track highlight
                sdf.rect(
                    0,
                    self.rect_size.y - slider_height,
                    self.rect_size.x,
                    slider_height
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.border_color_1,
                                self.border_color_1_focus,
                                self.focus
                            ),
                            mix(
                                self.border_color_1_hover, 
                                self.border_color_1_drag,
                                self.drag
                            ),
                            self.hover
                        ),
                        self.border_color_1_disabled,
                        self.disabled
                    )
                );
                    
                // // Amount
                sdf.rect(
                    0,
                    self.rect_size.y - slider_height * 2.,
                    self.slide_pos * (self.rect_size.x) + handle_size,
                    slider_height * 2. + 1.
                )
                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.val_color,
                                self.val_color_focus,
                                self.focus
                            ),
                            mix(
                                self.val_color_hover,
                                self.val_color_drag,
                                self.drag
                            ),
                            self.hover
                        ),
                        self.val_color_disabled,
                        self.disabled
                    )
                );
                    
                // Handle
                let handle_bg_x = self.slide_pos * (self.rect_size.x - handle_size) - handle_bg_size * 0.5 + 0.5 * handle_size;

                sdf.rect(
                    handle_bg_x,
                    self.rect_size.y - slider_height * 2.,
                    handle_bg_size,
                    slider_height * 2.
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.handle_color,
                                self.handle_color_focus,
                                self.focus
                            ),
                            mix(
                                self.handle_color_hover,
                                self.handle_color_drag,
                                self.drag
                            ),
                            self.hover
                        ),
                        self.handle_color_disabled,
                        self.disabled
                    )
                );

                return sdf.result
            }
        }

        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            instance empty: 0.0
            instance drag: 0.0
            instance disabled: 0.0

            color: (THEME_COLOR_LABEL_OUTER)
            uniform color_hover: (THEME_COLOR_LABEL_OUTER_HOVER)
            uniform color_drag: (THEME_COLOR_LABEL_OUTER_DRAG)
            uniform color_focus: (THEME_COLOR_LABEL_OUTER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)
            uniform color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)

            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_WDGT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }

            fn get_color(self) -> vec4 {
                return
                mix(
                    mix(
                        mix(
                            mix(
                                self.color,
                                self.color_focus,
                                self.focus
                            ),
                            self.color_empty,
                            self.empty
                        ),
                        mix(self.color_hover, self.color_drag, self.drag),
                        self.hover
                    ),
                    self.color_disabled,
                    self.disabled
                )
            }
        }
            
        label_walk: {
            width: Fill, height: Fit,
            margin: { top: 0., bottom: (THEME_SPACE_1) },
        }
            
        text_input: <TextInput> {
            empty_text: "0",
            is_numeric_only: true,
            is_read_only: false,

            width: Fit,
            label_align: {y: 0.},
            margin: 0.
            padding: 0.

            draw_text: {
                color: (THEME_COLOR_TEXT_VAL)
                color_hover: (THEME_COLOR_TEXT_HOVER)
                color_focus: (THEME_COLOR_TEXT_FOCUS)
                color_down: (THEME_COLOR_TEXT_DOWN)
                color_disabled: (THEME_COLOR_TEXT_DISABLED)
                color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
                color_empty_hover: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)
                color_empty_focus: (THEME_COLOR_TEXT_FOCUS)
            }

            
            draw_bg: {
                border_radius: 0.
                border_size: 0.

                color: (THEME_COLOR_U_HIDDEN)
                color_hover: (THEME_COLOR_U_HIDDEN)
                color_focus: (THEME_COLOR_U_HIDDEN)
                color_disabled: (THEME_COLOR_U_HIDDEN)
                color_empty: (THEME_COLOR_U_HIDDEN)

                border_color_1: (THEME_COLOR_U_HIDDEN)
                border_color_1_hover: (THEME_COLOR_U_HIDDEN)
                border_color_1_empty: (THEME_COLOR_U_HIDDEN)
                border_color_1_disabled: (THEME_COLOR_U_HIDDEN)
                border_color_1_focus: (THEME_COLOR_U_HIDDEN)

                border_color_2: (THEME_COLOR_U_HIDDEN)
                border_color_2_hover: (THEME_COLOR_U_HIDDEN)
                border_color_2_empty: (THEME_COLOR_U_HIDDEN)
                border_color_2_disabled: (THEME_COLOR_U_HIDDEN)
                border_color_2_focus: (THEME_COLOR_U_HIDDEN)
            }

            draw_cursor: { color: (THEME_COLOR_TEXT_CURSOR) }

            draw_selection: {
                border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

                color: (THEME_COLOR_D_HIDDEN)
                color_hover: (THEME_COLOR_D_HIDDEN)
                color_focus: (THEME_COLOR_D_HIDDEN)
                color_empty: (THEME_COLOR_U_HIDDEN)
                color_disabled: (THEME_COLOR_U_HIDDEN)
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
                    from: {all: Forward {duration: 0.2}}
                    ease: OutQuad
                    apply: {
                        draw_bg: { hover: 0.0 },
                        draw_text: { hover: 0.0 },
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_bg: { hover: 1.0 },
                        draw_text: { hover: 1.0 },
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        // draw_text: {focus: 0.0, hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        // draw_text: {focus: 1.0, hover: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {drag: 0.0}
                        draw_text: {drag: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_bg: {drag: 1.0}
                        draw_text: {drag: 1.0}
                    }
                }
            }
        }
    }

    pub SliderMinimalFlat = <SliderMinimal> {
        draw_bg: {
            border_color_1: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_1_drag: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_2_drag: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)
        }
    }
        
    pub Slider = <SliderMinimal> {
        height: 36;
        draw_bg: {
            instance disabled: 0.0,

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_disabled: (THEME_COLOR_INSET_DISABLED)
            uniform color_drag: (THEME_COLOR_INSET_DRAG)

            uniform handle_color_1: (THEME_COLOR_HANDLE_1)
            uniform handle_color_1_hover: (THEME_COLOR_HANDLE_1_HOVER)
            uniform handle_color_1_focus: (THEME_COLOR_HANDLE_1_FOCUS)
            uniform handle_color_1_disabled: (THEME_COLOR_HANDLE_1_DISABLED)
            uniform handle_color_1_drag: (THEME_COLOR_HANDLE_1_DRAG)

            uniform handle_color_2: (THEME_COLOR_HANDLE_2)
            uniform handle_color_2_hover: (THEME_COLOR_HANDLE_2_HOVER)
            uniform handle_color_2_focus: (THEME_COLOR_HANDLE_2_FOCUS)
            uniform handle_color_2_disabled: (THEME_COLOR_HANDLE_2_DISABLED)
            uniform handle_color_2_drag: (THEME_COLOR_HANDLE_2_DRAG)

            uniform border_color_1: (THEME_COLOR_BEVEL_INSET_2)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_INSET_2_DRAG)

            uniform border_color_2: (THEME_COLOR_BEVEL_INSET_1)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)

            uniform val_size: 3.

            uniform val_color: (THEME_COLOR_VAL)
            uniform val_color_hover: (THEME_COLOR_VAL_HOVER)
            uniform val_color_focus: (THEME_COLOR_VAL_FOCUS)
            uniform val_color_disabled: (THEME_COLOR_VAL_DISABLED)
            uniform val_color_drag: (THEME_COLOR_VAL_DRAG)

            uniform handle_size: 20.
            uniform bipolar: 0.0,

            fn pixel(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let handle_sz = self.handle_size;
                    
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)

                let offset_px = vec2(0, 20.)

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )
                    
                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let sz_px = vec2(
                    self.rect_size.x,
                    self.rect_size.y - offset_px.y
                );

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px.x,
                    self.rect_size.y / sz_px.y
                );

                let gradient_border = vec2(
                    self.pos.x * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2. - offset_px.y
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )
                    
                sdf.box(
                    self.border_size,
                    offset_px.y + self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - offset_px.y - self.border_size * 2.,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.color,
                                self.color_hover,
                                self.hover
                            ),
                            mix(
                                self.color_focus,
                                mix(
                                    self.color_hover,
                                    self.color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                )
                    
                sdf.stroke(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, gradient_border.y),
                            mix(
                                mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                mix(
                                    mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                    mix(self.border_color_1_drag, self.border_color_2_drag, gradient_border.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                // Ridge
                let offset_sides = self.border_size + 6.;
                sdf.rect(
                    self.border_size + offset_sides,
                    offset_px.y + (self.rect_size.y - offset_px.y) * 0.5 - self.border_size - 1,
                    self.rect_size.x - 2 * offset_sides - self.border_size * 2.,
                    self.border_size * 2. + 1. 
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_1_hover, self.hover),
                            mix(
                                self.border_color_1_focus,
                                mix(
                                    self.border_color_1_hover,
                                    self.border_color_1_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.border_color_1_disabled,
                        self.disabled
                    )
                );

                sdf.rect(
                    self.border_size + offset_sides,
                    offset_px.y + (self.rect_size.y - offset_px.y) * 0.5,
                    self.rect_size.x - 2 * offset_sides - self.border_size * 2. - 1,
                    self.border_size * 2.
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.border_color_2,
                                self.border_color_2_hover,
                                self.hover
                            ),
                            mix(
                                self.border_color_2_hover,
                                self.border_color_2_drag,
                                self.drag
                            ),
                            self.hover
                        ),
                        self.border_color_2_disabled,
                        self.disabled
                    )
                );
                    
                // Handle
                let track_length = self.rect_size.x - offset_sides * 4.;
                let val_x = self.slide_pos * track_length + offset_sides * 2.;
                
                let offset_top = self.rect_size.y - (self.rect_size.y - offset_px.y) * 0.5
                sdf.move_to(
                    mix(
                        offset_sides,
                        self.rect_size.x * 0.5,
                        self.bipolar
                    ),
                    offset_top
                );
                sdf.line_to(
                    val_x,
                    offset_top
                );

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                self.val_color,
                                self.val_color_hover,
                                self.hover
                            ),
                            mix(
                                self.val_color_focus,
                                mix(
                                    self.val_color_hover,
                                    self.val_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.val_color_disabled,
                        self.disabled
                    ) , self.val_size
                )
                    
                let ctrl_height = self.rect_size.y - offset_px.y;
                let handle_x = self.slide_pos * (self.rect_size.x - handle_sz - offset_sides) - 3;
                let handle_padding = 1.5;
                sdf.box(
                    handle_x + offset_sides + self.border_size,
                    offset_px.y + self.border_size + handle_padding,
                    self.handle_size - self.border_size * 2.,
                    ctrl_height - self.border_size * 2. - handle_padding * 2.,
                    self.border_radius
                )
                    
                sdf.fill_keep( 
                    mix(
                        mix(
                            mix(
                                mix(self.handle_color_1, self.handle_color_2, gradient_fill.y),
                                mix(self.handle_color_1_hover, self.handle_color_2_hover, gradient_fill.y),
                                self.hover
                            ),
                            mix(
                                mix(self.handle_color_1_focus, self.handle_color_2_focus, gradient_fill.y),
                                mix(
                                    mix(self.handle_color_1_hover, self.handle_color_2_hover, gradient_fill.y),
                                    mix(self.handle_color_1_drag, self.handle_color_2_drag, gradient_fill.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.handle_color_1_disabled, self.handle_color_2_disabled, gradient_fill.y),
                        self.disabled
                    )
                )
                
                sdf.stroke(
                    mix(
                        mix(
                            mix(self.border_color_2, self.border_color_1, gradient_border.y),
                            mix(
                                mix(self.border_color_2_hover, self.border_color_1_hover, gradient_border.y),
                                mix(self.border_color_2_drag, self.border_color_1_drag, gradient_border.y),
                                self.drag
                            ),
                            self.hover
                        ),
                        mix(self.border_color_2_disabled, self.border_color_1_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                );
                
                return sdf.result
            }
        }
    }

    pub SliderGradientY = <Slider> {
        draw_bg: {
            instance disabled: 0.0,

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1_HOVER)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)
            uniform color_1_disabled: (THEME_COLOR_INSET_1_DISABLED)
            uniform color_1_drag: (THEME_COLOR_INSET_1_DRAG)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2_HOVER)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
            uniform color_2_disabled: (THEME_COLOR_INSET_2_DISABLED)
            uniform color_2_drag: (THEME_COLOR_INSET_2_DRAG)

            uniform handle_color_1: (THEME_COLOR_HANDLE_1)
            uniform handle_color_1_hover: (THEME_COLOR_HANDLE_1_HOVER)
            uniform handle_color_1_focus: (THEME_COLOR_HANDLE_1_FOCUS)
            uniform handle_color_1_disabled: (THEME_COLOR_HANDLE_1_DISABLED)
            uniform handle_color_1_drag: (THEME_COLOR_HANDLE_1_DRAG)

            uniform handle_color_2: (THEME_COLOR_HANDLE_2)
            uniform handle_color_2_hover: (THEME_COLOR_HANDLE_2_HOVER)
            uniform handle_color_2_focus: (THEME_COLOR_HANDLE_2_FOCUS)
            uniform handle_color_2_disabled: (THEME_COLOR_HANDLE_2_DISABLED)
            uniform handle_color_2_drag: (THEME_COLOR_HANDLE_2_DRAG)

            uniform border_color_1: (THEME_COLOR_BEVEL_INSET_2)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_INSET_2_DRAG)

            uniform border_color_2: (THEME_COLOR_BEVEL_INSET_1)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)

            uniform val_size: 3.

            uniform val_color: (THEME_COLOR_VAL)
            uniform val_color_hover: (THEME_COLOR_VAL_HOVER)
            uniform val_color_focus: (THEME_COLOR_VAL_FOCUS)
            uniform val_color_disabled: (THEME_COLOR_VAL_DISABLED)
            uniform val_color_drag: (THEME_COLOR_VAL_DRAG)

            uniform handle_size: 20.
            uniform bipolar: 0.0,

            fn pixel(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let handle_sz = self.handle_size;
                    
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)

                let offset_px = vec2(0, 20.)

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )
                    
                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let sz_px = vec2(
                    self.rect_size.x,
                    self.rect_size.y - offset_px.y
                );

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px.x,
                    self.rect_size.y / sz_px.y
                );

                let gradient_border = vec2(
                    self.pos.x * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2. - offset_px.y
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )
                    
                sdf.box(
                    self.border_size,
                    offset_px.y + self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - offset_px.y - self.border_size * 2.,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, gradient_fill.y),
                                mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                                self.hover
                            ),
                            mix(
                                mix(self.color_1_focus, self.color_2_focus, gradient_fill.y),
                                mix(
                                    mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                                    mix(self.color_1_drag, self.color_2_drag, gradient_fill.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
                        self.disabled
                    )
                )
                    
                sdf.stroke(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, gradient_border.y),
                            mix(
                                mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                mix(
                                    mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                    mix(self.border_color_1_drag, self.border_color_2_drag, gradient_border.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                // Ridge
                let offset_sides = self.border_size + 6.;
                sdf.rect(
                    self.border_size + offset_sides,
                    offset_px.y + (self.rect_size.y - offset_px.y) * 0.5 - self.border_size - 1,
                    self.rect_size.x - 2 * offset_sides - self.border_size * 2.,
                    self.border_size * 2. + 1. 
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_1_hover, self.hover),
                            mix(
                                self.border_color_1_focus,
                                mix(
                                    self.border_color_1_hover,
                                    self.border_color_1_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.border_color_1_disabled,
                        self.disabled
                    )
                );

                sdf.rect(
                    self.border_size + offset_sides,
                    offset_px.y + (self.rect_size.y - offset_px.y) * 0.5,
                    self.rect_size.x - 2 * offset_sides - self.border_size * 2. - 1,
                    self.border_size * 2.
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.border_color_2,
                                self.border_color_2_hover,
                                self.hover
                            ),
                            mix(
                                self.border_color_2_hover,
                                self.border_color_2_drag,
                                self.drag
                            ),
                            self.hover
                        ),
                        self.border_color_2_disabled,
                        self.disabled
                    )
                );
                    
                // Handle
                let track_length = self.rect_size.x - offset_sides * 4.;
                let val_x = self.slide_pos * track_length + offset_sides * 2.;
                
                let offset_top = self.rect_size.y - (self.rect_size.y - offset_px.y) * 0.5
                sdf.move_to(
                    mix(
                        offset_sides,
                        self.rect_size.x * 0.5,
                        self.bipolar
                    ),
                    offset_top
                );
                sdf.line_to(
                    val_x,
                    offset_top
                );

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                self.val_color,
                                self.val_color_hover,
                                self.hover
                            ),
                            mix(
                                self.val_color_focus,
                                mix(
                                    self.val_color_hover,
                                    self.val_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.val_color_disabled,
                        self.disabled
                    ) , self.val_size
                )
                    
                let ctrl_height = self.rect_size.y - offset_px.y;
                let handle_x = self.slide_pos * (self.rect_size.x - handle_sz - offset_sides) - 3;
                let handle_padding = 1.5;
                sdf.box(
                    handle_x + offset_sides + self.border_size,
                    offset_px.y + self.border_size + handle_padding,
                    self.handle_size - self.border_size * 2.,
                    ctrl_height - self.border_size * 2. - handle_padding * 2.,
                    self.border_radius
                )
                    
                sdf.fill_keep( 
                    mix(
                        mix(
                            mix(
                                mix(self.handle_color_1, self.handle_color_2, gradient_fill.y),
                                mix(self.handle_color_1_hover, self.handle_color_2_hover, gradient_fill.y),
                                self.hover
                            ),
                            mix(
                                mix(self.handle_color_1_focus, self.handle_color_2_focus, gradient_fill.y),
                                mix(
                                    mix(self.handle_color_1_hover, self.handle_color_2_hover, gradient_fill.y),
                                    mix(self.handle_color_1_drag, self.handle_color_2_drag, gradient_fill.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.handle_color_1_disabled, self.handle_color_2_disabled, gradient_fill.y),
                        self.disabled
                    )
                )
                
                sdf.stroke(
                    mix(
                        mix(
                            mix(self.border_color_2, self.border_color_1, gradient_border.y),
                            mix(
                                mix(self.border_color_2_hover, self.border_color_1_hover, gradient_border.y),
                                mix(self.border_color_2_drag, self.border_color_1_drag, gradient_border.y),
                                self.drag
                            ),
                            self.hover
                        ),
                        mix(self.border_color_2_disabled, self.border_color_1_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                );
                
                return sdf.result
            }
        }
    }

    pub SliderFlat = <Slider> {
        draw_bg: {
            border_size: (THEME_BEVELING)

            uniform color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_disabled: (THEME_COLOR_INSET_DISABLED)
            uniform color_drag: (THEME_COLOR_INSET_DRAG)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_1_disabled: (THEME_COLOR_BEVEL_DISABLED)
            border_color_1_drag: (THEME_COLOR_BEVEL_DRAG)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_2_disabled: (THEME_COLOR_BEVEL_DISABLED)
            border_color_2_drag: (THEME_COLOR_BEVEL_DRAG)

            handle_color_1: (THEME_COLOR_HANDLE)
            handle_color_1_hover: (THEME_COLOR_HANDLE_HOVER)
            handle_color_1_focus: (THEME_COLOR_HANDLE_FOCUS)
            handle_color_1_disabled: (THEME_COLOR_HANDLE_DISABLED)
            handle_color_1_drag: (THEME_COLOR_HANDLE_DRAG)

            handle_color_2: (THEME_COLOR_HANDLE)
            handle_color_2_hover: (THEME_COLOR_HANDLE_HOVER)
            handle_color_2_focus: (THEME_COLOR_HANDLE_FOCUS)
            handle_color_2_disabled: (THEME_COLOR_HANDLE_DISABLED)
            handle_color_2_drag: (THEME_COLOR_HANDLE_DRAG)

            handle_size: 14.
        }

    }

    pub SliderFlatter = <SliderFlat> {
        draw_bg: {
            instance disabled: 0.0,

            handle_size: 0.
            border_size: 0.
        }
    }


    pub SLIDER_ALT1_HANDLE_SIZE = 4.0;
    pub SLIDER_ALT1_DATA_FONT_TOPMARGIN = 3.0;
    pub SLIDER_ALT1_VAL_PADDING = 2.5;

    pub SliderRound = <SliderMinimal> {
        height: 18.,
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        text_input: <TextInput> {
            width: Fit,
            padding: 0.,
            margin: { right: 7.5, top: (SLIDER_ALT1_DATA_FONT_TOPMARGIN) } 

            draw_text: {
                instance hover: 0.0
                instance focus: 0.0
                instance empty: 0.0
                instance drag: 0.0
                instance disabled: 0.0

                color: (THEME_COLOR_TEXT_VAL)
                uniform color_hover: (THEME_COLOR_TEXT_HOVER)
                uniform color_focus: (THEME_COLOR_TEXT_FOCUS)
                uniform color_drag: (THEME_COLOR_TEXT_DOWN)
                uniform color_disabled: (THEME_COLOR_TEXT_DISABLED)
                uniform color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
                uniform color_empty_hover: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)
                uniform color_empty_focus: (THEME_COLOR_TEXT_FOCUS)

                text_style: <THEME_FONT_REGULAR> {
                    font_size: (SLIDER_ALT1_DATA_FONTSIZE)
                }

                fn get_color(self) -> vec4 {
                    return
                    mix(
                        mix(
                            mix(
                                mix(
                                    self.color,
                                    self.color_empty,
                                    self.empty
                                ),
                                mix(self.color_hover, self.color_drag, self.drag),
                                self.hover
                            ),
                            mix(self.color_focus, self.color_hover, self.hover),
                            self.focus
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                }
            }

            draw_bg: {
                border_size: 0.

                color: (THEME_COLOR_U_HIDDEN)
                color_hover: (THEME_COLOR_U_HIDDEN)
                color_focus: (THEME_COLOR_U_HIDDEN)
                color_disabled: (THEME_COLOR_U_HIDDEN)
                color_empty: (THEME_COLOR_U_HIDDEN)
            }

            draw_selection: {
                border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

                color: (THEME_COLOR_D_HIDDEN)
                color_hover: (THEME_COLOR_D_HIDDEN)
                color_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
            }

        }

        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float
            instance instance: float

            label_size: 75.

            uniform val_heat: 10.

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS * 2.)

            uniform color_dither: 1.0
            
            uniform color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_disabled: (THEME_COLOR_INSET_DISABLED)
            uniform color_drag: (THEME_COLOR_INSET_DRAG)

            uniform border_color_1: (THEME_COLOR_BEVEL_INSET_2)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_INSET_2_DRAG)

            uniform border_color_2: (THEME_COLOR_BEVEL_INSET_1)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)

            uniform val_color_1: (THEME_COLOR_VAL_1)
            uniform val_color_1_hover: (THEME_COLOR_VAL_1_HOVER)
            uniform val_color_1_focus: (THEME_COLOR_VAL_1_FOCUS)
            uniform val_color_1_disabled: (THEME_COLOR_VAL_1_DISABLED)
            uniform val_color_1_drag: (THEME_COLOR_VAL_1_DRAG)

            uniform val_color_2: (THEME_COLOR_VAL_2)
            uniform val_color_2_hover: (THEME_COLOR_VAL_2_HOVER)
            uniform val_color_2_focus: (THEME_COLOR_VAL_2_FOCUS)
            uniform val_color_2_disabled: (THEME_COLOR_VAL_2_DISABLED)
            uniform val_color_2_drag: (THEME_COLOR_VAL_2_DRAG)

            uniform handle_color: (THEME_COLOR_HANDLE);
            uniform handle_color_hover: (THEME_COLOR_HANDLE_HOVER);
            uniform handle_color_focus: (THEME_COLOR_HANDLE_FOCUS);
            uniform handle_color_disabled: (THEME_COLOR_HANDLE_DISABLED);
            uniform handle_color_drag: (THEME_COLOR_HANDLE_DRAG);

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

                let label_sz_uv = self.label_size / self.rect_size.x;

                let gradient_fill = vec2(
                    (pow(self.pos.x, self.val_heat) - label_sz_uv) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )


                let handle_size = (SLIDER_ALT1_HANDLE_SIZE);
                let padding = (SLIDER_ALT1_VAL_PADDING);

                let track_length_bg = self.rect_size.x - self.label_size;
                let padding_full = padding * 2.;
                let min_size = padding_full + handle_size * 2.;
                let track_length_val = self.rect_size.x - self.label_size - padding_full - min_size;

                // Background
                sdf.box(
                    self.label_size + self.border_size,
                    self.border_size,
                    track_length_bg - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.color,
                                self.color_hover,
                                self.hover
                            ),
                            mix(
                                self.color_focus,
                                mix(
                                    self.color_hover,
                                    self.color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                )

                sdf.stroke(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, gradient_border.y),
                            mix(
                                mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                mix(
                                    mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                    mix(self.border_color_1_drag, self.border_color_2_drag, gradient_border.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                // Amount bar
                let handle_shift = self.label_size + padding_full + handle_size;
                let val_height = self.rect_size.y - padding_full - self.border_size * 2.;
                let val_offset_x = self.label_size + padding + self.border_size + val_height * 0.5;
                let val_target_x = track_length_val * self.slide_pos + min_size - self.border_size * 2. - val_height;

                sdf.circle(
                    val_offset_x,
                    self.rect_size.y * 0.5,
                    val_height * 0.5
                );

                sdf.box(
                    val_offset_x,
                    padding + self.border_size,
                    val_target_x,
                    self.rect_size.y - padding_full - self.border_size * 2.,
                    1.
                );

                sdf.circle(
                    track_length_val * self.slide_pos + handle_shift,
                    self.rect_size.y * 0.5,
                    val_height * 0.5
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.val_color_1, self.val_color_2, gradient_fill.x),
                                mix(self.val_color_1_hover, self.val_color_2_hover, gradient_fill.x),
                                self.hover
                            ),
                            mix(
                                mix(self.val_color_1_focus, self.val_color_2_focus, gradient_fill.x),
                                mix(
                                    mix(self.val_color_1_hover, self.val_color_2_hover, gradient_fill.x),
                                    mix(self.val_color_1_drag, self.val_color_2_drag, gradient_fill.x),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.val_color_1_disabled, self.val_color_2_disabled, gradient_fill.x),
                        self.disabled
                    )
                )

                // Handle
                sdf.circle(
                    track_length_val * self.slide_pos + handle_shift,
                    self.rect_size.y * 0.5,
                    mix(0., handle_size, self.hover)
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.handle_color,
                                self.handle_color_hover,
                                self.hover
                            ),
                            mix(
                                self.handle_color_focus,
                                mix(
                                    self.handle_color_hover,
                                    self.handle_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.handle_color_disabled,
                        self.disabled
                    )
                )
                
                return sdf.result
            }
        }

    }

    pub SliderRoundGradientY = <SliderRound> {
        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float
            instance instance: float

            label_size: 75.

            uniform val_heat: 10.

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS * 2.)

            uniform color_dither: 1.0
            
            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1_HOVER)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)
            uniform color_1_disabled: (THEME_COLOR_INSET_1_DISABLED)
            uniform color_1_drag: (THEME_COLOR_INSET_1_DRAG)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2_HOVER)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
            uniform color_2_disabled: (THEME_COLOR_INSET_2_DISABLED)
            uniform color_2_drag: (THEME_COLOR_INSET_2_DRAG)

            uniform border_color_1: (THEME_COLOR_BEVEL_INSET_2)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_INSET_2_DRAG)

            uniform border_color_2: (THEME_COLOR_BEVEL_INSET_1)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)

            uniform val_color_1: (THEME_COLOR_VAL_1)
            uniform val_color_1_hover: (THEME_COLOR_VAL_1_HOVER)
            uniform val_color_1_focus: (THEME_COLOR_VAL_1_FOCUS)
            uniform val_color_1_disabled: (THEME_COLOR_VAL_1_DISABLED)
            uniform val_color_1_drag: (THEME_COLOR_VAL_1_DRAG)

            uniform val_color_2: (THEME_COLOR_VAL_2)
            uniform val_color_2_hover: (THEME_COLOR_VAL_2_HOVER)
            uniform val_color_2_focus: (THEME_COLOR_VAL_2_FOCUS)
            uniform val_color_2_disabled: (THEME_COLOR_VAL_2_DISABLED)
            uniform val_color_2_drag: (THEME_COLOR_VAL_2_DRAG)

            uniform handle_color: (THEME_COLOR_HANDLE);
            uniform handle_color_hover: (THEME_COLOR_HANDLE_HOVER);
            uniform handle_color_focus: (THEME_COLOR_HANDLE_FOCUS);
            uniform handle_color_disabled: (THEME_COLOR_HANDLE_DISABLED);
            uniform handle_color_drag: (THEME_COLOR_HANDLE_DRAG);

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

                let label_sz_uv = self.label_size / self.rect_size.x;

                let gradient_fill = vec2(
                    (pow(self.pos.x, self.val_heat) - label_sz_uv) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )


                let handle_size = (SLIDER_ALT1_HANDLE_SIZE);
                let padding = (SLIDER_ALT1_VAL_PADDING);

                let track_length_bg = self.rect_size.x - self.label_size;
                let padding_full = padding * 2.;
                let min_size = padding_full + handle_size * 2.;
                let track_length_val = self.rect_size.x - self.label_size - padding_full - min_size;

                // Background
                sdf.box(
                    self.label_size + self.border_size,
                    self.border_size,
                    track_length_bg - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, gradient_fill.y),
                                mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                                self.hover
                            ),
                            mix(
                                mix(self.color_1_focus, self.color_2_focus, gradient_fill.y),
                                mix(
                                    mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                                    mix(self.color_1_drag, self.color_2_drag, gradient_fill.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus), mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y), self.disabled
                    )
                )

                sdf.stroke(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, gradient_border.y),
                            mix(
                                mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                mix(
                                    mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                    mix(self.border_color_1_drag, self.border_color_2_drag, gradient_border.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                // Amount bar
                let handle_shift = self.label_size + padding_full + handle_size;
                let val_height = self.rect_size.y - padding_full - self.border_size * 2.;
                let val_offset_x = self.label_size + padding + self.border_size + val_height * 0.5;
                let val_target_x = track_length_val * self.slide_pos + min_size - self.border_size * 2. - val_height;

                sdf.circle(
                    val_offset_x,
                    self.rect_size.y * 0.5,
                    val_height * 0.5
                );

                sdf.box(
                    val_offset_x,
                    padding + self.border_size,
                    val_target_x,
                    self.rect_size.y - padding_full - self.border_size * 2.,
                    1.
                );

                sdf.circle(
                    track_length_val * self.slide_pos + handle_shift,
                    self.rect_size.y * 0.5,
                    val_height * 0.5
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.val_color_1, self.val_color_2, gradient_fill.x),
                                mix(self.val_color_1_hover, self.val_color_2_hover, gradient_fill.x),
                                self.hover
                            ),
                            mix(
                                mix(self.val_color_1_focus, self.val_color_2_focus, gradient_fill.x),
                                mix(
                                    mix(self.val_color_1_hover, self.val_color_2_hover, gradient_fill.x),
                                    mix(self.val_color_1_drag, self.val_color_2_drag, gradient_fill.x),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.val_color_1_disabled, self.val_color_2_disabled, gradient_fill.x),
                        self.disabled
                    )
                )

                // Handle
                sdf.circle(
                    track_length_val * self.slide_pos + handle_shift,
                    self.rect_size.y * 0.5,
                    mix(0., handle_size, self.hover)
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.handle_color,
                                self.handle_color_hover,
                                self.hover
                            ),
                            mix(
                                self.handle_color_focus,
                                mix(
                                    self.handle_color_hover,
                                    self.handle_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.handle_color_disabled,
                        self.disabled
                    )
                )
                
                return sdf.result
            }
        }

    }

    pub SliderRoundFlat = <SliderRound> {
        draw_bg: {
            color: (THEME_COLOR_INSET)
            color_hover: (THEME_COLOR_INSET_HOVER)
            color_focus: (THEME_COLOR_INSET_FOCUS)
            color_disabled: (THEME_COLOR_INSET_DISABLED)
            color_drag: (THEME_COLOR_INSET_DRAG)

            uniform border_color_1: (THEME_COLOR_BEVEL)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_DISABLED)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_DRAG)

            uniform border_color_2: (THEME_COLOR_BEVEL)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_DISABLED)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_DRAG)
        }
    }

    pub SliderRoundFlatter = <SliderRoundFlat> {
        draw_bg: {
            border_size: 0.
        }
    }

    pub Rotary = <SliderMinimal> {
        height: 95., width: 65.,
        axis: Vertical,
        flow: Right
        align:{x:0.,y:0.0}
        label_walk:{
            margin:{top:0}
            width: Fill
        }
        text_input:{ 
            width: Fit
        }
        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform gap: 90.
            uniform val_padding: 10.
            uniform weight: 40.

            uniform border_size: (THEME_BEVELING)
            uniform val_size: 20.

            uniform color_dither: 1.,
            
            uniform color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_disabled: (THEME_COLOR_INSET_DISABLED)
            uniform color_drag: (THEME_COLOR_INSET_DRAG)

            uniform border_color_1: (THEME_COLOR_BEVEL_INSET_1)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_INSET_1_DRAG)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)

            uniform border_color_2: (THEME_COLOR_BEVEL_INSET_2)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_INSET_2_DRAG)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

            uniform handle_color: (THEME_COLOR_HANDLE);
            uniform handle_color_hover: (THEME_COLOR_HANDLE_HOVER);
            uniform handle_color_focus: (THEME_COLOR_HANDLE_FOCUS);
            uniform handle_color_disabled: (THEME_COLOR_HANDLE_DISABLED);
            uniform handle_color_drag: (THEME_COLOR_HANDLE_DRAG);

            uniform val_color_1: (THEME_COLOR_VAL_1);
            uniform val_color_1_hover: (THEME_COLOR_VAL_1);
            uniform val_color_1_focus: (THEME_COLOR_VAL_1);
            uniform val_color_1_disabled: (THEME_COLOR_VAL_1);
            uniform val_color_1_drag: (THEME_COLOR_VAL_1_DRAG);

            uniform val_color_2: (THEME_COLOR_VAL_2);
            uniform val_color_2_hover: (THEME_COLOR_VAL_2);
            uniform val_color_2_focus: (THEME_COLOR_VAL_2);
            uniform val_color_2_disabled: (THEME_COLOR_VAL_2);
            uniform val_color_2_drag: (THEME_COLOR_VAL_2_DRAG);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let one_deg = PI / 180;
                let threesixty_deg = 2. * PI;
                let gap_size = self.gap * one_deg;
                let val_length = threesixty_deg - (one_deg * self.gap);
                let start = gap_size * 0.5;
                let outer_end = start + val_length;
                let val_end = start + val_length * self.slide_pos;

                let label_offset_px = 20.;
                let label_offset_uv = self.rect_size.y;
                let scale_px = min(self.rect_size.x, self.rect_size.y)
                let scale_factor = scale_px * 0.02
                let resize = scale_factor * 0.2; // factor that works for all elements
                let outer_width = 10. * scale_factor
                let radius_px = (scale_px - outer_width) * 0.5;

                let center_px = vec2(
                    self.rect_size.x * 0.5,
                    radius_px + outer_width * 0.5 + label_offset_px
                )

                let offset_px = vec2(
                    center_px.x - radius_px,
                    label_offset_px
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_px = vec2(
                    radius_px * 2.,
                    radius_px * 2.
                )

                let gap_deg = self.gap * 0.25;
                let gap_rad = gap_deg * PI / 180;
                let arc_height_n = cos(gap_rad);
                let diam_px = radius_px * 2.;
                let arc_height_px = diam_px * arc_height_n

                let scale_border = vec2(
                    self.rect_size.x / border_sz_px.x,
                    self.rect_size.y / arc_height_px
                );

                let gradient_border = vec2(
                    self.pos.x * scale_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_border.y + dither
                )

                // Background
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    start,
                    outer_end,
                    outer_width
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.color,
                                self.color_hover,
                                self.hover
                            ),
                            mix(
                                self.color_focus,
                                mix(
                                    self.color_hover,
                                    self.color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                )

                let border_sz = self.border_size * 5. * resize;
                let gradient_down = pow(gradient_border.y, 2.)
                let gradient_up = pow(gradient_border.y, 0.5)

                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y + border_sz,
                    radius_px,
                    start,
                    outer_end, 
                    border_sz * 4.
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix((THEME_COLOR_U_HIDDEN), self.border_color_1, gradient_down),
                                mix((THEME_COLOR_U_HIDDEN), self.border_color_1_hover, gradient_down),
                                self.hover
                            ),
                            mix(
                                mix((THEME_COLOR_U_HIDDEN), self.border_color_1_focus, gradient_down),
                                mix(
                                    mix((THEME_COLOR_U_HIDDEN), self.border_color_1_hover, gradient_down),
                                    mix((THEME_COLOR_U_HIDDEN), self.border_color_1_drag, gradient_down),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix((THEME_COLOR_U_HIDDEN), self.border_color_1_disabled, gradient_down),
                        self.disabled
                    )
                );

                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y - border_sz,
                    radius_px,
                    start,
                    outer_end, 
                    border_sz * 4.
                );
                
                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_2, (THEME_COLOR_D_HIDDEN), gradient_up),
                                mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_up),
                                self.hover
                            ),
                            mix(
                                mix(self.border_color_2_focus, (THEME_COLOR_D_HIDDEN), gradient_up),
                                mix(
                                    mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_up),
                                    mix(self.border_color_2_drag, (THEME_COLOR_D_HIDDEN), gradient_up),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_2_disabled, (THEME_COLOR_D_HIDDEN), gradient_up),
                        self.disabled
                    )
                );

                // Track ridge
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    start,
                    outer_end, 
                    border_sz * 4.
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.border_color_2,
                                self.border_color_2_hover,
                                self.hover
                            ),
                            mix(
                                self.border_color_2_focus,
                                mix(
                                    self.border_color_2_hover,
                                    self.border_color_2_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.border_color_2_disabled,
                        self.disabled
                    )
                );

                let inner_width = outer_width - (self.val_padding * resize);

                // // Value
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    start,
                    val_end, 
                    inner_width
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.val_color_1, self.val_color_2, self.slide_pos),
                                mix(self.val_color_1_hover, self.val_color_2_hover, self.slide_pos),
                                self.hover
                            ),
                            mix(
                                mix(self.val_color_1_focus, self.val_color_2_focus, self.slide_pos),
                                mix(
                                mix(self.val_color_1_focus, self.val_color_2_hover, self.slide_pos),
                                mix(self.val_color_1_drag, self.val_color_2_drag, self.slide_pos),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                            mix(self.val_color_1_disabled, self.val_color_2_disabled, self.slide_pos),
                        self.disabled
                    )
                )

                // Handle
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    val_end, 
                    val_end, 
                    mix(
                        mix(0., inner_width, self.focus),
                        inner_width,
                        self.hover
                    )
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.handle_color,
                                self.handle_color_hover,
                                self.hover
                            ),
                            mix(
                                self.handle_color_focus,
                                mix(
                                    self.handle_color_hover,
                                    self.handle_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.handle_color_disabled,
                        self.disabled
                    )
                )

                // Bevel Outer
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px + outer_width * 0.5 - border_sz * 0.5,
                    start,
                    outer_end,
                    border_sz
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_2, (THEME_COLOR_D_HIDDEN), gradient_down),
                                mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_down),
                                self.hover
                            ),
                            mix(
                                mix(self.border_color_2_focus, (THEME_COLOR_D_HIDDEN), gradient_down),
                                mix(
                                    mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_down),
                                    mix(self.border_color_2_drag, (THEME_COLOR_D_HIDDEN), gradient_down),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_2_disabled, (THEME_COLOR_D_HIDDEN), gradient_down),
                        self.disabled
                    )
                );

                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px - outer_width * 0.5 - border_sz * 0.5,
                    start,
                    outer_end,
                    border_sz
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_1, (THEME_COLOR_U_HIDDEN), gradient_up),
                                mix(self.border_color_1_hover, (THEME_COLOR_U_HIDDEN), gradient_up),
                                self.hover
                            ),
                            mix(
                                mix(self.border_color_1_focus, (THEME_COLOR_U_HIDDEN), gradient_up),
                                mix(
                                    mix(self.border_color_1_hover, (THEME_COLOR_U_HIDDEN), gradient_up),
                                    mix(self.border_color_1_drag, (THEME_COLOR_U_HIDDEN), gradient_up),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_1_disabled, (THEME_COLOR_U_HIDDEN), gradient_up),
                        self.disabled
                    )
                );
                
                return sdf.result
            }
        }
    }

    pub RotaryGradientY = <Rotary> {
        draw_bg: {
            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1_HOVER)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)
            uniform color_1_disabled: (THEME_COLOR_INSET_1_DISABLED)
            uniform color_1_drag: (THEME_COLOR_INSET_1_DRAG)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2_HOVER)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
            uniform color_2_disabled: (THEME_COLOR_INSET_2_DISABLED)
            uniform color_2_drag: (THEME_COLOR_INSET_2_DRAG)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let one_deg = PI / 180;
                let threesixty_deg = 2. * PI;
                let gap_size = self.gap * one_deg;
                let val_length = threesixty_deg - (one_deg * self.gap);
                let start = gap_size * 0.5;
                let outer_end = start + val_length;
                let val_end = start + val_length * self.slide_pos;

                let label_offset_px = 20.;
                let label_offset_uv = self.rect_size.y;
                let scale_px = min(self.rect_size.x, self.rect_size.y)
                let scale_factor = scale_px * 0.02
                let resize = scale_factor * 0.2; // factor that works for all elements
                let outer_width = 10. * scale_factor
                let radius_px = (scale_px - outer_width) * 0.5;

                let center_px = vec2(
                    self.rect_size.x * 0.5,
                    radius_px + outer_width * 0.5 + label_offset_px
                )

                let offset_px = vec2(
                    center_px.x - radius_px,
                    label_offset_px
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_px = vec2(
                    radius_px * 2.,
                    radius_px * 2.
                )

                let gap_deg = self.gap * 0.25;
                let gap_rad = gap_deg * PI / 180;
                let arc_height_n = cos(gap_rad);
                let diam_px = radius_px * 2.;
                let arc_height_px = diam_px * arc_height_n

                let scale_border = vec2(
                    self.rect_size.x / border_sz_px.x,
                    self.rect_size.y / arc_height_px
                );

                let gradient_border = vec2(
                    self.pos.x * scale_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_border.y + dither
                )

                // Background
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    start,
                    outer_end,
                    outer_width
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, gradient_border.y),
                                mix(self.color_1_hover, self.color_2_hover, gradient_border.y),
                                self.hover
                            ),
                            mix(
                                mix(self.color_1_focus, self.color_2_focus, gradient_border.y),
                                mix(
                                    mix(self.color_1_hover, self.color_2_hover, gradient_border.y),
                                    mix(self.color_1_drag, self.color_2_drag, gradient_border.y),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_border.y),
                        self.disabled
                    )
                )

                let border_sz = self.border_size * 5. * resize;
                let gradient_down = pow(gradient_border.y, 2.)
                let gradient_up = pow(gradient_border.y, 0.5)

                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y + border_sz,
                    radius_px,
                    start,
                    outer_end, 
                    border_sz * 4.
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix((THEME_COLOR_U_HIDDEN), self.border_color_1, gradient_down),
                                mix((THEME_COLOR_U_HIDDEN), self.border_color_1_hover, gradient_down),
                                self.hover
                            ),
                            mix(
                                mix((THEME_COLOR_U_HIDDEN), self.border_color_1_focus, gradient_down),
                                mix(
                                    mix((THEME_COLOR_U_HIDDEN), self.border_color_1_hover, gradient_down),
                                    mix((THEME_COLOR_U_HIDDEN), self.border_color_1_drag, gradient_down),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix((THEME_COLOR_U_HIDDEN), self.border_color_1_disabled, gradient_down),
                        self.disabled
                    )
                );

                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y - border_sz,
                    radius_px,
                    start,
                    outer_end, 
                    border_sz * 4.
                );
                
                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_2, (THEME_COLOR_D_HIDDEN), gradient_up),
                                mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_up),
                                self.hover
                            ),
                            mix(
                                mix(self.border_color_2_focus, (THEME_COLOR_D_HIDDEN), gradient_up),
                                mix(
                                    mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_up),
                                    mix(self.border_color_2_drag, (THEME_COLOR_D_HIDDEN), gradient_up),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_2_disabled, (THEME_COLOR_D_HIDDEN), gradient_up),
                        self.disabled
                    )
                );

                // Track ridge
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    start,
                    outer_end, 
                    border_sz * 4.
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.border_color_2,
                                self.border_color_2_hover,
                                self.hover
                            ),
                            mix(
                                self.border_color_2_focus,
                                mix(
                                    self.border_color_2_hover,
                                    self.border_color_2_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.border_color_2_disabled,
                        self.disabled
                    )
                );

                let inner_width = outer_width - (self.val_padding * resize);

                // // Value
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    start,
                    val_end, 
                    inner_width
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.val_color_1, self.val_color_2, self.slide_pos),
                                mix(self.val_color_1_hover, self.val_color_2_hover, self.slide_pos),
                                self.hover
                            ),
                            mix(
                                mix(self.val_color_1_focus, self.val_color_2_focus, self.slide_pos),
                                mix(
                                mix(self.val_color_1_focus, self.val_color_2_hover, self.slide_pos),
                                mix(self.val_color_1_drag, self.val_color_2_drag, self.slide_pos),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                            mix(self.val_color_1_disabled, self.val_color_2_disabled, self.slide_pos),
                        self.disabled
                    )
                )

                // Handle
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px,
                    val_end, 
                    val_end, 
                    mix(
                        mix(0., inner_width, self.focus),
                        inner_width,
                        self.hover
                    )
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                self.handle_color,
                                self.handle_color_hover,
                                self.hover
                            ),
                            mix(
                                self.handle_color_focus,
                                mix(
                                    self.handle_color_hover,
                                    self.handle_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.handle_color_disabled,
                        self.disabled
                    )
                )

                // Bevel Outer
                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px + outer_width * 0.5 - border_sz * 0.5,
                    start,
                    outer_end,
                    border_sz
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_2, (THEME_COLOR_D_HIDDEN), gradient_down),
                                mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_down),
                                self.hover
                            ),
                            mix(
                                mix(self.border_color_2_focus, (THEME_COLOR_D_HIDDEN), gradient_down),
                                mix(
                                    mix(self.border_color_2_hover, (THEME_COLOR_D_HIDDEN), gradient_down),
                                    mix(self.border_color_2_drag, (THEME_COLOR_D_HIDDEN), gradient_down),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_2_disabled, (THEME_COLOR_D_HIDDEN), gradient_down),
                        self.disabled
                    )
                );

                sdf.arc_round_caps(
                    center_px.x,
                    center_px.y,
                    radius_px - outer_width * 0.5 - border_sz * 0.5,
                    start,
                    outer_end,
                    border_sz
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_1, (THEME_COLOR_U_HIDDEN), gradient_up),
                                mix(self.border_color_1_hover, (THEME_COLOR_U_HIDDEN), gradient_up),
                                self.hover
                            ),
                            mix(
                                mix(self.border_color_1_focus, (THEME_COLOR_U_HIDDEN), gradient_up),
                                mix(
                                    mix(self.border_color_1_hover, (THEME_COLOR_U_HIDDEN), gradient_up),
                                    mix(self.border_color_1_drag, (THEME_COLOR_U_HIDDEN), gradient_up),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.border_color_1_disabled, (THEME_COLOR_U_HIDDEN), gradient_up),
                        self.disabled
                    )
                );
                
                return sdf.result
            }
        }
    }

    pub RotaryFlat = <Rotary> {
        draw_bg: {
            uniform color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_disabled: (THEME_COLOR_INSET_DISABLED)
            uniform color_drag: (THEME_COLOR_INSET_DRAG)

            uniform border_color: (THEME_COLOR_BEVEL)
            uniform border_color_hover: (THEME_COLOR_BEVEL_HOVER)
            uniform border_color_focus: (THEME_COLOR_BEVEL_FOCUS)
            uniform border_color_disabled: (THEME_COLOR_BEVEL_DISABLED)
            uniform border_color_drag: (THEME_COLOR_BEVEL_DRAG)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                let label_offset = 20.;
                let one_deg = PI / 180;
                let threesixty_deg = 2. * PI;
                let gap_size = self.gap * one_deg;
                let val_length = threesixty_deg - (one_deg * self.gap);
                let start = gap_size * 0.5;
                let bg_end = start + val_length;
                let val_end = start + val_length * self.slide_pos;
                let effective_height = self.rect_size.y - label_offset;
                let radius_scaled = min(
                        (self.rect_size.x - self.border_size) * 0.5,
                        (self.rect_size.y - label_offset - self.border_size) * 0.5
                    );
                let radius_width_compensation = self.val_size * 0.5;
                let width_fix = 0.008;
                let bg_width_scaled = min(self.rect_size.x, effective_height) * self.val_size * width_fix;

                // Background
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation,
                    start,
                    bg_end, 
                    bg_width_scaled
                );

                let label_offset_uv = label_offset / self.rect_size.y;

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.color,
                                self.color_hover,
                                self.hover
                            ),
                            mix(
                                self.color_focus,
                                mix(
                                    self.color_hover,
                                    self.color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                )
                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                self.border_color,
                                self.border_color_hover,
                                self.hover
                            ),
                            mix(
                                self.border_color_focus,
                                mix(
                                    self.border_color_hover,
                                    self.border_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.border_color_disabled,
                        self.disabled
                    ), self.border_size);

                let val_size = (self.val_size - self.val_padding) * width_fix;
                let val_size_scaled = min(
                        self.rect_size.x * val_size,
                        effective_height * val_size
                    );

                // Value
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation,
                    start,
                    val_end, 
                    val_size_scaled - self.border_size
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(self.val_color_1, self.val_color_2, self.slide_pos),
                                mix(self.val_color_1_hover, self.val_color_2_hover, self.slide_pos),
                                self.hover
                            ),
                            mix(
                                mix(self.val_color_1_focus, self.val_color_2_focus, self.slide_pos),
                                mix(
                                    mix(self.val_color_1_hover, self.val_color_2_hover, self.slide_pos),
                                    mix(self.val_color_1_drag, self.val_color_2_drag, self.slide_pos),
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(self.val_color_1_disabled, self.val_color_2_disabled, self.slide_pos),
                        self.disabled
                    )
                )

                // Handle
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation,
                    val_end, 
                    val_end, 
                    mix(
                        mix(0., val_size_scaled, self.focus),
                        val_size_scaled,
                        self.hover
                    )
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                self.handle_color,
                                self.handle_color_hover,
                                self.hover
                            ),
                            mix(
                                self.handle_color_focus,
                                mix(
                                    self.handle_color_hover,
                                    self.handle_color_drag,
                                    self.drag
                                ),
                                self.hover
                            ),
                            self.focus
                        ),
                        self.handle_color_disabled,
                        self.disabled
                    )
                )
                
                return sdf.result
            }
        }
    }

    pub RotaryFlatter = <RotaryFlat> {
        draw_bg: {
            border_size: 0.,
        }
    }

}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum DragAxis {
    #[pick] Horizontal,
    Vertical
}

impl LiveHook for Slider{
    fn after_new_from_doc(&mut self, cx:&mut Cx){
        self.set_internal(self.default);
        self.update_text_input(cx);
    }
}


#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawSlider {
    #[deref] draw_super: DrawQuad,
    #[live] label_size: f32,
    #[live] slide_pos: f32,
}

#[derive(Live, Widget)]
#[designable]
pub struct Slider {
    #[area] #[redraw] #[live] draw_bg: DrawSlider,
    
    #[walk] walk: Walk,

    #[live(DragAxis::Horizontal)] pub axis: DragAxis,
    
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[rust] label_area: Area,
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] draw_text: DrawText,
    #[live] text: String,
    
    #[live] text_input: TextInput,
    
    #[live] precision: usize,
    
    #[live] min: f64,
    #[live] max: f64,
    #[live] step: f64,
    #[live] default: f64,
    
    #[live] bind: String,

    // Indicates if the label of the slider responds to hover events
    // The primary use case for this kind of emitted actions is for tooltips displaying
    // and it is turned on by default, since this component already consumes finger events
    #[live(true)] hover_actions_enabled: bool,
    
    #[rust] pub relative_value: f64,
    #[rust] pub dragging: Option<f64>,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum SliderAction {
    StartSlide,
    TextSlide(f64),
    Slide(f64),
    EndSlide,
    LabelHoverIn(Rect),
    LabelHoverOut,
    None
}

impl Slider {
    
    fn to_external(&self) -> f64 {
        let val = self.relative_value * (self.max - self.min);
        if self.step != 0.0{
            return (val / self.step).floor()* self.step + self.min
        }
        else{
            val  + self.min
        }
    }
    
    fn set_internal(&mut self, external: f64) -> bool {
        let old = self.relative_value;
        self.relative_value = (external - self.min) / (self.max - self.min);
        old != self.relative_value
    }
    
    pub fn update_text_input(&mut self, cx: &mut Cx) {
        let e = self.to_external();
        self.text_input.set_text(cx, &match self.precision{
            0=>format!("{:.0}",e),
            1=>format!("{:.1}",e),
            2=>format!("{:.2}",e),
            3=>format!("{:.3}",e),
            4=>format!("{:.4}",e),
            5=>format!("{:.5}",e),
            6=>format!("{:.6}",e),
            7=>format!("{:.7}",e),
            _=>format!("{}",e)
        });
        self.text_input.select_all(cx);
    }
    
    pub fn draw_walk_slider(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.slide_pos = self.relative_value as f32;
        self.draw_bg.begin(cx, walk, self.layout);
        
        if let Flow::Right = self.layout.flow{
            
            if let Some(mut dw) = cx.defer_walk(self.label_walk) {
                //, (self.value*100.0) as usize);
                let walk = self.text_input.walk(cx);
                let _ = self.text_input.draw_walk(cx, &mut Scope::empty(), walk);
        
                let label_walk = dw.resolve(cx);
                cx.begin_turtle(label_walk, Layout::default());
                self.draw_text.draw_walk(cx, label_walk, self.label_align, &self.text);
                cx.end_turtle_with_area(&mut self.label_area);
            }
        }
        else{
            let walk = self.text_input.walk(cx);
            let _ = self.text_input.draw_walk(cx, &mut Scope::empty(), walk);
            self.draw_text.draw_walk(cx, self.label_walk, self.label_align, &self.text);
        }
        
        self.draw_bg.end(cx);
    }

    pub fn value(&self) -> f64 {
        self.to_external()
    }

    pub fn set_value(&mut self, cx:&mut Cx, v: f64) {
        let prev_value = self.value();
        self.set_internal(v);
        if v != prev_value {
            self.update_text_input(cx);
        }
    }
    }

impl WidgetDesign for Slider{
    
}

impl Widget for Slider {
    fn set_disabled(&mut self, cx:&mut Cx, disabled:bool){
        self.animator_toggle(cx, disabled, Animate::Yes, id!(disabled.on), id!(disabled.off));
    }
                
    fn disabled(&self, cx:&Cx) -> bool {
        self.animator_in_state(cx, id!(disabled.on))
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        
        // alright lets match our designer against the slider backgdrop
        match event.hit_designer(cx, self.draw_bg.area()){
            HitDesigner::DesignerPick(_e)=>{
                cx.widget_action(uid, &scope.path, WidgetDesignAction::PickedBody)
            }
            _=>()
        }
        
        for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
            match action.as_widget_action().cast() {
                TextInputAction::KeyFocus => {
                    self.animator_play(cx, id!(focus.on));
                }
                TextInputAction::KeyFocusLost => {
                    self.animator_play(cx, id!(focus.off));
                }
                TextInputAction::Returned(value) => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.set_internal(v.max(self.min).min(self.max));
                    }
                    self.update_text_input(cx);
                    cx.widget_action(uid, &scope.path, SliderAction::TextSlide(self.to_external()));
                }
                TextInputAction::Escaped => {
                    self.update_text_input(cx);
                }
                _ => ()
            }
        };

        if self.hover_actions_enabled {
            match event.hits_with_capture_overload(cx, self.label_area, true) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, &scope.path, SliderAction::LabelHoverOut);
                },
                _ => ()
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                if self.animator.animator_in_state(cx, id!(disabled.on)) { return (); }
                self.animator_play(cx, id!(hover.on));
            },
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Grab);
            },
            Hit::FingerDown(FingerDownEvent {
                // abs,
                // rect,
                device,
                ..
            }) if device.is_primary_hit() => {
                if self.animator.animator_in_state(cx, id!(disabled.on)) { return (); }
                // cx.set_key_focus(self.slider.area());
                // self.relative_value = ((abs.x - rect.pos.x) / rect.size.x ).max(0.0).min(1.0);
                self.update_text_input(cx);

                self.text_input.set_is_read_only(cx, true);
                self.text_input.set_key_focus(cx);
                self.text_input.select_all(cx);
                self.text_input.redraw(cx);
                                
                self.animator_play(cx, id!(drag.on));
                self.dragging = Some(self.relative_value);
                cx.widget_action(uid, &scope.path, SliderAction::StartSlide);
                cx.set_cursor(MouseCursor::Grabbing);
            },
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if self.animator.animator_in_state(cx, id!(disabled.on)) { return (); }

                self.text_input.set_is_read_only(cx, false);
                // if the finger hasn't moved further than X we jump to edit-all on the text thing
                self.text_input.force_new_edit_group();
                self.animator_play(cx, id!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
                self.dragging = None;
                cx.widget_action(uid, &scope.path, SliderAction::EndSlide);
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerMove(fe) => {
                if self.animator.animator_in_state(cx, id!(disabled.on)) { return (); }

                let rel = fe.abs - fe.abs_start;
                if let Some(start_pos) = self.dragging {
                    if let DragAxis::Horizontal = self.axis {
                        self.relative_value = (start_pos + rel.x / (fe.rect.size.x - self.draw_bg.label_size as f64)).max(0.0).min(1.0);
                    } else {
                        self.relative_value = (start_pos - rel.y / fe.rect.size.y as f64).max(0.0).min(1.0);
                    }
                    self.set_internal(self.to_external());
                    self.draw_bg.redraw(cx);
                    self.update_text_input(cx);
                    cx.widget_action(uid, &scope.path, SliderAction::Slide(self.to_external()));
                }
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk_slider(cx, walk);
        DrawStep::done()
    }
    
    fn widget_to_data(&self, _cx: &mut Cx, actions:&Actions, nodes: &mut LiveNodeVec, path: &[LiveId])->bool{
        match actions.find_widget_action_cast(self.widget_uid()) {
            SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                nodes.write_field_value(path, LiveValue::Float64(v as f64));
                true
            }
            _ => false
        }
    }
    
    fn data_to_widget(&mut self, cx: &mut Cx, nodes:&[LiveNode], path: &[LiveId]){
        if let Some(value) = nodes.read_field_value(path) {
            if let Some(value) = value.as_float() {
                if self.set_internal(value) {
                    self.redraw(cx)
                }
                self.update_text_input(cx);
            }
        }
    }
    
    fn text(&self) -> String {
        format!("{}", self.to_external())
    }
        
    fn set_text(&mut self, cx:&mut Cx, v: &str) {
        if let Ok(v) = v.parse::<f64>(){
            self.set_internal(v);
            self.update_text_input(cx);
        }
    }
        
}

impl SliderRef{
    pub fn value(&self)->Option<f64> {
        if let Some(inner) = self.borrow(){
            return Some(inner.value())
        }

        return None
    }

    pub fn set_value(&self, cx:&mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v)
        }
    }
    
    pub fn slided(&self, actions:&Actions)->Option<f64>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::TextSlide(v) | SliderAction::Slide(v) => {
                    return Some(v)
                }
                _=>()
            }
        }
        None
    }

    pub fn label_hover_in(&self, actions:&Actions)->Option<Rect>{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::LabelHoverIn(rect) => Some(rect),
                _=> None
            }
        } else {
            None
        }
    }

    pub fn label_hover_out(&self, actions:&Actions)->bool{
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast(){
                SliderAction::LabelHoverOut => true,
                _=> false
            }
        } else {
            false
        }
    }
}