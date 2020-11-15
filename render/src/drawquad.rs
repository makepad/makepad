use crate::cx::*;

#[repr(C, packed)]
pub struct DrawQuad {
    pub shader: Shader,
    pub area: Area,
    pub lock: Option<LockedInstances>,
    pub slots: usize,
    pub rect_pos: Vec2,
    pub rect_size: Vec2,
    pub instance_z: f32
}

impl Clone for DrawQuad {
    fn clone(&self) -> Self {
        Self {
            shader: unsafe {self.shader.clone()},
            area: Area ::Empty,
            lock: None,
            slots: self.slots,
            rect_pos: Vec2::default(),
            rect_size: Vec2::default(),
            instance_z: self.instance_z
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
            lock: None,
            rect_pos: Vec2::default(),
            rect_size: Vec2::default(),
            instance_z: 0.0
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
                        instance_z + draw_zbias,
                        1.
                    )));
                }
                
                fn pixel() -> vec4 {
                    return #0f0;
                }
            }
        "#);
    }
    
    pub fn register_draw_input(cx: &mut Cx) {
        cx.live_styles.register_draw_input(live_item_id!(self::DrawQuad), Self::live_draw_input())
    }
    
    pub fn live_draw_input() -> LiveDrawInput {
        let mut def = LiveDrawInput::default();
        let mp = module_path!();
        def.add_instance(mp, "DrawQuad", "rect_pos", Vec2::ty_expr());
        def.add_instance(mp, "DrawQuad", "rect_size", Vec2::ty_expr());
        def.add_instance(mp, "DrawQuad", "instance_z", f32::ty_expr());
        return def
    }
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) {
        self.area = cx.add_aligned_instance(self.shader, self.as_slice());
        cx.begin_turtle(layout, self.area);
    }
    
    pub fn end_quad(&mut self, cx: &mut Cx)  {
        let rect = cx.end_turtle(self.area);
        unsafe {self.area.set_rect(cx, &rect)};
    }
    
    pub fn draw_quad(&mut self, cx: &mut Cx, walk: Walk) {
        let rect = cx.walk_turtle(walk);
        self.rect_pos = vec2(rect.x, rect.y);
        self.rect_size = vec2(rect.w, rect.h);
        self.area = cx.add_aligned_instance(self.shader, self.as_slice());
    }
    
    pub fn draw_quad_rel(&mut self, cx: &mut Cx, rect: Rect) {
        let rect = rect.translate(cx.get_turtle_origin());
        self.rect_pos = vec2(rect.x, rect.y);
        self.rect_size = vec2(rect.w, rect.h);
        self.area = cx.add_aligned_instance(self.shader, self.as_slice())
    }
    
    pub fn draw_quad_abs(&mut self, cx: &mut Cx, rect: Rect) {
        self.rect_pos = vec2(rect.x, rect.y);
        self.rect_size = vec2(rect.w, rect.h);
        self.area = cx.add_instance(self.shader, self.as_slice());
    }
    
    pub fn animate(&mut self, _animator: &mut Animator, _time: f64) {
    }
    
    pub fn area(&self) -> Area {
        self.area
    }
    
    pub fn last_animator(&mut self, _animator: &Animator) {
    }
    
    pub fn lock_quad(&mut self, cx: &mut Cx) {
        self.lock = Some(cx.lock_instances(self.shader, self.slots))
    }
    
    pub fn lock_aligned_quad(&mut self, cx: &mut Cx) {
        self.lock = Some(cx.lock_aligned_instances(self.shader, self.slots))
    }
    
    pub fn add_quad(&mut self, rect: Rect) {
        self.rect_pos = vec2(rect.x, rect.y);
        self.rect_size = vec2(rect.w, rect.h);
        unsafe {
            if let Some(li) = &mut self.lock {
                li.instances.extend_from_slice(std::slice::from_raw_parts(&self.rect_pos as *const _ as *const f32, self.slots));
            }
        }
    }
    
    pub fn get_next_locked_area(&mut self) -> Area {
        unsafe {
            if let Some(li) = &mut self.lock {
                // return the area for the last locked item
                return Area::Instance(InstanceArea {
                    instance_count: 1,
                    instance_offset: li.instances.len(),
                    ..li.instance_area.clone()
                })
            }
        }
        Area::Empty
    }
    
    pub fn unlock_quad(&mut self, cx: &mut Cx) {
        unsafe {
            if let Some(li) = self.lock.take() {
                self.area = cx.unlock_instances(li);
            }
        }
    }
    
    pub fn as_slice<'a>(&'a self) -> &'a [f32] {
        unsafe {
            std::slice::from_raw_parts(&self.rect_pos as *const _ as *const f32, self.slots)
        }
    }
}