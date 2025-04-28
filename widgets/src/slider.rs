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
        label_align: { y: 0.0 }
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        precision: 2,
        height: Fit,
        hover_actions_enabled: false,
        
        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform border_color_1: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_LIGHT_FOCUS)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_LIGHT_DRAG)

            uniform border_color_2: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_SHADOW_FOCUS)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_SHADOW_DRAG)

            uniform val_color: (THEME_COLOR_VAL)
            uniform val_color_hover: (THEME_COLOR_VAL_HOVER)
            uniform val_color_focus: (THEME_COLOR_VAL_HOVER)
            uniform val_color_drag: (THEME_COLOR_VAL_ACTIVE)

            uniform handle_color: (THEME_COLOR_SLIDER_MINIMAL_HANDLE)
            uniform handle_color_hover: (THEME_COLOR_SLIDER_MINIMAL_HANDLE_HOVER)
            uniform handle_color_focus: (THEME_COLOR_SLIDER_MINIMAL_HANDLE_HOVER)
            uniform handle_color_drag: (THEME_COLOR_SLIDER_MINIMAL_HANDLE_ACTIVE)

            fn pixel(self) -> vec4 {
                let slider_height = 3;
                let handle_size = mix(3, 5, self.hover);
                let handle_bg_size = mix(0, 10, self.hover)

                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
 
                // Track shadow
                sdf.rect(0, self.rect_size.y - slider_height * 1.25, self.rect_size.x, slider_height)
                sdf.fill(
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
                    )
                );
                    
                // Track highlight
                sdf.rect(0, self.rect_size.y - slider_height * 0.5, self.rect_size.x, slider_height)
                sdf.fill(
                    mix(
                        mix(self.border_color_1, self.border_color_1_focus, self.focus),
                        mix(self.border_color_1_hover, self.border_color_1_drag, self.drag),
                        self.hover
                    )

                );
                    
                // Amount
                sdf.rect(
                    0,
                    self.rect_size.y - slider_height,
                    self.slide_pos * (self.rect_size.x) + handle_size,
                    slider_height
                )
                sdf.fill(mix(
                        mix(self.val_color, self.val_color_focus, self.focus),
                        mix(self.val_color_hover, self.val_color_drag, self.drag),
                        self.hover
                    )
                );
                    
                // Handle
                let handle_bg_x = self.slide_pos * (self.rect_size.x - handle_size) - handle_bg_size * 0.5 + 0.5 * handle_size;

                sdf.rect(
                    handle_bg_x,
                    self.rect_size.y - slider_height * 1.25,
                    handle_bg_size,
                    slider_height
                )

                sdf.fill(mix(
                        mix(self.handle_color, self.handle_color_focus, self.focus),
                        mix(self.handle_color_hover, self.handle_color_drag, self.drag),
                        self.hover
                    )
                );

                return sdf.result
            }
        }

        draw_text: {
            instance hover: 0.0,
            instance focus: 0.0,
            instance drag: 0.0,

            uniform color: (THEME_COLOR_TEXT)
            uniform color_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_focus: (THEME_COLOR_TEXT_FOCUS)
            uniform color_drag: (THEME_COLOR_TEXT_HOVER)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(self.color, self.color_focus, self.focus),
                    mix(self.color_hover, self.color_drag, self.drag),
                    self.hover
                )
            }

            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }

        }
            
        label_walk: {
            width: Fill, height: Fit,
            margin: {bottom: (THEME_SPACE_1)},
        }
            
        text_input: <TextInput> {
            empty_text: "0",
            is_numeric_only: true,
            is_read_only: false,

            width: Fit,
            padding: 0.,
            label_align: {y: 0.},
            margin: { bottom: (THEME_SPACE_2), left: (THEME_SPACE_2) }
            
            draw_bg: {
                border_radius: 1.
                border_size: (THEME_BEVELING)

                color_dither: 1.0

                color: (THEME_COLOR_U_HIDDEN)
                color_hover: (THEME_COLOR_U_HIDDEN)
                color_focus: (THEME_COLOR_U_HIDDEN)

                border_color_1: (THEME_COLOR_U_HIDDEN)
                border_color_1_hover: (THEME_COLOR_U_HIDDEN)
                border_color_1_focus: (THEME_COLOR_U_HIDDEN)

                border_color_2: (THEME_COLOR_U_HIDDEN)
                border_color_2_hover: (THEME_COLOR_U_HIDDEN)
                border_color_2_focus: (THEME_COLOR_U_HIDDEN)
            }

            draw_text: {
                color: (THEME_COLOR_TEXT)
                color_hover: (THEME_COLOR_TEXT)
                color_focus: (THEME_COLOR_TEXT)
                color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
                color_empty_focus: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)

                text_style: <THEME_FONT_REGULAR> {
                    font_size: (THEME_FONT_SIZE_P)
                }
            }

            draw_cursor: { color: (THEME_COLOR_TEXT_CURSOR) }

            draw_selection: {
                border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)

                color: (THEME_COLOR_D_HIDDEN)
                color_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
                color_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)
            }
        }
            
        animator: {
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
            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1)
            uniform color_1_focus: (THEME_COLOR_INSET_1)
            uniform color_1_drag: (THEME_COLOR_INSET_1)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2)
            uniform color_2_focus: (THEME_COLOR_INSET_2)
            uniform color_2_drag: (THEME_COLOR_INSET_2)

            uniform handle_color_1: (THEME_COLOR_SLIDER_HANDLE_1)
            uniform handle_color_1_hover: (THEME_COLOR_SLIDER_HANDLE_1_HOVER)
            uniform handle_color_1_focus: (THEME_COLOR_SLIDER_HANDLE_1)
            uniform handle_color_1_drag: (THEME_COLOR_SLIDER_HANDLE_1)

            uniform handle_color_2: (THEME_COLOR_SLIDER_HANDLE_2)
            uniform handle_color_2_hover: (THEME_COLOR_SLIDER_HANDLE_2_HOVER)
            uniform handle_color_2_focus: (THEME_COLOR_SLIDER_HANDLE_2)
            uniform handle_color_2_drag: (THEME_COLOR_SLIDER_HANDLE_2)

            uniform border_color_1: (THEME_COLOR_OUTSET)
            uniform border_color_1_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_OUTSET_FOCUS)
            uniform border_color_1_drag: (THEME_COLOR_OUTSET_DRAG)

            uniform border_color_2: (THEME_COLOR_OUTSET)
            uniform border_color_2_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_OUTSET_FOCUS)
            uniform border_color_2_drag: (THEME_COLOR_OUTSET_DRAG)

            uniform val_size: 3.

            uniform val_color: (THEME_COLOR_VAL)
            uniform val_color_hover: (THEME_COLOR_VAL_HOVER)
            uniform val_color_focus: (THEME_COLOR_VAL_HOVER)
            uniform val_color_drag: (THEME_COLOR_VAL_ACTIVE)

            uniform bipolar: 0.0,

        }

    }
        
    pub Slider = <SliderMinimal> {
        height: 36

        draw_bg: {
            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1)
            uniform color_1_focus: (THEME_COLOR_INSET_1)
            uniform color_1_drag: (THEME_COLOR_INSET_1)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2)
            uniform color_2_focus: (THEME_COLOR_INSET_2)
            uniform color_2_drag: (THEME_COLOR_INSET_2)

            uniform handle_color_1: (THEME_COLOR_SLIDER_HANDLE_1)
            uniform handle_color_1_hover: (THEME_COLOR_SLIDER_HANDLE_1_HOVER)
            uniform handle_color_1_focus: (THEME_COLOR_SLIDER_HANDLE_1)
            uniform handle_color_1_drag: (THEME_COLOR_SLIDER_HANDLE_1)

            uniform handle_color_2: (THEME_COLOR_SLIDER_HANDLE_2)
            uniform handle_color_2_hover: (THEME_COLOR_SLIDER_HANDLE_2_HOVER)
            uniform handle_color_2_focus: (THEME_COLOR_SLIDER_HANDLE_2)
            uniform handle_color_2_drag: (THEME_COLOR_SLIDER_HANDLE_2)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_LIGHT)

            uniform val_size: 3.

            uniform val_color: (THEME_COLOR_VAL)
            uniform val_color_hover: (THEME_COLOR_VAL_HOVER)
            uniform val_color_focus: (THEME_COLOR_VAL_HOVER)
            uniform val_color_drag: (THEME_COLOR_VAL_ACTIVE)

            uniform handle_size: 20.
            uniform bipolar: 0.0,

            fn pixel(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                let handle_sz = self.handle_size;
                    
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let top = 20.0;
                    
                sdf.box(1.0, top, self.rect_size.x - 2, self.rect_size.y - top - 2, self.border_radius);
                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                            mix(
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                mix(self.color_1_drag, self.color_2_drag, self.pos.y + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                )
                    
                sdf.stroke(
                    mix(
                        mix(self.border_color_1, self.border_color_2, pow(self.pos.y, 10.0) + dither),
                        mix(
                            mix(self.border_color_1_focus, self.border_color_2_focus, pow(self.pos.y, 10.0) + dither),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, pow(self.pos.y, 10.0) + dither),
                                mix(self.border_color_1_drag, self.border_color_2_drag, pow(self.pos.y, 10.0) + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    ), self.border_size
                )

                // Ridge
                let offset_sides = 6.0;
                let offset_top = 5.0;
                sdf.rect(1.0 + offset_sides, top + offset_top, self.rect_size.x - 2 - 2 * offset_sides, 3);

                sdf.fill(
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
                    )
                );

                let offset_top = 7.0;
                sdf.rect(1.0 + offset_sides, top + offset_top, self.rect_size.x - 2 - 2 * offset_sides, 1.5);
                sdf.fill(
                    mix(
                        mix(self.border_color_2, self.border_color_2_hover, self.hover),
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
                    )
                );
                    
                // Handle
                let track_length = self.rect_size.x - offset_sides * 4.;
                let val_x = self.slide_pos * track_length + offset_sides * 2.;
                sdf.move_to(mix(offset_sides + 3.5, self.rect_size.x * 0.5, self.bipolar), top + offset_top);
                sdf.line_to(val_x, top + offset_top);

                sdf.stroke(
                    mix(
                        mix(self.val_color, self.val_color_hover, self.hover),
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
                    ), self.val_size)
                    

                let ctrl_height = self.rect_size.y - top - 4;
                let handle_x = self.slide_pos * (self.rect_size.x - handle_sz - offset_sides) - 3;
                sdf.box(handle_x + offset_sides, top + 1.0, self.handle_size, ctrl_height, self.border_radius)
                    
                sdf.fill_keep( 
                    mix(
                        mix(
                            mix(self.handle_color_1, self.handle_color_2, pow(self.pos.y, 1.5) + dither),
                            mix(self.handle_color_1_hover, self.handle_color_2_hover, pow(self.pos.y, 1.5) + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.handle_color_1_focus, self.handle_color_2_focus, pow(self.pos.y, 1.5) + dither),
                            mix(
                                mix(self.handle_color_1_hover, self.handle_color_2_hover, pow(self.pos.y, 1.5) + dither),
                                mix(self.handle_color_1_drag, self.handle_color_2_drag, pow(self.pos.y, 1.5) + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                )
                
                sdf.stroke(
                    mix(
                        mix(self.border_color_2, self.border_color_1, pow(self.pos.y, 2.) + dither),
                        mix(
                            mix(self.border_color_2_focus, self.border_color_1_focus, pow(self.pos.y, 2.0) + dither),
                            mix(
                                mix(self.border_color_2_hover, self.border_color_1_hover, pow(self.pos.y, 2.0) + dither),
                                mix(self.border_color_2_drag, self.border_color_1_drag, pow(self.pos.y, 2.0) + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    ), self.border_size
                );
                
                return sdf.result
            }
        }
    }

    pub SliderFlat = <Slider> {
        draw_bg: {
            border_size: (THEME_BEVELING)
            border_radius: (THEME_CORNER_RADIUS)

            color_dither: 1.0

            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_focus: (THEME_COLOR_INSET_FOCUS)
            color_1_drag: (THEME_COLOR_INSET_DRAG)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_focus: (THEME_COLOR_INSET_FOCUS)
            color_2_drag: (THEME_COLOR_INSET_DRAG)

            handle_color_1: (THEME_COLOR_SLIDER_HANDLE)
            handle_color_1_hover: (THEME_COLOR_SLIDER_HANDLE_HOVER)
            handle_color_1_focus: (THEME_COLOR_SLIDER_HANDLE_FOCUS)
            handle_color_1_drag: (THEME_COLOR_SLIDER_HANDLE_DRAG)

            handle_color_2: (THEME_COLOR_SLIDER_HANDLE)
            handle_color_2_hover: (THEME_COLOR_SLIDER_HANDLE_HOVER)
            handle_color_2_focus: (THEME_COLOR_SLIDER_HANDLE_FOCUS)
            handle_color_2_drag: (THEME_COLOR_SLIDER_HANDLE_DRAG)

            border_color_1: (THEME_COLOR_OUTSET)
            border_color_1_hover: (THEME_COLOR_OUTSET_HOVER)
            border_color_1_focus: (THEME_COLOR_OUTSET_FOCUS)
            border_color_1_drag: (THEME_COLOR_OUTSET_DRAG)

            border_color_2: (THEME_COLOR_OUTSET)
            border_color_2_hover: (THEME_COLOR_OUTSET_HOVER)
            border_color_2_focus: (THEME_COLOR_OUTSET_FOCUS)
            border_color_2_drag: (THEME_COLOR_OUTSET_DRAG)

            handle_size: 14.

            val_size: 2.

            val_color: (THEME_COLOR_VAL)
            val_color_hover: (THEME_COLOR_VAL_HOVER)
            val_color_focus: (THEME_COLOR_VAL_FOCUS)
            val_color_drag: (THEME_COLOR_VAL_ACTIVE)

            bipolar: 0.0,
        }

    }

    pub SliderFlatter = <SliderFlat> {
        draw_bg: {
            handle_size: 0.
            border_size: 0.
        }
    }


    pub SLIDER_ALT1_HANDLE_SIZE = 4.0;
    pub SLIDER_ALT1_LABEL_SIZE = 75.0;
    pub SLIDER_ALT1_DATA_FONT_TOPMARGIN = 3.0;
    pub SLIDER_ALT1_VAL_PADDING = 2.5;
    pub SLIDER_ALT1_VAL_COLOR_A = (THEME_COLOR_VAL * 0.8);
    pub SLIDER_ALT1_VAL_COLOR_B = (THEME_COLOR_VAL * 1.4);
    pub SLIDER_ALT1_HANDLE_COLOR_A = (THEME_COLOR_SLIDER_MINIMAL_HANDLE);

    pub SliderRound = <SliderMinimal> {
        height: 18.,
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }

        text_input: <TextInput> {
            width: Fit,
            padding: 0.,
            margin: { right: 7.5, top: (SLIDER_ALT1_DATA_FONT_TOPMARGIN) } 

            draw_text: {
                text_style: <THEME_FONT_REGULAR> {
                    font_size: (SLIDER_ALT1_DATA_FONTSIZE)
                }
            }
        }

        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float

            label_size: (SLIDER_ALT1_LABEL_SIZE);

            uniform val_heat: 10.

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS * 2.)

            uniform color_dither: 1.0
            
            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1)
            uniform color_1_focus: (THEME_COLOR_INSET_1)
            uniform color_1_drag: (THEME_COLOR_INSET_1)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2)
            uniform color_2_focus: (THEME_COLOR_INSET_2)
            uniform color_2_drag: (THEME_COLOR_INSET_2)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_LIGHT)

            uniform val_color_1: (SLIDER_ALT1_VAL_COLOR_A)
            uniform val_color_1_hover: (SLIDER_ALT1_VAL_COLOR_A)
            uniform val_color_1_focus: (SLIDER_ALT1_VAL_COLOR_A)
            uniform val_color_1_drag: (SLIDER_ALT1_VAL_COLOR_A)

            uniform val_color_2: (SLIDER_ALT1_VAL_COLOR_B)
            uniform val_color_2_hover: (SLIDER_ALT1_VAL_COLOR_B)
            uniform val_color_2_focus: (SLIDER_ALT1_VAL_COLOR_B)
            uniform val_color_2_drag: (SLIDER_ALT1_VAL_COLOR_B)

            uniform handle_color: (SLIDER_ALT1_HANDLE_COLOR_A);
            uniform handle_color_hover: (SLIDER_ALT1_HANDLE_COLOR_A);
            uniform handle_color_focus: (SLIDER_ALT1_HANDLE_COLOR_A);
            uniform handle_color_drag: (THEME_COLOR_W);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let handle_size = (SLIDER_ALT1_HANDLE_SIZE);
                let padding = (SLIDER_ALT1_VAL_PADDING);

                let track_length_bg = self.rect_size.x - self.label_size;
                let padding_full = padding * 2.;
                let min_size = padding_full + handle_size * 2.;
                let track_length_val = self.rect_size.x - self.label_size - padding_full - min_size;

                // Background
                sdf.box(
                    self.label_size,
                    0.0,
                    track_length_bg,
                    self.rect_size.y,
                    self.border_radius
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                            mix(
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                mix(self.color_1_drag, self.color_2_drag, self.pos.y + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                )

                sdf.stroke(
                    mix(
                        mix(self.border_color_1, self.border_color_2, pow(self.pos.y, self.val_heat) + dither),
                        mix(
                            mix(self.border_color_1_focus, self.border_color_2_focus, pow(self.pos.y, self.val_heat) + dither),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, pow(self.pos.y, self.val_heat) + dither),
                                mix(self.border_color_1_drag, self.border_color_2_drag, pow(self.pos.y, self.val_heat) + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    ), self.border_size * 1.5
                )

                // Amount bar
                sdf.box(
                    self.label_size + padding,
                    padding,
                    track_length_val * self.slide_pos + min_size,
                    self.rect_size.y - padding_full,
                    self.border_radius * 0.75
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(self.val_color_1, self.val_color_2, pow(self.pos.x, 10.0) + dither),
                            mix(self.val_color_1_hover, self.val_color_2_hover, pow(self.pos.x, 10.0) + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.val_color_1_focus, self.val_color_2_focus, pow(self.pos.x, 10.0) + dither),
                            mix(
                                mix(self.val_color_1_hover, self.val_color_2_hover, pow(self.pos.x, 10) + dither),
                                mix(self.val_color_1_drag, self.val_color_2_drag, pow(self.pos.x, 10.0) + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                )

                // Handle
                let handle_shift = self.label_size + padding_full + handle_size;

                sdf.circle(
                    track_length_val * self.slide_pos + handle_shift,
                    self.rect_size.y * 0.5,
                    mix(0., handle_size, self.hover)
                );

                sdf.fill_keep(
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
                    )
                )
                
                return sdf.result
            }
        }

    }

    pub SliderRoundFlat = <SliderRound> {
        draw_bg: {
            label_size: (SLIDER_ALT1_LABEL_SIZE);

            val_heat: 10.

            border_size: (THEME_BEVELING)
            border_radius: (THEME_CORNER_RADIUS * 2.)

            color_dither: 1.0
            
            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_focus: (THEME_COLOR_INSET_FOCUS)
            color_1_drag: (THEME_COLOR_INSET_DRAG)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_focus: (THEME_COLOR_INSET_FOCUS)
            color_2_drag: (THEME_COLOR_INSET_DRAG)

            border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)
            border_color_1_drag: (THEME_COLOR_BEVEL_SHADOW)

            border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)
            border_color_2_drag: (THEME_COLOR_BEVEL_LIGHT)

            val_color_1: (SLIDER_ALT1_VAL_COLOR_A)
            val_color_1_hover: (SLIDER_ALT1_VAL_COLOR_A)
            val_color_1_focus: (SLIDER_ALT1_VAL_COLOR_A)
            val_color_1_drag: (SLIDER_ALT1_VAL_COLOR_A)

            val_color_2: (SLIDER_ALT1_VAL_COLOR_B)
            val_color_2_hover: (SLIDER_ALT1_VAL_COLOR_B)
            val_color_2_focus: (SLIDER_ALT1_VAL_COLOR_B)
            val_color_2_drag: (SLIDER_ALT1_VAL_COLOR_B)

            handle_color: (SLIDER_ALT1_HANDLE_COLOR_A);
            handle_color_hover: (SLIDER_ALT1_HANDLE_COLOR_A);
            handle_color_focus: (SLIDER_ALT1_HANDLE_COLOR_A);
            handle_color_drag: (THEME_COLOR_W);
        }
    }

    pub SliderRoundFlatter = <SliderRoundFlat> {
        draw_bg: {
            border_size: 0.
        }
    }

    pub ROTARY_BG_COLOR_A = (THEME_COLOR_BG_CONTAINER);
    pub ROTARY_BG_HOVER_COLOR_A = (THEME_COLOR_BG_CONTAINER);
    pub ROTARY_BG_DRAG_COLOR_A = (THEME_COLOR_BG_CONTAINER * 1.25);
    pub ROTARY_BG_COLOR_B = (THEME_COLOR_D_2);
    pub ROTARY_BG_HOVER_COLOR_B = (THEME_COLOR_D_2);
    pub ROTARY_BG_DRAG_COLOR_B = (THEME_COLOR_D_2);
    pub ROTARY_VAL_COLOR_A = (THEME_COLOR_U_4_OPAQUE);
    pub ROTARY_VAL_COLOR_B = (THEME_COLOR_U_2_OPAQUE);
    pub ROTARY_HANDLE_COLOR = (THEME_COLOR_U_3);

    pub Rotary = <SliderMinimal> {
        height: 95., width: 65.,
        axis: Vertical,

        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform gap: 90.
            uniform val_padding: 3.0

            uniform border_size: (THEME_BEVELING)
            uniform val_size: 20.

            uniform color_dither: 1.,
            
            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1)
            uniform color_1_focus: (THEME_COLOR_INSET_1)
            uniform color_1_drag: (THEME_COLOR_INSET_1)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2)
            uniform color_2_focus: (THEME_COLOR_INSET_2)
            uniform color_2_drag: (THEME_COLOR_INSET_2)

            uniform border_color_1: (THEME_COLOR_OUTSET)
            uniform border_color_1_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform border_color_1_focus: (THEME_COLOR_OUTSET_FOCUS)
            uniform border_color_1_drag: (THEME_COLOR_OUTSET_DRAG)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT * 1.3)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT * 1.2)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_LIGHT * 1.3)

            uniform handle_color: (ROTARY_HANDLE_COLOR);
            uniform handle_color_hover: (THEME_COLOR_U_4);
            uniform handle_color_focus: (THEME_COLOR_U_3);
            uniform handle_color_drag: (THEME_COLOR_W);

            uniform val_color_1: (ROTARY_VAL_COLOR_A);
            uniform val_color_1_hover: (ROTARY_VAL_COLOR_A);
            uniform val_color_1_focus: (ROTARY_VAL_COLOR_A);
            uniform val_color_1_drag: (ROTARY_VAL_COLOR_A);

            uniform val_color_2: (ROTARY_VAL_COLOR_B);
            uniform val_color_2_hover: (ROTARY_VAL_COLOR_B);
            uniform val_color_2_focus: (ROTARY_VAL_COLOR_B);
            uniform val_color_2_drag: (ROTARY_VAL_COLOR_B);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

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

                let label_offset_norm = label_offset / self.rect_size.y;
                let arc_h_norm = (360. - self.gap) / 360.; // approximation
                let rotary_h = radius_scaled * 2. / self.rect_size.y * arc_h_norm;
                let gradient_y = pow(self.pos.y, 2.) / rotary_h - label_offset_norm;

                sdf.fill(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                            mix(
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                mix(self.color_1_drag, self.color_2_drag, self.pos.y + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                )

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation,
                    start,
                    bg_end, 
                    bg_width_scaled * 0.1
                );

                // Track ridge
                sdf.fill(
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
                    )
                );

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset + 1.,
                    radius_scaled - radius_width_compensation,
                    start,
                    bg_end, 
                    bg_width_scaled * 0.1
                );

                sdf.fill(
                    mix(
                        mix(
                            self.border_color_1,
                            self.border_color_1_hover,
                            self.hover
                        ),
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
                    )
                );


                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset - 2.,
                    radius_scaled - radius_width_compensation,
                    start,
                    bg_end, 
                    bg_width_scaled * 0.1
                );

                sdf.fill(
                    mix(
                        mix(
                            self.border_color_1,
                            self.border_color_1_hover,
                            self.hover
                        ),
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
                    )
                );

                // outer rim
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation + bg_width_scaled * 0.5,
                    start,
                    bg_end, 
                    radius_scaled * 0.035
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(self.border_color_1, self.border_color_2, gradient_y),
                            mix(self.border_color_1_hover, self.border_color_2_hover, gradient_y),
                            self.hover
                        ),
                        mix(
                            mix(self.border_color_1_focus, self.border_color_2_focus, gradient_y),
                            mix(
                            mix(self.border_color_1_hover, self.border_color_2_hover, gradient_y),
                            mix(self.border_color_1_drag, self.border_color_2_drag, gradient_y),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                )

                // inner rim
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation - bg_width_scaled * 0.5,
                    start,
                    bg_end, 
                    radius_scaled * 0.075
                );

                let gradient_y_inner = gradient_y + label_offset_norm * 2.;

                sdf.fill(
                    mix(
                        mix(
                            mix(self.border_color_2, #0000, gradient_y_inner),
                            mix(self.border_color_2_hover, #0000, gradient_y_inner),
                            self.hover
                        ),
                        mix(
                            mix(self.border_color_2_focus, #0000, gradient_y_inner),
                            mix(
                            mix(self.border_color_2_hover, #0000, gradient_y_inner),
                            mix(self.border_color_2_drag, #0000, gradient_y_inner),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                );

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
                    val_size_scaled
                );

                sdf.fill(
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
                    )
                )
                
                return sdf.result
            }
        }
    }

    pub ROTARY_FLAT_BG_COLOR = (THEME_COLOR_D_HIDDEN);
    pub ROTARY_FLAT_BG_HOVER_COLOR = (THEME_COLOR_U_1);
    pub ROTARY_FLAT_BG_FOCUS_COLOR = (THEME_COLOR_D_2);
    pub ROTARY_FLAT_BG_DRAG_COLOR = (THEME_COLOR_D_1);

    pub ROTARY_FLAT_BORDER_COLOR = (THEME_COLOR_BEVEL_SHADOW);
    pub ROTARY_FLAT_BORDER_HOVER_COLOR = (THEME_COLOR_D_3);
    pub ROTARY_FLAT_BORDER_FOCUS_COLOR = (THEME_COLOR_D_3);
    pub ROTARY_FLAT_BORDER_DRAG_COLOR = (THEME_COLOR_D_4);
    pub ROTARY_FLAT_VAL_COLOR_A = (THEME_COLOR_U_2);
    pub ROTARY_FLAT_VAL_COLOR_B = (THEME_COLOR_U_4);
    pub ROTARY_FLAT_HANDLE_COLOR = (THEME_COLOR_U_3);

    pub RotaryFlat = <Rotary> {
        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform val_size: 20. // TODO: REMOVE / CHANGE?
            uniform val_padding: 2.0
            
            uniform color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_drag: (THEME_COLOR_INSET_DRAG)

            uniform border_color: (ROTARY_FLAT_BORDER_COLOR)
            uniform border_color_hover: (ROTARY_FLAT_BORDER_HOVER_COLOR)
            uniform border_color_focus: (ROTARY_FLAT_BORDER_FOCUS_COLOR)
            uniform border_color_drag: (ROTARY_FLAT_BORDER_DRAG_COLOR)

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

                let label_offset_norm = label_offset / self.rect_size.y;
                let arc_h_norm = (360. - self.gap) / 360.; // approximation
                let rotary_flat_h = radius_scaled * 2. / self.rect_size.y * arc_h_norm;
                let gradient_y = pow(self.pos.y, 2.) / rotary_flat_h - label_offset_norm;

                sdf.fill_keep(
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
                    )
                )
                sdf.stroke(
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
                    )
                    , self.border_size);

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
                    val_size_scaled
                );

                sdf.fill(
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
                    )
                )
                
                return sdf.result
            }
        }
    }

    pub RotaryFlatter = <RotaryFlat> {
        draw_bg: {
            border_size: 0.,
            color: (THEME_COLOR_INSET)
            color_hover: (THEME_COLOR_INSET_HOVER)
            color_focus: (THEME_COLOR_INSET_FOCUS)
            color_drag: (THEME_COLOR_INSET_DRAG)
        }
    }

    pub ROTARY_SOLID_BG_COLOR_A = (THEME_COLOR_U_1);
    pub ROTARY_SOLID_BG_COLOR_B = (THEME_COLOR_BLACK);
    pub ROTARY_SOLID_HANDLE_COLOR = #FFA;

    pub RotarySolid = <Rotary> {
        draw_bg: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform border_size: (THEME_BEVELING)

            uniform gap: 90.
            uniform width: 10.

            uniform color_dither: 1.0

            uniform color_1: (ROTARY_SOLID_BG_COLOR_A)
            uniform color_1_hover: (ROTARY_SOLID_BG_COLOR_A)
            uniform color_1_focus: (ROTARY_SOLID_BG_COLOR_A)
            uniform color_1_drag: (ROTARY_SOLID_BG_COLOR_A * 0.75)

            uniform color_2: (ROTARY_SOLID_BG_COLOR_B)
            uniform color_2_hover: (ROTARY_SOLID_BG_COLOR_B)
            uniform color_2_focus: (ROTARY_SOLID_BG_COLOR_B)
            uniform color_2_drag: (ROTARY_SOLID_BG_COLOR_B)

            uniform border_color_1: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT * 1.3)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_LIGHT * 1.15)
            uniform border_color_1_drag: (THEME_COLOR_BEVEL_LIGHT)

            uniform border_color_2: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW * 1.3)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_SHADOW * 1.15)
            uniform border_color_2_drag: (THEME_COLOR_BEVEL_SHADOW * 1.3)

            uniform handle_color: (ROTARY_SOLID_HANDLE_COLOR);
            uniform handle_color_hover: (ROTARY_FLAT_HANDLE_COLOR * 1.5);
            uniform handle_color_focus: (THEME_COLOR_W);
            uniform handle_color_drag: (THEME_COLOR_W);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let label_offset = 20.;
                let gloss_width = 1.;

                let one_deg = PI / 180;
                let threesixty_deg = 2. * PI;
                let gap_size = self.gap * one_deg;
                let val_length = threesixty_deg - (one_deg * self.gap);
                let start = gap_size * 0.5;
                let bg_end = start + val_length;
                let val_end = start + val_length * self.slide_pos;
                let effective_height = self.rect_size.y - label_offset;
                let radius_scaled = min(
                        (self.rect_size.x - gloss_width) * 0.5,
                        (self.rect_size.y - label_offset - gloss_width) * 0.5
                    );
                let radius_width_compensation = self.val_size * 0.5;
                let width_fix = 0.006;
                let bg_width_scaled = min(self.rect_size.x, effective_height) * self.val_size * width_fix;

                // Background
                sdf.circle(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation
                );

                let label_offset_norm = label_offset / self.rect_size.y;
                let arc_h_norm = (360. - self.gap) / 360.; // approximation
                let rotary_solid_h = radius_scaled * 2. / self.rect_size.y * arc_h_norm;
                let gradient_y = self.pos.y;

                let texture = Math::random_2d(self.pos.xy);

                sdf.fill(
                    mix(
                        mix(
                            mix(self.color_1 * texture, self.color_2 * texture, self.pos.y + dither),
                            mix(self.color_1_hover * texture, self.color_2_hover * texture, self.pos.y + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_focus * texture, self.color_2_focus * texture, self.pos.y + dither),
                            mix(
                                mix(self.color_1_hover * texture, self.color_2_hover * texture, self.pos.y + dither),
                                mix(self.color_1_drag * texture, self.color_2_drag * texture, self.pos.y + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                )

                // outer rim
                sdf.circle(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation
                );

                sdf.stroke(
                    mix(
                        mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                        mix(
                            mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                mix(self.border_color_1_drag, self.border_color_2_drag, self.pos.y + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    ), self.border_size
                )

                // inner rim
                sdf.circle(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation - bg_width_scaled * 0.6
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                            mix(
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                mix(self.color_2_drag, self.color_1_drag, self.pos.y + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    )
                );

                sdf.stroke(
                    mix(
                        mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                        mix(
                            mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                mix(self.border_color_1_drag, self.border_color_2_drag, pow(self.pos.y, 2.) + dither),
                                self.drag
                            ),
                            self.hover
                        ),
                        self.focus
                    ), self.border_size * mix(self.border_size, self.border_size * 1.5, self.drag)
                )

                let val_size = self.val_size * 0.004;
                let val_size_scaled = min(
                        self.rect_size.x * val_size,
                        effective_height * val_size
                    );
                let markings_width_scaled = min(
                        self.rect_size.x * 0.05,
                        effective_height * 0.05
                    );

                // Handle
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation - bg_width_scaled * 1.3,
                    val_end, 
                    val_end, 
                    val_size_scaled * mix(1.0, 1.5, self.drag)
                );

                sdf.fill(
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
                    )
                )

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled * 1.1,
                    start, 
                    start, 
                    mix(markings_width_scaled * 0.5, markings_width_scaled, self.hover)
                );

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled * 1.1,
                    bg_end, 
                    bg_end, 
                    mix(markings_width_scaled * 0.5, markings_width_scaled, self.hover)
                );

                sdf.fill(
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
                    )
                )
                
                return sdf.result
            }
        }
    }


    pub RotarySolidFlat = <RotarySolid> {
        draw_bg: {
            border_size: (THEME_BEVELING)

            gap: 90.
            width: 10.

            color_dither: 1.0

            color_1: (THEME_COLOR_OUTSET)
            color_1_hover: (THEME_COLOR_OUTSET_HOVER)
            color_1_focus: (THEME_COLOR_OUTSET_FOCUS)
            color_1_drag: (THEME_COLOR_OUTSET_DRAG)

            color_2: (THEME_COLOR_OUTSET)
            color_2_hover: (THEME_COLOR_OUTSET_HOVER)
            color_2_focus: (THEME_COLOR_OUTSET_FOCUS)
            color_2_drag: (THEME_COLOR_OUTSET_DRAG)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_1_drag: (THEME_COLOR_BEVEL_DRAG)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_2_drag: (THEME_COLOR_BEVEL_DRAG)

            handle_color: (ROTARY_SOLID_HANDLE_COLOR);
            handle_color_hover: (ROTARY_FLAT_HANDLE_COLOR * 1.5);
            handle_color_focus: (THEME_COLOR_W);
            handle_color_drag: (THEME_COLOR_W);

        }
    }


}

#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum SliderType {
    #[pick] Horizontal = shader_enum(1),
    Vertical = shader_enum(2),
    Rotary = shader_enum(3),
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
    #[live] slide_posr_type: SliderType
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
        self.text_input.set_text(cx, match self.precision{
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
        
        if let Some(mut dw) = cx.defer_walk(self.label_walk) {
            //, (self.value*100.0) as usize);
            let walk = self.text_input.walk(cx);
            let mut scope = Scope::default();
            let _ = self.text_input.draw_walk(cx, &mut scope, walk);

            let label_walk = dw.resolve(cx);
            cx.begin_turtle(label_walk, Layout::default());
            self.draw_text.draw_walk(cx, label_walk, self.label_align, &self.text);
            cx.end_turtle_with_area(&mut self.label_area);
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