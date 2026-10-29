//! The media half of the HAL client: video and audio-file playback. The ops become the media
//! commands of `makepad-hal` (`VideoPrepare`, `VideoControl`); what the HAL's media module
//! learns (prepared, a new frame, ranges, errors) arrives as events and becomes the `Event`s
//! the Video widget already handles. The picture is not in Cx's hands: the HAL fills the
//! textures the prepare names.

use crate::{
    audio::{AudioBuffer, AudioDeviceDesc, AudioDeviceId, AudioDeviceType, AudioDevicesEvent, AudioInfo, AudioInputFn, AudioOutputFn, AudioTime},
    cx::Cx,
    event::{
        video_playback::*, Event,
    },
    makepad_live_id::LiveId,
    event::{LocationErrorEvent, LocationUpdateEvent},
    midi::{MidiData, MidiPortDesc, MidiPortId, MidiPortType, MidiPortsEvent},
    permission::{Permission, PermissionResult, PermissionStatus},
    texture::{Texture, TextureFormat, TextureId},
};
use makepad_hal::{media, HalEvent, Id};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
use crate::{
    event::game_input::{PressLatch, ForceTarget, GameInputConnectedEvent, GameInputForce, GameInputInfo, GameInputOutput, GameInputState, GamepadHapticCapabilities, GamepadHapticPulse, GamepadState, JoystickState, WheelState},
    game_input::CxGameInputApi,
    makepad_math::Vec2,
};
#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
use makepad_hal::media::{game, game_output};

/// The controllers the HAL told the session about, in the HAL's order (the order
/// `game_input_states` has), with their latest states.
#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
#[derive(Default)]
pub(crate) struct GameClient {
    infos: Vec<GameInputInfo>,
    /// What the app reads: `truth` with this frame's taps held (`latches`).
    states: Vec<GameInputState>,
    /// The latest states the HAL reported.
    truth: Vec<GameInputState>,
    latches: Vec<PressLatch>,
    devices: Vec<game::GameDevice>,
}

#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
impl GameClient {
    /// A frame was delivered to the app: the taps it saw as held are released.
    pub(crate) fn frame_end(&mut self) {
        for ((latch, truth), state) in self.latches.iter_mut().zip(&self.truth).zip(&mut self.states) {
            latch.frame_end(truth);
            *state = latch.view(truth);
        }
    }
}

#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
fn cx_state(s: game::GameState) -> GameInputState {
    match s {
        game::GameState::Gamepad(p) => GameInputState::Gamepad(GamepadState {
            a: p.a,
            b: p.b,
            x: p.x,
            y: p.y,
            left_shoulder: p.left_shoulder,
            right_shoulder: p.right_shoulder,
            left_trigger: p.left_trigger,
            right_trigger: p.right_trigger,
            select: p.select,
            start: p.start,
            home: p.home,
            left_thumb: p.left_thumb,
            right_thumb: p.right_thumb,
            dpad_up: p.dpad_up,
            dpad_down: p.dpad_down,
            dpad_left: p.dpad_left,
            dpad_right: p.dpad_right,
            left_stick: Vec2 { x: p.left_stick[0], y: p.left_stick[1] },
            right_stick: Vec2 { x: p.right_stick[0], y: p.right_stick[1] },
            presses: p.presses,
        }),
        game::GameState::Wheel(w) => GameInputState::Wheel(WheelState { steering: w.steering, throttle: w.throttle, brake: w.brake, clutch: w.clutch, steer_force: w.steer_force, buttons: w.buttons }),
        game::GameState::Joystick(j) => GameInputState::Joystick(JoystickState { x: j.x, y: j.y, twist: j.twist, throttle: j.throttle, hat: j.hat, buttons: j.buttons }),
    }
}

/// The state of a registered realtime audio entry: the HAL calls `audio_output_entry` /
/// `audio_input_entry` with a pointer to it from the OS's audio thread. It stays alive until the
/// batch that replaced or cleared it has been submitted (then the HAL no longer calls it).
pub(crate) struct AudioCtx {
    _owner: Box<dyn std::any::Any>,
}

pub(crate) struct OutputCtx {
    f: AudioOutputFn,
    buffer: AudioBuffer,
}

pub(crate) struct InputCtx {
    f: AudioInputFn,
    buffer: AudioBuffer,
}

fn audio_entry_buffer() -> AudioBuffer {
    // Linux and Android transports split every negotiated period into this scratch quantum. Allocate
    // before registering the entry so resize in a realtime callback only changes the length.
    #[cfg(target_os = "linux")]
    { AudioBuffer::new_with_size(makepad_hal::backend::linux::media::AUDIO_BLOCK_FRAMES, 2) }
    #[cfg(target_os = "android")]
    { AudioBuffer::new_with_size(makepad_hal::backend::android::media::AUDIO_BLOCK_FRAMES, 2) }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    { AudioBuffer::default() }
}

/// When an audio entry's real-time code is compiled (Relax; nothing to wait for in a rustc build).
#[cfg(relax)]
type RealtimeReady = std::relax::Realtime;
#[cfg(not(relax))]
type RealtimeReady = ();

/// An audio entry whose real-time code Relax still compiles (`Cx::hal_audio_entry`).
#[cfg(relax)]
pub(crate) struct AudioWaiting {
    dir: u32,
    index: u32,
    entry: u64,
    ptr: u64,
    ready: std::relax::Realtime,
    /// the state of the entry the HAL calls for this slot until it has this one
    live: Option<AudioCtx>,
}

/// The entry the HAL calls on the device's real-time thread and the app's function it calls:
/// under Relax everything they reach is compiled before the entry goes to the HAL, in the
/// background (a first call there would stall the device: a glitch), within a bounded wait. The UI
/// thread is woken when it is ready.
#[cfg(relax)]
fn realtime_entry<F: ?Sized>(entry: &extern "C" fn(*mut c_void, *mut media::AudioBlock), f: &F) -> std::relax::Realtime {
    extern "C" fn wake() {
        crate::thread::SignalToUI::set_internal_signal();
    }
    let _ = std::relax::realtime(entry, None);
    std::relax::realtime(f, Some(wake))
}

fn block_info(b: &media::AudioBlock) -> AudioInfo {
    AudioInfo {
        device_id: AudioDeviceId(LiveId(b.device)),
        time: (b.has_time != 0).then_some(AudioTime { sample_time: b.sample_time, host_time: b.host_time, rate_scalar: b.rate_scalar }),
        sample_rate: b.sample_rate,
    }
}

/// The HAL's output entry: the app's render function fills a buffer, which goes into the block.
/// It catches its own panics (an `extern "C"` fn that unwinds aborts the process): a block whose
/// render panicked stays silent.
extern "C" fn audio_output_entry(ctx: *mut c_void, block: *mut media::AudioBlock) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `ctx` is the OutputCtx registered with this entry; the HAL calls the entry
        // serially per slot and does not call it after the command that replaced it returned
        let ctx = unsafe { &mut *(ctx as *mut OutputCtx) };
        let b = unsafe { &mut *block };
        let (frames, channels) = (b.frames as usize, b.channels as usize);
        ctx.buffer.resize(frames, channels);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        ctx.buffer.data.fill(0.0);
        (ctx.f)(block_info(b), &mut ctx.buffer);
        for c in 0..channels {
            // SAFETY: the block names `channels` planes of `frames` floats
            let plane = unsafe { std::slice::from_raw_parts_mut(*b.planes.add(c), frames) };
            plane.copy_from_slice(ctx.buffer.channel(c));
        }
    }));
}

/// The HAL's input entry: the block's samples go into a buffer the app's function reads.
extern "C" fn audio_input_entry(ctx: *mut c_void, block: *mut media::AudioBlock) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: as in `audio_output_entry`
        let ctx = unsafe { &mut *(ctx as *mut InputCtx) };
        let b = unsafe { &*block };
        let (frames, channels) = (b.frames as usize, b.channels as usize);
        ctx.buffer.resize(frames, channels);
        for c in 0..channels {
            let plane = unsafe { std::slice::from_raw_parts(*b.planes.add(c), frames) };
            ctx.buffer.channel_mut(c).copy_from_slice(plane);
        }
        (ctx.f)(block_info(b), &ctx.buffer);
    }));
}

fn permission_code(p: Permission) -> u32 {
    match p {
        Permission::AudioInput => media::permission_kind::AUDIO_INPUT,
        Permission::Camera => media::permission_kind::CAMERA,
        Permission::HeadsetCamera => media::permission_kind::HEADSET_CAMERA,
        Permission::SceneAccess => media::permission_kind::SCENE_ACCESS,
        Permission::Location => media::permission_kind::LOCATION,
    }
}

fn permission_of(code: u32) -> Permission {
    match code {
        media::permission_kind::AUDIO_INPUT => Permission::AudioInput,
        media::permission_kind::CAMERA => Permission::Camera,
        media::permission_kind::HEADSET_CAMERA => Permission::HeadsetCamera,
        media::permission_kind::SCENE_ACCESS => Permission::SceneAccess,
        _ => Permission::Location,
    }
}

impl Cx {
    /// A permission check or request: the status arrives as `Event::PermissionResult`.
    pub(crate) fn hal_permission(&mut self, op: u32, permission: Permission, request_id: i32) {
        self.hal.frames.w.permission(op, permission_code(permission), request_id as u32);
    }

    /// A system haptic (`media::haptic_kind::*`).
    #[cfg(target_os = "android")]
    pub(crate) fn hal_haptic(&mut self, kind: crate::cx_api::HapticFeedback) {
        use crate::cx_api::HapticFeedback;
        self.hal.frames.w.haptic(match kind {
            HapticFeedback::Click => makepad_hal::media::haptic_kind::CLICK,
            HapticFeedback::VirtualKey => makepad_hal::media::haptic_kind::VIRTUAL_KEY,
            HapticFeedback::Tick => makepad_hal::media::haptic_kind::TICK,
        });
    }

    pub(crate) fn hal_location(&mut self, op: u32) {
        self.hal.frames.w.location(op);
    }

    /// A MIDI client command (`midi_op::*`) with the port ids it lists.
    pub(crate) fn hal_midi_op(&mut self, op: u32, ports: &[u64]) {
        let ports = media::pack_u64s(ports);
        self.hal.frames.w.midi(op, 0, 0, &ports);
    }

    pub(crate) fn hal_use_audio(&mut self, dir: u32, options: u32, devices: &[AudioDeviceId]) {
        let ids: Vec<u64> = devices.iter().map(|d| d.0 .0).collect();
        self.hal.frames.w.audio_use(dir, options, &media::pack_u64s(&ids));
    }

    pub(crate) fn hal_audio_output(&mut self, index: usize, f: AudioOutputFn) {
        let mut ctx = Box::new(OutputCtx { f, buffer: audio_entry_buffer() });
        let ptr = &mut *ctx as *mut OutputCtx as u64;
        let entry: extern "C" fn(*mut c_void, *mut media::AudioBlock) = audio_output_entry;
        #[cfg(relax)]
        let ready = realtime_entry(&entry, &*ctx.f);
        #[cfg(not(relax))]
        let ready = ();
        self.hal_audio_entry(media::audio_dir::OUTPUT, index, entry as usize as u64, ptr, AudioCtx { _owner: ctx }, ready);
    }

    pub(crate) fn hal_audio_input(&mut self, index: usize, f: AudioInputFn) {
        let mut ctx = Box::new(InputCtx { f, buffer: audio_entry_buffer() });
        let ptr = &mut *ctx as *mut InputCtx as u64;
        let entry: extern "C" fn(*mut c_void, *mut media::AudioBlock) = audio_input_entry;
        #[cfg(relax)]
        let ready = realtime_entry(&entry, &*ctx.f);
        #[cfg(not(relax))]
        let ready = ();
        self.hal_audio_entry(media::audio_dir::INPUT, index, entry as usize as u64, ptr, AudioCtx { _owner: ctx }, ready);
    }

    /// Registers an audio entry with the HAL; under Relax once the code it runs on the device's
    /// thread is compiled, or after a bounded wait (a few hundred ms at most: audio starts
    /// promptly). The state of the entry the HAL calls now stays alive until the HAL has the new one.
    fn hal_audio_entry(&mut self, dir: u32, index: usize, entry: u64, ptr: u64, ctx: AudioCtx, ready: RealtimeReady) {
        let list = &mut self.hal.audio_ctx[dir as usize];
        if list.len() <= index {
            list.resize_with(index + 1, || None);
        }
        let old = std::mem::replace(&mut list[index], Some(ctx));
        #[cfg(relax)]
        {
            // a waiting entry of this slot never reached the HAL: its state goes, the state the
            // HAL calls stays with the new one
            let mut live = None;
            if let Some(i) = self.hal.audio_waiting.iter().position(|w| w.dir == dir && w.index == index as u32) {
                live = self.hal.audio_waiting.remove(i).live;
                self.hal.retired_ctx.extend(old);
            } else if ready.ready() {
                self.hal.retired_ctx.extend(old);
            } else {
                live = old;
            }
            if !ready.ready() {
                self.hal.audio_waiting.push(AudioWaiting { dir, index: index as u32, entry, ptr, ready, live });
                return;
            }
            self.hal.retired_ctx.extend(live);
        }
        #[cfg(not(relax))]
        {
            let () = ready;
            self.hal.retired_ctx.extend(old);
        }
        self.hal.frames.w.audio_entry(dir, index as u32, entry, ptr);
    }

    /// The waiting audio entries whose code is compiled go to the HAL (with the next batch).
    #[cfg(relax)]
    pub(crate) fn hal_audio_waiting_ready(&mut self) {
        if self.hal.audio_waiting.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.hal.audio_waiting);
        for w in waiting {
            if w.ready.ready() {
                self.hal.frames.w.audio_entry(w.dir, w.index, w.entry, w.ptr);
                self.hal.retired_ctx.extend(w.live);
            } else {
                self.hal.audio_waiting.push(w);
            }
        }
    }

    /// A texture whose storage the HAL makes: its HAL id.
    fn hal_external_texture(&mut self, id: TextureId) -> Id {
        self.hal.frames.external_texture(id.0, id.generation())
    }

    /// One plane of a native biplanar 4:2:0 picture the app holds, as a texture over the
    /// picture's own memory: nothing is copied (macOS: `handle` is the picture's global
    /// IOSurface id; luma samples as R8, chroma as RG8 at half size). The app keeps the picture
    /// alive while frames that draw the texture may still run on the GPU.
    pub fn import_picture_plane(&mut self, handle: u64, plane: crate::video_file::PicturePlane) -> Texture {
        let texture = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
        let id = self.hal_external_texture(texture.texture_id());
        let kind = match plane {
            crate::video_file::PicturePlane::Luma => makepad_hal::texture_handle::LUMA,
            crate::video_file::PicturePlane::Chroma => makepad_hal::texture_handle::CHROMA,
        };
        self.hal.frames.w.texture_import(id, handle, kind);
        texture
    }

    /// A decoder's whole picture the app holds (`DecodedPicture::handle` of a GPU stream
    /// decoder whose pictures are not biplanar planes: Android's), as one texture over its
    /// memory for a `texture_video` slot: sampled there it gives the picture's raw Y'CbCr codes
    /// (.g = Y', .b = Cb, .r = Cr). Nothing is copied. The app keeps the picture alive while
    /// frames that draw the texture may still run on the GPU.
    pub fn import_picture(&mut self, handle: u64) -> Texture {
        let texture = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
        let id = self.hal_external_texture(texture.texture_id());
        self.hal.frames.w.texture_import(id, handle, makepad_hal::texture_handle::VIDEO);
        texture
    }

    /// Prepares video (or an audio file when `texture` is `None`) playback. The Y / U / V
    /// textures are made here and announced with `VideoYuvTexturesReady`, as every backend does.
    pub(crate) fn hal_prepare_video(&mut self, video_id: LiveId, source: VideoSource, texture: Option<TextureId>, autoplay: bool, looping: bool, camera_preview: CameraPreviewMode) {
        // Linux always delivers camera frames as textures: the preview mode is not consulted
        #[cfg(target_os = "linux")]
        let _ = camera_preview;
        #[cfg(target_os = "linux")]
        if source.is_adaptive_manifest() && std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some() {
            self.call_event_handler(&Event::VideoDecodingError(VideoDecodingErrorEvent {
                video_id, error: "MAKEPAD_FORCE_SOFTWARE_VIDEO cannot decode HLS/DASH manifests; unset it or use a progressive URL".into(),
            }));
            return;
        }
        let mut mode = 0u16;
        if autoplay {
            mode |= media::video_mode::AUTOPLAY;
        }
        if looping {
            mode |= media::video_mode::LOOP;
        }
        // the app's own decoder plays playback sessions, and everything when asked for
        if !matches!(source, VideoSource::Camera(..)) && (source.is_session() || std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some()) {
            self.hal_start_software_video(video_id, texture, source, autoplay, looping);
            return;
        }
        let camera_source;
        let (kind, bytes): (u16, &[u8]) = match &source {
            VideoSource::Network(url) => (media::src_kind::NETWORK, url.as_bytes()),
            VideoSource::Filesystem(path) => (media::src_kind::FILE, path.as_bytes()),
            VideoSource::InMemory(data) => (media::src_kind::MEMORY, data.as_slice()),
            VideoSource::Camera(input, format) => {
                // a native preview: a native view shows the camera, no textures. Linux and Android
                // also deliver camera frames as textures; the other platforms only have the view.
                #[cfg(not(target_os = "linux"))]
                if camera_preview == CameraPreviewMode::Native || cfg!(not(target_os = "android")) {
                    self.hal_prepare_camera_preview(video_id, *input, *format);
                    return;
                }
                mode |= media::video_mode::CAMERA;
                camera_source = media::pack_u64s(&[input.0 .0, format.0 .0]);
                (media::src_kind::MEMORY, &camera_source)
            }
            VideoSource::PlaybackSession(..) | VideoSource::Session(..) => return,
        };
        if let Some(mut old) = self.hal.software_videos.remove(&video_id) {
            old.cleanup();
        }
        self.hal.native_videos.insert(video_id, super::software_video::NativeVideo { texture, source: source.clone(), autoplay, looping, waiting_since: None, swallow_release: false });
        mode |= kind << media::video_mode::SRC_SHIFT;
        let Some(texture) = texture else {
            self.hal.frames.w.video_prepare(Id::NONE, mode | media::video_mode::AUDIO_ONLY, video_id.0, Id::NONE, Id::NONE, Id::NONE, bytes);
            return;
        };
        // YUV textures for the NV12 path; the widget binds them to its shader slots
        let tex_y = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
        let tex_u = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
        let tex_v = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
        let ids = [texture, tex_y.texture_id(), tex_u.texture_id(), tex_v.texture_id()].map(|t| self.hal_external_texture(t));
        self.hal.frames.w.video_prepare(ids[0], mode, video_id.0, ids[1], ids[2], ids[3], bytes);
        self.call_event_handler(&Event::VideoYuvTexturesReady(VideoYuvTexturesReady::planes(video_id, tex_y, tex_u, tex_v)));
    }

    pub(crate) fn hal_video_control(&mut self, video_id: LiveId, op: u32, arg: u64) {
        if self.hal_software_video_control(video_id, op, arg) {
            return;
        }
        if op == media::video_op::CLEANUP {
            self.hal_release_camera_preview(video_id);
        }
        self.hal.frames.w.video_control(video_id.0, op, arg);
    }

    /// The media events of the HAL, as the events the widgets know.
    pub(crate) fn hal_media_event(&mut self, event: HalEvent<'_>) {
        match event {
            HalEvent::VideoPrepared { video, size, duration_ms, seekable, tracks } => {
                crate::trace!("media", "VideoPrepared video {video:x} {}x{} {duration_ms} ms seekable {seekable}", size[0], size[1]);
                // a native player with a picture to show owes a frame soon
                if let Some(native) = self.hal.native_videos.get_mut(&LiveId(video)) {
                    // GStreamer owns its negotiation/fallback timing on Linux, including
                    // paused and live sources. The AVFoundation watchdog stays on macOS.
                    #[cfg(not(target_os = "linux"))]
                    { native.waiting_since = (size[0] > 0 && size[1] > 0).then(std::time::Instant::now); }
                    #[cfg(target_os = "linux")]
                    { native.waiting_since = None; }
                }
                let (video_tracks, audio_tracks) = media::read_tracks(tracks);
                self.call_event_handler(&Event::VideoPlaybackPrepared(VideoPlaybackPreparedEvent {
                    video_id: LiveId(video),
                    video_width: size[0] as u32,
                    video_height: size[1] as u32,
                    duration: duration_ms as u128,
                    is_seekable: seekable,
                    video_tracks,
                    audio_tracks,
                }));
            }
            HalEvent::VideoFrame { video, pos_ms, yuv, matrix } => {
                if crate::trace_enabled("media") {
                    static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let n = FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if n == 1 || n % 30 == 0 {
                        crate::trace!("media", "VideoFrame #{n} video {video:x} pos {pos_ms} ms");
                    }
                }
                if let Some(native) = self.hal.native_videos.get_mut(&LiveId(video)) {
                    native.waiting_since = None;
                }
                self.call_event_handler(&Event::VideoTextureUpdated(VideoTextureUpdatedEvent {
                    video_id: LiveId(video),
                    current_position_ms: pos_ms as u128,
                    yuv: VideoYuvMetadata {
                        enabled: yuv & media::yuv::ENABLED != 0,
                        matrix,
                        biplanar: yuv & media::yuv::BIPLANAR != 0,
                        full_range: yuv & media::yuv::FULL_RANGE != 0,
                        rotation_steps: 0.0,
                        external: false,
                        array: false,
                    },
                    rgba_gl_2d: false,
                }));
            }
            HalEvent::VideoStatus { video, kind, text } => {
                let video_id = LiveId(video);
                match kind {
                    // the native player failed to prepare: the app's decoder tries
                    media::video_status::ERROR if self.hal.native_videos.get(&video_id).is_some_and(|n| !matches!(n.source, VideoSource::Camera(..)) && !(cfg!(target_os = "linux") && n.source.is_adaptive_manifest())) => {
                        let reason = String::from_utf8_lossy(text).into_owned();
                        self.hal_switch_to_software(video_id, &reason);
                    }
                    media::video_status::RELEASED if self.hal.native_videos.get(&video_id).is_some_and(|n| n.swallow_release) => {
                        // the release the switch to software asked for
                        self.hal.native_videos.remove(&video_id);
                    }
                    media::video_status::RELEASED => {
                        self.hal.native_videos.remove(&video_id);
                        self.call_event_handler(&Event::VideoPlaybackResourcesReleased(VideoPlaybackResourcesReleasedEvent { video_id }))
                    }
                    media::video_status::COMPLETED => self.call_event_handler(&Event::VideoPlaybackCompleted(VideoPlaybackCompletedEvent { video_id })),
                    _ => self.call_event_handler(&Event::VideoDecodingError(VideoDecodingErrorEvent { video_id, error: String::from_utf8_lossy(text).into_owned() })),
                }
            }
            HalEvent::VideoRanges { video, kind, ranges } => {
                let video_id = LiveId(video);
                let ranges = media::read_ranges(ranges);
                match kind {
                    media::ranges::SEEKABLE => self.call_event_handler(&Event::VideoSeekableRanges(VideoSeekableRangesEvent { video_id, ranges })),
                    _ => self.call_event_handler(&Event::VideoBufferedRanges(VideoBufferedRangesEvent { video_id, ranges })),
                }
            }
            HalEvent::AudioDevices { devices } => {
                let descs = media::read_audio_devices(devices)
                    .into_iter()
                    .map(|d| AudioDeviceDesc {
                        device_id: AudioDeviceId(LiveId(d.id)),
                        device_type: match d.kind {
                            media::device_kind::INPUT => AudioDeviceType::Input,
                            media::device_kind::LOOPBACK => AudioDeviceType::Loopback,
                            _ => AudioDeviceType::Output,
                        },
                        is_default: d.is_default,
                        has_failed: d.has_failed,
                        channel_count: d.channels as usize,
                        name: d.name,
                    })
                    .collect();
                self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent { descs }));
            }
            HalEvent::MidiPorts { ports } => {
                crate::trace!("media", "MidiPorts {} ports", media::read_midi_ports(ports).len());
                let descs = media::read_midi_ports(ports)
                    .into_iter()
                    .map(|p| MidiPortDesc { name: p.name, port_id: MidiPortId(LiveId(p.id)), port_type: if p.output { MidiPortType::Output } else { MidiPortType::Input } })
                    .collect();
                self.call_event_handler(&Event::MidiPorts(MidiPortsEvent { descs }));
            }
            HalEvent::Location { kind, lon, lat, accuracy, time, alt, speed, heading, text } => {
                let opt = |v: f32| (!v.is_nan()).then_some(v as f64);
                match kind {
                    media::location_kind::FIX => self.call_event_handler(&Event::LocationUpdate(LocationUpdateEvent {
                        lon,
                        lat,
                        accuracy_m: accuracy,
                        altitude_m: opt(alt),
                        speed_mps: opt(speed),
                        heading_deg: opt(heading),
                        time,
                    })),
                    media::location_kind::DENIED => self.call_event_handler(&Event::LocationError(LocationErrorEvent::PermissionDenied)),
                    _ => self.call_event_handler(&Event::LocationError(LocationErrorEvent::Unavailable(String::from_utf8_lossy(text).into_owned()))),
                }
            }
            HalEvent::Permission { req, kind, status } => {
                crate::trace!("media", "Permission req {req} kind {kind} status {status}");
                let status = match status {
                    media::permission_status::GRANTED => PermissionStatus::Granted,
                    media::permission_status::DENIED_CAN_RETRY => PermissionStatus::DeniedCanRetry,
                    media::permission_status::DENIED_PERMANENT => PermissionStatus::DeniedPermanent,
                    _ => PermissionStatus::NotDetermined,
                };
                self.call_event_handler(&Event::PermissionResult(PermissionResult { permission: permission_of(kind), request_id: req as i32, status }));
            }
            #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
            HalEvent::GameDevices { devices } => {
                let list = game::read_devices(devices);
                let old: Vec<GameInputInfo> = std::mem::take(&mut self.hal.game.infos);
                let old_states = std::mem::take(&mut self.hal.game.states);
                let old_truth = std::mem::take(&mut self.hal.game.truth);
                let old_latches = std::mem::take(&mut self.hal.game.latches);
                let mut infos = Vec::new();
                let mut states = Vec::new();
                let (mut truth, mut latches) = (Vec::new(), Vec::new());
                for d in &list {
                    let info = GameInputInfo { id: LiveId(d.id), name: d.name.clone() };
                    // a device that stays keeps its state (and its latch) until the HAL reports a new one
                    match old.iter().position(|o| o.id == info.id) {
                        Some(i) => {
                            states.push(old_states[i].clone());
                            truth.push(old_truth[i].clone());
                            latches.push(old_latches[i].clone());
                        }
                        None => {
                            let idle = GameInputState::Gamepad(GamepadState::default());
                            states.push(idle.clone());
                            truth.push(idle);
                            // its first report sets the latch's base: counts before it are no taps
                            latches.push(PressLatch::default());
                        }
                    }
                    infos.push(info);
                }
                let announced = |l: &[game::GameDevice], id: u64| l.iter().any(|d| d.id == id && d.announce);
                let was: Vec<game::GameDevice> = std::mem::take(&mut self.hal.game.devices);
                let gone: Vec<GameInputInfo> = old.into_iter().filter(|o| announced(&was, o.id.0) && !announced(&list, o.id.0)).collect();
                let new: Vec<GameInputInfo> = infos.iter().filter(|i| announced(&list, i.id.0) && !announced(&was, i.id.0)).cloned().collect();
                self.hal.game.infos = infos;
                self.hal.game.states = states;
                self.hal.game.truth = truth;
                self.hal.game.latches = latches;
                self.hal.game.devices = list;
                for info in gone {
                    self.call_event_handler(&Event::GameInputConnected(GameInputConnectedEvent::Disconnected(info)));
                }
                for info in new {
                    self.call_event_handler(&Event::GameInputConnected(GameInputConnectedEvent::Connected(info)));
                }
            }
            #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
            HalEvent::GameInput { device, state } => {
                if let (Some(index), Some(state)) = (self.hal.game.infos.iter().position(|i| i.id.0 == device), game::read_state(state)) {
                    let truth = cx_state(state);
                    self.hal.game.states[index] = self.hal.game.latches[index].view(&truth);
                    self.hal.game.truth[index] = truth;
                }
            }
            #[cfg(target_vendor = "apple")]
            HalEvent::ScreenSurface { window, size, handle } => {
                // the HAL passes one reference of the capture buffer to the recorders' sinks
                let surface = crate::screen_capture::ScreenCaptureSurface {
                    inner: crate::screen_capture::SurfaceInner(handle as makepad_apple_sys::CVPixelBufferRef),
                    width: size[0] as u32,
                    height: size[1] as u32,
                    time_ns: 0,
                };
                crate::screen_capture::deliver_capture_surface(Some(window.index as usize), surface);
            }
            #[cfg(target_os = "windows")]
            HalEvent::ScreenSurface { window, size, handle } => {
                // the HAL passes one reference of its capture texture (on its device) to the sinks
                use crate::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
                let Ok(texture) = (unsafe { ID3D11Texture2D::from_raw(handle as *mut std::ffi::c_void) }) else { return };
                let Ok(device) = (unsafe { texture.as_ID3D11DeviceChild().GetDevice() }) else { return };
                let surface = crate::screen_capture::ScreenCaptureSurface {
                    inner: crate::screen_capture::SurfaceInner { texture, device },
                    width: size[0] as u32,
                    height: size[1] as u32,
                    time_ns: 0,
                };
                crate::screen_capture::deliver_capture_surface(Some(window.index as usize), surface);
            }
            HalEvent::MidiData { port, data } => {
                let message = (MidiPortId(LiveId(port)), MidiData::from(data));
                self.hal.midi_senders.retain(|s| s.send(message).is_ok());
            }
            #[cfg(target_os = "linux")]
            HalEvent::ScreenSurface { window, size: _, handle } => {
                let lease = makepad_hal::backend::linux::media::capture_surface::take_lease(self.hal.frames.w.session(), handle);
                if lease.found() {
                    let (width, height) = (lease.width(), lease.height());
                    crate::screen_capture::deliver_capture_surface(Some(window.index as usize), crate::screen_capture::ScreenCaptureSurface {
                        inner: crate::screen_capture::SurfaceInner(std::sync::Arc::new(lease)), width, height, time_ns: 0,
                    });
                }
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
impl crate::screen_capture::MappedCapture for makepad_hal::backend::linux::media::capture_surface::CaptureLease {
    fn bytes(&self) -> &[u8] { self.bytes() }
    fn stride(&self) -> usize { self.width() as usize * 4 }
    fn bgra(&self) -> bool { self.bgra() }
}

#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
impl CxGameInputApi for Cx {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState> {
        if self.in_makepad_studio && !cfg!(target_os = "linux") {
            return self.game_input_remote.get(index);
        }
        self.hal.game.states.get(index)
    }

    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState> {
        if self.in_makepad_studio && !cfg!(target_os = "linux") {
            return self.game_input_remote.get_mut(index);
        }
        self.hal.game.states.get_mut(index)
    }

    fn game_input_states(&mut self) -> &[GameInputState] {
        // Apple/Windows hosted apps use forwarded input. Linux reads evdev in each
        // process, including hosted apps; each app owns its focus/muting policy.
        if self.in_makepad_studio && !cfg!(target_os = "linux") {
            return &self.game_input_remote;
        }
        &self.hal.game.states
    }

    fn game_input_states_mut(&mut self) -> &mut [GameInputState] {
        if self.in_makepad_studio && !cfg!(target_os = "linux") {
            return &mut self.game_input_remote;
        }
        &mut self.hal.game.states
    }

    fn game_input_infos(&mut self) -> Vec<GameInputInfo> {
        self.hal.game.infos.clone()
    }

    fn game_input_output(&mut self, id: LiveId) -> Option<GameInputOutput> {
        let device = self.hal.game.devices.iter().find(|d| d.id == id.0 && (d.vendor != 0 || d.output != game_output::NONE))?;
        let (vendor, product, session) = (device.vendor, device.product, self.hal.frames.w.session());
        Some(match device.output {
            // an output report goes to the HAL at once, from the thread of the app's force loop
            game_output::REPORTS => GameInputOutput::new(
                id,
                vendor,
                product,
                std::sync::Arc::new(move |report_id: u8, data: &[u8]| -> bool {
                    let mut w = makepad_hal::CmdWriter::new(session);
                    w.game_out(id.0, makepad_hal::media::game_op::REPORT, report_id as u32, 0, 0, 0, data);
                    makepad_hal::hal_submit(&w.buf());
                    true
                }),
            ),
            // the platform's force effects: each target the app sets goes to the HAL at once
            game_output::FORCE => GameInputOutput::with_force(
                id,
                vendor,
                product,
                GameInputForce::forwarding(std::sync::Arc::new(move |t: ForceTarget| {
                    let mut w = makepad_hal::CmdWriter::new(session);
                    w.game_out(id.0, makepad_hal::media::game_op::FORCE, t.constant.to_bits(), t.spring.to_bits(), t.damper.to_bits(), 0, &[]);
                    makepad_hal::hal_submit(&w.buf());
                })),
            ),
            // a device the app cannot drive: its identity only, writes fail
            _ => GameInputOutput::new(id, vendor, product, std::sync::Arc::new(|_, _| false)),
        })
    }

    fn gamepad_haptic_capabilities(&mut self, id: LiveId) -> GamepadHapticCapabilities {
        self.hal
            .game
            .devices
            .iter()
            .find(|d| d.id == id.0)
            .map(|d| GamepadHapticCapabilities { handles: d.haptics.handles, separate_handles: d.haptics.separate_handles })
            .unwrap_or_default()
    }

    fn gamepad_haptic_pulse(&mut self, id: LiveId, pulse: GamepadHapticPulse) -> bool {
        let handles = self.hal.game.devices.iter().any(|d| d.id == id.0 && d.haptics.handles);
        if handles {
            let f = f32::to_bits;
            self.hal.frames.w.game_out(id.0, makepad_hal::media::game_op::PULSE, f(pulse.left), f(pulse.right), f(pulse.sharpness), f(pulse.duration_s), &[]);
        }
        handles
    }
}
