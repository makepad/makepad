use crate::{
    makepad_wasm_bridge::*,
    makepad_math::Vec4,
    cursor::MouseCursor,
    draw_shader::DrawShaderTextureInput,
    draw_vars::{
        DRAW_CALL_TEXTURE_SLOTS
    },
};




// WebBrowser API





#[derive(FromWasm)]
pub struct FromWasmLoadDeps {
    pub deps: Vec<String>
}


#[derive(FromWasm)]
pub struct FromWasmStartTimer {
    pub repeats: bool,
    pub timer_id: f64,
    pub interval: f64
}


#[derive(FromWasm)]
pub struct FromWasmStopTimer {
    pub id: f64,
}

#[derive(FromWasm)]
pub struct FromWasmFullScreen {
}

#[derive(FromWasm)]
pub struct FromWasmNormalScreen {
}

#[derive(FromWasm)]
pub struct FromWasmRequestAnimationFrame {
}

#[derive(FromWasm)]
pub struct FromWasmSetDocumentTitle {
    pub title: String
}

#[derive(FromWasm)]
pub struct FromWasmSetMouseCursor {
    pub web_cursor: u32
}

impl FromWasmSetMouseCursor {
    pub fn new(cursor: MouseCursor) -> Self {
        Self {
            web_cursor: match cursor {
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
pub struct FromWasmTextCopyResponse {
    pub response: String
}

#[derive(FromWasm)]
pub struct FromWasmShowTextIME {
    pub x: f64,
    pub y: f64
}

#[derive(FromWasm)]
pub struct FromWasmHideTextIME {
}
/*
#[derive(FromWasm)]
pub struct FromWasmWebSocketOpen {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub url: String,
    pub method: String,
    pub headers: String,
    pub body: WasmDataU8,
}

#[derive(FromWasm)]
pub struct FromWasmWebSocketSendBinary{
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub data: WasmDataU8
}

#[derive(FromWasm)]
pub struct FromWasmWebSocketSendString{
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub data: String
}
*/
#[derive(FromWasm)]
pub struct WTextureInput {
    pub ty: String,
    pub name: String
}

impl DrawShaderTextureInput{
    pub fn to_from_wasm_texture_input(&self)->WTextureInput{
        WTextureInput{
            ty: self.ty.to_string(),
            name: self.id.to_string()
        }
    }
}

#[derive(FromWasm)]
pub struct FromWasmCreateThread {
    pub context_ptr: u32,
}

#[derive(FromWasm)]
pub struct FromWasmHTTPRequest {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub metadata_id_lo: u32,
    pub metadata_id_hi: u32,
    pub url: String,
    pub method: String,
    pub headers: String,
    pub body: WasmDataU8,
}



// WebGL API

#[derive(FromWasm)]
pub struct FromWasmCompileWebGLShader {
    pub shader_id: usize,
    pub vertex: String,
    pub pixel: String,
    pub geometry_slots: usize,
    pub instance_slots: usize,
    pub textures: Vec<WTextureInput>
}

#[derive(FromWasm)]
pub struct FromWasmAllocArrayBuffer {
    pub buffer_id: usize,
    pub data: WasmDataF32,
}

#[derive(FromWasm)]
pub struct FromWasmAllocIndexBuffer {
    pub buffer_id: usize,
    pub data: WasmDataU32,
}

#[derive(FromWasm)]
pub struct FromWasmAllocVao {
    pub vao_id: usize,
    pub shader_id: usize,
    pub geom_ib_id: usize,
    pub geom_vb_id: usize,
    pub inst_vb_id: usize,
}


#[derive(FromWasm, Default)]
pub struct WColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32
}

impl Into<WColor> for Vec4{
    fn into(self)->WColor{WColor{r:self.x, g:self.y, b:self.z,a:self.w}}
}

#[derive(FromWasm)]
pub struct FromWasmAllocTextureImage2D {
    pub texture_id: usize,
    pub width: usize,
    pub height: usize,
    pub data: WasmDataU32
}

#[derive(FromWasm, Default)]
pub struct WColorTarget {
    pub texture_id: usize,
    pub init_only: bool,
    pub clear_color: WColor
}

#[derive(FromWasm, Default)]
pub struct WDepthTarget {
    pub texture_id: usize,
    pub init_only: bool,
    pub clear_depth: f32
}

#[derive(FromWasm)]
pub struct FromWasmBeginRenderTexture {
    pub pass_id: usize,
    pub width: usize,
    pub height: usize,
    pub color_targets: [WColorTarget;1],
    pub depth_target: WDepthTarget
}

#[derive(FromWasm)]
pub struct FromWasmBeginRenderCanvas {
    pub clear_color: WColor,
    pub clear_depth: f32
}

#[derive(FromWasm)]
pub struct FromWasmSetDefaultDepthAndBlendMode {
}

#[derive(FromWasm)]
pub struct FromWasmDrawCall {
    pub vao_id: usize,
    pub shader_id: usize,
    pub pass_uniforms: WasmDataF32,
    pub view_uniforms: WasmDataF32,
    pub draw_uniforms: WasmDataF32,
    pub user_uniforms: WasmDataF32,
    pub live_uniforms: WasmDataF32,
    pub const_table: WasmDataF32,
    pub textures: [Option<usize>; DRAW_CALL_TEXTURE_SLOTS],
}

#[derive(FromWasm)]
pub struct FromWasmXrStartPresenting {
}

#[derive(FromWasm)]
pub struct FromWasmXrStopPresenting {
}

// WebAudio API
#[derive(FromWasm)]
pub struct FromWasmQueryMidiPorts {
}

#[derive(FromWasm)]
pub struct FromWasmUseMidiInputs {
    pub input_uids: Vec<String>,
}

#[derive(FromWasm)]
pub struct FromWasmSendMidiOutput {
    pub uid: String,
    pub data: u32
}

#[derive(FromWasm)]
pub struct FromWasmQueryAudioDevices {}

#[derive(FromWasm)]
pub struct FromWasmStartAudioOutput {
    pub web_device_id: String,
    pub context_ptr: u32,
}

#[derive(FromWasm)]
pub struct FromWasmStopAudioOutput {
}




