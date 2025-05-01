use crate::makepad_draw::*;

live_design!{
    link widgets;
    use makepad_draw::shader::std::*;
    use link::widgets::View;

    pub LoadingSpinner = <View> {
        width: Fill, height: Fill
        show_bg: true,
        draw_bg: {
            color: #FFFFFF00
            instance rotation_speed: 1.0
            instance inner_radius: 0.6 // r
            instance outer_radius: 0.7 // R

            fn pixel(self) -> vec4 {
                let aspect = self.rect_size.x / self.rect_size.y;
                let pos = (self.pos - 0.5) * vec2(max(aspect, 1.), max(1. / aspect, 1.)) * 1.5;

                let radius = length(pos);
                let angle = atan(pos.y, pos.x);

                let ring_thickness = self.outer_radius - self.inner_radius; // R - r
                let cap_radius = ring_thickness / 2.0;

                let ring_center = (self.inner_radius + self.outer_radius) / 2.0;
                let edge = abs(radius - ring_center) - ring_thickness / 2.0;
                let d = smoothstep(0.005, -0.005, edge);

                // Define maximum arc length and oscillation frequency
                let max_arc = 1.93 * PI;
                let frequency = 0.75 * self.rotation_speed * 2.0 * PI;

                // Calculate tail angle (always rotate clockwise)
                let tail_angle = mod(self.time * self.rotation_speed * 2.0 * PI, 2.0 * PI);

                // Calculate the direction and magnitude of the arc length
                let theta = self.time * frequency;

                // Direction factor: >0 increases, <0 decreases
                let direction = sign(sin(theta));
                let arc_length = (1.0 - cos(theta)) * max_arc / 2.0;
                let signed_arc_length = direction * arc_length;

                // Defining the start and end points of an arc
                let start_angle = tail_angle + min(signed_arc_length, 0.0);
                let end_angle = tail_angle + max(signed_arc_length, 0.0);

                // Calculate the angle of the pixel relative to the starting point
                let relative_angle = mod(angle - start_angle, 2.0 * PI);
                let arc_length_abs = abs(signed_arc_length);
                let arc_alpha = float(relative_angle < arc_length_abs);

                // Calculate the angle of the pixel relative to the starting point
                let cap_center_tail = vec2(cos(start_angle), sin(start_angle)) * ring_center;
                let cap_center_head = vec2(cos(end_angle), sin(end_angle)) * ring_center;

                // Calculate the distance from the pixel to the center of the semicircle
                let dist_to_tail_cap = length(pos - cap_center_tail);
                let dist_to_head_cap = length(pos - cap_center_head);

                // Determine if the pixel is within the head or tail semicircle
                let in_tail_cap = dist_to_tail_cap < cap_radius;
                let in_head_cap = dist_to_head_cap < cap_radius;

                // Transparency of merged arcs and semicircles
                let alpha = clamp((arc_alpha + float(in_tail_cap) + float(in_head_cap)) * d, 0.0, 1.0);

                return vec4(self.color.rgb * alpha, alpha);
            }
        }
    }
}
