mod event_loop;
mod jit;
mod raster;
mod shader;
mod virtual_gpu;

use crate::os::cx_stdin::PollTimers;
use crate::{
    audio::{AudioDeviceId, AudioInputFn, AudioOutputFn},
    event::HttpRequest,
    media_api::CxMediaApi,
    midi::{MidiData, MidiInput, MidiOutput, MidiPortId},
    video::{VideoFormatId, VideoInputFn, VideoInputId},
    web_socket::WebSocketMessage,
    thread::MessageThreadPool,
    Cx,
};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

#[derive(Default, Clone)]
pub struct CxOsDrawList {}

#[derive(Default, Clone)]
pub struct CxOsDrawCall {}

#[derive(Default, Clone)]
pub struct CxOsPass {}

#[derive(Default, Clone)]
pub struct CxOsGeometry {}

#[derive(Default, Clone)]
pub struct CxOsTexture {}

#[derive(Default)]
pub struct CxOsDrawShader {
    pub source_hash: u64,
    pub dylib_path: Option<PathBuf>,
    pub load_error: Option<String>,
    pub module: Option<jit::HeadlessLoadedModule>,
    pub shader_version: Option<u32>,
    /// Total number of f32 slots in the varying buffer passed between vertex and fragment shaders.
    pub varying_total_slots: usize,
    /// Number of packed varying slots that come from dyn/rust instances.
    /// These must be treated as flat (non-interpolated) in rasterization.
    pub flat_varying_slots: usize,
    /// True when fragment code uses screen-space derivatives (dFdx/dFdy).
    pub uses_derivatives: bool,
    /// RenderCx layout — queried once from the loaded module.
    pub rcx_size: usize, // total byte size of RenderCx
    pub rcx_vary_offset: usize, // byte offset of varying region (Group 1)
    pub rcx_quad_mode_offset: usize, // byte offset of quad_mode field
    pub rcx_frag_offset: usize, // byte offset of frag_fb0
    pub rcx_discard_offset: usize, // byte offset of discard flag
}

pub struct CxOs {
    pub(crate) stdin_timers: PollTimers,
    pub(crate) start_time: Option<Instant>,
    pub(crate) shader_jit: jit::HeadlessShaderJit,
    pub(crate) frame_dir: Option<PathBuf>,
    pub(crate) render_pool: Option<MessageThreadPool<()>>,
    pub(crate) render_pool_threads: usize,
}

impl Default for CxOs {
    fn default() -> Self {
        Self {
            stdin_timers: Default::default(),
            start_time: None,
            shader_jit: Default::default(),
            frame_dir: None,
            render_pool: None,
            render_pool_threads: 0,
        }
    }
}

#[derive(Default)]
pub struct OsMidiInput {}

impl OsMidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        None
    }
}

#[derive(Default)]
pub struct OsMidiOutput {}

impl OsMidiOutput {
    pub fn send(&self, _port_id: Option<MidiPortId>, _data: MidiData) {}
}

pub struct OsWebSocket;

impl OsWebSocket {
    pub fn send_message(&mut self, _message: WebSocketMessage) -> Result<(), ()> {
        Ok(())
    }

    pub fn close(&mut self) {}

    pub fn open(
        _socket_id: u64,
        _request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> OsWebSocket {
        let _ = rx_sender.send(WebSocketMessage::Opened);
        OsWebSocket
    }
}

impl CxMediaApi for Cx {
    fn midi_input(&mut self) -> MidiInput {
        MidiInput(Some(OsMidiInput::default()))
    }

    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput::default()))
    }

    fn midi_reset(&mut self) {}

    fn use_midi_inputs(&mut self, _ports: &[MidiPortId]) {}

    fn use_midi_outputs(&mut self, _ports: &[MidiPortId]) {}

    fn use_audio_inputs(&mut self, _devices: &[AudioDeviceId]) {}

    fn use_audio_outputs(&mut self, _devices: &[AudioDeviceId]) {}

    fn audio_output_box(&mut self, _index: usize, _f: AudioOutputFn) {}

    fn audio_input_box(&mut self, _index: usize, _f: AudioInputFn) {}

    fn video_input_box(&mut self, _index: usize, _f: VideoInputFn) {}

    fn use_video_input(&mut self, _devices: &[(VideoInputId, VideoFormatId)]) {}
}

impl Cx {
    #[cfg(target_os = "macos")]
    pub fn share_texture_for_presentable_image(&mut self, _texture: &crate::Texture) -> u32 {
        0
    }

    #[cfg(target_os = "windows")]
    pub fn share_texture_for_presentable_image(&mut self, _texture: &crate::Texture) -> u64 {
        0
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn share_texture_for_presentable_image(
        &mut self,
        _texture: &crate::Texture,
    ) -> Option<crate::os::cx_stdin::LinuxOwnedImage> {
        None
    }
}
