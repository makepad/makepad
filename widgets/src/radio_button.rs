use crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        View,
        Image,
    };

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    use crate::view_ui::CachedRoundedView;
    
    DrawRadioButton = {{DrawRadioButton}} {}
    pub RadioButtonBase = {{RadioButton}} {}
    pub RadioButtonGroupBase = {{RadioButtonGroup }} {}
    
    pub RadioButton = <RadioButtonBase> {
        // TODO: adda  focus states
        width: Fit, height: 16.,
        align: { x: 0.0, y: 0.5 }
        
        icon_walk: { margin: { left: 20. } }
        
        label_walk: {
            width: Fit, height: Fit,
            margin: { left: 20. }
        }
        label_align: { y: 0.0 }
        
        draw_bg: {
            uniform size: 7.0,

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color: (THEME_COLOR_OUTSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_active: (THEME_COLOR_INSET_ACTIVE)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1)
            uniform color_1_active: (THEME_COLOR_INSET_1)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2)
            uniform color_2_active: (THEME_COLOR_INSET_2)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW_FOCUS)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT_FOCUS)

            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            uniform mark_color_focus: (THEME_COLOR_MARK_HOVER)
            uniform mark_color_active_focus: (THEME_COLOR_MARK_ACTIVE_FOCUS)
            uniform mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                match self.radio_type {
                    RadioType::Round => {
                        // return mix(#f00, #0ff, self.focus);
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);

                        // Draw background
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(
                                        mix(self.color_1, self.color_2, self.pos.y + dither),
                                        mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                                        self.focus
                                    ),
                                    mix(
                                        mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                                        mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                                        self.focus
                                    ),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                self.hover
                            )
                        )
                        sdf.stroke(
                            mix(
                                mix(
                                    mix(
                                        mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                                        self.focus
                                    ),
                                    mix(
                                        mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                                        self.focus
                                    ),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ), self.border_size
                        )

                        // Draw mark
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(
                            mix(
                                mix(
                                    mix(
                                        self.mark_color,
                                        self.mark_color_focus,
                                        self.focus
                                    ),
                                    self.mark_color_hover,
                                    self.hover
                                ),
                                mix(
                                    mix(
                                        mix(
                                            self.mark_color_active,
                                            self.mark_color_active_focus,
                                            self.focus
                                        ),
                                        self.mark_color_focus,
                                        self.focus
                                    ),
                                    self.mark_color_active_hover,
                                    self.hover
                                ),
                                self.active
                            )
                        );
                    }
                    RadioType::Tab => {
                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x - 2.0,
                            self.rect_size.y - 2.0,
                            self.border_radius
                        )

                        sdf.stroke_keep(
                            mix(
                                mix(
                                    mix(
                                        mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                                        self.focus
                                    ),
                                    mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                    self.hover
                                ),
                                mix(
                                    mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                                    self.focus
                                ),
                                self.active
                            ), self.border_size)

                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(
                                        mix(self.color_1, self.color_2, self.pos.y + dither),
                                        mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                                        self.focus
                                    ),
                                    mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                    self.hover
                                ),
                                mix(
                                    mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                                    mix(self.color_1_focus, self.color_2_focus, self.pos.y + dither),
                                    self.focus
                                ),
                                self.active
                            )
                        )
                            
                    }
                }
                return sdf.result
            }
        }
            
        draw_text: {
            instance active: 0.0
            instance focus: 0.0
            instance hover: 0.0
                
            uniform color: (THEME_COLOR_TEXT)
            uniform color_hover: (THEME_COLOR_TEXT)
            uniform color_active: (THEME_COLOR_TEXT)
            uniform color_focus: (THEME_COLOR_TEXT_FOCUS)
                
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(self.color, self.color_focus, self.focus),
                        self.color_hover,
                        self.hover
                    ),
                    mix(self.color_active, self.color_focus, self.focus),
                    self.active
                )
            }
        }
            
        draw_icon: {
            instance active: 0.0
            instance focus: 0.0
            instance hover: 0.0

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_WHITE)
            uniform color_1_active: (THEME_COLOR_TEXT_ACTIVE)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_WHITE)
            uniform color_2_active: (THEME_COLOR_TEXT_ACTIVE)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y),
                            mix(self.color_1_focus, self.color_2_focus, self.pos.y),
                            self.focus
                        ),
                        mix(self.color_1_hover, self.color_2_hover, self.pos.y),
                        self.hover
                    ),
                    mix(
                        mix(
                            mix(self.color_1_active, self.color_2_active, self.pos.y),
                            mix(self.color_1_focus, self.color_2_focus, self.pos.y),
                            self.focus
                        ),
                        mix(self.color_1_hover, self.color_2_hover, self.pos.y),
                        self.hover
                    ),
                    self.active
                )
            }
        }
            
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }
            active = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_icon: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_icon: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    pub RadioButtonFlat = <RadioButton> {
        draw_bg: {
            border_size: (THEME_BEVELING)

            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_active: (THEME_COLOR_INSET_ACTIVE)
            color_1_focus: (THEME_COLOR_INSET_1_FOCUS)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_active: (THEME_COLOR_INSET_ACTIVE)
            color_2_focus: (THEME_COLOR_INSET_2_FOCUS)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL)
            border_color_1_active: (THEME_COLOR_BEVEL)
            border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW_FOCUS)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL)
            border_color_2_active: (THEME_COLOR_BEVEL)
            border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT_FOCUS)
        }

    }

    pub RadioButtonFlatter = <RadioButton> {
        draw_bg: {
            border_size: 0.

            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_active: (THEME_COLOR_INSET_ACTIVE)
            color_1_focus: (THEME_COLOR_INSET_1_FOCUS)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_active: (THEME_COLOR_INSET_ACTIVE)
            color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
        }

    }
         
    pub RadioButtonGradientX = <RadioButton> {
        draw_bg: {
            uniform size: 7.0;

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color: (THEME_COLOR_OUTSET)
            uniform color_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform color_active: (THEME_COLOR_OUTSET_ACTIVE)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1)
            uniform color_1_active: (THEME_COLOR_INSET_1)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2)
            uniform color_2_active: (THEME_COLOR_INSET_2)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW_FOCUS)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT_FOCUS)

            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_hover: (THEME_COLOR_OUTSET_HOVER)
            uniform mark_color_focus: (THEME_COLOR_MARK_HOVER)
            uniform mark_color_active_focus: (THEME_COLOR_MARK_ACTIVE_FOCUS)
            uniform mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            uniform mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                match self.radio_type {
                    RadioType::Round => {
                        let sz = self.size;
                        let left = sz + 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);

                        // Draw background
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(
                                        mix(self.color_1, self.color_2, self.pos.x + dither),
                                        mix(self.color_1_focus, self.color_2_focus, self.pos.x + dither),
                                        self.focus
                                    ),
                                    mix(
                                        mix(self.color_1_active, self.color_2_active, self.pos.x + dither),
                                        mix(self.color_1_focus, self.color_2_focus, self.pos.x + dither),
                                        self.focus
                                    ),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.x + dither),
                                self.hover
                            )
                        )
                        sdf.stroke(
                            mix(
                                mix(
                                    mix(
                                        mix(self.border_color_1, self.border_color_2, self.pos.x + dither),
                                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.x + dither),
                                        self.focus
                                    ),
                                    mix(
                                        mix(self.border_color_1_active, self.border_color_2_active, self.pos.x + dither),
                                        mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.x + dither),
                                        self.focus
                                    ),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.x + dither),
                                self.hover
                            ), self.border_size
                        )

                        // Draw mark
                        let isz = sz * 0.5;
                        sdf.circle(left, c.y, isz);
                        sdf.fill(
                            mix(
                                mix(
                                    mix(
                                        self.mark_color,
                                        self.mark_color_focus,
                                        self.focus
                                    ),
                                    self.mark_color_hover,
                                    self.hover
                                ),
                                mix(
                                    mix(
                                        self.mark_color_active,
                                        self.mark_color_active_focus,
                                        self.focus
                                    ),
                                    self.mark_color_active_hover,
                                    self.hover
                                ),
                                self.active
                            )
                        );
                    }
                }
                return sdf.result
            }
        }
    }

    pub RadioButtonGradientY = <RadioButton> { }
    
    pub RadioButtonCustom = <RadioButton> {
        height: Fit,
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
        margin: { left: -17.5 }
        label_walk: {
            width: Fit, height: Fit,
            margin: { left: (THEME_SPACE_2) }
        }
    }
        
    pub RadioButtonTextual = <RadioButton> {
        height: Fit,
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }

        label_walk: {
            margin: 0.,
            width: Fit, height: Fit,
        }

        draw_text: {
            color: (THEME_COLOR_U_3)
            color_hover: (THEME_COLOR_TEXT_HOVER)
            color_active: (THEME_COLOR_TEXT_ACTIVE)
                
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
    }
        
    pub RadioButtonImage = <RadioButton> { }
        
    pub RadioButtonTab = <RadioButton> {
        height: Fit,
        label_walk: { margin: { left: 20., right: 5. } }

        draw_bg: {
            radio_type: Tab

            border_size: (THEME_BEVELING)

            color_1: (THEME_COLOR_OUTSET)
            color_1_hover: (THEME_COLOR_OUTSET_HOVER)
            color_1_active: (THEME_COLOR_OUTSET_ACTIVE)
            color_1_focus: (THEME_COLOR_INSET_1_FOCUS)

            color_2: (THEME_COLOR_OUTSET)
            color_2_hover: (THEME_COLOR_OUTSET_HOVER)
            color_2_active: (THEME_COLOR_OUTSET_ACTIVE)
            color_2_focus: (THEME_COLOR_INSET_2_FOCUS)

            border_color_1: (THEME_COLOR_BEVEL_LIGHT)
            border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT)
            border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)
            border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW_FOCUS)

            border_color_2: (THEME_COLOR_BEVEL_SHADOW)
            border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW)
            border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)
            border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT_FOCUS)
        }

        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1.25)}
            
        draw_text: {
            color: (THEME_COLOR_TEXT)
            color_hover: (THEME_COLOR_TEXT_HOVER)
            color_active: (THEME_COLOR_TEXT_ACTIVE)
        }
    }

    pub RadioButtonTabFlat = <RadioButtonTab> {
        draw_bg: {
            radio_type: Tab
            border_size: (THEME_BEVELING)

            color_1: (THEME_COLOR_OUTSET)
            color_1_hover: (THEME_COLOR_OUTSET_HOVER)
            color_1_active: (THEME_COLOR_OUTSET_ACTIVE)
            color_1_focus: (THEME_COLOR_OUTSET_ACTIVE)

            color_2: (THEME_COLOR_OUTSET)
            color_2_hover: (THEME_COLOR_OUTSET_HOVER)
            color_2_active: (THEME_COLOR_OUTSET_ACTIVE)
            color_2_focus: (THEME_COLOR_OUTSET_ACTIVE)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_active: (THEME_COLOR_BEVEL_ACTIVE)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_active: (THEME_COLOR_BEVEL_ACTIVE)
        }
    }

    pub RadioButtonTabFlatter = <RadioButtonTabFlat> {
        draw_bg: {
            border_size: 0.
        }
    }

    pub RadioButtonTabGradientX = <RadioButton> {
        height: Fit,

        label_walk: { margin: { left: 20., right: 5. } }

        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1.25)}
            
        draw_text: {
            color: (THEME_COLOR_TEXT)
            color_hover: (THEME_COLOR_TEXT_HOVER)
            color_active: (THEME_COLOR_TEXT_ACTIVE)
        }

        draw_bg: {
            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_OUTSET * 1.5)
            uniform color_1_hover: (THEME_COLOR_OUTSET_HOVER * 1.2)
            uniform color_1_active: (THEME_COLOR_OUTSET_ACTIVE * 2.0)
            uniform color_1_active_focus: (THEME_COLOR_INSET_1_FOCUS)

            uniform color_2: (THEME_COLOR_OUTSET)
            uniform color_2_hover: (THEME_COLOR_OUTSET_HOVER * 0.5)
            uniform color_2_active: (THEME_COLOR_OUTSET_ACTIVE * 0.5)
            uniform color_2_active_focus: (THEME_COLOR_INSET_2_FOCUS)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active_focus: (THEME_COLOR_BEVEL_SHADOW_FOCUS)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active_focus: (THEME_COLOR_BEVEL_LIGHT_FOCUS)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                                self.focus
                            ),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                            mix(self.border_color_1_active_focus, self.border_color_2_active_focus, self.pos.y + dither),
                            self.focus
                        ),
                        self.active
                    ), self.border_size)
                    

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, self.pos.x + dither),
                                mix(self.color_1_focus, self.color_2_focus, self.pos.x + dither),
                                self.focus
                            ),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_active, self.color_2_active, self.pos.x + dither),
                            mix(self.color_1_active_focus, self.color_2_active_focus, self.pos.x + dither),
                            self.focus
                        ),
                        self.active
                    )
                )
                return sdf.result
            }
        }
    }

    pub RadioButtonTabGradientY = <RadioButton> {
        height: Fit,

        label_walk: { margin: { left: 20., right: 5. } }

        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1.25)}
            
        draw_text: {
            color: (THEME_COLOR_TEXT)
            color_hover: (THEME_COLOR_TEXT_HOVER)
            color_active: (THEME_COLOR_TEXT_ACTIVE)
        }

        draw_bg: {
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_OUTSET * 1.5)
            uniform color_1_hover: (THEME_COLOR_OUTSET_HOVER * 1.2)
            uniform color_1_active: (THEME_COLOR_OUTSET_ACTIVE * 2.0)
            uniform color_1_active_focus: (THEME_COLOR_INSET_1_ACTIVE_FOCUS)

            uniform color_2: (THEME_COLOR_OUTSET)
            uniform color_2_hover: (THEME_COLOR_OUTSET_HOVER * 0.5)
            uniform color_2_active: (THEME_COLOR_OUTSET_ACTIVE * 0.5)
            uniform color_2_active_focus: (THEME_COLOR_INSET_2_ACTIVE_FOCUS)

            uniform border_color_1: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active_focus: (THEME_COLOR_BEVEL_SHADOW_FOCUS)

            uniform border_color_2: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active_focus: (THEME_COLOR_BEVEL_LIGHT_FOCUS)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                mix(self.border_color_1_focus, self.border_color_2_focus, self.pos.y + dither),
                                self.focus
                            ),
                            mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                            mix(self.border_color_1_active_focus, self.border_color_2_active_focus, self.pos.y + dither),
                            self.focus
                        ),
                        self.active
                    ), self.border_size)
                    

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(self.color_1, self.color_2, self.pos.x + dither),
                                mix(self.color_1_focus, self.color_2_focus, self.pos.x + dither),
                                self.focus
                            ),
                            mix(self.color_1_hover, self.color_2_hover, self.pos.x + dither),
                            self.hover
                        ),
                        mix(
                            mix(self.color_1_active, self.color_2_active, self.pos.x + dither),
                            mix(self.color_1_active_focus, self.color_2_active_focus, self.pos.x + dither),
                            self.focus
                        ),
                        self.active
                    )
                )
                return sdf.result
            }
        }
    }
    
    pub ButtonGroup = <CachedRoundedView> {
        height: Fit, width: Fit,
        spacing: 0.0,
        flow: Right
        align: { x: 0.0, y: 0.5 }
        draw_bg: { border_radius: 0.  }
    }

    pub RadioButtonGroupTab = <RadioButtonTab> {
        height: Fit,
        draw_bg: { radio_type: Tab }
        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1.25), right: (THEME_SPACE_2 * 2.)}
            
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawRadioButton {
    #[deref] draw_super: DrawQuad,
    #[live] radio_type: RadioType,
    #[live] hover: f32,
    #[live] focus: f32,
    #[live] active: f32
}


#[derive(Live, LiveHook)]
#[live_ignore]
#[repr(u32)]
pub enum RadioType {
    #[pick] Round = shader_enum(1),
    Tab = shader_enum(2),
}

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum MediaType {
    Image,
    #[pick] Icon,
    None,
}

#[derive(Live, LiveHook, Widget)]
pub struct RadioButtonGroup {
    #[deref] frame: View
}

#[derive(Live, LiveHook, Widget)]
pub struct RadioButton {
    #[redraw] #[live] draw_bg: DrawRadioButton,
    #[live] draw_icon: DrawIcon,
    #[live] draw_text: DrawText2,

    #[live] value: LiveValue,

    #[live] media: MediaType,
    
    #[live] icon_walk: Walk,
    #[walk] walk: Walk,

    #[live] image: Image,

    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    #[live] text: ArcStringMut,
    
    #[live] bind: String,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum RadioButtonAction {
    Clicked,
    None
}


impl RadioButtonGroup {
    pub fn draw_walk(&mut self, _cx: &mut Cx2d, _walk: Walk) {}
}

impl RadioButton {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);
        match self.media {
            MediaType::Image => {
                let image_walk = self.image.walk(cx);
                let _ = self.image.draw_walk(cx, image_walk);
            }
            MediaType::Icon => {
                self.draw_icon.draw_walk(cx, self.icon_walk);
            }
            MediaType::None => {}
        }
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());
    }
        
}

impl Widget for RadioButtonGroup {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        //let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
              
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
    
}

impl Widget for RadioButton {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::KeyFocus(_) => {
                self.animator_play(cx, id!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, id!(focus.off));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.animator_in_state(cx, id!(active.off)) {
                    self.animator_play(cx, id!(active.on));
                    cx.widget_action(uid, &scope.path, RadioButtonAction::Clicked);
                }
                self.set_key_focus(cx);
            },
            Hit::FingerUp(_fe) => {
                                
            }
            Hit::FingerMove(_fe) => {
                                
            }
            _ => ()
        }

    }
    
    fn set_disabled(&mut self, cx:&mut Cx, disabled:bool){
        self.animator_toggle(cx, disabled, Animate::Yes, id!(disabled.on), id!(disabled.off));
    }
                
    fn disabled(&self, cx:&Cx) -> bool {
        self.animator_in_state(cx, id!(disabled.on))
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk);
        DrawStep::done()
    }
    
    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }
            
    fn set_text(&mut self, cx:&mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl RadioButtonRef{
    fn unselect(&self, cx:&mut Cx){
        if let Some(mut inner) = self.borrow_mut(){
            inner.animator_play(cx, id!(active.off));
        }
    }

    pub fn select(&self, cx: &mut Cx, scope: &mut Scope){
        if let Some(mut inner) = self.borrow_mut(){
            if inner.animator_in_state(cx, id!(active.off)) {
                inner.animator_play(cx, id!(active.on));
                cx.widget_action(inner.widget_uid(), &scope.path, RadioButtonAction::Clicked);
            }
        }
    }
}

impl RadioButtonSet{
    
    pub fn selected(&self, cx: &mut Cx, actions: &Actions)->Option<usize>{
        for action in actions{
            if let Some(action) = action.as_widget_action(){
                match action.cast(){
                    RadioButtonAction::Clicked => if let Some(index) = self.0.iter().position(|v| action.widget_uid == v.widget_uid()){
                        for (i, item) in self.0.iter().enumerate(){
                            if i != index{
                                RadioButtonRef(item.clone()).unselect(cx);
                            }
                        }
                        return Some(index);
                    }
                    _ => ()
                }
            }
        }
        None
    }
    
    pub fn selected_to_visible(&self, cx: &mut Cx, ui:&WidgetRef, actions: &Actions, paths:&[&[LiveId]] ) {
        // find a widget action that is in our radiogroup
        if let Some(index) = self.selected(cx, actions){
            // ok now we set visible
            for (i,path) in paths.iter().enumerate(){
                let widget = ui.widget(path);
                widget.apply_over(cx, live!{visible:(i == index)});
                widget.redraw(cx);
            }
        }
    }
}