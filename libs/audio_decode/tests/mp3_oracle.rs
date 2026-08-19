//! MP3: streams synthesised bit by bit inside the test, a no-panic sweep over
//! mangled copies of them, and — behind an environment variable — a full
//! comparison against CoreAudio's decoder.
//!
//! No audio file is committed for MP3. Everything the fast tests need is built
//! here from the standard's own field layout, which has the useful property
//! that the *expected* output is known exactly rather than measured: a granule
//! carrying a single quantised spectral line of magnitude 1 at `global_gain`
//! 210 must come out as that line and nothing else.
//!
//! The oracle test needs reference WAVs that `afconvert` produced from the same
//! MP3s, so it stays opt-in:
//!
//! ```text
//! afconvert -f WAVE -d LEF32 in.mp3 $DIR/in.wav   # and cp in.mp3 $DIR/
//! MAKEPAD_MP3_ORACLE=$DIR cargo test -p makepad-audio-decode --release \
//!     --test mp3_oracle -- --nocapture oracle
//! ```
//!
//! CoreAudio's MP3 decoder emits 16-bit samples (its 24-bit and float outputs
//! have zero low bytes), so the comparison quantises our float output the same
//! way before measuring. Without that the metric reports the oracle's headroom
//! loss on loud masters — most club records decode past full scale — instead of
//! any difference between the decoders.

use makepad_audio_decode::{decode_any, mp3, AudioError, AudioFormat, Limits};

// ---------------------------------------------------------------------------
// a bitstream writer, and enough of an encoder to build valid frames
// ---------------------------------------------------------------------------

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            if self.bit % 8 == 0 {
                self.bytes.push(0);
            }
            if (value >> i) & 1 == 1 {
                let at = self.bytes.len() - 1;
                self.bytes[at] |= 0x80 >> (self.bit % 8);
            }
            self.bit += 1;
        }
    }

    fn pad_to(&mut self, bytes: usize) {
        while self.bytes.len() < bytes {
            self.bytes.push(0);
        }
        self.bit = self.bytes.len() * 8;
    }
}

#[derive(Clone, Copy)]
struct Version {
    /// Header version bits: 3 = MPEG-1, 2 = MPEG-2, 0 = MPEG-2.5.
    bits: u32,
    lsf: bool,
}

const MPEG1: Version = Version { bits: 3, lsf: false };
const MPEG2: Version = Version { bits: 2, lsf: true };
const MPEG25: Version = Version { bits: 0, lsf: true };

/// One granule/channel's worth of the side info this builder varies.
#[derive(Clone, Copy, Default)]
struct Gr {
    part2_3: u32,
    big_values: u32,
    global_gain: u32,
    table0: u32,
    scalefac_compress: u32,
    /// Emit a mixed block (window switching, short blocks, mixed flag) with
    /// these three subblock gains.
    mixed: Option<[u32; 3]>,
}

struct FrameSpec {
    version: Version,
    /// Layer III bitrate index (9 = 128 kbit/s on MPEG-1).
    bitrate_index: u32,
    rate_index: u32,
    /// 0 stereo, 1 joint stereo, 3 mono.
    mode: u32,
    /// Joint-stereo mode extension: bit 0 intensity, bit 1 mid/side.
    mode_ext: u32,
    granules: Vec<Vec<Gr>>,
    /// Main-data payload as (value, bit count) pairs.
    main: Vec<(u32, u32)>,
}

impl FrameSpec {
    fn silence(version: Version, mode: u32) -> Self {
        let channels = if mode == 3 { 1 } else { 2 };
        let granules = if version.lsf { 1 } else { 2 };
        Self {
            version,
            bitrate_index: 9,
            rate_index: 0,
            mode,
            mode_ext: 0,
            granules: vec![vec![Gr::default(); channels]; granules],
            main: Vec::new(),
        }
    }

    fn channels(&self) -> usize {
        if self.mode == 3 {
            1
        } else {
            2
        }
    }

    /// The rate the header encodes: MPEG-2 halves MPEG-1's, MPEG-2.5 quarters it.
    fn sample_rate(&self) -> u32 {
        let base: [u32; 3] = [44_100, 48_000, 32_000];
        let shift = match self.version.bits {
            3 => 0,
            2 => 1,
            _ => 2,
        };
        base[self.rate_index as usize] >> shift
    }

    fn side_info_bytes(&self) -> usize {
        match (self.version.lsf, self.channels()) {
            (false, 1) => 17,
            (false, _) => 32,
            (true, 1) => 9,
            (true, _) => 17,
        }
    }

    fn frame_bytes(&self) -> usize {
        let rate = self.sample_rate() as usize;
        let kbps: [usize; 16] = if self.version.lsf {
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0]
        } else {
            [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0]
        };
        let samples = if self.version.lsf { 576 } else { 1152 };
        (samples / 8) * kbps[self.bitrate_index as usize] * 1000 / rate
    }

    fn build(&self) -> Vec<u8> {
        let mut w = BitWriter::default();
        // Header.
        w.put(0x7ff, 11);
        w.put(self.version.bits, 2);
        w.put(1, 2); // layer III
        w.put(1, 1); // no CRC
        w.put(self.bitrate_index, 4);
        w.put(self.rate_index, 2);
        w.put(0, 1); // padding
        w.put(0, 1); // private
        w.put(self.mode, 2);
        w.put(self.mode_ext, 2);
        w.put(0, 4); // copyright / original / emphasis

        // Side info.
        let channels = self.channels();
        w.put(0, if self.version.lsf { 8 } else { 9 }); // main_data_begin
        w.put(
            0,
            match (self.version.lsf, channels) {
                (false, 1) => 5,
                (false, _) => 3,
                (true, 1) => 1,
                (true, _) => 2,
            },
        );
        if !self.version.lsf {
            w.put(0, 4 * channels as u32); // scfsi
        }
        for granule in &self.granules {
            for gr in granule.iter().take(channels) {
                w.put(gr.part2_3, 12);
                w.put(gr.big_values, 9);
                w.put(gr.global_gain, 8);
                w.put(gr.scalefac_compress, if self.version.lsf { 9 } else { 4 });
                match gr.mixed {
                    Some(gains) => {
                        w.put(1, 1); // window_switching
                        w.put(2, 2); // block_type: short
                        w.put(1, 1); // mixed_block_flag
                        w.put(gr.table0, 5);
                        w.put(gr.table0, 5); // only two tables when switching
                        for gain in gains {
                            w.put(gain, 3);
                        }
                    }
                    None => {
                        w.put(0, 1); // window_switching
                        w.put(gr.table0, 5);
                        w.put(0, 5);
                        w.put(0, 5); // table_select
                        w.put(0, 4); // region0_count
                        w.put(0, 3); // region1_count
                    }
                }
                if !self.version.lsf {
                    w.put(0, 1); // preflag, present with or without switching
                }
                w.put(0, 1); // scalefac_scale
                w.put(0, 1); // count1table_select
            }
        }
        assert_eq!(w.bytes.len(), 4 + self.side_info_bytes(), "side info width");

        // Main data, then zero padding out to the frame length.
        for &(value, bits) in &self.main {
            w.put(value, bits);
        }
        let total = self.frame_bytes();
        assert!(w.bytes.len() <= total, "main data does not fit the frame");
        w.pad_to(total);
        w.bytes
    }
}

/// A stream of `count` frames built from `spec`.
fn stream(spec: &FrameSpec, count: usize) -> Vec<u8> {
    let frame = spec.build();
    frame.repeat(count)
}

/// A frame whose first granule, first channel carries exactly one spectral
/// line: Huffman table 1's codeword for the pair (1, 0) is `01`, followed by
/// one sign bit. At `global_gain` 210 the requantised value is exactly 1.0.
fn one_line_frame(global_gain: u32, negative: bool) -> FrameSpec {
    let mut spec = FrameSpec::silence(MPEG1, 0);
    spec.granules[0][0] = Gr { part2_3: 3, big_values: 1, global_gain, table0: 1, ..Gr::default() };
    spec.main = vec![(0b01, 2), (u32::from(negative), 1)];
    spec
}

fn decode(bytes: &[u8], gapless: bool) -> makepad_audio_decode::DecodedAudio {
    if gapless {
        return mp3::decode_all(bytes).expect("decodes");
    }
    let mut decoder = mp3::Mp3Decoder::new(bytes).expect("syncs");
    decoder.set_gapless(false);
    let mut pcm = Vec::new();
    let (mut rate, mut channels) = (0, 0);
    while let Some(frame) = decoder.next_frame().expect("frame") {
        rate = frame.rate;
        channels = frame.channels;
        pcm.extend_from_slice(frame.pcm);
    }
    makepad_audio_decode::DecodedAudio { rate, channels, pcm_interleaved_f32: pcm }
}

// ---------------------------------------------------------------------------
// synthetic streams
// ---------------------------------------------------------------------------

/// Every version, sample rate and channel mode the standard defines, with the
/// bitrate index each one is legal at. This matrix was checked sample-for-
/// sample against CoreAudio (max 1 LSB deviation on all nine rates) using
/// streams built exactly like these; the committed test keeps the shape of
/// each variant honest without needing the oracle.
const VARIANTS: [(Version, u32, u32, u32, u32, &str); 9] = [
    (MPEG1, 0, 3, 9, 44_100, "mpeg1 44100 mono"),
    (MPEG1, 1, 0, 9, 48_000, "mpeg1 48000 stereo"),
    (MPEG1, 2, 1, 5, 32_000, "mpeg1 32000 joint"),
    (MPEG2, 0, 3, 8, 22_050, "mpeg2 22050 mono"),
    (MPEG2, 1, 0, 8, 24_000, "mpeg2 24000 stereo"),
    (MPEG2, 2, 0, 4, 16_000, "mpeg2 16000 stereo"),
    (MPEG25, 0, 3, 4, 11_025, "mpeg2.5 11025 mono"),
    (MPEG25, 1, 0, 6, 12_000, "mpeg2.5 12000 stereo"),
    (MPEG25, 2, 3, 2, 8_000, "mpeg2.5 8000 mono"),
];

#[test]
fn silence_decodes_to_silence_in_every_variant() {
    for (version, rate_index, mode, bitrate_index, rate, label) in VARIANTS {
        let mut spec = FrameSpec::silence(version, mode);
        spec.rate_index = rate_index;
        spec.bitrate_index = bitrate_index;
        let audio = decode(&stream(&spec, 6), false);
        let expected_channels = if mode == 3 { 1 } else { 2 };
        let per_frame = if version.lsf { 576 } else { 1152 };
        assert_eq!(audio.channels, expected_channels, "{label}");
        assert_eq!(audio.rate, rate, "{label}");
        assert_eq!(audio.frames(), 6 * per_frame, "{label}");
        assert!(audio.pcm_interleaved_f32.iter().all(|v| *v == 0.0), "{label} is not silent");
    }
}

#[test]
fn coded_content_decodes_in_every_variant() {
    for (version, rate_index, mode, bitrate_index, rate, label) in VARIANTS {
        let mut spec = FrameSpec::silence(version, mode);
        spec.rate_index = rate_index;
        spec.bitrate_index = bitrate_index;
        // One Huffman-coded pair in the first granule of the first channel,
        // the same construction the oracle sweep used.
        spec.granules[0][0] = Gr { part2_3: 3, big_values: 1, global_gain: 210, table0: 1, ..Gr::default() };
        spec.main = vec![(0b01, 2), (0, 1)];
        let audio = decode(&stream(&spec, 6), false);
        assert_eq!(audio.rate, rate, "{label}");
        let left = audio.channel(0);
        assert!(left.iter().all(|v| v.is_finite()), "{label} produced non-finite samples");
        assert!(left.iter().any(|v| v.abs() > 1e-4), "{label} produced no audio");
        // The line sits in subband 0, so the waveform must stay well inside
        // the range a single unit-magnitude coefficient can reach.
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 2.0, "{label} peak {peak}");
        if mode != 3 {
            let right = audio.channel(1);
            assert!(right.iter().all(|v| *v == 0.0), "{label} leaked into the other channel");
        }
    }
}

/// Eight table-1 codewords for the pair (1, 1) -- `000` plus two sign bits --
/// which is the densest content this builder can express in a fixed width.
const PAIRS: u32 = 8;

fn eight_pairs() -> Vec<(u32, u32)> {
    (0..PAIRS).flat_map(|_| [(0b000, 3), (0, 1), (0, 1)]).collect()
}

#[test]
fn intensity_stereo_pans_the_left_channel_across() {
    // Joint stereo, intensity on and mid/side off. The right channel codes
    // nothing but its scalefactors, so every band is intensity coded and the
    // pan comes from `is_pos`. With `scalefac_compress` 0 every position is 0,
    // which the standard's table maps to "all the way right": k_left 0,
    // k_right 1. Verified against CoreAudio at max 1 LSB.
    let mut spec = FrameSpec::silence(MPEG1, 1);
    spec.mode_ext = 1;
    let content = Gr {
        part2_3: PAIRS * 5,
        big_values: PAIRS,
        global_gain: 196,
        table0: 1,
        ..Gr::default()
    };
    for granule in spec.granules.iter_mut() {
        granule[0] = content;
        granule[1] = Gr { global_gain: 196, ..Gr::default() };
    }
    spec.main = [eight_pairs(), eight_pairs()].concat();
    let audio = decode(&stream(&spec, 6), false);
    let (left, right) = (audio.channel(0), audio.channel(1));
    let energy = |s: &[f32]| s.iter().map(|v| (v * v) as f64).sum::<f64>();
    assert!(energy(&right) > 1e-6, "the pan target is silent");
    assert!(
        energy(&left) < energy(&right) * 1e-6,
        "left {} should have been panned away against right {}",
        energy(&left),
        energy(&right)
    );
}

#[test]
fn lsf_intensity_scale_comes_from_the_right_channel() {
    // MPEG-2 intensity stereo pans in quarter-log2 steps, and the low bit of
    // `scalefac_compress` doubles the step. That bit belongs to the channel
    // that carries the positions -- the *right* one -- and reading it from the
    // left channel instead is wrong by exactly 2^(1/4) per band. CoreAudio was
    // the arbiter here, in both directions; this pins the answer down.
    //
    // The right channel codes only its positions (`big_values` 0), so every
    // band is intensity coded. `scalefac_compress` 144/145 both select an
    // intensity partition whose first field is two bits wide, and every
    // position is written as 1: odd, so k_left is the attenuated one.
    for (compress, expected) in [(144u32, 0.25f32), (145, 0.5)] {
        let mut spec = FrameSpec::silence(MPEG2, 1);
        spec.rate_index = 0;
        spec.bitrate_index = 8;
        spec.mode_ext = 1;
        spec.granules[0][0] = Gr {
            part2_3: PAIRS * 5,
            big_values: PAIRS,
            global_gain: 196,
            table0: 1,
            ..Gr::default()
        };
        spec.granules[0][1] = Gr {
            part2_3: 14,
            global_gain: 196,
            scalefac_compress: compress,
            ..Gr::default()
        };
        // Left: the coded pairs. Right: seven two-bit positions, all 1.
        spec.main = eight_pairs();
        spec.main.extend(std::iter::repeat_n((1u32, 2u32), 7));

        let audio = decode(&stream(&spec, 8), false);
        let rms = |s: &[f32]| {
            (s.iter().map(|v| (v * v) as f64).sum::<f64>() / s.len().max(1) as f64).sqrt()
        };
        let left = rms(&audio.channel(0));
        let right = rms(&audio.channel(1));
        assert!(right > 1e-5, "compress {compress}: nothing decoded");
        // k_left / k_right = 2^-expected.
        let ratio = (left / right) as f32;
        let want = (-expected).exp2();
        assert!(
            (ratio - want).abs() < 1e-3,
            "compress {compress}: channel ratio {ratio} should be {want}"
        );
    }
}

#[test]
fn mixed_blocks_decode_with_per_window_gain() {
    // A mixed block is long in the lowest two subbands and short above them,
    // and each of the three short windows carries its own gain. Verified
    // against CoreAudio at max 1 LSB.
    let mut spec = FrameSpec::silence(MPEG1, 0);
    let content = Gr {
        part2_3: PAIRS * 5,
        big_values: PAIRS,
        global_gain: 196,
        table0: 1,
        mixed: Some([0, 1, 2]),
        ..Gr::default()
    };
    for granule in spec.granules.iter_mut() {
        for channel in granule.iter_mut() {
            *channel = content;
        }
    }
    spec.main = (0..4).flat_map(|_| eight_pairs()).collect();
    let audio = decode(&stream(&spec, 6), false);
    assert!(audio.pcm_interleaved_f32.iter().all(|v| v.is_finite()));
    let peak = audio.pcm_interleaved_f32.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 1e-3, "mixed block produced no audio (peak {peak})");
    assert!(peak < 4.0, "mixed block peak {peak} is out of range");
}

#[test]
fn gapless_trims_the_filterbank_delay() {
    let spec = FrameSpec::silence(MPEG1, 0);
    let bytes = stream(&spec, 6);
    // With no LAME tag the only trim is the filterbank's own 529 samples.
    let trimmed = decode(&bytes, true);
    let raw = decode(&bytes, false);
    assert_eq!(raw.frames() - trimmed.frames(), 529);
}

#[test]
fn a_single_spectral_line_lands_in_the_lowest_subband() {
    let spec = one_line_frame(210, false);
    let audio = decode(&stream(&spec, 4), false);
    assert_eq!(audio.channels, 2);
    let left = audio.channel(0);
    let right = audio.channel(1);
    assert!(right.iter().all(|v| *v == 0.0), "the untouched channel must stay silent");
    assert!(left.iter().any(|v| v.abs() > 1e-4), "the coded line produced nothing");
    assert!(left.iter().all(|v| v.is_finite()));

    // Subband 0 spans 0..689 Hz at 44.1 kHz, so the energy must sit in the
    // first of 32 equal bands. Measure it as band energy over a full DFT of
    // one granule pair rather than by probing single frequencies: the signal
    // here is periodic with the granule, so probe tones placed at half-bin
    // offsets would be exactly orthogonal to it and read as silence.
    let slice = &left[576..1728];
    let mut band = [0.0f64; 32];
    for k in 0..576 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in slice.iter().enumerate() {
            let phase =
                -2.0 * std::f64::consts::PI * k as f64 * i as f64 / slice.len() as f64;
            re += v as f64 * phase.cos();
            im += v as f64 * phase.sin();
        }
        band[k / 18] += re * re + im * im;
    }
    let peak = band
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i);
    assert_eq!(peak, Some(0), "energy is not in subband 0: {band:?}");
    let total: f64 = band.iter().sum();
    assert!(band[0] / total > 0.9, "subband 0 holds only {:.3} of the energy", band[0] / total);
}

#[test]
fn global_gain_is_a_quarter_power_of_two_per_step() {
    let quiet = decode(&stream(&one_line_frame(210, false), 4), false);
    let loud = decode(&stream(&one_line_frame(214, false), 4), false);
    let peak = |a: &makepad_audio_decode::DecodedAudio| {
        a.channel(0).iter().fold(0.0f32, |m, v| m.max(v.abs()))
    };
    let ratio = peak(&loud) / peak(&quiet);
    assert!((ratio - 2.0).abs() < 1e-3, "four gain steps should double: {ratio}");
}

#[test]
fn the_sign_bit_inverts_the_line() {
    let positive = decode(&stream(&one_line_frame(210, false), 4), false);
    let negative = decode(&stream(&one_line_frame(210, true), 4), false);
    let (a, b) = (positive.channel(0), negative.channel(0));
    assert_eq!(a.len(), b.len());
    assert!(a.iter().zip(&b).all(|(x, y)| (x + y).abs() < 1e-6), "not an exact negation");
}

#[test]
fn probe_duration_matches_a_full_decode() {
    let spec = FrameSpec::silence(MPEG1, 0);
    let bytes = stream(&spec, 20);
    let probed = mp3::probe_duration(&bytes).expect("probe");
    let decoded = decode(&bytes, false).duration_secs();
    // The constant-bitrate estimate is exact for a constant-bitrate stream.
    assert!((probed - decoded).abs() < 1e-9, "{probed} vs {decoded}");
}

#[test]
fn probe_duration_ignores_bytes_that_are_not_frames() {
    // Anything appended after the last frame -- an APEv2 tag, a Lyrics3 block,
    // padding -- would inflate a duration estimated from the file length. The
    // probe walks the frame chain instead, so it does not move.
    let spec = FrameSpec::silence(MPEG1, 0);
    let bytes = stream(&spec, 20);
    let clean = mp3::probe_duration(&bytes).expect("probe");
    let mut padded = bytes.clone();
    padded.extend_from_slice(b"APETAGEX");
    padded.extend(std::iter::repeat_n(0x5au8, 40_000));
    let padded_probe = mp3::probe_duration(&padded).expect("probe");
    assert!(
        (clean - padded_probe).abs() < 1e-9,
        "{clean} became {padded_probe} because of trailing junk"
    );
    assert!((clean - 20.0 * 1152.0 / 44_100.0).abs() < 1e-9, "{clean}");
    // Leading junk before the first frame does not move it either.
    let mut prefixed = vec![0x11u8; 5_000];
    prefixed.extend_from_slice(&bytes);
    assert!((mp3::probe_duration(&prefixed).expect("probe") - clean).abs() < 1e-9);
}

#[test]
fn an_id3v2_tag_does_not_hide_the_stream() {
    let audio_bytes = stream(&FrameSpec::silence(MPEG1, 0), 4);
    let mut file = vec![0u8; 10];
    file[0..3].copy_from_slice(b"ID3");
    file[3] = 3;
    let payload: Vec<u8> = b"TIT2\0\0\0\x05\0\0\0Test".to_vec();
    let size = payload.len();
    file[6] = ((size >> 21) & 0x7f) as u8;
    file[7] = ((size >> 14) & 0x7f) as u8;
    file[8] = ((size >> 7) & 0x7f) as u8;
    file[9] = (size & 0x7f) as u8;
    file.extend_from_slice(&payload);
    file.extend_from_slice(&audio_bytes);
    // ...and an ID3v1 tag on the end.
    file.extend_from_slice(b"TAG");
    file.extend_from_slice(&[0u8; 125]);

    assert_eq!(makepad_audio_decode::sniff(&file), Some(AudioFormat::Mp3));
    let audio = decode_any(&file).expect("decodes through the entry point");
    assert_eq!(audio.rate, 44_100);
    assert_eq!(mp3::read_tags(&file).title.as_deref(), Some("Test"));
}

#[test]
fn streaming_frames_concatenate_to_the_whole_decode() {
    let bytes = stream(&one_line_frame(210, false), 8);
    let whole = mp3::decode_all(&bytes).expect("whole");
    let mut decoder = mp3::Mp3Decoder::new(&bytes).expect("syncs");
    let mut streamed = Vec::new();
    while let Some(frame) = decoder.next_frame().expect("frame") {
        streamed.extend_from_slice(frame.pcm);
    }
    // decode_all also applies the tail trim, which this stream does not have.
    assert_eq!(streamed, whole.pcm_interleaved_f32);
}

#[test]
fn free_format_and_other_layers_are_refused_by_name() {
    // Free format: the frame length is not derivable, so there is nothing to
    // sync to and the file is not recognised as MP3 at all.
    let mut spec = FrameSpec::silence(MPEG1, 0);
    spec.bitrate_index = 9;
    let good = stream(&spec, 4);
    assert!(mp3::decode_all(&good).is_ok());
    let mut free = good.clone();
    for frame in free.chunks_mut(spec.frame_bytes()) {
        frame[2] &= 0x0f; // bitrate index 0
    }
    assert!(matches!(mp3::decode_all(&free), Err(AudioError::Empty)));
    assert!(makepad_audio_decode::sniff(&free).is_none());

    let mut layer2 = good.clone();
    for frame in layer2.chunks_mut(spec.frame_bytes()) {
        frame[1] = (frame[1] & !0x06) | 0x04; // layer II
    }
    assert!(matches!(mp3::decode_all(&layer2), Err(AudioError::Empty)));
}

#[test]
fn damaged_frames_keep_the_timeline_exact() {
    // A decoder that drops an undecodable frame shortens the track and, worse,
    // leaves the bit reservoir describing bytes that no longer belong to the
    // frames after it -- so everything downstream decodes from misaligned main
    // data. Damage must cost silence, not time.
    let spec = one_line_frame(196, false);
    let frame_bytes = spec.frame_bytes();
    let clean = stream(&spec, 8);
    let expected = decode(&clean, false).frames();
    assert_eq!(expected, 8 * 1152);

    // `big_values` past its legal maximum: one flipped bit in the side info.
    let mut broken = clean.clone();
    broken[3 * frame_bytes + 8] = 0xff;
    assert_eq!(decode(&broken, false).frames(), expected, "side info");

    // An undefined Huffman table (4) selected for region 0.
    let mut undefined = spec;
    undefined.granules[0][0].table0 = 4;
    let stream_with_bad_table = stream(&undefined, 8);
    assert_eq!(decode(&stream_with_bad_table, false).frames(), expected, "table 4");

    // Truncating a frame's main data mid-granule.
    let mut short = clean.clone();
    short.drain(5 * frame_bytes + 40..5 * frame_bytes + 60);
    let got = decode(&short, false).frames();
    assert!(got <= expected, "truncation grew the stream: {got} vs {expected}");
}

#[test]
fn limits_are_enforced() {
    let bytes = stream(&FrameSpec::silence(MPEG1, 0), 20);
    let err = makepad_audio_decode::decode_audio_limited(
        &bytes,
        AudioFormat::Mp3,
        Limits::with_max_frames(1000),
    );
    assert!(matches!(err, Err(AudioError::TooLarge(_))), "{err:?}");
}

// ---------------------------------------------------------------------------
// nothing panics
// ---------------------------------------------------------------------------

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
}

/// Kept small so the default `cargo test` stays fast; raise it with
/// `MAKEPAD_MP3_FUZZ=20000` when the decoder changes.
fn fuzz_iterations() -> usize {
    std::env::var("MAKEPAD_MP3_FUZZ").ok().and_then(|v| v.parse().ok()).unwrap_or(200)
}

/// Every entry point, on bytes that may be nonsense. The only contract is that
/// it returns.
fn exercise(bytes: &[u8]) {
    let _ = makepad_audio_decode::sniff(bytes);
    let _ = mp3::probe_duration(bytes);
    let _ = mp3::read_tags(bytes);
    let _ = mp3::decode_all_limited(bytes, Limits::with_max_frames(200_000));
    if let Ok(mut decoder) = mp3::Mp3Decoder::with_limits(bytes, Limits::with_max_frames(200_000)) {
        let mut guard = 0;
        while let Ok(Some(_)) = decoder.next_frame() {
            guard += 1;
            if guard > 5_000 {
                break;
            }
        }
    }
}

fn fuzz_corpus() -> Vec<u8> {
    // A stream with real coded content, so the mangling reaches the Huffman
    // and requantization paths rather than stopping at the header.
    let mut bytes = stream(&one_line_frame(210, false), 12);
    bytes.extend_from_slice(&stream(&FrameSpec::silence(MPEG2, 3), 6));
    bytes
}

#[test]
fn truncation_at_every_sixteenth_never_panics() {
    let bytes = fuzz_corpus();
    for i in 0..=16 {
        exercise(&bytes[..bytes.len() * i / 16]);
    }
}

#[test]
fn flipped_bytes_never_panic() {
    let bytes = fuzz_corpus();
    let mut rng = Lcg(0x5eed_1234);
    for _ in 0..fuzz_iterations() {
        let mut copy = bytes.clone();
        let flips = 1 + (rng.next() % 8) as usize;
        for _ in 0..flips {
            let at = (rng.next() as usize) % copy.len();
            copy[at] ^= (rng.next() % 256) as u8;
        }
        exercise(&copy);
    }
}

#[test]
fn mangled_headers_never_panic() {
    let bytes = fuzz_corpus();
    // Walk a corruption across the first frame's header and side info, where
    // every field feeds a table index or a length.
    for at in 0..64.min(bytes.len()) {
        for value in [0x00u8, 0xff, 0xaa, 0x55] {
            let mut copy = bytes.clone();
            copy[at] = value;
            exercise(&copy);
        }
    }
    exercise(&[]);
    exercise(&[0xff; 8]);
    exercise(&[0xff, 0xfb, 0x90, 0x04]);
    exercise(&vec![0xff; 4096]);
}

// ---------------------------------------------------------------------------
// the CoreAudio oracle (opt-in)
// ---------------------------------------------------------------------------

fn read_wav(bytes: &[u8]) -> (u32, u16, Vec<f32>) {
    assert!(bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE");
    let (mut rate, mut channels, mut bits, mut format) = (0u32, 0u16, 0u16, 0u16);
    let mut data: &[u8] = &[];
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let end = (at + 8 + size).min(bytes.len());
        let body = &bytes[at + 8..end];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = body,
            _ => {}
        }
        at = end + (size & 1);
    }
    let pcm = match (format, bits) {
        (3, 32) => data
            .chunks_exact(4)
            .map(|p| f32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            .collect(),
        (1, 16) => data
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
            .collect(),
        other => panic!("reference wav format {other:?} is not supported"),
    };
    (rate, channels, pcm)
}

/// Quantise to the 16-bit grid CoreAudio's decoder actually produces.
fn as_oracle_sees_it(pcm: &[f32]) -> Vec<f32> {
    pcm.iter()
        .map(|v| (v.clamp(-1.0, 32_767.0 / 32_768.0) * 32_768.0).round() / 32_768.0)
        .collect()
}

fn best_offset(reference: &[f32], got: &[f32], channels: usize, span: isize) -> isize {
    let take = 200_000usize.min(reference.len() / channels).min(got.len() / channels);
    let (mut best, mut best_at) = (f64::NEG_INFINITY, 0isize);
    for offset in -span..=span {
        let (mut num, mut da, mut db) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..take {
            let j = i as isize + offset;
            if j < 0 || j as usize >= take {
                continue;
            }
            let x = reference[i * channels] as f64;
            let y = got[j as usize * channels] as f64;
            num += x * y;
            da += x * x;
            db += y * y;
        }
        if da > 0.0 && db > 0.0 {
            let score = num / (da.sqrt() * db.sqrt());
            if score > best {
                best = score;
                best_at = offset;
            }
        }
    }
    best_at
}

#[test]
fn oracle_matches_coreaudio() {
    let Ok(dir) = std::env::var("MAKEPAD_MP3_ORACLE") else {
        eprintln!("MAKEPAD_MP3_ORACLE not set; skipping the CoreAudio comparison");
        return;
    };
    let gate: f64 = std::env::var("MAKEPAD_MP3_SNR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(85.0);
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("oracle directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "mp3"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "no .mp3 files in {dir}");

    let mut checked = 0usize;
    for path in entries {
        let wav = path.with_extension("wav");
        let Ok(reference) = std::fs::read(&wav) else {
            eprintln!("no reference for {}", path.display());
            continue;
        };
        let bytes = std::fs::read(&path).expect("mp3");
        let got = mp3::decode_all(&bytes).expect("decode");
        let (rate, channels, reference) = read_wav(&reference);
        assert_eq!(rate, got.rate, "{}", path.display());
        assert_eq!(channels, got.channels, "{}", path.display());
        let ch = channels as usize;
        let matched = as_oracle_sees_it(&got.pcm_interleaved_f32);
        let offset = best_offset(&reference, &matched, ch, 8);
        assert_eq!(offset, 0, "{} is not sample-aligned", path.display());

        let (mut signal, mut noise, mut exact, mut within, mut worst, mut n) =
            (0.0f64, 0.0f64, 0u64, 0u64, 0.0f64, 0u64);
        let frames = (reference.len() / ch).min(matched.len() / ch);
        for i in 0..frames {
            for c in 0..ch {
                let x = reference[i * ch + c] as f64;
                let y = matched[i * ch + c] as f64;
                signal += x * x;
                noise += (x - y) * (x - y);
                let lsb = ((x - y) * 32_768.0).abs();
                worst = worst.max(lsb);
                exact += u64::from(lsb < 0.25);
                within += u64::from(lsb < 1.25);
                n += 1;
            }
        }
        let snr = 10.0 * (signal / noise.max(f64::MIN_POSITIVE)).log10();
        // The oracle's own length: gapless trimming must land on it exactly,
        // give or take the 529 samples it keeps when a stream has no LAME tag.
        let delta = reference.len() as i64 / ch as i64 - got.frames() as i64;
        eprintln!(
            "{:<48} {snr:6.2} dB  exact {:5.2}%  <=1LSB {:8.5}%  worst {worst:3.0} LSB  frames {delta:+}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            100.0 * exact as f64 / n as f64,
            100.0 * within as f64 / n as f64,
        );
        assert!(snr >= gate, "{}: SNR {snr:.2} dB below the {gate} dB gate", path.display());
        assert!(worst <= 2.0, "{}: {worst} LSB is more than a rounding step", path.display());
        assert!(
            (0..=529).contains(&delta),
            "{}: frame count differs by {delta}",
            path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no mp3/wav pairs were compared");
    eprintln!("compared {checked} files against CoreAudio");
}
