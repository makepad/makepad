use {
    self::super::{android_audio::*, android_camera::*, android_jni::*, android_midi::*},
    crate::{
        audio::*, cx::Cx, event::Event, media_api::CxMediaApi, midi::*, thread::SignalToUI,
        video::*,
    },
    std::sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct CxAndroidMedia {
    pub(crate) android_audio_change: SignalToUI,
    pub(crate) android_audio: Option<Arc<Mutex<AndroidAudioAccess>>>,
    pub(crate) android_midi_change: SignalToUI,
    pub(crate) android_midi: Option<Arc<Mutex<AndroidMidiAccess>>>,
    pub(crate) android_camera_change: SignalToUI,
    pub(crate) android_camera: Option<Arc<Mutex<AndroidCameraAccess>>>,
}

impl Cx {
    pub(crate) fn handle_media_signals(&mut self /*, to_java: &AndroidToJava*/) {
        if self.os.media.android_audio_change.check_and_clear() {
            let descs = self
                .os
                .media
                .android_audio()
                .lock()
                .unwrap()
                .get_updated_descs();
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent { descs }));
        }
        if self.os.media.android_midi_change.check_and_clear() {
            let descs = self
                .os
                .media
                .android_midi()
                .lock()
                .unwrap()
                .get_updated_descs();
            if let Some(descs) = descs {
                self.call_event_handler(&Event::MidiPorts(MidiPortsEvent { descs }));
            }
        }
        // Lazily initialize camera subsystem on first media signal check.
        let camera_first = self.os.media.android_camera.is_none();
        if camera_first || self.os.media.android_camera_change.check_and_clear() {
            let descs = self
                .os
                .media
                .android_camera()
                .lock()
                .unwrap()
                .get_updated_descs();
            self.call_event_handler(&Event::VideoInputs(VideoInputsEvent { descs }));
        }
    }

    pub fn reinitialise_media(&mut self) {
        // lets reinitialize cameras/midi/etc
        if self.os.media.android_audio.is_some() {
            self.os.media.android_audio_change.set();
        }
        if self.os.media.android_midi.is_some() {
            self.os.media.android_midi_change.set();
        }
        if self.os.media.android_camera.is_some() {
            self.os.media.android_camera_change.set();
        }
    }
}

impl CxAndroidMedia {
    pub fn android_audio(&mut self) -> Arc<Mutex<AndroidAudioAccess>> {
        if self.android_audio.is_none() {
            self.android_audio = Some(AndroidAudioAccess::new(self.android_audio_change.clone()));
        }
        self.android_audio.as_ref().unwrap().clone()
    }
    pub fn android_midi(&mut self) -> Arc<Mutex<AndroidMidiAccess>> {
        if self.android_midi.is_none() {
            self.android_midi = Some(AndroidMidiAccess::new(self.android_midi_change.clone()));
        }
        self.android_midi.as_ref().unwrap().clone()
    }
    pub fn android_camera(&mut self) -> Arc<Mutex<AndroidCameraAccess>> {
        if self.android_camera.is_none() {
            self.android_camera =
                Some(AndroidCameraAccess::new(self.android_camera_change.clone()));
        }
        self.android_camera.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    fn midi_input(&mut self) -> MidiInput {
        let amidi = self.os.media.android_midi().clone();
        self.os
            .media
            .android_midi()
            .lock()
            .unwrap()
            .create_midi_input(amidi)
    }

    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput {
            amidi: self.os.media.android_midi(),
        }))
    }

    fn midi_reset(&mut self) {}

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os
            .media
            .android_midi()
            .lock()
            .unwrap()
            .use_midi_inputs(ports);
    }

    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os
            .media
            .android_midi()
            .lock()
            .unwrap()
            .use_midi_outputs(ports);
    }

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os
            .media
            .android_audio()
            .lock()
            .unwrap()
            .use_audio_inputs(devices);
    }

    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os
            .media
            .android_audio()
            .lock()
            .unwrap()
            .use_audio_outputs(devices);
    }

    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        *self
            .os
            .media
            .android_audio()
            .lock()
            .unwrap()
            .audio_output_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        *self.os.media.android_audio().lock().unwrap().audio_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn video_input_box(&mut self, index: usize, f: VideoInputFn) {
        *self
            .os
            .media
            .android_camera()
            .lock()
            .unwrap()
            .video_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn camera_frame_input_box(&mut self, index: usize, f: CameraFrameInputFn) {
        *self
            .os
            .media
            .android_camera()
            .lock()
            .unwrap()
            .camera_frame_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn video_encoder_output_box(
        &mut self,
        index: usize,
        mut config: VideoEncoderConfig,
        f: VideoOutputFn,
    ) -> Result<(), VideoEncodeError> {
        if let VideoEncodeSource::Camera {
            mut input_id,
            mut format_id,
        } = config.source
        {
            let camera = self.os.media.android_camera();
            let camera = camera.lock().unwrap();
            if let Some((active_input, active_format)) = camera.active_inputs().get(index).copied()
            {
                input_id = active_input;
                format_id = active_format;
            }
            config.source = VideoEncodeSource::Camera {
                input_id,
                format_id,
            };
        }
        let result = self
            .os
            .media
            .android_camera()
            .lock()
            .unwrap()
            .configure_video_encoder(index, config, f);
        if let Err(err) = result {
            crate::error!("android video_encoder_output_box failed: {:?}", err);
        }
        result
    }

    fn video_encoder_push_frame(&mut self, index: usize, frame: CameraFrameRef<'_>) {
        self.os
            .media
            .android_camera()
            .lock()
            .unwrap()
            .video_encoder_push_frame(index, frame);
    }

    fn video_encoder_capture_texture_frame(
        &mut self,
        index: usize,
        timestamp_ns: u64,
    ) -> Result<(), VideoEncodeError> {
        let gl = self.os.gl() as *const _;
        self.os
            .media
            .android_camera()
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
        self.os
            .media
            .android_camera()
            .lock()
            .unwrap()
            .video_encoder_request_keyframe(index)
    }

    fn video_capabilities(&self) -> VideoCapabilities {
        let mut codecs = Vec::new();

        if let Some(probe) = unsafe { to_java_query_h264_codec_support() } {
            let encode_available = probe.encode_hardware || probe.encode_software;
            let decode_available = probe.decode_hardware || probe.decode_software;
            codecs.push(VideoCodecSupport {
                codec: VideoCodec::H264,
                encode_hardware: probe.encode_hardware,
                encode_software: probe.encode_software,
                decode_hardware: probe.decode_hardware,
                decode_software: probe.decode_software,
                encode_formats: if encode_available {
                    vec![VideoBitstreamFormat::AnnexB]
                } else {
                    Vec::new()
                },
                decode_formats: if decode_available {
                    vec![VideoBitstreamFormat::AnnexB, VideoBitstreamFormat::Avcc]
                } else {
                    Vec::new()
                },
                supports_camera_source: encode_available,
                supports_texture_source: encode_available,
                supports_cpu_frames_source: encode_available,
                supports_keyframe_request: encode_available,
                supports_dynamic_resolution: false,
                width_alignment: if probe.width_alignment > 0 {
                    Some(probe.width_alignment)
                } else {
                    Some(2)
                },
                height_alignment: if probe.height_alignment > 0 {
                    Some(probe.height_alignment)
                } else {
                    Some(2)
                },
                max_width: (probe.max_width > 0).then_some(probe.max_width),
                max_height: (probe.max_height > 0).then_some(probe.max_height),
                max_fps: (probe.max_fps > 0).then_some(probe.max_fps),
                max_bitrate: (probe.max_bitrate > 0).then_some(probe.max_bitrate),
            });
        } else {
            codecs.push(VideoCodecSupport::unsupported(VideoCodec::H264));
        }

        codecs.push(VideoCodecSupport::unsupported(VideoCodec::H265));
        crate::merge_video_capabilities(
            VideoCapabilities { codecs },
            crate::media_video_capabilities(),
        )
    }

    fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        self.os
            .media
            .android_camera()
            .lock()
            .unwrap()
            .use_video_input(inputs);
    }
}
