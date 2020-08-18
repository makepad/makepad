use crate::cx::*;

#[derive(Clone)]
pub struct Cube {
    pub shader: Shader,
    pub color: Color
}

impl Cube {
    pub fn proto_with_shader(cx: &mut Cx, shader: ShaderGen, name: &str) -> Self {
        Self {
            shader: cx.add_shader(shader, name),
            ..Self::new(cx)
        }
    }
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            shader: cx.add_shader(Self::def_cube_shader(), "Cube"),
            color: pick!(#c).get(cx)
        }
    }
    
    pub fn geom_pos() -> Vec3Id {uid!()}
    pub fn geom_id() -> FloatId {uid!()}
    pub fn geom_normal() -> Vec3Id {uid!()}
    pub fn geom_uv() -> Vec2Id {uid!()}
    
    pub fn transform() -> Mat4Id {uid!()}
    pub fn color() -> ColorId {uid!()}
    
    pub fn def_cube_shader() -> ShaderGen {
        // lets add the draw shader lib
        let mut sg = Cx::shader_defs(ShaderGen::new());
        
        sg.geometry.add_cube_3d(0.1,0.1,0.1,1,1,1);

        sg.compose(shader!{"
            geometry geom_pos: Self::geom_pos();
            geometry geom_id: Self::geom_id();
            geometry geom_normal: Self::geom_normal();
            geometry geom_uv: Self::geom_uv();
            
            instance transform: Self::transform();
            instance color: Self::color();
            
            fn vertex() -> vec4 {
                return camera_projection * (camera_view * (view_transform * (transform * vec4(geom_pos.x, geom_pos.y, geom_pos.z + draw_zbias, 1.))));
            }
            
            fn pixel() -> vec4 {
                if geom_id>4.5{
                    return pick!(red);
                }
                if geom_id>3.5{
                    return pick!(green);
                }
                if geom_id>2.5{
                    return pick!(blue);
                }
                if geom_id>1.5{
                    return pick!(orange);
                }
                if geom_id>0.5{
                    return pick!(yellow);
                }
                return pick!(white);
            }
            
        "})
    }
    
    pub fn draw_cube(&mut self, cx: &mut Cx, transform: &Mat4) -> InstanceArea {
        let inst = cx.new_instance(&self.shader, 1);
        if inst.need_uniforms_now(cx) {
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        inst.push_slice(cx, &transform.v);
        let data = [
            self.color.r,
            self.color.g,
            self.color.b,
            self.color.a,
        ];
        inst.push_slice(cx, &data);
        inst
    }
    
}
