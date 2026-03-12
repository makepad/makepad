use crate::makepad_draw::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.View

    mod.widgets.GlassPanel = View{
        show_bg: true
        draw_bg +: {
            tint_color: instance(#fff)
            tint_alpha: instance(0.2)

            border_color: instance(#fff)
            border_alpha: instance(0.35)
            border_width: instance(1.0)
            corner_radius: instance(12.0)

            specular_strength: instance(0.35)
            noise_strength: instance(0.035)
            blur_amount: instance(0.35)
            use_scene_blur: instance(0.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let inset = self.border_width * 0.5
                sdf.box(
                    inset
                    inset
                    self.rect_size.x - inset * 2.0
                    self.rect_size.y - inset * 2.0
                    self.corner_radius
                )

                let edge_uv = abs(self.pos * 2.0 - 1.0)
                let edge_gradient = clamp((edge_uv.x + edge_uv.y) * 0.5, 0.0, 1.0)
                let highlight = self.specular_strength * (0.65 * edge_gradient + 0.35 * (1.0 - self.pos.y))

                let noise = (
                    Math.random_2d(
                        self.pos * self.rect_size
                        + vec2(self.draw_pass.time * 37.0, self.draw_pass.time * 13.0)
                    ) - 0.5
                ) * self.noise_strength

                let blur_mix = clamp(self.use_scene_blur * self.blur_amount, 0.0, 1.0)
                let blur_fallback_color = vec3(0.86, 0.9, 0.96)
                let glass_color = self.tint_color.rgb.mix(blur_fallback_color, blur_mix * 0.45)
                let fill = vec4(glass_color + noise + highlight, self.tint_alpha)

                sdf.fill_keep(fill)
                if self.border_width > 0.0 {
                    sdf.stroke(
                        vec4(self.border_color.rgb, self.border_alpha)
                        self.border_width
                    )
                }
                return sdf.result
            }
        }
    }
}
