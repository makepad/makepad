use crate::cx::*;

#[derive(Clone)]
pub struct Cube {
    pub shader: Shader,
    pub color: Color
}

impl Cube {

    pub fn new(cx: &mut Cx) -> Self {
        Self {
            shader: live_shader!(cx, self::shader),
            color: Color::parse_hex_str("c").unwrap()
        }
    }
    
    pub fn style(cx: &mut Cx) {
        
        live_body!(cx, r#"self::shader: Shader {
            use crate::shader_std::prelude::*;
            use crate::shader_std::geometry_3d::*;
            default_geometry: crate::shader_std::cube_3d;
            
            instance transform: mat4;
            instance color: vec4;
            instance size: vec3;
            instance pos: vec3;
            
            varying lit_col: vec4;
            
            fn color_form_id() -> vec4 {
                return #c;
            }
            
            fn vertex() -> vec4 {
                let model_view =  view_transform * transform;
                let normal_matrix = mat3(model_view);
                let normal = normalize(normal_matrix * geom_normal);
                let dp = abs(normal.z);
                let color = color_form_id();
                lit_col = vec4(color.rgb * dp, color.a);
                return camera_projection * (camera_view * model_view * vec4(geom_pos.x * size.x + pos.x, geom_pos.y * size.y + pos.y, geom_pos.z * size.z + pos.z + draw_zbias, 1.));
            }
            
            fn pixel() -> vec4 {
                return lit_col;
            }
        }"#)
        
    }
    
    pub fn draw_cube(&mut self, cx: &mut Cx, size: Vec3, pos: Vec3, transform: &Mat4) -> InstanceArea {
        let inst = cx.new_instance(self.shader, None, 1);
        //println!("{:?} {}", area, cx.current_draw_list_id);
        inst.push_slice(cx, &transform.v);
        let data = [
            self.color.r,
            self.color.g,
            self.color.b,
            self.color.a,
            size.x,
            size.y,
            size.z,
            pos.x,
            pos.y,
            pos.z,
        ];
        inst.push_slice(cx, &data);
        inst
    }
    
}
