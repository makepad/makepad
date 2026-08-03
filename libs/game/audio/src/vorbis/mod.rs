//! Vorbis I decoder.
//!
//! Written here because every Kenney audio pack ships Ogg Vorbis and nothing
//! else, and the repo takes no external dependencies. Scope is what those
//! files need: floor 1, residue 0/1/2, square-polar coupling, mixed block
//! sizes. Floor 0 reports rather than guesses.
//!
//! Like the other decoders this is total on malformed input — it reads
//! downloaded files, so a corrupt pack must degrade to a missing sound.

pub mod codebook;
pub mod floor;
pub mod mdct;
pub mod residue;

use crate::bitread::{ilog, BitReader};
use crate::{ogg, AudioError, Pcm};
use codebook::Codebook;
use floor::Floor;
use residue::Residue;

/// Refuse absurd streams rather than allocating for them.
const MAX_CHANNELS: usize = 8;
const MAX_SAMPLES_PER_CHANNEL: usize = 48_000 * 60 * 10; // ten minutes

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

struct Ident {
    channels: usize,
    sample_rate: u32,
    blocksize_0: usize,
    blocksize_1: usize,
}

/// Decode an Ogg Vorbis file to interleaved f32 PCM.
pub fn decode(bytes: &[u8]) -> Result<Pcm, AudioError> {
    let stream = ogg::read_packets(bytes)?;
    if stream.packets.len() < 3 {
        return Err(AudioError::Truncated);
    }

    let ident = read_ident(&stream.packets[0])?;
    // packets[1] is the comment header: nothing we need.
    let setup = read_setup(&stream.packets[2], ident.channels)?;

    let mut out: Vec<Vec<f32>> = vec![Vec::new(); ident.channels];
    // Overlap-add state. Block centres are spaced by the average of the two
    // block half-sizes, which is what makes mixed long/short blocks line up.
    let mut prev_center: Option<usize> = None;
    let mut prev_n = 0usize;
    // Where valid PCM starts. Two things move it:
    //
    // 1. The first audio packet produces no output — its window only primes the
    //    overlap-add — so audio begins one block centre in.
    // 2. Vorbis carries encoder delay in the granule position: the first page
    //    that reports a granule says how many samples SHOULD have been emitted
    //    by then, and any excess we decoded is priming to discard from the
    //    front. That excess varies per file (measured 128, 960 and 1103 on
    //    three Kenney sounds), so it cannot be assumed — without reading it the
    //    stream plays shifted and starts with a burst of priming garbage.
    let mut first_center: Option<usize> = None;
    let mut single_block_center = 0usize;
    // Packet index whose page first reports a real granule, and that granule.
    let first_granule_page = stream
        .page_ends
        .iter()
        .find(|(count, g)| *g != u64::MAX && *g > 0 && *count > 0)
        .map(|(count, g)| (count - 1, *g));
    let mut priming: Option<usize> = None;
    let mut packet_index = 2usize; // packets[0..=2] are the headers

    for packet in stream.packets.iter().skip(3) {
        packet_index += 1;
        if packet.is_empty() {
            continue;
        }
        let block = match decode_packet(packet, &ident, &setup) {
            Ok(Some(b)) => b,
            // A bad packet mid-stream truncates playback rather than losing
            // the whole sound: partial audio beats silence.
            Ok(None) => continue,
            Err(_) => break,
        };
        let n = block.n;
        let center = match prev_center {
            None => {
                // Fallback only: a stream with a single audio packet has no
                // valid PCM by the spec, but emitting its centre onward beats
                // returning silence for a malformed-but-decodable file.
                single_block_center = n / 2;
                n / 2
            }
            Some(c) => c + (prev_n + n) / 4,
        };
        if prev_center.is_some() && first_center.is_none() {
            first_center = Some(center);
        }
        // The page that first reports a granule pins the priming: whatever we
        // decoded beyond what that page claims was encoder delay.
        if priming.is_none() {
            if let Some((idx, g)) = first_granule_page {
                if packet_index >= idx {
                    priming = Some(center.saturating_sub(g as usize));
                }
            }
        }
        if !overlap_add(&mut out, &block.channels, center, n) {
            break;
        }
        prev_center = Some(center);
        prev_n = n;
    }

    // Valid PCM starts after the encoder's priming, plus the half-window the
    // first (output-less) packet occupies. Falls back to the second block's
    // centre when no page reported a usable granule.
    let first_center = match priming {
        Some(p) => p + ident.blocksize_0 / 2,
        None => first_center.unwrap_or(single_block_center),
    };
    let mut frames = out[0].len().saturating_sub(first_center);
    if stream.last_granule > 0 && (stream.last_granule as usize) < frames {
        frames = stream.last_granule as usize;
    }

    let mut samples = Vec::with_capacity(frames * ident.channels);
    for i in 0..frames {
        for ch in out.iter() {
            let v = ch.get(first_center + i).copied().unwrap_or(0.0);
            samples.push(if v.is_finite() { v.clamp(-4.0, 4.0) } else { 0.0 });
        }
    }

    Ok(Pcm {
        channels: ident.channels,
        sample_rate: ident.sample_rate,
        samples,
    })
}

/// Diagnostic: the raw overlap-added buffer before trimming, plus the
/// computed valid start and the stream granule. Used to derive the trim
/// rule from reference audio rather than guessing it. Not used by playback.
pub fn debug_raw(bytes: &[u8]) -> Result<(Vec<Vec<f32>>, usize, u64), AudioError> {
    let stream = ogg::read_packets(bytes)?;
    if stream.packets.len() < 3 {
        return Err(AudioError::Truncated);
    }
    let ident = read_ident(&stream.packets[0])?;
    let setup = read_setup(&stream.packets[2], ident.channels)?;
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); ident.channels];
    let mut prev_center: Option<usize> = None;
    let mut prev_n = 0usize;
    let mut first_center: Option<usize> = None;
    for packet in stream.packets.iter().skip(3) {
        if packet.is_empty() {
            continue;
        }
        let block = match decode_packet(packet, &ident, &setup) {
            Ok(Some(b)) => b,
            Ok(None) => continue,
            Err(_) => break,
        };
        let n = block.n;
        let center = match prev_center {
            None => n / 2,
            Some(c) => c + (prev_n + n) / 4,
        };
        if prev_center.is_some() && first_center.is_none() {
            first_center = Some(center);
        }
        if !overlap_add(&mut out, &block.channels, center, n) {
            break;
        }
        prev_center = Some(center);
        prev_n = n;
    }
    Ok((out, first_center.unwrap_or(0), stream.last_granule))
}

/// Diagnostic: the block size of each audio packet, for alignment analysis.
/// Not used by playback.
pub fn debug_block_sizes(bytes: &[u8]) -> Result<Vec<usize>, AudioError> {
    let stream = ogg::read_packets(bytes)?;
    if stream.packets.len() < 3 {
        return Err(AudioError::Truncated);
    }
    let ident = read_ident(&stream.packets[0])?;
    let setup = read_setup(&stream.packets[2], ident.channels)?;
    let mut out = Vec::new();
    for packet in stream.packets.iter().skip(3) {
        if packet.is_empty() {
            continue;
        }
        match decode_packet(packet, &ident, &setup) {
            Ok(Some(b)) => out.push(b.n),
            Ok(None) => continue,
            Err(_) => break,
        }
    }
    Ok(out)
}

fn read_ident(packet: &[u8]) -> Result<Ident, AudioError> {
    if packet.len() < 28 || packet[0] != 1 || &packet[1..7] != b"vorbis" {
        return Err(AudioError::NotOgg);
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
    let blocksize_0 = 1usize << bs0.min(20);
    let blocksize_1 = 1usize << bs1.min(20);
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AudioError::UnsupportedChannels(channels as u16));
    }
    if sample_rate == 0 || sample_rate > 384_000 {
        return Err(AudioError::UnsupportedRate(sample_rate));
    }
    if !(64..=8192).contains(&blocksize_0)
        || !(64..=8192).contains(&blocksize_1)
        || blocksize_0 > blocksize_1
    {
        return Err(AudioError::Malformed);
    }
    Ok(Ident {
        channels,
        sample_rate,
        blocksize_0,
        blocksize_1,
    })
}

fn read_setup(packet: &[u8], channels: usize) -> Result<Setup, AudioError> {
    if packet.len() < 7 || packet[0] != 5 || &packet[1..7] != b"vorbis" {
        return Err(AudioError::Malformed);
    }
    let mut r = BitReader::new(&packet[7..]);

    let codebook_count = r.read(8).ok_or(AudioError::Truncated)? as usize + 1;
    let mut codebooks = Vec::with_capacity(codebook_count.min(256));
    for _ in 0..codebook_count {
        codebooks.push(Codebook::read(&mut r)?);
    }

    // Time-domain transforms: placeholders that must be zero.
    let time_count = r.read(6).ok_or(AudioError::Truncated)? as usize + 1;
    for _ in 0..time_count {
        if r.read(16).ok_or(AudioError::Truncated)? != 0 {
            return Err(AudioError::Malformed);
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
            return Err(AudioError::Malformed);
        }
        modes.push(Mode { blockflag, mapping });
    }

    Ok(Setup {
        codebooks,
        floors,
        residues,
        mappings,
        modes,
    })
}

fn read_mapping(
    r: &mut BitReader,
    channels: usize,
    floor_count: usize,
    residue_count: usize,
) -> Result<Mapping, AudioError> {
    let kind = r.read(16).ok_or(AudioError::Truncated)?;
    if kind != 0 {
        return Err(AudioError::Malformed);
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
                return Err(AudioError::Malformed);
            }
            coupling.push((magnitude, angle));
        }
    }
    if r.read(2).ok_or(AudioError::Truncated)? != 0 {
        return Err(AudioError::Malformed);
    }
    let mut mux = vec![0usize; channels];
    if submaps > 1 {
        for m in mux.iter_mut() {
            let v = r.read(4).ok_or(AudioError::Truncated)? as usize;
            if v >= submaps {
                return Err(AudioError::Malformed);
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
            return Err(AudioError::Malformed);
        }
        submap_floor.push(f);
        submap_residue.push(rs);
    }
    Ok(Mapping {
        coupling,
        mux,
        submap_floor,
        submap_residue,
    })
}

struct Block {
    n: usize,
    channels: Vec<Vec<f32>>,
}

fn decode_packet(packet: &[u8], ident: &Ident, setup: &Setup) -> Result<Option<Block>, AudioError> {
    let mut r = BitReader::new(packet);
    if r.read(1).ok_or(AudioError::Truncated)? != 0 {
        // Not an audio packet.
        return Ok(None);
    }
    let mode_bits = ilog(setup.modes.len() as i32 - 1);
    let mode_number = r.read(mode_bits).ok_or(AudioError::Truncated)? as usize;
    let mode = setup.modes.get(mode_number).ok_or(AudioError::Malformed)?;
    let blockflag = mode.blockflag;
    let n = if blockflag {
        ident.blocksize_1
    } else {
        ident.blocksize_0
    };
    let (prev_flag, next_flag) = if blockflag {
        (
            r.read_bit().ok_or(AudioError::Truncated)?,
            r.read_bit().ok_or(AudioError::Truncated)?,
        )
    } else {
        (true, true)
    };

    let mapping = setup
        .mappings
        .get(mode.mapping)
        .ok_or(AudioError::Malformed)?;
    let half = n / 2;
    let ch = ident.channels;

    // --- floor ---
    let mut floor_curves: Vec<Option<Vec<f32>>> = Vec::with_capacity(ch);
    let mut no_residue = vec![false; ch];
    for c in 0..ch {
        let submap = mapping.mux[c];
        let fi = mapping.submap_floor[submap];
        let Floor::Type1(f1) = setup.floors.get(fi).ok_or(AudioError::Malformed)?;
        match f1.decode(&mut r, &setup.codebooks)? {
            Some(y) => floor_curves.push(Some(f1.synthesize(&y, half))),
            None => {
                no_residue[c] = true;
                floor_curves.push(None);
            }
        }
    }

    // Coupled channels stand or fall together.
    for &(m, a) in &mapping.coupling {
        if !no_residue[m] || !no_residue[a] {
            no_residue[m] = false;
            no_residue[a] = false;
        }
    }

    // --- residue, per submap ---
    let mut spectra: Vec<Vec<f32>> = vec![vec![0.0f32; half]; ch];
    let submaps = mapping.submap_residue.len();
    for s in 0..submaps {
        let members: Vec<usize> = (0..ch).filter(|&c| mapping.mux[c] == s).collect();
        if members.is_empty() {
            continue;
        }
        let mut sub: Vec<Vec<f32>> = members.iter().map(|_| vec![0.0f32; half]).collect();
        let skip: Vec<bool> = members.iter().map(|&c| no_residue[c]).collect();
        let ri = mapping.submap_residue[s];
        let res = setup.residues.get(ri).ok_or(AudioError::Malformed)?;
        res.decode(&mut r, &setup.codebooks, &mut sub, &skip, half)?;
        for (k, &c) in members.iter().enumerate() {
            spectra[c] = std::mem::take(&mut sub[k]);
        }
    }

    // --- inverse coupling, in reverse order ---
    for &(m, a) in mapping.coupling.iter().rev() {
        for i in 0..half {
            let mag = spectra[m][i];
            let ang = spectra[a][i];
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
            spectra[m][i] = new_m;
            spectra[a][i] = new_a;
        }
    }

    // --- floor multiply, IMDCT, window ---
    // No normalisation here: `imdct` is the unnormalised sum, which is exactly
    // what Vorbis wants — the encoder's forward transform already carried the
    // 1/M factor, so applying it again here scaled every block down by n/2
    // (measured: reference = ours * 1024.0 on 2048-sample blocks, and a
    // different factor on 256-sample ones, because 2/n varies with block size
    // while the true correction is constant).
    let win = lapped_window(n, ident.blocksize_0, prev_flag, next_flag);
    let mut channels_out = Vec::with_capacity(ch);
    for c in 0..ch {
        let mut spec = std::mem::take(&mut spectra[c]);
        match &floor_curves[c] {
            Some(curve) => {
                for (s, f) in spec.iter_mut().zip(curve.iter()) {
                    *s *= f;
                }
            }
            None => {
                for s in spec.iter_mut() {
                    *s = 0.0;
                }
            }
        }
        let mut time = mdct::imdct(&spec);
        for (t, w) in time.iter_mut().zip(win.iter()) {
            *t *= *w;
        }
        channels_out.push(time);
    }

    Ok(Some(Block {
        n,
        channels: channels_out,
    }))
}

/// The window for one block, accounting for long/short neighbours.
/// Lap one decoded block into the output at its window centre.
///
/// A block's window is centred on `center` and reaches `n/2` either side, so an
/// early long block can begin BEFORE sample zero — a 2048-sample block whose
/// centre is 832 starts at -192. Those leading samples lie outside the stream
/// and must be DROPPED. Clamping the start to zero instead slides the whole
/// block later by the underflow, which corrupts every sample until the centres
/// grow past `n/2`: that is precisely why files opening `[256, 256, 2048, ...]`
/// decoded with a wrong head and a correct body.
///
/// Shared by `decode` and `debug_raw` so a diagnostic can never disagree with
/// the decoder it is meant to diagnose.
///
/// Returns false when the block would exceed the sample cap.
fn overlap_add(out: &mut [Vec<f32>], channels: &[Vec<f32>], center: usize, n: usize) -> bool {
    let start_signed = center as i64 - (n as i64) / 2;
    let (start, skip) = if start_signed < 0 {
        (0usize, (-start_signed) as usize)
    } else {
        (start_signed as usize, 0usize)
    };
    // Entirely before the stream: nothing of this block is audible.
    if skip >= n {
        return true;
    }
    let need = start + (n - skip);
    if need > MAX_SAMPLES_PER_CHANNEL {
        return false;
    }
    for (ch, samples) in channels.iter().enumerate() {
        let Some(buf) = out.get_mut(ch) else { continue };
        if buf.len() < need {
            buf.resize(need, 0.0);
        }
        for (i, s) in samples.iter().enumerate().skip(skip) {
            buf[start + i - skip] += s;
        }
    }
    true
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_refused_without_panicking() {
        assert!(decode(&[]).is_err());
        assert!(decode(b"OggS but not really vorbis").is_err());
    }

    #[test]
    fn ident_header_validates_its_fields() {
        assert!(read_ident(&[1, b'v']).is_err());
        let mut p = vec![1u8];
        p.extend_from_slice(b"vorbis");
        p.extend_from_slice(&0u32.to_le_bytes()); // version
        p.push(0); // channels = 0 -> refused
        p.extend_from_slice(&44100u32.to_le_bytes());
        p.extend_from_slice(&[0u8; 12]);
        p.push(0xBB);
        p.push(1);
        assert!(read_ident(&p).is_err());
    }

    #[test]
    fn lapped_window_is_power_complementary_for_equal_blocks() {
        let n = 256;
        let w = lapped_window(n, 64, true, true);
        for i in 0..n / 2 {
            let s = w[i] * w[i] + w[i + n / 2] * w[i + n / 2];
            assert!((s - 1.0).abs() < 1e-5, "i={i} {s}");
        }
    }

    #[test]
    fn lapped_window_narrows_against_a_short_neighbour() {
        let n = 256;
        let w = lapped_window(n, 64, false, true);
        assert_eq!(w[0], 0.0);
        assert_eq!(w[n / 4 - 64 / 4 - 1], 0.0);
        assert!((w[n / 2] - 1.0).abs() < 1e-5);
    }
}
