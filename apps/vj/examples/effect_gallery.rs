//! Effect gallery — the development/preview rig for the VJ effect
//! renderstack (apps/vj/src/effects).
//!
//!   cargo build --release -p makepad-vj --example effect_gallery
//!   ./target/release/examples/effect_gallery --remote
//!
//! Loads every `.splash` document from `apps/vj/resources/effects/`,
//! shows one at a time in a `VjFxView`, and cycles with Left/Right (or
//! `VJFX_DOC=<name>` to start on a given document). Documents that declare
//! `input0: "test"` get a generated color test pattern bound to texture
//! input 0 — the stand-in for a channel's live video frame in effect-pass
//! mode. The beat is the widget's free-running clock (VJFX_BPM, default
//! 122) so every effect pulses like it would on the VJ's beat bus.

use makepad_widgets::*;

#[path = "../src/effects/mod.rs"]
mod effects;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 800)
                body +: {
                    app_view := SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg +: {color: #x05060a}

                        header := SolidView{
                            width: Fill
                            height: 40.0
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 12.0 right: 12.0}
                            spacing: 12.0
                            draw_bg +: {color: #x11141c}

                            fx_name := H3{
                                text: "loading…"
                                draw_text +: {color: #xf0f4ff}
                            }
                            fx_status := Label{
                                text: ""
                                draw_text +: {color: #x8391a0}
                            }
                            hint := Label{
                                text: "left/right: switch effect"
                                draw_text +: {color: #x5a6472}
                            }
                        }

                        fx_view := mod.widgets.VjFxView{}
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
    #[rust]
    docs: Vec<(String, String)>,
    #[rust]
    current: usize,
    #[rust]
    started: bool,
}

impl App {
    fn docs_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/effects")
    }

    fn load_docs(&mut self) {
        let mut docs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(Self::docs_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("splash") {
                    if let Ok(source) = std::fs::read_to_string(&path) {
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("effect")
                            .to_string();
                        docs.push((name, source));
                    }
                }
            }
        }
        docs.sort_by(|a, b| a.0.cmp(&b.0));
        self.docs = docs;
    }

    fn show(&mut self, cx: &mut Cx, index: usize) {
        if self.docs.is_empty() {
            self.ui
                .label(cx, ids!(fx_name))
                .set_text(cx, "no documents in apps/vj/resources/effects");
            return;
        }
        self.current = index % self.docs.len();
        let (key, source) = self.docs[self.current].clone();
        let widget = self.ui.widget(cx, ids!(fx_view));
        let Some(mut view) = widget.borrow_mut::<effects::VjFxView>() else {
            return;
        };
        let bpm = std::env::var("VJFX_BPM")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(122.0);
        view.set_bpm(bpm);
        let result = view.set_effect_source(cx, &key, &source);
        // Texture-input docs get the runtime's built-in animated fallback
        // automatically — the gallery binds nothing.
        let (title, status) = match result {
            Ok(_) => (
                format!("[{}/{}] {}", self.current + 1, self.docs.len(), key),
                view.status.clone(),
            ),
            Err(e) => (format!("[{}/{}] {} FAILED", self.current + 1, self.docs.len(), key), e),
        };
        drop(view);
        log!("effect_gallery: {} — {}", title, status);
        self.ui.label(cx, ids!(fx_name)).set_text(cx, &title);
        self.ui.label(cx, ids!(fx_status)).set_text(cx, &status);
        self.ui.redraw(cx);
    }
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        crate::effects::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if !self.started {
            // First event after the UI exists: load the library and show
            // the requested (or first) document.
            self.started = true;
            self.load_docs();
            let start = std::env::var("VJFX_DOC")
                .ok()
                .and_then(|want| self.docs.iter().position(|(n, _)| *n == want))
                .unwrap_or(0);
            self.show(cx, start);
        }
        if let Event::KeyDown(ke) = event {
            match ke.key_code {
                KeyCode::ArrowRight => {
                    let next = self.current + 1;
                    self.show(cx, next);
                }
                KeyCode::ArrowLeft => {
                    let prev = (self.current + self.docs.len().max(1)) - 1;
                    self.show(cx, prev);
                }
                _ => {}
            }
        }
        // Scripted driving (the remote bridge's /t goes through the IME
        // path, immune to key repeat): "n" next, "p" prev, "g<name>" jump.
        if let Event::TextInput(te) = event {
            let cmd = te.input.trim();
            if cmd == "n" {
                let next = self.current + 1;
                self.show(cx, next);
            } else if cmd == "p" {
                let prev = (self.current + self.docs.len().max(1)) - 1;
                self.show(cx, prev);
            } else if cmd == "i" {
                // Perf info for the current effect (regen/tick costs).
                let widget = self.ui.widget(cx, ids!(fx_view));
                let line = widget.borrow::<effects::VjFxView>().map(|view| {
                    format!(
                        "effect_gallery perf: regen {:.3} ms, tick {:.3} ms ({} instr){}",
                        view.regen_ms,
                        view.tick_ms,
                        view.tick_instructions,
                        view.tick_error
                            .as_deref()
                            .map(|e| format!(", tick error: {e}"))
                            .unwrap_or_default()
                    )
                });
                if let Some(line) = line {
                    log!("{}", line);
                }
            } else if let Some(name) = cmd.strip_prefix("g") {
                if let Some(at) = self.docs.iter().position(|(n, _)| n == name) {
                    self.show(cx, at);
                }
            }
        }
    }
}
