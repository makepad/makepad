use crate::{
    cursor::MouseCursor, draw_shader::{CxDrawShaderMapping, DrawShaderGlslTextureBinding}, draw_vars::DRAW_CALL_TEXTURE_SLOTS,
    makepad_math::Vec4f, makepad_script::shader::{SamplerAddress, SamplerFilter, TextureType}, makepad_wasm_bridge::*,
};

// WebBrowser API

#[derive(FromWasm)]
pub struct FromWasmA11yUpdate {
    pub window: u32,
    pub bytes: WasmDataU8,
}

#[derive(FromWasm)]
pub struct FromWasmA11yActionDone {
    pub req_lo: u32,
    pub req_hi: u32,
    pub result: u32,
}

#[derive(FromWasm)]
pub struct FromWasmStartTimer {
    pub repeats: bool,
    pub timer_id: f64,
    pub interval: f64,
}

#[derive(FromWasm)]
pub struct FromWasmStopTimer {
    pub timer_id: f64,
}

#[derive(FromWasm)]
pub struct FromWasmStartLocationUpdates {}

#[derive(FromWasm)]
pub struct FromWasmStopLocationUpdates {}

#[derive(FromWasm)]
pub struct FromWasmFullScreen {}

#[derive(FromWasm)]
pub struct FromWasmNormalScreen {}

#[derive(FromWasm)]
pub struct FromWasmRequestAnimationFrame {}

/// Asks for an animation frame once `performance.now()` reaches `at`, in seconds.
#[derive(FromWasm)]
pub struct FromWasmRequestAnimationFrameAt {
    pub at: f64,
}

#[derive(FromWasm)]
pub struct FromWasmSetDocumentTitle {
    pub title: String,
}

#[derive(FromWasm)]
pub struct FromWasmSetMouseCursor {
    pub web_cursor: u32,
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
                MouseCursor::Grab => 24,
                MouseCursor::Grabbing => 25,
            },
        }
    }
}

#[derive(FromWasm)]
pub struct FromWasmTextCopyResponse {
    pub response: String,
    pub html: String,
    pub markdown: String,
}

#[derive(FromWasm)]
pub struct FromWasmCopyToClipboard {
    pub text: String,
    pub html: String,
}

/// Measure each frame's GPU time where the browser offers a timer query
/// (EXT_disjoint_timer_query_webgl2), for `gpu_frame_timer`.
#[derive(FromWasm)]
pub struct FromWasmGpuFrameTimer {
    pub on: bool,
}

/// The GPU inventory (`Cx::gpu_inventory`) as JSON, answering
/// `ToWasmGpuInventory`.
#[derive(FromWasm)]
pub struct FromWasmGpuInventory {
    pub request_id: u32,
    pub json: String,
}

/// A runtime-generated module to instantiate against this module's memory
/// and link into its function table (see `wasm_link`).
#[derive(FromWasm)]
pub struct FromWasmLinkModule {
    pub request_id: u32,
    pub bytes: WasmDataU8,
}

/// Table slots of linked modules to empty.
#[derive(FromWasm)]
pub struct FromWasmUnlinkSlots {
    pub slots: Vec<u32>,
}

#[derive(FromWasm)]
pub struct FromWasmStorageGet {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub namespace: String,
    pub key: String,
}

#[derive(FromWasm)]
pub struct FromWasmStorageSet {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub namespace: String,
    pub key: String,
    pub value: WasmDataU8,
}

#[derive(FromWasm)]
pub struct FromWasmStorageDelete {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub namespace: String,
    pub key: String,
}

#[derive(FromWasm)]
pub struct FromWasmStorageList {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub namespace: String,
    pub prefix: String,
    pub after: String,
    pub has_after: bool,
    pub limit: u32,
}

#[derive(FromWasm)]
pub struct FromWasmStorageGetRange {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub namespace: String,
    pub key: String,
    pub offset_lo: u32,
    pub offset_hi: u32,
    pub len: u32,
}

#[derive(FromWasm)]
pub struct FromWasmStorageStat {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub namespace: String,
    pub key: String,
}

#[derive(FromWasm)]
pub struct FromWasmStorageEstimate {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
    pub namespace: String,
}

#[derive(FromWasm)]
pub struct FromWasmOpenUrl {
    pub url: String,
    pub in_place: bool,
}

#[derive(FromWasm)]
pub struct FromWasmBrowserUpdateUrl {
    pub url: String,
    pub replace: bool,
}

#[derive(FromWasm)]
pub struct FromWasmBrowserHistoryGo {
    pub delta: f64,
}

#[derive(FromWasm)]
pub struct FromWasmShowTextIME {
    pub x: f64,
    pub y: f64,
}

#[derive(FromWasm)]
pub struct FromWasmHideTextIME {}

#[derive(FromWasm)]
pub struct FromWasmSetVirtualFileLimits {
    pub max_file_size: f64,
    pub max_total_size: f64,
}

#[derive(FromWasm)]
pub struct FromWasmSelectFileDialog {
    pub id_lo: u32,
    pub id_hi: u32,
    pub accept: String,
    pub multiple: bool,
    pub max_file_size: f64,
    pub max_total_size: f64,
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
#[derive(Clone, PartialEq, Eq, FromWasm)]
pub struct WTextureInput {
    pub ty: String,
    pub name: String,
    pub slot: u32,
    pub stages: u32,
    pub filter: u32,
    pub address: u32,
    pub compare: bool,
    pub anisotropy: u32,
}

impl WTextureInput {
    pub fn from_binding(mapping: &CxDrawShaderMapping, b: &DrawShaderGlslTextureBinding) -> Result<Self, &'static str> {
        let texture = mapping.textures.get(b.texture_slot as usize).ok_or("invalid WebGL texture slot")?;
        let sampler = mapping.samplers.get(b.sampler_index as usize).ok_or("invalid WebGL sampler index")?;
        let ty = match texture.tex_type {
            TextureType::Texture1d | TextureType::Texture2d | TextureType::TextureVideo => "sampler2D",
            TextureType::TextureDepth => "sampler2DShadow",
            TextureType::TextureCube => "samplerCube",
            TextureType::TextureCubeArray => return Err("WebGL2 does not support cube array textures"),
            TextureType::Texture1dArray | TextureType::Texture2dArray | TextureType::TextureDepthArray => "sampler2DArray",
            TextureType::Texture3d | TextureType::Texture3dArray => "sampler3D",
        };
        Ok(Self {
            ty: ty.to_owned(),
            name: b.uniform_name.clone(),
            slot: b.texture_slot,
            stages: u32::from(b.stages),
            filter: u32::from(sampler.filter == SamplerFilter::Linear),
            address: match sampler.address {
                SamplerAddress::Repeat => 0,
                SamplerAddress::ClampToEdge => 1,
                SamplerAddress::ClampToZero => 2,
                SamplerAddress::MirroredRepeat => 3,
            },
            compare: texture.tex_type == TextureType::TextureDepth,
            anisotropy: u32::from(sampler.anisotropy),
        })
    }
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

#[derive(FromWasm)]
pub struct FromWasmCancelHTTPRequest {
    pub request_id_lo: u32,
    pub request_id_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmCheckPermission {
    pub permission: String,
    pub request_id: u32,
}

#[derive(FromWasm)]
pub struct FromWasmRequestPermission {
    pub permission: String,
    pub request_id: u32,
}

// WebGL API

#[derive(FromWasm)]
pub struct WVertexAttrib {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub stride: u32,
    pub gl_type: u32,
    pub normalized: u32,
    pub integer: u32,
}

/// Each program's first draw in a frame of its own (`Cx::spread_first_draws`).
#[derive(FromWasm)]
pub struct FromWasmSpreadFirstDraws {
    pub on: bool,
}

#[derive(FromWasm)]
pub struct FromWasmCompileWebGLShader {
    pub shader_id: usize,
    pub shader_name: String,
    /// Same format codes as WColorTarget, for capability checks at compilation.
    pub color_format: u32,
    pub blend_add: bool,
    pub outputs: u32,
    pub vertex: String,
    pub pixel: String,
    pub geometry_slots: usize,
    pub instance_slots: usize,
    pub textures: Vec<WTextureInput>,
    pub geom_attribs: Vec<WVertexAttrib>,
    pub inst_attribs: Vec<WVertexAttrib>,
}

#[derive(FromWasm)]
pub struct FromWasmAllocArrayBuffer {
    pub buffer_id: usize,
    pub data: WasmPtrF32,
    pub byte_data: WasmPtrU8,
}

#[derive(FromWasm)]
pub struct FromWasmCreateInstanceSlab {
    pub slab_slot: u32,
    pub backing_generation: u32,
    // Power-of-two rounding can take a wasm32 byte count to 4 GiB.
    pub capacity_bytes: f64,
}

#[derive(FromWasm, ToWasm)]
pub struct WInstanceRegion {
    pub index: u32,
    pub gen: u32,
}

#[derive(FromWasm)]
pub struct FromWasmWriteInstanceRegion {
    pub slab_slot: u32,
    pub backing_generation: u32,
    pub region: WInstanceRegion,
    pub byte_off: usize,
    pub byte_len: usize,
    pub parts: Vec<WasmPtrU8>,
}

#[derive(FromWasm)]
pub struct WCustomUniformBuffer {
    pub block_name: String,
    pub data: WasmPtrU8,
    pub generation_lo: u32,
    pub generation_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmAllocIndexBuffer {
    pub buffer_id: usize,
    pub data: WasmPtrU32,
    pub byte_data: WasmPtrU8,
    pub index_width: u32,
}

#[derive(FromWasm)]
pub struct FromWasmAllocVao {
    pub vao_id: usize,
    pub shader_id: usize,
    pub geom_ib_id: usize,
    pub geom_vb_id: usize,
    pub slab_slot: u32,
    pub backing_generation: u32,
    pub region: WInstanceRegion,
    pub byte_off: usize,
    pub byte_len: usize,
    pub instance_byte_offset: usize,
}

#[derive(FromWasm)]
pub struct WInstanceSlab {
    pub slab_slot: u32,
    pub backing_generation: u32,
}

/// Deletes WebGL objects whose owning Rust pool slots are genuinely free.
/// Each list contains the exact JavaScript table ids; the consumer also
/// invalidates VAOs which depend on a retired vertex/index buffer.
#[derive(FromWasm)]
pub struct FromWasmFreeWebGLResources {
    pub instance_slabs: Vec<WInstanceSlab>,
    pub array_buffer_ids: Vec<usize>,
    pub index_buffer_ids: Vec<usize>,
    pub vao_ids: Vec<usize>,
    pub texture_ids: Vec<usize>,
    pub framebuffer_ids: Vec<usize>,
}

#[derive(FromWasm)]
pub struct FromWasmPollGpuCompletion {
    pub serial_lo: u32,
    pub serial_hi: u32,
}

#[derive(FromWasm, Default)]
pub struct WColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Into<WColor> for Vec4f {
    fn into(self) -> WColor {
        WColor {
            r: self.x,
            g: self.y,
            b: self.z,
            a: self.w,
        }
    }
}

#[allow(non_camel_case_types)]
#[derive(FromWasm)]
pub struct FromWasmAllocTextureImage2D_BGRAu8_32 {
    pub texture_id: usize,
    pub width: usize,
    pub height: usize,
    pub data: WasmPtrU32,
}

#[allow(non_camel_case_types)]
#[derive(FromWasm)]
pub struct FromWasmUpdateTextureRect_BGRAu8_32 {
    pub texture_id: usize,
    pub image_width: usize,
    pub image_height: usize,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub data: WasmPtrU32,
}

#[allow(non_camel_case_types)]
#[derive(FromWasm)]
pub struct FromWasmAllocTextureImage2D_Ru8 {
    pub texture_id: usize,
    pub width: usize,
    pub height: usize,
    pub data: WasmPtrU8,
}

#[allow(non_camel_case_types)]
#[derive(FromWasm)]
pub struct FromWasmAllocTextureImage2D_RGBAf32 {
    pub texture_id: usize,
    pub width: usize,
    pub height: usize,
    pub data: WasmPtrF32,
}

#[allow(non_camel_case_types)]
#[derive(FromWasm)]
pub struct FromWasmAllocTextureCube_BGRAu8_32 {
    pub texture_id: usize,
    pub width: usize,
    pub height: usize,
    pub data: WasmPtrU32,
}

/// A 2D array's level 0: `layers` images of width x height, layer-major.
#[allow(non_camel_case_types)]
#[derive(FromWasm)]
pub struct FromWasmAllocTextureArray_BGRAu8_32 {
    pub texture_id: usize,
    pub width: usize,
    pub height: usize,
    pub layers: usize,
    pub data: WasmPtrU32,
}

#[derive(FromWasm, Default)]
pub struct WColorTarget {
    pub texture_id: usize,
    pub init_only: bool,
    pub clear_color: WColor,
    /// Attachment format: 0 RGBA8, 1 R32F, 2 RGBA32F, 3 RGBA16F.
    /// Float color attachments require EXT_color_buffer_float.
    pub format: u32,
}

#[derive(FromWasm, Default)]
pub struct WDepthTarget {
    /// false = the pass has no depth texture (`Default`); a texture id of 0 is
    /// a real texture, so absence needs its own flag.
    pub attached: bool,
    pub texture_id: usize,
    pub init_only: bool,
    pub clear_depth: f32,
}

#[derive(FromWasm)]
pub struct FromWasmBeginRenderTexture {
    pub pass_id: usize,
    pub width: usize,
    pub height: usize,
    /// One per color attachment (several: MRT, written through drawBuffers).
    pub color_targets: Vec<WColorTarget>,
    pub depth_target: WDepthTarget,
}

/// Starts an asynchronous WebGL2 readback of a render-target texture.
/// JavaScript uses a pixel-pack buffer plus a fence and reports the bytes
/// through `ToWasmRenderTextureCapture` on a later animation frame.
#[derive(FromWasm)]
pub struct FromWasmRequestRenderTextureCapture {
    pub texture_id: usize,
    pub register_only: bool,
    pub ticket_lo: u32,
    pub ticket_hi: u32,
    pub width: usize,
    pub height: usize,
}

#[derive(FromWasm)]
pub struct FromWasmBeginRenderCanvas {
    pub clear_color: WColor,
    pub clear_depth: f32,
}

#[derive(FromWasm)]
pub struct FromWasmSetDefaultDepthAndBlendMode {}

#[derive(FromWasm)]
pub struct WInstanceRange {
    pub first: u32,
    pub count: u32,
}

impl From<[u32; 2]> for WInstanceRange {
    fn from([first, count]: [u32; 2]) -> Self {
        Self { first, count }
    }
}

#[derive(FromWasm, Clone, Copy, Debug, Default)]
pub struct WUniformRange {
    pub off: u32,
    pub len: u32,
}

impl From<crate::uniform_stream::UniformRange> for WUniformRange {
    fn from(range: crate::uniform_stream::UniformRange) -> Self {
        Self { off: range.off, len: range.len }
    }
}

#[derive(FromWasm)]
pub struct FromWasmUploadUniformStream {
    pub data: WasmPtrU8,
}

#[derive(FromWasm)]
pub struct FromWasmDrawCall {
    pub custom_uniforms: Vec<WCustomUniformBuffer>,
    pub instance_ranges: Vec<WInstanceRange>,
    pub vao_id: usize,
    pub shader_id: usize,
    pub index_width: u32,
    pub depth_write: bool,
    pub depth_test_greater: bool,
    pub alpha_blend: bool,
    /// The shader's `blend_op: @Max`: blend by per-channel max.
    pub blend_max: bool,
    /// The shader's `blend_op: @Add`: ONE/ONE/ADD, alpha included.
    pub blend_add: bool,
    /// 0 = None, 1 = Back, 2 = Front (matches `CullMode as u32`).
    pub cull_mode: u32,
    pub pass_uniforms: WUniformRange,
    pub draw_list_uniforms: WUniformRange,
    pub draw_call_uniforms: WUniformRange,
    pub user_uniforms: WUniformRange,
    pub live_uniforms: WUniformRange,
    pub textures: [Option<usize>; DRAW_CALL_TEXTURE_SLOTS],
}

#[derive(FromWasm)]
pub struct FromWasmXrStartPresenting {}

#[derive(FromWasm)]
pub struct FromWasmXrStopPresenting {}

// WebAudio API
#[derive(FromWasm)]
pub struct FromWasmQueryMidiPorts {}

#[derive(FromWasm)]
pub struct FromWasmUseMidiInputs {
    pub input_uids: Vec<String>,
}

#[derive(FromWasm)]
pub struct FromWasmSendMidiOutput {
    pub uid: String,
    pub data: u32,
}

#[derive(FromWasm)]
pub struct FromWasmQueryAudioDevices {}

#[derive(FromWasm)]
pub struct FromWasmStartAudioOutput {
    pub web_device_id: String,
    pub context_ptr: u32,
}

#[derive(FromWasm)]
pub struct FromWasmStopAudioOutput {}

/// A page opened with `?test=1`: an app's media clock for a test harness
/// (`Cx::web_test_clock`). `film`: the time the picture shows; `rendered`:
/// the media time at the end of the audio rendered so far; `speed`: media
/// seconds per second there.
#[derive(FromWasm)]
pub struct FromWasmTestClock {
    pub film: f64,
    pub rendered: f64,
    pub speed: f64,
}

// Video Playback API

#[derive(FromWasm)]
pub struct FromWasmPrepareVideoPlayback {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
    pub texture_id: usize,
    pub source_url: String,
    pub autoplay: bool,
    pub should_loop: bool,
}

#[derive(FromWasm)]
pub struct FromWasmBeginVideoPlayback {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmPauseVideoPlayback {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmResumeVideoPlayback {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmMuteVideoPlayback {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmUnmuteVideoPlayback {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmSeekVideoPlayback {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
    pub position_ms_lo: u32,
    pub position_ms_hi: u32,
}

#[derive(FromWasm)]
pub struct FromWasmCleanupVideoPlaybackResources {
    pub video_id_lo: u32,
    pub video_id_hi: u32,
}
