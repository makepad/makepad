//! The camera half of the HAL client: the camera list, the slots that read cameras, their
//! frame callbacks and the video encoders. The HAL runs the capture sessions and sends the
//! frames of the slots that want them (`CameraFrame`); what consumes a frame is the app's:
//! its callbacks and the encoders of its media plug-in, which are fed here.

use crate::{
    cx::Cx,
    event::Event,
    makepad_live_id::LiveId,
    texture::TextureId,
    video::*,
    video_encode::camera_video_encoder::VideoEncoder,
};
use makepad_hal::{media, HalEvent, Id};
use std::sync::{Arc, Mutex};

/// Event payload room for camera frames: a 1080p NV12 frame is 3 MB, a 4K BGRA readback 33 MB.
const FRAME_ARENA_BYTES: usize = 64 << 20;

type Slot<T> = Arc<Mutex<Option<T>>>;

/// A texture the app asked to have encoded, waiting for its pixels.
struct PendingCapture {
    slot: usize,
    timestamp_ns: u64,
}

pub(crate) struct CameraClient {
    descs: Vec<VideoInputDesc>,
    /// the HAL has listed the cameras at least once (an empty list is a list)
    listed: bool,
    /// the (camera, format) each slot reads
    active: Vec<(VideoInputId, VideoFormatId)>,
    /// slots opened with `video_input_open` that are not streaming: the HAL is not asked for their
    /// camera, so it stays closed and its light stays off
    idle: u32,
    video_cb: [Slot<VideoInputFn>; MAX_VIDEO_DEVICE_INDEX],
    frame_cb: [Slot<CameraFrameInputFn>; MAX_VIDEO_DEVICE_INDEX],
    output_cb: [Slot<VideoOutputFn>; MAX_VIDEO_DEVICE_INDEX],
    encoder_config: [Option<VideoEncoderConfig>; MAX_VIDEO_DEVICE_INDEX],
    encoder: [Slot<VideoEncoder>; MAX_VIDEO_DEVICE_INDEX],
    /// camera previews (videos of camera source): the camera and format each shows
    previews: std::collections::HashMap<LiveId, (VideoInputId, VideoFormatId)>,
    captures: std::collections::HashMap<u32, PendingCapture>,
}

impl CameraClient {
    fn open_slot(&mut self, slot: usize, input: VideoInputId, format: VideoFormatId) -> Result<VideoFormat, String> {
        if slot >= MAX_VIDEO_DEVICE_INDEX {
            return Err(format!("camera slot {slot} is out of range"));
        }
        let Some(desc) = self.descs.iter().find(|d| d.input_id == input) else {
            return Err("that camera is not connected any more".into());
        };
        let Some(found) = desc.formats.iter().find(|f| f.format_id == format).copied() else {
            return Err(format!("{} does not offer that format", desc.name));
        };
        while self.active.len() <= slot {
            self.active.push((VideoInputId(LiveId(0)), VideoFormatId(LiveId(0))));
        }
        self.active[slot] = (input, format);
        self.idle |= 1 << slot;
        Ok(found)
    }

    fn stream_slot(&mut self, slot: usize, on: bool) -> Result<(), String> {
        if !(slot < self.active.len() && self.active[slot].0 .0 != LiveId(0)) {
            return Err("no camera is open in this slot".into());
        }
        if on {
            self.idle &= !(1 << slot);
        } else {
            self.idle |= 1 << slot;
        }
        Ok(())
    }

    fn close_slot(&mut self, slot: usize) {
        if slot < self.active.len() {
            self.active[slot] = (VideoInputId(LiveId(0)), VideoFormatId(LiveId(0)));
        }
        self.idle &= !(1 << slot);
        while self.active.last().is_some_and(|(i, _)| i.0 == LiveId(0)) {
            self.active.pop();
        }
    }
}

impl Default for CameraClient {
    fn default() -> Self {
        Self {
            descs: Vec::new(),
            listed: false,
            active: Vec::new(),
            idle: 0,
            video_cb: Default::default(),
            frame_cb: Default::default(),
            output_cb: Default::default(),
            encoder_config: [None; MAX_VIDEO_DEVICE_INDEX],
            encoder: Default::default(),
            previews: Default::default(),
            captures: Default::default(),
        }
    }
}

/// The cameras the HAL is asked to hold, by slot: a slot that is only open (a slide that is next)
/// names no camera (id 0 matches none), so the device stays closed and its light off; a camera
/// nobody names any more is released by the HAL at once.
fn slot_pairs(active: &[(VideoInputId, VideoFormatId)], idle: u32) -> Vec<u64> {
    active.iter().enumerate().flat_map(|(slot, (i, f))| if idle & (1 << slot) != 0 { [0, 0] } else { [i.0 .0, f.0 .0] }).collect()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Cx {
    /// The slots that want frames: a callback, or a camera-source encoder.
    fn hal_camera_consumers(&self) -> u32 {
        let c = &self.hal.camera;
        (0..MAX_VIDEO_DEVICE_INDEX).fold(0u32, |mask, i| {
            let wants = lock(&c.video_cb[i]).is_some() || lock(&c.frame_cb[i]).is_some() || lock(&c.encoder[i]).is_some();
            if wants { mask | 1 << i } else { mask }
        })
    }

    /// Tells the HAL which cameras the slots read and which slots want frames.
    pub(crate) fn hal_camera_sync(&mut self) {
        let consumers = self.hal_camera_consumers();
        let ids = slot_pairs(&self.hal.camera.active, self.hal.camera.idle);
        self.hal.frames.w.video_input(consumers, &media::pack_u64s(&ids));
        if consumers != 0 {
            // camera frames are megabytes: the payload room of events grows once
            self.hal.event_arena_min = self.hal.event_arena_min.max(FRAME_ARENA_BYTES);
        }
    }

    pub(crate) fn hal_video_inputs(&self) -> Option<Vec<VideoInputDesc>> {
        self.hal.camera.listed.then(|| self.hal.camera.descs.clone())
    }

    fn hal_format_of(&self, slot: usize) -> Option<VideoFormat> {
        let (input, format) = *self.hal.camera.active.get(slot)?;
        self.hal.camera.descs.iter().find(|d| d.input_id == input)?.formats.iter().find(|f| f.format_id == format).copied()
    }

    /// (Re)starts the encoder of a slot whose config reads a camera, once the camera's format is known.
    fn hal_refresh_slot_encoder(&mut self, index: usize) {
        let config = self.hal.camera.encoder_config[index];
        let Some(mut config) = config else {
            *lock(&self.hal.camera.encoder[index]) = None;
            return;
        };
        if !matches!(config.source, VideoEncodeSource::Camera { .. }) {
            return;
        }
        let Some(format) = self.hal_format_of(index) else {
            *lock(&self.hal.camera.encoder[index]) = None;
            return;
        };
        if lock(&self.hal.camera.output_cb[index]).is_none() {
            *lock(&self.hal.camera.encoder[index]) = None;
            return;
        }
        let (input, format_id) = self.hal.camera.active[index];
        config.width = format.width as u32;
        config.height = format.height as u32;
        if let Some(fps) = format.frame_rate {
            config.fps_num = fps.max(1.0).round() as u32;
            config.fps_den = 1;
        }
        config.source = VideoEncodeSource::Camera { input_id: input, format_id };
        let output_cb = self.hal.camera.output_cb[index].clone();
        *lock(&self.hal.camera.encoder[index]) = VideoEncoder::start(
            config,
            Box::new(move |packet| {
                if let Some(cb) = &mut *lock(&output_cb) {
                    cb(packet);
                }
            }),
        );
    }

    /// The video `video_id` (a camera preview) is released: the camera it ran stops.
    pub(crate) fn hal_release_camera_preview(&mut self, video_id: LiveId) {
        if self.hal.camera.previews.remove(&video_id).is_some() {
            self.hal.camera.active.clear();
        }
    }

    /// Holds the camera `input` for `slot` at `format` without streaming it. The format is the one
    /// the camera list offers; the HAL negotiates the device's own at the first stream, and every
    /// frame carries the size it has.
    pub(crate) fn hal_video_input_open(&mut self, slot: usize, input: VideoInputId, format: VideoFormatId) -> Result<VideoFormat, String> {
        let found = self.hal.camera.open_slot(slot, input, format)?;
        crate::log!("camera slot {slot}: held at {}x{} {:?} {:?} fps, not streaming", found.width, found.height, found.pixel_format, found.frame_rate);
        self.hal_camera_sync();
        Ok(found)
    }

    /// Streams (or stops) the camera held in `slot`. Frames of a stopped slot are dropped on arrival,
    /// so none reaches the callback after this returns.
    pub(crate) fn hal_video_input_stream(&mut self, slot: usize, on: bool) -> Result<(), String> {
        self.hal.camera.stream_slot(slot, on)?;
        crate::log!("camera slot {slot}: {} (the HAL is asked {})", if on { "stream on" } else { "stream off" }, if on { "to open the device" } else { "to release the device" });
        self.hal_camera_sync();
        Ok(())
    }

    pub(crate) fn hal_video_input_close(&mut self, slot: usize) {
        self.hal.camera.close_slot(slot);
        crate::log!("camera slot {slot}: closed");
        self.hal_camera_sync();
    }

    pub(crate) fn hal_use_video_input(&mut self, devices: &[(VideoInputId, VideoFormatId)]) {
        self.hal.camera.idle = 0;
        self.hal.camera.active = devices.to_vec();
        for index in 0..MAX_VIDEO_DEVICE_INDEX {
            self.hal_refresh_slot_encoder(index);
        }
        self.hal_camera_sync();
    }

    pub(crate) fn hal_video_input_box(&mut self, index: usize, f: VideoInputFn) {
        *lock(&self.hal.camera.video_cb[index]) = Some(f);
        self.hal_camera_sync();
    }

    pub(crate) fn hal_camera_frame_input_box(&mut self, index: usize, f: CameraFrameInputFn) {
        *lock(&self.hal.camera.frame_cb[index]) = Some(f);
        self.hal_camera_sync();
    }

    pub(crate) fn hal_video_encoder_output_box(&mut self, index: usize, mut config: VideoEncoderConfig, f: VideoOutputFn) -> Result<(), VideoEncodeError> {
        if let VideoEncodeSource::Camera { mut input_id, mut format_id } = config.source {
            if let Some((active_input, active_format)) = self.hal.camera.active.get(index).copied() {
                input_id = active_input;
                format_id = active_format;
            }
            config.source = VideoEncodeSource::Camera { input_id, format_id };
        }
        let c = &mut self.hal.camera;
        *lock(&c.output_cb[index]) = Some(f);
        c.encoder_config[index] = Some(config);
        if matches!(config.source, VideoEncodeSource::Camera { .. }) {
            self.hal_refresh_slot_encoder(index);
            self.hal_camera_sync();
            return Ok(());
        }
        let c = &self.hal.camera;
        *lock(&c.encoder[index]) = None;
        let output_cb = c.output_cb[index].clone();
        *lock(&c.encoder[index]) = VideoEncoder::start(
            config,
            Box::new(move |packet| {
                if let Some(cb) = &mut *lock(&output_cb) {
                    cb(packet);
                }
            }),
        );
        if lock(&c.encoder[index]).is_none() {
            crate::error!("video encoder unavailable for slot {}", index);
            return Err(VideoEncodeError::CodecUnavailable);
        }
        Ok(())
    }

    pub(crate) fn hal_video_encoder_push_frame(&mut self, index: usize, frame: CameraFrameRef<'_>) {
        if let Some(encoder) = &*lock(&self.hal.camera.encoder[index]) {
            encoder.push_frame(frame);
        }
    }

    pub(crate) fn hal_video_encoder_request_keyframe(&mut self, index: usize) -> Result<(), VideoEncodeError> {
        let guard = lock(&self.hal.camera.encoder[index]);
        let encoder = guard.as_ref().ok_or(VideoEncodeError::EncoderNotStarted)?;
        encoder.request_keyframe()
    }

    /// Asks the HAL for the pixels of the texture an encoder reads; the frame goes to the encoder
    /// when they arrive (`Readback`), so the call returns once the request is made.
    pub(crate) fn hal_video_encoder_capture_texture_frame(&mut self, index: usize, timestamp_ns: u64) -> Result<(), VideoEncodeError> {
        let config = self.hal.camera.encoder_config[index].ok_or(VideoEncodeError::EncoderNotStarted)?;
        let texture_id = match config.source {
            VideoEncodeSource::Texture { texture_id } => texture_id,
            _ => return Err(VideoEncodeError::UnsupportedSource),
        };
        if lock(&self.hal.camera.encoder[index]).is_none() {
            return Err(VideoEncodeError::EncoderNotStarted);
        }
        let texture = &self.textures[texture_id];
        let alloc = texture.alloc.as_ref().ok_or(VideoEncodeError::InvalidTexture)?;
        if alloc.width == 0 || alloc.height == 0 {
            return Err(VideoEncodeError::InvalidTextureSize);
        }
        if alloc.pixel != crate::texture::TexturePixel::BGRAu8 {
            return Err(VideoEncodeError::UnsupportedTextureFormat);
        }
        let (w, h) = (alloc.width as u32, alloc.height as u32);
        let Some(hal_texture) = self.hal_texture_of(texture_id) else { return Err(VideoEncodeError::InvalidTexture) };
        let req = self.hal_readback_request();
        self.hal.camera.captures.insert(req, PendingCapture { slot: index, timestamp_ns });
        self.hal.event_arena_min = self.hal.event_arena_min.max(FRAME_ARENA_BYTES);
        self.hal.frames.w.texture_readback(hal_texture, req, [0, 0, w, h]);
        Ok(())
    }

    /// The HAL's id of a pool texture, when the HAL holds one.
    fn hal_texture_of(&self, id: TextureId) -> Option<Id> {
        self.hal.frames.texture_hal_id(id.0, id.generation())
    }

    /// A camera preview of a camera (the video is of camera source): the camera runs for it only.
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn hal_prepare_camera_preview(&mut self, video_id: LiveId, input: VideoInputId, format: VideoFormatId) {
        // the preview overrides slot 0: its frame callbacks are cleared
        *lock(&self.hal.camera.frame_cb[0]) = None;
        *lock(&self.hal.camera.video_cb[0]) = None;
        self.hal.camera.previews.insert(video_id, (input, format));
        let ids = media::pack_u64s(&[input.0 .0, format.0 .0]);
        self.hal.frames.w.video_prepare(Id::NONE, media::video_mode::CAMERA, video_id.0, Id::NONE, Id::NONE, Id::NONE, &ids);
    }

    /// The preview ops: attach / update / detach of the native view over the window.
    pub(crate) fn hal_camera_preview(&mut self, op: crate::cx_api::CxOsOp) {
        use crate::cx_api::CxOsOp;
        use media::{view_kind::CAMERA, view_op};
        let (video_id, area, visible, detach) = match op {
            CxOsOp::AttachCameraNativePreview { video_id, area } => (video_id, Some(area), true, false),
            CxOsOp::UpdateCameraNativePreview { video_id, area, visible } => (video_id, Some(area), visible, false),
            CxOsOp::DetachCameraNativePreview { video_id } => (video_id, None, false, true),
            _ => return,
        };
        if detach {
            self.hal.frames.w.view_op(Id::NONE, CAMERA | view_op::DETACH, video_id.0, [0.0; 4], 0, &[]);
            return;
        }
        let Some(area) = area else { return };
        let Some((window_id, _, clipped)) = self.hal_view_rects(&area) else { return };
        if !self.hal.camera.previews.contains_key(&video_id) {
            return;
        }
        let rect = [clipped.pos.x as f32, clipped.pos.y as f32, clipped.size.x as f32, clipped.size.y as f32];
        let window = crate::event::hal_intake::hal_window_id(window_id);
        // the HAL knows which camera the preview shows (it was prepared with it)
        self.hal.frames.w.view_op(window, CAMERA | view_op::UPDATE, video_id.0, rect, visible as u32, &[]);
    }

    /// The camera events of the HAL.
    pub(crate) fn hal_camera_event(&mut self, event: HalEvent<'_>) {
        match event {
            HalEvent::VideoInputs { inputs } => {
                let descs: Vec<VideoInputDesc> = media::read_video_inputs(inputs)
                    .into_iter()
                    .map(|i| VideoInputDesc {
                        input_id: VideoInputId(LiveId(i.id)),
                        name: i.name,
                        sensor_orientation: i.sensor_orientation,
                        formats: i
                            .formats
                            .into_iter()
                            .map(|f| VideoFormat {
                                format_id: VideoFormatId(LiveId(f.id)),
                                width: f.width as usize,
                                height: f.height as usize,
                                frame_rate: f.frame_rate,
                                pixel_format: match f.pixel {
                                    media::VideoPixel::RGB24 => VideoPixelFormat::RGB24,
                                    media::VideoPixel::YUY2 => VideoPixelFormat::YUY2,
                                    media::VideoPixel::NV12 => VideoPixelFormat::NV12,
                                    media::VideoPixel::YUV420 => VideoPixelFormat::YUV420,
                                    media::VideoPixel::GRAY => VideoPixelFormat::GRAY,
                                    media::VideoPixel::MJPEG => VideoPixelFormat::MJPEG,
                                    media::VideoPixel::Unsupported(f) => VideoPixelFormat::Unsupported(f),
                                },
                            })
                            .collect(),
                    })
                    .collect();
                crate::log!("cameras listed: {}", if descs.is_empty() { "none".to_string() } else { descs.iter().map(|d| format!("{} ({} formats)", d.name, d.formats.len())).collect::<Vec<_>>().join(", ") });
                self.hal.camera.descs = descs.clone();
                self.hal.camera.listed = true;
                // slot encoders wait for the format sizes
                for index in 0..MAX_VIDEO_DEVICE_INDEX {
                    self.hal_refresh_slot_encoder(index);
                }
                self.call_event_handler(&Event::VideoInputs(VideoInputsEvent { descs }));
            }
            HalEvent::CameraFrame { slots, frame } => {
                let slots = slots & !self.hal.camera.idle;
                if slots == 0 {
                    return;
                }
                let Some(data) = media::read_camera_frame(frame) else { return };
                // The platform's display rotation rides on its frames: what
                // `camera_upright_quarter_turns` turns them upright by.
                set_display_rotation(data.display_rotation as u32);
                let layout = match data.layout {
                    media::camera_layout::I420 => CameraFrameLayout::I420,
                    media::camera_layout::NV12 => CameraFrameLayout::NV12,
                    media::camera_layout::YUY2 => CameraFrameLayout::YUY2,
                    media::camera_layout::MJPEG => CameraFrameLayout::Mjpeg,
                    _ => CameraFrameLayout::Unknown,
                };
                let plane = |i: usize| match data.planes.get(i) {
                    Some(p) => CameraFramePlaneRef { bytes: p.bytes, row_stride: p.row_stride as usize, pixel_stride: p.pixel_stride as usize },
                    None => CameraFramePlaneRef::empty(),
                };
                let frame_ref = CameraFrameRef {
                    timestamp_ns: data.timestamp_ns,
                    width: data.width as usize,
                    height: data.height as usize,
                    layout,
                    matrix: match data.matrix { 0 => CameraColorMatrix::BT709, 1 => CameraColorMatrix::BT601, 2 => CameraColorMatrix::BT2020, _ => CameraColorMatrix::Unknown },
                    plane_count: data.planes.len(),
                    planes: [plane(0), plane(1), plane(2)],
                };
                // Raw callbacks receive the native contiguous planes (including row padding).
                // Make the aligned u32 view once per packet, shared by every selected slot.
                let raw_words = if data.layout != media::camera_layout::MJPEG
                    && (0..MAX_VIDEO_DEVICE_INDEX).any(|slot| slots & (1 << slot) != 0 && lock(&self.hal.camera.video_cb[slot]).is_some()) {
                    let count = data.planes.iter().map(|p| p.bytes.len()).sum::<usize>() / 4;
                    let mut words = Vec::with_capacity(count);
                    let mut bytes = data.planes.iter().flat_map(|p| p.bytes.iter().copied());
                    while let (Some(a), Some(b), Some(c), Some(d)) = (bytes.next(), bytes.next(), bytes.next(), bytes.next()) {
                        words.push(u32::from_ne_bytes([a, b, c, d]));
                    }
                    Some(words)
                } else { None };
                for slot in (0..MAX_VIDEO_DEVICE_INDEX).filter(|i| slots & (1 << i) != 0) {
                    if layout != CameraFrameLayout::Unknown {
                        if let Some(cb) = &mut *lock(&self.hal.camera.frame_cb[slot]) {
                            cb(frame_ref);
                        }
                        if let Some(encoder) = &*lock(&self.hal.camera.encoder[slot]) {
                            encoder.push_frame(frame_ref);
                        }
                    }
                    if let Some(mut format) = self.hal_format_of(slot) {
                        format.width = data.width as usize;
                        format.height = data.height as usize;
                        format.pixel_format = match data.layout {
                            media::camera_layout::I420 => VideoPixelFormat::YUV420,
                            media::camera_layout::NV12 => VideoPixelFormat::NV12,
                            media::camera_layout::YUY2 => VideoPixelFormat::YUY2,
                            media::camera_layout::MJPEG => VideoPixelFormat::MJPEG,
                            media::camera_layout::RGB24 => VideoPixelFormat::RGB24,
                            media::camera_layout::GRAY => VideoPixelFormat::GRAY,
                            _ => format.pixel_format,
                        };
                        if let Some(cb) = &mut *lock(&self.hal.camera.video_cb[slot]) {
                            if let Some(words) = &raw_words {
                                cb(VideoBufferRef { format, data: VideoBufferRefData::U32(words) });
                            } else if data.layout == media::camera_layout::MJPEG {
                                cb(VideoBufferRef { format, data: VideoBufferRefData::U8(frame_ref.planes[0].bytes) });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The pixels of a texture an encoder reads arrived: converted and pushed to the encoder.
    /// False when the request is not one of the camera client's.
    pub(crate) fn hal_encoder_readback(&mut self, req: u32, status: u32, rect: [u32; 4], pixels: &[u8]) -> bool {
        let Some(pending) = self.hal.camera.captures.remove(&req) else { return false };
        if status != makepad_hal::readback::OK {
            return true;
        }
        let mut frame = CameraFrameOwned::default();
        if !convert_rgba_8888_to_i420(pixels, rect[2] as usize, rect[3] as usize, pending.timestamp_ns, CameraColorMatrix::BT709, &mut frame) {
            return true;
        }
        let plane = |i: usize| CameraFramePlaneRef { bytes: &frame.planes[i].bytes, row_stride: frame.planes[i].row_stride, pixel_stride: frame.planes[i].pixel_stride };
        let frame_ref = CameraFrameRef {
            timestamp_ns: frame.timestamp_ns,
            width: frame.width,
            height: frame.height,
            layout: frame.layout,
            matrix: frame.matrix,
            plane_count: frame.plane_count,
            planes: [plane(0), plane(1), plane(2)],
        };
        if let Some(encoder) = &*lock(&self.hal.camera.encoder[pending.slot]) {
            encoder.push_frame(frame_ref);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(n: u64) -> (VideoInputId, VideoFormatId) {
        (VideoInputId(LiveId(n)), VideoFormatId(LiveId(n + 100)))
    }

    #[test]
    fn entering_the_slide_again_asks_for_the_device_again_five_times() {
        use crate::video::{VideoFormat, VideoInputDesc, VideoPixelFormat};
        let format = VideoFormat { format_id: VideoFormatId(LiveId(107)), width: 1920, height: 1080, frame_rate: Some(30.0), pixel_format: VideoPixelFormat::MJPEG };
        let mut c = CameraClient::default();
        c.descs = vec![VideoInputDesc { input_id: VideoInputId(LiveId(7)), name: "OBSBOT".into(), sensor_orientation: 0, formats: vec![format] }];
        let none = 0;
        for round in 0..5 {
            // enter: the next-slide pre-warm opens the slot, the device stays closed
            c.open_slot(0, VideoInputId(LiveId(7)), VideoFormatId(LiveId(107))).unwrap();
            assert_eq!(slot_pairs(&c.active, c.idle), [none, none], "round {round}");
            // shown: the device is asked for
            c.stream_slot(0, true).unwrap();
            assert_eq!(slot_pairs(&c.active, c.idle), [7, 107], "round {round}");
            // leave: released, then the slot closes
            c.stream_slot(0, false).unwrap();
            assert_eq!(slot_pairs(&c.active, c.idle), [none, none]);
            // a close still pending when the next enter arrives: the slot is opened again over it
            if round % 2 == 0 {
                c.close_slot(0);
                assert!(slot_pairs(&c.active, c.idle).is_empty());
            } else {
                c.open_slot(0, VideoInputId(LiveId(7)), VideoFormatId(LiveId(107))).unwrap();
                c.stream_slot(0, true).unwrap();
                assert_eq!(slot_pairs(&c.active, c.idle), [7, 107]);
                c.close_slot(0);
            }
        }
        assert!(c.stream_slot(0, true).is_err(), "a closed slot cannot stream");
    }

    #[test]
    fn a_camera_is_held_only_while_its_slide_shows() {
        let none = LiveId(0).0;
        // the next slide's camera is open but idle: the HAL is asked for nothing
        assert_eq!(slot_pairs(&[pair(7)], 0b1), [none, none]);
        // its slide shows: the device is asked for
        assert_eq!(slot_pairs(&[pair(7)], 0), [7, 107]);
        // the slide moves on beyond next: the slot is idle (or closed), the device is released
        assert_eq!(slot_pairs(&[pair(7)], 0b1), [none, none]);
        assert!(slot_pairs(&[], 0).is_empty());
        // presenter and output show the same camera in two slots: both name it, the HAL opens it once;
        // one leaving keeps the other's
        assert_eq!(slot_pairs(&[pair(7), pair(7)], 0), [7, 107, 7, 107]);
        assert_eq!(slot_pairs(&[pair(7), pair(7)], 0b01), [none, none, 7, 107]);
    }
}
