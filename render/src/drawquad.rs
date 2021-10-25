// ok lets implement these things
live_body!{
    
    use crate::shader_std::*;
    use crate::geometrygen::GeometryQuad2D;
    
    DrawQuad: DrawShader2D {
        //debug: true;
        rust_type: {{DrawQuad}};
        geometry: GeometryQuad2D {};
        
        varying pos: vec2; 
        
        fn scroll(self) -> vec2 {
            return self.draw_scroll.xy;
        }
        
        fn vertex(self) -> vec4 {
            let scr = self.scroll();
            
            let clipped: vec2 = clamp(
                self.geom_pos * self.rect_size + self.rect_pos - scr,
                self.draw_clip.xy,
                self.draw_clip.zw
            );
            self.pos = (clipped + scr - self.rect_pos) / self.rect_size;
            // only pass the clipped position forward
            return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                clipped.x,
                clipped.y,
                self.draw_depth + self.draw_zbias,
                1.
            )));
        }
        
        fn pixel(self) -> vec4 {
            return #f0f;
        }
    }
}

use crate::cx::*;

const DRAW_QUAD_VAR_UNIFORMS: usize = 32;
const DRAW_QUAD_VAR_INSTANCES: usize = 32;

//#[derive(Debug)]
#[repr(C)]
pub struct DrawQuad {
    //#[local()]
    pub uniforms: [f32; DRAW_QUAD_VAR_UNIFORMS],
    
    //#[local()]
    pub area: Area,
    
    //#[local()]
    many: Option<ManyInstances>,
    //#[local()]
    many_old_area: Area,
    
    //#[local()]
    pub draw_shader_ptr: Option<DrawShaderPtr>,
    
    //#[local(0)]
    uniform_start: usize,
    //#[local(0)]
    uniform_slots: usize,

    //#[local(0)]
    instance_start: usize,
    //#[local(5)]
    instance_slots: usize,
    
    //#[live()]
    pub geometry: GeometryQuad2D,
    
    //#[local()]
    pub draw_shader: Option<DrawShader>,
    
    //#[local()]
    pub instances: [f32; DRAW_QUAD_VAR_INSTANCES],
    
    //#[live(Vec2::all(0.0))]
    pub rect_pos: Vec2,
    //#[live(Vec2::all(0.0))]
    pub rect_size: Vec2,
    //#[live(1.0)]
    pub draw_depth: f32,
    //#[pad_f32()]
}

impl DrawQuad {
    pub fn live_update_value(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {
        match id {
            id!(geometry) => self.geometry.live_update(cx, ptr),
            id!(rect_pos) => self.rect_pos.live_update(cx, ptr),
            id!(rect_size) => self.rect_size.live_update(cx, ptr),
            id!(draw_depth) => self.draw_depth.live_update(cx, ptr),
            _ => self.live_update_value_unknown(cx, id, ptr)
        }
    }
}

impl LiveUpdateHooks for DrawQuad {
    fn live_update_value_unknown(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {
        cx.update_var_inputs(self.draw_shader_ptr.unwrap(), ptr, id, &mut self.uniforms, &mut self.instances);
    }
    
    fn before_live_update(&mut self, cx:&mut Cx, live_ptr: LivePtr){
        self.draw_shader_ptr = Some(DrawShaderPtr(live_ptr));
        self.draw_shader = cx.get_draw_shader_from_ptr(self.draw_shader_ptr.unwrap(), &self.geometry);
    }
    
    fn after_live_update(&mut self, cx: &mut Cx, _live_ptr:LivePtr) {
        cx.get_var_inputs_instance_layout(
            self.draw_shader,
            &mut self.instance_start,
            &mut self.instance_slots,
            DRAW_QUAD_VAR_INSTANCES,
        );
    }
}

// how could we compile this away
impl LiveNew for DrawQuad {
    fn live_new(cx: &mut Cx) -> Self {
        Self {
            uniforms: [0.0; DRAW_QUAD_VAR_UNIFORMS],
            
            area: Area::Empty,
            many: None,
            many_old_area: Area::Empty,
            
            uniform_start: 0,
            uniform_slots: 0,
            instance_start: 0,
            instance_slots: 0,
            draw_shader: None,
            geometry: LiveNew::live_new(cx),
            
            draw_shader_ptr: None,
            instances: [0.0; DRAW_QUAD_VAR_INSTANCES],
            rect_pos: Vec2::all(0.0),
            rect_size: Vec2::all(0.0),
            draw_depth: 1.0,
        }
    }
    
    fn live_type() -> LiveType {
        LiveType(std::any::TypeId::of::<DrawQuad>())
    }
    
    fn live_register(cx: &mut Cx) {
        cx.register_live_body(live_body());
        struct Factory();
        impl LiveFactory for Factory {
            fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveUpdate> {
                Box::new(DrawQuad ::live_new(cx))
            }
            
            fn live_fields(&self, fields: &mut Vec<LiveField>) {
                fields.push(LiveField {id: Id::from_str("geometry").unwrap(), live_type: GeometryQuad2D::live_type()});
                fields.push(LiveField {id: Id::from_str("rect_pos").unwrap(), live_type: Vec2::live_type()});
                fields.push(LiveField {id: Id::from_str("rect_size").unwrap(), live_type: Vec2::live_type()});
                fields.push(LiveField {id: Id::from_str("draw_depth").unwrap(), live_type: f32::live_type()});
                // can i somehow someway autogenerate this
                fields.push(LiveField {id: Id(0), live_type: f32::live_type()});
            }
            
            fn live_type(&self) -> LiveType {
                DrawQuad::live_type()
            }
        }
        cx.register_factory(DrawQuad::live_type(), Box::new(Factory()));
    }
}

impl LiveUpdate for DrawQuad {
    fn live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {
        self.before_live_update(cx, live_ptr);
        // how do we verify this?
        if let Some(mut iter) = cx.shader_registry.live_registry.live_class_iterator(live_ptr) {
            while let Some((id, live_ptr)) = iter.next(&cx.shader_registry.live_registry) {
                if id == id!(rust_type) && !cx.verify_type_signature(live_ptr, Self::live_type()) {
                    // give off an error/warning somehow!
                    return;
                }
                self.live_update_value(cx, id, live_ptr)
            }
        }
        self.after_live_update(cx, live_ptr);
    }
    
    fn _live_type(&self) -> LiveType {
        Self::live_type()
    }
}


impl DrawQuad {
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) {
        if self.many.is_some() {
            panic!("Cannot use begin_quad inside a many block");
        }
        if let Some(draw_shader) = self.draw_shader {
            let new_area = cx.add_aligned_instance(draw_shader, self.as_slice());
            self.area = cx.update_area_refs(self.area, new_area);
        }
        cx.begin_turtle(layout, self.area);
    }
    
    pub fn end_quad(&mut self, cx: &mut Cx) {
        let rect = cx.end_turtle(self.area);
        self.area.set_rect(cx, &rect);
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
        if let Some(mi) = &mut self.many {
            let new_area = if let Area::Instance(ia) = &mut self.area {
                // we need to update the area pointer
                if mi.instance_area.redraw_id != ia.redraw_id {
                    Some(Area::Instance(InstanceArea {
                        instance_count: 1,
                        instance_offset: mi.instances.len(),
                        ..mi.instance_area.clone()
                    }))
                }
                else { // just patch up the area without notifying Cx
                    ia.instance_count = 1;
                    ia.instance_offset = mi.instances.len();
                    None
                }
            }
            else {
                None
            };
            unsafe {
                mi.instances.extend_from_slice(std::slice::from_raw_parts((&self.instances[self.instance_start - 1] as *const _ as *const f32).offset(1), self.instance_slots));
            }
            
            if let Some(new_area) = new_area {
                self.area = cx.update_area_refs(self.area, new_area);
            }
            return
        }
        if let Some(draw_shader) = self.draw_shader {
            let new_area = cx.add_aligned_instance(draw_shader, self.as_slice());
            self.area = cx.update_area_refs(self.area, new_area);
        }
    }
    
    pub fn begin_many(&mut self, cx: &mut Cx) {
        if let Some(draw_shader) = self.draw_shader {
            let mi = cx.begin_many_aligned_instances(draw_shader, self.instance_slots);
            self.many_old_area = self.area;
            //self.many_set_area = false;
            self.area = Area::Instance(InstanceArea {
                instance_count: 0,
                instance_offset: mi.instances.len(),
                ..mi.instance_area.clone()
            });
            self.many = Some(mi);
        }
    }
    
    pub fn end_many(&mut self, cx: &mut Cx) {
        if let Some(mi) = self.many.take() {
            // update area pointer
            let new_area = cx.end_many_instances(mi);
            self.area = cx.update_area_refs(self.many_old_area, new_area);
        }
    }
    
    pub fn as_slice<'a>(&'a self) -> &'a [f32] {
        unsafe {
            std::slice::from_raw_parts((&self.instances[self.instance_start - 1] as *const _ as *const f32).offset(1), self.instance_slots)
        }
    }
    
}

