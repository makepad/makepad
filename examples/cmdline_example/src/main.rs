use makepad_shader_macro::*;
use makepad_shader_compiler::lex;

fn main() {
    
    let shader = "
        
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
    ";
    eprintln!("HERE");
    let source = &shader;
    let tokens = lex::lex(source.chars()).collect::<Result<Vec<_>, _>>();
    if let Err(err) = tokens {
        return eprintln!("Shader lex error: {}", err);
    }
    let tokens = tokens.unwrap();
    
}