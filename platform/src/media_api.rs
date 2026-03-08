use crate::{
    audio::{AudioBuffer, AudioDeviceId, AudioInfo, AudioInputFn, AudioOutputFn},
    midi::*,
    video::*,
};

pub trait CxMediaApi {
    fn midi_input(&mut self) -> MidiInput;
    fn midi_output(&mut self) -> MidiOutput;
    fn midi_reset(&mut self);

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]);
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]);

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]);
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]);

    fn audio_output<F>(&mut self, index: usize, f: F)
    where
        F: FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static,
    {
        self.audio_output_box(index, Box::new(f))
    }
    fn audio_input<F>(&mut self, index: usize, f: F)
    where
        F: FnMut(AudioInfo, &AudioBuffer) + Send + 'static,
    {
        self.audio_input_box(index, Box::new(f))
    }

    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn);
    fn audio_input_box(&mut self, index: usize, f: AudioInputFn);

    fn video_input<F>(&mut self, index: usize, f: F)
    where
        F: FnMut(VideoBufferRef) + Send + 'static,
    {
        self.video_input_box(index, Box::new(f))
    }

    fn video_input_box(&mut self, index: usize, f: VideoInputFn);

    fn camera_frame_input<F>(&mut self, index: usize, f: F)
    where
        F: for<'a> FnMut(CameraFrameRef<'a>) + Send + 'static,
    {
        self.camera_frame_input_box(index, Box::new(f))
    }

    /// Platform-agnostic camera frame transport hook.
    ///
    /// Backends that support structured camera frame transport should override this.
    /// Backends that do not support it yet can keep the default no-op implementation.
    fn camera_frame_input_box(&mut self, _index: usize, _f: CameraFrameInputFn) {}

    fn video_encoder_output<F>(&mut self, index: usize, config: VideoEncoderConfig, f: F)
    where
        F: for<'a> FnMut(EncodedVideoPacketRef<'a>) + Send + 'static,
    {
        if let Err(err) = self.video_encoder_output_try(index, config, f) {
            crate::error!("video encode setup failed: {:?}", err);
        }
    }

    fn video_encoder_output_try<F>(
        &mut self,
        index: usize,
        config: VideoEncoderConfig,
        f: F,
    ) -> Result<(), VideoEncodeError>
    where
        F: for<'a> FnMut(EncodedVideoPacketRef<'a>) + Send + 'static,
    {
        let result = self.video_encoder_output_box(index, config, Box::new(f));
        if let Err(err) = result {
            crate::error!("video encode setup failed: {:?}", err);
        }
        result
    }

    fn video_encoder_output_box(
        &mut self,
        _index: usize,
        _config: VideoEncoderConfig,
        _f: VideoOutputFn,
    ) -> Result<(), VideoEncodeError> {
        Err(VideoEncodeError::UnsupportedSource)
    }

    fn video_encoder_push_frame(&mut self, _index: usize, _frame: CameraFrameRef<'_>) {}

    /// Capture one frame from a configured texture source.
    ///
    /// Must be called on the render thread for backends that require render-context access.
    fn video_encoder_capture_texture_frame(
        &mut self,
        _index: usize,
        _timestamp_ns: u64,
    ) -> Result<(), VideoEncodeError> {
        Err(VideoEncodeError::UnsupportedSource)
    }

    fn video_encoder_request_keyframe(&mut self, _index: usize) -> Result<(), VideoEncodeError> {
        Err(VideoEncodeError::UnsupportedCodec)
    }

    fn video_decoder_start_box(
        &mut self,
        _index: usize,
        _config: VideoDecoderConfig,
        _f: VideoDecodedFrameOutputFn,
    ) -> Result<(), VideoDecodeError> {
        Err(VideoDecodeError::UnsupportedCodec)
    }

    fn video_decoder_push_packet(
        &mut self,
        _index: usize,
        _packet: VideoDecoderPacketRef<'_>,
    ) -> Result<(), VideoDecodeError> {
        Err(VideoDecodeError::DecoderNotStarted)
    }

    fn video_decoder_stop(&mut self, _index: usize) {}

    fn video_capabilities(&self) -> VideoCapabilities {
        VideoCapabilities::default()
    }

    fn use_video_input(&mut self, devices: &[(VideoInputId, VideoFormatId)]);
}
