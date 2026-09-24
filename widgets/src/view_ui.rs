use crate::makepad_platform::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.widgets.View =  mod.widgets.ViewBase {}

    mod.widgets.Hr = mod.widgets.View {
        width: Fill
        height: theme.space_2 * 7.5
        flow: Down
        margin: 0.

        show_bg: true
        draw_bg +: {
            color: instance(theme.color_bevel_outset_2)
            color_2: instance(theme.color_bevel_outset_1)
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
            color: instance(theme.color_bevel_outset_2)
            color_2: instance(theme.color_bevel_outset_1)
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
            color: instance(#0000)

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
            color: instance(#0000)
            color_dither: uniform(1.0)
            border_size: uniform(0.0)
            border_inset: uniform(vec4(0))
            gradient_fill_horizontal: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)

            color_2: instance(vec4(-1))

            border_color: instance(#0000)
            border_color_2: instance(vec4(-1))

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
                }

                sdf.rect(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                )

                sdf.fill_keep(fill_color)

                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
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
            color: instance(#0000)
            color_dither: uniform(1.0)
            border_size: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: instance(vec4(-1))
            border_color: instance(#f00)
            border_color_2: instance(vec4(-1))

            shadow_color: instance(#0007)
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

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
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

                sdf.fill_keep(fill_color)
                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
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
            color: instance(#8)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            color_2: instance(vec4(-1))

            border_radius: uniform(2.5)
            border_size: uniform(0.0)
            border_color: instance(#0000)
            border_color_2: instance(vec4(-1))

            shadow_color: instance(#0007)
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

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
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

                sdf.fill_keep(fill_color)

                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
                }
                return sdf.result
            }
        }
    }

    // A RoundedShadowView lifted by one elevation step of the theme: the
    // shadow's blur, drop and colour come from the elevation_N tokens, so
    // every card, menu and dialog built on these agrees on how high it sits.
    mod.widgets.ElevatedView1 = mod.widgets.RoundedShadowView{
        draw_bg +: {
            shadow_radius: uniform(theme.elevation_1_radius)
            shadow_offset: uniform(vec2(0., theme.elevation_1_offset_y))
            shadow_color: instance(theme.color_elevation_1)
        }
    }

    mod.widgets.ElevatedView2 = mod.widgets.RoundedShadowView{
        draw_bg +: {
            shadow_radius: uniform(theme.elevation_2_radius)
            shadow_offset: uniform(vec2(0., theme.elevation_2_offset_y))
            shadow_color: instance(theme.color_elevation_2)
        }
    }

    mod.widgets.ElevatedView3 = mod.widgets.RoundedShadowView{
        draw_bg +: {
            shadow_radius: uniform(theme.elevation_3_radius)
            shadow_offset: uniform(vec2(0., theme.elevation_3_offset_y))
            shadow_color: instance(theme.color_elevation_3)
        }
    }

    mod.widgets.ElevatedView4 = mod.widgets.RoundedShadowView{
        draw_bg +: {
            shadow_radius: uniform(theme.elevation_4_radius)
            shadow_offset: uniform(vec2(0., theme.elevation_4_offset_y))
            shadow_color: instance(theme.color_elevation_4)
        }
    }

    mod.widgets.ElevatedView5 = mod.widgets.RoundedShadowView{
        draw_bg +: {
            shadow_radius: uniform(theme.elevation_5_radius)
            shadow_offset: uniform(vec2(0., theme.elevation_5_offset_y))
            shadow_color: instance(theme.color_elevation_5)
        }
    }

    mod.widgets.RoundedView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: instance(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            border_size: uniform(0.0)
            border_radius: uniform(2.5)
            color_2: instance(vec4(-1))
            border_color: instance(#0000)
            border_color_2: instance(vec4(-1))
            border_inset: uniform(vec4(0))

            // THE MATERIAL, from the theme. Packed exactly as `ReliefView`
            // packs its own, so the two read the same way and a stylesheet's
            // tokens mean the same thing on both. Zero in every stock theme:
            // at `material` 0 this shader draws what it always drew.
            /** surface material tier: 0 flat, 1 relief, 2 relief with rim, gloss and specular 0..2 step 1 */
            material: uniform(theme.material_level)
            /** key light: direction (x right, y down, z out) and intensity */
            material_light: uniform(vec4(theme.material_light_x, theme.material_light_y, theme.material_light_z, theme.material_light_intensity))
            /** bevel width, profile curve, raise, specular */
            material_relief: uniform(vec4(theme.material_bevel_width, theme.material_bevel_curve, theme.material_raise, theme.material_specular))
            /** occlusion, rim, gloss, roughness */
            material_finish: uniform(vec4(theme.material_ao, theme.material_rim, theme.material_gloss, theme.material_roughness))
            /** face gradient, hairline, occlusion reach, sink */
            material_tune: uniform(vec4(theme.material_face_gradient, theme.material_hairline, theme.material_ao_reach, theme.material_sink))
            /** cast shadow strength, blur, falloff (0 linear 1 expo), contact occlusion */
            material_shadow: uniform(vec4(theme.material_shadow, theme.material_shadow_blur, theme.material_shadow_falloff, theme.material_contact_ao))
            /** inner shadow, inner blur, ground lip, glow */
            material_inner: uniform(vec4(theme.material_inner_shadow, theme.material_inner_radius, theme.material_ground_lip, theme.material_glow))
            /** the ink a lit shoulder is tinted toward */
            material_light_ink: uniform(theme.color_material_light)
            /** the ink a shaded shoulder and the occlusion are tinted toward */
            material_shadow_ink: uniform(theme.color_material_shadow)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
                }

                sdf.box(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    max(1.0 self.border_radius)
                )
                // THE MATERIAL. Behind a uniform, so a stock theme pays one
                // compare per draw call and draws exactly what it always did.
                // A raised rounded view is `ReliefView`'s raised face read
                // from the theme: the cast shadow, contact and lip go UNDER
                // the face (the `sdf.clear` idiom of `RoundedShadowView`),
                // and the face itself is lit by `Material.face`. The shadow
                // only has somewhere to land where the shape is inset from
                // the quad -- makepad clips by default, so a material control
                // buys its overhang out of `border_inset` rather than by
                // growing its geometry -- and it fades out before the quad
                // edge so it never ends on a straight line.
                if self.material > 0.5 {
                    let p = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let lower = vec2(self.border_inset.x + self.border_size, self.border_inset.y + self.border_size)
                    let upper = vec2(self.rect_size.x - (self.border_inset.z + self.border_size), self.rect_size.y - (self.border_inset.w + self.border_size))
                    let c = (lower + upper) * 0.5
                    let h = max((upper - lower) * 0.5, vec2(0.5, 0.5))
                    // `sdf.box` draws a corner of TWICE its argument, clamped.
                    let r = min(2.0 * max(1.0, self.border_radius), min(h.x, h.y))
                    let d = sdf.shape
                    let e = 0.5
                    var g = vec2(
                        Material.sd_box(p + vec2(e, 0.0), c, h, r) - Material.sd_box(p - vec2(e, 0.0), c, h, r),
                        Material.sd_box(p + vec2(0.0, e), c, h, r) - Material.sd_box(p - vec2(0.0, e), c, h, r)
                    )
                    if length(g) > 0.00001 {
                        g = normalize(g)
                    } else {
                        g = vec2(0.0, 1.0)
                    }
                    let raise = self.material_relief.z
                    let off = Material.cast_offset(raise, self.material_light)
                    let margin = min(min(self.border_inset.x, self.border_inset.y), min(self.border_inset.z, self.border_inset.w)) + self.border_size
                    // A shadow wider than the margin it falls into would only
                    // be cut off: its blur stays within reach of the quad's edge.
                    let sh = vec4(self.material_shadow.x, min(self.material_shadow.y, max(margin, 1.0) * 1.2), self.material_shadow.z, self.material_shadow.w)
                    var under = Material.cast(
                        d,
                        Material.sd_box(p - off, c, h, r),
                        Material.sd_box(p + off, c, h, r),
                        g, px, raise, raise,
                        self.material_light, sh, self.material_inner.z,
                        self.material_shadow_ink.rgb, self.material_light_ink.rgb
                    )
                    let qc = self.rect_size * 0.5
                    let edge = -Material.sd_box(p, qc, qc, min(r + margin, min(qc.x, qc.y)))
                    under = under * smoothstep(0.0, max(margin, 1.0), edge)
                    // `clear` premultiplies what it is given.
                    sdf.clear(vec4(under.rgb / max(under.a, 0.0001), under.a))
                    // Tier 1 is the relief alone: no rim, gloss or specular.
                    let t2 = step(1.5, self.material)
                    let fin = vec4(self.material_finish.x, self.material_finish.y * t2, self.material_finish.z * t2, self.material_finish.w)
                    let rel = vec4(self.material_relief.x, self.material_relief.y, raise, self.material_relief.w * t2)
                    let uv = (p - c) / (2.0 * h) + vec2(0.5, 0.5)
                    fill_color = vec4(Material.face(
                        fill_color.rgb, d, g, uv, raise, raise, 0.0, 0.0, 0.0,
                        self.material_light, rel, fin, self.material_tune, self.material_inner.x,
                        self.material_light_ink.rgb, self.material_shadow_ink.rgb, 1.0
                    ), fill_color.a)
                }
                sdf.fill_keep(fill_color)
                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
                }
                return sdf.result
            }
        }
    }

    // Application surfaces share a material contract. Classic styles replace
    // the rounded edge with a two-line raised/sunken frame in widgets.splash.
    mod.widgets.PanelView = mod.widgets.RoundedView {
        draw_bg +: {
            color: theme.color_bg_container
            border_radius: min(theme.container_corner_radius 12.0)
            bevel: uniform(0.0)
            sunken: uniform(0.0)
            bevel_light: uniform(#ffffff)
            bevel_dark: uniform(#808080)
            bevel_shadow: uniform(#000000)
            pixel: fn() {
                let p = self.pos * self.rect_size
                if self.bevel > 0.5 {
                    let tl = min(p.x, p.y)
                    let br = min(self.rect_size.x - p.x, self.rect_size.y - p.y)
                    let d = min(tl, br)
                    let upper = if self.sunken > 0.5 {tl > br} else {tl < br}
                    let light = if d < 1.0 {self.bevel_light} else {self.color}
                    let dark = if d < 1.0 {self.bevel_shadow} else {self.bevel_dark}
                    return if d < 2.0 {if upper {light} else {dark}} else {self.color}
                }
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
                // `sunken` flips the sign of the relief, and that is the
                // whole difference between a panel standing off its ground
                // and a well cut into it: the same material, lit from the
                // other side, so the lit and shaded shoulders swap. A panel
                // genuinely is inverted rather than merely lowered, so here
                // depth and convexity move together -- unlike a pressed cap,
                // which descends with its face still convex.
                //
                // A SURFACE IS SHALLOWER THAN A CONTROL: a panel is a step in
                // the housing and a cap stands proud OF it, so the panel takes
                // a fraction of the theme's elevation. Scaling it up instead
                // makes a whole surface deeper than the buttons sitting on it.
                //
                // The panel's face fills its quad, so it has no ground of its
                // own to throw a shadow on: only the face is lit here. A
                // sunken one takes the surround's inner shadow, the real
                // blurred coverage of this rect shifted down-light, the way
                // `ReliefView` computes it -- a distance falloff would crease
                // along the corner diagonals.
                var fill = self.color
                if self.material > 0.5 {
                    let elev = mix(self.material_relief.z, -self.material_tune.w, self.sunken) * 0.55
                    let c = self.rect_size * 0.5
                    let h = max(c, vec2(0.5, 0.5))
                    let r = min(2.0 * self.border_radius, min(h.x, h.y))
                    let d = sdf.shape
                    let e = 0.5
                    var g = vec2(
                        Material.sd_box(p + vec2(e, 0.0), c, h, r) - Material.sd_box(p - vec2(e, 0.0), c, h, r),
                        Material.sd_box(p + vec2(0.0, e), c, h, r) - Material.sd_box(p - vec2(0.0, e), c, h, r)
                    )
                    if length(g) > 0.00001 {
                        g = normalize(g)
                    } else {
                        g = vec2(0.0, 1.0)
                    }
                    var insh = 0.0
                    if self.material_inner.x > 0.001 && elev < 0.0 {
                        let ioff = Material.shadow_dir(self.material_light) * abs(elev) * 1.6
                        insh = 1.0 - Material.box_cov(ioff, self.rect_size + ioff, p, max(self.material_inner.y * 0.5, 0.35), r)
                    }
                    let t2 = step(1.5, self.material)
                    let fin = vec4(self.material_finish.x, self.material_finish.y * t2, self.material_finish.z * t2, self.material_finish.w)
                    let rel = vec4(self.material_relief.x, self.material_relief.y, self.material_relief.z, self.material_relief.w * t2)
                    fill = vec4(Material.face(
                        self.color.rgb, d, g, self.pos, elev, elev, 0.0, insh, 0.0,
                        self.material_light, rel, fin, self.material_tune, self.material_inner.x,
                        self.material_light_ink.rgb, self.material_shadow_ink.rgb, 1.0
                    ), self.color.a)
                }
                sdf.fill(fill)
                return sdf.result
            }
        }
    }
    mod.widgets.InsetPanelView = mod.widgets.PanelView {draw_bg +: {sunken: 1.0}}

    mod.widgets.RoundedXView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: instance(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: instance(vec4(-1))

            border_size: uniform(0.0)
            border_color: instance(#0000)
            border_color_2: instance(vec4(-1))
            border_inset: uniform(vec4(0))
            border_radius: uniform(vec2(2.5 2.5))

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
                }

                sdf.box_x(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    self.border_radius.x
                    self.border_radius.y
                )
                sdf.fill_keep(fill_color)
                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
                }
                return sdf.result
            }
        }
    }

    mod.widgets.RoundedYView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: instance(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: instance(vec4(-1))

            border_size: uniform(0.0)
            border_color: instance(#0000)
            border_color_2: instance(vec4(-1))
            border_inset: uniform(vec4(0))
            border_radius: uniform(vec2(2.5 2.5))

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
                }

                sdf.box_y(
                    self.border_inset.x + self.border_size
                    self.border_inset.y + self.border_size
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0)
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                    self.border_radius.x
                    self.border_radius.y
                )

                sdf.fill_keep(fill_color)

                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
                }

                return sdf.result
            }
        }
    }

    mod.widgets.RoundedAllView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: instance(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: instance(vec4(-1))
            border_size: uniform(0.0)
            border_color: instance(#0000)
            border_color_2: instance(vec4(-1))
            border_inset: uniform(vec4(0))
            border_radius: uniform(vec4(2.5))

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
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

                sdf.fill_keep(fill_color)

                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
                }

                return sdf.result
            }
        }
    }

    mod.widgets.CircleView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: instance(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            color_2: instance(vec4(-1))
            border_size: uniform(0.0)
            border_color: instance(#0000)
            border_color_2: instance(vec4(-1))
            border_inset: uniform(vec4(0.0))
            border_radius: uniform(0.0)

            radius: varying(float)

            vertex: fn() {
                let inset_size = vec2(self.border_inset.x + self.border_inset.z, self.border_inset.y + self.border_inset.w);
                let inner_size = self.rect_size - inset_size - vec2(2.0 * self.border_size);

                self.radius = self.border_radius;
                if self.radius <= 0.0 {
                    self.radius = min(inner_size.x, inner_size.y) * 0.5;
                }

                return self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
                }

                // The center of the bounds is used for drawing the circle.
                sdf.circle(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.radius
                )

                sdf.fill_keep(fill_color)

                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
                }

                return sdf.result
            }
        }
    }

    mod.widgets.HexagonView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: instance(#0000)
            color_dither: uniform(1.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_2: instance(vec4(-1))
            border_color_2: instance(vec4(-1))

            border_size: uniform(0.0)
            border_color: instance(#0000)
            border_inset: uniform(vec4(0.0))
            border_radius: uniform(vec2(0.0 1.0))

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }

                let mut stroke_color = self.border_color
                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_border_horizontal > 0.5 self.pos.x else self.pos.y
                    stroke_color = mix(self.border_color self.border_color_2 dir + dither)
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
                            (self.rect_size.x - (self.border_inset.x + self.border_inset.z + 2.0 * self.border_size)) * 0.5,
                            (self.rect_size.y - (self.border_inset.y + self.border_inset.w + 2.0 * self.border_size)) * 0.5
                        )
                    )
                }

                sdf.fill_keep(fill_color)

                if self.border_size > 0.0 {
                    sdf.stroke(stroke_color self.border_size)
                }

                return sdf.result
            }
        }
    }

    mod.widgets.GradientXView = mod.widgets.ViewBase {
        show_bg: true
        draw_bg +: {
            color: instance(#00f)
            gradient_fill_horizontal: uniform(1.0)
            color_dither: uniform(1.0)
            color_2: instance(vec4(-1))

            get_color: fn() {
                let mut fill_color = self.color
                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let dir = if self.gradient_fill_horizontal > 0.5 self.pos.x else self.pos.y
                    fill_color = mix(self.color self.color_2 dir + dither)
                }
                return fill_color
            }

            pixel: fn() {
                return Pal.premul(self.get_color())
            }
        }
    }

    mod.widgets.GradientYView = mod.widgets.GradientXView {
        show_bg: true
        draw_bg +: {
            color: instance(#00f)
            gradient_fill_horizontal: uniform(0.0)
            color_2: instance(vec4(-1))
            color_dither: uniform(1.0)
        }
    }

    mod.widgets.CachedView = mod.widgets.ViewBase {
        texture_caching: true
        draw_bg +: {
            image: texture_2d(float)
            scale: varying(vec2(0))
            shift: varying(vec2(0))
            vertex: fn() {
                let dpi = self.draw_pass.dpi_factor
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size
                self.shift = (self.rect_pos - floor_pos) / ceil_size
                return self.clip_and_transform_vertex(self.rect_pos self.rect_size)
            }
            pixel: fn() {
                return self.image.sample(self.pos * self.scale + self.shift)
            }
        }
    }

    mod.widgets.CachedRoundedView = mod.widgets.ViewBase {
        texture_caching: true
        draw_bg +: {
            border_size: uniform(0.0)
            border_color: instance(#000F)
            border_inset: uniform(vec4(0))
            border_radius: uniform(2.5)

            image: texture_2d(float)
            scale: varying(vec2(0))
            shift: varying(vec2(0))

            get_border_color: fn() {
                return self.border_color
            }

            vertex: fn() {
                let dpi = self.draw_pass.dpi_factor
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
                let color = self.image.sample(self.pos * self.scale + self.shift)
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
            scroll_bar_x.drag_scrolling: true
            scroll_bar_y.drag_scrolling: true
        }
    }

    mod.widgets.CachedScrollX = mod.widgets.CachedView {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: true show_scroll_y: false
            scroll_bar_x.drag_scrolling: true
        }
    }

    mod.widgets.CachedScrollY = mod.widgets.CachedView {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: false show_scroll_y: true
            scroll_bar_y.drag_scrolling: true
        }
    }

    mod.widgets.ScrollXYView = mod.widgets.ViewBase {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: true show_scroll_y: true
            scroll_bar_x.drag_scrolling: true
            scroll_bar_y.drag_scrolling: true
        }
    }

    mod.widgets.ScrollXView = mod.widgets.ViewBase {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: true show_scroll_y: false
            scroll_bar_x.drag_scrolling: true
        }
    }

    mod.widgets.ScrollYView = mod.widgets.ViewBase {
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: false show_scroll_y: true
            scroll_bar_y.drag_scrolling: true
        }
    }
}
