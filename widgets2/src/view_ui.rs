use {
    crate::makepad_platform::*
};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.sdf.*
    use mod.theme
    use mod.draw
    use mod.shader.*
    use mod.widgets.*
    use mod.turtle.*
    use mod.turtle.Flow.*
    use mod.turtle.Size.*
    use mod.widgets.ViewOptimize
    mod.widgets.View =  mod.widgets.ViewBase {}
    
    mod.widgets.Hr = mod.widgets.View {
        width: Fill
        height: theme.space_2 * 7.5
        flow: Down
        margin: 0.

        show_bg: true
        draw_bg +: {
            color: uniform(theme.color_bevel_outset_2)
            color_2: uniform(theme.color_bevel_outset_1)
            border_size: uniform(theme.beveling)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let sz = self.border_size * 2.

                sdf.rect(
                    0.
                    self.rect_size.y * 0.5 - sz * 2
                    self.rect_size.x
                    sz + 1.
                )

                sdf.fill(self.color)

                sdf.rect(
                    0
                    self.rect_size.y * 0.5 - sz
                    self.rect_size.x
                    sz 
                )

                sdf.fill(self.color_2)
                return sdf.result
            }
        }
    }
    
    mod.widgets.Vr = mod.widgets.View {
        width: theme.space_2 * 2.
        height: Fill
        flow: Right

        show_bg: true
        draw_bg +: {
            color: uniform(theme.color_bevel_outset_2)
            color_2: uniform(theme.color_bevel_outset_1)
            border_size: uniform(theme.beveling)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let sz = self.border_size * 2.

                sdf.rect(
                    self.rect_size.x * 0.5
                    0.
                    sz + 1.
                    self.rect_size.y
                )

                sdf.fill(self.color)

                sdf.rect(
                    self.rect_size.x * 0.5 + sz
                    0.
                    sz
                    self.rect_size.y
                )

                sdf.fill(self.color_2)

                return sdf.result
            }
        }
    }
    
    mod.widgets.Filler = mod.widgets.View { width: Fill height: Fill }
    
    mod.widgets.SolidView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)

            get_color: fn() {
                return self.color
            }
                    
            pixel: fn() {
                return Pal.premul(self.get_color())
            }
        }
    }

    mod.widgets.RectView = mod.widgets.ViewBase {
        show_bg: true

        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            border_size: uniform(0.0)
            border_inset: uniform(vec4(0))
            gradient_fill_horizontal: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)

            color_2: uniform(vec4(-1))

            border_color: uniform(#0000)
            border_color_2: uniform(vec4(-1))
                        
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = self.color
                let border_color_2 = self.border_color_2

                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                sdf.rect(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                )

                sdf.fill_keep(mix(self.color color_2 gradient_fill_dir))

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size
                    )
                }
                return sdf.result
            }
        }
    }
        
    mod.widgets.RectShadowView = mod.widgets.ViewBase {
        clip_x: false
        clip_y: false
                
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            border_size: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: uniform(vec4(-1))
            border_color: uniform(#f00)
            border_color_2: uniform(vec4(-1))

            shadow_color: uniform(#0007)
            shadow_offset: uniform(vec2(0))
            shadow_radius: uniform(10.0)
                    
            rect_size2: varying(vec2(0))
            rect_size3: varying(vec2(0))
            sdf_rect_pos: varying(vec2(0))
            sdf_rect_size: varying(vec2(0))
            rect_pos2: varying(vec2(0))
            rect_shift: varying(vec2(0))

            vertex: fn() {
                let min_offset = min(self.shadow_offset vec2(0))
                self.rect_size2 = self.rect_size + 2.0*vec2(self.shadow_radius)
                self.rect_size3 = self.rect_size2 + abs(self.shadow_offset)
                self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
                self.rect_shift = -min_offset
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius)
                return self.clip_and_transform_vertex(self.rect_pos2 self.rect_size3)
            }
                                                
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                sdf.rect(
                    self.sdf_rect_pos.x
                    self.sdf_rect_pos.y
                    self.sdf_rect_size.x
                    self.sdf_rect_size.y 
                )
                if sdf.shape > -1.0 {
                    let m = self.shadow_radius
                    let o = self.shadow_offset + self.rect_shift
                    let v = GaussShadow.box_shadow(vec2(m) + o self.rect_size2+o self.pos * (self.rect_size3+vec2(m)) m*0.5)
                    sdf.clear(self.shadow_color*v)
                }
                                                                
                sdf.fill_keep(mix(self.color color_2 gradient_fill_dir))
                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir) self.border_size)
                }
                return sdf.result
            }
        }
    }
                
    mod.widgets.RoundedShadowView = mod.widgets.ViewBase {
        clip_x: false
        clip_y: false
                            
        show_bg: true
        draw_bg +: {
            color: uniform(#8)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            color_2: uniform(vec4(-1))

            border_radius: uniform(2.5)
            border_size: uniform(0.0)
            border_color: uniform(#0000)
            border_color_2: uniform(vec4(-1))

            shadow_color: uniform(#0007)
            shadow_radius: uniform(20.0)
            shadow_offset: uniform(vec2(0))
                                            
            rect_size2: varying(vec2(0))
            rect_size3: varying(vec2(0))
            rect_pos2: varying(vec2(0))
            rect_shift: varying(vec2(0))
            sdf_rect_pos: varying(vec2(0))
            sdf_rect_size: varying(vec2(0))
                                              
            vertex: fn() {
                let min_offset = min(self.shadow_offset vec2(0))
                self.rect_size2 = self.rect_size + 2.0*vec2(self.shadow_radius)
                self.rect_size3 = self.rect_size2 + abs(self.shadow_offset)
                self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius)
                self.rect_shift = -min_offset
                                                            
                return self.clip_and_transform_vertex(self.rect_pos2 self.rect_size3)
            }
                                                        
            pixel: fn() {                                                
                let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                sdf.box(
                    self.sdf_rect_pos.x
                    self.sdf_rect_pos.y
                    self.sdf_rect_size.x
                    self.sdf_rect_size.y
                    max(1.0 self.border_radius)
                )
                if sdf.shape > -1.0 {
                    let m = self.shadow_radius
                    let o = self.shadow_offset + self.rect_shift
                    let v = GaussShadow.rounded_box_shadow(vec2(m) + o self.rect_size2+o self.pos * (self.rect_size3+vec2(m)) self.shadow_radius*0.5 self.border_radius*2.0)
                    sdf.clear(self.shadow_color*v)
                }
                                                                    
                sdf.fill_keep(mix(self.color color_2 gradient_fill_dir))

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size)
                }
                return sdf.result
            }
        }
    }
                
    mod.widgets.RoundedView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            border_size: uniform(0.0)
            border_radius: uniform(2.5)
            color_2: uniform(vec4(-1))
            border_color: uniform(#0000)
            border_color_2: uniform(vec4(-1))
            border_inset: uniform(vec4(0.0 0.0 0.0 0.0))
                                
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                var color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                var border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                var gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                var gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                sdf.box(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    max(1.0 self.border_radius)
                )
                sdf.fill_keep(
                    mix(self.color color_2 gradient_fill_dir)
                )
                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size
                    )
                }
                return sdf.result
            }
        }
    }
                
    mod.widgets.RoundedXView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: uniform(vec4(-1))

            border_size: uniform(0.0)
            border_color: uniform(#0000)
            border_color_2: uniform(vec4(-1))
            border_inset: uniform(vec4(0.0 0.0 0.0 0.0))
            border_radius: uniform(vec2(2.5 2.5))
                            
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                sdf.box_x(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    self.border_radius.x
                    self.border_radius.y
                )
                sdf.fill_keep(
                    mix(self.color color_2 gradient_fill_dir)
                )
                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size
                    )
                }
                return sdf.result
            }
        }
    }
                
    mod.widgets.RoundedYView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: uniform(vec4(-1))

            border_size: uniform(0.0)
            border_color: uniform(#0000)
            border_color_2: uniform(vec4(-1))
            border_inset: uniform(vec4(0.0 0.0 0.0 0.0))
            border_radius: uniform(vec2(2.5 2.5))
                                                        
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                sdf.box_y(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    self.border_radius.x
                    self.border_radius.y
                )

                sdf.fill_keep(
                    mix(self.color color_2 gradient_fill_dir)
                )

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size
                    )
                }

                return sdf.result
            }
        }
    }
                
    mod.widgets.RoundedAllView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: uniform(vec4(-1))
            border_size: uniform(0.0)
            border_color: uniform(#0000)
            border_color_2: uniform(vec4(-1))
            border_inset: uniform(vec4(0.0 0.0 0.0 0.0))
            border_radius: uniform(vec4(2.5 2.5 2.5 2.5))
                            
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }
                            
                sdf.box_all(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    self.border_radius.x
                    self.border_radius.y
                    self.border_radius.z
                    self.border_radius.w
                )

                sdf.fill_keep(
                    mix(self.color color_2 gradient_fill_dir)
                )

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size
                    )
                }

                return sdf.result
            }
        }
    }
                
    mod.widgets.CircleView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            color_2: uniform(vec4(-1))
            border_size: uniform(0.0)
            border_color: uniform(#0000)
            border_color_2: uniform(vec4(-1))
            border_inset: uniform(vec4(0.0 0.0 0.0 0.0))
            border_radius: uniform(5.0)
                            
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }
                            
                if self.border_radius > 0.0 {
                    sdf.circle(
                        self.rect_size.x * 0.5
                        self.rect_size.y * 0.5
                        self.border_radius
                    )
                }
                else {
                    sdf.circle(
                        self.rect_size.x * 0.5
                        self.rect_size.y * 0.5
                        min(
                            (self.rect_size.x - (self.border_inset.x + self.border_inset.z + 2.0 * self.border_size)) * 0.5
                            (self.rect_size.y - (self.border_inset.y + self.border_inset.w + 2.0 * self.border_size)) * 0.5
                        )
                    )
                }

                sdf.fill_keep(
                    mix(self.color color_2 gradient_fill_dir)
                )

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size
                    )
                }

                return sdf.result
            }
        }
    }
                
    mod.widgets.HexagonView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: uniform(vec4(-1))
            border_color_2: uniform(vec4(-1))

            border_size: uniform(0.0)
            border_color: uniform(#0000)
            border_inset: uniform(vec4(0.0 0.0 0.0 0.0))
            border_radius: uniform(vec2(0.0 1.0))
                            
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                
                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let border_color_2 = self.border_color
                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2
                }

                let gradient_border_dir = self.pos.y + dither
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = self.pos.x + dither
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                if self.border_radius.x > 0.0 {
                    sdf.hexagon(
                        self.rect_size.x * 0.5
                        self.rect_size.y * 0.5
                        self.border_radius.x
                    )
                }
                else {
                    sdf.hexagon(
                        self.rect_size.x * 0.5
                        self.rect_size.y * 0.5
                        min(
                            (self.rect_size.x - (self.border_inset.x + self.border_inset.z + 2.0 * self.border_size)) * 0.5
                            (self.rect_size.y - (self.border_inset.y + self.border_inset.w + 2.0 * self.border_size)) * 0.5
                        )
                    )
                }

                sdf.fill_keep(
                    mix(self.color color_2 gradient_fill_dir)
                )

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color border_color_2 gradient_border_dir)
                        self.border_size
                    )
                }
                
                return sdf.result
            }
        }
    }
                
    mod.widgets.GradientXView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: uniform(#00f)
            gradient_fill_horizontal: uniform(1.0)
            color_dither: uniform(1.0)
            color_2: uniform(vec4(-1))

            get_color: fn() {
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                let color_2 = self.color
                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2
                }

                let gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                return mix(self.color color_2 gradient_fill_dir + dither)
            }
                            
            pixel: fn() {
                return Pal.premul(self.get_color())
            }
        }
    }
                
    mod.widgets.GradientYView = mod.widgets.GradientXView {
        show_bg: true
        draw_bg +: {
            color: uniform(#00f)
            gradient_fill_horizontal: uniform(0.0)
            color_2: uniform(vec4(-1))
            color_dither: uniform(1.0)
        }
    }
                
    mod.widgets.CachedView = mod.widgets.ViewBase {
        optimize: ViewOptimize.Texture
        draw_bg +: {
            image: texture_2d(float)
            scale: varying(vec2(0))
            shift: varying(vec2(0))
            vertex: fn() {
                let dpi = self.dpi_factor
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size
                self.shift = (self.rect_pos - floor_pos) / ceil_size
                return self.clip_and_transform_vertex(self.rect_pos self.rect_size)
            }
            pixel: fn() {
                return sample2d_rt(self.image self.pos * self.scale + self.shift)
            }
        }
    }
            
    mod.widgets.CachedRoundedView = mod.widgets.ViewBase {
        optimize: ViewOptimize.Texture
        draw_bg +: {
            border_size: uniform(0.0)
            border_color: uniform(#000F)
            border_inset: uniform(vec4(0. 0. 0. 0.))
            border_radius: uniform(2.5)
                                
            image: texture_2d(float)
            scale: varying(vec2(0))
            shift: varying(vec2(0))
                                            
            get_border_color: fn() {
                return self.border_color
            }
                                        
            vertex: fn() {
                let dpi = self.dpi_factor
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size
                self.shift = (self.rect_pos - floor_pos) / ceil_size
                return self.clip_and_transform_vertex(self.rect_pos self.rect_size)
            }
                                
            pixel: fn() {
                                        
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    max(1.0 self.border_radius)
                )
                let color = sample2d_rt(self.image self.pos * self.scale + self.shift)
                sdf.fill_keep_premul(color)
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color() self.border_size)
                }
                return sdf.result
            }
        }
    }
    
    mod.widgets.CachedScrollXY = mod.widgets.CachedView {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: true show_scroll_y: true
            scroll_bar_x +: {drag_scrolling: true}
            scroll_bar_y +: {drag_scrolling: true}
        }
    }
        
    mod.widgets.CachedScrollX = mod.widgets.CachedView {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: true show_scroll_y: false
            scroll_bar_x +: {drag_scrolling: true}
        }
    }
        
    mod.widgets.CachedScrollY = mod.widgets.CachedView {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: false show_scroll_y: true
            scroll_bar_y +: {drag_scrolling: true}
        }
    }
        
    mod.widgets.ScrollXYView = mod.widgets.ViewBase {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: true show_scroll_y: true
            scroll_bar_x +: {drag_scrolling: true}
            scroll_bar_y +: {drag_scrolling: true}
        }
    }

    mod.widgets.ScrollXView = mod.widgets.ViewBase {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: true show_scroll_y: false
            scroll_bar_x +: {drag_scrolling: true}
        }
    }

    mod.widgets.ScrollYView = mod.widgets.ViewBase {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: false show_scroll_y: true
            scroll_bar_y +: {drag_scrolling: true}
        }
    }
}
