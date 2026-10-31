//! MilkDrop explorer: browse a preset pack by category, search it, rate and
//! favourite presets, and play them through `MilkdropView` driven by a
//! synthetic beat (no microphone, nothing played aloud).
//!
//! The browsing is the shared [`MilkdropBrowser`] widget, which keeps the
//! pack in the MilkDrop data folder every app shares
//! (`makepad_milkdrop::browser::data_dir`) and offers to download projectM's
//! "cream of the crop" pack while it is missing.
//!
//! Keys: Cmd/Ctrl+→ and ← next and previous in the shown list, Cmd/Ctrl+R a
//! random preset, Cmd/Ctrl+D favourite, Cmd/Ctrl+1…5 rate (0 clears).

pub use makepad_milkdrop::makepad_widgets;

use makepad_milkdrop::{MilkdropBrowserWidgetRefExt, MilkdropViewWidgetRefExt, PresetPick};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "MilkDrop Explorer"
                window.inner_size: vec2(1400 900)
                body +: {
                    flow: Overlay
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        show_bg: true
                        draw_bg +: {color: theme.color_bg_app}
                        stage := View{
                            width: Fill
                            height: 520
                            show_bg: true
                            draw_bg +: {color: #x000000}
                            milk := MilkdropView{width: Fill height: Fill}
                        }
                        View{
                            width: Fill height: Fit flow: Right spacing: 8 padding: Inset{left: 10 right: 10 top: 6 bottom: 6}
                            align: Align{y: 0.5}
                            previous := Button{text: "Previous"}
                            random := Button{text: "Random"}
                            next := Button{text: "Next"}
                            favourite := Button{text: "Favourite"}
                            now := Label{width: Fill text: ""}
                        }
                        presets := MilkdropBrowser{padding: Inset{left: 10 right: 10 bottom: 8}}
                    }
                    menus := MenuLayer{}
                }
            }
        }
    }
}

// ---- the synthetic beat ----

/// A 124 BPM kick, a hi-hat on the off-beats and a slow bass sweep: enough
/// for bass, mid and treble to move, generated per frame, never played.
struct Beat {
    rate: f32,
    t: f64,
    noise: u32,
    buffer: Vec<f32>,
}

impl Beat {
    fn new() -> Self {
        Self { rate: 44_100.0, t: 0.0, noise: 0x1234_5678, buffer: Vec::new() }
    }

    /// Samples covering `dt` seconds (at most a tenth of a second).
    fn fill(&mut self, dt: f64) -> &[f32] {
        let count = ((dt.clamp(0.0, 0.1)) * self.rate as f64) as usize;
        self.buffer.clear();
        let beat = 60.0 / 124.0;
        for _ in 0..count {
            let t = self.t;
            let phase = t % beat;
            let kick = (-phase * 18.0).exp() * (std::f64::consts::TAU * (50.0 + 90.0 * (-phase * 30.0).exp()) * phase).sin();
            let off = (t + beat * 0.5) % beat;
            self.noise ^= self.noise << 13;
            self.noise ^= self.noise >> 17;
            self.noise ^= self.noise << 5;
            let hat = (-off * 60.0).exp() * ((self.noise as f64 / u32::MAX as f64) * 2.0 - 1.0);
            let sweep = 0.25 * (std::f64::consts::TAU * (110.0 + 80.0 * (t * 0.2).sin()) * t).sin();
            self.buffer.push((0.8 * kick + 0.3 * hat + sweep) as f32 * 0.6);
            self.t += 1.0 / self.rate as f64;
        }
        &self.buffer
    }
}

// ---- the app ----

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust(Beat::new())]
    beat: Beat,
    /// Whether the view plays (from the first preset on).
    #[rust]
    playing: bool,
    #[rust]
    beat_frame: NextFrame,
    #[rust]
    last_frame: Option<f64>,
}

impl App {
    /// Load `pick` into the view (read and parsed off the UI thread, then
    /// faded to) and start the beat.
    fn play(&mut self, cx: &mut Cx, pick: Option<PresetPick>) {
        let Some(pick) = pick else { return };
        self.ui.label(cx, ids!(now)).set_text(cx, &pick.line());
        let view = self.ui.milkdrop_view(cx, ids!(milk));
        view.load_preset(cx, &pick.path);
        if !self.playing {
            self.playing = true;
            view.set_playing(cx, true);
            self.beat_frame = cx.new_next_frame();
        }
    }

    fn handle_key(&mut self, cx: &mut Cx, key: &KeyEvent) {
        if !(key.modifiers.logo || key.modifiers.control) {
            return;
        }
        let presets = self.ui.milkdrop_browser(cx, ids!(presets));
        match key.key_code {
            KeyCode::ArrowRight => {
                let pick = presets.step(cx, 1);
                self.play(cx, pick);
            }
            KeyCode::ArrowLeft => {
                let pick = presets.step(cx, -1);
                self.play(cx, pick);
            }
            KeyCode::KeyR => {
                let pick = presets.random(cx);
                self.play(cx, pick);
            }
            KeyCode::KeyD => {
                presets.toggle_favourite(cx);
            }
            KeyCode::Key0 => presets.rate(cx, 0),
            KeyCode::Key1 => presets.rate(cx, 1),
            KeyCode::Key2 => presets.rate(cx, 2),
            KeyCode::Key3 => presets.rate(cx, 3),
            KeyCode::Key4 => presets.rate(cx, 4),
            KeyCode::Key5 => presets.rate(cx, 5),
            _ => {}
        }
    }

    /// Feed the view the next slice of the synthetic beat, while it plays.
    fn feed_beat(&mut self, cx: &mut Cx, time: f64) {
        if !self.playing {
            self.last_frame = None;
            return;
        }
        let view = self.ui.milkdrop_view(cx, ids!(milk));
        let dt = self.last_frame.map(|last| time - last).unwrap_or(1.0 / 60.0);
        self.last_frame = Some(time);
        let rate = self.beat.rate;
        view.feed_audio(self.beat.fill(dt), rate);
        self.beat_frame = cx.new_next_frame();
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let presets = self.ui.milkdrop_browser(cx, ids!(presets));
        if let Some(pick) = presets.played(actions) {
            self.play(cx, Some(pick));
        }
        if self.ui.button(cx, ids!(next)).clicked(actions) {
            let pick = presets.step(cx, 1);
            self.play(cx, pick);
        }
        if self.ui.button(cx, ids!(previous)).clicked(actions) {
            let pick = presets.step(cx, -1);
            self.play(cx, pick);
        }
        if self.ui.button(cx, ids!(random)).clicked(actions) {
            let pick = presets.random(cx);
            self.play(cx, pick);
        }
        if self.ui.button(cx, ids!(favourite)).clicked(actions) {
            presets.toggle_favourite(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_milkdrop::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(frame) = self.beat_frame.is_event(event) {
            self.feed_beat(cx, frame.time);
        }
        if let Event::KeyDown(key) = event {
            self.handle_key(cx, key);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
