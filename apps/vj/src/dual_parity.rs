//! DUAL-DECK WARP PARITY — step 7 of the transport migration (design-v2
//! §8): identical inputs ⇒ identical per-frame warp readback, single- vs
//! dual-deck.
//!
//! DEBUG RIG — working tree only, never committed.
//!
//! Phase SINGLE drives deck A's tween view alone over an exact
//! (pair, t) grid of synthetic NV12 frames (a square marching +8 px per
//! pair — the selftest's motion) and records a hash of every warp
//! readback. Phase DUAL drives BOTH decks over the same grid and asserts
//! (a) deck A's readbacks are BYTE-IDENTICAL to its single-deck run — a
//! neighbour deck must not perturb a deck's picture — and (b) A and B,
//! fed identical inputs, produce byte-identical warps. Prints a summary
//! and exits.
//!
//! ```text
//! VJ_DUAL_PARITY=1 MAKEPAD_HIDE_WINDOWS=1 VJ_ASSET_EMBED=always \
//!   VJ_AUTO_CUE=none ./target/release/makepad-vj --remote
//! ```

use makepad_widgets::*;

use crate::{App, SlotId};

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VJ_DUAL_PARITY").map(|v| v == "1").unwrap_or(false))
}

const W: usize = 320;
const H: usize = 180;
const SQ: usize = 48;
const PAIRS: usize = 6;
const TS: usize = 8;
/// Frames to let a state settle before the readback (the texture holds
/// the PREVIOUS frame's warp; a pair change re-derives the whole stack
/// and the two phases may skew by a frame).
const SETTLE: usize = 4;

fn frame(pair: usize) -> Vec<u8> {
    crate::selftest_nv12(W, H, 24 + 8 * pair, 40, SQ)
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Default)]
struct ParityState {
    phase: usize, // 0 = single, 1 = dual, 2 = done
    step: usize,  // grid index * SETTLE + settle counter
    /// hash per grid point from the single run.
    single: Vec<u64>,
    a_mismatch: usize,
    ab_mismatch: usize,
    worst_bytes: usize,
    points: usize,
}

thread_local! {
    static STATE: std::cell::RefCell<ParityState> = std::cell::RefCell::new(ParityState::default());
}

impl App {
    pub(crate) fn pump_dual_parity(&mut self, cx: &mut Cx) {
        let (phase, step) = STATE.with(|s| {
            let s = s.borrow();
            (s.phase, s.step)
        });
        if phase == 2 {
            return;
        }
        let grid = step / SETTLE;
        let settle = step % SETTLE;
        let total = PAIRS * TS;
        if grid >= total {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.phase += 1;
                s.step = 0;
                if s.phase == 2 {
                    println!(
                        "dualparity: {} points; A dual-vs-single mismatches {}; A-vs-B mismatches {}; worst differing bytes {}",
                        s.points, s.a_mismatch, s.ab_mismatch, s.worst_bytes
                    );
                    println!(
                        "dualparity: {}",
                        if s.a_mismatch == 0 && s.ab_mismatch == 0 { "PASS" } else { "FAIL" }
                    );
                    std::process::exit(if s.a_mismatch == 0 && s.ab_mismatch == 0 { 0 } else { 1 });
                }
            });
            self.video_pump = cx.new_next_frame();
            return;
        }
        let pair = grid / TS;
        let t = (grid % TS) as f32 / TS as f32;
        let decks: &[SlotId] = if phase == 0 { &[SlotId::A] } else { &[SlotId::A, SlotId::B] };
        if settle == 0 {
            // Drive the grid point.
            let (fa, fb) = (frame(pair), frame(pair + 1));
            for slot in decks {
                self.tween_view(cx, *slot, |cx, view| {
                    view.set_ai_tier(cx, crate::flow_tween::AiTier::Off);
                    view.set_fade(cx, false);
                    view.set_safe(cx, false);
                    view.set_cut(cx, false);
                    if grid % TS == 0 {
                        view.reset_seed();
                        view.set_pair(cx, &fa, &fb, W as u32, H as u32);
                    }
                    view.set_t(cx, t);
                });
            }
        }
        if settle == SETTLE - 1 {
            // Read back (the warp for THIS grid point is on screen now).
            let mut hashes = [0u64; 2];
            let mut lens = [0usize; 2];
            let mut bufs: [Option<Vec<u8>>; 2] = [None, None];
            for (k, slot) in decks.iter().enumerate() {
                let tex = self
                    .tween_view(cx, *slot, |_cx, view| view.output_texture())
                    .flatten();
                if let Some(tex) = tex {
                    if let Some((_, _, bytes)) = cx.debug_read_render_texture(&tex) {
                        hashes[k] = fnv(&bytes);
                        lens[k] = bytes.len();
                        bufs[k] = Some(bytes);
                    }
                }
            }
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                if phase == 0 {
                    s.single.push(hashes[0]);
                } else if grid == 0 {
                    // Warm-up: deck B's first-ever readback predates its
                    // first draw; nothing to compare.
                } else {
                    s.points += 1;
                    if s.single.get(grid).copied() != Some(hashes[0]) {
                        s.a_mismatch += 1;
                        eprintln!("dualparity: A diverged from its single run at pair {pair} t {t:.3}");
                    }
                    if hashes[0] != hashes[1] {
                        s.ab_mismatch += 1;
                        let diff = match (&bufs[0], &bufs[1]) {
                            (Some(a), Some(b)) if a.len() == b.len() => {
                                a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
                            }
                            _ => lens[0].max(lens[1]),
                        };
                        s.worst_bytes = s.worst_bytes.max(diff);
                        eprintln!(
                            "dualparity: A != B at pair {pair} t {t:.3} ({diff} differing bytes)"
                        );
                    }
                }
            });
        }
        STATE.with(|s| s.borrow_mut().step += 1);
        self.video_pump = cx.new_next_frame();
    }
}
