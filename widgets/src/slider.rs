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
    
    pub Slider = <SliderBase> {
        min: 0.0, max: 1.0,
        step: 0.0,
        label_align: { y: 0.0 }
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        precision: 2,
        height: Fit,
        hover_actions_enabled: false,
        
        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float
            
            fn pixel(self) -> vec4 {
                let slider_height = 3;
                let nub_size = mix(3, 5, self.hover);
                let nubbg_size = mix(0, 13, self.hover)
                
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                
                let slider_bg_color = mix(mix(THEME_COLOR_AMOUNT_TRACK_DEFAULT, THEME_COLOR_AMOUNT_TRACK_HOVER, self.hover), THEME_COLOR_AMOUNT_TRACK_ACTIVE, self.focus);
                let slider_color = mix(
                    mix(THEME_COLOR_AMOUNT_DEFAULT, THEME_COLOR_AMOUNT_HOVER, self.hover),
                THEME_COLOR_AMOUNT_ACTIVE,
                self.focus);
                    
                let nub_color = (THEME_COLOR_SLIDER_NUB_DEFAULT);
                let nubbg_color = mix(THEME_COLOR_SLIDER_NUB_HOVER, THEME_COLOR_SLIDER_NUB_ACTIVE, self.drag);
                    
                sdf.rect(0, self.rect_size.y - slider_height * 1.25, self.rect_size.x, slider_height)
                sdf.fill(slider_bg_color);
                    
                sdf.rect(0, self.rect_size.y - slider_height * 0.5, self.rect_size.x, slider_height)
                sdf.fill(THEME_COLOR_BEVEL_LIGHT);
                    
                sdf.rect(
                    0,
                    self.rect_size.y - slider_height * 1.25,
                    self.slide_pos * (self.rect_size.x - nub_size) + nub_size,
                    slider_height
                )
                sdf.fill(slider_color);
                    
                let nubbg_x = self.slide_pos * (self.rect_size.x - nub_size) - nubbg_size * 0.5 + 0.5 * nub_size;
                sdf.rect(
                    nubbg_x,
                    self.rect_size.y - slider_height * 1.25,
                    nubbg_size,
                    slider_height
                )
                sdf.fill(nubbg_color);
                    
                // the nub
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size);
                sdf.rect(
                    nub_x,
                    self.rect_size.y - slider_height * 1.25,
                    nub_size,
                    slider_height
                )
                sdf.fill(nub_color);
                return sdf.result
            }
        }
            
        draw_text: {
            color: (THEME_COLOR_TEXT_DEFAULT),
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
            
        label_walk: { width: Fill, height: Fit }
            
        text_input: <TextInput> {
            width: Fit, padding: 0.,
            empty_message: "0",
            is_numeric_only: true,
                
            label_align: {y: 0.},
            margin: { bottom: (THEME_SPACE_2), left: (THEME_SPACE_2) }
            draw_bg: {
                instance radius: 1.0
                instance border_width: 0.0
                instance border_color: (#f00) // TODO: This appears not to do anything.
                instance inset: vec4(0.0, 0.0, 0.0, 0.0)
                instance focus: 0.0,
                color: (THEME_COLOR_D_HIDDEN)
                instance color_selected: (THEME_COLOR_D_HIDDEN)
                    
                fn get_color(self) -> vec4 {
                    return mix(self.color, self.color_selected, self.focus)
                }
                    
                fn get_border_color(self) -> vec4 {
                    return self.border_color
                }
                    
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                        max(1.0, self.radius)
                    )
                    sdf.fill_keep(self.get_color())
                    if self.border_width > 0.0 {
                        sdf.stroke(self.get_border_color(), self.border_width)
                    }
                    return sdf.result;
                }
            },
        }
            
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: OutQuad
                    apply: {
                        draw_slider: { hover: 0.0 },
                        // draw_text: { hover: 0.0 }
                        // text_input: { draw_bg: { hover: 0.0}}
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: { hover: 1.0 },
                        // draw_text: { hover: 1.0 }
                        // text_input: { draw_bg: { hover: 1.0}}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_slider: {drag: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {draw_slider: {drag: 1.0}}
                }
            }
        }
    }
        
    pub SliderBig = <Slider> {
        height: 36
        text: "CutOff1"
        // draw_text: {text_style: <H2_TEXT_BOLD> {}, color: (COLOR_UP_5)}
        text_input: {
            // cursor_margin_bottom: (THEME_SPACE_1),
            // cursor_margin_top: (THEME_SPACE_1),
            // select_pad_edges: (THEME_SPACE_1),
            // cursor_size: (THEME_SPACE_1),
            empty_message: "0",
            is_numeric_only: true,
            draw_bg: {
                color: (THEME_COLOR_D_HIDDEN)
            },
        }
        draw_slider: {
            instance line_color: (THEME_COLOR_AMOUNT_DEFAULT_BIG),
            instance bipolar: 0.0,

            fn pixel(self) -> vec4 {
                let nub_size = 3
                    
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let top = 20.0;
                    
                sdf.box(1.0, top, self.rect_size.x - 2, self.rect_size.y - top - 2, 1);
                sdf.fill_keep(
                    mix(
                        mix((THEME_COLOR_INSET_PIT_TOP), (THEME_COLOR_INSET_PIT_BOTTOM) * 0.1, pow(self.pos.y, 1.0)),
                        mix((THEME_COLOR_INSET_PIT_TOP_HOVER) * 1.75, (THEME_COLOR_BEVEL_LIGHT) * 0.1, pow(self.pos.y, 1.0)),
                        self.drag
                    )
                ) // Control backdrop gradient
                    
                sdf.stroke(mix(mix(THEME_COLOR_BEVEL_SHADOW, THEME_COLOR_BEVEL_SHADOW * 1.25, self.drag), THEME_COLOR_BEVEL_LIGHT, pow(self.pos.y, 10.0)), 1.0) // Control outline
                let in_side = 5.0;
                let in_top = 5.0; // Ridge: vertical position
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 3);
                sdf.fill(mix(THEME_COLOR_AMOUNT_TRACK_DEFAULT, THEME_COLOR_AMOUNT_TRACK_ACTIVE, self.drag)); // Ridge color
                let in_top = 7.0;
                sdf.rect(1.0 + in_side, top + in_top, self.rect_size.x - 2 - 2 * in_side, 1.5);
                sdf.fill(THEME_COLOR_BEVEL_LIGHT); // Ridge: Rim light catcher
                    
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 9);
                sdf.move_to(mix(in_side + 3.5, self.rect_size.x * 0.5, self.bipolar), top + in_top);
                    
                sdf.line_to(nub_x + in_side + nub_size * 0.5, top + in_top);
                sdf.stroke_keep(mix((THEME_COLOR_U_HIDDEN), self.line_color, self.drag), 1.5)
                sdf.stroke(
                    mix(mix(self.line_color * 0.85, self.line_color, self.hover), THEME_COLOR_AMOUNT_ACTIVE, self.drag),
                    1.5
                )
                    
                let nub_x = self.slide_pos * (self.rect_size.x - nub_size - in_side * 2 - 3) - 3;
                sdf.box(nub_x + in_side, top + 1.0, 11, 11, 1.)
                    
                sdf.fill_keep(mix(
                    mix(
                        mix(THEME_COLOR_SLIDER_BIG_NUB_TOP, THEME_COLOR_SLIDER_BIG_NUB_TOP_HOVER, self.hover),
                        mix(THEME_COLOR_SLIDER_BIG_NUB_BOTTOM, THEME_COLOR_SLIDER_BIG_NUB_BOTTOM_HOVER, self.hover),
                        self.pos.y
                    ),
                    mix(THEME_COLOR_SLIDER_BIG_NUB_BOTTOM, THEME_COLOR_SLIDER_BIG_NUB_TOP, pow(self.pos.y, 1.5)),
                    self.drag
                ))
                
                sdf.stroke(
                    mix(
                        mix(THEME_COLOR_BEVEL_LIGHT, THEME_COLOR_BEVEL_LIGHT * 1.2, self.hover),
                        THEME_COLOR_BLACK,
                        pow(self.pos.y, 1.)
                    ),
                    1.
                ); // Nub outline gradient
                
                
                return sdf.result
            }
        }
    }


    pub SLIDER_ALT1_ROUNDING = (THEME_CORNER_RADIUS * 2.);
    pub SLIDER_ALT1_PEAK_COMPRESSION = 3.5;
    pub SLIDER_ALT1_HANDLE_SIZE = 4.0;

    pub SLIDER_ALT1_LABEL_SIZE = 75.0;
    pub SLIDER_ALT1_LABEL_FONTSIZE = (THEME_FONT_SIZE_P);
    pub SLIDER_ALT1_LABEL_COLOR = (THEME_COLOR_TEXT_DEFAULT);

    pub SLIDER_ALT1_BG_COLOR_A = (THEME_COLOR_BG_CONTAINER);
    pub SLIDER_ALT1_BG_HOVER_COLOR_A = (THEME_COLOR_BG_CONTAINER);
    pub SLIDER_ALT1_BG_DRAG_COLOR_A = (THEME_COLOR_BG_CONTAINER * 1.25);
    pub SLIDER_ALT1_BG_COLOR_B = (THEME_COLOR_D_HIDDEN);
    pub SLIDER_ALT1_BG_HOVER_COLOR_B = (THEME_COLOR_D_HIDDEN);
    pub SLIDER_ALT1_BG_DRAG_COLOR_B = (THEME_COLOR_D_HIDDEN);

    pub SLIDER_ALT1_DATA_FONT_TOPMARGIN = 3.0;
    pub SLIDER_ALT1_DATA_FONTSIZE = (THEME_FONT_SIZE_BASE);

    pub SLIDER_ALT1_DATA_COLOR = (THEME_COLOR_TEXT_DEFAULT);

    pub SLIDER_ALT1_BORDER_COLOR_A = (THEME_COLOR_BEVEL_SHADOW);
    pub SLIDER_ALT1_BORDER_HOVER_COLOR_A = (THEME_COLOR_BEVEL_SHADOW);
    pub SLIDER_ALT1_BORDER_DRAG_COLOR_A = (THEME_COLOR_BEVEL_SHADOW);
    pub SLIDER_ALT1_BORDER_COLOR_B = (THEME_COLOR_BEVEL_LIGHT);
    pub SLIDER_ALT1_BORDER_HOVER_COLOR_B = (THEME_COLOR_BEVEL_LIGHT);
    pub SLIDER_ALT1_BORDER_DRAG_COLOR_B = (THEME_COLOR_BEVEL_LIGHT);

    pub SLIDER_ALT1_VAL_PADDING = 2.5;
    pub SLIDER_ALT1_VAL_COLOR_A = (THEME_COLOR_AMOUNT_DEFAULT * 0.8);
    pub SLIDER_ALT1_VAL_COLOR_B = (THEME_COLOR_AMOUNT_DEFAULT * 1.4);

    pub SLIDER_ALT1_HANDLE_COLOR_A = (THEME_COLOR_SLIDER_NUB_DEFAULT);
    pub SLIDER_ALT1_HANDLE_COLOR_B = (THEME_COLOR_U_1);

    pub SliderAlt1 = <SliderBase> {
        height: 18.,
        width: Fill,

        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        label_align: { y: 0.0 }

        min: 0.0, max: 1.0,
        step: 0.0,
        precision: 2,

        text: "Label",
        hover_actions_enabled: false,

        label_walk: {
            width: Fill,
            height: Fit
        }

        // Label
        draw_text: {
            instance hover: 0.0;
            uniform color: (SLIDER_ALT1_LABEL_COLOR),
            text_style: <THEME_FONT_REGULAR> {
                font_size: (SLIDER_ALT1_LABEL_FONTSIZE)
            }

            fn get_color(self) -> vec4 {
                return self.color;
            }
        }

        // Data input
        text_input: <TextInput> {
            width: Fit, padding: 0.,
            label_align: {y: 0.},

            empty_message: "0",
            is_numeric_only: true,
            margin: { right: 7.5, top: (SLIDER_ALT1_DATA_FONT_TOPMARGIN) } 

            draw_selection: {
                instance hover: 0.0
                instance focus: 0.0
                uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.box(
                        0.,
                        0.,
                        self.rect_size.x,
                        self.rect_size.y,
                        self.border_radius
                    )
                    sdf.fill(
                        mix(THEME_COLOR_U_HIDDEN,
                            THEME_COLOR_D_3,
                            self.focus)
                    ); // Pad color
                    return sdf.result
                }
            }

            draw_bg: {
                instance radius: 1.0
                instance border_width: 0.0
                instance border_color: (#f00) // TODO: This appears not to do anything.
                instance inset: vec4(0.0, 0.0, 0.0, 0.0)
                instance focus: 0.0,
                color: (THEME_COLOR_D_HIDDEN)
                instance color_selected: (THEME_COLOR_D_HIDDEN)
                    
                fn get_color(self) -> vec4 {
                    return mix(self.color, self.color_selected, self.focus)
                }
                    
                fn get_border_color(self) -> vec4 {
                    return self.border_color
                }
                    
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                        max(1.0, self.radius)
                    )
                    sdf.fill_keep(self.get_color())
                    if self.border_width > 0.0 {
                        sdf.stroke(self.get_border_color(), self.border_width)
                    }
                    return sdf.result;
                }
            }

            draw_text: {
                uniform val_text_color: (SLIDER_ALT1_DATA_COLOR);
                text_style: <THEME_FONT_REGULAR> {
                    font_size: (SLIDER_ALT1_DATA_FONTSIZE)
                }

                fn get_color(self) -> vec4 {
                    return
                    mix(
                        mix(
                            mix(
                                self.val_text_color,
                                mix(self.val_text_color, #f, 0.4),
                                self.hover
                            ),
                            mix(
                                mix(self.val_text_color, #f, 0.4),
                                mix(self.val_text_color, #f, 0.8),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(
                            mix(self.val_text_color, #0, 0.4),
                            self.val_text_color,
                            self.hover
                        ),
                        self.is_empty
                    )
                }
            }
        }

        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float

            label_size: (SLIDER_ALT1_LABEL_SIZE);
            uniform val_color_a: (SLIDER_ALT1_VAL_COLOR_A);
            uniform val_color_b: (SLIDER_ALT1_VAL_COLOR_B);
            uniform handle_color_a: (SLIDER_ALT1_HANDLE_COLOR_A);
            uniform handle_color_b: (SLIDER_ALT1_HANDLE_COLOR_B);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let handle_size = (SLIDER_ALT1_HANDLE_SIZE);
                let padding = (SLIDER_ALT1_VAL_PADDING);

                let track_length_bg = self.rect_size.x - self.label_size;

                // Background
                sdf.box(
                    self.label_size,
                    0.0,
                    track_length_bg,
                    self.rect_size.y,
                    SLIDER_ALT1_ROUNDING
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(SLIDER_ALT1_BG_COLOR_A, SLIDER_ALT1_BG_COLOR_B, pow(self.pos.y, 1.0)),
                            mix(SLIDER_ALT1_BG_HOVER_COLOR_A, SLIDER_ALT1_BG_HOVER_COLOR_B, pow(self.pos.y, 1.0)),
                            self.hover
                        ),
                        mix(SLIDER_ALT1_BG_DRAG_COLOR_A, SLIDER_ALT1_BG_DRAG_COLOR_B, pow(self.pos.y, 1.0)),
                        self.drag
                    )
                )

                sdf.stroke(
                    mix(
                        mix(
                            mix(SLIDER_ALT1_BORDER_COLOR_A, SLIDER_ALT1_BORDER_COLOR_B, pow(self.pos.y, 3.0)),
                            mix(SLIDER_ALT1_BORDER_HOVER_COLOR_A, SLIDER_ALT1_BORDER_HOVER_COLOR_B, pow(self.pos.y, 3.0)),
                            self.hover
                        ),
                        mix(SLIDER_ALT1_BORDER_DRAG_COLOR_A, SLIDER_ALT1_BORDER_DRAG_COLOR_B, pow(self.pos.y, 3.0)),
                        self.drag
                    ), 1.0
                )

                let padding_full = padding * 2.;
                let min_size = padding_full + handle_size * 2.;
                let track_length_val = self.rect_size.x - self.label_size - padding_full - min_size;

                // Amount bar
                sdf.box(
                    self.label_size + padding,
                    padding,
                    track_length_val * self.slide_pos + min_size,
                    self.rect_size.y - padding_full,
                    SLIDER_ALT1_ROUNDING * 0.75
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(self.val_color_a, self.val_color_b, pow(self.pos.x, SLIDER_ALT1_PEAK_COMPRESSION)),
                            mix(mix(self.val_color_a, #f, 0.05), mix(self.val_color_b, #f, 0.05), pow(self.pos.x, SLIDER_ALT1_PEAK_COMPRESSION)),
                            self.hover
                        ),
                        mix(mix(self.val_color_a, #f, 0.05), mix(self.val_color_b, #f, 0.05), pow(self.pos.x, SLIDER_ALT1_PEAK_COMPRESSION)),
                        self.drag
                    )
                )

                let handle_shift = self.label_size + padding_full + handle_size;

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
                                self.handle_color_a,
                                self.handle_color_b,
                                self.pos.y
                            ),
                            mix(
                                self.handle_color_a,
                                self.handle_color_b,
                                self.pos.y
                            ),
                            self.hover
                        ),
                        mix(
                            mix(self.handle_color_a, #0, 1.0),
                            mix(self.handle_color_b, #0, 0.1),
                            self.pos.y
                        ),
                        self.drag
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
                    ease: OutQuad
                    apply: {
                        draw_slider: { hover: 0.0 },
                        draw_text: { hover: 0.0 },
                        text_input: {
                            draw_selection: { hover: 0.0},
                            draw_bg: { hover: 0.0},
                            draw_text: { hover: 0.0},
                        }
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: { hover: 1.0 },
                        draw_text: { hover: 1.0 }
                        text_input: {
                            draw_selection: { hover: 1.0},
                            draw_bg: { hover: 1.0},
                            draw_text: { hover: 1.0},
                        }
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_slider: {drag: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {draw_slider: {drag: 1.0}}
                }
            }
        }
    }

    pub ROTARY_LABEL_FONTSIZE = (THEME_FONT_SIZE_P);
    pub ROTARY_LABEL_COLOR = (THEME_COLOR_TEXT_DEFAULT);
    pub ROTARY_DATA_COLOR = (THEME_COLOR_TEXT_DEFAULT);
    pub ROTARY_BG_COLOR_A = (THEME_COLOR_BG_CONTAINER);
    pub ROTARY_BG_HOVER_COLOR_A = (THEME_COLOR_BG_CONTAINER);
    pub ROTARY_BG_DRAG_COLOR_A = (THEME_COLOR_BG_CONTAINER * 1.25);
    pub ROTARY_BG_COLOR_B = (THEME_COLOR_D_2);
    pub ROTARY_BG_HOVER_COLOR_B = (THEME_COLOR_D_2);
    pub ROTARY_BG_DRAG_COLOR_B = (THEME_COLOR_D_2);
    pub ROTARY_VAL_COLOR_A = (THEME_COLOR_U_4_OPAQUE);
    pub ROTARY_VAL_COLOR_B = (THEME_COLOR_U_2_OPAQUE);
    pub ROTARY_HANDLE_COLOR = (THEME_COLOR_U_3);

    pub Rotary = <SliderBase> {
        axis: Vertical,
        step: 0.0,
        precision: 2,
        min: 0.0, max: 1.0,
        hover_actions_enabled: false,

        height: 95., width: 65.,
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        text: "Label",

        align: { x: 0., y: 0.0 }
        label_walk: {
            margin: <THEME_MSPACE_1> {},
            width: Fill, height: Fit
        }

        // Label
        draw_text: {
            instance hover: 0.0;
            uniform color: (ROTARY_LABEL_COLOR),
            text_style: <THEME_FONT_REGULAR> {
                font_size: (ROTARY_LABEL_FONTSIZE)
            }

            fn get_color(self) -> vec4 {
                return self.color;
            }
        }

        // Data input
        text_input: <TextInput> {
            empty_message: "0",
            is_numeric_only: true,

            width: Fit, height: Fit,
            padding: <THEME_MSPACE_1> {},
            label_align: {x: 0.0, y: 0.0 },

            draw_bg: {
                instance radius: (THEME_CORNER_RADIUS)
                instance hover: 0.0
                instance focus: 0.0
                instance bodytop: (THEME_COLOR_INSET_DEFAULT)
                instance bodybottom: (THEME_COLOR_CTRL_ACTIVE)
                
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    return sdf.result
                }
            }

            draw_selection: {
                instance hover: 0.0
                instance focus: 0.0
                uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.box(
                        0.,
                        0.,
                        self.rect_size.x,
                        self.rect_size.y,
                        self.border_radius
                    )
                    sdf.fill(
                        mix(THEME_COLOR_U_HIDDEN,
                            THEME_COLOR_D_3,
                            self.focus)
                    ); // Pad color
                    return sdf.result
                }
            }

            draw_text: {
                uniform val_text_color: (ROTARY_DATA_COLOR);
                fn get_color(self) -> vec4 {
                    return
                    mix(
                        mix(
                            mix(
                                self.val_text_color,
                                mix(self.val_text_color, #f, 0.4),
                                self.hover
                            ),
                            mix(
                                mix(self.val_text_color, #f, 0.4),
                                mix(self.val_text_color, #f, 0.8),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(
                            mix(self.val_text_color, #0, 0.4),
                            self.val_text_color,
                            self.hover
                        ),
                        self.is_empty
                    )
                }
            }
        }

        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform gap: 90.
            uniform padding: 2.0
            uniform width: 10.
            uniform handle_color: (ROTARY_HANDLE_COLOR);
            uniform val_color_a: (ROTARY_VAL_COLOR_A);
            uniform val_color_b: (ROTARY_VAL_COLOR_B);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                let label_offset = 20.;
                let outline_width = 1.;
                let one_deg = PI / 180;
                let threesixty_deg = 2. * PI;
                let gap_size = self.gap * one_deg;
                let val_length = threesixty_deg - (one_deg * self.gap);
                let start = gap_size * 0.5;
                let bg_end = start + val_length;
                let val_end = start + val_length * self.slide_pos;
                let effective_height = self.rect_size.y - label_offset;
                let radius_scaled = min(
                        (self.rect_size.x - outline_width) * 0.5,
                        (self.rect_size.y - label_offset - outline_width) * 0.5
                    );
                let radius_width_compensation = self.width * 0.5;
                let width_fix = 0.008;
                let bg_width_scaled = min(self.rect_size.x, effective_height) * self.width * width_fix;

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
                            mix(ROTARY_BG_COLOR_A, ROTARY_BG_COLOR_B, gradient_y),
                            mix(ROTARY_BG_HOVER_COLOR_A, ROTARY_BG_HOVER_COLOR_B, gradient_y),
                            self.hover
                        ),
                        mix(ROTARY_BG_DRAG_COLOR_A, ROTARY_BG_DRAG_COLOR_B, gradient_y),
                        self.drag
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

                sdf.fill(#0004);

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset + 1.,
                    radius_scaled - radius_width_compensation,
                    start,
                    bg_end, 
                    bg_width_scaled * 0.1
                );

                sdf.fill(#fff2);

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset - 2.,
                    radius_scaled - radius_width_compensation,
                    start,
                    bg_end, 
                    bg_width_scaled * 0.1
                );

                sdf.fill(#0008);

                // outer rim
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation + bg_width_scaled * 0.5,
                    start,
                    bg_end, 
                    radius_scaled * 0.035
                );

                sdf.fill(mix(#000, #fff3, gradient_y));

                // inner rim
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation - bg_width_scaled * 0.5,
                    start,
                    bg_end, 
                    radius_scaled * 0.075
                );

                sdf.fill(mix(#fff2, #0000, gradient_y + label_offset_norm * 2.));

                let val_width = (self.width - self.padding) * width_fix;
                let val_width_scaled = min(
                        self.rect_size.x * val_width,
                        effective_height * val_width
                    );

                // Value
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation,
                    start,
                    val_end, 
                    val_width_scaled
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(self.val_color_a, self.val_color_b, self.slide_pos),
                            mix(
                                mix(self.val_color_a, #f, 0.1),
                                mix(self.val_color_b, #f, 0.1),
                                self.slide_pos
                            ),
                            self.hover
                        ),
                        mix(
                            mix(self.val_color_a, #0, 0.1),
                            mix(self.val_color_b, #0, 0.1),
                            self.slide_pos
                        ),
                        self.drag
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
                        0.,
                        val_width_scaled,
                        self.hover
                    )
                );

                sdf.fill_keep(
                    mix(
                        self.handle_color,
                        mix(self.handle_color, #f, 0.25),
                        self.drag
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
                    ease: OutQuad
                    apply: {
                        draw_slider: { hover: 0.0 },
                        draw_text: { hover: 0.0 },
                        text_input: {
                            draw_selection: { hover: 0.0},
                            draw_bg: { hover: 0.0},
                            draw_text: { hover: 0.0},
                        }
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: { hover: 1.0 },
                        draw_text: { hover: 1.0 }
                        text_input: {
                            draw_selection: { hover: 1.0},
                            draw_bg: { hover: 1.0},
                            draw_text: { hover: 1.0},
                        }
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply:
                        {
                            draw_slider: {drag: 0.0},
                            text_input: {
                                draw_selection: { hover: 0.0},
                                draw_bg: { hover: 0.0},
                                draw_text: { hover: 0.0},
                            }
                        }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: {drag: 1.0},
                        text_input: {
                            draw_selection: { hover: 0.0},
                            draw_bg: { hover: 0.0},
                            draw_text: { hover: 0.0},
                        }
                    }
                }
            }
        }
    }

    pub ROTARY_FLAT_LABEL_FONTSIZE = (THEME_FONT_SIZE_P);
    pub ROTARY_FLAT_LABEL_COLOR = (THEME_COLOR_TEXT_DEFAULT);
    pub ROTARY_FLAT_DATA_COLOR = (THEME_COLOR_TEXT_DEFAULT);
    pub ROTARY_FLAT_BG_COLOR = (THEME_COLOR_D_HIDDEN);
    pub ROTARY_FLAT_BG_DRAG_COLOR = (THEME_COLOR_D_1);
    pub ROTARY_FLAT_BORDER_COLOR = (THEME_COLOR_BEVEL_SHADOW);
    pub ROTARY_FLAT_VAL_COLOR_A = (THEME_COLOR_U_2);
    pub ROTARY_FLAT_VAL_COLOR_B = (THEME_COLOR_U_4);
    pub ROTARY_FLAT_HANDLE_COLOR = (THEME_COLOR_U_3);

    pub RotaryFlat = <SliderBase> {
        axis: Vertical,
        step: 0.0,
        precision: 2,
        min: 0.0, max: 1.0,
        hover_actions_enabled: false,

        height: 95., width: 65.,
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        text: "Label",

        align: { x: 0., y: 0.0 }
        label_walk: {
            margin: <THEME_MSPACE_1> {},
            width: Fill, height: Fit
        }

        // Label
        draw_text: {
            instance hover: 0.0;
            uniform color: (ROTARY_FLAT_LABEL_COLOR),
            text_style: <THEME_FONT_REGULAR> {
                font_size: (ROTARY_FLAT_LABEL_FONTSIZE)
            }

            fn get_color(self) -> vec4 {
                return self.color;
            }
        }

        // Data input
        text_input: <TextInput> {
            empty_message: "0",
            is_numeric_only: true,

            width: Fit, height: Fit,
            padding: <THEME_MSPACE_1> {},
            label_align: {x: 0.0, y: 0.0 },

            draw_bg: {
                instance radius: (THEME_CORNER_RADIUS)
                instance hover: 0.0
                instance focus: 0.0
                instance bodytop: (THEME_COLOR_INSET_DEFAULT)
                instance bodybottom: (THEME_COLOR_CTRL_ACTIVE)
                
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    return sdf.result
                }
            }

            draw_selection: {
                instance hover: 0.0
                instance focus: 0.0
                uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.box(
                        0.,
                        0.,
                        self.rect_size.x,
                        self.rect_size.y,
                        self.border_radius
                    )
                    sdf.fill(
                        mix(THEME_COLOR_U_HIDDEN,
                            THEME_COLOR_D_3,
                            self.focus)
                    ); // Pad color
                    return sdf.result
                }
            }

            draw_text: {
                uniform val_text_color: (ROTARY_FLAT_DATA_COLOR);
                fn get_color(self) -> vec4 {
                    return
                    mix(
                        mix(
                            mix(
                                self.val_text_color,
                                mix(self.val_text_color, #f, 0.4),
                                self.hover
                            ),
                            mix(
                                mix(self.val_text_color, #f, 0.4),
                                mix(self.val_text_color, #f, 0.8),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(
                            mix(self.val_text_color, #0, 0.4),
                            self.val_text_color,
                            self.hover
                        ),
                        self.is_empty
                    )
                }
            }
        }

        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform gap: 90.
            uniform width: 5.
            uniform padding: 4.0
            uniform handle_color: (ROTARY_FLAT_HANDLE_COLOR);
            uniform val_color_a: (ROTARY_FLAT_VAL_COLOR_A);
            uniform val_color_b: (ROTARY_FLAT_VAL_COLOR_B);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                let label_offset = 20.;
                let outline_width = 1.;

                let one_deg = PI / 180;
                let threesixty_deg = 2. * PI;
                let gap_size = self.gap * one_deg;
                let val_length = threesixty_deg - (one_deg * self.gap);
                let start = gap_size * 0.5;
                let bg_end = start + val_length;
                let val_end = start + val_length * self.slide_pos;
                let effective_height = self.rect_size.y - label_offset;
                let radius_scaled = min(
                        (self.rect_size.x - outline_width) * 0.5,
                        (self.rect_size.y - label_offset - outline_width) * 0.5
                    );
                let radius_width_compensation = self.width * 0.5;
                let width_fix = 0.008;
                let bg_width_scaled = min(self.rect_size.x, effective_height) * self.width * width_fix;

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
                            ROTARY_FLAT_BG_COLOR,
                            ROTARY_FLAT_BG_COLOR,
                            self.hover
                        ),
                        ROTARY_FLAT_BG_DRAG_COLOR,
                        self.drag
                    )
                )
                sdf.stroke(ROTARY_FLAT_BORDER_COLOR, outline_width);

                let val_width = (self.width - self.padding) * width_fix;
                let val_width_scaled = min(
                        self.rect_size.x * val_width,
                        effective_height * val_width
                    );

                // Value
                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation,
                    start,
                    val_end, 
                    val_width_scaled
                );

                sdf.fill(
                    mix(
                        mix(
                            mix(self.val_color_a, self.val_color_b, self.slide_pos),
                            mix(
                                mix(self.val_color_a, #f, 0.1),
                                mix(self.val_color_b, #f, 0.1),
                                self.slide_pos
                            ),
                            self.hover
                        ),
                        mix(
                            mix(self.val_color_a, #0, 0.1),
                            mix(self.val_color_b, #0, 0.1),
                            self.slide_pos
                        ),
                        self.drag
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
                        0.,
                        val_width_scaled,
                        self.hover
                    )
                );

                sdf.fill_keep(
                    mix(
                        self.handle_color,
                        mix(self.handle_color, #f, 0.25),
                        self.drag
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
                    ease: OutQuad
                    apply: {
                        draw_slider: { hover: 0.0 },
                        draw_text: { hover: 0.0 },
                        text_input: {
                            draw_selection: { hover: 0.0},
                            draw_bg: { hover: 0.0},
                            draw_text: { hover: 0.0},
                        }
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: { hover: 1.0 },
                        draw_text: { hover: 1.0 }
                        text_input: {
                            draw_selection: { hover: 1.0},
                            draw_bg: { hover: 1.0},
                            draw_text: { hover: 1.0},
                        }
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply:
                        {
                            draw_slider: {drag: 0.0},
                            text_input: {
                                draw_selection: { hover: 0.0},
                                draw_bg: { hover: 0.0},
                                draw_text: { hover: 0.0},
                            }
                        }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: {drag: 1.0},
                        text_input: {
                            draw_selection: { hover: 0.0},
                            draw_bg: { hover: 0.0},
                            draw_text: { hover: 0.0},
                        }
                    }
                }
            }
        }
    }

    pub ROTARY_SOLID_LABEL_FONTSIZE = (THEME_FONT_SIZE_P);
    pub ROTARY_SOLID_LABEL_COLOR = (THEME_COLOR_TEXT_DEFAULT);
    pub ROTARY_SOLID_DATA_COLOR = (THEME_COLOR_TEXT_DEFAULT);
    pub ROTARY_SOLID_BG_COLOR_A = (THEME_COLOR_D_2);
    pub ROTARY_SOLID_BG_COLOR_B = (THEME_COLOR_D_4);
    pub ROTARY_SOLID_HANDLE_COLOR = #FFA;

    pub RotarySolid = <SliderBase> {
        axis: Vertical,
        step: 0.0,
        precision: 2,
        min: 0.0, max: 1.0,
        hover_actions_enabled: false,

        height: 95., width: 65.,
        margin: <THEME_MSPACE_1> { top: (THEME_SPACE_2) }
        text: "Label",

        align: { x: 0., y: 0. }
        label_walk: {
            margin: <THEME_MSPACE_1> {},
            width: Fill, height: Fit
        }

        // Label
        draw_text: {
            instance hover: 0.;
            uniform color: (ROTARY_SOLID_LABEL_COLOR),
            text_style: <THEME_FONT_REGULAR> {
                font_size: (ROTARY_SOLID_LABEL_FONTSIZE)
            }

            fn get_color(self) -> vec4 {
                return self.color;
            }
        }

        // Data input
        text_input: <TextInput> {
            empty_message: "0",
            is_numeric_only: true,

            width: Fit, height: Fit,
            padding: <THEME_MSPACE_1> {},
            label_align: {x: 0.0, y: 0.0 },

            draw_bg: {
                instance radius: (THEME_CORNER_RADIUS)
                instance hover: 0.0
                instance focus: 0.0
                instance bodytop: (THEME_COLOR_INSET_DEFAULT)
                instance bodybottom: (THEME_COLOR_CTRL_ACTIVE)
                
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    return sdf.result
                }
            }

            draw_selection: {
                instance hover: 0.0
                instance focus: 0.0
                uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.box(
                        0.,
                        0.,
                        self.rect_size.x,
                        self.rect_size.y,
                        self.border_radius
                    )
                    sdf.fill(
                        mix(THEME_COLOR_U_HIDDEN,
                            THEME_COLOR_D_3,
                            self.focus)
                    ); // Pad color
                    return sdf.result
                }
            }

            draw_text: {
                uniform val_text_color: (ROTARY_SOLID_DATA_COLOR);
                fn get_color(self) -> vec4 {
                    return
                    mix(
                        mix(
                            mix(
                                self.val_text_color,
                                mix(self.val_text_color, #f, 0.4),
                                self.hover
                            ),
                            mix(
                                mix(self.val_text_color, #f, 0.4),
                                mix(self.val_text_color, #f, 0.8),
                                self.hover
                            ),
                            self.focus
                        ),
                        mix(
                            mix(self.val_text_color, #0, 0.4),
                            self.val_text_color,
                            self.hover
                        ),
                        self.is_empty
                    )
                }
            }
        }

        draw_slider: {
            instance hover: float
            instance focus: float
            instance drag: float

            uniform gap: 90.
            uniform width: 10.
            uniform handle_color: (ROTARY_SOLID_HANDLE_COLOR);

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

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
                let radius_width_compensation = self.width * 0.5;
                let width_fix = 0.008;
                let bg_width_scaled = min(self.rect_size.x, effective_height) * self.width * width_fix;

                // Background
                sdf.circle(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation
                );

                let label_offset_norm = label_offset / self.rect_size.y;
                let arc_h_norm = (360. - self.gap) / 360.; // approximation
                let rotary_solid_h = radius_scaled * 2. / self.rect_size.y * arc_h_norm;
                let gradient_y = pow(self.pos.y, 2.) / rotary_solid_h - label_offset_norm;

                sdf.fill(
                    mix(ROTARY_SOLID_BG_COLOR_A, ROTARY_SOLID_BG_COLOR_B, gradient_y)
                )

                sdf.circle(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation - bg_width_scaled * 0.5
                );

                sdf.fill(
                    mix(
                        mix((THEME_COLOR_U_1), (THEME_COLOR_U_2), gradient_y),
                        mix((THEME_COLOR_U_HIDDEN), (THEME_COLOR_U_2), gradient_y),
                        self.drag
                    )
                )

                // outer rim
                sdf.circle(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation
                );

                sdf.stroke(mix((THEME_COLOR_U_HIDDEN), (THEME_COLOR_BEVEL_LIGHT), gradient_y), 1.5);

                // inner rim
                sdf.circle(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled - radius_width_compensation - bg_width_scaled * 0.5
                );

                sdf.fill_keep(mix((THEME_COLOR_U_2), (THEME_COLOR_D_1), gradient_y));
                sdf.stroke(mix(
                        mix((THEME_COLOR_BEVEL_LIGHT), (THEME_COLOR_BEVEL_SHADOW), gradient_y),
                        mix((THEME_COLOR_U_4), (THEME_COLOR_D_4), gradient_y),
                        self.hover
                    ), gloss_width * 1.5);

                let val_width = self.width * 0.004;
                let val_width_scaled = min(
                        self.rect_size.x * val_width,
                        effective_height * val_width
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
                    val_width_scaled
                );

                sdf.fill(
                    mix(
                        self.handle_color,
                        mix(self.handle_color, #f, 0.25),
                        self.drag
                    )
                )

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled * 1.1,
                    start, 
                    start, 
                    markings_width_scaled
                );

                sdf.arc_round_caps(
                    self.rect_size.x / 2.,
                    radius_scaled + label_offset,
                    radius_scaled * 1.1,
                    bg_end, 
                    bg_end, 
                    markings_width_scaled
                );

                sdf.fill(mix((THEME_COLOR_U_2), (THEME_COLOR_U_4), self.hover))
                
                return sdf.result
            }
        }

        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: OutQuad
                    apply: {
                        draw_slider: { hover: 0.0 },
                        draw_text: { hover: 0.0 },
                        text_input: {
                            draw_selection: { hover: 0.0},
                            draw_bg: { hover: 0.0},
                            draw_text: { hover: 0.0},
                        }
                    }
                }
                on = {
                    //cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: { hover: 1.0 },
                        draw_text: { hover: 1.0 }
                        text_input: {
                            draw_selection: { hover: 1.0},
                            draw_bg: { hover: 1.0},
                            draw_text: { hover: 1.0},
                        }
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_slider: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_slider: {focus: 1.0}
                    }
                }
            }
            drag = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply:
                        {
                            draw_slider: {drag: 0.0},
                            text_input: {
                                draw_selection: { hover: 0.0},
                                draw_bg: { hover: 0.0},
                                draw_text: { hover: 0.0},
                            }
                        }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Snap}
                    apply: {
                        draw_slider: {drag: 1.0},
                        text_input: {
                            draw_selection: { hover: 0.0},
                            draw_bg: { hover: 0.0},
                            draw_text: { hover: 0.0},
                        }
                    }
                }
            }
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
    #[area] #[redraw] #[live] draw_slider: DrawSlider,
    
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
        self.text_input.text = match self.precision{
            0=>format!("{:.0}",e),
            1=>format!("{:.1}",e),
            2=>format!("{:.2}",e),
            3=>format!("{:.3}",e),
            4=>format!("{:.4}",e),
            5=>format!("{:.5}",e),
            6=>format!("{:.6}",e),
            7=>format!("{:.7}",e),
            _=>format!("{}",e)
        };
        self.text_input.select_all();
        self.text_input.redraw(cx);
    }
    
    pub fn draw_walk_slider(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_slider.slide_pos = self.relative_value as f32;
        self.draw_slider.begin(cx, walk, self.layout);
        
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
        
        self.draw_slider.end(cx);
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

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        
        // alright lets match our designer against the slider backgdrop
        match event.hit_designer(cx, self.draw_slider.area()){
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
                TextInputAction::Return(value) => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.set_internal(v.max(self.min).min(self.max));
                    }
                    self.update_text_input(cx);
                    cx.widget_action(uid, &scope.path, SliderAction::TextSlide(self.to_external()));
                }
                TextInputAction::Escape => {
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

        match event.hits(cx, self.draw_slider.area()) {
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

                self.text_input.is_read_only = true;
                self.text_input.set_key_focus(cx);
                self.text_input.select_all();
                self.text_input.redraw(cx);
                                
                self.animator_play(cx, id!(drag.on));
                self.dragging = Some(self.relative_value);
                cx.widget_action(uid, &scope.path, SliderAction::StartSlide);
                cx.set_cursor(MouseCursor::Grabbing);
            },
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                self.text_input.is_read_only = false;
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
                        self.relative_value = (start_pos + rel.x / (fe.rect.size.x - self.draw_slider.label_size as f64)).max(0.0).min(1.0);
                    } else {
                        self.relative_value = (start_pos - rel.y / fe.rect.size.y as f64).max(0.0).min(1.0);
                    }
                    self.set_internal(self.to_external());
                    self.draw_slider.redraw(cx);
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