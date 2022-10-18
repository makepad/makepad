use {
    crate::{
        makepad_platform::*,
        view::ManyInstances,
        geometry::GeometryQuad2D,
        cx_2d::Cx2d,
        turtle::{Walk, Layout}
    },
};

live_design!{
    
    DrawQuad = {{DrawQuad}} {
        varying pos: vec2
        
        fn clip_and_transform_vertex(self) -> vec4 {
            let clipped: vec2 = clamp(
                self.geom_pos * self.rect_size + self.rect_pos,
                self.draw_clip.xy,
                self.draw_clip.zw
            )
            self.pos = (clipped - self.rect_pos) / self.rect_size
            // only pass the clipped position forward
            return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                clipped.x,
                clipped.y,
                self.draw_depth + self.draw_zbias,
                1.
            )))
        }
        
        fn transform_vertex(self) -> vec4 {
            let clipped: vec2 = self.geom_pos * self.rect_size + self.rect_pos;
            
            self.pos = (clipped - self.rect_pos) / self.rect_size
            // only pass the clipped position forward
            return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                clipped.x,
                clipped.y,
                self.draw_depth + self.draw_zbias,
                1.
            )))
        }
        
        fn vertex(self) -> vec4 {
            return self.clip_and_transform_vertex()
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
    #[calc] pub draw_clip: Vec4,
    #[live(1.0)] pub draw_depth: f32,
}

impl DrawQuad {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk, layout: Layout) {
        self.draw_clip = cx.turtle().draw_clip().into();
        cx.begin_turtle(walk, layout);
        if self.draw_vars.draw_shader.is_some() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        let rect = cx.end_turtle();
        self.draw_vars.area.set_rect(cx, &rect);
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        let rect = cx.walk_turtle(walk);
        self.draw_clip = cx.turtle().draw_clip().into();
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx);
        rect
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = &mut self.many_instances {
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
        }
        else if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_clip = cx.turtle().draw_clip().into();
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx);
    }
    
    pub fn draw_rel(&mut self, cx: &mut Cx2d, rect: Rect) {
        let rect = rect.translate(cx.turtle().origin());
        self.draw_clip = cx.turtle().draw_clip().into();
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx);
    }
    
    pub fn new_draw_call(&self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }
    
    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }
    
    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        let mi = cx.begin_many_aligned_instances(&self.draw_vars);
        self.many_instances = mi;
    }
    
    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
