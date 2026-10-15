//! Video the HAL's native player does not play: the app's media plug-in decodes it (playback
//! sessions, `MAKEPAD_FORCE_SOFTWARE_VIDEO`, a container AVFoundation refuses, a player that
//! produces no picture). The decoder is the app's, so it runs here, in the app: its YUV planes
//! go to textures like any image, and the Video widget gets the same events as from a native
//! player.
//!
//! Moved from the software half of os/apple/apple_video_player.rs (`AppleUnifiedVideoPlayer`);
//! the planes are uploaded through the frame builder's vec textures instead of Metal calls.

use crate::{
    cx::Cx,
    event::{video_playback::*, Event},
    makepad_live_id::LiveId,
    texture::{Texture, TextureFormat, TextureId, TextureUpdated},
    video_decode::software_video::PlaybackSessionHandle,
    video_decode::yuv::YuvPlaneData,
};
use makepad_hal::media;
use std::time::{Duration, Instant};

/// A native player that prepared but showed no picture for this long is given to the software
/// decoder (the 60 polls of the display-rate loop the AVPlayer path used).
const NATIVE_NO_FRAME: Duration = Duration::from_secs(1);

/// A video the software decoder plays.
pub(crate) struct SoftwareVideo {
    handle: PlaybackSessionHandle,
    planes: Option<[Texture; 3]>,
}

impl SoftwareVideo {
    pub(crate) fn cleanup(&mut self) {
        self.handle.cleanup();
    }
}

/// A video the HAL's native player plays, with what the fallback needs.
pub(crate) struct NativeVideo {
    pub(crate) texture: Option<TextureId>,
    pub(crate) source: VideoSource,
    pub(crate) autoplay: bool,
    pub(crate) looping: bool,
    /// when it said it was prepared, until its first picture
    pub(crate) waiting_since: Option<Instant>,
    /// its release was asked for to make room for the software decoder: the answer is not news
    pub(crate) swallow_release: bool,
}

fn vec_plane(width: usize, height: usize, data: Vec<u8>) -> TextureFormat {
    TextureFormat::VecRu8 { width, height, data: Some(data), unpack_row_length: None, updated: TextureUpdated::Full }
}

/// `rows` rows of `width` bytes taken out of `src` whose rows are `stride` apart.
fn tight(src: &[u8], width: usize, rows: usize, stride: usize) -> Vec<u8> {
    let stride = stride.max(width);
    if stride == width && src.len() >= width * rows {
        return src[..width * rows].to_vec();
    }
    let mut out = Vec::with_capacity(width * rows);
    for row in 0..rows {
        let start = row * stride;
        match src.get(start..start + width) {
            Some(r) => out.extend_from_slice(r),
            None => out.resize(out.len() + width, 0),
        }
    }
    out
}

impl Cx {
    /// Plays `source` with the app's decoder. `texture` is the video's own texture (unused by the
    /// YUV path).
    pub(crate) fn hal_start_software_video(&mut self, video_id: LiveId, texture: Option<TextureId>, source: VideoSource, autoplay: bool, looping: bool) {
        let handle = PlaybackSessionHandle::new(video_id, texture.unwrap_or_default(), source, autoplay, looping);
        let planes = texture.map(|_| {
            let planes = [(); 3].map(|_| Texture::new_with_format(self, vec_plane(1, 1, vec![0])));
            self.call_event_handler(&Event::VideoYuvTexturesReady(VideoYuvTexturesReady::planes(video_id, planes[0].clone(), planes[1].clone(), planes[2].clone())));
            planes
        });
        self.hal.software_videos.insert(video_id, SoftwareVideo { handle, planes });
    }

    /// The native player of `video_id` is given up: the software decoder takes over.
    pub(crate) fn hal_switch_to_software(&mut self, video_id: LiveId, reason: &str) {
        let Some(native) = self.hal.native_videos.get_mut(&video_id) else { return };
        crate::log!("VIDEO: native playback failed, falling back to the software video decoder: {}", reason);
        native.swallow_release = true;
        native.waiting_since = None;
        let (texture, source, autoplay, looping) = (native.texture, native.source.clone(), native.autoplay, native.looping);
        self.hal.frames.w.video_control(video_id.0, media::video_op::CLEANUP, 0);
        self.hal_start_software_video(video_id, texture, source, autoplay, looping);
    }

    /// A control op for a video the software decoder plays. False when it is not one.
    pub(crate) fn hal_software_video_control(&mut self, video_id: LiveId, op: u32, arg: u64) -> bool {
        if op == media::video_op::CLEANUP {
            if let Some(mut video) = self.hal.software_videos.remove(&video_id) {
                video.handle.cleanup();
                self.call_event_handler(&Event::VideoPlaybackResourcesReleased(VideoPlaybackResourcesReleasedEvent { video_id }));
                return true;
            }
            return false;
        }
        let Some(video) = self.hal.software_videos.get_mut(&video_id) else { return false };
        let h = &mut video.handle;
        match op {
            media::video_op::PLAY => h.play(),
            media::video_op::PAUSE => h.pause(),
            media::video_op::RESUME => h.resume(),
            media::video_op::MUTE => h.mute(),
            media::video_op::UNMUTE => h.unmute(),
            media::video_op::SEEK => h.seek_to(arg),
            media::video_op::VOLUME => h.set_volume(f32::from_bits(arg as u32) as f64),
            media::video_op::RATE => h.set_playback_rate(f32::from_bits(arg as u32) as f64),
            _ => {}
        }
        true
    }

    /// Whether a software player needs the loop to run.
    pub(crate) fn hal_software_video_active(&self) -> bool {
        !self.hal.software_videos.is_empty() || self.hal.native_videos.values().any(|n| n.waiting_since.is_some())
    }

    /// One poll of the software players (preparation, new pictures), and the native players that
    /// showed nothing for too long.
    pub(crate) fn hal_poll_software_videos(&mut self) {
        let late: Vec<LiveId> = self.hal.native_videos.iter().filter(|(_, n)| n.waiting_since.is_some_and(|t| t.elapsed() >= NATIVE_NO_FRAME) && !n.source.is_network_stream()).map(|(id, _)| *id).collect();
        for id in late {
            self.hal_switch_to_software(id, "the native player produced no frames");
        }
        let ids: Vec<LiveId> = self.hal.software_videos.keys().copied().collect();
        for video_id in ids {
            let mut events = Vec::new();
            let mut upload: Option<(YuvPlaneData, u128)> = None;
            if let Some(video) = self.hal.software_videos.get_mut(&video_id) {
                match video.handle.check_prepared() {
                    Some(Ok(p)) => {
                        events.push(Event::VideoPlaybackPrepared(VideoPlaybackPreparedEvent {
                            video_id,
                            video_width: p.width,
                            video_height: p.height,
                            duration: p.duration_ms,
                            is_seekable: p.is_seekable,
                            video_tracks: p.video_tracks,
                            audio_tracks: p.audio_tracks,
                        }));
                        let seekable = video.handle.seekable_ranges();
                        if !seekable.is_empty() {
                            events.push(Event::VideoSeekableRanges(VideoSeekableRangesEvent { video_id, ranges: seekable }));
                        }
                        let buffered = video.handle.buffered_ranges();
                        if !buffered.is_empty() {
                            events.push(Event::VideoBufferedRanges(VideoBufferedRangesEvent { video_id, ranges: buffered }));
                        }
                    }
                    Some(Err(error)) => events.push(Event::VideoDecodingError(VideoDecodingErrorEvent { video_id, error })),
                    None => {}
                }
                if video.handle.poll_frame() {
                    if let Some(planes) = video.handle.take_yuv_frame() {
                        upload = Some((planes, video.handle.current_position_ms()));
                    }
                }
            }
            for event in events {
                self.call_event_handler(&event);
            }
            if let Some((planes, position)) = upload {
                let Some(textures) = self.hal.software_videos.get(&video_id).and_then(|v| v.planes.clone()) else { continue };
                let (cw, ch) = planes.layout.chroma_size(planes.width, planes.height);
                let (w, h, cw, ch) = (planes.width as usize, planes.height as usize, cw as usize, ch as usize);
                let data = [
                    (tight(&planes.y, w, h, planes.y_bytes_per_row() as usize), w, h),
                    (tight(&planes.u, cw, ch, planes.u_bytes_per_row() as usize), cw, ch),
                    (tight(&planes.v, cw, ch, planes.v_bytes_per_row() as usize), cw, ch),
                ];
                for (texture, (bytes, width, height)) in textures.iter().zip(data) {
                    self.textures[texture.texture_id()].format = vec_plane(width.max(1), height.max(1), if bytes.is_empty() { vec![0] } else { bytes });
                }
                self.call_event_handler(&Event::VideoTextureUpdated(VideoTextureUpdatedEvent {
                    video_id,
                    current_position_ms: position,
                    yuv: VideoYuvMetadata { enabled: true, matrix: planes.matrix.as_f32(), biplanar: false, full_range: false, rotation_steps: 0.0, external: false, array: false },
                    rgba_gl_2d: false,
                }));
            }
        }
    }
}
