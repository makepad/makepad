use {
    crate::{
        makepad_platform::*,
        draw_list_2d::ManyInstances,
        geometry::GeometryCube3D,
        cx_draw::CxDraw,
    },
};

live_design!{
    use link::shaders::*;
    pub DrawCube = {{DrawCube}} {
        
        varying lit_color: vec4;
        fn vertex(self) -> vec4 {
            let pos = self.cube_size * self.geom_pos + self.cube_pos;
            let model_view = self.view_transform * self.transform;
            
            let normal_matrix = mat3(model_view);
            let normal = normalize(normal_matrix * self.geom_normal);
            let dp = max(dot(normal, normalize(vec3(0.0,1.0,1.0))), 0.0);
            let ambient = vec3(0.2,0.2,0.2)
            let color = self.color.xyz * dp + ambient;
            
            self.lit_color = vec4(color * self.color.w, self.color.w);
             
            return self.camera_projection * (self.camera_view * (model_view * vec4(pos, 1.)))
        }
        
        fn pixel(self) -> vec4 {
            return self.lit_color
        }
    }
}

#[derive(Live, LiveRegister)]
#[repr(C)]
pub struct DrawCube {
    #[rust] pub many_instances: Option<ManyInstances>,
    #[live] pub geometry: GeometryCube3D,
    #[deref] pub draw_vars: DrawVars,
    #[live] pub color: Vec4,
    #[calc] pub transform: Mat4,
    #[live(vec3(1.0,1.0,1.0))] pub cube_size: Vec3,
    #[live(vec3(0.,0.,0.))] pub cube_pos: Vec3,
}

impl LiveHook for DrawCube{
    fn before_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]){
        self.draw_vars.before_apply_init_shader(cx, apply, index, nodes, &self.geometry);
    }
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.after_apply_update_self(cx, apply, index, nodes, &self.geometry);
    }
}

impl DrawCube {

    pub fn draw(&mut self, cx: &mut CxDraw) {
        if let Some(mi) = &mut self.many_instances {
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
        }
        else if self.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
    
    pub fn new_draw_call(&self, cx: &mut CxDraw) {
        cx.new_draw_call(&self.draw_vars);
    }
    
    pub fn begin_many_instances(&mut self, cx: &mut CxDraw) {
        let mi = cx.begin_many_instances(&self.draw_vars);
        self.many_instances = mi;
    }
    
    pub fn end_many_instances(&mut self, cx: &mut CxDraw) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
