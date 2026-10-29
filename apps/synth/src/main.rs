//! Makepad Synth: live-coded music. One score (a Splash document that is a
//! flow graph, loop code and an arrangement) open on the SCORE desk
//! (`desk/`), played on the app's own audio root (`audio.rs`) from the
//! audio output callback.

pub use makepad_widgets;

use makepad_widgets::*;

mod about;
mod ai;
mod audio;
mod desk;
mod library;
mod paths;
mod patch_library;
mod resources;
mod song_library;
mod writer;

use ai::Ai;
use desk::Desk;
use resources::Resources;
use song_library::SongLibrary;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let stage = mod.stage

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Makepad Synth"
                window.inner_size: vec2(1400 900)
                pass +: {clear_color: stage.bg}
                // One bar: the window's controls where the platform keeps
                // them, and the score's caption (`desk::SynthCaption`) in the
                // rest. The stock title goes (the score's name is the title).
                caption_bar +: {
                    draw_bg.color: stage.bg
                    caption_label +: {
                        flow: Right spacing: 0 align: Align{x: 0.0 y: 0.5}
                        label +: {visible: false}
                        caption_icon +: {width: 0 height: 0 margin: 0}
                        // the traffic lights' room where macOS draws them over the caption
                        caption_window_gap := View{width: 72 height: 1}
                        score_caption := mod.widgets.SynthCaption{}
                    }
                }
                body +: {
                    padding: 0 margin: 0 spacing: 0
                    desk := mod.widgets.SynthDesk{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    desk: Desk,
    /// The chat's service: the score's tools.
    #[rust]
    ai: Ai,
    /// The downloads (the GM SoundFont, the song library) and the songs.
    #[rust]
    resources: Resources,
    #[rust]
    songs: SongLibrary,
    /// The default output devices, opened once the first event names them
    /// (a muted, hidden instance renders and writes silence).
    #[rust]
    output_devices: Vec<AudioDeviceId>,
}

/// A hidden instance (tests, headless runs) makes no sound.
fn muted() -> bool {
    std::env::var_os("SANDBOX_MUTE").is_some_and(|v| v != "0")
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let mut output = self.desk.engine.output();
        cx.audio_output(0, move |info, buffer| output.fill(info.sample_rate, buffer));
        self.desk.engine.set_muted(muted());
        let ui = self.ui.clone();
        self.desk.start(cx, &ui);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let ui = self.ui.clone();
        self.desk.handle_actions(cx, &ui, actions);
        self.resources.handle_actions(cx, &ui, actions);
        about::handle_actions(cx, &ui, actions);
        let wants_resources = self.songs.handle_actions(cx, &ui, &mut self.desk, actions);
        if wants_resources || ui.button(cx, ids!(score_resources)).clicked(actions) || ui.button(cx, ids!(score_gm_install)).clicked(actions) {
            self.resources.open(cx, &ui);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        makepad_code_editor::script_mod(vm);
        // Inline value controls in the code view.
        makepad_code_values::script_mod(vm);
        makepad_app_aichat::script_mod(vm);
        // The console look the desk paints with (`mod.stage`).
        makepad_look::script_mod(vm);
        // The editor panels: every panel isolate gets the panel widgets, the
        // synth widgets and `scene` over the score's document kind.
        makepad_value_pickers::script_mod(vm);
        makepad_editor_panel::register(vm, desk::score_doc::score_doc(), &[makepad_synth_panel::isolate_mod]);
        resources::script_mod(vm);
        about::script_mod(vm);
        song_library::script_mod(vm);
        desk::register(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::AudioDevices(devices) = event {
            self.output_devices = devices.default_output();
            cx.use_audio_outputs(&self.output_devices);
        }
        self.match_event(cx, event);
        let ui = self.ui.clone();
        self.ai.handle_event(cx, &ui, &mut self.desk, event);
        self.resources.handle_event(cx, &ui, event);
        if self.resources.take_changed() {
            self.desk.gm_changed();
        }
        if self.resources.take_catalogued() {
            self.songs.reload();
        }
        self.songs.handle_event(cx, &ui, &mut self.desk, event);
        self.desk.handle_event(cx, &ui, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
