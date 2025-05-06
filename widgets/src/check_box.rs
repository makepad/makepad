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
            margin: <THEME_MSPACE_H_1> { left: 13. }
        }
        
        draw_bg: {
            instance disabled: 0.0,
            instance down: 0.0,

            uniform size: 14.0;

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_down: (THEME_COLOR_INSET_DOWN)
            uniform color_active: (THEME_COLOR_INSET_ACTIVE)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_disabled: (THEME_COLOR_INSET_DISABLED)

            uniform border_color_1: (THEME_COLOR_BEVEL_INSET_2)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_1_down: (THEME_COLOR_BEVEL_INSET_2_DOWN)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_INSET_2_ACTIVE)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

            uniform border_color_2: (THEME_COLOR_BEVEL_INSET_1)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            uniform border_color_2_down: (THEME_COLOR_BEVEL_INSET_1_DOWN)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_INSET_1_ACTIVE)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)

            uniform mark_size: 0.65
            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_hover: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_down: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_active: (THEME_COLOR_MARK_ACTIVE)
            uniform mark_color_active_hover: (THEME_COLOR_MARK_ACTIVE_HOVER)
            uniform mark_color_focus: (THEME_COLOR_MARK_FOCUS)
            uniform mark_color_disabled: (THEME_COLOR_MARK_DISABLED)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = self.size;
                let sz_inner_px = sz_px - self.border_size * 2.;
                let shift_px = vec2(0, 0)
                let center_px = vec2(
                    sz_px * 0.5,
                    self.rect_size.y * 0.5
                )
                
                let offset_px = vec2(
                    shift_px.x,
                    shift_px.y + center_px.y - sz_px * 0.5
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px,
                    self.rect_size.y / sz_px 
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px,
                    self.rect_size.y / sz_inner_px 
                );

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                match self.check_type {
                    CheckType::Check => {

                        // Draw background
                        sdf.box(
                            offset_px.x + self.border_size,
                            offset_px.y + self.border_size,
                            sz_px - self.border_size * 2.,
                            sz_px - self.border_size * 2.,
                            self.border_radius * 0.5
                        );

                        sdf.stroke_keep(
                            mix(
                                mix(
                                    mix(
                                        mix(
                                            mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                            mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                            self.focus
                                        ),
                                        mix(
                                            mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                                            mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                            self.focus
                                        ),
                                        self.active
                                    ),
                                    mix(
                                        mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                                        mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                        self.down
                                    ),
                                    self.hover
                                ),
                                mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                                self.disabled
                            ), self.border_size
                        )

                        sdf.fill(
                            mix(
                                mix(
                                    mix(
                                        mix(
                                            self.color,
                                            self.color_focus,
                                            self.focus
                                        ),
                                        mix(
                                            self.color_active,
                                            self.color_focus,
                                            self.focus
                                        ),
                                        self.active
                                    ),
                                    mix(
                                        self.color_hover,
                                        self.color_down,
                                        self.down
                                    ),
                                    self.hover
                                ),
                                self.color_disabled,
                                self.disabled
                            )
                        )

                        // Draw mark
                        let mark_padding = 0.275 * self.size
                        sdf.move_to(mark_padding, center_px.y);
                        sdf.line_to(center_px.x, center_px.y + sz_px * 0.5 - mark_padding);
                        sdf.line_to(sz_px - mark_padding, offset_px.y + mark_padding);

                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.mark_color, self.mark_color_hover, self.hover),
                                    mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                                    self.active
                                ),
                                self.mark_color_disabled,
                                self.disabled
                            ), self.size * 0.09
                        );

                    }

                    // CheckType::Toggle => { }

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
            instance down: 0.0
            instance active: 0.0
            instance disabled: 0.0

            uniform color: (THEME_COLOR_LABEL_OUTER)
            uniform color_hover: (THEME_COLOR_LABEL_OUTER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_OUTER_DOWN)
            uniform color_focus: (THEME_COLOR_LABEL_OUTER_FOCUS)
            uniform color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE)
            uniform color_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)

            fn get_color(self) -> vec4 {
                return
                    mix(
                        mix(
                            mix(
                                mix(self.color, self.color_active, self.active),
                                self.color_focus,
                                self.focus
                            ),
                            mix(
                                self.color_hover,
                                self.color_down,
                                self.down
                            ),
                            self.hover
                        ),
                        self.color_disabled,
                        self.disabled
                    )
            }
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
        }
            
        draw_icon: {
            instance active: 0.0
            instance disabled: 0.0

            uniform color: (THEME_COLOR_ICON)
            uniform color_active: (THEME_COLOR_ICON_ACTIVE)
            uniform color_disabled: (THEME_COLOR_ICON_DISABLED)

            fn get_color(self) -> vec4 {
                return
                    mix(
                        mix(
                            self.color,
                            self.color_active,
                            self.active
                        ),
                        self.color_disabled,
                        self.disabled
                    )

            }
        }
            
        icon_walk: {
            width: 14.0, height: Fit
        }
            
        animator: {
            disabled = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                        draw_icon: {disabled: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_icon: {disabled: 1.0}
                    }
                }
            }
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 0.0}], hover: 0.0}
                        draw_text: {down: [{time: 0.0, value: 0.0}], hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 0.0}], hover: 1.0}
                        draw_text: {down: [{time: 0.0, value: 0.0}], hover: 1.0}
                    }
                }
                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
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
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
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
            color: (THEME_COLOR_INSET)
            color_hover: (THEME_COLOR_INSET_HOVER)
            color_down: (THEME_COLOR_INSET_DOWN)
            color_active: (THEME_COLOR_INSET_ACTIVE)
            color_focus: (THEME_COLOR_INSET_FOCUS)
            color_disabled: (THEME_COLOR_INSET_DISABLED)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_down: (THEME_COLOR_BEVEL_DOWN)
            border_color_1_active: (THEME_COLOR_BEVEL_ACTIVE)
            border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_1_down: (THEME_COLOR_BEVEL_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_down: (THEME_COLOR_BEVEL_DOWN)
            border_color_2_active: (THEME_COLOR_BEVEL_ACTIVE)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_2_down: (THEME_COLOR_BEVEL_DISABLED)
        }

    }

    pub CheckBoxFlatter = <CheckBoxFlat> {
        draw_bg: {
            border_color_1: (THEME_COLOR_U_HIDDEN)
            border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            border_color_1_down: (THEME_COLOR_U_HIDDEN)
            border_color_1_active: (THEME_COLOR_U_HIDDEN)
            border_color_1_focus: (THEME_COLOR_U_HIDDEN)
            border_color_1_disabled: (THEME_COLOR_U_HIDDEN)

            border_color_2: (THEME_COLOR_U_HIDDEN)
            border_color_2_hover: (THEME_COLOR_U_HIDDEN)
            border_color_2_down: (THEME_COLOR_U_HIDDEN)
            border_color_2_active: (THEME_COLOR_U_HIDDEN)
            border_color_2_focus: (THEME_COLOR_U_HIDDEN)
            border_color_2_disabled: (THEME_COLOR_U_HIDDEN)
        }

    }

    pub CheckBoxGradientY = <CheckBox> {
        width: Fit, height: Fit,
        padding: <THEME_MSPACE_2> {}
        align: { x: 0., y: 0. }
        
        label_walk: {
            width: Fit, height: Fit,
            margin: <THEME_MSPACE_H_1> { left: 13. }
        }
        
        draw_bg: {
            instance disabled: 0.0,
            instance down: 0.0,

            uniform size: 15.0;

            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_INSET_1)
            uniform color_1_hover: (THEME_COLOR_INSET_1_HOVER)
            uniform color_1_down: (THEME_COLOR_INSET_1_DOWN)
            uniform color_1_active: (THEME_COLOR_INSET_1_ACTIVE)
            uniform color_1_focus: (THEME_COLOR_INSET_1_FOCUS)
            uniform color_1_disabled: (THEME_COLOR_INSET_1_DISABLED)

            uniform color_2: (THEME_COLOR_INSET_2)
            uniform color_2_hover: (THEME_COLOR_INSET_2_HOVER)
            uniform color_2_down: (THEME_COLOR_INSET_2_DOWN)
            uniform color_2_active: (THEME_COLOR_INSET_2_ACTIVE)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
            uniform color_2_disabled: (THEME_COLOR_INSET_2_DISABLED)

            uniform border_color_1: (THEME_COLOR_BEVEL_INSET_2)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_1_down: (THEME_COLOR_BEVEL_INSET_2_DOWN)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_INSET_2_ACTIVE)
            uniform border_color_1_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

            uniform border_color_2: (THEME_COLOR_BEVEL_INSET_1)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            uniform border_color_2_down: (THEME_COLOR_BEVEL_INSET_1_DOWN)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_INSET_1_ACTIVE)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)

            uniform mark_size: 0.65
            uniform mark_color: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_hover: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_down: (THEME_COLOR_U_HIDDEN)
            uniform mark_color_active: (THEME_COLOR_MARK_ACTIVE)
            uniform mark_color_active_hover: (THEME_COLOR_MARK_ACTIVE_HOVER)
            uniform mark_color_focus: (THEME_COLOR_MARK_FOCUS)
            uniform mark_color_disabled: (THEME_COLOR_MARK_DISABLED)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = self.size;
                let sz_inner_px = sz_px - self.border_size * 2.;
                let shift_px = vec2(0, 0)
                let center_px = vec2(
                    sz_px * 0.5,
                    self.rect_size.y * 0.5
                )
                
                let offset_px = vec2(
                    shift_px.x,
                    shift_px.y + center_px.y - sz_px * 0.5
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px,
                    self.rect_size.y / sz_px 
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px,
                    self.rect_size.y / sz_inner_px 
                );

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                match self.check_type {
                    CheckType::Check => {

                        // Draw background
                        sdf.box(
                            offset_px.x + self.border_size,
                            offset_px.y + self.border_size,
                            sz_px - self.border_size * 2.,
                            sz_px - self.border_size * 2.,
                            self.border_radius * 0.5
                        );

                        sdf.stroke_keep(
                            mix(
                                mix(
                                    mix(
                                        mix(
                                            mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                            mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                            self.focus
                                        ),
                                        mix(
                                            mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                                            mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                            self.focus
                                        ),
                                        self.active
                                    ),
                                    mix(
                                        mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                                        mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                        self.down
                                    ),
                                    self.hover
                                ),
                                mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                                self.disabled
                            ), self.border_size
                        )

                        sdf.fill(
                            mix(
                                mix(
                                    mix(
                                        mix(
                                            mix(self.color_1, self.color_2, gradient_fill.y),
                                            mix(self.color_1_focus, self.color_2_focus, gradient_fill.y),
                                            self.focus
                                        ),
                                        mix(
                                            mix(self.color_1_active, self.color_2_active, gradient_fill.y),
                                            mix(self.color_1_focus, self.color_2_focus, gradient_fill.y),
                                            self.focus
                                        ),
                                        self.active
                                    ),
                                    mix(
                                        mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                                        mix(self.color_1_down, self.color_2_down, gradient_fill.y),
                                        self.down
                                    ),
                                    self.hover
                                ),
                                mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
                                self.disabled
                            )
                        )

                        // Draw mark
                        let mark_padding = 0.275 * self.size
                        sdf.move_to(mark_padding, center_px.y);
                        sdf.line_to(center_px.x, center_px.y + sz_px * 0.5 - mark_padding);
                        sdf.line_to(sz_px - mark_padding, offset_px.y + mark_padding);

                        sdf.stroke(
                            mix(
                                mix(
                                    mix(self.mark_color, self.mark_color_hover, self.hover),
                                    mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                                    self.active
                                ),
                                self.mark_color_disabled,
                                self.disabled
                            ), self.size * 0.09
                        );

                    }

                    // CheckType::Toggle => { }

                    CheckType::None => {
                        sdf.fill(THEME_COLOR_D_HIDDEN);
                    }
                }
                return sdf.result
            }
        }
    }
        
    pub CheckBoxGradientX = <CheckBoxGradientY> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = self.size;
                let sz_inner_px = sz_px - self.border_size * 2.;
                let shift_px = vec2(0, 0)
                let center_px = vec2(
                    sz_px * 0.5,
                    self.rect_size.y * 0.5
                )
                
                let offset_px = vec2(
                    shift_px.x,
                    shift_px.y + center_px.y - sz_px * 0.5
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px,
                    self.rect_size.y / sz_px 
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px,
                    self.rect_size.y / sz_inner_px 
                );

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                // Draw background
                sdf.box(
                    offset_px.x + self.border_size,
                    offset_px.y + self.border_size,
                    sz_px - self.border_size * 2.,
                    sz_px - self.border_size * 2.,
                    self.border_radius * 0.5
                );

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                mix(
                                    mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                self.active
                            ),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, gradient_fill.x),
                                    mix(self.color_1_focus, self.color_2_focus, gradient_fill.x),
                                    self.focus
                                ),
                                mix(
                                    mix(self.color_1_active, self.color_2_active, gradient_fill.x),
                                    mix(self.color_1_focus, self.color_2_focus, gradient_fill.x),
                                    self.focus
                                ),
                                self.active
                            ),
                            mix(
                                mix(self.color_1_hover, self.color_2_hover, gradient_fill.x),
                                mix(self.color_1_down, self.color_2_down, gradient_fill.x),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.x),
                        self.disabled
                    )
                )

                // Draw mark
                let mark_padding = 0.275 * self.size
                sdf.move_to(mark_padding, center_px.y);
                sdf.line_to(center_px.x, center_px.y + sz_px * 0.5 - mark_padding);
                sdf.line_to(sz_px - mark_padding, offset_px.y + mark_padding);

                sdf.stroke(
                    mix(
                        mix(
                            mix(self.mark_color, self.mark_color_hover, self.hover),
                            mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                            self.active
                        ),
                        self.mark_color_disabled,
                        self.disabled
                    ), self.size * 0.09
                );

                return sdf.result
            }
        } 
    }

    pub Toggle = <CheckBox> {
        label_walk: {
            margin: <THEME_MSPACE_H_1> { left: (15.0 + THEME_SPACE_2) }
        }

        draw_bg: {
            uniform size: 15.;

            mark_color: (THEME_COLOR_LABEL_OUTER)
            mark_color_hover: (THEME_COLOR_LABEL_OUTER_ACTIVE)
            mark_color_down: (THEME_COLOR_LABEL_OUTER_DOWN)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = vec2(
                    self.size * 1.6,
                    self.size
                );
                let sz_inner_px = vec2(
                    sz_px.x - self.border_size * 2.,
                    sz_px.y - self.border_size * 2.
                );
                let shift_px = vec2(0., 0.);
                let center_px = vec2(
                    sz_px.x * 0.5,
                    self.rect_size.y * 0.5
                )
                
                let offset_px = vec2(
                    shift_px.x,
                    shift_px.y + center_px.y - sz_px.y * 0.5
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px.x,
                    self.rect_size.y / sz_px.y
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                // Draw background                        
                sdf.box(
                    offset_px.x + self.border_size,
                    offset_px.y + self.border_size,
                    sz_px.x - self.border_size * 2.,
                    sz_px.y - self.border_size * 2.,
                    self.border_radius * self.size * 0.1
                );

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                mix(
                                    mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                self.active
                            ),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(
                                    self.color,
                                    self.color_active,
                                    self.active
                                ),
                                self.color_focus,
                                self.focus
                            ),
                            mix(
                                self.color_hover,
                                self.color_down,
                                self.down
                            ),
                            self.hover
                        ),
                        self.color_disabled,
                        self.disabled
                    )
                )

                    // mix(
                    //     mix(
                    //         mix(
                    //             mix(
                    //                 mix(self.color_1, self.color_2, gradient_fill.y),
                    //                 mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                    //                 self.hover
                    //             ),
                    //             mix(
                    //                 mix(self.color_1_active, self.color_2_active, gradient_fill.y),
                    //                 mix(self.color_1_focus, self.color_2_focus, gradient_fill.y),
                    //                 self.focus
                    //             ),
                    //             self.active
                    //         ),
                    //         mix(
                    //             // mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                    //             #f00,
                    //             mix(self.color_1_down, self.color_2_down, gradient_fill.y),
                    //             self.down
                    //         ),
                    //         self.hover
                    //     ),
                    //     mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
                    //     self.disabled
                    // )
                // )
                    
                // Draw mark
                let mark_padding = 1.5;
                let mark_size = sz_px.y * 0.5 - self.border_size - mark_padding;
                let mark_target_y = sz_px.y - sz_px.x + self.border_size + mark_padding;
                let mark_pos_y = sz_px.y * 0.5 + self.border_size - mark_target_y * self.active;

                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size
                );
                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size * 0.45
                );
                sdf.subtract();

                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size
                );

                sdf.blend(self.active)

                sdf.fill(
                    mix(
                        mix(
                            mix(self.mark_color, self.mark_color_hover, self.hover),
                            mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                            self.active
                        ),
                        self.mark_color_disabled,
                        self.disabled
                    )
                )
                return sdf.result
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
                        draw_icon: {disabled: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_icon: {disabled: 1.0}
                    }
                }
            }
        
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 0.0}], hover: 0.0}
                        draw_text: {down: [{time: 0.0, value: 0.0}], hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 0.0}], hover: 1.0}
                        draw_text: {down: [{time: 0.0, value: 0.0}], hover: 1.0}
                    }
                }
                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
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
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
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

    pub ToggleGradientY = <CheckBoxGradientY> {
        label_walk: {
            margin: <THEME_MSPACE_H_1> { left: (15.0 + THEME_SPACE_2) }
        }

        draw_bg: {
            uniform size: 15.;

            mark_color: (THEME_COLOR_LABEL_OUTER)
            mark_color_hover: (THEME_COLOR_LABEL_OUTER_ACTIVE)
            mark_color_down: (THEME_COLOR_LABEL_OUTER_DOWN)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = vec2(
                    self.size * 1.6,
                    self.size
                );
                let sz_inner_px = vec2(
                    sz_px.x - self.border_size * 2.,
                    sz_px.y - self.border_size * 2.
                );
                let shift_px = vec2(0., 0.);
                let center_px = vec2(
                    sz_px.x * 0.5,
                    self.rect_size.y * 0.5
                )
                
                let offset_px = vec2(
                    shift_px.x,
                    shift_px.y + center_px.y - sz_px.y * 0.5
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px.x,
                    self.rect_size.y / sz_px.y
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                // Draw background                        
                sdf.box(
                    offset_px.x + self.border_size,
                    offset_px.y + self.border_size,
                    sz_px.x - self.border_size * 2.,
                    sz_px.y - self.border_size * 2.,
                    self.border_radius * self.size * 0.1
                );

                sdf.stroke_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                mix(
                                    mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                self.active
                            ),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, gradient_fill.y),
                                    mix(self.color_1_active, self.color_2_active, gradient_fill.y),
                                    self.active
                                ),
                                mix(self.color_1_focus, self.color_2_focus, gradient_fill.y),
                                self.focus
                            ),
                            mix(
                                mix(self.color_1_hover, self.color_2_hover, gradient_fill.y),
                                mix(self.color_1_down, self.color_2_down, gradient_fill.y),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
                        self.disabled
                    )
                )
                    
                // Draw mark
                let mark_padding = 1.5;
                let mark_size = sz_px.y * 0.5 - self.border_size - mark_padding;
                let mark_target_y = sz_px.y - sz_px.x + self.border_size + mark_padding;
                let mark_pos_y = sz_px.y * 0.5 + self.border_size - mark_target_y * self.active;

                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size
                );
                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size * 0.45
                );
                sdf.subtract();

                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size
                );

                sdf.blend(self.active)

                sdf.fill(
                    mix(
                        mix(
                            mix(self.mark_color, self.mark_color_hover, self.hover),
                            mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                            self.active
                        ),
                        self.mark_color_disabled,
                        self.disabled
                    )
                )
                return sdf.result
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
                        draw_icon: {disabled: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_icon: {disabled: 1.0}
                    }
                }
            }
        
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 0.0}], hover: 0.0}
                        draw_text: {down: [{time: 0.0, value: 0.0}], hover: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 0.0}], hover: 1.0}
                        draw_text: {down: [{time: 0.0, value: 0.0}], hover: 1.0}
                    }
                }
                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
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
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
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
            color: (THEME_COLOR_INSET)
            color_hover: (THEME_COLOR_INSET_HOVER)
            color_down: (THEME_COLOR_INSET_DOWN)
            color_active: (THEME_COLOR_INSET_ACTIVE)
            color_focus: (THEME_COLOR_INSET_FOCUS)
            color_down: (THEME_COLOR_INSET_DISABLED)

            border_color_1: (THEME_COLOR_BEVEL)
            border_color_1_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_1_down: (THEME_COLOR_BEVEL_DOWN)
            border_color_1_active: (THEME_COLOR_BEVEL_ACTIVE)
            border_color_1_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_1_disabled: (THEME_COLOR_BEVEL_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_down: (THEME_COLOR_BEVEL_DOWN)
            border_color_2_active: (THEME_COLOR_BEVEL_ACTIVE)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_2_disabled: (THEME_COLOR_BEVEL_DISABLED)
        }
    }
        
    pub ToggleFlatter = <ToggleFlat> {
        draw_bg: {
            border_color_1: (THEME_COLOR_U_HIDDEN)
            border_color_1_hover: (THEME_COLOR_U_HIDDEN)
            border_color_1_down: (THEME_COLOR_U_HIDDEN)
            border_color_1_active: (THEME_COLOR_U_HIDDEN)
            border_color_1_focus: (THEME_COLOR_U_HIDDEN)
            border_color_1_disabled: (THEME_COLOR_U_HIDDEN)

            border_color_2: (THEME_COLOR_U_HIDDEN)
            border_color_2_hover: (THEME_COLOR_U_HIDDEN)
            border_color_2_down: (THEME_COLOR_U_HIDDEN)
            border_color_2_active: (THEME_COLOR_U_HIDDEN)
            border_color_2_focus: (THEME_COLOR_U_HIDDEN)
            border_color_2_disabled: (THEME_COLOR_U_HIDDEN)
        }
    }

    pub ToggleGradientX = <ToggleGradientY> {
        draw_bg: {
            uniform size: 15.;

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = vec2(
                    self.size * 1.6,
                    self.size
                );
                let sz_inner_px = vec2(
                    sz_px.x - self.border_size * 2.,
                    sz_px.y - self.border_size * 2.
                );
                let shift_px = vec2(0., 0.);
                let center_px = vec2(
                    sz_px.x * 0.5,
                    self.rect_size.y * 0.5
                )
                
                let offset_px = vec2(
                    shift_px.x,
                    shift_px.y + center_px.y - sz_px.y * 0.5
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px.x,
                    self.rect_size.y / sz_px.y
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                // Draw background                        
                sdf.box(
                    offset_px.x + self.border_size,
                    offset_px.y + self.border_size,
                    sz_px.x - self.border_size * 2.,
                    sz_px.y - self.border_size * 2.,
                    self.border_radius * self.size * 0.1
                );

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, gradient_fill.x),
                                    mix(self.color_1_focus, self.color_2_focus, gradient_fill.x),
                                    self.focus
                                ),
                                mix(
                                    mix(self.color_1_active, self.color_2_active, gradient_fill.x),
                                    mix(self.color_1_focus, self.color_2_focus, gradient_fill.x),
                                    self.focus
                                ),
                                self.active
                            ),
                            mix(
                                mix(self.color_1_hover, self.color_2_hover, gradient_fill.x),
                                mix(self.color_1_down, self.color_2_down, gradient_fill.x),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.x),
                        self.disabled
                    )
                )
 
                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color_1, self.border_color_2, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                mix(
                                    mix(self.border_color_1_active, self.border_color_2_active, gradient_border.y),
                                    mix(self.border_color_1_focus, self.border_color_2_focus, gradient_border.y),
                                    self.focus
                                ),
                                self.active
                            ),
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, gradient_border.y),
                                mix(self.border_color_1_down, self.border_color_2_down, gradient_border.y),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, gradient_border.y),
                        self.disabled
                    ), self.border_size
                )


                // Draw mark
                let mark_padding = 1.5;
                let mark_size = sz_px.y * 0.5 - self.border_size - mark_padding;
                let mark_target_y = sz_px.y - sz_px.x + self.border_size + mark_padding;
                let mark_pos_y = sz_px.y * 0.5 + self.border_size - mark_target_y * self.active;

                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size
                );
                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size * 0.45
                );
                sdf.subtract();

                sdf.circle(
                    mark_pos_y,
                    center_px.y,
                    mark_size
                );

                sdf.blend(self.active)

                sdf.fill(
                    mix(
                        mix(
                            mix(self.mark_color, self.mark_color_hover, self.hover),
                            mix(self.mark_color_active, self.mark_color_active_hover, self.hover),
                            self.active
                        ),
                        self.mark_color_disabled,
                        self.disabled
                    )
                )
                return sdf.result
            }
        }
    }

    pub CheckBoxCustom = <CheckBox> {
        draw_bg: { check_type: None }
        width: Fit, height: Fit,

        padding: <THEME_MSPACE_2> {}
        align: { x: 0., y: 0.5 }

        label_walk: {
            margin: <THEME_MSPACE_H_2> {}
        }
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

#[derive(Live, Widget)]
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

    #[visible] #[live(true)]
    pub visible: bool,
    
    #[live(None)]
    pub active: Option<bool>,
    
    #[live] bind: String,
    #[action_data] #[rust] action_data: WidgetActionData,
}

// map the 'active' bool to the animator state
impl LiveHook for CheckBox{
    fn after_new_from_doc(&mut self, cx: &mut Cx){
        if let Some(active) = self.active.take() {
            self.animator_toggle(cx, active, Animate::No, id!(active.on), id!(active.off));
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum CheckBoxAction {
    Change(bool),
    None
}

impl CheckBox {
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_text.draw_walk(cx, self.label_walk, self.label_align, self.text.as_ref());
        self.draw_bg.end(cx);
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());
        DrawStep::done() 
   }
}

impl Widget for CheckBox {

    fn set_disabled(&mut self, cx:&mut Cx, disabled:bool){
        self.animator_toggle(cx, disabled, Animate::Yes, id!(disabled.on), id!(disabled.off));
    }
                
    fn disabled(&self, cx:&Cx) -> bool {
        self.animator_in_state(cx, id!(disabled.on))
    }
    
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
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.set_key_focus(cx);
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
        if !self.visible {
            return DrawStep::done();
        }
        self.draw_walk(cx, walk)
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
