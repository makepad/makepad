use crate::makepad_draw::*;

live_design!{
    link widgets;
    use makepad_draw::shader::std::*;
    use link::widgets::View;

    pub LoadingSpinner = <View> {
            width: Fill,
            height: Fill
            show_bg: true,
            draw_bg: {
                color: #00000000
                
                instance rotation_speed: 1.2
                instance stroke_width: 6.0
                instance radius: 30.0
                instance max_gap_ratio: 0.92
                instance min_gap_ratio: 0.12
                
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    
                    let center = self.rect_size * 0.5;
                    
                    let rotation = self.time * self.rotation_speed * 2.0 * PI;
                    
                    let rotation_cycles = rotation / (2.0 * PI);
                    let arc_phase = mod(rotation_cycles * 0.5, 1.0);
                    
                    let expand_phase = clamp(arc_phase / 0.6, 0.0, 1.0);
                    let contract_phase = clamp((arc_phase - 0.6) / 0.4, 0.0, 1.0);
                    
                    let cycle = expand_phase * (1.0 - contract_phase);
                    
                    let gap_ratio = mix(self.min_gap_ratio, self.max_gap_ratio, cycle);
                    let gap_radians = gap_ratio * 2.0 * PI;
                    
                    let start_angle = rotation;
                    
                    sdf.arc_round_caps(
                        center.x, 
                        center.y, 
                        self.radius, 
                        start_angle, 
                        start_angle + 2.0 * PI - gap_radians, 
                        self.stroke_width
                    );
                    
                    let spinner_color = vec3(0.29, 0.56, 0.89);
                    
                    return sdf.fill(vec4(spinner_color.x, spinner_color.y, spinner_color.z, 1.0));
                }
            }
    }
}
