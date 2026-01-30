use crate::makepad_draw::*;

script_mod!{
    use mod.prelude.widgets_internal.*
    use mod.widgets.View

    mod.widgets.LoadingSpinner = View{
        width: Fill
        height: Fill
        show_bg: true
        draw_bg +: {
            color: uniform(theme.color_makepad)
            
            rotation_speed: uniform(1.2)
            border_size: uniform(20.0)
            max_gap_ratio: uniform(0.92)
            min_gap_ratio: uniform(0.12)
            stroke_width: uniform(3.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let radius = min(self.rect_size.x * 0.5, self.rect_size.y * 0.5) - self.stroke_width * 0.5
                let center = self.rect_size * 0.5
                
                let rotation = self.draw_pass.time * self.rotation_speed * 2.0 * PI
                
                let rotation_cycles = rotation / (2.0 * PI)
                let arc_phase = modf(rotation_cycles * 0.5, 1.0)
                
                let expand_phase = clamp(arc_phase / 0.55, 0.0, 1.0)
                let contract_phase = clamp((arc_phase - 0.55) / 0.45, 0.0, 1.0)
                
                let cycle = expand_phase * (1.0 - contract_phase)
                
                let gap_ratio = mix(self.min_gap_ratio, self.max_gap_ratio, cycle)
                let gap_radians = gap_ratio * 2.0 * PI
                
                let start_angle = rotation
                
                sdf.arc_round_caps(
                    center.x 
                    center.y 
                    radius
                    start_angle 
                    start_angle + 2.0 * PI - gap_radians 
                    self.stroke_width
                )
                                
                return sdf.fill(self.color)
            }
        }
    }
}
