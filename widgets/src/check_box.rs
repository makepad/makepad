use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    }
};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub DrawCheckBox = {{DrawCheckBox}} {}
    pub CheckBoxBase = {{CheckBox}} {}
    
    pub CheckBox = <CheckBoxBase> {
        width: Fit, height: Fit,
        padding: <THEME_MSPACE_2> {}
        align: { x: 0., y: 0. }
        
        label_walk: {
            width: Fit, height: Fit,
            margin: <THEME_MSPACE_H_1> { left: 12.5 }
        }
        
        draw_bg: {
            uniform size: 7.5;

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_1_hover: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_1_active: (THEME_COLOR_INSET_PIT_TOP)

            uniform color_2: (THEME_COLOR_INSET_PIT_BOTTOM)
            uniform color_2_hover: (THEME_COLOR_INSET_PIT_BOTTOM)
            uniform color_2_active: (THEME_COLOR_INSET_PIT_BOTTOM)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)

            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_hover: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            uniform mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            uniform mark_color_focus: (#f00)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                match self.check_type {
                    CheckType::Check => {
                        let left = 1.;
                        let sz = self.size - 1.0;

                        let c = vec2(left + sz, self.rect_size.y * 0.5);

                        // Draw background                        
                        sdf.box(left, c.y - sz, sz * 2.0, sz * 2.0, self.border_radius * 0.5);

                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.y + dither),
                                    mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                self.hover
                            )
                        )

                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                    mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ), self.border_size
                        )
                        

                        // Draw mark
                        let szs = sz * 0.5;
                        sdf.move_to(left + 4.0, c.y);
                        sdf.line_to(c.x, c.y + szs);
                        sdf.line_to(c.x + szs, c.y - szs);
                        sdf.stroke(
                            mix(
                                mix(self.mark_color, self.mark_color_hover, self.hover),
                                mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                                self.active
                            ), 1.25
                        );

                    }

                    CheckType::Radio => {
                        let sz = self.size;
                        let left = 0.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.y + dither),
                                    mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                self.hover
                            )
                        )
                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                    mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ), self.border_size
                        )
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
                                        self.mark_color_focus,
                                        self.focus
                                    ),
                                    self.mark_color_hover,
                                    self.hover
                                ),
                                self.active
                            )
                        );
                    }
                    CheckType::Toggle => {
                        let sz = self.size;
                        let left = 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);

                        // Draw background                        
                        sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, self.border_radius * 1.4);
                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.y + dither),
                                    mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                self.hover
                            )
                        )
                        
                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                    mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ), self.border_size
                        )
                            
                        // Draw mark
                        let isz = sz * 0.65;
                        sdf.circle(left + sz + self.active * sz, c.y - 0.5, isz);
                        sdf.circle(left + sz + self.active * sz, c.y - 0.5, 0.425 * isz);
                        sdf.subtract();
                        sdf.circle(left + sz + self.active * sz, c.y - 0.5, isz);
                        sdf.blend(self.active)
                        sdf.fill(
                            mix(
                                mix(self.mark_color, self.mark_color_hover, self.hover),
                                mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                                self.active
                            )
                        )
                    }
                    CheckType::None => {
                        sdf.fill(THEME_COLOR_D_HIDDEN);
                    }
                }
                return sdf.result
            }
        }
            
        draw_text: {
            instance focus: 0.0
            instance hover: 0.0
            instance active: 0.0

            uniform color: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_hover: (THEME_COLOR_TEXT_DEFAULT)
            uniform color_focus: (THEME_COLOR_TEXT_FOCUSED)
            uniform color_active: (THEME_COLOR_TEXT_DEFAULT)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(self.color, self.color_focus, self.focus),
                        self.color_hover,
                        self.hover
                    ),
                    self.color_active,
                    self.active
                )
            }
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
            
        draw_icon: {
            instance focus: 0.0
            instance hover: 0.0
            instance active: 0.0

            uniform color: (THEME_COLOR_D_3)
            uniform color_hover: (THEME_COLOR_D_4)
            uniform color_focus: (THEME_COLOR_TEXT_FOCUSED)
            uniform color_active: (THEME_COLOR_TEXT_ACTIVE)

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        self.color,
                        self.color_hover,
                        self.hover
                    ),
                    self.color_active,
                    self.active
                )
            }
        }
            
        icon_walk: { width: 13.0, height: Fit }
            
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
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            active = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 0.0},
                        draw_text: {active: 0.0},
                        draw_icon: {active: 0.0},
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                        draw_icon: {active: 1.0},
                    }
                }
            }
        }
    }

    pub CheckBoxFlat = <CheckBox> {
        draw_bg: {
            border_size: (THEME_BEVELING)
            border_radius: (THEME_CORNER_RADIUS)

            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_active: (THEME_COLOR_INSET_ACTIVE)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_active: (THEME_COLOR_INSET_ACTIVE)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_active: (THEME_COLOR_BEVEL_FOCUS)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_active: (THEME_COLOR_BEVEL_FOCUS)

            mark_color: (THEME_COLOR_U_HIDDEN)
            mark_color_hover: (THEME_COLOR_U_HIDDEN)
            mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            mark_color_focus: (#f00)

        }

    }

    pub CheckBoxFlatter = <CheckBox> {
        draw_bg: {
            border_size: 0.0
            border_radius: (THEME_CORNER_RADIUS)

            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_active: (THEME_COLOR_INSET_ACTIVE)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_active: (THEME_COLOR_INSET_ACTIVE)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_active: (THEME_COLOR_BEVEL_FOCUS)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_active: (THEME_COLOR_BEVEL_FOCUS)

            mark_color: (THEME_COLOR_U_HIDDEN)
            mark_color_hover: (THEME_COLOR_U_HIDDEN)
            mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            mark_color_focus: (#f00)

        }

    }
        
    pub CheckBoxGradientX = <CheckBox> {
        draw_bg: {
            uniform size: 7.5;

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_1_hover: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_1_active: (THEME_COLOR_INSET_PIT_TOP)

            uniform color_2: (THEME_COLOR_INSET_PIT_BOTTOM)
            uniform color_2_hover: (THEME_COLOR_INSET_PIT_BOTTOM)
            uniform color_2_active: (THEME_COLOR_INSET_PIT_BOTTOM)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)

            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_hover: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            uniform mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            uniform mark_color_focus: (#f00)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                match self.check_type {
                    CheckType::Check => {
                        let left = 1.;
                        let sz = self.size - 1.0;

                        let c = vec2(left + sz, self.rect_size.y * 0.5);

                        // Draw background                        
                        sdf.box(left, c.y - sz, sz * 2.0, sz * 2.0, self.border_radius * 0.5);

                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.x + dither),
                                    mix(self.color_1_active, self.color_2_active, self.pos.x + dither),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.x + dither),
                                self.hover
                            )
                        )

                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                    mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ), self.border_size
                        )

                        // Draw mark
                        let szs = sz * 0.5;
                        sdf.move_to(left + 4.0, c.y);
                        sdf.line_to(c.x, c.y + szs);
                        sdf.line_to(c.x + szs, c.y - szs);
                        sdf.stroke(
                            mix(
                                mix(self.mark_color, self.mark_color_hover, self.hover),
                                mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                                self.active
                            ), 1.25
                        );

                    }

                    CheckType::Radio => {
                        let sz = self.size;
                        let left = 0.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        sdf.circle(left, c.y, sz);
                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.y + dither),
                                    mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                self.hover
                            )
                        )
                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                    mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ), self.border_size
                        )
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
                                        self.mark_color_focus,
                                        self.focus
                                    ),
                                    self.mark_color_hover,
                                    self.hover
                                ),
                                self.active
                            )
                        );
                    }
                    CheckType::Toggle => {
                        let sz = self.size;
                        let left = 1.;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);

                        // Draw background                        
                        sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, self.border_radius * 1.4);
                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, self.pos.y + dither),
                                    mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.color_1_hover, self.color_2_hover, self.pos.y + dither),
                                self.hover
                            )
                        )
                        
                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, self.pos.y + dither),
                                    mix(self.border_color_1_active, self.border_color_2_active, self.pos.y + dither),
                                    self.active
                                ),
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                self.hover
                            ), self.border_size
                        )
                            
                        // Draw mark
                        let isz = sz * 0.65;
                        sdf.circle(left + sz + self.active * sz, c.y - 0.5, isz);
                        sdf.circle(left + sz + self.active * sz, c.y - 0.5, 0.425 * isz);
                        sdf.subtract();
                        sdf.circle(left + sz + self.active * sz, c.y - 0.5, isz);
                        sdf.blend(self.active)
                        sdf.fill(
                            mix(
                                mix(self.mark_color, self.mark_color_hover, self.hover),
                                mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                                self.active
                            )
                        )
                    }
                    CheckType::None => {
                        sdf.fill(THEME_COLOR_D_HIDDEN);
                    }
                }
                return sdf.result
            }
        }

    }

    pub CheckBoxGradientY = <CheckBox> { }

    pub Toggle = <CheckBox> {
        align: { x: 0., y: 0. }
        draw_bg: {
            uniform size: 7.5;

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_1_hover: (THEME_COLOR_INSET_PIT_TOP)
            uniform color_1_active: (THEME_COLOR_INSET_PIT_TOP)

            uniform color_2: (THEME_COLOR_INSET_PIT_BOTTOM)
            uniform color_2_hover: (THEME_COLOR_INSET_PIT_BOTTOM)
            uniform color_2_active: (THEME_COLOR_INSET_PIT_BOTTOM)

            uniform border_color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_SHADOW)

            uniform border_color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_LIGHT)

            uniform mark_color: (THEME_COLOR_TEXT_ACTIVE)
            uniform mark_color_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            uniform mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            uniform mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            uniform mark_color_focus: (#f00)
            check_type: Toggle
        }
        label_walk: {
            margin: <THEME_MSPACE_H_1> { left: 22.5 }
        }
            
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.25}}
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
            focus = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                        draw_icon: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                        draw_icon: {focus: 1.0}
                    }
                }
            }
            active = {
                default: off
                off = {
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 0.0},
                        draw_text: {active: 0.0},
                        draw_icon: {active: 0.0},
                    }
                }
                on = {
                    ease: OutQuad
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                        draw_icon: {active: 1.0},
                    }
                }
            }
        }
    }

    pub ToggleFlat = <Toggle> {
        draw_bg: {
            size: 7.5;

            border_size: (THEME_BEVELING)
            border_radius: (THEME_CORNER_RADIUS)

            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_active: (THEME_COLOR_INSET_ACTIVE)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_active: (THEME_COLOR_INSET_ACTIVE)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL)
            border_color_1_active: (THEME_COLOR_BEVEL)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_active: (THEME_COLOR_BEVEL_ACTIVE)

            mark_color: (THEME_COLOR_TEXT_ACTIVE)
            mark_color_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            mark_color_focus: (#f00)
        }
    }
        
    pub ToggleFlatter = <Toggle> {
        draw_bg: {
            border_size: 0.
            border_radius: (THEME_CORNER_RADIUS)

            color_1: (THEME_COLOR_INSET)
            color_1_hover: (THEME_COLOR_INSET_HOVER)
            color_1_active: (THEME_COLOR_INSET_ACTIVE)

            color_2: (THEME_COLOR_INSET)
            color_2_hover: (THEME_COLOR_INSET_HOVER)
            color_2_active: (THEME_COLOR_INSET_ACTIVE)

            mark_color: (THEME_COLOR_TEXT_ACTIVE)
            mark_color_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            mark_color_active: (THEME_COLOR_TEXT_ACTIVE)
            mark_color_active_hover: (THEME_COLOR_TEXT_ACTIVE * 1.5)
            mark_color_focus: (#f00)
        }
    }

    pub CheckBoxCustom = <CheckBox> {
        draw_bg: { check_type: None }
        align: { x: 0.0, y: 0.5}
        label_walk: { margin: <THEME_MSPACE_H_2> {} }
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawCheckBox {
    #[deref] draw_super: DrawQuad,
    #[live] check_type: CheckType,
    #[live] hover: f32,
    #[live] focus: f32,
    #[live] active: f32
}

#[derive(Live, LiveHook, LiveRegister)]
#[live_ignore]
#[repr(u32)]
pub enum CheckType {
    #[pick] Check = shader_enum(1),
    Radio = shader_enum(2),
    Toggle = shader_enum(3),
    None = shader_enum(4),
}

#[derive(Live, LiveHook, Widget)]
pub struct CheckBox {
    
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[animator] animator: Animator,
    
    #[live] icon_walk: Walk,
    #[live] label_walk: Walk,
    #[live] label_align: Align,
    
    #[redraw] #[live] draw_bg: DrawCheckBox,
    #[live] draw_text: DrawText,
    #[live] draw_icon: DrawIcon,
    
    #[live] text: ArcStringMut,
    
    #[live] bind: String,
    #[action_data] #[rust] action_data: WidgetActionData,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum CheckBoxAction {
    Change(bool),
    None
}

impl CheckBox {
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_bg.end(cx);
    }
}

impl Widget for CheckBox {
    
    fn widget_to_data(&self, _cx: &mut Cx, actions: &Actions, nodes: &mut LiveNodeVec, path: &[LiveId]) -> bool {
        match actions.find_widget_action_cast(self.widget_uid()) {
            CheckBoxAction::Change(v) => {
                nodes.write_field_value(path, LiveValue::Bool(v));
                true
            }
            _ => false
        }
    }
    
    fn data_to_widget(&mut self, cx: &mut Cx, nodes: &[LiveNode], path: &[LiveId]) {
        if let Some(value) = nodes.read_field_value(path) {
            if let Some(value) = value.as_bool() {
                self.animator_toggle(cx, value, Animate::Yes, id!(active.on), id!(active.off));
            }
        }
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
                
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.animator_in_state(cx, id!(active.on)) {
                    self.animator_play(cx, id!(active.off));
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, CheckBoxAction::Change(false));
                }
                else {
                    self.animator_play(cx, id!(active.on));
                    cx.widget_action_with_data(&self.action_data, uid, &scope.path, CheckBoxAction::Change(true));
                }
            },
            Hit::FingerUp(_fe) => {
                                
            }
            Hit::FingerMove(_fe) => {
                                
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
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

impl CheckBoxRef {
    pub fn changed(&self, actions: &Actions) -> Option<bool> {
        if let CheckBoxAction::Change(b) = actions.find_widget_action_cast(self.widget_uid()) {
            return Some(b)
        }
        None
    }
    
    pub fn set_text(&self, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            let s = inner.text.as_mut_empty();
            s.push_str(text);
        }
    }
    
    pub fn active(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.animator_in_state(cx, id!(active.on))
        }
        else {
            false
        }
    }
    
    pub fn set_active(&self, cx: &mut Cx, value: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_toggle(cx, value, Animate::Yes, id!(active.on), id!(active.off));
        }
    }
}
