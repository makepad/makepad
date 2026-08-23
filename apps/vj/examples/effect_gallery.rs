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
    /// `VJFX_INPUT=<image path>`: decoded once, bound as REAL content on
    /// input 0 for every shown effect — the content-coupling verify lever
    /// (without it, effects run standalone on the animated fallback).
    #[rust]
    input_tex: Option<Texture>,
    /// Deck stand-ins for transition docs: [deck A, deck B, premix]. A
    /// transition rendered with nothing bound is a BLACK frame — and a
    /// black gallery grab reads as a broken document.
    #[rust]
    trans_tex: [Option<Texture>; 3],
    /// `VJFX_SWEEP=<dir>`: the parity sweep — grab every document once at
    /// the pinned clock, then quit. See [`Sweep`].
    #[rust]
    sweep: Option<Sweep>,
    #[rust]
    frame: NextFrame,
}

/// THE PARITY SWEEP (`VJFX_SWEEP=<dir>`, with `VJFX_CAPTURE=<frames>`).
///
/// A self-terminating capture run: load document i, let the widget advance
/// its fixed frame budget and freeze, write one PNG, move on, quit after
/// the last one. Every grab is then a pure function of the document, so
/// two sweeps over two builds compare pixel for pixel — which is how a
/// shader migration proves it changed no look. Existing PNGs are skipped,
/// so an interrupted sweep resumes instead of restarting.
#[derive(Default)]
pub struct Sweep {
    dir: std::path::PathBuf,
    /// Frames still to let pass before asking for this document's grab.
    settle: u32,
    /// Frames spent waiting for the PNG file to appear (bounded).
    waited: u32,
    /// Where the current grab is being written (None = still settling).
    pending: Option<std::path::PathBuf>,
    done: bool,
}

impl App {
    /// `VJFX_DIR=<dir>` points the gallery at another preset directory —
    /// how the migration measured its own cost, by running ONE binary over
    /// the pre- and post-migration document sets.
    fn docs_dir() -> std::path::PathBuf {
        match std::env::var("VJFX_DIR") {
            Ok(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
            _ => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/effects"),
        }
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
        // `VJFX_ONLY=a,b,c` keeps just the documents whose name contains one
        // of the fragments — a one-family sweep instead of the whole
        // library (and a much shorter capture run).
        if let Ok(only) = std::env::var("VJFX_ONLY") {
            let want: Vec<String> = only.split(',').map(|s| s.trim().to_string()).collect();
            docs.retain(|(n, _)| want.iter().any(|w| !w.is_empty() && n.contains(w.as_str())));
        }
        self.docs = docs;
    }

    /// `VJFX_CAPTURE=<frames>` or `<frames>@<dt seconds>` (default step
    /// 1/60 s). Unset = the live free-running clock.
    fn capture_config() -> Option<(f64, u32)> {
        let spec = std::env::var("VJFX_CAPTURE").ok()?;
        let (frames, dt) = match spec.split_once('@') {
            Some((f, d)) => (f, d.parse::<f64>().ok()?),
            None => (spec.as_str(), 1.0 / 60.0),
        };
        Some((dt.clamp(0.0001, 1.0), frames.parse::<u32>().ok()?.clamp(1, 100_000)))
    }

    /// One sweep step, driven off the app's own frame timer. Waits out the
    /// widget's capture budget, asks for the PNG, waits for it to land,
    /// advances — and quits when the library is exhausted.
    fn sweep_step(&mut self, cx: &mut Cx) {
        let Some(sweep) = &mut self.sweep else { return };
        if sweep.done {
            return;
        }
        if let Some(path) = sweep.pending.clone() {
            sweep.waited += 1;
            if !path.exists() && sweep.waited < 240 {
                return;
            }
            if !path.exists() {
                log!("effect_gallery sweep: NO GRAB for {}", path.display());
            }
            sweep.pending = None;
            sweep.waited = 0;
            let next = self.current + 1;
            if next >= self.docs.len() {
                if let Some(sweep) = &mut self.sweep {
                    sweep.done = true;
                }
                log!("effect_gallery sweep: complete ({} documents)", self.docs.len());
                cx.quit();
                return;
            }
            self.sweep_show(cx, next);
            return;
        }
        if sweep.settle > 0 {
            sweep.settle -= 1;
            return;
        }
        // The widget has frozen on its last capture frame: whatever is on
        // screen now is the document's deterministic frame.
        let (name, _) = self.docs[self.current].clone();
        let path = sweep.dir.join(format!("{name}.png"));
        sweep.pending = Some(path.clone());
        cx.capture_next_frame_to_file(path);
    }

    /// Load document `index` for the sweep, skipping any already grabbed.
    fn sweep_show(&mut self, cx: &mut Cx, index: usize) {
        let mut index = index;
        let (budget, dir) = match &self.sweep {
            Some(s) => (Self::capture_config().map(|c| c.1).unwrap_or(90), s.dir.clone()),
            None => return,
        };
        while index < self.docs.len() {
            let name = self.docs[index].0.clone();
            if !dir.join(format!("{name}.png")).exists() {
                break;
            }
            index += 1;
        }
        if index >= self.docs.len() {
            if let Some(sweep) = &mut self.sweep {
                sweep.done = true;
            }
            log!("effect_gallery sweep: complete (nothing left to grab)");
            cx.quit();
            return;
        }
        self.show(cx, index);
        if let Some(sweep) = &mut self.sweep {
            // The widget freezes after `budget` frames; a small margin
            // covers the load frame and the compositor catching up.
            sweep.settle = budget + 12;
            sweep.waited = 0;
            sweep.pending = None;
        }
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
        // DETERMINISTIC CAPTURE (the parity harness): `VJFX_CAPTURE=<frames>`
        // — optionally `<frames>@<dt>` — makes the widget advance by a fixed
        // step for exactly that many frames after the load and then freeze,
        // so a grab is a pure function of the document. This is what a
        // before/after migration sweep compares; without it the wall clock
        // decides what is on screen and no two grabs match.
        view.set_capture(Self::capture_config());
        // LOAD COST: document eval + hook-object build + engine build. The
        // shader itself compiles lazily on first draw, so this is the CPU
        // half of "what does a document with its own shader cost?".
        let t0 = std::time::Instant::now();
        let result = view.set_effect_source(cx, &key, &source);
        let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // Content coupling verify lever: VJFX_INPUT=<image> binds a real
        // texture to input 0 (a stand-in for the channel's live video).
        if let Ok(path) = std::env::var("VJFX_INPUT") {
            if self.input_tex.is_none() && !path.is_empty() {
                match std::fs::read(&path) {
                    Ok(bytes) => match decode_image_from_data(&bytes) {
                        Ok(buf) => self.input_tex = Some(buf.into_new_texture(cx)),
                        Err(e) => log!("VJFX_INPUT {path}: decode failed: {e:?}"),
                    },
                    Err(e) => log!("VJFX_INPUT {path}: {e}"),
                }
            }
            if let Some(tex) = &self.input_tex {
                view.set_input_texture(0, Some(tex.clone()));
            }
        }
        // Transition docs render BLACK with nothing bound — give them the
        // same two distinct deck stand-ins the thumbnail renderer uses, so
        // the gallery (and any sweep grab) shows the transition working.
        // Two-deck docs get separate inputs and the default mid fader;
        // premix transitions get one mid-dissolve frame on input 0.
        if view.wants_deck_inputs() {
            let a = gallery_deck_pattern(cx, &mut self.trans_tex[0], 0.0);
            let b = gallery_deck_pattern(cx, &mut self.trans_tex[1], 1.0);
            view.set_input_texture(0, Some(a));
            view.set_input_texture(1, Some(b));
        } else if effects::seed::is_transition_preset(&key) && self.input_tex.is_none() {
            let premix = gallery_deck_pattern(cx, &mut self.trans_tex[2], 0.5);
            view.set_input_texture(0, Some(premix));
        }
        // Otherwise texture-input docs get the runtime's built-in animated
        // fallback automatically — the gallery binds nothing.
        let (title, status) = match result {
            Ok(_) => (
                format!("[{}/{}] {}", self.current + 1, self.docs.len(), key),
                view.status.clone(),
            ),
            Err(e) => (format!("[{}/{}] {} FAILED", self.current + 1, self.docs.len(), key), e),
        };
        drop(view);
        log!("effect_gallery: {} — {} [load {:.2} ms]", title, status, load_ms);
        self.ui.label(cx, ids!(fx_name)).set_text(cx, &title);
        self.ui.label(cx, ids!(fx_status)).set_text(cx, &status);
        self.ui.redraw(cx);
    }
}

/// Two visibly different deck stand-ins, dissolved by `m`: a warm gradient
/// with a bright disc (m = 0, "deck A") and a cool grid with a vertical bar
/// (m = 1, "deck B") — the same visual contract as the thumbnail renderer's
/// transition inputs, static so capture sweeps stay deterministic.
fn gallery_deck_pattern(cx: &mut Cx, slot: &mut Option<Texture>, m: f32) -> Texture {
    const W: usize = 192;
    const H: usize = 120;
    if let Some(tex) = slot {
        return tex.clone();
    }
    let mut data = vec![0u32; W * H];
    for y in 0..H {
        let v = y as f32 / H as f32;
        for x in 0..W {
            let u = x as f32 / W as f32;
            let mut ar = 0.85 - 0.5 * v;
            let mut ag = 0.45 + 0.3 * u;
            let mut ab = 0.15 + 0.2 * (1.0 - u);
            let (ddx, ddy) = (u - 0.62, v - 0.4);
            if ddx * ddx + ddy * ddy < 0.02 {
                ar = 1.0;
                ag = 0.95;
                ab = 0.7;
            }
            let grid = x % 24 < 2 || y % 24 < 2;
            let mut br = 0.08;
            let mut bg = 0.25 + 0.35 * v;
            let mut bb = 0.7 + 0.3 * (1.0 - v);
            if grid {
                br = 0.4;
                bg = 0.9;
                bb = 1.0;
            }
            if (u - 0.3).abs() < 0.03 {
                br = 0.9;
                bg = 1.0;
                bb = 1.0;
            }
            let r = ((ar + (br - ar) * m).clamp(0.0, 1.0) * 255.0) as u32;
            let g = ((ag + (bg - ag) * m).clamp(0.0, 1.0) * 255.0) as u32;
            let b = ((ab + (bb - ab) * m).clamp(0.0, 1.0) * 255.0) as u32;
            data[y * W + x] = 0xff00_0000 | (r << 16) | (g << 8) | b;
        }
    }
    let tex = Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 {
            width: W,
            height: H,
            data: Some(data),
            updated: TextureUpdated::Full,
        },
    );
    *slot = Some(tex.clone());
    tex
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
            if let Ok(dir) = std::env::var("VJFX_SWEEP") {
                let dir = std::path::PathBuf::from(dir);
                let _ = std::fs::create_dir_all(&dir);
                self.sweep = Some(Sweep { dir, ..Default::default() });
                self.frame = cx.new_next_frame();
                self.sweep_show(cx, 0);
                return;
            }
            let start = std::env::var("VJFX_DOC")
                .ok()
                .and_then(|want| self.docs.iter().position(|(n, _)| *n == want))
                .unwrap_or(0);
            self.show(cx, start);
        }
        if self.frame.is_event(event).is_some() {
            self.frame = cx.new_next_frame();
            self.sweep_step(cx);
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
                        "effect_gallery perf: regen {:.3} ms, tick {:.3} ms ({} instr), \
                         sim {:.3} ms{}",
                        view.regen_ms,
                        view.tick_ms,
                        view.tick_instructions,
                        view.sim_ms,
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
