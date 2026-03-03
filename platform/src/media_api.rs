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

    fn camera_av1_output<F>(&mut self, index: usize, config: CameraAv1EncoderConfig, f: F)
    where
        F: for<'a> FnMut(EncodedAv1PacketRef<'a>) + Send + 'static,
    {
        self.camera_av1_output_box(index, config, Box::new(f))
    }

    /// Register AV1 packet callback for camera input index.
    ///
    /// Backends with camera frame transport should override this and route
    /// frames into the shared camera->AV1 encoder worker.
    fn camera_av1_output_box(
        &mut self,
        _index: usize,
        _config: CameraAv1EncoderConfig,
        _f: CameraAv1OutputFn,
    ) {
    }

    fn use_video_input(&mut self, devices: &[(VideoInputId, VideoFormatId)]);
}
