pub use {
    std::{
        rc::Rc,
        cell::RefCell,
        io::prelude::*,
        fs::File,
        collections::HashMap,
    },
    crate::{
        makepad_platform::*,
        cx_2d::Cx2d,
        turtle::{Walk, Layout},
        view::{ManyInstances, View, ViewRedrawingApi},
        geometry::GeometryQuad2D,
        makepad_vector::font::Glyph,
        makepad_vector::trapezoidator::Trapezoidator,
        makepad_vector::geometry::{AffineTransformation, Transform, Vector},
        makepad_vector::internal_iter::*,
        makepad_vector::path::PathIterator,
    }
};

live_design!{
    DrawTrapezoidVector= {{DrawTrapezoidVector}} {
        
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
        
        fn compute_clamped_trapezoid_area(self, p_min: vec2, p_max: vec2) -> float {
            let a0 = compute_clamped_right_trapezoid_area(self.v_p0, self.v_p1, p_min, p_max);
            let a1 = compute_clamped_right_trapezoid_area(self.v_p2, self.v_p3, p_min, p_max);
            return a0 - a1;
        }
        
        fn pixel(self) -> vec4 {
            let p_min = self.v_pixel.xy - 0.5;
            let p_max = self.v_pixel.xy + 0.5;
            let t_area = self.compute_clamped_trapezoid_area(p_min, p_max);
            if self.chan < 0.5 {
                return vec4(t_area, 0., 0., 0.);
            }
            if self.chan < 1.5 {
                return vec4(0., t_area, 0., 0.);
            }
            if self.chan < 2.5 {
                return vec4(0., 0., t_area, 0.);
            }
            return vec4(t_area, t_area, t_area, 0.);
        }
        
        fn vertex(self) -> vec4 {
            let pos_min = vec2(self.a_xs.x, min(self.a_ys.x, self.a_ys.y));
            let pos_max = vec2(self.a_xs.y, max(self.a_ys.z, self.a_ys.w));
            let pos = mix(pos_min - 1.0, pos_max + 1.0, self.geom_pos);
            
            // set the varyings
            self.v_p0 = vec2(self.a_xs.x, self.a_ys.x);
            self.v_p1 = vec2(self.a_xs.y, self.a_ys.y);
            self.v_p2 = vec2(self.a_xs.x, self.a_ys.z);
            self.v_p3 = vec2(self.a_xs.y, self.a_ys.w);
            self.v_pixel = pos;
            return self.camera_projection * vec4(pos, 0.0, 1.0);
        }
    }
}


#[derive(Live)]
#[repr(C)]
pub struct DrawTrapezoidVector {
    #[rust] pub trapezoidator: Trapezoidator,
    #[live] pub geometry: GeometryQuad2D,
    #[calc] pub draw_vars: DrawVars,
    #[calc] pub a_xs: Vec2,
    #[calc] pub a_ys: Vec4,
    #[calc] pub chan: f32,
}

impl LiveHook for DrawTrapezoidVector{
    fn before_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]){
        self.draw_vars.before_apply_init_shader(cx, apply_from, index, nodes, &self.geometry);
    }
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.after_apply_update_self(cx, apply_from, index, nodes, &self.geometry);
    }
}

