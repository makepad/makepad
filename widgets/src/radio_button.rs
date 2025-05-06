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
        width: Fit, height: Fit,
        align: { x: 0., y: 0. }
        padding: <THEME_MSPACE_V_2> { left: (THEME_SPACE_2)}
        
        icon_walk: { margin: { left: 20. } }
        
        label_walk: {
            width: Fit, height: Fit,
            margin: <THEME_MSPACE_H_1> { left: 13. }
        }
        label_align: { y: 0.0 }
        
        draw_bg: {
            instance disabled: 0.,
            instance down: 0.,

            uniform size: 15.0,

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

            uniform mark_color: (THEME_COLOR_MARK_OFF)
            uniform mark_color_active: (THEME_COLOR_MARK_ACTIVE)
            uniform mark_color_disabled: (THEME_COLOR_MARK_DISABLED)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = self.size;
                let sz_inner_px = sz_px - self.border_size * 4.;

                let radius_px = sz_px * 0.5

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let center_px = vec2(
                    radius_px,
                    self.rect_size.y * 0.5
                )

                let offset_px = vec2(
                    0.,
                    center_px.y - radius_px
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px,
                    self.rect_size.y / sz_px
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px,
                    self.rect_size.y / sz_inner_px 
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x - border_sz_uv.x * 2.) * scale_factor_fill.x + dither,
                    (self.pos.y - offset_uv.y - border_sz_uv.y * 2.) * scale_factor_fill.y + dither
                )

                match self.radio_type {
                    RadioType::Round => {

                        // Draw background
                        sdf.circle(
                            center_px.x,
                            center_px.y,
                            radius_px - self.border_size
                        );

                        sdf.fill_keep(
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
                        sdf.circle(
                            center_px.x,
                            center_px.y,
                            radius_px * 0.5 - self.border_size * 0.75
                        );

                        sdf.fill(
                            mix(
                                mix(
                                    self.mark_color,
                                    self.mark_color_active,
                                    self.active
                                ),
                                self.mark_color_disabled,
                                self.disabled
                            )
                        );
                    }
                    RadioType::Tab => {
                        let border_sz_uv = vec2(
                            self.border_size / self.rect_size.x,
                            self.border_size / self.rect_size.y
                        )

                        let scale_factor_border = vec2(
                            self.rect_size.x / self.rect_size.x,
                            self.rect_size.y / self.rect_size.y
                        );

                        let gradient_border = vec2(
                            self.pos.x * scale_factor_border.x + dither,
                            self.pos.y * scale_factor_border.y + dither
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

                        sdf.fill_keep(
                            mix(
                                mix(
                                    self.color,
                                    self.color_active,
                                    self.active
                                ),
                                self.color_disabled,
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
                            
                    }
                }
                return sdf.result
            }
        }
            
        draw_text: {
            instance active: 0.0
            instance focus: 0.0
            instance down: 0.,
            instance hover: 0.0
            instance disabled: 0.,
                
            uniform color: (THEME_COLOR_LABEL_OUTER)
            uniform color_hover: (THEME_COLOR_LABEL_OUTER_HOVER)
            uniform color_down: (THEME_COLOR_LABEL_OUTER_DOWN)
            uniform color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE)
            uniform color_focus: (THEME_COLOR_LABEL_OUTER_FOCUS)
            uniform color_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)
                
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
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
        }
            
        draw_icon: {
            instance focus: 0.0
            instance disabled: 0.,
            instance active: 0.0

            uniform color_dither: 1.0
            uniform color_1: (THEME_COLOR_LABEL_OUTER)
            uniform color_1_active: (THEME_COLOR_LABEL_OUTER_ACTIVE)
            uniform color_1_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)

            uniform color_2: (THEME_COLOR_LABEL_OUTER)
            uniform color_2_active: (THEME_COLOR_LABEL_OUTER_ACTIVE)
            uniform color_2_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)

            fn get_color(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                return
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                            self.active
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, self.pos.y + dither),
                        self.disabled
                    )
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
                        draw_text: {focus: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }
        

    pub RadioButtonGradientY = <RadioButton> {
        draw_bg: {
            instance disabled: 0.,
            instance down: 0.,

            uniform size: 15.0,

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

            uniform mark_color: (THEME_COLOR_MARK_OFF)
            uniform mark_color_active: (THEME_COLOR_MARK_ACTIVE)
            uniform mark_color_disabled: (THEME_COLOR_MARK_DISABLED)
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = self.size;
                let sz_inner_px = sz_px - self.border_size * 4.;

                let radius_px = sz_px * 0.5

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let center_px = vec2(
                    radius_px,
                    self.rect_size.y * 0.5
                )

                let offset_px = vec2(
                    0.,
                    center_px.y - radius_px
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px,
                    self.rect_size.y / sz_px
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px,
                    self.rect_size.y / sz_inner_px 
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x - border_sz_uv.x * 2.) * scale_factor_fill.x + dither,
                    (self.pos.y - offset_uv.y - border_sz_uv.y * 2.) * scale_factor_fill.y + dither
                )

                match self.radio_type {
                    RadioType::Round => {

                        // Draw background
                        sdf.circle(
                            center_px.x,
                            center_px.y,
                            radius_px - self.border_size
                        );

                        sdf.fill_keep(
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
                        sdf.circle(
                            center_px.x,
                            center_px.y,
                            radius_px * 0.5 - self.border_size * 0.75
                        );

                        sdf.fill(
                            mix(
                                mix(
                                    self.mark_color,
                                    self.mark_color_active,
                                    self.active
                                ),
                                self.mark_color_disabled,
                                self.disabled
                            )
                        );
                    }
                    RadioType::Tab => {
                        let border_sz_uv = vec2(
                            self.border_size / self.rect_size.x,
                            self.border_size / self.rect_size.y
                        )

                        let scale_factor_border = vec2(
                            self.rect_size.x / self.rect_size.x,
                            self.rect_size.y / self.rect_size.y
                        );

                        let gradient_border = vec2(
                            self.pos.x * scale_factor_border.x + dither,
                            self.pos.y * scale_factor_border.y + dither
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

                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, gradient_fill.y),
                                    mix(self.color_1_active, self.color_2_active, gradient_fill.y),
                                    self.active
                                ),
                                mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
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
                            
                    }
                }
                return sdf.result
            }
        }
    }

    pub RadioButtonGradientX = <RadioButtonGradientY> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let sz_px = self.size;
                let sz_inner_px = sz_px - self.border_size * 4.;

                let radius_px = sz_px * 0.5

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let center_px = vec2(
                    radius_px,
                    self.rect_size.y * 0.5
                )

                let offset_px = vec2(
                    0.,
                    center_px.y - radius_px
                )

                let offset_uv = vec2(
                    offset_px.x / self.rect_size.x,
                    offset_px.y / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / sz_px,
                    self.rect_size.y / sz_px
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px,
                    self.rect_size.y / sz_inner_px 
                );

                let gradient_border = vec2(
                    (self.pos.x - offset_uv.x) * scale_factor_border.x + dither,
                    (self.pos.y - offset_uv.y) * scale_factor_border.y + dither
                )

                let gradient_fill = vec2(
                    (self.pos.x - offset_uv.x - border_sz_uv.x * 2.) * scale_factor_fill.x + dither,
                    (self.pos.y - offset_uv.y - border_sz_uv.y * 2.) * scale_factor_fill.y + dither
                )

                match self.radio_type {
                    RadioType::Round => {

                        // Draw background
                        sdf.circle(
                            center_px.x,
                            center_px.y,
                            radius_px - self.border_size
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
                        sdf.circle(
                            center_px.x,
                            center_px.y,
                            radius_px * 0.5 - self.border_size * 0.75
                        );

                        sdf.fill(
                            mix(
                                mix(
                                    self.mark_color,
                                    self.mark_color_active,
                                    self.active
                                ),
                                self.mark_color_disabled,
                                self.disabled
                            )
                        );
                    }
                    RadioType::Tab => {
                        let border_sz_uv = vec2(
                            self.border_size / self.rect_size.x,
                            self.border_size / self.rect_size.y
                        )

                        let scale_factor_border = vec2(
                            self.rect_size.x / self.rect_size.x,
                            self.rect_size.y / self.rect_size.y
                        );

                        let gradient_border = vec2(
                            self.pos.x * scale_factor_border.x + dither,
                            self.pos.y * scale_factor_border.y + dither
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

                        sdf.fill_keep(
                            mix(
                                mix(
                                    mix(self.color_1, self.color_2, gradient_fill.y),
                                    mix(self.color_1_active, self.color_2_active, gradient_fill.y),
                                    self.active
                                ),
                                mix(self.color_1_disabled, self.color_2_disabled, gradient_fill.y),
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
                            
                    }
                }
                return sdf.result
            }
        }
    }

    pub RadioButtonFlat = <RadioButton> {
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
            border_color_1_disabled: (THEME_COLOR_BEVEL_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL)
            border_color_2_hover: (THEME_COLOR_BEVEL_HOVER)
            border_color_2_down: (THEME_COLOR_BEVEL_DOWN)
            border_color_2_active: (THEME_COLOR_BEVEL_ACTIVE)
            border_color_2_focus: (THEME_COLOR_BEVEL_FOCUS)
            border_color_2_disabled: (THEME_COLOR_BEVEL_DISABLED)

            uniform mark_color_active: (THEME_COLOR_MARK_ACTIVE)
        }

    }

    pub RadioButtonFlatter = <RadioButtonFlat> {
        draw_bg: {
            border_size: 0.
        }

    }
         
    pub RadioButtonCustom = <RadioButton> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }

        draw_icon: {
            color_1: (THEME_COLOR_MARK_EMPTY)
            color_1_active: (THEME_COLOR_MARK_ACTIVE)
            color_1_disabled: (THEME_COLOR_MARK_DISABLED)

            color_2: (THEME_COLOR_MARK_EMPTY)
            color_2_active: (THEME_COLOR_MARK_ACTIVE)
            color_2_disabled: (THEME_COLOR_MARK_DISABLED)
        }
        margin: { left: -17.5 }

        label_walk: { margin: { left: (THEME_SPACE_2) } }
    }
        
    pub RadioButtonTextual = <RadioButton> {
        draw_text: {
            color: (THEME_COLOR_LABEL_OUTER_OFF)
            color_hover: (THEME_COLOR_LABEL_OUTER_HOVER)
            color_down: (THEME_COLOR_LABEL_OUTER_DOWN)
            color_active: (THEME_COLOR_LABEL_OUTER_ACTIVE)
            color_disabled: (THEME_COLOR_LABEL_OUTER_DISABLED)
        }

        label_walk: { margin: 0. }

        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                return sdf.result
            }
        }
    }
        
    pub RadioButtonTab = <RadioButton> {
        height: Fit,
        label_walk: {
            margin: {
                left: (THEME_SPACE_3 * 2.)
                right: (THEME_SPACE_1)
            }
        }

        draw_bg: {
            radio_type: Tab

            color: (THEME_COLOR_OUTSET)
            color_active: (THEME_COLOR_OUTSET_ACTIVE)
            color_disabled: (THEME_COLOR_OUTSET_DISABLED)

            border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
            border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1_HOVER)
            border_color_1_down: (THEME_COLOR_BEVEL_OUTSET_1_DOWN)
            border_color_1_active: (THEME_COLOR_BEVEL_OUTSET_1_ACTIVE)
            border_color_1_focus: (THEME_COLOR_BEVEL_OUTSET_1_FOCUS)
            border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
            border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2_HOVER)
            border_color_2_down: (THEME_COLOR_BEVEL_OUTSET_2_DOWN)
            border_color_2_active: (THEME_COLOR_BEVEL_OUTSET_2_ACTIVE)
            border_color_2_focus: (THEME_COLOR_BEVEL_OUTSET_2_FOCUS)
            border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)
        }

        padding: <THEME_MSPACE_2> { left: (THEME_SPACE_2 * -1.25)}
            
        draw_text: {
            color: (THEME_COLOR_LABEL_INNER)
            color_hover: (THEME_COLOR_LABEL_INNER_HOVER)
            color_down: (THEME_COLOR_LABEL_INNER_DOWN)
            color_active: (THEME_COLOR_LABEL_INNER_ACTIVE)
            color_disabled: (THEME_COLOR_LABEL_INNER_DISABLED)
        }
    }
    

    pub RadioButtonTabFlat = <RadioButtonTab> {
        draw_bg: {
            color: (THEME_COLOR_OUTSET)
            color_active: (THEME_COLOR_OUTSET_ACTIVE)
            color_disabled: (THEME_COLOR_OUTSET_DISABLED)

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

    pub RadioButtonTabFlatter = <RadioButtonTabFlat> { draw_bg: { border_size: 0.  }
    }

    pub RadioButtonTabGradientX = <RadioButtonTab> {
        draw_bg: {
            uniform border_size: (THEME_BEVELING)
            uniform border_radius: (THEME_CORNER_RADIUS)

            uniform color_dither: 1.0

            uniform color_1: (THEME_COLOR_OUTSET_1)
            uniform color_1_active: (THEME_COLOR_OUTSET_1_ACTIVE)
            uniform color_1_disabled: (THEME_COLOR_OUTSET_1_DISABLED)

            uniform color_2: (THEME_COLOR_OUTSET_2)
            uniform color_2_active: (THEME_COLOR_OUTSET_2_ACTIVE)
            uniform color_2_disabled: (THEME_COLOR_OUTSET_2_DISABLED)

            uniform border_color_1: (THEME_COLOR_BEVEL_OUTSET_1)
            uniform border_color_1_hover: (THEME_COLOR_BEVEL_OUTSET_1_HOVER)
            uniform border_color_1_down: (THEME_COLOR_BEVEL_OUTSET_1_DOWN)
            uniform border_color_1_active: (THEME_COLOR_BEVEL_OUTSET_1_ACTIVE)
            uniform border_color_1_disabled: (THEME_COLOR_BEVEL_OUTSET_1_DISABLED)
            uniform border_color_1_active_focus: (THEME_COLOR_BEVEL_OUTSET_1_FOCUS)

            uniform border_color_2: (THEME_COLOR_BEVEL_OUTSET_2)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_OUTSET_2_HOVER)
            uniform border_color_2_down: (THEME_COLOR_BEVEL_OUTSET_2_DOWN)
            uniform border_color_2_active: (THEME_COLOR_BEVEL_OUTSET_2_ACTIVE)
            uniform border_color_2_active_focus: (THEME_COLOR_BEVEL_OUTSET_2_ACTIVE)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_OUTSET_2_DISABLED)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.x + dither),
                            mix(self.color_1_active, self.color_2_active, self.pos.x + dither),
                            self.active
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, self.pos.x + dither),
                        self.disabled
                    )
                )

                sdf.stroke(
                    mix(
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
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                mix(self.border_color_1_down, self.border_color_2_down, self.pos.y + dither),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, self.pos.y + dither),
                        self.disabled
                    ), self.border_size
                )
                return sdf.result
            }
        }
    }

    pub RadioButtonTabGradientY = <RadioButtonTabGradientX> {
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(self.color_1, self.color_2, self.pos.y + dither),
                            mix(self.color_1_active, self.color_2_active, self.pos.y + dither),
                            self.active
                        ),
                        mix(self.color_1_disabled, self.color_2_disabled, self.pos.y + dither),
                        self.disabled
                    )
                )

                sdf.stroke(
                    mix(
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
                            mix(
                                mix(self.border_color_1_hover, self.border_color_2_hover, self.pos.y + dither),
                                mix(self.border_color_1_down, self.border_color_2_down, self.pos.y + dither),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.border_color_1_disabled, self.border_color_2_disabled, self.pos.y + dither),
                        self.disabled
                    ), self.border_size
                )
                return sdf.result
            }
        }
    }
    
    pub ButtonGroup = <CachedRoundedView> {
        flow: Right
        height: Fit, width: Fit,
        spacing: (THEME_SPACE_2)
        align: { x: 0.0, y: 0.5 }
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
    #[live] draw_text: DrawText,

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
                    self.animator_play(cx, id!(hover.down));
                }
                self.set_key_focus(cx);
            },
            Hit::FingerUp(_fe) => {
                self.animator_play(cx, id!(hover.on));
                if self.animator_in_state(cx, id!(active.off)) {
                    self.animator_play(cx, id!(active.on));
                    cx.widget_action(uid, &scope.path, RadioButtonAction::Clicked);
                } else {
                    self.animator_play(cx, id!(active.off));
                }
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