use {
    self::super::{
        alsa_audio::AlsaAudioAccess, alsa_midi::*, pulse_audio::PulseAudioAccess,
        v4l2_camera::V4l2CameraAccess,
    },
    crate::{
        audio::*, cx::Cx, event::Event, media_api::CxMediaApi, midi::*, thread::SignalToUI,
        video::*,
    },
    std::sync::{Arc, Mutex},
};

fn pulse_audio_enabled() -> bool {
    std::env::var_os("MAKEPAD_DISABLE_PULSE_AUDIO").is_none()
}

impl Cx {
    pub(crate) fn handle_media_signals(&mut self) {
        let pulse_enabled = pulse_audio_enabled();
        let audio_first = self.os.media.alsa_audio.is_none()
            || (pulse_enabled
                && self.os.media.pulse_audio.is_none()
                && !self.os.media.pulse_audio_unavailable);
        if audio_first || self.os.media.audio_change.check_and_clear() {
            // alright so. if we 'failed' opening a device here
            // what do we do. we could flag our device as 'failed' on the desc
            let mut descs = self
                .os
                .media
                .alsa_audio()
                .lock()
                .unwrap()
                .get_updated_descs();
            if pulse_enabled {
                if let Some(pulse_audio) = self.os.media.pulse_audio() {
                    let descs2 = pulse_audio.lock().unwrap().get_updated_descs();
                    descs.extend(descs2);
                }
            }
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent { descs }));
        }
        if self.os.media.alsa_midi_change.check_and_clear() {
            let descs = self
                .os
                .media
                .alsa_midi()
                .lock()
                .unwrap()
                .get_updated_descs();
            self.call_event_handler(&Event::MidiPorts(MidiPortsEvent { descs }));
        }
        // Lazily initialize V4L2 camera subsystem on first media signal check.
        // This starts the /dev watcher which sets v4l2_change on device changes.
        let v4l2_first = self.os.media.v4l2_camera.is_none();
        if v4l2_first || self.os.media.v4l2_change.check_and_clear() {
            let descs = self
                .os
                .media
                .v4l2_camera()
                .lock()
                .unwrap()
                .get_updated_descs();
            self.call_event_handler(&Event::VideoInputs(VideoInputsEvent { descs }));
        }
    }
}

#[derive(Default)]
pub struct CxLinuxMedia {
    pub(crate) pulse_audio: Option<Arc<Mutex<PulseAudioAccess>>>,
    /// Set when connecting to a PulseAudio server failed, so we don't retry
    /// (and re-log) on every audio device change on machines without one.
    pub(crate) pulse_audio_unavailable: bool,
    pub(crate) alsa_audio: Option<Arc<Mutex<AlsaAudioAccess>>>,
    pub(crate) audio_change: SignalToUI,
    pub(crate) alsa_midi: Option<Arc<Mutex<AlsaMidiAccess>>>,
    pub(crate) alsa_midi_change: SignalToUI,
    pub(crate) v4l2_camera: Option<Arc<Mutex<V4l2CameraAccess>>>,
    pub(crate) v4l2_change: SignalToUI,
}

impl CxLinuxMedia {
    /// Returns `None` when there is no usable PulseAudio server, in which case
    /// audio runs through ALSA alone.
    pub fn pulse_audio(&mut self) -> Option<Arc<Mutex<PulseAudioAccess>>> {
        if self.pulse_audio.is_none() && !self.pulse_audio_unavailable {
            self.pulse_audio = PulseAudioAccess::new(
                self.audio_change.clone(),
                &self.alsa_audio().lock().unwrap(),
            );
            self.pulse_audio_unavailable = self.pulse_audio.is_none();
        }
        self.pulse_audio.clone()
    }

    pub fn alsa_audio(&mut self) -> Arc<Mutex<AlsaAudioAccess>> {
        if self.alsa_audio.is_none() {
            self.alsa_audio = Some(AlsaAudioAccess::new(self.audio_change.clone()));
        }
        self.alsa_audio.as_ref().unwrap().clone()
    }

    pub fn alsa_midi(&mut self) -> Arc<Mutex<AlsaMidiAccess>> {
        if self.alsa_midi.is_none() {
            self.alsa_midi = Some(AlsaMidiAccess::new(self.alsa_midi_change.clone()));
        }
        self.alsa_midi.as_ref().unwrap().clone()
    }

    pub fn v4l2_camera(&mut self) -> Arc<Mutex<V4l2CameraAccess>> {
        if self.v4l2_camera.is_none() {
            self.v4l2_camera = Some(V4l2CameraAccess::new(self.v4l2_change.clone()));
        }
        self.v4l2_camera.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    fn midi_input(&mut self) -> MidiInput {
        self.os
            .media
            .alsa_midi()
            .lock()
            .unwrap()
            .create_midi_input()
    }

    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput(self.os.media.alsa_midi())))
    }

    fn midi_reset(&mut self) {}

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os
            .media
            .alsa_midi()
            .lock()
            .unwrap()
            .use_midi_inputs(ports);
    }

    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os
            .media
            .alsa_midi()
            .lock()
            .unwrap()
            .use_midi_outputs(ports);
    }

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os
            .media
            .alsa_audio()
            .lock()
            .unwrap()
            .use_audio_inputs(devices);
        if pulse_audio_enabled() {
            if let Some(pulse_audio) = self.os.media.pulse_audio() {
                pulse_audio.lock().unwrap().use_audio_inputs(devices);
            }
        }
    }

    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os
            .media
            .alsa_audio()
            .lock()
            .unwrap()
            .use_audio_outputs(devices);
        if pulse_audio_enabled() {
            if let Some(pulse_audio) = self.os.media.pulse_audio() {
                pulse_audio.lock().unwrap().use_audio_outputs(devices);
            }
        }
    }

    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        *self.os.media.alsa_audio().lock().unwrap().audio_output_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        *self.os.media.alsa_audio().lock().unwrap().audio_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn video_input_box(&mut self, index: usize, f: VideoInputFn) {
        *self.os.media.v4l2_camera().lock().unwrap().video_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn camera_frame_input_box(&mut self, index: usize, f: CameraFrameInputFn) {
        *self
            .os
            .media
            .v4l2_camera()
            .lock()
            .unwrap()
            .camera_frame_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn video_encoder_output_box(
        &mut self,
        index: usize,
        config: VideoEncoderConfig,
        f: VideoOutputFn,
    ) -> Result<(), VideoEncodeError> {
        let result = self
            .os
            .media
            .v4l2_camera()
            .lock()
            .unwrap()
            .configure_video_encoder(index, config, f);
        if let Err(err) = result {
            crate::error!("linux video_encoder_output_box failed: {:?}", err);
        }
        result
    }

    fn video_encoder_push_frame(&mut self, index: usize, frame: CameraFrameRef<'_>) {
        let Some(camera) = self.os.media.v4l2_camera.as_ref() else {
            return;
        };
        camera.lock().unwrap().video_encoder_push_frame(index, frame);
    }

    fn video_encoder_capture_texture_frame(
        &mut self,
        index: usize,
        timestamp_ns: u64,
    ) -> Result<(), VideoEncodeError> {
        let Some(opengl_cx) = self.os.opengl_cx.as_ref() else {
            return Err(VideoEncodeError::EncoderNotStarted);
        };
        opengl_cx.make_current();
        let gl = self.os.gl() as *const _;
        self.os
            .media
            .v4l2_camera()
            .lock()
            .unwrap()
            .video_encoder_capture_texture_frame(
                index,
                timestamp_ns,
                unsafe { &*gl },
                &mut self.textures,
            )
    }

    fn video_encoder_request_keyframe(&mut self, index: usize) -> Result<(), VideoEncodeError> {
        let Some(camera) = self.os.media.v4l2_camera.as_ref() else {
            return Err(VideoEncodeError::EncoderNotStarted);
        };
        camera
            .lock()
            .unwrap()
            .video_encoder_request_keyframe(index)
    }

    fn video_capabilities(&self) -> VideoCapabilities {
        crate::merge_video_capabilities(
            VideoCapabilities::default(),
            crate::media_video_capabilities(),
        )
    }

    fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        self.os
            .media
            .v4l2_camera()
            .lock()
            .unwrap()
            .use_video_input(inputs);
    }
}
