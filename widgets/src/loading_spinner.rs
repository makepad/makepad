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

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                let center = self.rect_size * 0.5;

                let to_center = self.pos * self.rect_size - center;
                let angle = atan(to_center.y, to_center.x);

                let rotation = self.time * self.rotation_speed * 2.0 * PI;
                let normalized_angle = mod(angle + rotation + PI, 2.0 * PI);

                let gradient_factor = normalized_angle / (2.0 * PI);

                sdf.circle(center.x, center.y, self.radius);
                let sdf_result = sdf.stroke(vec4(1.0, 1.0, 1.0, 1.0), self.thickness);
                let final_alpha = sdf_result.w * gradient_factor;

                return vec4(self.color.rgb, final_alpha);
            }
        }
    }
}
