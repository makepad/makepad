//! Vorbis I decoder.
//!
//! Adapted from the same author's decoder in the sandbox repo
//! (`apps/sandbox/libs/audio/src/vorbis/*`), re-homed here so the non-sandbox
//! apps can use it: same transform and bitstream reading, rebuilt around this
//! crate's [`DecodedAudio`]/[`Limits`] contract and around a streaming
//! [`VorbisDecoder`] whose per-block loop allocates nothing.
//!
//! Scope is what real files need: floor 1, residue 0/1/2, square-polar
//! coupling, mixed block sizes. Floor 0 reports rather than guesses.
//!
//! Like the rest of the crate this is total on malformed input — it reads
//! downloaded files, so a corrupt track must degrade to a short track or a
//! clean error, never a panic.

pub mod bits;
pub mod codebook;
pub mod floor;
pub mod mdct;
pub mod residue;

use crate::error::AudioError;
use crate::ogg::{self, PacketReader};
use crate::{DecodedAudio, Limits, Tags};
use bits::{ilog, BitReader};
use codebook::Codebook;
use floor::Floor;
use mdct::Mdct;
use residue::{Residue, ResidueScratch};

/// Largest block size the spec allows.
const MAX_BLOCKSIZE: usize = 8192;
/// Highest sample rate accepted; well past anything an encoder produces.
const MAX_RATE: u32 = 384_000;
/// Slots (codebook entries plus VQ table floats) one setup header may claim.
/// Generous next to real files — the largest ship a few hundred thousand — and
/// finite, so a header cannot name a gigabyte of codebook it does not contain.
const SETUP_BUDGET: usize = 1 << 22;

// -- headers ---------------------------------------------------------------

struct Ident {
    channels: usize,
    sample_rate: u32,
    blocksize_0: usize,
    blocksize_1: usize,
}

struct Mapping {
    coupling: Vec<(usize, usize)>,
    mux: Vec<usize>,
    submap_floor: Vec<usize>,
    submap_residue: Vec<usize>,
}

struct Mode {
    blockflag: bool,
    mapping: usize,
}

struct Setup {
    codebooks: Vec<Codebook>,
    floors: Vec<Floor>,
    residues: Vec<Residue>,
    mappings: Vec<Mapping>,
    modes: Vec<Mode>,
}

fn read_ident(packet: &[u8], limits: &Limits) -> Result<Ident, AudioError> {
    if packet.len() < 30 || packet[0] != 1 || &packet[1..7] != b"vorbis" {
        return Err(AudioError::BadHeader("vorbis identification"));
    }
    let mut r = BitReader::new(&packet[7..]);
    let version = r.read(32).ok_or(AudioError::Truncated)?;
    if version != 0 {
        return Err(AudioError::Unsupported("vorbis version"));
    }
    let channels = r.read(8).ok_or(AudioError::Truncated)? as usize;
    let sample_rate = r.read(32).ok_or(AudioError::Truncated)?;
    let _bitrate_max = r.read(32).ok_or(AudioError::Truncated)?;
    let _bitrate_nom = r.read(32).ok_or(AudioError::Truncated)?;
    let _bitrate_min = r.read(32).ok_or(AudioError::Truncated)?;
    let bs0 = r.read(4).ok_or(AudioError::Truncated)?;
    let bs1 = r.read(4).ok_or(AudioError::Truncated)?;
    let framing = r.read_bit().ok_or(AudioError::Truncated)?;
    if !framing {
        return Err(AudioError::BadHeader("vorbis framing bit"));
    }
    let blocksize_0 = 1usize << bs0.min(20);
    let blocksize_1 = 1usize << bs1.min(20);
    if channels == 0 {
        return Err(AudioError::BadHeader("vorbis channel count"));
    }
    if channels > limits.max_channels {
        return Err(AudioError::TooLarge("vorbis channel count"));
    }
    if sample_rate == 0 || sample_rate > MAX_RATE {
        return Err(AudioError::BadHeader("vorbis sample rate"));
    }
    if !(64..=MAX_BLOCKSIZE).contains(&blocksize_0)
        || !(64..=MAX_BLOCKSIZE).contains(&blocksize_1)
        || blocksize_0 > blocksize_1
    {
        return Err(AudioError::BadHeader("vorbis block size"));
    }
    Ok(Ident { channels, sample_rate, blocksize_0, blocksize_1 })
}

/// Vorbis comments: a vendor string then `KEY=value` pairs.
fn read_comments(packet: &[u8], tags: &mut Tags) -> Result<(), AudioError> {
    if packet.len() < 7 || packet[0] != 3 || &packet[1..7] != b"vorbis" {
        return Err(AudioError::BadHeader("vorbis comment"));
    }
    let mut at = 7usize;
    let u32_at = |at: &mut usize| -> Option<usize> {
        let end = at.checked_add(4)?;
        let v = u32::from_le_bytes(packet.get(*at..end)?.try_into().ok()?);
        *at = end;
        Some(v as usize)
    };
    let vendor_len = u32_at(&mut at).ok_or(AudioError::Truncated)?;
    at = at.checked_add(vendor_len).ok_or(AudioError::Truncated)?;
    if at > packet.len() {
        return Err(AudioError::Truncated);
    }
    let count = u32_at(&mut at).ok_or(AudioError::Truncated)?;
    // Each comment costs at least its four length bytes, so a lying count
    // cannot make this loop long.
    let count = count.min(packet.len().saturating_sub(at) / 4);
    for _ in 0..count {
        let len = match u32_at(&mut at) {
            Some(len) => len,
            None => break,
        };
        let end = match at.checked_add(len) {
            Some(end) if end <= packet.len() => end,
            _ => break,
        };
        let text = String::from_utf8_lossy(&packet[at..end]);
        at = end;
        if let Some((key, value)) = text.split_once('=') {
            tags.push(key, value);
        }
    }
    Ok(())
}

fn read_setup(packet: &[u8], channels: usize) -> Result<Setup, AudioError> {
    if packet.len() < 7 || packet[0] != 5 || &packet[1..7] != b"vorbis" {
        return Err(AudioError::BadHeader("vorbis setup"));
    }
    let mut r = BitReader::new(&packet[7..]);
    let mut budget = SETUP_BUDGET;

    let codebook_count = r.read(8).ok_or(AudioError::Truncated)? as usize + 1;
    let mut codebooks = Vec::with_capacity(codebook_count);
    for _ in 0..codebook_count {
        codebooks.push(Codebook::read(&mut r, &mut budget)?);
    }

    // Time-domain transforms: placeholders that must be zero.
    let time_count = r.read(6).ok_or(AudioError::Truncated)? as usize + 1;
    for _ in 0..time_count {
        if r.read(16).ok_or(AudioError::Truncated)? != 0 {
            return Err(AudioError::Corrupt("vorbis time transform"));
        }
    }

    let floor_count = r.read(6).ok_or(AudioError::Truncated)? as usize + 1;
    let mut floors = Vec::with_capacity(floor_count);
    for _ in 0..floor_count {
        floors.push(Floor::read(&mut r, codebooks.len())?);
    }

    let residue_count = r.read(6).ok_or(AudioError::Truncated)? as usize + 1;
    let mut residues = Vec::with_capacity(residue_count);
    for _ in 0..residue_count {
        residues.push(Residue::read(&mut r, codebooks.len())?);
    }

    let mapping_count = r.read(6).ok_or(AudioError::Truncated)? as usize + 1;
    let mut mappings = Vec::with_capacity(mapping_count);
    for _ in 0..mapping_count {
        mappings.push(read_mapping(&mut r, channels, floors.len(), residues.len())?);
    }

    let mode_count = r.read(6).ok_or(AudioError::Truncated)? as usize + 1;
    let mut modes = Vec::with_capacity(mode_count);
    for _ in 0..mode_count {
        let blockflag = r.read_bit().ok_or(AudioError::Truncated)?;
        let _windowtype = r.read(16).ok_or(AudioError::Truncated)?;
        let _transformtype = r.read(16).ok_or(AudioError::Truncated)?;
        let mapping = r.read(8).ok_or(AudioError::Truncated)? as usize;
        if mapping >= mappings.len() {
            return Err(AudioError::Corrupt("vorbis mode mapping"));
        }
        modes.push(Mode { blockflag, mapping });
    }
    if !r.read_bit().ok_or(AudioError::Truncated)? {
        return Err(AudioError::Corrupt("vorbis setup framing"));
    }

    Ok(Setup { codebooks, floors, residues, mappings, modes })
}

fn read_mapping(
    r: &mut BitReader,
    channels: usize,
    floor_count: usize,
    residue_count: usize,
) -> Result<Mapping, AudioError> {
    let kind = r.read(16).ok_or(AudioError::Truncated)?;
    if kind != 0 {
        return Err(AudioError::Corrupt("vorbis mapping type"));
    }
    let submaps = if r.read_bit().ok_or(AudioError::Truncated)? {
        r.read(4).ok_or(AudioError::Truncated)? as usize + 1
    } else {
        1
    };
    let mut coupling = Vec::new();
    if r.read_bit().ok_or(AudioError::Truncated)? {
        let steps = r.read(8).ok_or(AudioError::Truncated)? as usize + 1;
        let bits = ilog(channels as i32 - 1);
        for _ in 0..steps {
            let magnitude = r.read(bits).ok_or(AudioError::Truncated)? as usize;
            let angle = r.read(bits).ok_or(AudioError::Truncated)? as usize;
            if magnitude == angle || magnitude >= channels || angle >= channels {
                return Err(AudioError::Corrupt("vorbis coupling"));
            }
            coupling.push((magnitude, angle));
        }
    }
    if r.read(2).ok_or(AudioError::Truncated)? != 0 {
        return Err(AudioError::Corrupt("vorbis mapping reserved"));
    }
    let mut mux = vec![0usize; channels];
    if submaps > 1 {
        for m in mux.iter_mut() {
            let v = r.read(4).ok_or(AudioError::Truncated)? as usize;
            if v >= submaps {
                return Err(AudioError::Corrupt("vorbis submap"));
            }
            *m = v;
        }
    }
    let mut submap_floor = Vec::with_capacity(submaps);
    let mut submap_residue = Vec::with_capacity(submaps);
    for _ in 0..submaps {
        let _unused = r.read(8).ok_or(AudioError::Truncated)?;
        let f = r.read(8).ok_or(AudioError::Truncated)? as usize;
        let rs = r.read(8).ok_or(AudioError::Truncated)? as usize;
        if f >= floor_count || rs >= residue_count {
            return Err(AudioError::Corrupt("vorbis submap index"));
        }
        submap_floor.push(f);
        submap_residue.push(rs);
    }
    Ok(Mapping { coupling, mux, submap_floor, submap_residue })
}

// -- windows ---------------------------------------------------------------

/// The window for one block, accounting for long/short neighbours. The overlap
/// with either neighbour is only as wide as the shorter of the two blocks, so a
/// long block next to a short one has a narrow slope and a flat shoulder.
fn lapped_window(n: usize, short_n: usize, prev_long: bool, next_long: bool) -> Vec<f32> {
    let left_n = if prev_long { n } else { short_n };
    let right_n = if next_long { n } else { short_n };
    let mut w = vec![0.0f32; n];

    let left_begin = n / 4 - left_n / 4;
    let left_end = left_begin + left_n / 2;
    let right_begin = (n * 3) / 4 - right_n / 4;
    let right_end = right_begin + right_n / 2;

    let ls = mdct::window(left_n);
    for i in 0..left_n / 2 {
        if left_begin + i < n {
            w[left_begin + i] = ls[i];
        }
    }
    for slot in w.iter_mut().take(right_begin.min(n)).skip(left_end) {
        *slot = 1.0;
    }
    let rs = mdct::window(right_n);
    for i in 0..right_n / 2 {
        let idx = right_begin + i;
        if idx < n {
            w[idx] = rs[right_n / 2 + i];
        }
    }
    for slot in w.iter_mut().take(n).skip(right_end) {
        *slot = 0.0;
    }
    w
}

// -- the decoder -----------------------------------------------------------

/// Progressive Vorbis decode: one overlap-added block at a time into a
/// caller-visible internal buffer.
pub struct VorbisDecoder<'a> {
    reader: PacketReader<'a>,
    core: Core,
    tags: Tags,
}

/// Everything that is not the container reader, kept apart from it so a packet
/// can borrow the reader while the decoder writes its own buffers.
struct Core {
    ident: Ident,
    setup: Setup,

    /// Windows, indexed `(prev_long as usize) | (next_long as usize) << 1`;
    /// short blocks always lap over `blocksize_0` and use `win_short`.
    win_long: [Vec<f32>; 4],
    win_short: Vec<f32>,
    mdct_long: Mdct,
    mdct_short: Mdct,

    // per-packet scratch, all sized from the headers
    floor_y: Vec<i32>,
    floor_stride: usize,
    curves: Vec<f32>,
    spectra: Vec<f32>,
    time: Vec<f32>,
    /// Half of `blocksize_1`: the stride of `curves` and `spectra`.
    half_stride: usize,
    /// `blocksize_1`: the stride of `time`.
    time_stride: usize,
    /// Per channel: the floor said "nothing here" for this packet.
    silent: Vec<bool>,
    /// Per channel: no residue is coded for it either, after coupling has had
    /// its say.
    no_residue: Vec<bool>,
    members: Vec<usize>,
    skip: Vec<bool>,
    res_scratch: ResidueScratch,

    // overlap-add state, all positions on one timeline of PCM samples
    acc: Vec<f32>,
    acc_stride: usize,
    /// Timeline position of `acc[.., 0]`.
    base: u64,
    /// Window centre of the block just added.
    center: u64,
    prev_n: usize,
    have_prev: bool,
    /// Timeline position where valid PCM starts.
    front_trim: Option<u64>,
    /// Frames the stream claims, from the last page's granule.
    total: Option<u64>,
    emitted: u64,
    finished: bool,

    /// Interleaved output handed to the caller.
    out: Vec<f32>,
}

impl<'a> VorbisDecoder<'a> {
    /// Parse the three Vorbis headers. No audio is decoded.
    pub fn new(bytes: &'a [u8]) -> Result<Self, AudioError> {
        Self::with_limits(bytes, Limits::default())
    }

    pub fn with_limits(bytes: &'a [u8], limits: Limits) -> Result<Self, AudioError> {
        let mut reader = PacketReader::new(bytes);
        let mut tags = Tags::default();

        let ident = {
            let packet = reader.next_packet()?.ok_or(AudioError::Empty)?;
            read_ident(packet.data, &limits)?
        };
        let mut setup: Option<Setup> = None;
        {
            let packet = reader.next_packet()?.ok_or(AudioError::Truncated)?;
            match packet.data.first() {
                Some(3) => read_comments(packet.data, &mut tags)?,
                Some(5) => setup = Some(read_setup(packet.data, ident.channels)?),
                _ => return Err(AudioError::BadHeader("vorbis header order")),
            }
        }
        if setup.is_none() {
            let packet = reader.next_packet()?.ok_or(AudioError::Truncated)?;
            setup = Some(read_setup(packet.data, ident.channels)?);
        }
        let setup = setup.ok_or(AudioError::Truncated)?;

        // The last page's granule is the stream's exact length in frames, and
        // the first page that carries one pins the encoder delay to discard
        // from the front. Both are read from the container, not the audio.
        let total = ogg::last_page(bytes, reader.serial())
            .map(|p| p.granule)
            .filter(|&g| g != ogg::GRANULE_NONE);
        let front_trim = scan_front_trim(&reader, &ident, &setup);

        let core = Core::new(ident, setup, total, front_trim);
        Ok(VorbisDecoder { reader, core, tags })
    }

    pub fn rate(&self) -> u32 {
        self.core.ident.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.core.ident.channels as u16
    }

    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    /// Frames the stream claims to hold, from the last page's granule position.
    pub fn total_frames(&self) -> Option<u64> {
        self.core.total
    }

    /// Next overlap-added block of interleaved samples, or `None` at the end of
    /// the stream. The slice borrows an internal buffer, so it must be dropped
    /// before the next call; the loop allocates nothing.
    pub fn next_block(&mut self) -> Result<Option<&[f32]>, AudioError> {
        let core = &mut self.core;
        let reader = &mut self.reader;
        let mut count;
        loop {
            if core.finished {
                return Ok(None);
            }
            let packet = match reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    core.finished = true;
                    count = core.flush();
                    break;
                }
                Err(e) => {
                    // A container-level failure ends playback where it is
                    // rather than losing the whole track.
                    core.finished = true;
                    if core.emitted == 0 {
                        return Err(e);
                    }
                    count = core.flush();
                    break;
                }
            };
            let end_of_stream = packet.end_of_stream;
            match core.decode_block(packet.data) {
                Ok(true) => {}
                // Not an audio packet: skip it.
                Ok(false) => continue,
                Err(e) => {
                    // A bad packet mid-stream truncates playback rather than
                    // losing the whole track: partial audio beats silence.
                    core.finished = true;
                    if core.emitted == 0 && core.center == 0 {
                        return Err(e);
                    }
                    count = core.flush();
                    break;
                }
            }
            count = core.emit();
            if end_of_stream {
                core.finished = true;
                count += core.flush_into(count);
            }
            if count > 0 || core.finished {
                break;
            }
        }
        if count == 0 {
            return Ok(None);
        }
        Ok(Some(&core.out[..count]))
    }
}

impl Core {
    fn new(ident: Ident, setup: Setup, total: Option<u64>, front_trim: Option<u64>) -> Core {
        let ch = ident.channels;
        let half_stride = ident.blocksize_1 / 2;
        let time_stride = ident.blocksize_1;
        let acc_stride = ident.blocksize_1 * 2;
        let floor_stride = floor::MAX_POINTS;
        let win_long = [
            lapped_window(ident.blocksize_1, ident.blocksize_0, false, false),
            lapped_window(ident.blocksize_1, ident.blocksize_0, true, false),
            lapped_window(ident.blocksize_1, ident.blocksize_0, false, true),
            lapped_window(ident.blocksize_1, ident.blocksize_0, true, true),
        ];
        let win_short = lapped_window(ident.blocksize_0, ident.blocksize_0, true, true);
        let mut res_scratch = ResidueScratch::default();
        let (mut classes, mut flat) = (0usize, 0usize);
        for r in &setup.residues {
            let (c, f) = r.scratch_needed(half_stride, ch, &setup.codebooks);
            classes = classes.max(c);
            flat = flat.max(f);
        }
        res_scratch.reserve(classes, flat);
        Core {
            win_long,
            win_short,
            mdct_long: Mdct::new(ident.blocksize_1),
            mdct_short: Mdct::new(ident.blocksize_0),
            floor_y: vec![0; floor_stride * ch],
            floor_stride,
            curves: vec![0.0; half_stride * ch],
            spectra: vec![0.0; half_stride * ch],
            time: vec![0.0; time_stride * ch],
            half_stride,
            time_stride,
            silent: vec![false; ch],
            no_residue: vec![false; ch],
            members: Vec::with_capacity(ch),
            skip: Vec::with_capacity(ch),
            res_scratch,
            acc: vec![0.0; acc_stride * ch],
            acc_stride,
            base: 0,
            center: 0,
            prev_n: 0,
            have_prev: false,
            front_trim,
            total,
            emitted: 0,
            finished: false,
            out: vec![0.0; acc_stride * ch],
            ident,
            setup,
        }
    }

    /// Decode one packet into `self.time` and lap it into the accumulator.
    /// `false` means "not an audio packet".
    fn decode_block(&mut self, packet: &[u8]) -> Result<bool, AudioError> {
        let Some(n) = self.decode_spectra(packet)? else {
            return Ok(false);
        };
        // Block centres are spaced by the average of the two half-sizes, which
        // is what makes mixed long/short blocks line up.
        self.center = if self.have_prev {
            self.center + ((self.prev_n + n) / 4) as u64
        } else {
            (n / 2) as u64
        };
        self.have_prev = true;
        self.prev_n = n;
        if self.front_trim.is_none() {
            // No page reported a granule, so nothing says samples were trimmed:
            // valid PCM starts at the first block's centre.
            self.front_trim = Some(self.center);
        }
        self.lap(n);
        Ok(true)
    }

    /// Floor, residue, coupling, IMDCT and window for one packet, leaving
    /// `channels * n` windowed samples in `self.time`. `None` for a packet that
    /// is not audio.
    fn decode_spectra(&mut self, packet: &[u8]) -> Result<Option<usize>, AudioError> {
        let mut r = BitReader::new(packet);
        if r.read(1).ok_or(AudioError::Truncated)? != 0 {
            return Ok(None);
        }
        let mode_bits = ilog(self.setup.modes.len() as i32 - 1);
        let mode_number = r.read(mode_bits).ok_or(AudioError::Truncated)? as usize;
        let mode = self.setup.modes.get(mode_number).ok_or(AudioError::Corrupt("vorbis mode"))?;
        let blockflag = mode.blockflag;
        let n = if blockflag { self.ident.blocksize_1 } else { self.ident.blocksize_0 };
        let (prev_flag, next_flag) = if blockflag {
            (r.read_bit().ok_or(AudioError::Truncated)?, r.read_bit().ok_or(AudioError::Truncated)?)
        } else {
            (true, true)
        };
        let mapping = self
            .setup
            .mappings
            .get(mode.mapping)
            .ok_or(AudioError::Corrupt("vorbis mode mapping"))?;
        let half = n / 2;
        let ch = self.ident.channels;

        // --- floor ---
        self.curves[..half * ch].fill(0.0);
        for c in 0..ch {
            let submap = mapping.mux[c];
            let fi = mapping.submap_floor[submap];
            let Floor::Type1(f1) = self.setup.floors.get(fi).ok_or(AudioError::Corrupt("floor"))?;
            let y = &mut self.floor_y[c * self.floor_stride..(c + 1) * self.floor_stride];
            self.silent[c] = !f1.decode(&mut r, &self.setup.codebooks, y)?;
            self.no_residue[c] = self.silent[c];
        }
        // Coupling dirties which channels take part in the residue — a silent
        // channel still carries its partner's angle — but NOT which channels
        // are silent: a channel whose floor said nothing is zeroed at the end
        // whatever its partner did. Conflating the two lets a stale floor curve
        // multiply a coupled residue and put a quiet burst where the reference
        // decoder has exact silence.
        for &(m, a) in &mapping.coupling {
            if !self.no_residue[m] || !self.no_residue[a] {
                self.no_residue[m] = false;
                self.no_residue[a] = false;
            }
        }

        // --- residue, per submap ---
        self.spectra[..self.half_stride * ch].fill(0.0);
        for s in 0..mapping.submap_residue.len() {
            self.members.clear();
            self.skip.clear();
            for c in 0..ch {
                if mapping.mux[c] == s {
                    self.members.push(c);
                    self.skip.push(self.no_residue[c]);
                }
            }
            if self.members.is_empty() {
                continue;
            }
            let ri = mapping.submap_residue[s];
            let res = self.setup.residues.get(ri).ok_or(AudioError::Corrupt("residue"))?;
            res.decode(
                &mut r,
                &self.setup.codebooks,
                &mut self.spectra,
                self.half_stride,
                &self.members,
                &self.skip,
                half,
                &mut self.res_scratch,
            )?;
        }

        // --- inverse coupling, in reverse order ---
        for &(m, a) in mapping.coupling.iter().rev() {
            let (mb, ab) = (m * self.half_stride, a * self.half_stride);
            for i in 0..half {
                let mag = self.spectra[mb + i];
                let ang = self.spectra[ab + i];
                let (new_m, new_a) = if mag > 0.0 {
                    if ang > 0.0 {
                        (mag, mag - ang)
                    } else {
                        (mag + ang, mag)
                    }
                } else if ang > 0.0 {
                    (mag, mag + ang)
                } else {
                    (mag - ang, mag)
                };
                self.spectra[mb + i] = new_m;
                self.spectra[ab + i] = new_a;
            }
        }

        // --- floor curves, floor multiply, IMDCT, window ---
        for c in 0..ch {
            let base = c * self.half_stride;
            if self.silent[c] {
                self.spectra[base..base + half].fill(0.0);
                continue;
            }
            let submap = mapping.mux[c];
            let fi = mapping.submap_floor[submap];
            let Floor::Type1(f1) = self.setup.floors.get(fi).ok_or(AudioError::Corrupt("floor"))?;
            let y = &self.floor_y[c * self.floor_stride..(c + 1) * self.floor_stride];
            f1.synthesize(y, &mut self.curves[base..base + half]);
            for i in 0..half {
                self.spectra[base + i] *= self.curves[base + i];
            }
        }

        let win = if blockflag {
            &self.win_long[(prev_flag as usize) | ((next_flag as usize) << 1)]
        } else {
            &self.win_short
        };
        let transform = if blockflag { &mut self.mdct_long } else { &mut self.mdct_short };
        for c in 0..ch {
            let sbase = c * self.half_stride;
            let tbase = c * self.time_stride;
            transform.imdct(
                &self.spectra[sbase..sbase + half],
                &mut self.time[tbase..tbase + n],
            );
            for (t, w) in self.time[tbase..tbase + n].iter_mut().zip(win.iter()) {
                *t *= *w;
            }
        }
        Ok(Some(n))
    }

    /// Add the block in `self.time` to the accumulator at its window centre.
    ///
    /// A block's window is centred on `center` and reaches `n/2` either side, so
    /// an early long block can begin before the start of the stream — a
    /// 2048-sample block centred at 832 starts at -192. Those leading samples
    /// lie outside the timeline and are dropped; sliding the block later
    /// instead would corrupt every sample until the centres grow past `n/2`,
    /// which is exactly why files opening `[256, 256, 2048, ...]` used to
    /// decode with a wrong head and a correct body. (The window is zero there
    /// in any case: the lap with the previous block never reaches that far.)
    fn lap(&mut self, n: usize) {
        let ch = self.ident.channels;
        let start = self.center as i64 - (n as i64) / 2;
        let off = start - self.base as i64;
        let skip = if off < 0 { (-off) as usize } else { 0 };
        if skip >= n {
            return;
        }
        let at = (off + skip as i64) as usize;
        let take = (n - skip).min(self.acc_stride.saturating_sub(at));
        for c in 0..ch {
            let src = &self.time[c * self.time_stride + skip..c * self.time_stride + skip + take];
            let dst = &mut self.acc[c * self.acc_stride + at..c * self.acc_stride + at + take];
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d += *s;
            }
        }
    }

    /// Hand out everything before the current block's centre, which no later
    /// block can touch, and slide the accumulator up. Returns the number of
    /// values written to `self.out`.
    fn emit(&mut self) -> usize {
        let upto = self.center;
        let count = self.take(upto);
        if let Some(total) = self.total {
            // The stream said how long it is; past that there is nothing to
            // decode, however many packets follow.
            if self.emitted >= total {
                self.finished = true;
            }
        }
        // Slide: the samples before `upto` are spent.
        let shift = (upto - self.base) as usize;
        if shift > 0 {
            let ch = self.ident.channels;
            for c in 0..ch {
                let base = c * self.acc_stride;
                if shift < self.acc_stride {
                    self.acc.copy_within(base + shift..base + self.acc_stride, base);
                }
                let keep = self.acc_stride.saturating_sub(shift);
                self.acc[base + keep..base + self.acc_stride].fill(0.0);
            }
            self.base = upto;
        }
        count
    }

    /// At the end of the stream the tail past the last centre is real audio up
    /// to the granule the final page claims.
    fn flush(&mut self) -> usize {
        self.flush_into(0)
    }

    fn flush_into(&mut self, at: usize) -> usize {
        let Some(total) = self.total else {
            return 0;
        };
        if total <= self.emitted {
            return 0;
        }
        let upto = self.base + self.acc_stride as u64;
        let upto = upto.min(self.center + (self.prev_n / 2) as u64);
        self.take_into(upto, at)
    }

    fn take(&mut self, upto: u64) -> usize {
        self.take_into(upto, 0)
    }

    /// Interleave `[max(base, front_trim), upto)` into `self.out` at `at`,
    /// stopping at the frame count the stream claims.
    fn take_into(&mut self, upto: u64, at: usize) -> usize {
        let trim = self.front_trim.unwrap_or(0);
        let from = self.base.max(trim);
        if upto <= from {
            return 0;
        }
        let mut frames = (upto - from) as usize;
        if let Some(total) = self.total {
            frames = frames.min(total.saturating_sub(self.emitted) as usize);
        }
        let ch = self.ident.channels;
        let offset = (from - self.base) as usize;
        if offset >= self.acc_stride {
            return 0;
        }
        frames = frames.min(self.acc_stride - offset);
        let need = at + frames * ch;
        if self.out.len() < need {
            self.out.resize(need, 0.0);
        }
        for i in 0..frames {
            for c in 0..ch {
                let v = self.acc[c * self.acc_stride + offset + i];
                // A corrupt stream must not put NaN or a huge spike into an
                // audio device.
                let v = if v.is_finite() { v.clamp(-4.0, 4.0) } else { 0.0 };
                self.out[at + i * ch + c] = v;
            }
        }
        self.emitted += frames as u64;
        frames * ch
    }
}

/// Walk the head of the stream for the first page that reports a granule, and
/// turn it into the timeline position where valid PCM starts.
///
/// Vorbis carries the encoder delay there: the page says how many samples
/// should have been output by its end, and whatever the decoder produced beyond
/// that is priming to discard from the front. It varies per file, so it cannot
/// be assumed — without it a stream plays shifted and opens with a burst of
/// priming garbage. This is the same arithmetic libvorbis does when it sets
/// `pcm_returned` to the first block's centre and then advances it by
/// `sample_count - granulepos`.
///
/// Never earlier than the first block's centre, which is where the overlap-add
/// first reconstructs anything: a stream whose granule positions do not start
/// at zero (a file cut mid-stream) would otherwise open with half a window.
///
/// Only packet headers are read; no audio is decoded.
fn scan_front_trim(reader: &PacketReader<'_>, ident: &Ident, setup: &Setup) -> Option<u64> {
    let mut probe = reader.clone();
    let mut center = 0u64;
    let mut first_center = 0u64;
    let mut prev_n = 0usize;
    let mut have = false;
    // A granule cannot be more than a handful of pages away; stop rather than
    // walk a whole file that never reports one.
    for _ in 0..4096 {
        let packet = probe.next_packet().ok()??;
        let granule = packet.granule;
        if let Some(n) = block_size(packet.data, ident, setup) {
            center = if have { center + ((prev_n + n) / 4) as u64 } else { (n / 2) as u64 };
            if !have {
                first_center = center;
            }
            prev_n = n;
            have = true;
            if let Some(g) = granule {
                return Some(center.saturating_sub(g).max(first_center));
            }
        }
    }
    None
}

/// Block size of an audio packet, from its mode number alone.
fn block_size(packet: &[u8], ident: &Ident, setup: &Setup) -> Option<usize> {
    let mut r = BitReader::new(packet);
    if r.read(1)? != 0 {
        return None;
    }
    let mode_bits = ilog(setup.modes.len() as i32 - 1);
    let mode = setup.modes.get(r.read(mode_bits)? as usize)?;
    Some(if mode.blockflag { ident.blocksize_1 } else { ident.blocksize_0 })
}

// -- whole-file API --------------------------------------------------------

/// Decode a whole Ogg Vorbis file.
pub fn decode_all(bytes: &[u8]) -> Result<DecodedAudio, AudioError> {
    decode_all_limited(bytes, Limits::default())
}

pub fn decode_all_limited(bytes: &[u8], limits: Limits) -> Result<DecodedAudio, AudioError> {
    let mut decoder = VorbisDecoder::with_limits(bytes, limits)?;
    let channels = decoder.channels();
    let rate = decoder.rate();
    let mut pcm: Vec<f32> = Vec::new();
    if let Some(total) = decoder.total_frames() {
        if total as usize > limits.max_frames {
            return Err(AudioError::TooLarge("vorbis frame count"));
        }
        // Reserve for what the stream claims, but never more than its bytes
        // could possibly decode to: a granule position is a header field, and a
        // small file must not be able to ask for gigabytes. 64 samples per input
        // byte is far past the lowest bitrate anything real is encoded at.
        let want = (total as usize).saturating_mul(channels as usize);
        pcm.reserve(want.min(bytes.len().saturating_mul(64)));
    }
    let cap = limits.max_frames.saturating_mul(channels as usize);
    while let Some(block) = decoder.next_block()? {
        if pcm.len() + block.len() > cap {
            return Err(AudioError::TooLarge("vorbis frame count"));
        }
        pcm.extend_from_slice(block);
    }
    if pcm.is_empty() {
        return Err(AudioError::Empty);
    }
    Ok(DecodedAudio { rate, channels, pcm_interleaved_f32: pcm })
}

/// Duration in seconds from the last page's granule position — the container's
/// own sample count, so this reads two pages of a 100 MB file rather than
/// decoding it.
pub fn probe_duration(bytes: &[u8]) -> Result<f64, AudioError> {
    let mut reader = PacketReader::new(bytes);
    let rate = {
        let packet = reader.next_packet()?.ok_or(AudioError::Empty)?;
        read_ident(packet.data, &Limits::default())?.sample_rate
    };
    let page = ogg::last_page(bytes, reader.serial()).ok_or(AudioError::Truncated)?;
    if page.granule == ogg::GRANULE_NONE {
        return Err(AudioError::Truncated);
    }
    Ok(page.granule as f64 / rate as f64)
}

/// Vorbis comments from the second header packet.
pub fn read_tags(bytes: &[u8]) -> Result<Tags, AudioError> {
    let mut reader = PacketReader::new(bytes);
    {
        let packet = reader.next_packet()?.ok_or(AudioError::Empty)?;
        read_ident(packet.data, &Limits::default())?;
    }
    let mut tags = Tags::default();
    let packet = reader.next_packet()?.ok_or(AudioError::Truncated)?;
    read_comments(packet.data, &mut tags)?;
    Ok(tags)
}

#[cfg(test)]
mod tests;
