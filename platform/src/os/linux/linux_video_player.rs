//! Unified video player for Linux that wraps GStreamer native player,
//! software rav1d fallback, and V4L2 camera capture.

use {
    super::gl_sys::LibGl,
    super::gl_video_upload::upload_yuv_to_gl,
    super::gstreamer_sys::LibGStreamer,
    super::linux_video_gpu::{
        present_dmabuf_nv12, present_gl_memory_rgba, LinuxDmabufPresentCache,
        LinuxGlMemoryPresentCache, LinuxGlTextureTarget,
    },
    super::linux_video_playback::{
        poll_pending_gstreamer_teardowns, GStreamerVideoPlayer, GstRuntimeEvent, YuvTextureIds,
    },
    super::opengl_cx::OpenglCx,
    super::v4l2_camera_player::V4l2CameraPlayer,
    crate::{
        event::{
            video_playback::{
                VideoBufferedRangesEvent, VideoDecodingErrorEvent, VideoPlaybackCompletedEvent,
                VideoPlaybackPreparedEvent, VideoSeekableRangesEvent, VideoSource,
                VideoTextureUpdatedEvent, VideoTracksChangedEvent,
            },
            Event,
        },
        makepad_live_id::LiveId,
        media_plugin::PlaybackPrepared,
        texture::{CxTexturePool, Texture, TextureFormat, TextureId},
        video_decode::software_video::PlaybackSessionHandle,
    },
};

#[derive(Clone)]
pub struct YuvTextureSet {
    pub tex_y: Texture,
    pub tex_u: Texture,
    pub tex_v: Texture,
    pub tex_y_oes: Option<Texture>,
    pub tex_u_oes: Option<Texture>,
    pub ids: YuvTextureIds,
}

impl YuvTextureSet {
    pub fn new(tex_y: Texture, tex_u: Texture, tex_v: Texture) -> Self {
        Self {
            ids: YuvTextureIds {
                tex_y_id: tex_y.texture_id(),
                tex_u_id: tex_u.texture_id(),
                tex_v_id: tex_v.texture_id(),
                tex_y_oes_id: None,
                tex_u_oes_id: None,
            },
            tex_y,
            tex_u,
            tex_v,
            tex_y_oes: None,
            tex_u_oes: None,
        }
    }

    pub fn with_oes(mut self, tex_y_oes: Texture, tex_u_oes: Texture) -> Self {
        self.ids.tex_y_oes_id = Some(tex_y_oes.texture_id());
        self.ids.tex_u_oes_id = Some(tex_u_oes.texture_id());
        self.tex_y_oes = Some(tex_y_oes);
        self.tex_u_oes = Some(tex_u_oes);
        self
    }
}

/// Which decoder backend to use for a non-camera Linux video source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxDecodeBackend {
    GStreamer,
    Software,
}

/// Choose GStreamer vs software, refusing software for HLS/DASH manifests.
pub fn choose_linux_decode_backend(
    source: &VideoSource,
    gstreamer_available: bool,
    force_software: bool,
) -> Result<LinuxDecodeBackend, String> {
    if force_software {
        if source.is_adaptive_manifest() {
            return Err(
                "MAKEPAD_FORCE_SOFTWARE_VIDEO cannot decode HLS/DASH manifests; unset it or use a progressive URL"
                    .into(),
            );
        }
        return Ok(LinuxDecodeBackend::Software);
    }
    if source.is_session() {
        return Ok(LinuxDecodeBackend::Software);
    }
    if gstreamer_available {
        return Ok(LinuxDecodeBackend::GStreamer);
    }
    if source.is_adaptive_manifest() {
        return Err(
            "GStreamer is required for HLS/DASH playback on Linux. Install packages from tools/linux_deps.sh (gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav)."
                .into(),
        );
    }
    Ok(LinuxDecodeBackend::Software)
}

/// Outcome of preparing a non-camera Linux video source.
pub enum LinuxPrepareResult {
    Ready {
        player: LinuxVideoPlayer,
        yuv: Option<YuvTextureSet>,
    },
    Failed(String),
}

/// Shared X11/Wayland prepare path for file/network/session video sources.
pub fn prepare_desktop_linux_video(
    gstreamer: &mut Option<LibGStreamer>,
    textures: &mut CxTexturePool,
    video_id: LiveId,
    source: VideoSource,
    texture_id: TextureId,
    autoplay: bool,
    should_loop: bool,
    opengl_cx: Option<&super::opengl_cx::OpenglCx>,
) -> LinuxPrepareResult {
    let force_software = std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some();
    if force_software {
        crate::log!("VIDEO: MAKEPAD_FORCE_SOFTWARE_VIDEO set, using software video decoder");
    } else if source.is_session() {
        crate::log!("VIDEO: session source uses software video decoder");
    }

    if gstreamer.is_none() {
        match LibGStreamer::try_load() {
            Some(gst) => {
                gst.init();
                *gstreamer = Some(gst);
            }
            None => crate::log!("VIDEO: GStreamer not available"),
        }
    }

    let gst_available = gstreamer.is_some();
    let backend = match choose_linux_decode_backend(&source, gst_available, force_software) {
        Ok(b) => b,
        Err(error) => return LinuxPrepareResult::Failed(error),
    };

    let mut use_software = backend == LinuxDecodeBackend::Software;
    if !use_software {
        if let Some(gst) = gstreamer.as_ref() {
            let yuv = YuvTextureSet::new(
                textures.alloc(TextureFormat::VideoYuvPlane),
                textures.alloc(TextureFormat::VideoYuvPlane),
                textures.alloc(TextureFormat::VideoYuvPlane),
            )
            .with_oes(
                textures.alloc(TextureFormat::VideoExternal),
                textures.alloc(TextureFormat::VideoExternal),
            );
            let player = GStreamerVideoPlayer::new(
                gst,
                video_id,
                texture_id,
                Some(yuv.ids),
                source.clone(),
                autoplay,
                should_loop,
                opengl_cx,
            );
            if player.is_active() {
                return LinuxPrepareResult::Ready {
                    player: LinuxVideoPlayer::GStreamer {
                        player,
                        yuv: Some(yuv.clone()),
                    },
                    yuv: Some(yuv),
                };
            }
            if source.is_adaptive_manifest() {
                return LinuxPrepareResult::Failed(
                    "GStreamer failed to open HLS/DASH stream. Install packages from tools/linux_deps.sh (gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav)."
                        .into(),
                );
            }
            crate::log!(
                "VIDEO: GStreamer pipeline failed, falling back to software video decoder"
            );
            use_software = true;
        }
    }

    if use_software {
        // Allocate OES Y/UV slots so MediaPlugin DMA-Buf NV12 zero-copy can present
        // without a reopen (same layout as the GStreamer path).
        let yuv = YuvTextureSet::new(
            textures.alloc(TextureFormat::VideoYuvPlane),
            textures.alloc(TextureFormat::VideoYuvPlane),
            textures.alloc(TextureFormat::VideoYuvPlane),
        )
        .with_oes(
            textures.alloc(TextureFormat::VideoExternal),
            textures.alloc(TextureFormat::VideoExternal),
        );
        let player = PlaybackSessionHandle::new(
            video_id,
            texture_id,
            source,
            autoplay,
            should_loop,
        );
        return LinuxPrepareResult::Ready {
            player: LinuxVideoPlayer::Software {
                player,
                yuv: yuv.clone(),
                texture_id,
                yuv_matrix: 0.0,
                yuv_biplanar: false,
                yuv_external: false,
                rgba_gl_2d: false,
                dmabuf_cache: LinuxDmabufPresentCache::default(),
                gl_memory_cache: LinuxGlMemoryPresentCache::default(),
            },
            yuv: Some(yuv),
        };
    }

    LinuxPrepareResult::Failed("No video backend available".into())
}

pub enum LinuxVideoPlayer {
    GStreamer {
        player: GStreamerVideoPlayer,
        yuv: Option<YuvTextureSet>,
    },
    Software {
        player: PlaybackSessionHandle,
        yuv: YuvTextureSet,
        /// Main `Video` texture (RGBA / GLMemory adopt target).
        texture_id: TextureId,
        yuv_matrix: f32,
        yuv_biplanar: bool,
        yuv_external: bool,
        rgba_gl_2d: bool,
        dmabuf_cache: LinuxDmabufPresentCache,
        gl_memory_cache: LinuxGlMemoryPresentCache,
    },
    Camera(V4l2CameraPlayer),
}

impl LinuxVideoPlayer {
    pub fn video_id(&self) -> LiveId {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.video_id,
            LinuxVideoPlayer::Software { player: p, .. } => p.video_id,
            LinuxVideoPlayer::Camera(p) => p.video_id,
        }
    }

    pub fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.check_prepared(),
            LinuxVideoPlayer::Software { player: p, .. } => p.check_prepared(),
            LinuxVideoPlayer::Camera(p) => p.check_prepared(),
        }
    }

    pub fn poll_frame(
        &mut self,
        gl: &LibGl,
        textures: &mut CxTexturePool,
        opengl_cx: Option<&super::opengl_cx::OpenglCx>,
    ) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => {
                p.poll_frame(gl, textures, opengl_cx)
            }
            LinuxVideoPlayer::Software {
                player: p,
                yuv,
                texture_id,
                yuv_matrix,
                yuv_biplanar,
                yuv_external,
                rgba_gl_2d,
                dmabuf_cache,
                gl_memory_cache,
            } => {
                if !p.poll_frame() {
                    return false;
                }

                // Prefer DMA-Buf NV12 zero-copy when the MediaPlugin supplies it.
                if let (Some(y_oes), Some(u_oes), Some(cx)) = (
                    yuv.ids.tex_y_oes_id,
                    yuv.ids.tex_u_oes_id,
                    opengl_cx,
                ) {
                    if let Some(gpu) = p.take_linux_dmabuf_nv12_frame() {
                        match present_dmabuf_nv12(
                            cx,
                            gl,
                            textures,
                            y_oes,
                            u_oes,
                            &gpu,
                            dmabuf_cache,
                        ) {
                            Ok(()) => {
                                *yuv_matrix = gpu.matrix.as_f32();
                                *yuv_biplanar = true;
                                *yuv_external = true;
                                *rgba_gl_2d = false;
                                return true;
                            }
                            Err(err) => {
                                crate::error!("VIDEO: MediaPlugin DMA-Buf NV12 present failed: {err}");
                            }
                        }
                    }
                }

                // GStreamer GLMemory / share-group RGBA texture.
                if let Some(gpu) = p.take_linux_gl_memory_rgba_frame() {
                    match present_gl_memory_rgba(gl, textures, *texture_id, &gpu, gl_memory_cache)
                    {
                        Ok(()) => {
                            *yuv_external = false;
                            *yuv_biplanar = false;
                            *rgba_gl_2d = matches!(gpu.target, LinuxGlTextureTarget::Texture2D);
                            // YUV disabled — RGBA external path.
                            *yuv_matrix = 0.0;
                            return true;
                        }
                        Err(err) => {
                            crate::error!("VIDEO: MediaPlugin GLMemory present failed: {err}");
                        }
                    }
                }

                if let Some(planes) = p.take_yuv_frame() {
                    *yuv_matrix = planes.matrix.as_f32();
                    *yuv_biplanar = false;
                    *yuv_external = false;
                    *rgba_gl_2d = false;
                    upload_yuv_to_gl(
                        gl,
                        textures,
                        yuv.ids.tex_y_id,
                        yuv.ids.tex_u_id,
                        yuv.ids.tex_v_id,
                        &planes,
                    );
                    true
                } else {
                    false
                }
            }
            LinuxVideoPlayer::Camera(p) => p.poll_frame(gl, textures),
        }
    }

    pub fn select_video_track(&mut self, index: usize) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.select_video_track(index),
            _ => false,
        }
    }

    pub fn select_audio_track(&mut self, index: usize) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.select_audio_track(index),
            _ => false,
        }
    }

    pub fn check_eos(&mut self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.check_eos(),
            LinuxVideoPlayer::Software { player: p, .. } => p.check_eos(),
            LinuxVideoPlayer::Camera(_) => false, // camera never ends
        }
    }

    pub fn poll_runtime(&mut self) -> Vec<GstRuntimeEvent> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.poll_runtime(),
            _ => Vec::new(),
        }
    }

    pub fn take_buffered_ranges_if_changed(&mut self) -> Option<Vec<(f64, f64)>> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.take_buffered_ranges_if_changed(),
            LinuxVideoPlayer::Software { .. } => None,
            LinuxVideoPlayer::Camera(_) => None,
        }
    }

    pub fn is_active(&self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.is_active(),
            LinuxVideoPlayer::Software { player: p, .. } => p.is_active(),
            LinuxVideoPlayer::Camera(p) => p.is_active(),
        }
    }

    pub fn play(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.play(),
            LinuxVideoPlayer::Software { player: p, .. } => p.play(),
            LinuxVideoPlayer::Camera(_) => {} // camera is always playing
        }
    }

    pub fn pause(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.pause(),
            LinuxVideoPlayer::Software { player: p, .. } => p.pause(),
            LinuxVideoPlayer::Camera(_) => {} // no-op for camera
        }
    }

    pub fn resume(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.resume(),
            LinuxVideoPlayer::Software { player: p, .. } => p.resume(),
            LinuxVideoPlayer::Camera(_) => {} // no-op for camera
        }
    }

    pub fn mute(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.mute(),
            LinuxVideoPlayer::Software { player: p, .. } => p.mute(),
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn unmute(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.unmute(),
            LinuxVideoPlayer::Software { player: p, .. } => p.unmute(),
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.seek_to(position_ms),
            LinuxVideoPlayer::Software { player: p, .. } => p.seek_to(position_ms),
            LinuxVideoPlayer::Camera(_) => {} // camera is not seekable
        }
    }

    pub fn set_volume(&mut self, volume: f64) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.set_volume(volume),
            LinuxVideoPlayer::Software { player: p, .. } => p.set_volume(volume),
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn set_playback_rate(&self, rate: f64) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.set_playback_rate(rate),
            LinuxVideoPlayer::Software { player: p, .. } => p.set_playback_rate(rate),
            LinuxVideoPlayer::Camera(_) => {}
        }
    }

    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.seekable_ranges(),
            LinuxVideoPlayer::Software { player: p, .. } => p.seekable_ranges(),
            LinuxVideoPlayer::Camera(_) => vec![],
        }
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.buffered_ranges(),
            LinuxVideoPlayer::Software { player: p, .. } => p.buffered_ranges(),
            LinuxVideoPlayer::Camera(_) => vec![],
        }
    }

    pub fn current_position_ms(&self) -> u128 {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.current_position_ms(),
            LinuxVideoPlayer::Software { player: p, .. } => p.current_position_ms(),
            LinuxVideoPlayer::Camera(_) => 0,
        }
    }

    pub fn cleanup(&mut self) {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.cleanup(),
            LinuxVideoPlayer::Software { player: p, .. } => p.cleanup(),
            LinuxVideoPlayer::Camera(p) => p.cleanup(),
        }
    }

    pub fn is_software_mode(&self) -> bool {
        matches!(self, LinuxVideoPlayer::Software { .. })
    }

    pub fn is_camera_mode(&self) -> bool {
        matches!(self, LinuxVideoPlayer::Camera(_))
    }

    pub fn is_yuv_mode(&self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player, .. } => player.is_yuv_mode(),
            LinuxVideoPlayer::Software {
                rgba_gl_2d,
                ..
            } => !*rgba_gl_2d,
            LinuxVideoPlayer::Camera(_) => true,
        }
    }

    pub fn is_gl_memory_rgba(&self) -> bool {
        match self {
            LinuxVideoPlayer::GStreamer { player, .. } => player.is_gl_memory_rgba(),
            LinuxVideoPlayer::Software { rgba_gl_2d, .. } => *rgba_gl_2d,
            _ => false,
        }
    }

    pub fn yuv_texture_set(&self) -> Option<&YuvTextureSet> {
        match self {
            LinuxVideoPlayer::GStreamer { yuv: Some(yuv), .. } => Some(yuv),
            LinuxVideoPlayer::Software { yuv, .. } => Some(yuv),
            _ => None,
        }
    }

    pub fn yuv_matrix(&self) -> f32 {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.yuv_matrix(),
            LinuxVideoPlayer::Software { yuv_matrix, .. } => *yuv_matrix,
            LinuxVideoPlayer::Camera(_) => 1.0, // BT.601
        }
    }

    pub fn yuv_metadata(&self) -> crate::event::video_playback::VideoYuvMetadata {
        match self {
            LinuxVideoPlayer::GStreamer { player: p, .. } => p.yuv_metadata(),
            LinuxVideoPlayer::Software {
                yuv_matrix,
                yuv_biplanar,
                yuv_external,
                rgba_gl_2d,
                ..
            } => {
                if *rgba_gl_2d {
                    crate::event::video_playback::VideoYuvMetadata::disabled()
                } else {
                    crate::event::video_playback::VideoYuvMetadata {
                        enabled: true,
                        matrix: *yuv_matrix,
                        biplanar: *yuv_biplanar,
                        full_range: false,
                        rotation_steps: 0.0,
                        external: *yuv_external,
                    array: false,
                    }
                }
            }
            LinuxVideoPlayer::Camera(_) => crate::event::video_playback::VideoYuvMetadata {
                enabled: true,
                matrix: 1.0,
                biplanar: false,
                full_range: false,
                rotation_steps: 0.0,
                external: false,
            array: false,
            },
        }
    }
}

/// Shared X11/Wayland timer-tick poll for one desktop Linux video player.
pub fn collect_linux_video_player_events(
    player: &mut LinuxVideoPlayer,
    gl: &LibGl,
    textures: &mut CxTexturePool,
    opengl_cx: Option<&OpenglCx>,
) -> Vec<Event> {
    poll_pending_gstreamer_teardowns();
    let mut video_events = Vec::new();
    match player.check_prepared() {
        Some(Ok(PlaybackPrepared {
            width,
            height,
            duration_ms: duration,
            is_seekable,
            video_tracks,
            audio_tracks,
        })) => {
            video_events.push(Event::VideoPlaybackPrepared(
                VideoPlaybackPreparedEvent {
                    video_id: player.video_id(),
                    video_width: width,
                    video_height: height,
                    duration,
                    is_seekable,
                    video_tracks,
                    audio_tracks,
                },
            ));
            let seekable = player.seekable_ranges();
            if !seekable.is_empty() {
                video_events.push(Event::VideoSeekableRanges(VideoSeekableRangesEvent {
                    video_id: player.video_id(),
                    ranges: seekable,
                }));
            }
            let buffered = player.buffered_ranges();
            if !buffered.is_empty() {
                video_events.push(Event::VideoBufferedRanges(VideoBufferedRangesEvent {
                    video_id: player.video_id(),
                    ranges: buffered,
                }));
            }
        }
        Some(Err(err)) => {
            video_events.push(Event::VideoDecodingError(VideoDecodingErrorEvent {
                video_id: player.video_id(),
                error: err,
            }));
        }
        None => {}
    }
    for ev in player.poll_runtime() {
        match ev {
            GstRuntimeEvent::Error(error) => {
                video_events.push(Event::VideoDecodingError(VideoDecodingErrorEvent {
                    video_id: player.video_id(),
                    error,
                }));
            }
            GstRuntimeEvent::Eos => {
                video_events.push(Event::VideoPlaybackCompleted(
                    VideoPlaybackCompletedEvent {
                        video_id: player.video_id(),
                    },
                ));
            }
            GstRuntimeEvent::TracksChanged {
                video_tracks,
                audio_tracks,
            } => {
                video_events.push(Event::VideoTracksChanged(VideoTracksChangedEvent {
                    video_id: player.video_id(),
                    video_tracks,
                    audio_tracks,
                }));
            }
        }
    }
    if let Some(ranges) = player.take_buffered_ranges_if_changed() {
        video_events.push(Event::VideoBufferedRanges(VideoBufferedRangesEvent {
            video_id: player.video_id(),
            ranges,
        }));
    }
    if player.poll_frame(gl, textures, opengl_cx) {
        video_events.push(Event::VideoTextureUpdated(VideoTextureUpdatedEvent {
            video_id: player.video_id(),
            current_position_ms: player.current_position_ms(),
            yuv: player.yuv_metadata(),
            rgba_gl_2d: player.is_gl_memory_rgba(),
        }));
    }
    if player.check_eos() {
        video_events.push(Event::VideoPlaybackCompleted(
            VideoPlaybackCompletedEvent {
                video_id: player.video_id(),
            },
        ));
    }
    video_events
}
