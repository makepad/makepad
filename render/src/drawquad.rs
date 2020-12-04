use crate::cx::*;

#[repr(C, packed)]
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
            shader: unsafe {self.shader.clone()},
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
        Self::with_slots(cx, default_shader_overload!(cx, shader, self::shader), 0)
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
        
        live_body!(cx, r#"
            
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
        "#);
    }
    
    pub fn with_draw_depth(mut self, draw_depth: f32) -> Self {self.draw_depth = draw_depth;self}
    pub fn with_rect_pos(mut self, rect_pos: Vec2) -> Self {self.rect_pos = rect_pos;self}
    pub fn with_rect_size(mut self, rect_size: Vec2) -> Self {self.rect_size = rect_size;self}
    //    Self {rect_size, ..self}}
    
    pub fn set_draw_depth(&mut self, cx:&mut Cx, v: f32) {
        self.draw_depth = v;
        write_draw_input!(cx, self.area(), self::DrawQuad::draw_depth, v);
    }

    pub fn set_rect_pos(&mut self, cx:&mut Cx, v: Vec2) {
        self.rect_pos = v;
        write_draw_input!(cx, self.area(), self::DrawQuad::rect_pos, v);
    }

    pub fn set_rect_size(&mut self, cx:&mut Cx, v: Vec2) {
        self.rect_size = v;
        write_draw_input!(cx, self.area(), self::DrawQuad::rect_size, v);
    }
    
    pub fn register_draw_input(cx: &mut Cx) {
        cx.live_styles.register_draw_input(live_item_id!(self::DrawQuad), Self::live_draw_input())
    }
    
    pub fn live_draw_input() -> LiveDrawInput {
        let mut def = LiveDrawInput::default();
        let mp = module_path!();
        def.add_instance(mp, "DrawQuad", "rect_pos", Vec2::ty_expr());
        def.add_instance(mp, "DrawQuad", "rect_size", Vec2::ty_expr());
        def.add_instance(mp, "DrawQuad", "draw_depth", f32::ty_expr());
        return def
    }

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
    }
    
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


