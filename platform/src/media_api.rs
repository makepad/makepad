use crate::{
    audio::{AudioBuffer, AudioDeviceId, AudioInfo, AudioInputFn, AudioInputOptions, AudioOutputFn},
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
    /// `use_audio_inputs` with per-capture options (e.g. echo cancellation).
    /// Platforms without an implementation ignore the options.
    fn use_audio_inputs_with_options(
        &mut self,
        devices: &[AudioDeviceId],
        _options: AudioInputOptions,
    ) {
        self.use_audio_inputs(devices);
    }
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

    /// Install the app's output callback, wrapped so every buffer it fills
    /// is also offered to the audio-output taps (see
    /// [`crate::audio_output_tap`]) — one seam, so a recorder does not have
    /// to be re-plumbed into each backend's realtime callback.
    ///
    /// The same seam fences it (see [`crate::audio_output_fence`]): a panic
    /// in the app's closure costs that closure and its buffer, not the
    /// device thread it was called from. The taps are fed either way, so a
    /// recording keeps its place through a fault rather than ending at it.
    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        let mut fenced = crate::audio_output_fence::FencedOutput::new(index, f);
        self.audio_output_box_os(
            index,
            Box::new(move |info, buffer| {
                fenced.call(info, buffer);
                crate::audio_output_tap::feed_audio_output_tap(info, buffer);
            }),
        )
    }

    /// Backend-implemented half of [`Self::audio_output_box`]. Apps call the
    /// wrapper; only the OS media layers implement this.
    fn audio_output_box_os(&mut self, index: usize, f: AudioOutputFn);
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

/// The seam where an app's output closure is wrapped: the fence, then the
/// taps. Every test takes an index of its own so nothing here can be read
/// as another test's fault, and only the tap test uses index 0, which is
/// the one the taps listen to.
///
/// Under `panic = "abort"` there is nothing to catch and these cannot run.
#[cfg(all(test, panic = "unwind"))]
mod tests {
    use super::*;
    use crate::audio::{AudioDeviceId, MAX_AUDIO_DEVICE_INDEX};
    use crate::audio_output_fence::{
        audio_output_panics, take_audio_output_panic_note,
    };
    use crate::live_id::LiveId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A backend that keeps the wrapped closure instead of handing it to a
    /// device, so the seam can be called by hand. Nothing else on the trait
    /// is reachable from these tests.
    #[derive(Default)]
    struct Bench {
        installed: Option<AudioOutputFn>,
    }

    impl CxMediaApi for Bench {
        fn audio_output_box_os(&mut self, _index: usize, f: AudioOutputFn) {
            self.installed = Some(f);
        }
        fn midi_input(&mut self) -> MidiInput {
            unimplemented!()
        }
        fn midi_output(&mut self) -> MidiOutput {
            unimplemented!()
        }
        fn midi_reset(&mut self) {
            unimplemented!()
        }
        fn use_midi_inputs(&mut self, _ports: &[MidiPortId]) {
            unimplemented!()
        }
        fn use_midi_outputs(&mut self, _ports: &[MidiPortId]) {
            unimplemented!()
        }
        fn use_audio_inputs(&mut self, _devices: &[AudioDeviceId]) {
            unimplemented!()
        }
        fn use_audio_outputs(&mut self, _devices: &[AudioDeviceId]) {
            unimplemented!()
        }
        fn audio_input_box(&mut self, _index: usize, _f: AudioInputFn) {
            unimplemented!()
        }
        fn use_video_input(&mut self, _devices: &[(VideoInputId, VideoFormatId)]) {
            unimplemented!()
        }
        fn video_input_box(&mut self, _index: usize, _f: VideoInputFn) {
            unimplemented!()
        }
    }

    fn info() -> AudioInfo {
        AudioInfo { device_id: AudioDeviceId(LiveId(11)), time: None, sample_rate: 48_000.0 }
    }

    fn loud() -> AudioBuffer {
        let mut buffer = AudioBuffer::new_with_size(64, 2);
        for sample in buffer.data.iter_mut() {
            *sample = 1.0;
        }
        buffer
    }

    fn seam<F>(index: usize, f: F) -> AudioOutputFn
    where
        F: FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static,
    {
        let mut bench = Bench::default();
        bench.audio_output(index, f);
        bench.installed.expect("the seam installs what it wrapped")
    }

    #[test]
    fn a_panic_in_the_output_callback_does_not_reach_the_device_thread() {
        let mut call = seam(3, |_info, _buffer| panic!("a deck with no record"));
        let mut buffer = loud();
        // Returning at all is the law: on the desktop backends this frame
        // holds the lock the next buffer needs, and on the mobile, web and
        // Apple ones it is a foreign frame an unwind may not cross.
        call(info(), &mut buffer);
        assert_eq!(audio_output_panics(3), 1);
    }

    #[test]
    fn the_buffer_a_panic_interrupted_is_handed_over_as_silence_not_as_what_was_in_it() {
        let mut call = seam(4, |_info, buffer| {
            for sample in buffer.data.iter_mut() {
                *sample = 1.0;
            }
            panic!("half way through");
        });
        let mut buffer = loud();
        call(info(), &mut buffer);
        assert!(
            buffer.data.iter().all(|sample| *sample == 0.0),
            "the backends reuse one buffer, so a half-written one plays as a stuck loop"
        );
    }

    #[test]
    fn a_callback_that_has_panicked_is_never_called_again_and_its_output_stays_silent() {
        let entered = Arc::new(AtomicUsize::new(0));
        let seen = entered.clone();
        let mut call = seam(5, move |_info, _buffer| {
            seen.fetch_add(1, Ordering::SeqCst);
            panic!("once is enough");
        });
        for _ in 0..4 {
            let mut buffer = loud();
            call(info(), &mut buffer);
            assert!(buffer.data.iter().all(|sample| *sample == 0.0));
        }
        assert_eq!(entered.load(Ordering::SeqCst), 1, "retired means retired");
        assert_eq!(audio_output_panics(5), 1);
    }

    #[test]
    fn a_retired_callback_is_kept_alive_rather_than_freed_on_the_audio_thread() {
        struct Witness(Arc<AtomicUsize>);
        impl Drop for Witness {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let freed = Arc::new(AtomicUsize::new(0));
        let witness = Witness(freed.clone());
        let mut call = seam(6, move |_info, _buffer| {
            let _held = &witness;
            panic!("and everything it holds");
        });
        for _ in 0..11 {
            call(info(), &mut loud());
        }
        assert_eq!(
            freed.load(Ordering::SeqCst),
            0,
            "freeing an app's mix graph is not something a realtime thread may do"
        );
    }

    #[test]
    fn a_panic_is_counted_against_its_own_output_and_no_other() {
        let mut room = seam(7, |_info, _buffer| panic!("the room"));
        room(info(), &mut loud());
        assert_eq!(audio_output_panics(7), 1);
        assert_eq!(audio_output_panics(8), 0, "the monitor is a different output");
    }

    #[test]
    fn a_panic_leaves_its_own_words_for_the_app_to_say_on_its_own_thread() {
        let mut call = seam(9, |_info, _buffer| panic!("a voice with no frames"));
        call(info(), &mut loud());
        // A second panic on the same output must not displace the first
        // words: dropping the earlier payload would be a free on the
        // realtime thread.
        let mut again = seam(9, |_info, _buffer| panic!("and a later one"));
        again(info(), &mut loud());
        let said = take_audio_output_panic_note(9).expect("the words were parked");
        assert!(said.contains("a voice with no frames"), "{said}");
        assert_eq!(take_audio_output_panic_note(9), None, "taken once");
        assert_eq!(audio_output_panics(9), 2, "both faults counted");
    }

    /// A recording keeps its place through a fault rather than ending at
    /// it: today a panic skips the tap feed and takes the device thread
    /// with it, so a recorder loses that buffer and every one after.
    /// Fenced, it is handed the silence and goes on being handed buffers.
    ///
    /// Taps receive every output, so distinguish this test's device from
    /// the callbacks other tests can feed concurrently.
    #[test]
    fn the_taps_are_handed_the_silence_too_so_a_recording_keeps_its_place() {
        let tap_info = AudioInfo { device_id: AudioDeviceId(LiveId(12)), ..info() };
        let fed = Arc::new(AtomicUsize::new(0));
        let quiet = Arc::new(AtomicUsize::new(0));
        let (seen, was_quiet) = (fed.clone(), quiet.clone());
        let id = crate::audio_output_tap::add_audio_output_tap(move |info, buffer| {
            if info.device_id != tap_info.device_id {
                return;
            }
            seen.fetch_add(1, Ordering::SeqCst);
            if buffer.data.iter().all(|sample| *sample == 0.0) {
                was_quiet.fetch_add(1, Ordering::SeqCst);
            }
        });
        let mut call = seam(0, |_info, buffer| {
            for sample in buffer.data.iter_mut() {
                *sample = 1.0;
            }
            panic!("mid buffer");
        });
        call(tap_info, &mut loud());
        call(tap_info, &mut loud());
        crate::audio_output_tap::remove_audio_output_tap(id);
        assert_eq!(fed.load(Ordering::SeqCst), 2, "the fault buffer and the one after");
        assert_eq!(quiet.load(Ordering::SeqCst), 2, "and both were silence");
    }

    #[test]
    fn an_output_index_past_the_end_is_counted_by_nobody_rather_than_panicking() {
        assert_eq!(audio_output_panics(MAX_AUDIO_DEVICE_INDEX), 0);
        assert_eq!(take_audio_output_panic_note(MAX_AUDIO_DEVICE_INDEX), None);
        let mut call = seam(MAX_AUDIO_DEVICE_INDEX, |_info, _buffer| panic!("off the end"));
        call(info(), &mut loud());
    }
}
