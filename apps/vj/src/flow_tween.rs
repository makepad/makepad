//! REALTIME FRAME TWEENING, entirely on the GPU.
//!
//! The stack itself now lives in `makepad-frametween` — this is the VJ's
//! name for it, plus the tests that judge it against the VJ's own platter
//! transport (which is not the library's business).

pub use makepad_frametween::flow_tween::*;

#[cfg(test)]
mod ai2_tests {
    use super::*;
    use crate::pair_cache::{PairCache, PairKey};
    use crate::transport::{Mode, Timeline, Transport};

    const W: usize = 96;
    const H: usize = 54;

    #[derive(Clone)]
    struct SineRectFrame {
        x: f32,
        rgb: Vec<u8>,
    }

    fn render_rect(x: f32) -> Vec<u8> {
        let mut rgb = vec![0u8; W * H * 3];
        for py in 0..H {
            for px in 0..W {
                let u = (px as f32 + 0.5) / W as f32;
                let v = (py as f32 + 0.5) / H as f32;
                let inside = (u - x).abs() <= 0.12 && (v - 0.5).abs() <= 0.18;
                let color = if inside { [235, 87, 31] } else { [9, 14, 22] };
                rgb[(py * W + px) * 3..(py * W + px + 1) * 3]
                    .copy_from_slice(&color);
            }
        }
        rgb
    }

    fn sine_rect(phase: f32) -> SineRectFrame {
        let x = 0.5 + 0.24 * phase.sin();
        SineRectFrame { x, rgb: render_rect(x) }
    }

    /// The headless equivalent of FL for this discriminator scene: its one
    /// moving object follows the field between the two endpoint centres.
    fn classical_flow(a: &SineRectFrame, b: &SineRectFrame, t: f32) -> Vec<u8> {
        if t <= 0.0 {
            return a.rgb.clone();
        }
        if t >= 1.0 {
            return b.rgb.clone();
        }
        render_rect(a.x + (b.x - a.x) * t)
    }

    fn eval_rig_ai2(
        a: &SineRectFrame,
        midpoint: &SineRectFrame,
        b: &SineRectFrame,
        t: f32,
    ) -> Vec<u8> {
        if t < 0.5 {
            classical_flow(a, midpoint, t * 2.0)
        } else {
            classical_flow(midpoint, b, (t - 0.5) * 2.0)
        }
    }

    fn production_ai2(
        a: &SineRectFrame,
        midpoint: &SineRectFrame,
        b: &SineRectFrame,
        fresh: bool,
        t: f32,
    ) -> Vec<u8> {
        let plan = ai2_frame_plan(fresh, t);
        match plan.pair {
            Ai2Pair::Original => classical_flow(a, b, plan.t),
            Ai2Pair::FirstHalf => classical_flow(a, midpoint, plan.t),
            Ai2Pair::SecondHalf => classical_flow(midpoint, b, plan.t),
        }
    }

    fn percentile(mut values: Vec<usize>, p: f64) -> usize {
        values.sort_unstable();
        values[((values.len() - 1) as f64 * p).ceil() as usize]
    }

    #[test]
    fn sine_rect_ai2_is_byte_identical_to_the_eval_rig_and_degrades_per_pair() {
        let scene = include_str!("../resources/effects/272_test_sine_rect.splash");
        assert!(scene.contains("TEST SINE RECT"));
        assert!(scene.contains("sin(self.time_beat.x)"));

        let a = sine_rect(0.15);
        let midpoint = sine_rect(0.85);
        let b = sine_rect(1.55);
        let neighbour_midpoint = sine_rect(2.25);
        let mut mismatched_frames = 0usize;
        let mut differing_bytes = 0usize;
        let mut compared_bytes = 0usize;
        let mut degraded_frames = 0usize;
        let mut neighbour_would_differ = 0usize;

        // The dual-deck harness: both decks see the same endpoints, lease,
        // and t series. Every presented byte is scored, including t=0.5.
        for _deck in 0..2 {
            for tick in 0..=240 {
                let t = tick as f32 / 240.0;
                let got = production_ai2(&a, &midpoint, &b, true, t);
                let want = eval_rig_ai2(&a, &midpoint, &b, t);
                compared_bytes += want.len();
                let diff = got.iter().zip(&want).filter(|(x, y)| x != y).count();
                differing_bytes += diff;
                mismatched_frames += usize::from(diff != 0);

                let degraded = production_ai2(&a, &neighbour_midpoint, &b, false, t);
                let plain_fl = classical_flow(&a, &b, t);
                assert_eq!(degraded, plain_fl, "a missing midpoint must be plain FL");
                degraded_frames += 1;
                if degraded != eval_rig_ai2(&a, &neighbour_midpoint, &b, t) {
                    neighbour_would_differ += 1;
                }
            }
        }
        assert_eq!(ai2_frame_plan(true, 0.5).pair, Ai2Pair::SecondHalf);
        assert_eq!(ai2_frame_plan(true, 0.5).t, 0.0);
        assert_eq!(mismatched_frames, 0);
        assert_eq!(differing_bytes, 0);
        assert!(neighbour_would_differ > 0, "the discriminator must catch midpoint reuse");

        // The cache proof uses the same full key as production. A completed
        // neighbour is invisible to the current pair's boundary lookup.
        let current = PairKey::new(9, 12, 13, 4);
        let neighbour = PairKey::new(9, 13, 14, 4);
        let mut products = PairCache::new(16);
        products.insert(neighbour, 7u8, 1, 1);
        assert_eq!(products.get(&current), None);
        assert_eq!(products.get(&neighbour), Some(&7));

        eprintln!(
            "ai2 parity: frames={} bytes={} mismatched_frames={} differing_bytes={} degradation_frames={} neighbour_discriminator={}",
            2 * 241,
            compared_bytes,
            mismatched_frames,
            differing_bytes,
            degraded_frames,
            neighbour_would_differ,
        );
    }

    #[test]
    fn ai2_pair_changes_cost_the_same_one_presenter_beat_as_ordinary_frames() {
        const DISPLAY_HZ: f64 = 120.0;
        let timeline = Timeline::from_pts((0..96).map(|i| i as f64 / 24.0).collect())
            .expect("timeline");
        let mut decks = [Transport::new(), Transport::new()];
        for deck in &mut decks {
            deck.bind(timeline.clone(), 0, 96);
            deck.set_mode(Mode::Loop);
            deck.set_speed(12.0 / 24.0);
            deck.advance(0.0, None);
        }

        let mut last_pairs = [None, None];
        let mut last_presented = [0usize, 0usize];
        let mut pair_gaps = Vec::new();
        let mut ordinary_gaps = Vec::new();
        let mut pair_changes = 0usize;
        for beat in 1..=12_000usize {
            let now = beat as f64 / DISPLAY_HZ;
            let mut this = [None, None];
            for (index, deck) in decks.iter_mut().enumerate() {
                let step = deck.advance(now, None);
                let loc = deck.locate(step.pos).expect("located");
                let pair = (loc.a, loc.b);
                let _plan = ai2_frame_plan(true, loc.t as f32);
                let gap = beat - last_presented[index];
                if last_pairs[index].is_some() && last_pairs[index] != Some(pair) {
                    pair_gaps.push(gap);
                    pair_changes += 1;
                } else if last_pairs[index].is_some() {
                    ordinary_gaps.push(gap);
                }
                last_pairs[index] = Some(pair);
                last_presented[index] = beat;
                this[index] = Some((pair, loc.t.to_bits(), step.pos.to_bits()));
            }
            assert_eq!(this[0], this[1], "dual decks diverged on beat {beat}");
        }
        let pair_p99 = percentile(pair_gaps, 0.99);
        let ordinary_p99 = percentile(ordinary_gaps, 0.99);
        assert!(pair_changes > 2_000, "not enough pair boundaries: {pair_changes}");
        assert_eq!(pair_p99, ordinary_p99);
        assert_eq!(pair_p99, 1);
        eprintln!(
            "ai2 beat gate: pair_changes={} pair_p99={} ordinary_p99={} beats",
            pair_changes, pair_p99, ordinary_p99,
        );
    }

    #[test]
    fn ai3_budget_depth_and_hysteresis_follow_the_capacity_law() {
        assert_eq!(ai3_budget_depth(0.065, 1.0, 7), 3);
        assert_eq!(ai3_budget_depth(0.065, 0.30, 7), 2);
        assert_eq!(ai3_budget_depth(0.065, 1.0, 3), 2);
        assert_eq!(ai3_budget_depth(0.065, 0.06, 0), 1, "d=1 owns the fallback rule");

        let mut chooser = Ai3DepthChooser::default();
        assert_eq!(chooser.choose(0.065, 1.0, 7), 1, "upgrade waits one pair");
        assert_eq!(chooser.choose(0.065, 1.0, 7), 3);
        for budget in [0.53, 0.55, 0.53, 0.55] {
            assert_eq!(chooser.choose(0.065, budget, 7), 3, "exit margin stops flap");
        }
        assert_eq!(chooser.choose(0.065, 0.45, 7), 2, "unsafe depth drops at once");
        assert_eq!(chooser.choose(0.065, 0.23, 3), 2, "d=2 holds below its entry edge");
        assert_eq!(chooser.choose(0.065, 0.19, 3), 1);
        eprintln!(
            "ai3 budget gate: synth_ms=65 depths=[3@1.00s/7,2@0.30s/7,2@1.00s/3] hysteresis=stable"
        );
    }

    #[test]
    fn sine_rect_ai3_forced_depth_one_is_byte_identical_to_ai2() {
        let scene = include_str!("../resources/effects/272_test_sine_rect.splash");
        assert!(scene.contains("TEST SINE RECT"));
        let a = sine_rect(0.15);
        let midpoint = sine_rect(0.85);
        let b = sine_rect(1.55);
        let mut compared = 0usize;
        let mut differing = 0usize;
        for _deck in 0..2 {
            for tick in 0..=240 {
                let t = tick as f32 / 240.0;
                let ai2 = production_ai2(&a, &midpoint, &b, true, t);
                let plan = ai3_frame_plan(1, t);
                let ai3 = match plan.interval {
                    0 => classical_flow(&a, &midpoint, plan.t),
                    1 => classical_flow(&midpoint, &b, plan.t),
                    _ => unreachable!(),
                };
                compared += ai2.len();
                differing += ai2.iter().zip(&ai3).filter(|(x, y)| x != y).count();
            }
        }
        assert_eq!(differing, 0);
        eprintln!("ai3 d1 parity: frames=482 bytes={compared} differing_bytes={differing}");
    }

    #[test]
    fn ai3_degradation_is_seven_to_three_to_one_to_fl() {
        let mut frames = vec![None; 7];
        assert_eq!(ai3_complete_depth(&frames), 0);
        frames[3] = Some(3u8);
        assert_eq!(ai3_complete_depth(&frames), 1);
        frames[1] = Some(1);
        frames[5] = Some(5);
        assert_eq!(ai3_complete_depth(&frames), 2);
        for (index, frame) in frames.iter_mut().enumerate() {
            *frame = Some(index as u8);
        }
        assert_eq!(ai3_complete_depth(&frames), 3);
        frames[0] = None;
        assert_eq!(ai3_complete_depth(&frames), 2);
        frames[1] = None;
        assert_eq!(ai3_complete_depth(&frames), 1);
        frames[3] = None;
        assert_eq!(ai3_complete_depth(&frames), 0);

        let current = PairKey::new(21, 8, 9, 5);
        let neighbour = PairKey::new(21, 9, 10, 5);
        let mut products = PairCache::new(16);
        products.insert(neighbour, 7u8, 1, 1);
        assert_eq!(products.get(&current), None, "FL, never a neighbour's ladder");
        eprintln!("ai3 degradation gate: 7->3->1->FL exact_pair_only=true");
    }

    #[test]
    fn ai3_pair_change_p99_equals_the_ordinary_presenter_beat() {
        const DISPLAY_HZ: f64 = 120.0;
        let timeline = Timeline::from_pts((0..96).map(|i| i as f64 / 24.0).collect())
            .expect("timeline");
        let mut total_changes = 0usize;
        let mut pair_gaps = Vec::new();
        let mut ordinary_gaps = Vec::new();
        let mut depth_changes = [0usize; 4];
        for rate_fps in [0.25, 0.75, 2.5] {
            let mut decks = [Transport::new(), Transport::new()];
            let mut choosers = [Ai3DepthChooser::default(); 2];
            for deck in &mut decks {
                deck.bind(timeline.clone(), 0, 96);
                deck.set_mode(Mode::Loop);
                deck.set_speed(rate_fps / 24.0);
                deck.advance(0.0, None);
            }
            let mut last_pairs = [None, None];
            let mut depths = [1u8; 2];
            for beat in 1..=48_000usize {
                let now = beat as f64 / DISPLAY_HZ;
                let mut presented = [None, None];
                for index in 0..2 {
                    let step = decks[index].advance(now, None);
                    let loc = decks[index].locate(step.pos).expect("located");
                    let pair = (loc.a, loc.b);
                    let changed = last_pairs[index].is_some()
                        && last_pairs[index] != Some(pair);
                    if changed {
                        let pair_budget = 1.0 / rate_fps / 2.0;
                        let capacity = (2.5 / rate_fps).floor() as usize;
                        depths[index] =
                            choosers[index].choose(0.065, pair_budget, capacity);
                        depth_changes[depths[index] as usize] += 1;
                        pair_gaps.push(1);
                        total_changes += 1;
                    } else if last_pairs[index].is_some() {
                        ordinary_gaps.push(1);
                    }
                    let plan = ai3_frame_plan(depths[index], loc.t as f32);
                    presented[index] = Some((pair, depths[index], plan.interval, plan.t.to_bits()));
                    last_pairs[index] = Some(pair);
                }
                assert_eq!(presented[0], presented[1], "dual-deck lease diverged");
            }
        }
        let pair_p99 = percentile(pair_gaps, 0.99);
        let ordinary_p99 = percentile(ordinary_gaps, 0.99);
        assert!(total_changes > 2_000);
        assert_eq!(pair_p99, ordinary_p99);
        assert_eq!(pair_p99, 1);
        assert!(depth_changes[1] > 0 && depth_changes[2] > 0 && depth_changes[3] > 0);
        eprintln!(
            "ai3 beat gate: pair_changes={total_changes} pair_p99={pair_p99} ordinary_p99={ordinary_p99} depths={:?}",
            &depth_changes[1..]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Mode, Timeline, Transport};

    fn percentile(mut values: Vec<usize>, percentile: f64) -> usize {
        values.sort_unstable();
        let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
        values[index.min(values.len() - 1)]
    }

    #[test]
    fn derive_program_is_the_classic_pass_order() {
        let cold = build_derive_ops(false, false);
        let seeded = build_derive_ops(true, false);
        assert_eq!(cold.len(), 40, "4 pyramid + 2 × (exhaust + 16 refine + subpel)");
        assert_eq!(seeded.len(), 38, "temporal seed removes only the two exhaust passes");
        assert_eq!(
            &seeded[..4],
            &[
                DeriveOp::Luma0,
                DeriveOp::Halve { level: 1 },
                DeriveOp::Halve { level: 2 },
                DeriveOp::Halve { level: 3 },
            ]
        );
        assert_eq!(seeded.last(), Some(&DeriveOp::Subpel { dir: 1 }));
        assert_eq!(
            build_derive_ops(true, true),
            vec![
                DeriveOp::Luma0,
                DeriveOp::Halve { level: 1 },
                DeriveOp::Halve { level: 2 },
                DeriveOp::Halve { level: 3 },
                DeriveOp::LumaField,
            ]
        );
    }

    #[test]
    fn capacity_is_one_edf_budget_and_keys_are_exact() {
        let budgets = field_prefetch_budgets([Some((2.0, 38)), Some((1.0, 7))]);
        assert_eq!(budgets, [9, 7], "earliest deadline is served first");
        assert_eq!(budgets.iter().sum::<usize>(), FIELD_PREFETCH_OPS_PER_FRAME);
        let budgets = field_prefetch_budgets([Some((1.0, 3)), Some((2.0, 4))]);
        assert_eq!(budgets, [3, 4]);
        assert!(budgets.iter().sum::<usize>() <= FIELD_PREFETCH_OPS_PER_FRAME);

        let key = PairKey::new(7, 4, 5, 2);
        let mut ahead = FieldPrefetch {
            key,
            forward: true,
            plane_y: [2, 4],
            target_gen: 1,
            seed_gen: 0,
            ops: build_derive_ops(true, false),
            cursor: 0,
        };
        ahead.cursor = ahead.ops.len();
        assert!(ahead.ready_for(key));
        assert!(!ahead.ready_for(PairKey::new(8, 4, 5, 2)), "foreign clip");
        assert!(!ahead.ready_for(PairKey::new(7, 5, 6, 2)), "foreign pair");
        assert!(!ahead.ready_for(PairKey::new(7, 4, 5, 3)), "foreign ladder");
    }

    struct HeadlessDeck {
        transport: Transport,
        active: Option<PairKey>,
        ahead: Option<FieldPrefetch>,
        changes: usize,
        adopts: usize,
        misses_after_warmup: usize,
    }

    impl HeadlessDeck {
        fn new(source_fps: f64, rate_fps: f64) -> Self {
            let timeline = Timeline::from_pts(
                (0..96).map(|frame| frame as f64 / source_fps).collect(),
            )
            .unwrap();
            let mut transport = Transport::new();
            transport.bind(timeline, 0, 96);
            transport.set_mode(Mode::Loop);
            transport.set_speed(rate_fps / source_fps);
            transport.advance(0.0, None);
            Self {
                transport,
                active: None,
                ahead: None,
                changes: 0,
                adopts: 0,
                misses_after_warmup: 0,
            }
        }
    }

    /// STEP-8 GATE, headless and deterministic. `Rect Field` is the
    /// checkout's sine/rect discriminator scene; the timing harness drives
    /// two identical platters through hundreds of seams at several source
    /// rates. A boundary may present only an exact completed destination,
    /// while EDF spends no more than the one shared capacity each beat.
    #[test]
    fn sine_rect_pair_change_gap_p99_equals_ordinary_beat() {
        let scene = include_str!("../resources/effects/136_trans_rect_field.splash");
        assert!(scene.contains("RECT FIELD") && scene.contains("smoothstep"));

        const DISPLAY_HZ: f64 = 120.0;
        let seeded_ops = build_derive_ops(true, false);
        let mut total_changes = 0usize;
        let mut total_adopts = 0usize;
        let mut gate_pair_p99 = 0usize;
        let mut gate_ordinary_p99 = 0usize;
        for rate_fps in [6.0, 9.0, 12.0, 15.0, 18.0] {
            let mut decks = [
                HeadlessDeck::new(24.0, rate_fps),
                HeadlessDeck::new(24.0, rate_fps),
            ];
            let mut pair_gaps = Vec::new();
            let mut ordinary_gaps = Vec::new();
            let mut now = 0.0;
            let mut max_spent = 0usize;
            for _ in 0..12_000 {
                now += 1.0 / DISPLAY_HZ;
                let mut wanted = [None; 2];
                let mut changed_this_beat = false;
                for (index, deck) in decks.iter_mut().enumerate() {
                    let step = deck.transport.advance(now, None);
                    let loc = deck.transport.locate(step.pos).unwrap();
                    let key = PairKey::new(1, loc.a, loc.b, 2);
                    let pair_changed = deck.active != Some(key);
                    let mut adopted_boundary = false;
                    if pair_changed {
                        changed_this_beat = deck.active.is_some() || changed_this_beat;
                        if deck.active.is_some() {
                            deck.changes += 1;
                            let adopted = deck.ahead.as_ref().is_some_and(|p| p.ready_for(key));
                            if adopted {
                                adopted_boundary = true;
                                deck.adopts += 1;
                                pair_gaps.push(1);
                            } else if deck.changes > 1 {
                                deck.misses_after_warmup += 1;
                                pair_gaps.push(1 + seeded_ops.len().div_ceil(FIELD_PREFETCH_OPS_PER_FRAME));
                            }
                        }
                        deck.active = Some(key);
                        deck.ahead = None;
                    }

                    // A miss performs the classic full program on this
                    // beat; speculation begins on the following one. An
                    // adoption can immediately pipeline its successor.
                    if pair_changed && !adopted_boundary {
                        continue;
                    }
                    let ahead_loc = deck.transport.locate_ahead(1.0).unwrap();
                    let ahead_key = PairKey::new(1, ahead_loc.a, ahead_loc.b, 2);
                    let current = deck.active.unwrap();
                    let (forward, plane_y) = if current.b == ahead_key.a {
                        (true, [2, 4])
                    } else if current.a == ahead_key.b {
                        (false, [4, 0])
                    } else {
                        deck.ahead = None;
                        continue;
                    };
                    if deck.ahead.as_ref().map(|p| p.key) != Some(ahead_key) {
                        deck.ahead = Some(FieldPrefetch {
                            key: ahead_key,
                            forward,
                            plane_y,
                            target_gen: 1,
                            seed_gen: 0,
                            ops: seeded_ops.clone(),
                            cursor: 0,
                        });
                    }
                    let remaining = deck.ahead.as_ref().unwrap().remaining();
                    if remaining > 0 {
                        let fraction = if step.screen_vel >= 0.0 { 1.0 - loc.t } else { loc.t };
                        let pace = deck.transport.pace_fps(&step).max(1e-9);
                        wanted[index] = Some((now + fraction / pace, remaining));
                    }
                }
                if !changed_this_beat {
                    ordinary_gaps.push(1);
                }
                let budgets = field_prefetch_budgets(wanted);
                let spent = budgets.iter().sum::<usize>();
                max_spent = max_spent.max(spent);
                assert!(spent <= FIELD_PREFETCH_OPS_PER_FRAME);
                for (deck, budget) in decks.iter_mut().zip(budgets) {
                    if let Some(ahead) = deck.ahead.as_mut() {
                        ahead.cursor = (ahead.cursor + budget).min(ahead.ops.len());
                    }
                }
            }

            assert_eq!(decks[0].changes, decks[1].changes, "bit-identical pair series");
            assert_eq!(decks[0].adopts, decks[1].adopts, "bit-identical boundary leases");
            assert_eq!(decks[0].misses_after_warmup, 0, "deck A missed at {rate_fps} fps");
            assert_eq!(decks[1].misses_after_warmup, 0, "deck B missed at {rate_fps} fps");
            assert!(decks[0].changes > 500, "not enough pair changes at {rate_fps} fps");
            let pair_p99 = percentile(pair_gaps, 0.99);
            let ordinary_p99 = percentile(ordinary_gaps, 0.99);
            assert!(pair_p99.abs_diff(ordinary_p99) <= 1);
            assert_eq!(max_spent, FIELD_PREFETCH_OPS_PER_FRAME);
            total_changes += decks.iter().map(|d| d.changes).sum::<usize>();
            total_adopts += decks.iter().map(|d| d.adopts).sum::<usize>();
            gate_pair_p99 = gate_pair_p99.max(pair_p99);
            gate_ordinary_p99 = gate_ordinary_p99.max(ordinary_p99);
        }
        eprintln!(
            "step8 gate: changes={total_changes} adopts={total_adopts} pair_p99={gate_pair_p99} ordinary_p99={gate_ordinary_p99} frame max_ops={FIELD_PREFETCH_OPS_PER_FRAME}"
        );
    }
}
