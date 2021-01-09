use crate::cx::*;

#[repr(C, packed)]
pub struct DrawCube {
    pub shader: Shader,
    pub area: Area,
    pub many: Option<ManyInstances>,
    pub slots: usize,
    
    pub transform: Mat4,
    pub color: Vec4,
    pub cube_size: Vec3,
    pub cube_pos: Vec3
}

impl Clone for DrawCube {
    fn clone(&self) -> Self {
        Self {
            shader: unsafe {self.shader.clone()},
            area: Area ::Empty,
            many: None,
            slots: self.slots,
            transform: self.transform,
            color: self.color,
            cube_size: self.cube_size,
            cube_pos: self.cube_pos
        }
    }
}

impl DrawCube {
    
    pub fn new(cx: &mut Cx, shader: Shader) -> Self {
        Self::with_slots(cx, default_shader_overload!(cx, shader, self::shader), 0)
    }
    
    pub fn with_slots(_cx: &mut Cx, shader: Shader, slots: usize) -> Self {
        Self {
            shader: shader,
            slots: slots + 26,
            area: Area::Empty,
            many: None,
            
            transform: Mat4::identity(),
            color: vec4(1.0, 0.0, 0.0, 1.0),
            cube_size: vec3(0.1, 0.1, 0.1),
            cube_pos: vec3(0.0, 0.0, 0.0)
        }
    }
    
    pub fn with_cube_size(self, cube_size: Vec3)->Self {Self {cube_size, ..self}}
    pub fn with_cube_pos(self, cube_pos: Vec3)->Self {Self {cube_pos, ..self}}
    pub fn with_transform(self, transform: Mat4)->Self {Self {transform, ..self}}
    pub fn with_color(self, color: Vec4)->Self {Self {color, ..self}}
    
    pub fn live_draw_input() -> LiveDrawInput {
        let mut def = LiveDrawInput::default();
        let mp = module_path!();
        def.add_instance(mp, "DrawCube", "transform", Mat4::ty_expr());
        def.add_instance(mp, "DrawCube", "color", Vec4::ty_expr());
        def.add_instance(mp, "DrawCube", "cube_size", Vec3::ty_expr());
        def.add_instance(mp, "DrawCube", "cube_pos", Vec3::ty_expr());
        return def
    }
    
    pub fn register_draw_input(cx: &mut Cx) {
        cx.live_styles.register_draw_input(live_item_id!(self::DrawCube), Self::live_draw_input())
    }
    
    pub fn style(cx: &mut Cx) {
        
        Self::register_draw_input(cx);
        
        live_body!(cx, {
            self::shader: Shader {
                use crate::shader_std::prelude::*;
                use crate::shader_std::geometry_3d::*;
                default_geometry: crate::shader_std::cube_3d;
                draw_input: self::DrawCube;
                
                varying lit_col: vec4;
                
                fn vertex() -> vec4 {
                    let model_view = view_transform * transform;
                    let normal_matrix = mat3(model_view);
                    let normal = normalize(normal_matrix * geom_normal);
                    let dp = abs(normal.z);
                    
                    lit_col = vec4(color.rgb * dp, color.a);
                    return camera_projection * (camera_view * model_view * vec4(
                        geom_pos.x * cube_size.x + cube_pos.x,
                        geom_pos.y * cube_size.y + cube_pos.y,
                        geom_pos.z * cube_size.z + cube_pos.z + draw_zbias,
                        1.
                    ));
                }
                
                fn pixel() -> vec4 {
                    return lit_col;
                }
            }
        })
        
    }
    
    pub fn area(&self) -> Area {
        self.area
    }
    
    pub fn set_transform(&mut self, cx: &mut Cx, transform: Mat4) {
        self.transform = transform;
        write_draw_input!(cx, self.area(), self::DrawCube::transform, transform);
    }
    
    pub fn set_cube_pos(&mut self, cx: &mut Cx, cube_pos: Vec3) {
        self.cube_pos = cube_pos;
        write_draw_input!(cx, self.area(), self::DrawCube::cube_pos, cube_pos);
    }
    
    pub fn set_cube_size(&mut self, cx: &mut Cx, cube_size: Vec3) {
        self.cube_size = cube_size;
        write_draw_input!(cx, self.area(), self::DrawCube::cube_size, cube_size);
    }
    
    pub fn get_transform(&mut self)->&Mat4 {
        unsafe{&self.transform}
    }
    
    
    pub fn draw_cube(
        &mut self,
        cx: &mut Cx,
    ) {
        self.area = cx.add_instance(self.shader, self.as_slice());
    }
    
    pub fn begin_many(&mut self, cx: &mut Cx) {
        self.many = Some(cx.begin_many_instances(self.shader, self.slots))
    }
    
    pub fn add_cube(&mut self) {
        unsafe {
            if let Some(li) = &mut self.many {
                li.instances.extend_from_slice(std::slice::from_raw_parts(&self.transform as *const _ as *const f32, self.slots));
            }
        }
    }
    
    pub fn end_many(&mut self, cx: &mut Cx) {
        unsafe {
            if let Some(li) = self.many.take() {
                self.area = cx.end_many_instances(li);
            }
        }
    }
    
    pub fn as_slice<'a>(&'a self) -> &'a [f32] {
        unsafe {
            std::slice::from_raw_parts(&self.transform as *const _ as *const f32, self.slots)
        }
    }
}
