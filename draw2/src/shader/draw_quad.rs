
use {
    crate::{
        makepad_platform::*,
        draw_list_2d::ManyInstances,
        cx_2d::*,
        turtle::*,
    },
};

script_mod!{
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    
    mod.draw.DrawQuad = mod.std.set_type_default() do #(DrawQuad::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        pos: varying(vec2f)
        world: varying(vec4f)
        
        clip_and_transform_vertex: fn(rect_pos, rect_size){
            let clipped = clamp(
                clamp(
                    self.geom.pos * rect_size + rect_pos
                    self.draw_clip.xy
                    self.draw_clip.zw
                )
                + self.draw_list.view_shift
                self.draw_list.view_clip.xy
                self.draw_list.view_clip.zw
            )
            //clipped = self.geom_pos * rect_size + rect_pos;
            self.pos = (clipped - rect_pos) / rect_size
            self.world = self.draw_list.view_transform * vec4(
                clipped.x
                clipped.y
                self.draw_depth + self.draw_call.zbias
                1.
            );
            // only pass the clipped position forward
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * (self.world))
        }
        
        transform_vertex: fn(rect_pos, rect_size){
            let clipped = self.geom.pos * rect_size + rect_pos;
            self.pos = (clipped - rect_pos) / rect_size
            // only pass the clipped position forward
            self.world = self.draw_list.view_transform * vec4(
                clipped.x
                clipped.y
                self.draw_depth + self.draw_call.zbias
                1.
            )
            return self.draw_list.camera_projection * (self.draw_list.camera_view * (self.world ))
        }
                
        vertex: fn() {
            self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
        }
                
        fragment: fn(){
            self.fb0 = self.pixel()
        }
        
        pixel: fn(){
            #0f0
        }
    }
    
    mod.draw.DrawColor = mod.std.set_type_default() do #(DrawColor::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            return vec4(self.color.rgb*self.color.a, self.color.a);
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawQuad {
    #[rust] pub many_instances: Option<ManyInstances>,
    #[deref] pub draw_vars: DrawVars,
    #[live] pub rect_pos: Vec2f,
    #[live] pub rect_size: Vec2f,
    #[live] pub draw_clip: Vec4f,
    #[live(1.0)] pub depth_clip: f32,
    #[live(1.0)] pub draw_depth: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawColor {
    #[deref] pub draw_super: DrawQuad,
    #[live] pub color: Vec4f
}

impl DrawQuad {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk, layout: Layout) {
        cx.begin_turtle(walk, layout);
        if self.draw_vars.draw_shader_id.is_some() {
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
        
    pub fn update_abs(&mut self, cx: &mut Cx, rect: Rect) {
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw_vars.update_rect(cx, rect);
    }
        
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx);
    }
        
    pub fn draw_rel(&mut self, cx: &mut Cx2d, rect: Rect) {
        let rect = rect.translate(cx.turtle().origin());
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx);
    }
        
    pub fn new_draw_call(&self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }
        
    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.append_to_draw_call(&self.draw_vars);
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

/*
live_design!{
    use link::shaders::*;
    pub DrawQuad = {{DrawQuad}} {
        varying pos: vec2
        varying world: vec4,
        fn clip_and_transform_vertex(self, rect_pos:vec2, rect_size:vec2) -> vec4 {
            let clipped: vec2 = clamp(
                clamp(
                    self.geom_pos * rect_size + rect_pos,
                    self.draw_clip.xy,
                    self.draw_clip.zw
                )
                + self.view_shift,
                self.view_clip.xy,
                self.view_clip.zw
            );
            //clipped = self.geom_pos * rect_size + rect_pos;
            self.pos = (clipped - rect_pos) / rect_size
            self.world = self.view_transform * vec4(
                clipped.x,
                clipped.y,
                self.draw_depth + self.draw_zbias,
                1.
            );
            // only pass the clipped position forward
            return self.camera_projection * (self.camera_view * (self.world))
        }
        
        fn transform_vertex(self, rect_pos:vec2, rect_size:vec2) -> vec4 {
            let clipped: vec2 = self.geom_pos * rect_size + rect_pos;
            
            self.pos = (clipped - rect_pos) / rect_size
            // only pass the clipped position forward
            self.world = self.view_transform * vec4(
                clipped.x,
                clipped.y,
                self.draw_depth + self.draw_zbias,
                1.
            );
            return self.camera_projection * (self.camera_view * (self.world ))
        }
        
        fn vertex(self) -> vec4 {
            return self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
        }
        
        fn pixel(self)->vec4{
            return #f00
        }
        
        fn fragment(self) -> vec4 {
            return depth_clip(self.world, self.pixel(), self.depth_clip);
        }
    }
}*/

/*
#[derive(Live, LiveRegister)]
#[repr(C)]
pub struct DrawQuad {
    #[rust] pub many_instances: Option<ManyInstances>,
    #[live] pub geometry: GeometryQuad2D,
    #[deref] pub draw_vars: DrawVars,
    #[calc] pub rect_pos: Vec2f,
    #[calc] pub rect_size: Vec2f,
    #[calc] pub draw_clip: Vec4f,
    #[live(1.0)] pub depth_clip: f32,
    #[live(1.0)] pub draw_depth: f32,
}

impl LiveHook for DrawQuad{
    fn before_apply(&mut self, cx: &mut Cx, apply: &Apply, index: usize, nodes: &[LiveNode]){
        self.draw_vars.before_apply_init_shader(cx, apply, index, nodes, &self.geometry);
    }
    fn after_apply(&mut self, cx: &mut Cx, apply: &Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.after_apply_update_self(cx, apply, index, nodes, &self.geometry);
    }
}

impl DrawQuad {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk, layout: Layout) {
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
    
    pub fn update_abs(&mut self, cx: &mut Cx, rect: Rect) {
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw_vars.update_rect(cx, rect);
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx);
    }
    
    pub fn draw_rel(&mut self, cx: &mut Cx2d, rect: Rect) {
        let rect = rect.translate(cx.turtle().origin());
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx);
    }
    
    pub fn new_draw_call(&self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }
    
    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.append_to_draw_call(&self.draw_vars);
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
*/