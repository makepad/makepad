use crate::makepad_draw::*;

live_design!{
    link widgets;
    use makepad_draw::shader::std::*;
    use link::theme::*;
    use link::widgets::View;

    pub LoadingSpinner = <View> {
        width: Fill,
        height: Fill
        show_bg: true,
        draw_bg: {
            color: (THEME_COLOR_MAKEPAD)
            
            uniform rotation_speed: 1.2
            uniform border_size: 20.0
            uniform radius: -1.
            uniform max_gap_ratio: 0.92
            uniform min_gap_ratio: 0.12
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                let sz_factor = min(self.rect_size.x * 0.5, self.rect_size.y * 0.5);

                let border_sz = self.border_size * sz_factor * 0.004;
                let sz = self.radius;
                if (sz < -0.5) {
                    sz = sz_factor - self.border_size * 0.5;
                }

                let center = self.rect_size * 0.5;
                
                let rotation = self.time * self.rotation_speed * 2.0 * PI;
                
                let rotation_cycles = rotation / (2.0 * PI);
                let arc_phase = mod(rotation_cycles * 0.5, 1.0);
                
                let expand_phase = clamp(arc_phase / 0.55, 0.0, 1.0);
                let contract_phase = clamp((arc_phase - 0.55) / 0.45, 0.0, 1.0);
                
                let cycle = expand_phase * (1.0 - contract_phase);
                
                let gap_ratio = mix(self.min_gap_ratio, self.max_gap_ratio, cycle);
                let gap_radians = gap_ratio * 2.0 * PI;
                
                let start_angle = rotation;
                
                sdf.arc_round_caps(
                    center.x, 
                    center.y, 
                    sz,
                    start_angle, 
                    start_angle + 2.0 * PI - gap_radians, 
                    border_sz
                );
                
                return sdf.fill(self.color);
            }
        }
    }
}