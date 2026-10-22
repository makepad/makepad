//! Native HAL client state: command/event buffers and CPU bookkeeping.
//! Native resources live in HAL tables keyed by Cx ids.

mod camera;
mod domain;
pub use domain::HalParentSender;
#[cfg(target_os = "linux")]
mod display;
#[cfg(test)]
mod domain_test;
pub(crate) use domain::lent_id;
pub(crate) use hosted::HostedDone;
mod event_loop;
mod fonts;
#[cfg(not(target_os = "android"))]
pub(crate) mod gl_interop;
mod hosted;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
mod media;
mod ops;
mod query;
mod readback;
mod shader;
mod software_video;

pub use fonts::*;

pub use crate::texture::{
    ReadbackError, ReadbackRequest, ReadbackTicket, TextureReadback, TextureReadbackUsage,
    TEXTURE_READBACK_MAX_BYTES, TEXTURE_READBACK_MAX_REQUESTS,
};

use crate::{
    audio::{AudioDeviceId, AudioInputFn, AudioOutputFn},
    cx_api::OpenUrlInPlace,
    event::DragItem,
    frame_build::FrameBuilder,
    media_api::CxMediaApi,
    midi::{MidiData, MidiInput, MidiOutput, MidiPortId},
    video::{VideoFormatId, VideoInputFn, VideoInputId},
    window::WindowId,
    Cx,
};
use makepad_hal::{EventStore, HalInfo};
use std::collections::HashMap;
use std::sync::Arc;

/// The HAL sessions whose loops this Cx code runs (statics are per process natively and per
/// domain under relax, so a domain's signals wake its own session only).
static WAKE_SESSIONS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

fn wake_sessions() -> std::sync::MutexGuard<'static, Vec<u32>> {
    WAKE_SESSIONS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The session's loop is one this code's signals wake.
pub(crate) fn register_wake_session(session: u32) {
    let mut sessions = wake_sessions();
    if !sessions.contains(&session) {
        sessions.push(session);
    }
}

/// The session's loop has ended.
pub(crate) fn unregister_wake_session(session: u32) {
    wake_sessions().retain(|s| *s != session);
}

/// Any thread (signals, actions, audio and network threads) wakes the loops blocked in
/// `hal_pump`: the sessions this code runs, not every app in the process. Before a session is
/// open there is no pump to wake (the loop services a pending signal at its first turn).
pub(crate) fn wake_ui_event_loop() {
    for session in wake_sessions().iter() {
        makepad_hal::hal_wake_session(*session);
    }
}

/// `FileDialog::kind` of the HAL command (low byte; bit 8: choose several).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    SelectFile = 0,
    SaveFile = 1,
    SelectFolder = 2,
    SaveFolder = 3,
}

/// A dialog in flight: its answer becomes a `FileDialogAction` carrying the app's id.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingDialog {
    pub(crate) kind: DialogKind,
    pub(crate) id: crate::makepad_live_id::LiveId,
}

/// What the HAL told Cx about a window it created.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HalWindow {
    /// the HAL confirmed the window (its first `Geom` arrived); frames go to confirmed windows only
    pub(crate) confirmed: bool,
    /// Native button bounds and level from Chrome, retained for subsequent Geom events.
    pub(crate) chrome: Option<(crate::makepad_math::Rect, bool)>,
    /// The window system paces this window with frames of its own (a `FrameReady` named it):
    /// its pass paints only with `credit`, so each window presents at its own display's
    /// rate. A window the backend never names (the second window of a single-output backend)
    /// is not paced and paints on any beat, as before.
    pub(crate) paced: bool,
    /// A `FrameReady` for this window arrived and no frame of it was built since.
    pub(crate) credit: bool,
}

pub struct HalClient {
    /// An embedder pumps this Cx and controls presentation. It owns the HAL session.
    pub(crate) externally_driven: bool,
    /// the open session: its id and the backend's capabilities
    pub(crate) info: Option<HalInfo>,
    /// memory `hal_pump` writes events into, kept between turns
    pub(crate) events: Option<EventStore>,
    pub(crate) frames: FrameBuilder,
    /// HAL clock (ns) at `init_cx_os`: `seconds_since_app_start` counts from it
    pub(crate) start_ns: u64,
    pub(crate) windows: HashMap<usize, HalWindow>,
    /// Only the first window may take a window offered by the host, even after it closes.
    pub(crate) window_created: bool,
    #[cfg(target_os = "linux")]
    pub(crate) direct_cursor: crate::cursor::MouseCursor,
    #[cfg(target_os = "linux")]
    pub(crate) direct_display: display::DisplayClient,
    /// External compositor state for `--stdin-loop`; ordinary windows have none.
    pub(crate) hosted: Option<hosted::HostedClient>,
    /// a window drag started by the app (`CxOsOp::StartDragging`): the in-window drag protocol
    /// synthesises Drag / Drop events from mouse move and up
    pub(crate) internal_drag_items: Option<Arc<Vec<DragItem>>>,
    /// the oldest input event (HAL ns) consumed since the last frame was built
    pub(crate) input_ns: u64,
    /// a composition is in flight (`Ime` start / update seen, not yet committed or cancelled)
    pub(crate) composing: bool,
    /// the soft keyboard is up and its height (layout points), to drop repeated reports
    pub(crate) ime_visible: bool,
    pub(crate) ime_height: f64,
    /// a window announced a frame (`FrameReady`)
    pub(crate) beat: bool,
    /// The frame profile (`makepad_hal::profile`): the last beat, input intake since it.
    pub(crate) profile_beat: u64,
    pub(crate) profile_input_ns: u64,
    /// this beat's CPU phases (`FrameMeta::phases`): start, end of dispatch, next-frame
    /// callbacks, draw, then build / submit / spare (unused)
    pub(crate) beat_phases: [[u64; 2]; 6],
    /// the HAL said the GPU device is lost and has not reset it yet: no frames are built
    pub(crate) device_lost: bool,
    /// another thread woke the loop (`Wake`)
    pub(crate) wake: bool,
    /// the window that has the keyboard focus, for the ops that name no window
    pub(crate) focused: Option<WindowId>,
    /// frames the HAL answered with `FrameDone`
    pub(crate) frames_done: u64,
    /// file dialogs the HAL was asked for and has not answered: request id -> what to post
    pub(crate) dialogs: HashMap<u32, PendingDialog>,
    pub(crate) next_dialog: u32,
    /// request ids of `hal_start_domain` / `hal_list_domains`
    pub(crate) next_domain_request: u32,
    /// the application icon went to the HAL
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    pub(crate) icon_sent: bool,
    /// the `MidiInput` handles: each gets every message
    pub(crate) midi_senders: Vec<crate::makepad_network::mpsc::Sender<(MidiPortId, MidiData)>>,
    /// the registered audio entries' state (input, output), by slot
    pub(crate) audio_ctx: [Vec<Option<media::AudioCtx>>; 2],
    /// entries the HAL has replaced: freed once the batch that replaced them is submitted
    pub(crate) retired_ctx: Vec<media::AudioCtx>,
    #[cfg(any(target_vendor = "apple", target_os = "windows"))]
    pub(crate) game: media::GameClient,
    pub(crate) camera: camera::CameraClient,
    /// videos the app's own decoder plays, and the ones the HAL's native player plays
    pub(crate) software_videos: HashMap<crate::makepad_live_id::LiveId, software_video::SoftwareVideo>,
    pub(crate) native_videos: HashMap<crate::makepad_live_id::LiveId, software_video::NativeVideo>,
    /// the least event payload room the session needs (camera frames and readbacks are large)
    pub(crate) event_arena_min: usize,
    pub(crate) readbacks: readback::Readbacks,
    /// font service answers the text stack has not taken yet
    pub(crate) font_events: Vec<HalFontEvent>,
    pub(crate) glyph_wait: fonts::GlyphWait,
    /// the GLES backend's share group and the Cx thread's GL context (`Cx::with_gl`)
    #[cfg(not(target_os = "android"))]
    pub(crate) gl: gl_interop::GlInterop,
    /// the last safe-area insets the shell reported (native points), for windows that appear later
    pub(crate) safe_area: crate::event::SafeAreaInsets,
    /// APK font files already mapped (see `Cx::get_resource_font_asset_bytes_by_path`)
    #[cfg(target_os = "android")]
    pub(crate) mapped_font_assets: std::cell::RefCell<HashMap<String, Option<crate::shared_bytes::SharedBytes>>>,
}

impl Default for HalClient {
    fn default() -> Self {
        Self {
            externally_driven: false,
            info: None,
            events: None,
            frames: FrameBuilder::default(),
            start_ns: 0,
            windows: HashMap::new(),
            window_created: false,
            #[cfg(target_os = "linux")]
            direct_cursor: Default::default(),
            #[cfg(target_os = "linux")]
            direct_display: Default::default(),
            hosted: None,
            internal_drag_items: None,
            input_ns: 0,
            composing: false,
            ime_visible: false,
            ime_height: 0.0,
            beat: false,
            profile_beat: 0,
            profile_input_ns: 0,
            beat_phases: [[0; 2]; 6],
            device_lost: false,
            wake: false,
            focused: None,
            frames_done: 0,
            dialogs: HashMap::new(),
            next_dialog: 0,
            next_domain_request: 0,
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            icon_sent: false,
            midi_senders: Vec::new(),
            audio_ctx: [Vec::new(), Vec::new()],
            retired_ctx: Vec::new(),
            #[cfg(any(target_vendor = "apple", target_os = "windows"))]
            game: Default::default(),
            camera: Default::default(),
            software_videos: HashMap::new(),
            native_videos: HashMap::new(),
            readbacks: Default::default(),
            event_arena_min: 0,
            font_events: Vec::new(),
            glyph_wait: Default::default(),
            #[cfg(not(target_os = "android"))]
            gl: Default::default(),
            safe_area: Default::default(),
            #[cfg(target_os = "android")]
            mapped_font_assets: Default::default(),
        }
    }
}

impl HalClient {
    pub(crate) fn window_ready(&self, id: WindowId) -> bool {
        self.windows.get(&id.0).is_some_and(|w| w.confirmed)
    }

    /// A frame of the window may be built now: it is not paced by its display, or its display
    /// announced a frame it has not used. Two windows on two displays (DECK's output window)
    /// share one beat, which comes at every vsync of either: without this a window was built
    /// and presented at both displays' vsyncs together, more often than its display shows
    /// frames, and the presents queued up and blocked.
    pub(crate) fn window_may_paint(&self, id: WindowId) -> bool {
        let display_clock = self.info.as_ref().is_some_and(|info| info.flags & makepad_hal::info_flags::DISPLAY_CLOCK != 0);
        !display_clock || self.hosted.is_some() || self.externally_driven || self.windows.get(&id.0).is_none_or(|w| !w.paced || w.credit)
    }

    /// A frame of the window was built: its credit is used.
    pub(crate) fn window_painted(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get_mut(&id.0) {
            w.credit = false;
        }
    }

    /// Seconds on the app clock for a HAL time stamp.
    pub(crate) fn secs(&self, ns: u64) -> f64 {
        ns.saturating_sub(self.start_ns) as f64 / 1e9
    }
}

/// MIDI messages from the HAL's input ports, for one `MidiInput` handle.
pub struct OsMidiInput(crate::makepad_network::mpsc::Receiver<(MidiPortId, MidiData)>);

impl OsMidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        self.0.try_recv().ok()
    }
}

/// Sends channel messages to the HAL's output ports. A send goes to the HAL at once, from any
/// thread (a command batch of its own), so a note does not wait for the app's next turn.
#[derive(Clone)]
pub struct OsMidiOutput {
    session: u32,
}

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        let mut w = makepad_hal::CmdWriter::new(self.session);
        let data = (d.data[0] as u32) << 16 | (d.data[1] as u32) << 8 | d.data[2] as u32;
        w.midi(makepad_hal::media::midi_op::SEND, port_id.map_or(0, |p| p.0 .0), data, &[]);
        makepad_hal::hal_submit(&w.buf());
    }
}

impl CxMediaApi for Cx {
    fn midi_input(&mut self) -> MidiInput {
        self.hal_midi_op(makepad_hal::media::midi_op::OPEN, &[]);
        let (send, recv) = crate::makepad_network::mpsc::channel();
        self.hal.midi_senders.push(send);
        MidiInput(Some(OsMidiInput(recv)))
    }

    fn midi_output(&mut self) -> MidiOutput {
        self.hal_midi_op(makepad_hal::media::midi_op::OPEN, &[]);
        MidiOutput(Some(OsMidiOutput { session: self.hal.frames.w.session() }))
    }

    fn midi_reset(&mut self) {
        self.hal_midi_op(makepad_hal::media::midi_op::RESET, &[]);
    }

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        let ids: Vec<u64> = ports.iter().map(|p| p.0 .0).collect();
        self.hal_midi_op(makepad_hal::media::midi_op::USE_INPUTS, &ids);
    }

    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        let ids: Vec<u64> = ports.iter().map(|p| p.0 .0).collect();
        self.hal_midi_op(makepad_hal::media::midi_op::USE_OUTPUTS, &ids);
    }

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.hal_use_audio(makepad_hal::media::audio_dir::INPUT, 0, devices);
    }

    fn use_audio_inputs_with_options(&mut self, devices: &[AudioDeviceId], options: crate::audio::AudioInputOptions) {
        let bits = if options.echo_cancellation { makepad_hal::media::audio_option::ECHO_CANCELLATION } else { 0 };
        self.hal_use_audio(makepad_hal::media::audio_dir::INPUT, bits, devices);
    }

    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.hal_use_audio(makepad_hal::media::audio_dir::OUTPUT, 0, devices);
    }

    fn audio_output_box_os(&mut self, index: usize, f: AudioOutputFn) {
        self.hal_audio_output(index, f);
    }

    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        self.hal_audio_input(index, f);
    }

    fn video_input_box(&mut self, index: usize, f: VideoInputFn) {
        self.hal_video_input_box(index, f);
    }

    fn camera_frame_input_box(&mut self, index: usize, f: crate::video::CameraFrameInputFn) {
        self.hal_camera_frame_input_box(index, f);
    }

    fn video_encoder_output_box(&mut self, index: usize, config: crate::video::VideoEncoderConfig, f: crate::video::VideoOutputFn) -> Result<(), crate::video::VideoEncodeError> {
        let result = self.hal_video_encoder_output_box(index, config, f);
        if let Err(err) = &result {
            crate::error!("video_encoder_output_box failed: {:?}", err);
        }
        result
    }

    fn video_encoder_push_frame(&mut self, index: usize, frame: crate::video::CameraFrameRef<'_>) {
        self.hal_video_encoder_push_frame(index, frame);
    }

    fn video_encoder_capture_texture_frame(&mut self, index: usize, timestamp_ns: u64) -> Result<(), crate::video::VideoEncodeError> {
        self.hal_video_encoder_capture_texture_frame(index, timestamp_ns)
    }

    fn video_encoder_request_keyframe(&mut self, index: usize) -> Result<(), crate::video::VideoEncodeError> {
        self.hal_video_encoder_request_keyframe(index)
    }

    #[cfg(target_vendor = "apple")]
    fn video_capabilities(&self) -> crate::video::VideoCapabilities {
        use crate::video::*;
        let probe = makepad_hal::video_h264_probe();
        let encode_available = probe.encode_hardware || probe.encode_software;
        let decode_available = probe.decode_hardware || probe.decode_software;
        let h264 = VideoCodecSupport {
            codec: VideoCodec::H264,
            encode_hardware: probe.encode_hardware,
            encode_software: probe.encode_software,
            decode_hardware: probe.decode_hardware,
            decode_software: probe.decode_software,
            encode_formats: if encode_available { vec![VideoBitstreamFormat::AnnexB] } else { Vec::new() },
            decode_formats: if decode_available { vec![VideoBitstreamFormat::AnnexB, VideoBitstreamFormat::Avcc] } else { Vec::new() },
            supports_camera_source: encode_available,
            supports_texture_source: encode_available,
            supports_cpu_frames_source: encode_available,
            supports_keyframe_request: false,
            supports_dynamic_resolution: false,
            width_alignment: Some(2),
            height_alignment: Some(2),
            max_width: None,
            max_height: None,
            max_fps: None,
            max_bitrate: None,
        };
        crate::merge_video_capabilities(VideoCapabilities { codecs: vec![h264, VideoCodecSupport::unsupported(VideoCodec::H265)] }, crate::media_video_capabilities())
    }

    fn use_video_input(&mut self, devices: &[(VideoInputId, VideoFormatId)]) {
        self.hal_use_video_input(devices);
    }

    fn video_inputs(&self) -> Option<Vec<crate::video::VideoInputDesc>> {
        self.hal_video_inputs()
    }

    fn video_input_open(&mut self, index: usize, input: VideoInputId, format: VideoFormatId) -> Result<crate::video::VideoFormat, String> {
        self.hal_video_input_open(index, input, format)
    }

    fn video_input_stream(&mut self, index: usize, on: bool) -> Result<(), String> {
        self.hal_video_input_stream(index, on)
    }

    fn video_input_close(&mut self, index: usize) {
        self.hal_video_input_close(index);
    }
}

#[cfg(target_os = "android")]
mod android_socket;

impl crate::cx_api::CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.hal.start_ns = makepad_hal::clock_ns();
        // an Android APK and an iOS / tvOS device have no build tree: the resources are in the
        // package, under `makepad/` (the iOS simulator reads the build machine's files like a
        // desktop app)
        #[cfg(target_os = "android")]
        {
            android_socket::register();
            self.package_root = Some("makepad".to_string());
            self.native_load_dependencies();
        }
        #[cfg(all(any(target_os = "ios", target_os = "tvos"), not(apple_sim)))]
        {
            self.package_root = Some("makepad".to_string());
            self.bundle_load_dependencies();
        }
        #[cfg(not(any(target_os = "android", all(any(target_os = "ios", target_os = "tvos"), not(apple_sim)))))]
        {
            if let Some(item) = crate::app_meta::package_dir() {
                self.package_root = Some(item.to_string());
            }
            self.native_load_dependencies();
        }
    }

    fn seconds_since_app_start(&self) -> f64 {
        if let Some(time) = crate::remote::app_clock::now() {
            return time;
        }
        self.hal.secs(makepad_hal::clock_ns())
    }

    fn open_url(&mut self, url: &str, _in_place: OpenUrlInPlace) {
        #[cfg(any(target_vendor = "apple", target_os = "windows"))]
        if !makepad_hal::open_url(url) { crate::error!("open_url: the system did not open {url}"); }
        #[cfg(not(any(target_vendor = "apple", target_os = "windows")))]
        crate::warning!("open_url is unavailable on this HAL backend: {url}");
    }
}

impl Cx {
    /// Loads the dependencies from the app bundle's resources (iOS / tvOS devices).
    #[cfg(all(any(target_os = "ios", target_os = "tvos"), not(apple_sim)))]
    fn bundle_load_dependencies(&mut self) {
        let root = self.package_root.clone().unwrap_or_default();
        for (path, dep) in &mut self.dependencies {
            dep.data = Some(match makepad_hal::bundle_resource(&format!("{root}/{path}")) {
                Some(bytes) => Ok(std::rc::Rc::new(bytes)),
                None => Err(format!("bundled file {root}/{path} could not be read")),
            });
        }
    }
}

impl Cx {
    /// A shader can draw once the HAL said its pipeline is ready; a caller with a fallback asks
    /// this and keeps drawing the fallback until then.
    pub fn is_draw_shader_window_ready(&self, shader_id: crate::draw_shader::DrawShaderId) -> bool {
        self.draw_shaders.shaders[shader_id.index].pipeline_id.is_some_and(|pipe| self.hal.frames.pipeline_is_ready(pipe))
    }

    pub(crate) fn poll_texture_lifetimes(&mut self) {
        // the HAL frees what a TextureDestroy names; shared blocks retire in the frame builder
        self.publications.retire_without_backing();
    }

    /// Gives the texture's HAL storage back (the next pass or upload creates it again).
    pub(crate) fn release_texture_allocation(&mut self, id: crate::texture::TextureId) {
        self.hal.frames.texture_destroy(id.0);
        self.textures[id].reset_allocation();
    }

}

/// The host reads APK assets; no app-side JNI state is needed by the HAL client.
#[cfg(target_os = "android")]
pub(crate) fn read_asset(path: &str) -> Option<Vec<u8>> {
    let path = if path.starts_with("makepad/") {
        path.to_string()
    } else {
        format!("makepad/{path}")
    };
    let len = usize::try_from(makepad_hal::hal_read_asset(&path, &mut [])).ok()?;
    let mut bytes = vec![0; len];
    (makepad_hal::hal_read_asset(&path, &mut bytes) == len as i64).then_some(bytes)
}

/// Maps a stored APK asset without a heap copy; compressed or missing assets return None. The
/// mapping is named `apk:<asset>` so the HAL's font service maps the same asset by that name.
#[cfg(target_os = "android")]
pub(crate) fn map_asset(path: &str) -> Option<crate::shared_bytes::SharedBytes> {
    use std::os::fd::{FromRawFd, OwnedFd, AsRawFd};
    let mut range = [0i64; 2];
    let mut name = path.to_string();
    let mut fd = makepad_hal::hal_map_asset(&name, &mut range);
    if fd < 0 && !path.starts_with("makepad/") {
        name = format!("makepad/{path}");
        fd = makepad_hal::hal_map_asset(&name, &mut range);
    }
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    crate::shared_bytes::SharedBytes::map_fd_range_named(fd.as_raw_fd(), u64::try_from(range[0]).ok()?, usize::try_from(range[1]).ok()?, &format!("{}{name}", makepad_hal::APK_ASSET_PREFIX)).ok()
}
