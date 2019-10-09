use crate::cx::*;

#[derive(Default, Clone, Debug)]
pub struct Pass {
    pub pass_id: Option<usize>
}


impl Pass {
    pub fn begin_pass(&mut self, cx: &mut Cx) {
        
        if self.pass_id.is_none() { // we need to allocate a CxPass
            self.pass_id = Some(if cx.passes_free.len() != 0 {
                cx.passes_free.pop().unwrap()
            } else {
                cx.passes.push(CxPass::default());
                cx.passes.len() - 1
            });
        }
        let pass_id = self.pass_id.unwrap();
        
        if let Some(window_id) = cx.window_stack.last() {
            if cx.windows[*window_id].main_pass_id.is_none() { // we are the main pass of a window
                let cxpass = &mut cx.passes[pass_id];
                cx.windows[*window_id].main_pass_id = Some(pass_id);
                cxpass.dep_of = CxPassDepOf::Window(*window_id);
                cxpass.pass_size = cx.windows[*window_id].get_inner_size();
                cx.current_dpi_factor = cx.get_delegated_dpi_factor(pass_id);
            }
            else if let Some(dep_of_pass_id) = cx.pass_stack.last() { 
                let dep_of_pass_id = *dep_of_pass_id;
                cx.passes[pass_id].dep_of = CxPassDepOf::Pass(dep_of_pass_id);
                cx.passes[pass_id].pass_size = cx.passes[dep_of_pass_id].pass_size;
                cx.current_dpi_factor = cx.get_delegated_dpi_factor(dep_of_pass_id);
            }
            else {
                cx.passes[pass_id].dep_of = CxPassDepOf::None;
                cx.passes[pass_id].override_dpi_factor = Some(1.0);
                cx.current_dpi_factor = 1.0;
            }
        }
        else {
            cx.passes[pass_id].dep_of = CxPassDepOf::None;
            cx.passes[pass_id].override_dpi_factor = Some(1.0);
            cx.current_dpi_factor = 1.0;
        }
        
        let cxpass = &mut cx.passes[pass_id];
        cxpass.main_view_id = None;
        cxpass.color_textures.truncate(0);
        cx.pass_stack.push(pass_id);
        
        //let pass_size = cxpass.pass_size;
        //self.set_ortho_matrix(cx, Vec2::zero(), pass_size);
    }
    
    pub fn override_dpi_factor(&mut self, cx: &mut Cx, dpi_factor:f32){
        if let Some(pass_id) = self.pass_id {
            cx.passes[pass_id].override_dpi_factor = Some(dpi_factor);
            cx.current_dpi_factor = dpi_factor;
        }
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
    
    pub fn add_color_texture(&mut self, cx: &mut Cx, texture: &mut Texture, clear_color: ClearColor) {
        texture.set_desc(cx, None);
        let pass_id = self.pass_id.expect("Please call add_color_texture after begin_pass");
        let cxpass = &mut cx.passes[pass_id];
        cxpass.color_textures.push(CxPassColorTexture {
            texture_id: texture.texture_id.unwrap(),
            clear_color: clear_color
        })
    }
    
    pub fn set_depth_texture(&mut self, cx: &mut Cx, texture: &mut Texture, clear_depth: ClearDepth) {
        texture.set_desc(cx, None);
        let pass_id = self.pass_id.expect("Please call set_depth_texture after begin_pass");
        let cxpass = &mut cx.passes[pass_id];
        cxpass.depth_texture = texture.texture_id;
        cxpass.clear_depth = clear_depth;
    }
    
    
    pub fn end_pass(&mut self, cx: &mut Cx) {
        cx.pass_stack.pop();
        if cx.pass_stack.len()>0{
            cx.current_dpi_factor = cx.get_delegated_dpi_factor(*cx.pass_stack.last().unwrap());
        }
    }
    
    pub fn redraw_pass_area(&mut self, cx: &mut Cx) {
        if let Some(pass_id) = self.pass_id {
            cx.redraw_pass_and_sub_passes(pass_id);
        }
    }
    
}

#[derive(Clone, Debug)]
pub enum ClearColor {
    InitWith(Color),
    ClearWith(Color)
}

impl Default for ClearColor {
    fn default() -> Self {
        ClearColor::ClearWith(Color::zero())
    }
}

#[derive(Clone, Debug)]
pub enum ClearDepth {
    InitWith(f64),
    ClearWith(f64)
}

#[derive(Default, Clone, Debug)]
pub struct CxPassColorTexture {
    pub clear_color: ClearColor,
    pub texture_id: usize
}

#[derive(Clone, Debug)]
pub struct CxPass {
    pub color_textures: Vec<CxPassColorTexture>,
    pub depth_texture: Option<usize>,
    pub clear_depth: ClearDepth,
    pub depth_init: f64,
    pub override_dpi_factor: Option<f32>,
    pub main_view_id: Option<usize>,
    pub dep_of: CxPassDepOf,
    pub paint_dirty: bool,
    pub pass_size: Vec2,
    pub uniforms: Vec<f32>,
    pub zbias_step: f32,
    pub platform: CxPlatformPass,
}

impl Default for CxPass {
    fn default() -> Self {
        let mut uniforms: Vec<f32> = Vec::new();
        uniforms.resize(CX_UNI_SIZE, 0.0);
        CxPass {
            zbias_step: 0.001,
            uniforms: uniforms,
            color_textures: Vec::new(),
            depth_texture: None,
            override_dpi_factor: None,
            clear_depth: ClearDepth::ClearWith(1.0),
            depth_init: 1.0,
            main_view_id: None,
            dep_of: CxPassDepOf::None,
            paint_dirty: false,
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
    pub fn def_uniforms(sg: ShaderGen) -> ShaderGen {
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
            100.,
            -100.,
            1.0,
            1.0
        );
        
        //println!("{} {}", ortho_matrix.v[10], ortho_matrix.v[14]);
        //println!("CHECK {} {} {:?}", size.x, size.y,ortho_matrix.transform_vec4(Vec4{x:200.,y:300.,z:100.,w:1.0}));
        self.uniform_camera_projection(&ortho_matrix);
        //self.set_matrix(cx, &ortho_matrix);
    }
    
    //pub fn set_matrix(&mut self, cx: &mut Cx, matrix: &Mat4) {
    //let pass_id = self.pass_id.expect("Please call set_ortho_matrix after begin_pass");
    //let cxpass = &mut cx.passes[pass_id];
    // }
}