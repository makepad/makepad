live_register!{
    
    use crate::shader_std::*;
    use crate::geometrygen::GeometryQuad2D;
    
    DrawQuad: DrawShader2D {
        rust_type: {{DrawQuad}}
        geometry: GeometryQuad2D {}
        varying pos: vec2
        
        fn scroll(self) -> vec2 {
            return self.draw_scroll.xy
        }
        
        fn vertex(self) -> vec4 {
            let scr = self.scroll()
            
            let clipped: vec2 = clamp(
                self.geom_pos * self.rect_size + self.rect_pos - scr,
                self.draw_clip.xy,
                self.draw_clip.zw
            )
            self.pos = (clipped + scr - self.rect_pos) / self.rect_size
            // only pass the clipped position forward
            return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                clipped.x,
                clipped.y,
                self.draw_depth + self.draw_zbias,
                1.
            )))
        }
        
        fn pixel(self) -> vec4 {
            return #f0f
        }
    }
}

use crate::cx::*;

#[derive(LiveComponent, LiveApply)]
#[repr(C)]
pub struct DrawQuad {
    #[live] pub geometry: GeometryQuad2D,
    #[calc] pub draw_vars: DrawVars,
    #[calc] pub rect_pos: Vec2,
    #[calc] pub rect_size: Vec2,
    #[live(1.0)] pub draw_depth: f32,
}

impl DrawQuad {
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) {
        if self.draw_vars.draw_shader.is_some() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
            cx.begin_turtle(layout, self.draw_vars.area);
        }
    }
    
    pub fn end_quad(&mut self, cx: &mut Cx) {
        if self.draw_vars.draw_shader.is_some() {
            let rect = cx.end_turtle(self.draw_vars.area);
            self.draw_vars.area.set_rect(cx, &rect);
        }
    }
    
    pub fn draw_quad_walk(&mut self, cx: &mut Cx, walk: Walk) {
        let rect = cx.walk_turtle(walk);
        self.rect_pos = rect.pos;
        self.rect_size = rect.size;
        self.draw_quad(cx);
    }
    
    pub fn draw_quad_abs(&mut self, cx: &mut Cx, rect: Rect) {
        self.rect_pos = rect.pos;
        self.rect_size = rect.size;
        self.draw_quad(cx);
    }
    
    pub fn draw_quad_rel(&mut self, cx: &mut Cx, rect: Rect) {
        let rect = rect.translate(cx.get_turtle_origin());
        self.rect_pos = rect.pos;
        self.rect_size = rect.size;
        self.draw_quad(cx);
    }
    
    pub fn draw_quad(&mut self, cx: &mut Cx) {
        if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
