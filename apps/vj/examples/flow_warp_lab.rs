//! flow_warp_lab — standalone rig for the FLOW-WARP playback path
//! (apps/vj/src/flow_warp.rs): loads one enhance-service mp4 (with its
//! `mkfl` motion payload), runs the GPU warp pass full-window, and exposes
//! the pair-space transport to the remote bridge.
//!
//!   cargo build --release -p makepad-vj --example flow_warp_lab
//!   VJ_FLOW_CLIP=/path/clip.mp4 ./target/release/examples/flow_warp_lab --remote
//!
//! Text commands (send via the bridge's `/t?t=...`, the IME path):
//!
//!   rate<f>   set playback rate (any float, negative runs backwards)
//!   pos<f>    park the clock at pair-space position <f> (pauses)
//!   b         toggle bounce      p   toggle play/pause
//!   i         log perf line (pass ms, measured fps)
//!
//! `VJ_FLOW_DUMP_PAIR=<k>` additionally decodes pair k's endpoint frames and
//! the BAKED in-between (video frame k·stride + stride/2 — RIFE's own t=0.5
//! answer on a tweened clip) as raw BGRA files into `VJ_FLOW_DUMP_DIR`, for
//! pixel comparison against a grab of the warp parked at `pos<k>.5`.

use makepad_widgets::makepad_platform::video_file::{nv12, VideoFileDecoder};
use makepad_widgets::*;
use std::sync::mpsc::{channel, Receiver};

#[path = "../src/clock.rs"]
mod clock;

// The lab compiles the flow modules standalone; host-facing API the lab
// does not exercise (clear/output/seek — main.rs's surface) stays quiet.
#[path = "../src/flow.rs"]
#[allow(dead_code)]
mod flow;
#[path = "../src/flow_warp.rs"]
#[allow(dead_code)]
mod flow_warp;

use flow_warp::{FlowClipData, FlowWarpView};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(640, 396)
                body +: {
                    SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg +: {color: #x05060a}

                        header := View{
                            width: Fill
                            height: 44
                            flow: Down
                            padding: Inset{left: 8.0 top: 4.0 right: 8.0}
                            hud := Label{
                                text: "flow_warp_lab: waiting for clip…"
                                draw_text +: {color: #xd0e0f0}
                                draw_text.text_style.font_size: 9
                            }
                            hud2 := Label{
                                text: "rate<f> pos<f> b p i"
                                draw_text +: {color: #x8391a0}
                                draw_text.text_style.font_size: 9
                            }
                        }

                        warp := mod.widgets.FlowWarpView{
                            composite: true
                            width: Fill
                            height: Fill
                        }
                    }
                }
            }
        }
    }
}

type PreparedMsg = Result<Option<Box<FlowClipData>>, String>;

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started: bool,
    #[rust]
    rx: Option<Receiver<PreparedMsg>>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: Option<f64>,
    /// Rolling display-frame intervals for the fps line.
    #[rust]
    frame_dts: Vec<f64>,
}

fn clip_path() -> Option<String> {
    std::env::var("VJ_FLOW_CLIP").ok().or_else(|| {
        std::env::args().skip(1).find(|a| a.ends_with(".mp4"))
    })
}

/// Decode specific video frame indices to raw BGRA files (worker side).
fn dump_frames(path: &str, dir: &str, wanted: &[u64]) {
    let mut decoder = match VideoFileDecoder::open(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("flow_warp_lab dump: open failed: {e}");
            return;
        }
    };
    let _ = std::fs::create_dir_all(dir);
    let mut rgb = Vec::new();
    let mut index: u64 = 0;
    let max = wanted.iter().copied().max().unwrap_or(0);
    loop {
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                if wanted.contains(&index) {
                    nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, &mut rgb);
                    let mut bgra =
                        Vec::with_capacity((frame.width * frame.height) as usize * 4);
                    for px in rgb.chunks_exact(3) {
                        bgra.extend_from_slice(&[px[2], px[1], px[0], 0xff]);
                    }
                    let file = format!(
                        "{dir}/frame-{index}-{}x{}.bgra",
                        frame.width, frame.height
                    );
                    match std::fs::write(&file, &bgra) {
                        Ok(_) => eprintln!("flow_warp_lab dump: wrote {file}"),
                        Err(e) => eprintln!("flow_warp_lab dump: {file}: {e}"),
                    }
                }
                if index >= max {
                    return;
                }
                index += 1;
            }
            Ok(None) => return,
            Err(e) => {
                eprintln!("flow_warp_lab dump: decode failed: {e}");
                return;
            }
        }
    }
}

impl App {
    fn start_load(&mut self) {
        let Some(path) = clip_path() else {
            log!("flow_warp_lab: set VJ_FLOW_CLIP=/path/clip.mp4");
            return;
        };
        let (tx, rx) = channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let prepared = flow_warp::prepare_flow_clip(std::path::Path::new(&path));
            if let (Ok(Some(data)), Ok(pair)) = (
                &prepared,
                std::env::var("VJ_FLOW_DUMP_PAIR").map(|v| v.parse::<u64>().unwrap_or(0)),
            ) {
                let dir = std::env::var("VJ_FLOW_DUMP_DIR")
                    .unwrap_or_else(|_| "/tmp/flow_warp_lab".into());
                let s = data.stride as u64;
                // Pair endpoints and — on a tweened clip — the baked mid.
                let mut wanted = vec![pair * s, (pair + 1) * s];
                if s >= 2 {
                    wanted.push(pair * s + s / 2);
                }
                dump_frames(&path, &dir, &wanted);
            }
            let _ = tx.send(prepared);
        });
    }

    fn with_warp<R>(
        &mut self,
        cx: &mut Cx,
        f: impl FnOnce(&mut Cx, &mut FlowWarpView) -> R,
    ) -> Option<R> {
        let widget = self.ui.widget(cx, ids!(warp));
        let mut view = widget.borrow_mut::<FlowWarpView>()?;
        Some(f(cx, &mut view))
    }

    fn hud_line(&mut self, cx: &mut Cx) -> String {
        let fps = if self.frame_dts.is_empty() {
            0.0
        } else {
            self.frame_dts.len() as f64 / self.frame_dts.iter().sum::<f64>()
        };
        self.with_warp(cx, |_cx, view| {
            let pos = view.position_pairs();
            let pair = pos.floor().min((view.pairs().max(1) - 1) as f64);
            format!(
                "pair {:.0}  t {:.3}  pos {:.2}/{}  {:.2}s  rate {:+.3}  bounce {}  {}  |  {:.0} fps  pass {:.2} ms",
                pair,
                pos - pair,
                pos,
                view.pairs(),
                view.position_secs(),
                view.rate(),
                if view.bounce() { "ON" } else { "off" },
                if view.is_playing() { "PLAY" } else { "PAUSED" },
                fps,
                view.last_pass_ms,
            )
        })
        .unwrap_or_else(|| "no warp view".into())
    }
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        crate::flow_warp::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if !self.started {
            self.started = true;
            self.start_load();
            self.next_frame = cx.new_next_frame();
        }
        if self.next_frame.is_event(event).is_some() {
            // Adopt a finished load.
            if let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.rx = None;
                match msg {
                    Ok(Some(data)) => {
                        log!(
                            "flow_warp_lab: clip ready — {} pairs, {}x{}, stride {}, {:.2} pairs/s",
                            data.pairs,
                            data.width,
                            data.height,
                            data.stride,
                            data.pairs_per_sec
                        );
                        self.with_warp(cx, |cx, view| {
                            view.set_clip(cx, data);
                            view.set_playing(true);
                        });
                    }
                    Ok(None) => {
                        log!("flow_warp_lab: clip has no usable flow map (see stderr)");
                        self.ui.label(cx, ids!(hud)).set_text(cx, "NO FLOW MAP");
                    }
                    Err(e) => {
                        log!("flow_warp_lab: prepare failed: {e}");
                        self.ui.label(cx, ids!(hud)).set_text(cx, &format!("FAILED: {e}"));
                    }
                }
            }
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            let dt = (time - last).clamp(0.0, 0.25);
            self.frame_dts.push(dt.max(1e-6));
            if self.frame_dts.len() > 120 {
                self.frame_dts.remove(0);
            }
            let has_clip = self
                .with_warp(cx, |cx, view| {
                    view.advance(cx, dt);
                    view.has_clip()
                })
                .unwrap_or(false);
            if has_clip {
                let line = self.hud_line(cx);
                self.ui.label(cx, ids!(hud)).set_text(cx, &line);
            }
            self.next_frame = cx.new_next_frame();
        }
        // Remote-scriptable transport (the bridge's /t goes through the IME
        // path): rate<f>, pos<f>, b, p, i.
        if let Event::TextInput(te) = event {
            let cmd = te.input.trim().to_string();
            if let Some(v) = cmd.strip_prefix("rate").and_then(|v| v.parse::<f64>().ok()) {
                self.with_warp(cx, |_cx, view| view.set_rate(v));
                log!("flow_warp_lab: rate {v}");
            } else if let Some(v) = cmd.strip_prefix("pos").and_then(|v| v.parse::<f64>().ok())
            {
                self.with_warp(cx, |cx, view| {
                    view.set_playing(false);
                    view.set_position_pairs(cx, v);
                });
                log!("flow_warp_lab: parked at {v}");
            } else if cmd == "b" {
                self.with_warp(cx, |_cx, view| {
                    let on = !view.bounce();
                    view.set_bounce(on);
                    on
                })
                .map(|on| log!("flow_warp_lab: bounce {on}"));
            } else if cmd == "p" {
                self.with_warp(cx, |_cx, view| {
                    let on = !view.is_playing();
                    view.set_playing(on);
                    on
                })
                .map(|on| log!("flow_warp_lab: playing {on}"));
            } else if cmd == "x" {
                self.with_warp(cx, |_cx, view| {
                    view.debug_show_frame = !view.debug_show_frame;
                    view.debug_show_frame
                })
                .map(|on| log!("flow_warp_lab: debug_show_frame {on}"));
            } else if cmd == "i" {
                let line = self.hud_line(cx);
                log!("flow_warp_lab: {line}");
            }
        }
    }
}
