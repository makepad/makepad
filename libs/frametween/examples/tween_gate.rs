//! THE GATE — every mode, on the real GPU, judged from pixels.
//!
//! Feeds one synthetic pair (a marker block riding a textured field, 96 px
//! of motion between the two endpoints) through each tier at t = 0.5, reads
//! the warp target back, and says where the marker landed:
//!
//! ```text
//! None       marker at frame B, full strength      (a hard swap)
//! Crossfade  marker in BOTH places, half strength  (a blend, nothing moved)
//! Flow       ONE marker, full strength, halfway    (features moved)
//! AI1/2/3    the same, from the neural producer
//! ```
//!
//! Run it through the GPU guard, never bare:
//!
//! ```text
//! local/tools/gpu-guard -t 300 -n tweengate -- \
//!   cargo run -p makepad-frametween --example tween_gate --release
//! ```
//!
//! Exit status is the verdict: 0 = every tier behaved, 1 = one did not.
//! `FRAMETWEEN_GATE_PNG=<dir>` also dumps what each tier drew.

pub use makepad_widgets;

use makepad_frametween::selftest::{
    gate_pair, read_block_bgra, BlockReading, BLOCK_B_X, BLOCK_MID_X, GATE_H, GATE_W,
};
use makepad_frametween::{
    ai2_frame_plan, default_model_path, rife_proxy_dims, Ai2Pair, FlowTweenView, Mode, RifeJob,
    RifeProduct, RifeProductKind, RifeService, RifeSource,
};
use makepad_widgets::*;
use std::sync::Arc;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(320, 200)
                body +: {
                    tween := FlowTweenView{}
                }
            }
        }
    }
}

/// How long a stage may wait before the gate calls it failed.
const SETTLE_FRAMES: u32 = 10;
const NEURAL_FRAMES: u32 = 900;

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    /// Upload the pair and let the flow stack derive.
    Feed,
    /// Ask the neural producer for this tier's product and wait for it.
    Neural(u32),
    /// Let the warp draw with everything in place.
    Settle(u32),
    /// Read the target back and judge.
    Read,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    pump: NextFrame,
    #[rust(0usize)]
    which: usize,
    #[rust(Stage::Feed)]
    stage: Stage,
    #[rust]
    rife: Option<RifeService>,
    #[rust]
    rife_broken: bool,
    #[rust]
    offered: bool,
    #[rust]
    failures: usize,
    #[rust]
    reported: usize,
}

impl App {
    fn view<R>(
        &self,
        cx: &mut Cx,
        f: impl FnOnce(&mut Cx, &mut FlowTweenView) -> R,
    ) -> Option<R> {
        let widget = self.ui.widget(cx, ids!(tween));
        let mut view = widget.borrow_mut::<FlowTweenView>()?;
        Some(f(cx, &mut view))
    }

    /// The tiers to run: all of them, minus the neural ones when there is
    /// no checkpoint to run them with (said out loud, never skipped
    /// silently).
    fn plan(&self) -> Vec<Mode> {
        let neural = default_model_path().exists() && !self.rife_broken;
        Mode::ALL
            .iter()
            .copied()
            .filter(|m| neural || !m.uses_ai())
            .collect()
    }

    /// Where this tier is supposed to put the marker, and what shape the
    /// reading should have.
    fn judge(mode: Mode, r: BlockReading) -> Result<String, String> {
        match mode {
            Mode::None => {
                let off = r.offset_from(BLOCK_B_X);
                if r.moved() && off.abs() <= 6 {
                    Ok(format!("hard swap: marker at frame B, offset {off:+}px"))
                } else {
                    Err(format!("expected frame B untouched, got {r:?}"))
                }
            }
            Mode::Crossfade => {
                if r.ghosted() {
                    Ok(format!(
                        "blend: no full-strength marker anywhere, {} px tinted across both places",
                        r.tinted_width
                    ))
                } else {
                    Err(format!("expected a dissolve (two half ghosts), got {r:?}"))
                }
            }
            _ => {
                let off = r.offset_from(BLOCK_MID_X);
                if r.moved() && off.abs() <= 14 {
                    Ok(format!(
                        "MOVED: one full-strength marker {} px wide, {off:+}px off the midpoint",
                        r.full_width
                    ))
                } else if r.ghosted() {
                    Err(format!("ghosted like a crossfade — features did not move: {r:?}"))
                } else {
                    Err(format!("marker is {off:+}px off the midpoint: {r:?}"))
                }
            }
        }
    }

    fn feed(&mut self, cx: &mut Cx, mode: Mode) {
        let (a, b) = gate_pair();
        let (w, h) = (GATE_W as u32, GATE_H as u32);
        self.view(cx, |cx, view| {
            view.clear(cx);
            view.set_pair_rgb8(cx, &a, &b, w, h);
            view.set_fade(cx, matches!(mode, Mode::None | Mode::Crossfade));
            // NONE holds the newest picture: t = 1 samples frame B exactly
            // through every producer's math.
            view.set_t(cx, if mode == Mode::None { 1.0 } else { 0.5 });
            view.redraw(cx);
        });
    }

    /// Offer this tier's neural job and adopt the product when it lands.
    fn neural(&mut self, cx: &mut Cx, mode: Mode) -> bool {
        if self.rife.is_none() {
            match RifeService::start(&default_model_path()) {
                Ok(service) => self.rife = Some(service),
                Err(error) => {
                    log!("gate: neural producer unavailable: {error}");
                    self.rife_broken = true;
                    return true;
                }
            }
        }
        let (a, b) = gate_pair();
        let (pw, ph) = rife_proxy_dims(GATE_W as u32, GATE_H as u32);
        let rife = self.rife.as_ref().unwrap();
        if !self.offered {
            self.offered = rife.offer_next(RifeJob {
                generation: 1,
                a: 0,
                b: 1,
                kind: match mode {
                    Mode::Ai2 => RifeProductKind::Midpoint,
                    Mode::Ai3 => RifeProductKind::Subdivision { depth: 1 },
                    _ => RifeProductKind::Field,
                },
                frames: RifeSource::Rgb8 {
                    a: Arc::new(a),
                    b: Arc::new(b),
                    width: GATE_W,
                    height: GATE_H,
                },
                width: pw,
                height: ph,
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(60),
            });
            return false;
        }
        let Some(product) = rife.take() else { return false };
        self.view(cx, |cx, view| match &product {
            RifeProduct::Field(field) => {
                view.set_rife_field(cx, 0, field.width, field.height, &field.flow, &field.mask);
            }
            RifeProduct::Midpoint(midpoint) => {
                view.set_ai2_midpoint(cx, midpoint);
                // At t = 0.5 a fresh midpoint hands the beat to the second
                // half-pair at its own t = 0 — which IS the neural picture.
                let plan = ai2_frame_plan(true, 0.5);
                view.select_ai2_pair(cx, plan.pair);
                view.set_t(cx, plan.t);
                debug_assert_eq!(plan.pair, Ai2Pair::SecondHalf);
            }
            RifeProduct::Subdivision(subdivision) => {
                view.set_ai3_subdivision(cx, subdivision, 1);
                view.select_ai3_pair(cx, 1);
                view.set_t(cx, 0.0);
            }
        });
        true
    }

    fn read(&mut self, cx: &mut Cx, mode: Mode) {
        let tex = self.view(cx, |_cx, view| view.output_texture()).flatten();
        let Some(tex) = tex else {
            log!("FAIL {:<10} the warp never rendered", mode.label());
            self.failures += 1;
            return;
        };
        let Some((w, h, bgra)) = cx.debug_read_render_texture(&tex) else {
            log!("FAIL {:<10} render target readback failed", mode.label());
            self.failures += 1;
            return;
        };
        if let Some(dir) = std::env::var_os("FRAMETWEEN_GATE_PNG") {
            let mut rgba = vec![255u8; bgra.len()];
            for (o, px) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
                (o[0], o[1], o[2]) = (px[2], px[1], px[0]);
            }
            let path = std::path::Path::new(&dir)
                .join(format!("tween_gate_{}.png", mode.short().to_lowercase()));
            if let Ok(png) = encode_png_rgba(&rgba, w as u32, h as u32) {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(&path, png);
            }
        }
        let reading = read_block_bgra(&bgra, w, h);
        match Self::judge(mode, reading) {
            Ok(note) => log!("PASS {:<10} {note}", mode.label()),
            Err(note) => {
                log!("FAIL {:<10} {note}", mode.label());
                self.failures += 1;
            }
        }
        self.reported += 1;
    }

    fn step(&mut self, cx: &mut Cx) {
        let plan = self.plan();
        let Some(&mode) = plan.get(self.which) else {
            let skipped = Mode::ALL.len() - self.reported;
            if skipped > 0 {
                log!("gate: {skipped} neural tier(s) skipped — no RIFE checkpoint at {}",
                    default_model_path().display());
            }
            log!(
                "gate: {} of {} tiers behaved",
                self.reported - self.failures,
                self.reported
            );
            if self.failures == 0 {
                log!("gate: PASS");
            } else {
                log!("gate: FAIL — {} tier(s) did not behave", self.failures);
            }
            cx.quit();
            return;
        };
        match self.stage {
            Stage::Feed => {
                self.feed(cx, mode);
                self.stage = if mode.uses_ai() {
                    Stage::Neural(0)
                } else {
                    Stage::Settle(0)
                };
            }
            Stage::Neural(waited) => {
                if self.neural(cx, mode) {
                    self.stage = Stage::Settle(0);
                } else if waited >= NEURAL_FRAMES {
                    log!("FAIL {:<10} the neural producer never answered", mode.label());
                    self.failures += 1;
                    self.reported += 1;
                    self.which += 1;
                    self.offered = false;
                    self.stage = Stage::Feed;
                } else {
                    self.stage = Stage::Neural(waited + 1);
                }
            }
            Stage::Settle(waited) => {
                self.view(cx, |cx, view| view.redraw(cx));
                self.stage = if waited >= SETTLE_FRAMES {
                    Stage::Read
                } else {
                    Stage::Settle(waited + 1)
                };
            }
            Stage::Read => {
                self.read(cx, mode);
                self.which += 1;
                self.offered = false;
                self.stage = Stage::Feed;
            }
        }
        self.pump = cx.new_next_frame();
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        log!("gate: {} x {}, marker moves {} px between the endpoints", GATE_W, GATE_H,
            BLOCK_B_X - makepad_frametween::selftest::BLOCK_A_X);
        self.pump = cx.new_next_frame();
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_frametween::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if self.pump.is_event(event).is_some() {
            self.step(cx);
        }
    }
}

/// A minimal PNG writer — the gate's dumps are a debugging convenience and
/// do not deserve a dependency.
fn encode_png_rgba(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, ()> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in bytes {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut with_tag = tag.to_vec();
        with_tag.extend_from_slice(body);
        out.extend_from_slice(&with_tag);
        out.extend_from_slice(&crc32(&with_tag).to_be_bytes());
    }
    if rgba.len() < (w * h * 4) as usize {
        return Err(());
    }
    // Stored (uncompressed) deflate blocks + the adler32 the format wants.
    let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
    for y in 0..h {
        raw.push(0u8);
        let at = (y * w * 4) as usize;
        raw.extend_from_slice(&rgba[at..at + (w * 4) as usize]);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = if (i + 1) * 65535 >= raw.len() { 1u8 } else { 0 };
        z.push(last);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}
