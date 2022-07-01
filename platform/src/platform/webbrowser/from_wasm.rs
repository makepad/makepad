use crate::{
    makepad_wasm_msg::*,
    makepad_live_id::{LiveId,id},
    cursor::MouseCursor,
};

#[derive(FromWasm)]
struct FromWasmShaderPropDef {
    ty: String,
    name: String
}

#[derive(FromWasm)]
struct FromWasmCompileWebGLShader {
    shader_id: usize,
    fragment: String,
    vertex: String,
    geometry_slots: usize,
    instance_slots: usize,
    pass_uniforms: Vec<FromWasmShaderPropDef>,
    view_uniforms: Vec<FromWasmShaderPropDef>,
    draw_uniforms: Vec<FromWasmShaderPropDef>,
    user_uniforms: Vec<FromWasmShaderPropDef>,
    live_uniforms: Vec<FromWasmShaderPropDef>,
    textures: Vec<FromWasmShaderPropDef>
}

#[derive(FromWasm)]
struct FromWasmAllocArrayBuffer {
    buffer_id: usize,
    len: usize,
    data: WasmPtrF32,
}

#[derive(FromWasm)]
struct FromWasmAllocIndexBuffer {
    buffer_id: usize,
    len: usize,
    data: WasmPtrU32,
}

#[derive(FromWasm)]
struct FromWasmAllocVao {
    vao_id: usize,
    shader_id: usize,
    geom_ib_id: usize,
    geom_vb_id: usize,
    inst_vb_id: usize,
}

#[derive(FromWasm)]
struct FromWasmDrawCall {
    vao_id: usize,
    shader_id: usize,
    pass_uniforms: WasmPtrF32,
    view_uniforms: WasmPtrF32,
    draw_uniforms: WasmPtrF32,
    user_uniforms: WasmPtrF32,
    live_uniforms: WasmPtrF32,
    textures: WasmPtrU32,
    const_table: WasmPtrU32
}

#[derive(FromWasm)]
struct FromWasmColor {
    r: f32,
    g: f32,
    b: f32,
    a: f32
}

#[derive(FromWasm)]
struct FromWasmClear {
    color: FromWasmColor
}

#[derive(FromWasm)]
struct FromWasmLoadDeps {
    deps: Vec<String>
}

#[derive(FromWasm)]
struct FromWasmUpdateTextureImage2D {
    texture_id: usize,
    texture_width: usize,
    texture_height: usize,
    image: WasmPtrU32
}

#[derive(FromWasm)]
struct FromWasmRequestAnimationFrame {
}

#[derive(FromWasm)]
struct FromWasmSetDocumentTitle {
    title: String
}

#[derive(FromWasm)]
struct FromWasmSetMouseCursor {
    web_cursor: u32
}

impl FromWasmSetMouseCursor {
    fn new(mouse_cursor: MouseCursor) -> Self {
        Self {
            web_cursor: match mouse_cursor {
                MouseCursor::Hidden => 0,
                MouseCursor::Default => 1,
                MouseCursor::Crosshair => 2,
                MouseCursor::Hand => 3,
                MouseCursor::Arrow => 4,
                MouseCursor::Move => 5,
                MouseCursor::Text => 6,
                MouseCursor::Wait => 7,
                MouseCursor::Help => 8,
                MouseCursor::NotAllowed => 9,
                MouseCursor::NResize => 10,
                MouseCursor::NeResize => 11,
                MouseCursor::EResize => 12,
                MouseCursor::SeResize => 13,
                MouseCursor::SResize => 14,
                MouseCursor::SwResize => 15,
                MouseCursor::WResize => 16,
                MouseCursor::NwResize => 17,
                
                MouseCursor::NsResize => 18,
                MouseCursor::NeswResize => 19,
                MouseCursor::EwResize => 20,
                MouseCursor::NwseResize => 21,
                MouseCursor::ColResize => 22,
                MouseCursor::RowResize => 23,
            }
        }
    }
}

#[derive(FromWasm)]
struct FromWasmReadFile {
    id: u32,
    path: String
}

#[derive(FromWasm)]
struct FromWasmShowTextIME {
    x: f32,
    y: f32
}

#[derive(FromWasm)]
struct FromWasmHideTextIME {
}

#[derive(FromWasm)]
struct FromWasmTextCopyResponse {
    response: String
}

#[derive(FromWasm)]
struct FromWasmStartTimer {
    repeats: bool,
    id: usize,
    interval: f64
}

#[derive(FromWasm)]
struct FromWasmStopTimer {
    id: usize,
}

#[derive(FromWasm)]
struct FromWasmXrStartPresenting {
}

#[derive(FromWasm)]
struct FromWasmXrStopPresenting {
}

#[derive(FromWasm)]
struct FromWasmBeginRenderTargets {
    pass_id: usize,
    width: usize,
    height: usize
}


#[derive(FromWasm)]
struct FromWasmAddColorTarget {
    texture_id: usize,
    init_only: bool,
    color: FromWasmColor
}

#[derive(FromWasm)]
struct FromWasmSetDepthTarget {
    texture_id: usize,
    init_only: bool,
    depth: f32
}

#[derive(FromWasm)]
struct FromWasmEndRenderTargets {
}

#[derive(FromWasm)]
struct FromWasmBeginMainCanvas {
    color: FromWasmColor,
    depth: f32
}

#[derive(FromWasm)]
struct FromWasmFullScreen {
}

#[derive(FromWasm)]
struct FromWasmNormalScreen {
}
