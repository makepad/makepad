use {
    crate::{
        audio::*, cx::Cx, event::Event, media_api::CxMediaApi, midi::*, os::apple::apple_sys::*,
        os::apple::audio_unit::AudioUnitAccess, os::apple::av_capture::AvCaptureAccess,
        os::apple::core_midi::*, thread::SignalToUI, video::*,
    },
    std::sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct CxAppleMedia {
    pub(crate) core_midi: Option<Arc<Mutex<CoreMidiAccess>>>,
    pub(crate) audio_unit: Option<Arc<Mutex<AudioUnitAccess>>>,
    pub(crate) av_capture: Option<Arc<Mutex<AvCaptureAccess>>>,
    pub(crate) core_audio_change: SignalToUI,
    pub(crate) core_midi_change: SignalToUI,
    pub(crate) av_capture_change: SignalToUI,
}

impl Cx {
    pub(crate) fn handle_media_signals(&mut self) {
        if self.os.media.core_midi_change.check_and_clear() {
            let descs = self
                .os
                .media
                .core_midi()
                .lock()
                .unwrap()
                .get_updated_descs();
            self.call_event_handler(&Event::MidiPorts(MidiPortsEvent { descs }));
        }
        if self.os.media.core_audio_change.check_and_clear() {
            let descs = self
                .os
                .media
                .audio_unit()
                .lock()
                .unwrap()
                .get_updated_descs();
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent { descs }));
        }
        if self.os.media.av_capture_change.check_and_clear() {
            let descs = self
                .os
                .media
                .av_capture()
                .lock()
                .unwrap()
                .get_updated_descs();
            self.call_event_handler(&Event::VideoInputs(VideoInputsEvent { descs }));
        }
    }
}

impl CxAppleMedia {
    pub fn audio_unit(&mut self) -> Arc<Mutex<AudioUnitAccess>> {
        if self.audio_unit.is_none() {
            self.audio_unit = Some(AudioUnitAccess::new(self.core_audio_change.clone()));
            self.core_audio_change.set();
        }
        self.audio_unit.as_ref().unwrap().clone()
    }

    pub fn core_midi(&mut self) -> Arc<Mutex<CoreMidiAccess>> {
        if self.core_midi.is_none() {
            self.core_midi = Some(CoreMidiAccess::new(self.core_midi_change.clone()));
            self.core_midi_change.set();
        }
        self.core_midi.as_ref().unwrap().clone()
    }

    pub fn av_capture(&mut self) -> Arc<Mutex<AvCaptureAccess>> {
        if self.av_capture.is_none() {
            self.av_capture = Some(AvCaptureAccess::new(self.av_capture_change.clone()));
            self.av_capture_change.set();
        }
        self.av_capture.as_ref().unwrap().clone()
    }
}

#[derive(Default, Clone, Copy)]
struct AppleH264Probe {
    encode_hardware: bool,
    encode_software: bool,
    decode_hardware: bool,
    decode_software: bool,
}

fn probe_apple_h264() -> AppleH264Probe {
    unsafe {
        let mut probe = AppleH264Probe::default();

        probe.encode_hardware = VTIsHardwareEncodeSupported(kCMVideoCodecType_H264) == YES;

        let mut enc: VTCompressionSessionRef = std::ptr::null_mut();
        let enc_status = VTCompressionSessionCreate(
            std::ptr::null(),
            64,
            64,
            kCMVideoCodecType_H264,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            None,
            std::ptr::null_mut(),
            &mut enc,
        );
        let encode_available = enc_status == 0 && !enc.is_null();
        if encode_available {
            VTCompressionSessionInvalidate(enc);
            CFRelease(enc as *const std::ffi::c_void);
        }
        probe.encode_software = encode_available && !probe.encode_hardware;

        probe.decode_hardware = VTIsHardwareDecodeSupported(kCMVideoCodecType_H264) == YES;

        // Baseline 64x64 SPS/PPS probe stream.
        let sps: [u8; 23] = [
            0x67, 0x42, 0xC0, 0x1E, 0xDA, 0x02, 0x80, 0x2D, 0xD0, 0x80, 0x88, 0x45, 0xA1, 0x00,
            0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xF1,
        ];
        let pps: [u8; 4] = [0x68, 0xCE, 0x3C, 0x80];
        let mut format_desc: CMFormatDescriptionRef = std::ptr::null_mut();
        let parameter_set_ptrs: [*const u8; 2] = [sps.as_ptr(), pps.as_ptr()];
        let parameter_set_sizes: [usize; 2] = [sps.len(), pps.len()];

        let fmt_status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
            std::ptr::null(),
            2,
            parameter_set_ptrs.as_ptr(),
            parameter_set_sizes.as_ptr(),
            4,
            &mut format_desc,
        );

        let mut decode_available = false;
        if fmt_status == 0 && !format_desc.is_null() {
            let mut dec: VTDecompressionSessionRef = std::ptr::null_mut();
            let callback_record = VTDecompressionOutputCallbackRecord {
                decompressionOutputCallback: None,
                decompressionOutputRefCon: std::ptr::null_mut(),
            };
            let dec_status = VTDecompressionSessionCreate(
                std::ptr::null(),
                format_desc,
                std::ptr::null(),
                std::ptr::null(),
                &callback_record,
                &mut dec,
            );
            decode_available = dec_status == 0 && !dec.is_null();
            if !dec.is_null() {
                VTDecompressionSessionInvalidate(dec);
                CFRelease(dec as *const std::ffi::c_void);
            }
            CFRelease(format_desc as *const std::ffi::c_void);
        }

        probe.decode_software = decode_available && !probe.decode_hardware;
        probe
    }
}

impl CxMediaApi for Cx {
    fn midi_input(&mut self) -> MidiInput {
        self.os
            .media
            .core_midi()
            .lock()
            .unwrap()
            .create_midi_input()
    }

    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput(self.os.media.core_midi())))
    }

    fn midi_reset(&mut self) {
        self.os.media.core_midi().lock().unwrap().midi_reset();
    }

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os
            .media
            .core_midi()
            .lock()
            .unwrap()
            .use_midi_inputs(ports);
    }

    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os
            .media
            .core_midi()
            .lock()
            .unwrap()
            .use_midi_outputs(ports);
    }

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os
            .media
            .audio_unit()
            .lock()
            .unwrap()
            .use_audio_inputs(devices);
    }

    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os
            .media
            .audio_unit()
            .lock()
            .unwrap()
            .use_audio_outputs(devices);
    }

    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        *self.os.media.audio_unit().lock().unwrap().audio_output_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        *self.os.media.audio_unit().lock().unwrap().audio_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn video_input_box(&mut self, index: usize, f: VideoInputFn) {
        *self.os.media.av_capture().lock().unwrap().video_input_cb[index]
            .lock()
            .unwrap() = Some(f);
    }

    fn camera_frame_input_box(&mut self, index: usize, f: CameraFrameInputFn) {
        *self
            .os
            .media
            .av_capture()
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
            let camera = self.os.media.av_capture();
            let camera = camera.lock().unwrap();
            let active = camera.active_inputs();
            if let Some((active_input, active_format)) = active.get(index).copied() {
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
            .av_capture()
            .lock()
            .unwrap()
            .configure_video_encoder(index, config, f);
        if let Err(err) = result {
            crate::error!("apple video_encoder_output_box failed: {:?}", err);
        }
        result
    }

    fn video_encoder_push_frame(&mut self, index: usize, frame: CameraFrameRef<'_>) {
        self.os
            .media
            .av_capture()
            .lock()
            .unwrap()
            .video_encoder_push_frame(index, frame);
    }

    fn video_encoder_capture_texture_frame(
        &mut self,
        index: usize,
        timestamp_ns: u64,
    ) -> Result<(), VideoEncodeError> {
        self.os
            .media
            .av_capture()
            .lock()
            .unwrap()
            .video_encoder_capture_texture_frame(index, timestamp_ns, &mut self.textures)
    }

    fn video_encoder_request_keyframe(&mut self, index: usize) -> Result<(), VideoEncodeError> {
        self.os
            .media
            .av_capture()
            .lock()
            .unwrap()
            .video_encoder_request_keyframe(index)
    }

    fn video_capabilities(&self) -> VideoCapabilities {
        let mut codecs = Vec::new();

        let probe = probe_apple_h264();
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
            supports_keyframe_request: false,
            supports_dynamic_resolution: false,
            width_alignment: Some(2),
            height_alignment: Some(2),
            max_width: None,
            max_height: None,
            max_fps: None,
            max_bitrate: None,
        });

        codecs.push(VideoCodecSupport::unsupported(VideoCodec::H265));
        crate::merge_video_capabilities(
            VideoCapabilities { codecs },
            crate::media_video_capabilities(),
        )
    }

    fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        self.os
            .media
            .av_capture()
            .lock()
            .unwrap()
            .use_video_input(inputs);
    }
}
