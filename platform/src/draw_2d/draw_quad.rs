use {
    crate::{
        makepad_derive_live::*,
        makepad_math::*,
        cx::Cx,
        draw_2d::cx_2d::Cx2d,
        live_traits::*,
        shader::geometry_gen::GeometryQuad2D,
        draw_vars::DrawVars,
        draw_2d::view::ManyInstances,
        draw_2d::turtle::{Layout, Walk, Rect}
    },
};

live_register!{
    
    DrawQuad: {{DrawQuad}} {
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

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawQuad {
    #[rust] pub many_instances: Option<ManyInstances>,
    #[live] pub geometry: GeometryQuad2D,
    #[calc] pub draw_vars: DrawVars,
    #[calc] pub rect_pos: Vec2,
    #[calc] pub rect_size: Vec2,
    #[live(1.0)] pub draw_depth: f32,
}

impl DrawQuad {
    
    pub fn begin(&mut self, cx: &mut Cx2d, layout: Layout) {
        if self.draw_vars.draw_shader.is_some() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
        cx.begin_turtle_with_guard(layout, self.draw_vars.area);
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        let rect = cx.end_turtle_with_guard(self.draw_vars.area);
        self.draw_vars.area.set_rect(cx, &rect);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        let rect = cx.walk_turtle(walk);
        self.rect_pos = rect.pos;
        self.rect_size = rect.size;
        self.draw(cx);
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.rect_pos = rect.pos;
        self.rect_size = rect.size;  
        self.draw(cx);
    }
    
    pub fn draw_rel(&mut self, cx: &mut Cx2d, rect: Rect) {
        let rect = rect.translate(cx.get_turtle_origin());
        self.rect_pos = rect.pos;
        self.rect_size = rect.size;
        self.draw(cx);
    }

    pub fn new_draw_call(&self, cx:&mut Cx2d){
        cx.new_draw_call(&self.draw_vars);
    }

    pub fn begin_many_instances(&mut self, cx: &mut Cx2d){
        let mi = cx.begin_many_aligned_instances(&self.draw_vars);
        self.many_instances = mi;   
    }

    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = &mut self.many_instances{
            mi.instances.extend_from_slice(self.draw_vars.as_slice());            
        }
        else if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
