pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(640, 360)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 14
                        padding: 24

                        title_label := Label{
                            text: "Audio Decode Probe"
                            draw_text.text_style.font_size: 26
                        }

                        status_label := Label{
                            text: "Pending: decoder API not found in platform/src/audio.rs"
                            draw_text.text_style.font_size: 16
                        }

                        detail_label := Label{
                            text: "This example watches the Month 2 audio decoder support API and probes it when present."
                        }

                        summary_label := Label{
                            text: "Expected API: detect_audio_format, AudioBuffer::from_mp3, AudioBuffer::from_ogg_opus, and AudioError."
                        }

                        probe_button := Button{
                            text: "Run Decoder Probe"
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl App {
    fn set_report(&mut self, cx: &mut Cx, report: ProbeReport) {
        self.ui
            .label(cx, ids!(status_label))
            .set_text(cx, report.status);
        self.ui
            .label(cx, ids!(detail_label))
            .set_text(cx, report.detail);
        self.ui
            .label(cx, ids!(summary_label))
            .set_text(cx, report.summary);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(probe_button)).clicked(actions) {
            self.set_report(cx, run_decoder_probe());
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

struct ProbeReport {
    status: &'static str,
    detail: &'static str,
    summary: &'static str,
}

#[cfg(not(makepad_audio_decode_api))]
fn run_decoder_probe() -> ProbeReport {
    ProbeReport {
        status: "Pending: decoder API not found in platform/src/audio.rs",
        detail: "The spec API has not landed yet, so this example is acting as a watcher.",
        summary: "When the API appears, this same runnable probes magic-byte detection and decoder error handling.",
    }
}

#[cfg(makepad_audio_decode_api)]
fn run_decoder_probe() -> ProbeReport {
    use makepad_widgets::makepad_platform::audio::{detect_audio_format, AudioBuffer, AudioError};

    let checks = [
        detect_audio_format(&[0x49, 0x44, 0x33, 0x04]) == Some("mp3"),
        detect_audio_format(&[0xff, 0xfb]) == Some("mp3"),
        detect_audio_format(b"OggS") == Some("ogg"),
        detect_audio_format(&[0; 8]).is_none(),
        detect_audio_format(&[]).is_none(),
        matches!(
            AudioBuffer::from_mp3(&[0x49, 0x44, 0x33, 0x04]),
            Err(AudioError::Mp3Decode(_))
        ),
        matches!(
            AudioBuffer::from_ogg_opus(b"OggS"),
            Err(AudioError::OggOpusDecode(_))
        ),
        matches!(AudioBuffer::from_mp3(&[]), Err(AudioError::EmptyData)),
    ];

    let passed = checks.iter().filter(|passed| **passed).count();

    #[cfg(makepad_audio_decode_fixtures)]
    let fixture_summary = decode_fixture_summary();
    #[cfg(not(makepad_audio_decode_fixtures))]
    let fixture_summary = "Valid decode fixtures are missing; add mono_100ms_44100.mp3 and stereo_100ms_48000.opus.ogg under this example's fixtures directory.";

    if passed == checks.len() {
        ProbeReport {
            status: "Decoder API probe passed",
            detail: "Magic-byte detection and required truncated-data error cases match the spec.",
            summary: fixture_summary,
        }
    } else {
        ProbeReport {
            status: "Decoder API probe failed",
            detail: "At least one required format-detection or error-handling case did not match the spec.",
            summary: fixture_summary,
        }
    }
}

#[cfg(all(makepad_audio_decode_api, makepad_audio_decode_fixtures))]
fn decode_fixture_summary() -> &'static str {
    use makepad_widgets::makepad_platform::audio::AudioBuffer;

    const MP3_FIXTURE: &[u8] = include_bytes!("../fixtures/mono_100ms_44100.mp3");
    const OGG_FIXTURE: &[u8] = include_bytes!("../fixtures/stereo_100ms_48000.opus.ogg");

    let mp3_ok = AudioBuffer::from_mp3(MP3_FIXTURE)
        .map(|buffer| buffer.channel_count() == 1 && buffer.frame_count() >= 4410)
        .unwrap_or(false);
    let ogg_ok = AudioBuffer::from_ogg_opus(OGG_FIXTURE)
        .map(|buffer| buffer.channel_count() == 2 && buffer.frame_count() >= 4800)
        .unwrap_or(false);

    if mp3_ok && ogg_ok {
        "Valid fixture decode passed for mono MP3 and stereo OGG/Opus."
    } else {
        "Valid fixture decode failed for mono MP3 or stereo OGG/Opus."
    }
}
