// ok lets implement these things
live_body!{
    
    use crate::shader_std::*;
    use crate::geometrygen::GeometryQuad2D;
    
    DrawQuad: DrawShader2D {
        rust_type: {{DrawQuad}};
        geometry: GeometryQuad2D {};
        varying pos: vec2;
        
        //let dpi_dilate: float<Uniform>;
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
        
        fn pixel() -> vec4 {
            return #0f0;
        }
    }
}

use crate::cx::*;

const DRAW_QUAD_UNIFORMS: usize = 32;
const DRAW_QUAD_INSTANCES: usize = 32;

//#[derive(Debug)]
#[repr(C)]
pub struct DrawQuad {
    //#[after_live_update()]
    //#[private()]
    pub uniforms: [f32; DRAW_QUAD_UNIFORMS],
    
    //#[private()]
    pub area: Area,
    
    //#[private()]
    pub many: Option<ManyInstances>,
    //#[private()]
    pub many_old_area: Area,
    
    //#[private()]
    pub live_ptr: Option<LivePtr>,
    
    //#[private()]
    pub instance_start: usize,
    //#[private()]
    pub instance_slots: usize,
    
    pub geometry: GeometryQuad2D,
    
    //#[private()]
    pub shader: Option<Shader>,
    
    //#[private()]
    pub instances: [f32; DRAW_QUAD_INSTANCES],
    
    //#[default(Vec2::all(0.0))]
    pub rect_pos: Vec2,
    //#[default(Vec2::all(0.0))]
    pub rect_size: Vec2,
    //#[default(1.0)]
    pub draw_depth: f32
}

impl DrawQuad {
    fn live_update_value(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {
        match id {
            id!(rect_pos) => self.rect_pos.live_update(cx, ptr),
            id!(rect_size) => self.rect_size.live_update(cx, ptr),
            id!(draw_depth) => self.draw_depth.live_update(cx, ptr),
            id!(geometry) => self.geometry.live_update(cx, ptr),
            _ => ()
        }
    }
}

impl DrawQuad {
    fn after_live_update(&mut self, cx: &mut Cx) {
        // lets fetch/compile our shader from the info we have
        self.shader = cx.get_shader_from_ptr(self.live_ptr.unwrap(), &self.geometry);
    }
}

// how could we compile this away
impl LiveNew for DrawQuad {
    fn live_new(cx: &mut Cx) -> Self {
        Self {
            uniforms: [0.0; DRAW_QUAD_UNIFORMS],
            
            area: Area::Empty,
            many: None,
            many_old_area: Area::Empty,
            
            instance_start: DRAW_QUAD_INSTANCES,
            instance_slots: 5,
            shader: None,
            geometry: LiveNew::live_new(cx),
            
            live_ptr: None,
            instances: [0.0; DRAW_QUAD_INSTANCES],
            rect_pos: Vec2::all(0.0),
            rect_size: Vec2::all(0.0),
            draw_depth: 1.0
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
                fields.push(LiveField {id: id!(geometry), live_type: GeometryQuad2D::live_type()});
                fields.push(LiveField {id: id!(rect_pos), live_type: Vec2::live_type()});
                fields.push(LiveField {id: id!(rect_size), live_type: Vec2::live_type()});
                fields.push(LiveField {id: id!(draw_depth), live_type: f32::live_type()});
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
        self.live_ptr = Some(live_ptr);
        if let Some(mut iter) = cx.shader_registry.live_registry.live_class_iterator(live_ptr) {
            while let Some((id, live_ptr)) = iter.next(&cx.shader_registry.live_registry) {
                if id == id!(rust_type) && !cx.verify_type_signature(live_ptr, Self::live_type()) {
                    return;
                }
                self.live_update_value(cx, id, live_ptr)
            }
        }
        self.after_live_update(cx);
    }
    
    fn _live_type(&self) -> LiveType {
        Self::live_type()
    }
}


pub struct DrawColor {
    base: DrawQuad,
    color: Vec4
}

impl std::ops::Deref for DrawColor {
    type Target = DrawQuad;
    fn deref(&self) -> &Self::Target {&self.base}
}

impl std::ops::DerefMut for DrawColor {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.base}
}





impl DrawQuad {
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) {
        if self.many.is_some() {
            panic!("Cannot use begin_quad inside a many block");
        }
        if let Some(shader) = self.shader {
            let new_area = cx.add_aligned_instance(shader, self.as_slice());
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
                mi.instances.extend_from_slice(std::slice::from_raw_parts(&self.instances[self.instance_start] as *const _ as *const f32, self.instance_slots));
            }
            
            if let Some(new_area) = new_area {
                self.area = cx.update_area_refs(self.area, new_area);
            }
            return
        }
        if let Some(shader) = self.shader {
            let new_area = cx.add_aligned_instance(shader, self.as_slice());
            self.area = cx.update_area_refs(self.area, new_area);
        }
    }
    
    pub fn begin_many(&mut self, cx: &mut Cx) {
        if let Some(shader) = self.shader {
            let mi = cx.begin_many_aligned_instances(shader, self.instance_slots);
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
            std::slice::from_raw_parts(&self.instances[self.instance_start] as *const _ as *const f32, self.instance_slots)
        }
    }
    
}

// ok so what if we have general purpose reflection
// how does inheritance work and how does the inner one know its full outer structure


/*
#[derive(Debug)]
#[repr(C)]
pub struct DrawQuad {
    pub shader: Shader,
    pub area: Area,
    pub many: Option<ManyInstances>,
    pub many_old_area: Area,
   //pub many_set_area: bool,
    pub slots: usize,
    pub rect_pos: Vec2,
    pub rect_size: Vec2,
    pub draw_depth: f32
}

impl Clone for DrawQuad {
    fn clone(&self) -> Self {
        Self {
            shader: self.shader.clone(),
            area: Area ::Empty,
            many: None,
            // many_set_area: false,
            many_old_area: Area::Empty,
            slots: self.slots,
            rect_pos: self.rect_pos,
            rect_size: self.rect_size,
            draw_depth: self.draw_depth
        }
    }
}

impl DrawQuad {
    pub fn new(cx: &mut Cx, shader: Shader) -> Self {
        Self::with_slots(cx, default_shader!(), 0)
    }
    
    pub fn with_slots(_cx: &mut Cx, shader: Shader, slots: usize) -> Self {
        Self {
            shader: shader,
            slots: slots + 5,
            area: Area::Empty,
            many: None,
            //many_set_area: false,
            many_old_area: Area::Empty,
            rect_pos: Vec2::default(),
            rect_size: Vec2::default(),
            draw_depth: 0.0
        }
    }
        
    pub fn style(cx: &mut Cx) {
        
        Self::register_draw_input(cx);
        
        live_body!(cx, {
            
            self::shader: Shader {
                
                use crate::shader_std::prelude::*;
                
                default_geometry: crate::shader_std::quad_2d;
                geometry geom: vec2;
                
                varying pos: vec2;
                
                draw_input: self::DrawQuad;
                
                //let dpi_dilate: float<Uniform>;
                fn scroll() -> vec2 {
                    return draw_scroll.xy;
                }
                
                fn vertex() -> vec4 {
                    let scr = scroll();
                    
                    let clipped: vec2 = clamp(
                        geom * rect_size + rect_pos - scr,
                        draw_clip.xy,
                        draw_clip.zw
                    );
                    pos = (clipped + scr - rect_pos) / rect_size;
                    // only pass the clipped position forward
                    return camera_projection * (camera_view * (view_transform * vec4(
                        clipped.x,
                        clipped.y,
                        draw_depth + draw_zbias,
                        1.
                    )));
                }
                
                fn pixel() -> vec4 {
                    return #0f0;
                }
            }
        });
    }
    
    pub fn with_draw_depth(mut self, draw_depth: f32) -> Self {self.draw_depth = draw_depth;self}
    pub fn with_rect_pos(mut self, rect_pos: Vec2) -> Self {self.rect_pos = rect_pos;self}
    pub fn with_rect_size(mut self, rect_size: Vec2) -> Self {self.rect_size = rect_size;self}
    //    Self {rect_size, ..self}}
    /*
    pub fn set_draw_depth(&mut self, cx:&mut Cx, v: f32) {
        self.draw_depth = v;
        write_draw_input!(cx, self.area(), draw_depth, v);
    }

    pub fn set_rect_pos(&mut self, cx:&mut Cx, v: Vec2) {
        self.rect_pos = v;
        write_draw_input!(cx, self.area(), rect_pos, v);
    }

    pub fn set_rect_size(&mut self, cx:&mut Cx, v: Vec2) {
        self.rect_size = v;
        write_draw_input!(cx, self.area(), rect_size, v);
    }
    
    pub fn register_draw_input(cx: &mut Cx) {
        cx.shader_registry.register_draw_input(live_id!(self::DrawQuad), Self::live_draw_input())
    }
    
    pub fn live_draw_input() -> DrawShaderInput {
        let mut def = DrawShaderInput::default();
        let mp = module_path!();
        def.add_instance(mp, "DrawQuad", "rect_pos", Vec2::to_ty());
        def.add_instance(mp, "DrawQuad", "rect_size", Vec2::to_ty());
        def.add_instance(mp, "DrawQuad", "draw_depth", f32::to_ty());
        def.end_level();
        return def
    }*/
/*
    pub fn last_animate(&mut self, animator:&Animator){
        if let Some(v) = Vec2::last_animate(animator, live_item_id!(self::DrawQuad::rect_pos)){
            self.rect_pos = v;
        }
        if let Some(v) = Vec2::last_animate(animator, live_item_id!(self::DrawQuad::rect_size)){
            self.rect_size = v;
        }
    }
    
    pub fn animate(&mut self, cx: &mut Cx, animator:&mut Animator, time:f64){
        if let Some(v) = Vec2::animate(cx, animator, time, live_item_id!(self::DrawQuad::rect_pos)){
            self.set_rect_pos(cx, v);
        }
        if let Some(v) = Vec2::animate(cx, animator, time, live_item_id!(self::DrawQuad::rect_size)){
            self.set_rect_size(cx, v);
        }
    }*/
    
    pub fn new_draw_call(&mut self, cx:&mut Cx){
        cx.new_draw_call(self.shader);
    }
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) {
        if unsafe{self.many.is_some()}{
            panic!("Cannot use begin_quad inside a many block");
        }
        let new_area = cx.add_aligned_instance(self.shader, self.as_slice());
        self.area = cx.update_area_refs(self.area, new_area);
        cx.begin_turtle(layout, self.area);
    }
    
    pub fn end_quad(&mut self, cx: &mut Cx) {
        let rect = cx.end_turtle(self.area);
        //println!("GOT RECT {:?}", rect);
        unsafe {self.area.set_rect(cx, &rect)};
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
        unsafe {
            if let Some(mi) = &mut self.many {
                
                let new_area = if let Area::Instance(ia) = &mut self.area{
                    // we need to update the area pointer
                    if mi.instance_area.redraw_id != ia.redraw_id{
                        Some(Area::Instance(InstanceArea {
                            instance_count: 1,
                            instance_offset: mi.instances.len(),
                            ..mi.instance_area.clone()
                        }))
                    }
                    else{ // just patch up the area without notifying Cx
                        ia.instance_count = 1;
                        ia.instance_offset=  mi.instances.len();
                        None
                    }
                }
                else{
                    None
                };
                mi.instances.extend_from_slice(std::slice::from_raw_parts(&self.rect_pos as *const _ as *const f32, self.slots));
                
                if let Some(new_area) = new_area{
                    self.area = cx.update_area_refs(self.area, new_area);
                }
                return
            }
        }
        let new_area = cx.add_aligned_instance(self.shader, self.as_slice());
        self.area = cx.update_area_refs(self.area, new_area);
    }
    
    pub fn area(&self) -> Area {
        self.area
    }

    pub fn set_area(&mut self, area:Area) {
        self.area = area;
    }

    pub fn shader(&self) -> Shader{
        self.shader
    }

    pub fn set_shader(&mut self, shader: Shader){
        self.shader = shader;
    }

    pub fn begin_many(&mut self, cx: &mut Cx) {
        let mi = cx.begin_many_aligned_instances(self.shader, self.slots);
        self.many_old_area = self.area;
        //self.many_set_area = false;
        self.area = Area::Instance(InstanceArea {
            instance_count: 0,
            instance_offset: mi.instances.len(),
            ..mi.instance_area.clone()
        });
        self.many = Some(mi);
    }
    
    pub fn end_many(&mut self, cx: &mut Cx) {
        unsafe {
            if let Some(mi) = self.many.take() {
                // update area pointer
                let new_area = cx.end_many_instances(mi);
                self.area = cx.update_area_refs(self.many_old_area, new_area);
            }
        }
    }
    
    pub fn as_slice<'a>(&'a self) -> &'a [f32] {
        unsafe {
            std::slice::from_raw_parts(&self.rect_pos as *const _ as *const f32, self.slots)
        }
    }
}

*/
