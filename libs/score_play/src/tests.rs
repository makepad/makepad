use crate::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

struct CountingAllocator;

std::thread_local! {
    static COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
}

static TRACKED_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                TRACKED_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
        });
        // SAFETY: delegates the exact allocation request to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout originated from `System.alloc` above.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                TRACKED_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
        });
        // SAFETY: delegates the exact allocation request to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                TRACKED_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
        });
        // SAFETY: the pointer/layout pair came from this allocator; `size` is forwarded.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, Debug)]
struct Recorded {
    event: SynthEvent,
    timing: SynthEventTiming,
}

#[derive(Clone, Copy, Debug, Default)]
struct TestVoice {
    id: NoteId,
    source: EventSource,
}

struct TestSynth {
    records: [Option<Recorded>; 256],
    record_len: usize,
    voices: [TestVoice; 64],
    voice_len: usize,
}

impl Default for TestSynth {
    fn default() -> Self {
        Self {
            records: [None; 256],
            record_len: 0,
            voices: [TestVoice {
                id: 0,
                source: EventSource::Playback,
            }; 64],
            voice_len: 0,
        }
    }
}

impl TestSynth {
    fn records(&self) -> impl Iterator<Item = Recorded> + '_ {
        self.records[..self.record_len].iter().flatten().copied()
    }

    fn remove_voice(&mut self, note_id: NoteId) {
        if let Some(index) = self.voices[..self.voice_len]
            .iter()
            .position(|voice| voice.id == note_id)
        {
            self.voices.copy_within(index + 1..self.voice_len, index);
            self.voice_len -= 1;
        }
    }

    fn remove_source(&mut self, source: EventSource) {
        let mut read = 0;
        let mut write = 0;
        while read < self.voice_len {
            let voice = self.voices[read];
            if voice.source != source {
                self.voices[write] = voice;
                write += 1;
            }
            read += 1;
        }
        self.voice_len = write;
    }
}

impl SynthBackend for TestSynth {
    fn dispatch(&mut self, event: SynthEvent, timing: SynthEventTiming) {
        if self.record_len < self.records.len() {
            self.records[self.record_len] = Some(Recorded { event, timing });
            self.record_len += 1;
        }
        match event.kind {
            SynthEventKind::NoteOn { note_id, .. } => {
                if self.voice_len < self.voices.len() {
                    self.voices[self.voice_len] = TestVoice {
                        id: note_id,
                        source: event.source,
                    };
                    self.voice_len += 1;
                }
            }
            SynthEventKind::NoteOff { note_id, .. } => self.remove_voice(note_id),
            SynthEventKind::AllNotesOff { source, .. } => self.remove_source(source),
            _ => {}
        }
    }

    fn render_range(&mut self, channels: &mut [&mut [f32]], range: std::ops::Range<usize>) {
        let value = self.voice_len as f32;
        for channel in channels {
            for sample in &mut channel[range.clone()] {
                *sample = value;
            }
        }
    }

    fn voice_count(&self) -> usize {
        self.voice_len
    }
}

fn constant_tempo(sample_rate: u32, bpm: f64) -> TempoMap {
    TempoMap::constant(sample_rate, bpm).expect("test tempo")
}

fn compile_plan(sample_rate: u32, bpm: f64, notes: Vec<NoteInput>, end: f64) -> PerformancePlan {
    Scheduler::compile(
        PlanInput {
            notes,
            end_quarter: end,
            ..PlanInput::default()
        },
        constant_tempo(sample_rate, bpm),
        ScheduleOptions::default(),
    )
    .expect("test plan")
}

fn exact_note(at: f64, duration: f64, id: NoteId, key: u8) -> NoteInput {
    NoteInput {
        at_quarter: at,
        duration_quarters: duration,
        part: 0,
        note_id: id,
        key,
        dynamic: 0.7,
        articulation: Articulation::Custom {
            gate: 1.0,
            attack: 1.0,
        },
        swing_eligible: false,
    }
}

fn context(first: u64, frames: usize, sample_rate: u32) -> RenderContext {
    RenderContext {
        device_sample_rate: 0,
        first_device_sample: first,
        first_presentation_host_ns: first.saturating_mul(1_000_000_000 / u64::from(sample_rate)),
        frames,
        output_latency_frames: 0,
        stream_generation: 1,
        clock_quality: ClockQuality::Exact,
    }
}

fn push_batch<const N: usize>(ring: &SpscRing<AudioMessage, N>, batch: EventBatch) {
    for event in batch.as_slice().iter().copied() {
        assert!(ring.push(event).is_ok());
    }
}

#[test]
fn events_land_at_exact_offsets_inside_a_block() {
    let plan = compile_plan(100, 60.0, vec![exact_note(0.5, 0.25, 7, 64)], 2.0);
    assert_eq!(plan.events()[0].at, 50);
    assert_eq!(plan.events()[1].at, 75);
    let ring = SpscRing::<AudioMessage, 8>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    engine.play(&plan);
    let mut output = [0.0f32; 128];
    let mut channels: [&mut [f32]; 1] = [&mut output];
    assert_eq!(
        engine.render(context(0, 128, 100), &mut channels, &plan, &ring, &mixer, &clock),
        RenderStatus::Rendered
    );
    let note_events: Vec<_> = engine
        .backend()
        .records()
        .filter(|record| {
            matches!(
                record.event.kind,
                SynthEventKind::NoteOn { .. } | SynthEventKind::NoteOff { .. }
            )
        })
        .collect();
    assert_eq!(note_events[0].timing.block_offset, 50);
    assert_eq!(note_events[0].timing.device_sample, 50);
    assert_eq!(note_events[1].timing.block_offset, 75);
}

/// A queued Play message must leave the transport running: it enters `Playing`, keeps that
/// state to the end of the callback, advances the timeline by the frames it rendered, and
/// publishes both facts on the clock. A plan whose end lands at or before the start point
/// used to stop it again inside the same block, which reads as "Play does nothing".
#[test]
fn a_queued_play_message_starts_and_keeps_the_transport_running() {
    let plan = compile_plan(
        1_000,
        60.0,
        vec![exact_note(0.0, 0.5, 1, 60), exact_note(1.0, 0.5, 2, 67)],
        2.0,
    );
    let ring = SpscRing::<AudioMessage, 8>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    assert_eq!(engine.state(), PlaybackState::Stopped);
    assert!(ring
        .push(AudioMessage {
            at_device_sample: 32,
            sequence: 1,
            kind: AudioMessageKind::Play,
        })
        .is_ok());
    let mut output = [0.0f32; 128];
    let mut channels: [&mut [f32]; 1] = [&mut output];
    assert_eq!(
        engine.render(context(0, 128, 1_000), &mut channels, &plan, &ring, &mixer, &clock),
        RenderStatus::Rendered
    );
    assert_eq!(engine.state(), PlaybackState::Playing);
    assert_eq!(engine.timeline_sample(), 96);
    let published = clock.read();
    assert_eq!(published.state, PlaybackState::Playing);
    assert_eq!(published.presentation_sample, 96.0);
    assert_eq!(published.device_sample, 128);
    assert!(engine
        .backend()
        .records()
        .any(|record| matches!(record.event.kind, SynthEventKind::NoteOn { .. })));

    let mut channels: [&mut [f32]; 1] = [&mut output];
    assert_eq!(
        engine.render(context(128, 128, 1_000), &mut channels, &plan, &ring, &mixer, &clock),
        RenderStatus::Rendered
    );
    assert_eq!(engine.state(), PlaybackState::Playing);
    assert_eq!(engine.timeline_sample(), 224);
}

/// A plan compiled at one rate played out of a device running at another must advance in
/// PLAN samples per DEVICE frame, not one-for-one. A 2000 Hz plan on a 1000 Hz device runs
/// two plan samples per frame; getting this wrong plays the whole piece at the wrong tempo
/// (a 48 kHz plan on 44.1 kHz output ran 8.8% slow).
#[test]
fn the_timeline_advances_in_plan_samples_per_device_frame() {
    let plan = compile_plan(2_000, 60.0, vec![exact_note(0.0, 0.5, 1, 60)], 8.0);
    let ring = SpscRing::<AudioMessage, 8>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    assert!(ring
        .push(AudioMessage { at_device_sample: 0, sequence: 1, kind: AudioMessageKind::Play })
        .is_ok());

    let mut output = [0.0f32; 100];
    let mut channels: [&mut [f32]; 1] = [&mut output];
    let mut ctx = context(0, 100, 1_000);
    ctx.device_sample_rate = 1_000;
    assert_eq!(
        engine.render(ctx, &mut channels, &plan, &ring, &mixer, &clock),
        RenderStatus::Rendered
    );
    // 100 device frames at 1 kHz = 0.1 s, which is 200 samples of a 2 kHz plan.
    assert_eq!(engine.timeline_sample(), 200);
    assert_eq!(clock.read().tempo_scale, 2.0);

    // device_sample_rate 0 keeps the old one-for-one behaviour.
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    assert!(ring
        .push(AudioMessage { at_device_sample: 0, sequence: 1, kind: AudioMessageKind::Play })
        .is_ok());
    let mut channels: [&mut [f32]; 1] = [&mut output];
    assert_eq!(
        engine.render(context(0, 100, 1_000), &mut channels, &plan, &ring, &mixer, &clock),
        RenderStatus::Rendered
    );
    assert_eq!(engine.timeline_sample(), 100);
}

/// Pressing Play with the transport parked at the end restarts the piece instead of
/// stopping again on the first sample.
#[test]
fn play_at_the_end_of_the_plan_rewinds_and_runs() {
    let plan = compile_plan(1_000, 60.0, vec![exact_note(0.0, 0.5, 1, 60)], 1.0);
    let ring = SpscRing::<AudioMessage, 8>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    engine.seek(&plan, plan.end_sample());
    assert_eq!(engine.timeline_sample(), plan.end_sample());
    engine.play(&plan);
    assert_eq!(engine.timeline_sample(), 0);
    let mut output = [0.0f32; 64];
    let mut channels: [&mut [f32]; 1] = [&mut output];
    let _ = engine.render(context(0, 64, 1_000), &mut channels, &plan, &ring, &mixer, &clock);
    assert_eq!(engine.state(), PlaybackState::Playing);
    assert_eq!(engine.timeline_sample(), 64);
}

fn render_with_blocks(plan: &PerformancePlan, blocks: &[usize], total: usize) -> Vec<f32> {
    let ring = SpscRing::<AudioMessage, 8>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 32, 1>::new(TestSynth::default());
    assert!(engine.set_tempo_scale(1.37));
    engine.play(plan);
    let mut result = Vec::with_capacity(total);
    let mut device = 0u64;
    let mut block_index = 0;
    while result.len() < total {
        let frames = blocks[block_index % blocks.len()].min(total - result.len());
        let mut output = vec![0.0f32; frames];
        let mut channels: [&mut [f32]; 1] = [&mut output];
        assert_eq!(
            engine.render(
                context(device, frames, plan.tempo_map().sample_rate()),
                &mut channels,
                plan,
                &ring,
                &mixer,
                &clock,
            ),
            RenderStatus::Rendered
        );
        result.extend_from_slice(&output);
        device += frames as u64;
        block_index += 1;
    }
    result
}

#[test]
fn output_is_invariant_to_callback_block_size() {
    let plan = compile_plan(
        1_000,
        120.0,
        vec![
            exact_note(0.14, 0.31, 1, 60),
            exact_note(0.33, 0.17, 2, 67),
            exact_note(0.81, 0.2, 3, 72),
        ],
        1.4,
    );
    let fixed = render_with_blocks(&plan, &[64], 800);
    let irregular = render_with_blocks(&plan, &[1, 7, 31, 2, 127, 19], 800);
    assert_eq!(fixed, irregular);
}

#[test]
fn loop_wrap_is_exact_even_inside_one_large_callback() {
    let mut plan = compile_plan(100, 60.0, vec![exact_note(0.25, 0.5, 1, 60)], 2.0);
    plan.prepare_checkpoint(20);
    let ring = SpscRing::<AudioMessage, 4>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    engine.seek(&plan, 20);
    engine.set_loop(TransportLoop::new(20, 50));
    engine.play(&plan);
    let mut output = [0.0f32; 100];
    let mut channels: [&mut [f32]; 1] = [&mut output];
    let _ = engine.render(context(1_000, 100, 100), &mut channels, &plan, &ring, &mixer, &clock);
    let resets: Vec<_> = engine
        .backend()
        .records()
        .filter(|record| matches!(record.event.kind, SynthEventKind::TransportReset { .. }))
        .map(|record| record.timing.device_sample)
        .collect();
    assert_eq!(resets, vec![1_000, 1_030, 1_060, 1_090]);
    assert_eq!(engine.timeline_sample(), 30);
    assert_eq!(engine.loop_count(), 3);
}

#[test]
fn tempo_map_matches_hand_computed_constant_changes_and_ritardando() {
    let stepped = TempoMap::new(
        48_000,
        vec![
            TempoPoint::constant(0.0, 120.0),
            TempoPoint::constant(2.0, 60.0),
        ],
    )
    .expect("stepped map");
    assert_eq!(stepped.quarter_to_sample(1.0), 24_000);
    assert_eq!(stepped.quarter_to_sample(2.0), 48_000);
    assert_eq!(stepped.quarter_to_sample(3.0), 96_000);

    let rit = TempoMap::new(
        48_000,
        vec![TempoPoint::ramp(0.0, 120.0), TempoPoint::constant(4.0, 60.0)],
    )
    .expect("rit map");
    assert_eq!(rit.quarter_to_sample(2.0), 55_235);
    assert_eq!(rit.quarter_to_sample(4.0), 133_084);
    let inverse = rit.sample_to_quarter(rit.quarter_to_sample(3.25));
    assert!((inverse - 3.25).abs() < 0.000_05);
}

#[test]
fn scheduler_applies_swing_dynamics_articulation_hairpins_controls_and_pedal() {
    let plan = Scheduler::compile(
        PlanInput {
            notes: vec![
                NoteInput {
                    at_quarter: 0.5,
                    duration_quarters: 0.5,
                    part: 0,
                    note_id: 1,
                    key: 60,
                    dynamic: 0.5,
                    articulation: Articulation::Normal,
                    swing_eligible: true,
                },
                NoteInput {
                    at_quarter: 0.5,
                    duration_quarters: 0.5,
                    part: 0,
                    note_id: 2,
                    key: 64,
                    dynamic: 0.5,
                    articulation: Articulation::Accent,
                    swing_eligible: false,
                },
            ],
            pedals: vec![PedalInput {
                at_quarter: 0.25,
                part: 0,
                value: u16::MAX,
            }],
            controls: vec![ScoreControlInput {
                at_quarter: 0.25,
                part: 0,
                control: 74,
                value: 123,
            }],
            hairpins: vec![HairpinInput {
                at_quarter: 0.0,
                duration_quarters: 1.0,
                part: 0,
                from: 10_000,
                to: 50_000,
            }],
            end_quarter: 2.0,
        },
        constant_tempo(100, 60.0),
        ScheduleOptions {
            swing: Some(Swing {
                unit_quarters: 0.5,
                first_fraction: 2.0 / 3.0,
            }),
            ..ScheduleOptions::default()
        },
    )
    .expect("expressive plan");

    let note_ons: Vec<_> = plan
        .events()
        .iter()
        .filter_map(|event| match event.event.kind {
            SynthEventKind::NoteOn {
                note_id, velocity, ..
            } => Some((note_id, event.at, velocity)),
            _ => None,
        })
        .collect();
    assert_eq!(note_ons[0].0, 2);
    assert_eq!(note_ons[0].1, 50);
    assert_eq!(note_ons[1].0, 1);
    assert_eq!(note_ons[1].1, 67);
    assert!(note_ons[0].2 > note_ons[1].2);
    assert!(plan.events().iter().any(|event| {
        event.at == 25 && matches!(event.event.kind, SynthEventKind::Pedal { .. })
    }));
    assert!(plan.events().iter().any(|event| {
        event.at == 25 && matches!(event.event.kind, SynthEventKind::Control { control: 74, .. })
    }));
    assert!(plan.events().iter().any(|event| {
        matches!(
            event.event.kind,
            SynthEventKind::ExpressionRamp {
                from: 10_000,
                to: 50_000,
                end_sample: 100,
                ..
            }
        )
    }));
}

#[test]
fn timestamped_midi_maps_host_time_to_the_audio_clock_without_byte_assumptions() {
    let packet = TimestampedMidiEvent::new(
        7,
        3,
        MidiProtocol::Midi1Ump,
        1_250_000_000,
        TimestampQuality::Native,
        1,
        [0x2390_3c7f, 0, 0, 0],
    )
    .expect("UMP packet")
    .with_audio_anchor(1_000_000_000, 48_000, 48_000, 240);
    assert_eq!(packet.captured_sample, Some(59_760));
    assert_eq!(packet.word_count, 1);
    assert!(TimestampedMidiEvent::new(
        0,
        16,
        MidiProtocol::Midi2Ump,
        0,
        TimestampQuality::Estimated,
        2,
        [0; 4],
    )
    .is_none());
}

fn click_samples(events: &[ScheduledEvent]) -> Vec<(u64, ClickLevel)> {
    events
        .iter()
        .filter_map(|event| match event.event.kind {
            SynthEventKind::Click { level } => Some((event.at, level)),
            _ => None,
        })
        .collect()
}

#[test]
fn metronome_accents_simple_compound_irregular_and_count_in() {
    let tempo = constant_tempo(100, 60.0);
    let simple = Meter::new(4, 4, &[]).expect("4/4");
    assert_eq!(
        click_samples(&Metronome::schedule(
            &tempo,
            0,
            0.0,
            1,
            simple,
            MetronomeConfig::default(),
            0,
        )),
        vec![
            (0, ClickLevel::Bar),
            (100, ClickLevel::Beat),
            (200, ClickLevel::Beat),
            (300, ClickLevel::Beat),
        ]
    );

    let compound = Meter::new(6, 8, &[]).expect("6/8");
    assert_eq!(
        click_samples(&Metronome::schedule(
            &tempo,
            0,
            0.0,
            1,
            compound,
            MetronomeConfig::default(),
            0,
        )),
        vec![(0, ClickLevel::Bar), (150, ClickLevel::Beat)]
    );
    let subdivided = Metronome::schedule(
        &tempo,
        0,
        0.0,
        1,
        compound,
        MetronomeConfig {
            subdivisions_per_unit: 1,
        },
        0,
    );
    assert_eq!(
        click_samples(&subdivided)
            .into_iter()
            .map(|event| event.0)
            .collect::<Vec<_>>(),
        vec![0, 50, 100, 150, 200, 250]
    );

    let irregular = Meter::new(7, 8, &[2, 2, 3]).expect("7/8");
    assert_eq!(
        click_samples(&Metronome::schedule(
            &tempo,
            0,
            0.0,
            1,
            irregular,
            MetronomeConfig::default(),
            0,
        )),
        vec![
            (0, ClickLevel::Bar),
            (100, ClickLevel::Beat),
            (200, ClickLevel::Beat),
        ]
    );

    let plan = Scheduler::compile(
        PlanInput {
            notes: vec![exact_note(0.0, 1.0, 9, 60)],
            end_quarter: 1.0,
            ..PlanInput::default()
        },
        tempo,
        ScheduleOptions {
            count_in: Some(CountInSpec {
                meter: simple,
                bars: 1,
                subdivisions_per_unit: 0,
            }),
            ..ScheduleOptions::default()
        },
    )
    .expect("count-in plan");
    assert_eq!(plan.score_origin_sample(), 400);
    assert_eq!(
        plan.events()
            .iter()
            .find(|event| matches!(event.event.kind, SynthEventKind::NoteOn { .. }))
            .map(|event| event.at),
        Some(400)
    );
    assert_eq!(
        click_samples(plan.events()),
        vec![
            (0, ClickLevel::Bar),
            (100, ClickLevel::Beat),
            (200, ClickLevel::Beat),
            (300, ClickLevel::Beat),
        ]
    );
}

#[test]
fn rapid_hover_releases_prior_audition_voices_without_touching_playback() {
    let plan = compile_plan(1_000, 60.0, Vec::new(), 1.0);
    let ring = SpscRing::<AudioMessage, 32>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut audition = AuditionController::<4>::new();
    push_batch(
        &ring,
        audition
            .audition(0, 0, &[60, 64], 40_000, 5)
            .expect("first chord"),
    );
    push_batch(
        &ring,
        audition
            .audition(4, 0, &[67], 40_000, 5)
            .expect("second chord"),
    );
    assert_eq!(audition.active_voice_count(), 1);
    let mut engine = PlaybackEngine::<TestSynth, 32, 1>::new(TestSynth::default());
    let mut output = [0.0f32; 8];
    let mut channels: [&mut [f32]; 1] = [&mut output];
    let _ = engine.render(context(0, 8, 1_000), &mut channels, &plan, &ring, &mixer, &clock);
    assert_eq!(engine.backend().voice_count(), 1);
    let audition_offs = engine
        .backend()
        .records()
        .filter(|record| {
            record.event.source == EventSource::Audition
                && matches!(record.event.kind, SynthEventKind::NoteOff { .. })
        })
        .count();
    assert_eq!(audition_offs, 2);
}

#[test]
fn scrub_is_rate_limited_deduplicated_and_orders_future_releases() {
    let mut scrub = ScrubController::<4>::new(ScrubConfig::for_sample_rate(1_000));
    let first = match scrub.update(
        0,
        ScrubHit {
            token: 10,
            part: 0,
            pitches: &[60],
            cursor_units_per_second: 4.0,
        },
    ) {
        ScrubOutcome::Triggered(batch) => batch,
        _ => panic!("first hit must trigger"),
    };
    assert!(matches!(
        scrub.update(
            10,
            ScrubHit {
                token: 11,
                part: 0,
                pitches: &[62],
                cursor_units_per_second: 10.0,
            }
        ),
        ScrubOutcome::RateLimited
    ));
    assert!(matches!(
        scrub.update(
            40,
            ScrubHit {
                token: 10,
                part: 0,
                pitches: &[60],
                cursor_units_per_second: 4.0,
            }
        ),
        ScrubOutcome::Duplicate
    ));
    let second = match scrub.update(
        40,
        ScrubHit {
            token: 12,
            part: 0,
            pitches: &[67],
            cursor_units_per_second: 20.0,
        },
    ) {
        ScrubOutcome::Triggered(batch) => batch,
        _ => panic!("new hit after interval must trigger"),
    };

    let plan = compile_plan(1_000, 60.0, Vec::new(), 1.0);
    let ring = SpscRing::<AudioMessage, 16>::new();
    push_batch(&ring, first);
    push_batch(&ring, second);
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    let mut output = [0.0f32; 140];
    let mut channels: [&mut [f32]; 1] = [&mut output];
    let _ = engine.render(context(0, 140, 1_000), &mut channels, &plan, &ring, &mixer, &clock);
    let scrub_ons: Vec<_> = engine
        .backend()
        .records()
        .filter(|record| {
            record.event.source == EventSource::Scrub
                && matches!(record.event.kind, SynthEventKind::NoteOn { .. })
        })
        .map(|record| record.timing.device_sample)
        .collect();
    assert_eq!(scrub_ons, vec![0, 40]);
    assert_eq!(engine.backend().voice_count(), 0);
}

#[test]
fn spsc_ring_reports_overflow_and_preserves_fifo_order() {
    let ring = SpscRing::<u32, 2>::new();
    assert_eq!(ring.capacity(), 2);
    assert_eq!(ring.push(10), Ok(()));
    assert_eq!(ring.push(20), Ok(()));
    assert_eq!(ring.push(30), Err(30));
    assert_eq!(ring.overflow_count(), 1);
    assert_eq!(ring.peek(), Some(10));
    assert_eq!(ring.pop(), Some(10));
    assert_eq!(ring.pop(), Some(20));
    assert_eq!(ring.pop(), None);
}

#[test]
fn visual_estimate_and_beat_light_share_the_audio_anchor() {
    let plan = compile_plan(100, 60.0, Vec::new(), 8.0);
    let snapshot = AudioClockSnapshot {
        device_sample: 100,
        presentation_sample: 100.0,
        presentation_host_ns: 1_000_000_000,
        score_quarter: 1.0,
        sample_rate: 100,
        state: PlaybackState::Playing,
        tempo_scale: 1.0,
        quality: ClockQuality::Exact,
        ..AudioClockSnapshot::default()
    };
    let at_frame = snapshot.estimate(&plan, 1_500_000_000);
    assert_eq!(at_frame.timeline_sample, 150.0);
    assert!((at_frame.score_quarter - 1.5).abs() < 1.0e-12);
    let meter = Meter::new(4, 4, &[]).expect("meter");
    let light = BeatIndicator::from_clock(snapshot, &plan, 1_500_000_000, meter);
    assert_eq!(light.group_index, 1);
    assert!((light.phase - 0.5).abs() < 1.0e-12);
}

#[test]
fn render_path_performs_no_allocations() {
    let plan = compile_plan(1_000, 120.0, vec![exact_note(0.1, 0.5, 1, 60)], 1.0);
    let ring = SpscRing::<AudioMessage, 8>::new();
    let mixer = PartMixer::<1>::new();
    let clock = AtomicAudioClock::new();
    let mut engine = PlaybackEngine::<TestSynth, 16, 1>::new(TestSynth::default());
    engine.play(&plan);
    let mut output = [0.0f32; 256];
    let mut channels: [&mut [f32]; 1] = [&mut output];

    COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
    TRACKED_ALLOCATIONS.store(0, Ordering::Relaxed);
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(true));
    let status = engine.render(
        context(0, 256, 1_000),
        &mut channels,
        &plan,
        &ring,
        &mixer,
        &clock,
    );
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
    assert_eq!(status, RenderStatus::Rendered);
    assert_eq!(TRACKED_ALLOCATIONS.load(Ordering::Relaxed), 0);
}

#[test]
fn realtime_source_has_no_forbidden_callback_apis() {
    let source = include_str!("realtime.rs");
    let start = source.find("BEGIN REALTIME_RENDER_PATH").expect("start marker");
    let end = source.find("END REALTIME_RENDER_PATH").expect("end marker");
    let path = &source[start..end];
    for forbidden in [
        "Vec<",
        "Box<",
        "Mutex",
        "RwLock",
        "println!",
        "eprintln!",
        "panic!",
        ".unwrap(",
        ".expect(",
        "std::fs",
        "std::net",
    ] {
        assert!(!path.contains(forbidden), "forbidden realtime API: {forbidden}");
    }
}

#[test]
fn mixer_solo_and_tempo_trainer_are_loop_boundary_driven() {
    let mixer = PartMixer::<2>::new();
    assert!(mixer.set(
        1,
        PartMix {
            solo: true,
            pan: 1.0,
            ..PartMix::default()
        }
    ));
    let mut snapshot = [MixSnapshot::default(); 2];
    mixer.snapshot(&mut snapshot);
    assert!(!snapshot[0].audible);
    assert!(snapshot[1].audible);
    assert!(snapshot[1].right > snapshot[1].left);

    let mut trainer = TempoTrainer::new(60.0, 72.0, TempoRampStep::Percent(10.0))
        .expect("trainer");
    assert!((trainer.on_loop_boundary(60.0) - 1.1).abs() < 1.0e-12);
    assert!((trainer.on_loop_boundary(60.0) - 1.2).abs() < 1.0e-12);
    assert_eq!(trainer.current_bpm(), 72.0);
}

#[test]
fn tap_tempo_uses_median_and_rejects_an_outlier() {
    let mut taps = TapTempo::new();
    assert_eq!(taps.tap(0), None);
    let _ = taps.tap(500_000_000);
    let _ = taps.tap(1_000_000_000);
    let _ = taps.tap(1_900_000_000);
    let bpm = taps.tap(2_400_000_000).expect("tap bpm");
    assert!((bpm - 120.0).abs() < 8.0);
}
