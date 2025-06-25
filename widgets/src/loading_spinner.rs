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
            
            instance rotation_speed: 2.0
            instance thickness: 3.0
            instance radius: 30.0
            instance arc_cycle_speed: 2.5
            instance max_arc_ratio: 0.85
            instance min_arc_ratio: 0.02
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                
                let center = self.rect_size * 0.5;
                
                sdf.circle(center.x, center.y, self.radius);
                let stroke_color = vec4(1.0, 1.0, 1.0, 1.0);
                sdf.stroke(stroke_color, self.thickness);
                
                let to_center = self.pos * self.rect_size - center;
                let pixel_angle = atan(to_center.y, to_center.x);
                
                let rotation = self.time * self.rotation_speed * 2.0 * PI;
                
                let arc_cycle = sin(self.time * self.arc_cycle_speed) * 0.5 + 0.5;
                let current_arc_ratio = mix(self.min_arc_ratio, self.max_arc_ratio, arc_cycle);
                let current_arc_length = current_arc_ratio * 2.0 * PI;
                
                let arc_start = rotation;
                let arc_end = arc_start + current_arc_length;
                
                let normalized_pixel_angle = mod(arc_start - pixel_angle + PI, 2.0 * PI);
                let normalized_arc_length = mod(current_arc_length, 2.0 * PI);
                
                let in_arc = step(0.0, normalized_pixel_angle) * (1.0 - step(normalized_arc_length, normalized_pixel_angle));
                
                let fade_width = min(0.4, normalized_arc_length * 0.15);
                let head_fade = smoothstep(0.0, fade_width, normalized_pixel_angle);
                let tail_fade = smoothstep(normalized_arc_length - fade_width, normalized_arc_length, normalized_pixel_angle);
                
                let fade_factor = head_fade * (1.0 - tail_fade);
                fade_factor = smoothstep(0.0, 1.0, fade_factor);
                
                let sdf_alpha = sdf.fill(vec4(1.0, 1.0, 1.0, 1.0)).w;
                
                let final_alpha = sdf_alpha * in_arc * fade_factor;
                
                return vec4(self.color.rgb, final_alpha);
            }
        }
    }
}
