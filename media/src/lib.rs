#[cfg(target_os = "android")]
mod h264_android;
#[cfg(target_os = "android")]
pub mod h264_android_decoder;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod h264_apple;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod h264_apple_decoder;

pub mod aac;
pub mod audio_capture;
pub mod playback;
pub mod audio_playout;
#[cfg(target_os = "linux")]
pub mod h264_gstreamer;
pub mod dummy_video_encoder;
pub mod h264_packets;
pub mod demux;
pub mod direct_media;
pub mod fmp4_demux;
#[allow(dead_code)]
pub mod mp4_decode;
#[allow(dead_code)]
pub mod mse_player;
pub mod mux;
#[cfg(feature = "opus")]
pub mod opus;
pub mod av1_software_encoder;
pub mod dav1d_ffi;
pub mod svt_av1_ffi;
pub mod yuv;
pub mod software_av1;
pub mod session_player;
pub mod video_session;
#[cfg(feature = "avif")]
pub mod avif;

pub use makepad_platform::{
    EncodedVideoPacketOwned, MediaPlaybackSession, MseAudioTrackInfo, MseDecodedAudioFrame,
    MseDecodedFrame, MseEngineOutput, MseInitMetadata, MsePlaybackEngine,
    MseVideoTrackInfo, PlaybackPrepared, VideoBitstreamFormat, VideoCodec,
    video_decode::yuv::{YuvColorMatrix, YuvLayout, YuvPlaneData},
};

pub use aac::{AacDecoderError, AacLcDecoder, AudioSpecificConfig};
pub use audio_capture::{
    AudioCaptureConfig, AudioCaptureError, AudioCaptureFrameizer, AudioCaptureReceiver,
    AudioTrackConfig, EncodedOpusPacket, MakepadAudioInputAdapter, PcmAudioFrame,
};
pub use audio_playout::{
    AudioPlayoutConfig, AudioPlayoutError, MakepadAudioOutputAdapter, PcmAudioOutputAdapter,
    PcmAudioPlayoutBuffer,
};
#[cfg(feature = "opus")]
pub use audio_playout::OpusAudioPlayoutBuffer;
#[cfg(feature = "opus")]
pub use opus::{
    OPUS_CHANNELS, OPUS_FRAME_DURATION_MS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE, OpusDecoder,
    OpusEncoder, OpusError,
};
pub use direct_media::{
    ByteRegionCache, DirectByteSourceReader, DirectMediaCursor, DirectMediaMachine,
    DirectMediaPlaybackConfig, DirectPumpOutput,
};
pub use playback::{
    DirectMediaPlaybackSession, MsePlaybackSession, NativePlaybackSession, PlaybackKind,
    SharedMseAppendOutcome, SharedMsePlaybackHandle, SharedMsePlaybackStatus,
    register_direct_media_playback_session,
};
pub use fmp4_demux::{FragmentSeekIndex, FragmentSeekPoint, index_fragmented_mp4};
pub use video_session::{VideoFrameSession, VideoFrameSessionId, VideoSessionState};

pub fn has_dav1d() -> bool {
    cfg!(has_dav1d)
}

pub fn has_svt_av1() -> bool {
    cfg!(has_svt_av1)
}

pub mod mp4 {
    pub use crate::{demux, mux};
}

use {
    crate::{
        av1_software_encoder::{
            Av1SoftwareEncoder as SwAv1Encoder, EncodedPacket, EncoderConfig, I420Frame,
            QueuePolicy,
        },
        software_av1::{SoftwareAv1Player as RuntimeSoftwareAv1Player, VideoSource as RuntimeVideoSource},
        yuv as media_yuv,
    },
    makepad_platform::{
        AudioBuffer, AudioInfo, MediaPlaybackSessionId,
        event::VideoSource,
        register_active_media_audio, register_media_plugin,
        take_registered_media_playback_session, unregister_active_media_audio,
        CameraFrameOwned, CameraFrameRef, MediaPlugin,
        MediaVideoEncoder, VideoCapabilities, VideoCodecSupport,
        VideoDecodeError, VideoEncodeError, VideoEncoderConfig, VideoEncodeSource, VideoOutputFn,
    },
    std::sync::{Arc, Mutex},
};

pub fn install() {
    let _ = register_media_plugin(Arc::new(DefaultMediaPlugin));
}

struct DefaultMediaPlugin;

impl MediaPlugin for DefaultMediaPlugin {
    fn create_video_encoder(
        &self,
        config: VideoEncoderConfig,
        output: VideoOutputFn,
    ) -> Option<Box<dyn MediaVideoEncoder>> {
        match config.codec {
            VideoCodec::Av1 => {
                let cfg = EncoderConfig {
                    width: config.width,
                    height: config.height,
                    fps_num: config.fps_num,
                    fps_den: config.fps_den,
                    target_bitrate: config.target_bitrate,
                    keyint: config.keyint,
                    codec_mode: config.codec_mode,
                    queue_policy: match config.queue_policy {
                        makepad_platform::VideoQueuePolicy::LatestWins => QueuePolicy::LatestWins,
                    },
                    queue_capacity: config.queue_capacity,
                };

                let mut output = output;
                let encoder = SwAv1Encoder::start(
                    cfg,
                    Box::new(move |pkt: EncodedPacket| {
                        output(makepad_platform::EncodedVideoPacketRef {
                            codec: VideoCodec::Av1,
                            format: VideoBitstreamFormat::Av1Obu,
                            pts_ns: pkt.pts_ns,
                            dts_ns: None,
                            is_key: pkt.is_key,
                            is_config: false,
                            is_eos: pkt.is_eos,
                            config_id: 0,
                            data: &pkt.data,
                        });
                    }),
                )?;

                Some(Box::new(Av1EncoderSession { inner: encoder }))
            }
            VideoCodec::H264 => {
                if let VideoEncodeSource::Dummy { layout } = config.source {
                    let mut adjusted = config;
                    adjusted.source = VideoEncodeSource::CpuFrames { layout };
                    #[cfg(target_os = "android")]
                    {
                        let inner = h264_android::AndroidH264Encoder::start(adjusted, output)
                            .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>)?;
                        return dummy_video_encoder::DummyVideoEncoder::with_inner(
                            inner,
                            adjusted.width,
                            adjusted.height,
                            adjusted.fps_num,
                            adjusted.fps_den,
                        )
                        .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>);
                    }
                    #[cfg(any(target_os = "macos", target_os = "ios"))]
                    {
                        let inner = h264_apple::AppleH264Encoder::start(adjusted, output)
                            .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>)?;
                        return dummy_video_encoder::DummyVideoEncoder::with_inner(
                            inner,
                            adjusted.width,
                            adjusted.height,
                            adjusted.fps_num,
                            adjusted.fps_den,
                        )
                        .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>);
                    }
                    #[cfg(target_os = "linux")]
                    {
                        let inner = h264_gstreamer::GstreamerH264Encoder::start(adjusted, output)
                            .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>)?;
                        return dummy_video_encoder::DummyVideoEncoder::with_inner(
                            inner,
                            adjusted.width,
                            adjusted.height,
                            adjusted.fps_num,
                            adjusted.fps_den,
                        )
                        .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>);
                    }
                    #[cfg(not(any(
                        target_os = "android",
                        target_os = "macos",
                        target_os = "ios",
                        target_os = "linux"
                    )))]
                    {
                        let _ = adjusted;
                        let _ = output;
                        return None;
                    }
                }
                #[cfg(target_os = "android")]
                {
                    return h264_android::AndroidH264Encoder::start(config, output)
                        .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>);
                }
                #[cfg(any(target_os = "macos", target_os = "ios"))]
                {
                    return h264_apple::AppleH264Encoder::start(config, output)
                        .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>);
                }
                #[cfg(target_os = "linux")]
                {
                    return h264_gstreamer::GstreamerH264Encoder::start(config, output)
                        .map(|enc| Box::new(enc) as Box<dyn MediaVideoEncoder>);
                }
                #[cfg(not(any(
                    target_os = "android",
                    target_os = "macos",
                    target_os = "ios",
                    target_os = "linux"
                )))]
                {
                    let _ = output;
                    None
                }
            }
            _ => None,
        }
    }

    fn create_playback_session(
        &self,
        _video_id: makepad_platform::LiveId,
        _texture_id: makepad_platform::TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Result<Box<dyn MediaPlaybackSession>, VideoDecodeError> {
        if let VideoSource::PlaybackSession(id) = source {
            let session = take_registered_media_playback_session(id)
                .ok_or(VideoDecodeError::UnsupportedCodec)?;
            return Ok(Box::new(RegisteredPlaybackSession::new(id, session)));
        }

        if let VideoSource::Session(id) = source {
            let player = session_player::VideoFrameSessionPlayer::from_registered(id, autoplay)
                .map_err(|_| VideoDecodeError::UnsupportedCodec)?;
            return Ok(Box::new(player));
        }

        let inner = RuntimeSoftwareAv1Player::new(map_source(source), autoplay, is_looping);
        Ok(Box::new(RuntimeSoftwareVideoSession { inner }))
    }

    fn video_capabilities(&self) -> VideoCapabilities {
        let av1_encode = has_svt_av1();
        let av1_decode = has_dav1d();
        let mut codecs = Vec::new();

        #[cfg(target_os = "linux")]
        {
            let h264_encode = h264_gstreamer::has_gstreamer_h264_encoder();
            let h264_decode = h264_gstreamer::has_gstreamer_h264_decoder();
            codecs.push(VideoCodecSupport {
                codec: VideoCodec::H264,
                encode_hardware: false,
                encode_software: h264_encode,
                decode_hardware: false,
                decode_software: h264_decode,
                encode_formats: if h264_encode {
                    vec![VideoBitstreamFormat::AnnexB]
                } else {
                    Vec::new()
                },
                decode_formats: if h264_decode {
                    vec![VideoBitstreamFormat::AnnexB, VideoBitstreamFormat::Avcc]
                } else {
                    Vec::new()
                },
                supports_camera_source: h264_encode,
                supports_texture_source: false,
                supports_cpu_frames_source: false,
                supports_keyframe_request: false,
                supports_dynamic_resolution: false,
                width_alignment: Some(2),
                height_alignment: Some(2),
                max_width: None,
                max_height: None,
                max_fps: None,
                max_bitrate: None,
            });
        }

        codecs.push(VideoCodecSupport {
            codec: VideoCodec::Av1,
            encode_hardware: false,
            encode_software: av1_encode,
            decode_hardware: false,
            decode_software: av1_decode,
            encode_formats: if av1_encode {
                vec![VideoBitstreamFormat::Av1Obu]
            } else {
                Vec::new()
            },
            decode_formats: if av1_decode {
                vec![VideoBitstreamFormat::Av1Obu]
            } else {
                Vec::new()
            },
            supports_camera_source: av1_encode,
            supports_texture_source: av1_encode,
            supports_cpu_frames_source: av1_encode,
            supports_keyframe_request: false,
            supports_dynamic_resolution: false,
            width_alignment: Some(2),
            height_alignment: Some(2),
            max_width: None,
            max_height: None,
            max_fps: None,
            max_bitrate: None,
        });

        VideoCapabilities { codecs }
    }

    fn on_android_h264_packet(&self, encoder_id: u64, pts_us: i64, flags: i32, data: Vec<u8>) {
        #[cfg(target_os = "android")]
        h264_android::on_java_h264_packet(encoder_id, pts_us, flags, data);
        #[cfg(not(target_os = "android"))]
        {
            let _ = (encoder_id, pts_us, flags, data);
        }
    }

    fn on_android_h264_error(&self, encoder_id: u64, message: String) {
        #[cfg(target_os = "android")]
        h264_android::on_java_h264_error(encoder_id, message);
        #[cfg(not(target_os = "android"))]
        {
            let _ = (encoder_id, message);
        }
    }

    fn create_mse_playback_engine(&self, mime: &str) -> Result<Box<dyn MsePlaybackEngine>, String> {
        let player = mse_player::SoftwareMsePlaybackEngine::new(mime)?;
        Ok(Box::new(player))
    }

    fn create_video_frame_decoder(
        &self,
        config: makepad_platform::FrameDecoderConfig,
    ) -> Result<Box<dyn makepad_platform::VideoFrameDecoder>, String> {
        match config.codec {
            makepad_platform::FrameDecoderCodec::H264 => {
                // Convert AVCC config to Annex B SPS/PPS (shared across platforms)
                let sps_pps = if !config.codec_config.is_empty() {
                    if let Some((sps, pps, _)) =
                        h264_packets::avcc_config_to_sps_pps(&config.codec_config)
                    {
                        h264_packets::sps_pps_to_annexb(&sps, &pps)
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };

                #[cfg(target_os = "linux")]
                {
                    let dec = h264_gstreamer::GstreamerH264Decoder::new(
                        &sps_pps,
                        config.width,
                        config.height,
                    )?;
                    return Ok(Box::new(dec));
                }

                #[cfg(any(target_os = "macos", target_os = "ios"))]
                {
                    let dec = h264_apple_decoder::AppleH264Decoder::new(
                        &sps_pps,
                    )?;
                    return Ok(Box::new(dec));
                }

                #[cfg(target_os = "android")]
                {
                    let dec = h264_android_decoder::AndroidH264Decoder::new(
                        &sps_pps,
                        config.width,
                        config.height,
                    )?;
                    return Ok(Box::new(dec));
                }

                #[cfg(not(any(
                    target_os = "linux",
                    target_os = "macos",
                    target_os = "ios",
                    target_os = "android"
                )))]
                {
                    let _ = sps_pps;
                    Err("H.264 frame decoder not available on this platform".into())
                }
            }
        }
    }
}

struct Av1EncoderSession {
    inner: SwAv1Encoder,
}

impl MediaVideoEncoder for Av1EncoderSession {
    fn push_frame(&self, frame: CameraFrameRef<'_>) {
        let mut owned = CameraFrameOwned::default();
        if !owned.convert_to_i420(frame) {
            return;
        }
        let media_frame = I420Frame {
            timestamp_ns: owned.timestamp_ns,
            width: owned.width as u32,
            height: owned.height as u32,
            y_stride: owned.planes[0].row_stride as u32,
            u_stride: owned.planes[1].row_stride as u32,
            v_stride: owned.planes[2].row_stride as u32,
            y: owned.planes[0].bytes.clone(),
            u: owned.planes[1].bytes.clone(),
            v: owned.planes[2].bytes.clone(),
        };
        self.inner.push_i420(media_frame);
    }

    fn request_keyframe(&self) -> Result<(), VideoEncodeError> {
        Err(VideoEncodeError::UnsupportedCodec)
    }

    fn stop(&self) {
        self.inner.stop();
    }
}

struct RuntimeSoftwareVideoSession {
    inner: RuntimeSoftwareAv1Player,
}

struct RegisteredPlaybackSession {
    id: MediaPlaybackSessionId,
    inner: Arc<Mutex<Box<dyn MediaPlaybackSession + Send>>>,
}

impl RegisteredPlaybackSession {
    fn new(id: MediaPlaybackSessionId, session: Box<dyn MediaPlaybackSession + Send>) -> Self {
        let inner = Arc::new(Mutex::new(session));
        register_active_media_audio(id, inner.clone());
        Self { id, inner }
    }
}

impl MediaPlaybackSession for RuntimeSoftwareVideoSession {
    fn check_prepared(
        &mut self,
    ) -> Option<Result<PlaybackPrepared, String>> {
        self.inner.check_prepared()
    }

    fn poll_frame(&mut self) -> bool {
        self.inner.poll_frame()
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.inner.take_yuv_frame().map(map_yuv)
    }

    fn check_eos(&mut self) -> bool {
        self.inner.check_eos()
    }

    fn play(&mut self) {
        self.inner.play();
    }

    fn pause(&mut self) {
        self.inner.pause();
    }

    fn resume(&mut self) {
        self.inner.resume();
    }

    fn is_playing(&self) -> bool {
        self.inner.is_playing()
    }

    fn seek_to(&mut self, position_ms: u64) {
        self.inner.seek_to(position_ms);
    }

    fn set_volume(&self, volume: f64) {
        self.inner.set_volume(volume);
    }

    fn current_position_ms(&self) -> u128 {
        self.inner.current_position_ms()
    }

    fn mute(&self) {
        self.inner.mute();
    }

    fn unmute(&self) {
        self.inner.unmute();
    }

    fn set_playback_rate(&self, rate: f64) {
        self.inner.set_playback_rate(rate);
    }

    fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        self.inner.seekable_ranges()
    }

    fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.inner.buffered_ranges()
    }

    fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    fn cleanup(&mut self) {
        self.inner.cleanup();
    }
}

impl MediaPlaybackSession for RegisteredPlaybackSession {
    fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        self.inner.lock().ok().and_then(|mut inner| inner.check_prepared())
    }

    fn poll_frame(&mut self) -> bool {
        self.inner.lock().ok().map(|mut inner| inner.poll_frame()).unwrap_or(false)
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.inner.lock().ok().and_then(|mut inner| inner.take_yuv_frame())
    }

    fn check_eos(&mut self) -> bool {
        self.inner.lock().ok().map(|mut inner| inner.check_eos()).unwrap_or(false)
    }

    fn play(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.play();
        }
    }

    fn pause(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pause();
        }
    }

    fn resume(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.resume();
        }
    }

    fn is_playing(&self) -> bool {
        self.inner.lock().ok().map(|inner| inner.is_playing()).unwrap_or(false)
    }

    fn seek_to(&mut self, position_ms: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.seek_to(position_ms);
        }
    }

    fn set_volume(&self, volume: f64) {
        if let Ok(inner) = self.inner.lock() {
            inner.set_volume(volume);
        }
    }

    fn current_position_ms(&self) -> u128 {
        self.inner.lock().ok().map(|inner| inner.current_position_ms()).unwrap_or(0)
    }

    fn mute(&self) {
        if let Ok(inner) = self.inner.lock() {
            inner.mute();
        }
    }

    fn unmute(&self) {
        if let Ok(inner) = self.inner.lock() {
            inner.unmute();
        }
    }

    fn set_playback_rate(&self, rate: f64) {
        if let Ok(inner) = self.inner.lock() {
            inner.set_playback_rate(rate);
        }
    }

    fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        self.inner.lock().ok().map(|inner| inner.seekable_ranges()).unwrap_or_default()
    }

    fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.inner.lock().ok().map(|inner| inner.buffered_ranges()).unwrap_or_default()
    }

    fn fill_audio_output(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.fill_audio_output(info, output);
        }
    }

    fn is_active(&self) -> bool {
        self.inner.lock().ok().map(|inner| inner.is_active()).unwrap_or(false)
    }

    fn cleanup(&mut self) {
        unregister_active_media_audio(self.id);
        if let Ok(mut inner) = self.inner.lock() {
            inner.cleanup();
        }
    }
}

impl Drop for RegisteredPlaybackSession {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn map_source(source: VideoSource) -> RuntimeVideoSource {
    match source {
        VideoSource::InMemory(bytes) => RuntimeVideoSource::InMemory(bytes),
        VideoSource::Network(url) => RuntimeVideoSource::Network(url),
        VideoSource::Filesystem(path) => RuntimeVideoSource::Filesystem(path),
        VideoSource::Camera(_, _) => RuntimeVideoSource::Camera,
        VideoSource::PlaybackSession(_) | VideoSource::Session(_) => {
            unreachable!("session sources are handled before AV1 runtime mapping")
        }
    }
}

fn map_yuv(planes: media_yuv::YuvPlaneData) -> YuvPlaneData {
    YuvPlaneData {
        y: planes.y,
        u: planes.u,
        v: planes.v,
        width: planes.width,
        height: planes.height,
        layout: match planes.layout {
            media_yuv::YuvLayout::I420 => YuvLayout::I420,
            media_yuv::YuvLayout::I422 => YuvLayout::I422,
            media_yuv::YuvLayout::I444 => YuvLayout::I444,
            media_yuv::YuvLayout::I400 => YuvLayout::I400,
        },
        matrix: match planes.matrix {
            media_yuv::YuvColorMatrix::BT709 => YuvColorMatrix::BT709,
            media_yuv::YuvColorMatrix::BT601 => YuvColorMatrix::BT601,
            media_yuv::YuvColorMatrix::BT2020 => YuvColorMatrix::BT2020,
        },
    }
}
