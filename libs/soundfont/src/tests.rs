use core::cell::Cell;
use core::mem::needs_drop;
use std::alloc::{GlobalAlloc, Layout, System};

use super::*;

struct CountingAllocator;

thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let tracking = TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false);
        if tracking {
            let _ = ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
        }
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout)
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let tracking = TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false);
        if tracking {
            let _ = ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
        }
        System.realloc(pointer, layout, new_size)
    }
}

#[global_allocator]
static TEST_ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations_during(run: impl FnOnce()) -> usize {
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(false));
    ALLOCATION_COUNT.with(|count| count.set(0));
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(true));
    run();
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(false));
    ALLOCATION_COUNT.with(Cell::get)
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_i16(output: &mut Vec<u8>, value: i16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn fixed(output: &mut Vec<u8>, text: &str, width: usize) {
    let bytes = text.as_bytes();
    let count = bytes.len().min(width);
    output.extend_from_slice(&bytes[..count]);
    output.resize(output.len() + width - count, 0);
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(id);
    push_u32(&mut output, data.len() as u32);
    output.extend_from_slice(data);
    if data.len() & 1 != 0 {
        output.push(0);
    }
    output
}

fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut data = kind.to_vec();
    for item in chunks {
        data.extend_from_slice(item);
    }
    chunk(b"LIST", &data)
}

fn generator(output: &mut Vec<u8>, operator: u16, amount: u16) {
    push_u16(output, operator);
    push_u16(output, amount);
}

fn bag(output: &mut Vec<u8>, generator: u16, modulator: u16) {
    push_u16(output, generator);
    push_u16(output, modulator);
}

fn modulator(output: &mut Vec<u8>, source: u16, destination: u16, amount: i16) {
    push_u16(output, source);
    push_u16(output, destination);
    push_i16(output, amount);
    push_u16(output, 0);
    push_u16(output, 0);
}

fn preset_header(output: &mut Vec<u8>, name: &str, program: u16, bank: u16, bag: u16) {
    fixed(output, name, 20);
    push_u16(output, program);
    push_u16(output, bank);
    push_u16(output, bag);
    push_u32(output, 0);
    push_u32(output, 0);
    push_u32(output, 0);
}

fn instrument_header(output: &mut Vec<u8>, name: &str, bag: u16) {
    fixed(output, name, 20);
    push_u16(output, bag);
}

#[derive(Clone, Copy)]
struct TestSample {
    start: u32,
    end: u32,
    loop_start: u32,
    loop_end: u32,
    link: u16,
    sample_type: u16,
}

fn sample_header(output: &mut Vec<u8>, name: &str, sample: TestSample) {
    fixed(output, name, 20);
    push_u32(output, sample.start);
    push_u32(output, sample.end);
    push_u32(output, sample.loop_start);
    push_u32(output, sample.loop_end);
    push_u32(output, 44_100);
    output.push(60);
    output.push(0);
    push_u16(output, sample.link);
    push_u16(output, sample.sample_type);
}

fn make_sf2(with_sm24: bool) -> Vec<u8> {
    let samples = [TestSample {
        start: 0,
        end: 8,
        loop_start: 2,
        loop_end: 6,
        link: 0,
        sample_type: 1,
    }];
    make_sf2_with_samples(with_sm24, &samples, 0)
}

fn make_sf2_with_samples(with_sm24: bool, samples: &[TestSample], selected_sample: u16) -> Vec<u8> {
    let point_count = samples
        .iter()
        .filter(|sample| sample.sample_type & 0x8000 == 0)
        .map(|sample| sample.end as usize)
        .max()
        .unwrap_or(8)
        .max(8);
    let mut smpl = Vec::new();
    for index in 0..point_count {
        let value = if with_sm24 {
            match index {
                0 => i16::MIN,
                1 => -1,
                2 => 0,
                3 => 1,
                _ => index as i16 * 100,
            }
        } else {
            index as i16 * 1000
        };
        push_i16(&mut smpl, value);
    }
    let mut sdta_chunks = vec![chunk(b"smpl", &smpl)];
    if with_sm24 {
        let mut low = vec![0; point_count];
        low[1] = 255;
        low[3] = 128;
        sdta_chunks.push(chunk(b"sm24", &low));
    }

    let mut phdr = Vec::new();
    preset_header(&mut phdr, "Test preset", 0, 0, 0);
    preset_header(&mut phdr, "EOP", 0, 0, 2);

    let mut pbag = Vec::new();
    bag(&mut pbag, 0, 0);
    bag(&mut pbag, 2, 1);
    bag(&mut pbag, 4, 2);

    let mut pgen = Vec::new();
    generator(&mut pgen, 48, 10u16);
    generator(&mut pgen, 43, u16::from(10u8) | (u16::from(100u8) << 8));
    generator(&mut pgen, 48, 20u16);
    generator(&mut pgen, 41, 0);
    generator(&mut pgen, 60, 0);

    let mut inst = Vec::new();
    instrument_header(&mut inst, "Test instrument", 0);
    instrument_header(&mut inst, "EOI", 2);

    let mut ibag = Vec::new();
    bag(&mut ibag, 0, 0);
    bag(&mut ibag, 3, 1);
    bag(&mut ibag, 8, 2);

    let mut igen = Vec::new();
    generator(&mut igen, 48, 100u16);
    generator(&mut igen, 43, u16::from(20u8) | (u16::from(80u8) << 8));
    generator(&mut igen, 34, (-2400i16) as u16);
    generator(&mut igen, 48, 30u16);
    generator(&mut igen, 44, u16::from(40u8) | (u16::from(90u8) << 8));
    generator(&mut igen, 52, 25u16);
    generator(&mut igen, 54, 1u16);
    generator(&mut igen, 53, selected_sample);
    generator(&mut igen, 60, 0);

    let mut pmod = Vec::new();
    modulator(&mut pmod, 2, 48, 100);
    modulator(&mut pmod, 2, 48, 20);
    modulator(&mut pmod, 0, 0, 0);
    let mut imod = Vec::new();
    modulator(&mut imod, 2, 48, 30);
    modulator(&mut imod, 2, 48, 5);
    modulator(&mut imod, 0, 0, 0);
    let mut shdr = Vec::new();
    for (index, sample) in samples.iter().copied().enumerate() {
        sample_header(&mut shdr, &format!("Sample {index}"), sample);
    }
    sample_header(
        &mut shdr,
        "EOS",
        TestSample {
            start: point_count as u32,
            end: point_count as u32,
            loop_start: point_count as u32,
            loop_end: point_count as u32,
            link: 0,
            sample_type: 1,
        },
    );

    let info = list(
        b"INFO",
        &[
            chunk(b"ifil", &[2, 0, 4, 0]),
            chunk(b"INAM", b"Hermetic fixture\0"),
            chunk(b"XTRA", b"ignored\0"),
        ],
    );
    let sdta = list(b"sdta", &sdta_chunks);
    let pdta = list(
        b"pdta",
        &[
            chunk(b"phdr", &phdr),
            chunk(b"pbag", &pbag),
            chunk(b"pmod", &pmod),
            chunk(b"pgen", &pgen),
            chunk(b"inst", &inst),
            chunk(b"ibag", &ibag),
            chunk(b"imod", &imod),
            chunk(b"igen", &igen),
            chunk(b"shdr", &shdr),
        ],
    );
    let mut body = b"sfbk".to_vec();
    body.extend_from_slice(&chunk(b"JUNK", b"walk over me"));
    body.extend_from_slice(&info);
    body.extend_from_slice(&sdta);
    body.extend_from_slice(&pdta);
    let mut output = b"RIFF".to_vec();
    push_u32(&mut output, body.len() as u32);
    output.extend_from_slice(&body);
    output
}

#[test]
fn riff_walks_lists_and_builds_full_hierarchy() {
    let font = parse_sf2(&make_sf2(false)).unwrap();
    assert_eq!(font.info.iter().find(|entry| entry.id == *b"INAM").unwrap().value, "Hermetic fixture");
    assert_eq!(font.presets.len(), 1);
    assert_eq!(font.presets[0].zones.len(), 2);
    assert_eq!(font.presets[0].zones[0].modulators[0].amount, 100);
    assert_eq!(font.presets[0].zones[1].modulators[0].amount, 20);
    assert_eq!(font.instruments.len(), 1);
    assert_eq!(font.instruments[0].zones.len(), 2);
    assert_eq!(font.instruments[0].zones[0].modulators[0].amount, 30);
    assert_eq!(font.instruments[0].zones[1].modulators[0].amount, 5);
    assert_eq!(font.samples.len(), 1);
    assert_eq!(font.sample_precision, 16);
}

#[test]
fn sf2_global_override_and_preset_addition_rules() {
    let font = parse_sf2(&make_sf2(false)).unwrap();
    let zone = &font.zones[0];
    // Preset local 20 replaces preset global 10. Instrument local 30 replaces
    // instrument global 100. The two resolved levels then add: 20 + 30 = 50 cB.
    let expected_gain = 10.0_f32.powf(-50.0 / 200.0);
    assert!((zone.parameters.gain - expected_gain).abs() < 1e-6);
    assert_eq!(zone.key_range, Range { low: 20, high: 80 });
    assert_eq!(zone.velocity_range, Range { low: 40, high: 90 });
    assert!((zone.parameters.envelope.attack - 0.25).abs() < 1e-6);
    assert_eq!(zone.parameters.tune_cents, 25.0);
    // The nonzero intrinsic filter default is introduced once at instrument
    // level, rather than accidentally being added again at preset level.
    assert!((zone.parameters.filter_cutoff_hz - 19_912.0).abs() < 2.0);
}

#[test]
fn sf2_selection_honors_inclusive_key_velocity_boundaries() {
    let font = parse_sf2(&make_sf2(false)).unwrap();
    assert_eq!(font.select(0, 20, 40).len(), 1);
    assert_eq!(font.select(0, 80, 90).len(), 1);
    assert!(font.select(0, 19, 40).is_empty());
    assert!(font.select(0, 20, 39).is_empty());
    assert!(font.select(1, 60, 64).is_empty());
}

#[test]
fn sf2_decodes_24_bit_extension() {
    let font = parse_sf2(&make_sf2(true)).unwrap();
    assert_eq!(font.sample_precision, 24);
    assert_eq!(font.pcm()[0], -1.0);
    assert_eq!(font.pcm()[1], -1.0 / 8_388_608.0);
    assert_eq!(font.pcm()[3], 384.0 / 8_388_608.0);
}

#[test]
fn stereo_links_and_rom_samples_have_nonblocking_semantics() {
    let stereo = [
        TestSample { start: 0, end: 4, loop_start: 1, loop_end: 3, link: 1, sample_type: 4 },
        TestSample { start: 4, end: 8, loop_start: 5, loop_end: 7, link: 0, sample_type: 2 },
    ];
    let font = parse_sf2(&make_sf2_with_samples(false, &stereo, 0)).unwrap();
    assert_eq!(
        font.read_frame(0, 1),
        SampleRead::Resident { left: 1000.0 / 32768.0, right: 5000.0 / 32768.0 }
    );

    let rom = [TestSample {
        start: 1_000_000,
        end: 1_000_008,
        loop_start: 1_000_002,
        loop_end: 1_000_006,
        link: 0,
        sample_type: 0x8001,
    }];
    let font = parse_sf2(&make_sf2_with_samples(false, &rom, 0)).unwrap();
    assert_eq!(font.read_frame(0, 0), SampleRead::Missing);
}

#[test]
fn malformed_and_truncated_sf2_return_typed_errors() {
    assert!(matches!(parse_sf2(b"not riff"), Err(LoadError::Truncated { .. })));
    let mut truncated = make_sf2(false);
    truncated.truncate(truncated.len() - 3);
    assert!(matches!(parse_sf2(&truncated), Err(LoadError::Truncated { .. })));
    let mut wrong = make_sf2(false);
    wrong[0] = b'X';
    assert_eq!(parse_sf2(&wrong).unwrap_err(), LoadError::InvalidRiff);
}

#[test]
fn sfz_subset_inherits_scopes_and_ignores_unknown_opcodes() {
    let text = r#"
        <global> volume=-6 mystery_opcode=hello
        <group> lokey=C4 hikey=C5 lovel=20 hivel=100 ampeg_attack=0.01
        <region> sample="Piano Samples/C4.wav" key=C4 tune=-7 loop_mode=loop_sustain
                 loop_start=10 loop_end=19 pan=-25 ampeg_sustain=60
        <region> sample=second.wav lokey=61 hikey=72 pitch_keycenter=64
    "#;
    let mut sfz = parse_sfz(text).unwrap();
    assert_eq!(sfz.zones.len(), 2);
    assert_eq!(sfz.samples[0].path, "Piano Samples/C4.wav");
    assert_eq!(sfz.ignored_opcodes, ["mystery_opcode"]);
    assert_eq!(sfz.select(60, 20).len(), 1);
    assert!(sfz.select(60, 19).is_empty());
    let selected = sfz.select(60, 64)[0];
    assert_eq!(selected.root_key, 60.0);
    assert_eq!(selected.tune_cents, -7.0);
    assert_eq!(selected.loop_mode, LoopMode::UntilRelease);
    assert_eq!((selected.loop_start, selected.loop_end), (10, 20));
    assert!((selected.pan + 0.25).abs() < 1e-6);
    assert!((selected.envelope.attack - 0.01).abs() < 1e-6);
    assert!((selected.envelope.sustain - 0.6).abs() < 1e-6);
    assert!(sfz.set_sample_metadata(
        0,
        SampleMetadata {
            sample_rate: 48_000,
            frames: 200,
            loop_start: Some(12),
            loop_end: Some(30),
        },
    ));
    let selected = sfz.select(60, 64)[0];
    assert_eq!(selected.sample_rate, 48_000);
    // Explicit region loop points continue to override decoded metadata.
    assert_eq!((selected.loop_start, selected.loop_end), (10, 20));
}

#[test]
fn envelope_stage_timing_uses_worked_frame_counts() {
    let mut envelope = EnvelopeRunner::new(
        Envelope { delay: 0.2, attack: 0.4, hold: 0.2, decay: 0.3, sustain: 0.4, release: 0.3 },
        10.0,
    );
    let values: Vec<f32> = (0..12).map(|_| envelope.next_value()).collect();
    let expected = [0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.4];
    for (actual, expected) in values.iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
    envelope.release();
    let release = [envelope.next_value(), envelope.next_value(), envelope.next_value()];
    for (actual, expected) in release.into_iter().zip([0.4 * 2.0 / 3.0, 0.4 / 3.0, 0.0]) {
        assert!((actual - expected).abs() < 1e-6);
    }
    assert!(envelope.is_finished());
}

#[test]
fn pitch_ratio_combines_root_tuning_and_rate() {
    let mut parameters = sample_parameters();
    parameters.key = 72;
    parameters.root_key = 60.0;
    parameters.tune_cents = 1200.0;
    parameters.sample_rate = 22_050;
    assert_eq!(parameters.pitch_ratio(44_100.0), 2.0);
}

struct SliceSource<'a>(&'a [f32]);

impl SampleSource for SliceSource<'_> {
    fn read_frame(&self, _sample_id: u32, frame: i64) -> SampleRead {
        self.0
            .get(usize::try_from(frame).unwrap_or(usize::MAX))
            .copied()
            .map(|value| SampleRead::Resident { left: value, right: value })
            .unwrap_or(SampleRead::Missing)
    }
}

fn sample_parameters() -> VoiceParameters {
    VoiceParameters {
        source: VoiceSource::Sample { sample_id: 0 },
        key: 60,
        velocity: 127,
        root_key: 60.0,
        tune_cents: 0.0,
        scale_tuning: 100.0,
        sample_rate: 44_100,
        start_frame: 0,
        end_frame: 64,
        loop_start: 2,
        loop_end: 5,
        loop_mode: LoopMode::NoLoop,
        release_on_note_off: true,
        envelope: Envelope { delay: 0.0, attack: 0.0, hold: 0.0, decay: 0.0, sustain: 1.0, release: 0.0 },
        gain: 2.0_f32.sqrt(),
        pan: 0.0,
        filter_cutoff_hz: 20_000.0,
        filter_resonance_db: 0.0,
        exclusive_class: 0,
    }
}

#[test]
fn cubic_interpolation_loop_wraps_at_exclusive_end() {
    let data = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let source = SliceSource(&data);
    let mut parameters = sample_parameters();
    parameters.end_frame = data.len() as i64;
    parameters.loop_mode = LoopMode::Continuous;
    let events = [TimedEvent { offset: 0, event: SamplerEvent::NoteOn { note_id: 1, parameters } }];
    let mut left = [0.0; 8];
    let mut right = [0.0; 8];
    Sampler::<2>::new(44_100.0).render(&source, &events, &mut left, &mut right);
    for (actual, expected) in left.into_iter().zip([0.0, 1.0, 2.0, 3.0, 4.0, 2.0, 3.0, 4.0]) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }
}

#[test]
fn sustain_loop_stops_wrapping_during_release() {
    let data = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let source = SliceSource(&data);
    let mut parameters = sample_parameters();
    parameters.sample_rate = 10;
    parameters.end_frame = data.len() as i64;
    parameters.loop_mode = LoopMode::UntilRelease;
    parameters.envelope.release = 1.0;
    let events = [
        TimedEvent { offset: 0, event: SamplerEvent::NoteOn { note_id: 1, parameters } },
        TimedEvent { offset: 6, event: SamplerEvent::NoteOff { note_id: 1 } },
    ];
    let mut left = [0.0; 10];
    let mut right = [0.0; 10];
    Sampler::<1>::new(10.0).render(&source, &events, &mut left, &mut right);
    for (actual, expected) in left.into_iter().zip([
        0.0, 1.0, 2.0, 3.0, 4.0, 2.0, 2.7, 3.2, 3.5, 3.6,
    ]) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }
}

#[test]
fn spsc_queue_is_bounded_fifo_and_reports_overflow() {
    let mut queue = SpscQueue::<u32, 4>::new();
    assert_eq!(queue.capacity(), 3);
    assert!(queue.is_empty());
    let (mut producer, mut consumer) = queue.split();
    assert_eq!(producer.push(10), Ok(()));
    assert_eq!(producer.push(11), Ok(()));
    assert_eq!(producer.push(12), Ok(()));
    assert_eq!(producer.push(13), Err(13));
    assert_eq!(consumer.len(), 3);
    assert_eq!(consumer.pop(), Some(10));
    assert_eq!(producer.push(13), Ok(()));
    assert_eq!(consumer.pop(), Some(11));
    assert_eq!(consumer.pop(), Some(12));
    assert_eq!(consumer.pop(), Some(13));
    assert_eq!(consumer.pop(), None);
}

#[test]
fn spsc_queue_transfers_concurrently_in_order() {
    let mut queue = SpscQueue::<u32, 32>::new();
    let (mut producer, mut consumer) = queue.split();
    std::thread::scope(|scope| {
        let consumer_thread = scope.spawn(move || {
            for expected in 0..10_000 {
                loop {
                    if let Some(actual) = consumer.pop() {
                        assert_eq!(actual, expected);
                        break;
                    }
                    std::thread::yield_now();
                }
            }
        });
        for value in 0..10_000 {
            while producer.push(value).is_err() {
                std::thread::yield_now();
            }
        }
        consumer_thread.join().unwrap();
    });
    assert!(queue.is_empty());
}

#[test]
fn events_are_sample_accurate_inside_a_block() {
    let data = [1.0; 64];
    let source = SliceSource(&data);
    let parameters = sample_parameters();
    let events = [
        TimedEvent { offset: 3, event: SamplerEvent::NoteOn { note_id: 7, parameters } },
        TimedEvent { offset: 6, event: SamplerEvent::NoteOff { note_id: 7 } },
    ];
    let mut left = [9.0; 9];
    let mut right = [9.0; 9];
    let report = Sampler::<2>::new(44_100.0).render(&source, &events, &mut left, &mut right);
    assert_eq!(report.events_applied, 2);
    assert_eq!(&left[..3], &[0.0, 0.0, 0.0]);
    assert!(left[3] > 0.99 && left[4] > 0.99 && left[5] > 0.99);
    assert_eq!(&left[6..], &[0.0, 0.0, 0.0]);
}

fn render_with_blocks(block_size: usize) -> Vec<u32> {
    let source = NoSamples;
    let mut queue = SpscQueue::<ScheduledEvent, 16>::new();
    let (mut producer, mut consumer) = queue.split();
    let piano = piano_fallback(64, 104);
    let click = metronome_click(true);
    producer
        .push(ScheduledEvent { frame: 5, event: SamplerEvent::NoteOn { note_id: 1, parameters: piano } })
        .unwrap();
    producer
        .push(ScheduledEvent { frame: 137, event: SamplerEvent::NoteOff { note_id: 1 } })
        .unwrap();
    producer
        .push(ScheduledEvent { frame: 203, event: SamplerEvent::NoteOn { note_id: 2, parameters: click } })
        .unwrap();
    let mut sampler = Sampler::<8>::new(44_100.0);
    let mut result = Vec::new();
    let mut remaining = 400;
    while remaining > 0 {
        let frames = remaining.min(block_size);
        let mut left = vec![0.0; frames];
        let mut right = vec![0.0; frames];
        sampler.render_from_queue(&source, &mut consumer, &mut left, &mut right);
        result.extend(left.into_iter().map(f32::to_bits));
        remaining -= frames;
    }
    result
}

#[test]
fn output_is_bit_identical_across_block_sizes() {
    let reference = render_with_blocks(400);
    assert_eq!(render_with_blocks(1), reference);
    assert_eq!(render_with_blocks(7), reference);
    assert_eq!(render_with_blocks(64), reference);
    assert_eq!(render_with_blocks(127), reference);
}

#[test]
fn render_path_allocates_nothing_and_owns_no_drop_state() {
    assert!(!needs_drop::<Sampler<8>>());
    assert!(!needs_drop::<SamplerEvent>());
    assert!(!needs_drop::<SpscQueue<ScheduledEvent, 16>>());
    let source = SliceSource(&[0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5]);
    let mut parameters = sample_parameters();
    parameters.end_frame = 8;
    parameters.loop_start = 0;
    parameters.loop_end = 8;
    parameters.loop_mode = LoopMode::Continuous;
    let events = [TimedEvent { offset: 0, event: SamplerEvent::NoteOn { note_id: 1, parameters } }];
    let mut sampler = Sampler::<8>::new(44_100.0);
    let mut left = [0.0; 32];
    let mut right = [0.0; 32];
    let count = allocations_during(|| {
        sampler.render(&source, &events, &mut left, &mut right);
    });
    assert_eq!(count, 0, "render performed {count} heap allocations");
}
