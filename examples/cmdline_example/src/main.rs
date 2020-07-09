use makepad_shader_macro::*;

fn main() {
    
    pub fn def_trapezoid_shader() -> ShaderGen { 
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sg.geometry_indices = vec![0, 1, 2, 2, 3, 0];
         
        sg.compose(shader!{"
            
            let geom: vec2<Geometry>;
            
            instance a_xs: Self::a_xs();
            instance a_ys: Self::a_ys();
            instance chan: Self::chan();
            
            varying v_p0: vec2;
            varying v_p1: vec2;
            varying v_p2: vec2;
            varying v_p3: vec2;
            varying v_pixel: vec2;
            
            fn intersect_line_segment_with_vertical_line(p0: vec2, p1: vec2, x: float) -> vec2 {
                return vec2(
                    x,
                    mix(p0.y, p1.y, (x - p0.x) / (p1.x - p0.x))
                );
            }
            
            fn intersect_line_segment_with_horizontal_line(p0: vec2, p1: vec2, y: float) -> vec2 {
                return vec2(
                    mix(p0.x, p1.x, (y - p0.y) / (p1.y - p0.y)),
                    y
                );
            }
            
            fn compute_clamped_right_trapezoid_area(p0: vec2, p1: vec2, p_min: vec2, p_max: vec2) -> float {
                let x0 = clamp(p0.x, p_min.x, p_max.x);
                let x1 = clamp(p1.x, p_min.x, p_max.x);
                if (p0.x < p_min.x && p_min.x < p1.x) {
                    p0 = intersect_line_segment_with_vertical_line(p0, p1, p_min.x);
                }
                if (p0.x < p_max.x && p_max.x < p1.x) {
                    p1 = intersect_line_segment_with_vertical_line(p0, p1, p_max.x);
                }
                if (p0.y < p_min.y && p_min.y < p1.y) {
                    p0 = intersect_line_segment_with_horizontal_line(p0, p1, p_min.y);
                }
                if (p1.y < p_min.y && p_min.y < p0.y) {
                    p1 = intersect_line_segment_with_horizontal_line(p1, p0, p_min.y);
                }
                if (p0.y < p_max.y && p_max.y < p1.y) {
                    p1 = intersect_line_segment_with_horizontal_line(p0, p1, p_max.y);
                }
                if (p1.y < p_max.y && p_max.y < p0.y) {
                    p0 = intersect_line_segment_with_horizontal_line(p1, p0, p_max.y);
                }
                p0 = clamp(p0, p_min, p_max);
                p1 = clamp(p1, p_min, p_max);
                let h0 = p_max.y - p0.y;
                let h1 = p_max.y - p1.y;
                let a0 = (p0.x - x0) * h0;
                let a1 = (p1.x - p0.x) * (h0 + h1) * 0.5;
                let a2 = (x1 - p1.x) * h1;
                return a0 + a1 + a2;
            }
            
            fn compute_clamped_trapezoid_area(p_min: vec2, p_max: vec2) -> float {
                let a0 = compute_clamped_right_trapezoid_area(v_p0, v_p1, p_min, p_max);
                let a1 = compute_clamped_right_trapezoid_area(v_p2, v_p3, p_min, p_max);
                return a0 - a1;
            }
            
            fn pixel() -> vec4 {
                //if fmod(v_pixel.x,2.0) > 1.0 && fmod(v_pixel.y,2.0) > 1.0{
                //    return color("white")
               // }
               // return color("black");
                let p_min = v_pixel.xy - 0.5;
                let p_max = v_pixel.xy + 0.5;
                let t_area = compute_clamped_trapezoid_area(p_min, p_max);
                if chan < 0.5{
                    return vec4(t_area,0.,0.,0.);
                }
                if chan < 1.5{
                    return vec4(0., t_area, 0.,0.);
                }
                if chan < 2.5{
                    return vec4(0., 0., t_area,0.);
                }
                return vec4(t_area, t_area, t_area,0.);
                
                //let b_minx = p_min.x + 1.0 / 3.0;
                //let r_minx = p_min.x - 1.0 / 3.0;
                //let b_maxx = p_max.x + 1.0 / 3.0;
                //let r_maxx = p_max.x - 1.0 / 3.0;
                //return vec4(
                //    compute_clamped_trapezoid_area(vec2(r_minx, p_min.y), vec2(r_maxx, p_max.y)),
                //    compute_clamped_trapezoid_area(p_min, p_max),
                //    compute_clamped_trapezoid_area(vec2(b_minx, p_min.y), vec2(b_maxx, p_max.y)),
                //    0.0
                //);
            }
            
            fn vertex() -> vec4 {
                let pos_min = vec2(a_xs.x, min(a_ys.x, a_ys.y));
                let pos_max = vec2(a_xs.y, max(a_ys.z, a_ys.w));
                let pos = mix(pos_min - 1.0, pos_max + 1.0, geom);
                
                // set the varyings
                v_p0 = vec2(a_xs.x, a_ys.x);
                v_p1 = vec2(a_xs.y, a_ys.y);
                v_p2 = vec2(a_xs.x, a_ys.z);
                v_p3 = vec2(a_xs.y, a_ys.w);
                v_pixel = pos;
                //return vec4(vec2(pos.x, 1.0-pos.y) * 2.0 / vec2(4096.,4096.) - 1.0, 0.0, 1.0);
                return camera_projection * vec4(pos, 0.0, 1.0);
            }
        "})
    }
    
}