use crate::cx::*;

#[derive(Default, Clone, Debug)]
pub struct Pass {
    pub pass_id: Option<usize>
}

impl Pass {
    pub fn begin_pass(&mut self, cx: &mut Cx) {
        
        let window_id = *cx.window_stack.last().expect("No window found when begin_pass");
        
        let cxwindow = &mut cx.windows[window_id];
        
        if self.pass_id.is_none() { // we need to allocate a CxPass
            if cx.passes_free.len() != 0 {
                self.pass_id = Some(cx.passes_free.pop().unwrap());
            }
            else {
                self.pass_id = Some(cx.passes.len());
                cx.passes.push(CxPass {..Default::default()});
            }
        }
        let pass_id = self.pass_id.unwrap();
        
        if cxwindow.main_pass_id.is_none() { // we are the main pass of a window
            let cxpass = &mut cx.passes[pass_id];
            cxwindow.main_pass_id = Some(pass_id);
            cxpass.dep_of = CxPassDepOf::Window(window_id);
            cxpass.pass_size = cxwindow.get_inner_size();
        }
        else if let Some(dep_of_pass_id) = cx.pass_stack.last() {
            cx.passes[pass_id].dep_of = CxPassDepOf::Pass(*dep_of_pass_id);
            cx.passes[pass_id].pass_size = cx.passes[*dep_of_pass_id].pass_size
        }
        else {
            cx.passes[pass_id].dep_of = CxPassDepOf::None;
        }
        
        let cxpass = &mut cx.passes[pass_id];
        cxpass.main_view_id = None;
        cxpass.color_textures.truncate(0);
        cx.pass_stack.push(pass_id);
        
        //let pass_size = cxpass.pass_size;
        //self.set_ortho_matrix(cx, Vec2::zero(), pass_size);
    }
    
    pub fn make_dep_of_pass(&mut self, cx: &mut Cx, pass: &Pass) {
        let cxpass = &mut cx.passes[self.pass_id.unwrap()];
        if let Some(pass_id) = pass.pass_id {
            cxpass.dep_of = CxPassDepOf::Pass(pass_id)
        }
        else {
            cxpass.dep_of = CxPassDepOf::None
        }
    }
    
    pub fn set_size(&mut self, cx: &mut Cx, pass_size: Vec2) {
        let cxpass = &mut cx.passes[self.pass_id.unwrap()];
        cxpass.pass_size = pass_size;
    }
    
    pub fn add_color_texture(&mut self, cx: &mut Cx, texture: &mut Texture, clear_color: Option<Color>) {
        texture.set_desc(cx, None);
        let pass_id = self.pass_id.expect("Please call add_color_texture after begin_pass");
        let cxpass = &mut cx.passes[pass_id];
        cxpass.color_textures.push(CxPassColorTexture {
            texture_id: texture.texture_id.unwrap(),
            clear_color: clear_color
        })
    }
    
    pub fn set_depth_texture(&mut self, cx: &mut Cx, texture: &mut Texture) {
        texture.set_desc(cx, None);
        let pass_id = self.pass_id.expect("Please call set_depth_texture after begin_pass");
        let cxpass = &mut cx.passes[pass_id];
        cxpass.depth_texture = texture.texture_id;
    }
    
    
    pub fn end_pass(&mut self, cx: &mut Cx) {
        cx.pass_stack.pop();
    }
    
    pub fn redraw_pass_area(&mut self, cx: &mut Cx) {
        if let Some(pass_id) = self.pass_id {
            cx.redraw_pass_and_sub_passes(pass_id);
        }
    }
    
}

#[derive(Default, Clone, Debug)]
pub struct CxPassColorTexture {
    pub clear_color: Option<Color>,
    pub texture_id: usize
}

#[derive(Clone, Debug)]
pub struct CxPass {
    pub color_textures: Vec<CxPassColorTexture>,
    pub depth_texture: Option<usize>,
    pub main_view_id: Option<usize>,
    pub dep_of: CxPassDepOf,
    pub paint_dirty: bool,
    pub pass_size: Vec2,
    pub uniforms: Vec<f32>,
    pub platform: CxPlatformPass,
}

impl Default for CxPass {
    fn default() -> Self {
        let mut uniforms: Vec<f32> = Vec::new();
        uniforms.resize(CX_UNI_SIZE, 0.0);
        CxPass {
            uniforms: uniforms,
            color_textures: Vec::new(),
            depth_texture: None,
            main_view_id: None,
            dep_of: CxPassDepOf::None,
            paint_dirty: true,
            pass_size: Vec2::zero(),
            platform: CxPlatformPass::default()
        }
    }
}

#[derive(Clone, Debug)]
pub enum CxPassDepOf {
    Window(usize),
    Pass(usize),
    None
}

const CX_UNI_CAMERA_PROJECTION: usize = 0;
const CX_UNI_CAMERA_VIEW: usize = 16;
const CX_UNI_DPI_FACTOR: usize = 32;
const CX_UNI_DPI_DILATE: usize = 33;
const CX_UNI_SIZE: usize = 36;

impl CxPass {
    pub fn def_uniforms(sg: ShaderGen)->ShaderGen{
        sg.compose(shader_ast!({
            let camera_projection: mat4<UniformCx>;
            let camera_view: mat4<UniformCx>;
            let dpi_factor: float<UniformCx>;
            let dpi_dilate: float<UniformCx>;
        }))
    }
    
    pub fn uniform_camera_projection(&mut self, v: &Mat4) {
        //dump in uniforms
        for i in 0..16 {
            self.uniforms[CX_UNI_CAMERA_PROJECTION + i] = v.v[i];
        }
    }
    
    pub fn uniform_camera_view(&mut self, v: &Mat4) {
        //dump in uniforms
        for i in 0..16 {
            self.uniforms[CX_UNI_CAMERA_VIEW + i] = v.v[i];
        }
    }
    
    pub fn set_dpi_factor(&mut self, dpi_factor: f32) {
        let dpi_dilate = (2. - dpi_factor).max(0.).min(1.);
        self.uniforms[CX_UNI_DPI_FACTOR + 0] = dpi_factor;
        self.uniforms[CX_UNI_DPI_DILATE + 0] = dpi_dilate;
    }
    
    pub fn set_ortho_matrix(&mut self, offset: Vec2, size: Vec2) {
        let ortho_matrix = Mat4::ortho(
            offset.x,
            offset.x + size.x,
            offset.y,
            offset.y + size.y,
            -100.0,
            100.0,
            1.0,
            1.0
        );
        self.uniform_camera_projection(&ortho_matrix);
        //self.set_matrix(cx, &ortho_matrix);
    }
    
    //pub fn set_matrix(&mut self, cx: &mut Cx, matrix: &Mat4) {
        //let pass_id = self.pass_id.expect("Please call set_ortho_matrix after begin_pass");
        //let cxpass = &mut cx.passes[pass_id];
   // }
}