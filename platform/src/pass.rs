use crate::{
    makepad_live_compiler::{
        LiveType,
        LiveNode,
        LiveModuleId,
        LiveTypeInfo,
        LiveNodeSliceApi
    },
    makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
    makepad_live_id::*,
    makepad_math::*,
    id_pool::*,
    area::Area,
    window::WindowId,
    os::CxOsPass,
    cx::Cx,
    draw_list::DrawListId,
    live_traits::*,
    texture::{
        Texture,
    }
};

#[derive(Debug)]
pub struct Pass(PoolId);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassId(pub (crate) usize);

impl Pass {
}

#[derive(Default)]
pub struct CxPassPool(pub (crate) IdPool<CxPass>);
impl CxPassPool {
    fn alloc(&mut self) -> Pass {
        Pass(self.0.alloc())
    }
    
    pub fn id_iter(&self) -> PassIterator {
        PassIterator {
            cur: 0,
            len: self.0.pool.len()
        }
    }
}

pub struct PassIterator {
    cur: usize,
    len: usize
}

impl Iterator for PassIterator {
    type Item = PassId;
    fn next(&mut self) -> Option<Self::Item> {
        if self.cur >= self.len {
            return None;
        }
        let cur = self.cur;
        self.cur += 1;
        Some(PassId(cur))
    }
}

impl std::ops::Index<PassId> for CxPassPool {
    type Output = CxPass;
    fn index(&self, index: PassId) -> &Self::Output {
        &self.0.pool[index.0].item
    }
}

impl std::ops::IndexMut<PassId> for CxPassPool {
    fn index_mut(&mut self, index: PassId) -> &mut Self::Output {
        &mut self.0.pool[index.0].item
    }
}

impl LiveHook for Pass {}
impl LiveNew for Pass {
    fn live_design_with(_cx:&mut Cx){}
    fn new(cx: &mut Cx) -> Self {
        let pass = cx.passes.alloc();
        pass
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            live_ignore: true,
            //kind: LiveTypeKind::Object,
            type_name: id_lut!(Pass)
        }
    }
}

impl LiveApply for Pass {
    
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, live_id!(View));
            return nodes.skip_node(start_index);
        }
        
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                live_id!(clear_color) => cx.passes[self.pass_id()].clear_color = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes),
                live_id!(dont_clear) => cx.passes[self.pass_id()].dont_clear = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes),
                _ => {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}

impl Pass {
    pub fn id_equals(&self, id:usize)->bool{
        self.0.id == id
    }
    
    pub fn new_with_name(cx: &mut Cx, name:&str) -> Self {
        let pass = cx.passes.alloc();
        pass.set_pass_name(cx, name);
        pass
    }
    
    pub fn pass_id(&self) -> PassId {PassId(self.0.id)}
    
        
    pub fn set_as_xr_pass(&self, cx: &mut Cx) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.parent = CxPassParent::Xr;
    }
    
    pub fn set_pass_parent(&self, cx: &mut Cx, pass: &Pass) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.parent = CxPassParent::Pass(pass.pass_id());
    }
    
    pub fn set_pass_name(&self, cx: &mut Cx, name: &str) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.debug_name = name.to_string();
    }
    
    pub fn pass_name<'a>(&self, cx: &'a mut Cx)->&'a str{
        let cxpass = &mut cx.passes[self.pass_id()];
        &cxpass.debug_name
    }
    
    pub fn set_size(&self, cx: &mut Cx, pass_size: DVec2) {
        let mut pass_size = pass_size;
        if pass_size.x < 1.0 {pass_size.x = 1.0};
        if pass_size.y < 1.0 {pass_size.y = 1.0};
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.pass_rect = Some(CxPassRect::Size(pass_size));
    }

    pub fn size(&self, cx: &mut Cx)->Option<DVec2> {
        let cxpass = &mut cx.passes[self.pass_id()];
        if let Some(CxPassRect::Size(size)) = &cxpass.pass_rect{
            return Some(*size)
        } 
        None
    }
        
    pub fn set_window_clear_color(&self, cx: &mut Cx, clear_color: Vec4) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.clear_color = clear_color;
    }
    
    pub fn clear_color_textures(&self, cx: &mut Cx) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.color_textures.truncate(0);
    }
    
    pub fn add_color_texture(&self, cx: &mut Cx, texture: &Texture, clear_color: PassClearColor) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.color_textures.push(CxPassColorTexture {
            texture: texture.clone(),
            clear_color: clear_color
        })
    }
    
    pub fn set_color_texture(&self, cx: &mut Cx, texture: &Texture, clear_color: PassClearColor) {
        let cxpass = &mut cx.passes[self.pass_id()];
        if cxpass.color_textures.len()!=0{
            cxpass.color_textures[0] = CxPassColorTexture {
                texture: texture.clone(),
                clear_color: clear_color
            }
        }
        else{
            cxpass.color_textures.push(CxPassColorTexture {
                texture: texture.clone(),
                clear_color: clear_color
            })
        }
    }
        
    pub fn set_depth_texture(&self, cx: &mut Cx, texture: &Texture, clear_depth: PassClearDepth) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.depth_texture = Some(texture.clone());
        cxpass.clear_depth = clear_depth;
    }
    
    pub fn set_debug(&mut self, cx: &mut Cx, debug: bool) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.debug = debug;
    }
    
        
    pub fn set_dpi_factor(&mut self, cx: &mut Cx, dpi: f64) {
        let cxpass = &mut cx.passes[self.pass_id()];
        cxpass.dpi_factor = Some(dpi);
    }
    
}

#[derive(Clone)]
pub enum PassClearColor {
    InitWith(Vec4),
    ClearWith(Vec4)
}

impl Default for PassClearColor {
    fn default() -> Self {
        Self::ClearWith(Vec4::default())
    }
}

#[derive(Clone)]
pub enum PassClearDepth {
    InitWith(f32),
    ClearWith(f32)
}

#[derive(Clone)]
pub struct CxPassColorTexture {
    pub clear_color: PassClearColor,
    pub texture: Texture
}

#[derive(Default, Clone)]
#[repr(C)]
pub struct PassUniforms {
    pub camera_projection: Mat4,
    pub camera_projection_r: Mat4,
    pub camera_view: Mat4,
    pub camera_view_r: Mat4,
    pub depth_projection: Mat4,
    pub depth_projection_r: Mat4,
    pub depth_view: Mat4,
    pub depth_view_r: Mat4,
    pub camera_inv: Mat4,
    pub dpi_factor: f32,
    pub dpi_dilate: f32,
    pub time: f32,
    pub pad2: f32
}

impl PassUniforms {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<PassUniforms>() >> 2] {
        unsafe {std::mem::transmute(self)}
    }
}


#[derive(Clone)]
pub enum CxPassRect {
    Area(Area),
    AreaOrigin(Area, DVec2),
    Size(DVec2)
}

#[derive(Clone)]
pub struct CxPass {
    pub debug: bool,
    pub debug_name: String,
    pub color_textures: Vec<CxPassColorTexture>,
    pub depth_texture: Option<Texture>,
    pub clear_depth: PassClearDepth,
    pub dont_clear: bool,
    pub depth_init: f64,
    pub clear_color: Vec4,
    pub dpi_factor: Option<f64>,
    pub main_draw_list_id: Option<DrawListId>,
    pub parent: CxPassParent,
    pub paint_dirty: bool,
    pub pass_rect: Option<CxPassRect>,
    pub view_shift: DVec2,
    pub view_scale: DVec2,
    pub pass_uniforms: PassUniforms,
    pub zbias_step: f32,
    pub os: CxOsPass,
}

impl Default for CxPass {
    fn default() -> Self {
        CxPass {
            debug: false,
            dont_clear: false,
            debug_name: String::new(),
            zbias_step: 0.001,
            pass_uniforms: PassUniforms::default(),
            color_textures: Vec::new(),
            depth_texture: None,
            dpi_factor: None,
            clear_depth: PassClearDepth::ClearWith(1.0),
            clear_color: Vec4::default(),
            depth_init: 1.0,
            main_draw_list_id: None,
            view_shift: dvec2(0.0,0.0),
            view_scale: dvec2(1.0,1.0),
            parent: CxPassParent::None,
            paint_dirty: false,
            pass_rect: None,
            os: CxOsPass::default()
        }
    }
}

#[derive(Clone, Debug)]
pub enum CxPassParent {
    Xr,
    Window(WindowId),
    Pass(PassId),
    None
}

impl CxPass {
    pub fn set_time(&mut self, time: f32) {
        self.pass_uniforms.time = time;
    }
    
    pub fn set_dpi_factor(&mut self, dpi_factor: f64) {
        let dpi_dilate = (2. - dpi_factor).max(0.).min(1.);
        self.pass_uniforms.dpi_factor = dpi_factor as f32;
        self.pass_uniforms.dpi_dilate = dpi_dilate as f32;
    }
    
    pub fn set_ortho_matrix(&mut self, offset: DVec2, size: DVec2) {
        
        let offset = offset + self.view_shift;
        let size = size * self.view_scale;
        
        let ortho = Mat4::ortho(
            offset.x as f32,
            (offset.x + size.x) as f32,
            offset.y as f32,
            (offset.y + size.y) as f32,
            100.,
            -100.,
            1.0,
            1.0
        );
        self.pass_uniforms.camera_projection = ortho;
        self.pass_uniforms.camera_view = Mat4::identity();
    }
}