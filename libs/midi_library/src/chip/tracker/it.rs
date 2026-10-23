//! IT: Impulse Tracker. Instruments with a key map (note and sample per
//! key), envelopes with sustain loops, new note actions; samples 8- or
//! 16-bit, signed or not, stereo, plain or IT2.14/2.15 compressed.

use super::{s3m, Bytes, Cell, Envelope, Instrument, Kind, Loop, Module, Pattern, Sample, Vol, NOTE_CUT, NOTE_FADE, NOTE_OFF, ORDER_END, ORDER_SKIP};

fn volume(v: u8) -> Vol {
    match v {
        0..=64 => Vol::Set(v),
        65..=74 => Vol::FineSlide((v - 65) as i8),
        75..=84 => Vol::FineSlide(-((v - 75) as i8)),
        85..=94 => Vol::Slide((v - 85) as i8),
        95..=104 => Vol::Slide(-((v - 95) as i8)),
        105..=114 => Vol::PortaDown((v - 105) * 4),
        115..=124 => Vol::PortaUp((v - 115) * 4),
        128..=192 => Vol::Pan(v - 128),
        193..=202 => Vol::TonePorta([0, 1, 4, 8, 16, 32, 64, 96, 128, 255][(v - 193) as usize]),
        203..=212 => Vol::VibratoDepth(v - 203),
        _ => Vol::None,
    }
}

fn envelope(b: &Bytes, at: usize, values: impl Fn(i8) -> f32) -> Option<Envelope> {
    let flags = b.u8(at);
    let n = (b.u8(at + 1) as usize).min(25);
    if flags & 1 == 0 || n == 0 {
        return None;
    }
    let (lb, le, sb, se) = (b.u8(at + 2) as usize, b.u8(at + 3) as usize, b.u8(at + 4) as usize, b.u8(at + 5) as usize);
    let points = (0..n).map(|i| (b.u16(at + 6 + i * 3 + 1), values(b.u8(at + 6 + i * 3) as i8))).collect();
    Some(Envelope {
        points,
        looped: (flags & 2 != 0 && lb <= le && le < n).then_some((lb, le)),
        sustain: (flags & 4 != 0 && sb <= se && se < n).then_some((sb, se)),
    })
}

/// Little-endian bits, lowest first.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..n {
            let byte = self.data.get(self.pos >> 3).copied().unwrap_or(0);
            v |= (((byte >> (self.pos & 7)) & 1) as u32) << i;
            self.pos += 1;
        }
        v
    }
}

/// IT2.14 (or, `it215`, 2.15) compressed sample data: `frames` frames of
/// one channel from `data`; the bytes used.
pub fn decompress(data: &[u8], frames: usize, wide: bool, it215: bool) -> (Vec<f32>, usize) {
    let mut out = Vec::with_capacity(frames);
    let mut at = 0;
    let block_frames = if wide { 0x4000 } else { 0x8000 };
    while out.len() < frames && at + 2 <= data.len() {
        let len = u16::from_le_bytes([data[at], data[at + 1]]) as usize;
        at += 2;
        let block = &data[at..(at + len).min(data.len())];
        at += len;
        let mut bits = Bits { data: block, pos: 0 };
        let todo = block_frames.min(frames - out.len());
        let (top, mut width): (u32, u32) = if wide { (17, 17) } else { (9, 9) };
        let (mut d1, mut d2): (i32, i32) = (0, 0);
        let mut done = 0;
        while done < todo && bits.pos < block.len() * 8 + 8 {
            let mut v = bits.read(width);
            if width < 7 {
                if v == 1 << (width - 1) {
                    v = bits.read(if wide { 4 } else { 3 }) + 1;
                    width = if v < width { v } else { v + 1 };
                    continue;
                }
            } else if width < top {
                let border = ((if wide { 0xFFFFu32 } else { 0xFFu32 }) >> (top - width)) - if wide { 8 } else { 4 };
                if v > border && v <= border + if wide { 16 } else { 8 } {
                    v -= border;
                    width = if v < width { v } else { v + 1 };
                    continue;
                }
            } else if width == top {
                if v & (1 << (top - 1)) != 0 {
                    width = (v + 1) & 0xFF;
                    continue;
                }
            } else {
                break;
            }
            // Sign-extend from `width` bits to the sample width.
            let sample_bits = if wide { 16 } else { 8 };
            let value = if width < sample_bits {
                let shift = 32 - width;
                ((v << shift) as i32) >> shift
            } else if wide {
                v as u16 as i16 as i32
            } else {
                v as u8 as i8 as i32
            };
            if wide {
                d1 = (d1 + value) as i16 as i32;
                d2 = (d2 + d1) as i16 as i32;
            } else {
                d1 = (d1 + value) as i8 as i32;
                d2 = (d2 + d1) as i8 as i32;
            }
            let s = if it215 { d2 } else { d1 };
            out.push(s as f32 / if wide { 32768.0 } else { 128.0 });
            done += 1;
            if width == 0 || width > top {
                break;
            }
        }
        // A broken block: fill it with silence and go on.
        while done < todo {
            out.push(0.0);
            done += 1;
        }
    }
    out.resize(frames, 0.0);
    (out, at)
}

fn sample(b: &Bytes, p: usize, cwt: u16) -> Sample {
    let name = b.text(p + 0x14, 26);
    let mut s = Sample::silent(name);
    let flags = b.u8(p + 0x12);
    s.global_volume = b.u8(p + 0x11).min(64) as f32 / 64.0;
    s.volume = b.u8(p + 0x13).min(64) as f32 / 64.0;
    let cvt = b.u8(p + 0x2E);
    let dfp = b.u8(p + 0x2F);
    if dfp & 0x80 != 0 {
        s.pan = Some(((dfp & 0x7F).min(64) as u16 * 255 / 64) as u8);
    }
    let frames = b.u32(p + 0x30) as usize;
    let (lb, le) = (b.u32(p + 0x34) as usize, b.u32(p + 0x38) as usize);
    s.rate = b.u32(p + 0x3C).max(1) as f64;
    let (sb, se) = (b.u32(p + 0x40) as usize, b.u32(p + 0x44) as usize);
    let ptr = b.u32(p + 0x48) as usize;
    if flags & 1 == 0 || frames == 0 {
        return s;
    }
    let wide = flags & 2 != 0;
    let stereo = flags & 4 != 0;
    let signed = cvt & 1 != 0;
    let frames = frames.min(b.0.len().saturating_sub(ptr).saturating_mul(if flags & 8 != 0 { 8 } else { 1 }).max(1));
    let channels = if stereo { 2 } else { 1 };
    let mut chans: Vec<Vec<f32>> = Vec::with_capacity(channels);
    if flags & 8 != 0 {
        let it215 = cvt & 4 != 0 && cwt >= 0x215;
        let mut at = ptr.min(b.0.len());
        for _ in 0..channels {
            let (data, used) = decompress(&b.0[at..], frames, wide, it215);
            at = (at + used).min(b.0.len());
            chans.push(data);
        }
    } else {
        let width = if wide { 2 } else { 1 };
        for c in 0..channels {
            let base = ptr + c * frames * width;
            chans.push(
                (0..frames)
                    .map(|i| {
                        let at = base + i * width;
                        if wide {
                            let v = b.u16(at);
                            if signed { v as i16 as f32 / 32768.0 } else { (v as f32 - 32768.0) / 32768.0 }
                        } else {
                            let v = b.u8(at);
                            if signed { v as i8 as f32 / 128.0 } else { (v as f32 - 128.0) / 128.0 }
                        }
                    })
                    .collect(),
            );
        }
    }
    s.left = chans.remove(0);
    s.right = chans.pop();
    let len = s.left.len();
    if flags & 16 != 0 && le > lb + 1 && lb < len {
        s.looped = Some(Loop { from: lb, to: le.min(len), pingpong: flags & 64 != 0 });
    }
    if flags & 32 != 0 && se > sb + 1 && sb < len {
        s.sustain = Some(Loop { from: sb, to: se.min(len), pingpong: flags & 128 != 0 });
    }
    super::unroll_pingpong(&mut s);
    s
}

fn instrument(b: &Bytes, p: usize, cmwt: u16, samples: usize) -> Instrument {
    let name = b.text(p + 0x20, 26);
    let keymap = (0..128)
        .map(|k: usize| {
            if k >= 120 {
                return None;
            }
            let note = b.u8(p + 0x40 + k * 2);
            let smp = b.u8(p + 0x41 + k * 2) as usize;
            (smp > 0 && smp <= samples).then_some((smp - 1, note.min(119)))
        })
        .collect();
    if cmwt < 0x200 {
        // The old format: a fadeout in 512ths and no point envelope here.
        let fadeout = b.u16(p + 0x18) as f32 / 512.0;
        return Instrument { name, keymap, volume_env: None, pan_env: None, fadeout, global_volume: 1.0, pan: None, nna: b.u8(p + 0x1A) };
    }
    let fadeout = b.u16(p + 0x14) as f32 / 1024.0;
    let dfp = b.u8(p + 0x19);
    Instrument {
        name,
        keymap,
        volume_env: envelope(b, p + 0x130, |v| (v.clamp(0, 64)) as f32 / 64.0),
        pan_env: envelope(b, p + 0x182, |v| (v.clamp(-32, 32) as f32 + 32.0) / 64.0),
        fadeout,
        global_volume: b.u8(p + 0x18).min(128) as f32 / 128.0,
        pan: (dfp & 0x80 == 0).then_some((dfp.min(64) as u16 * 255 / 64) as u8),
        nna: b.u8(p + 0x11).min(3),
    }
}

pub fn read(bytes: &[u8]) -> Result<Module, String> {
    let b = Bytes(bytes);
    if bytes.len() < 0xC0 {
        return Err("IT: the file is too short".into());
    }
    let title = b.text(4, 26);
    let rows_per_beat = b.u8(0x1E);
    let ord_n = b.u16(0x20) as usize;
    let ins_n = b.u16(0x22) as usize;
    let smp_n = b.u16(0x24) as usize;
    let pat_n = b.u16(0x26) as usize;
    let cwt = b.u16(0x28);
    let cmwt = b.u16(0x2A);
    let flags = b.u16(0x2C);
    let special = b.u16(0x2E);
    let global = b.u8(0x30).min(128) as f32 / 128.0;
    let mix = b.u8(0x31).min(128) as f32 / 128.0;
    let speed = b.u8(0x32).max(1);
    let tempo = b.u8(0x33).max(32);
    let msg_len = b.u16(0x36) as usize;
    let msg_at = b.u32(0x38) as usize;
    let stereo = flags & 1 != 0;
    let use_instruments = flags & 4 != 0;
    let mut at = 0xC0;
    let orders: Vec<u16> = (0..ord_n)
        .map(|i| match b.u8(at + i) {
            254 => ORDER_SKIP,
            255 => ORDER_END,
            p => p as u16,
        })
        .collect();
    at += ord_n;
    let ins_ptrs: Vec<usize> = (0..ins_n).map(|i| b.u32(at + i * 4) as usize).collect();
    at += ins_n * 4;
    let smp_ptrs: Vec<usize> = (0..smp_n).map(|i| b.u32(at + i * 4) as usize).collect();
    at += smp_n * 4;
    let pat_ptrs: Vec<usize> = (0..pat_n).map(|i| b.u32(at + i * 4) as usize).collect();
    let samples: Vec<Sample> = smp_ptrs.iter().map(|&p| sample(&b, p, cwt)).collect();
    let instruments: Vec<Instrument> = if use_instruments {
        ins_ptrs.iter().map(|&p| instrument(&b, p, cmwt, samples.len())).collect()
    } else {
        samples.iter().enumerate().map(|(i, s)| Instrument::of_sample(s.name.clone(), i)).collect()
    };
    let mut channels = 1;
    let mut patterns = Vec::with_capacity(pat_n);
    for &p in &pat_ptrs {
        if p == 0 {
            patterns.push((64, Vec::new()));
            continue;
        }
        let len = b.u16(p) as usize;
        let rows = (b.u16(p + 2) as usize).clamp(1, 256);
        let mut at = p + 8;
        let end = (at + len).min(bytes.len());
        let mut masks = [0u8; 64];
        let mut last = [Cell::EMPTY; 64];
        let mut last_raw = [(0u8, 0u8); 64];
        let mut cells: Vec<(usize, usize, Cell)> = Vec::new();
        let mut row = 0;
        while row < rows && at < end {
            let cv = b.u8(at);
            at += 1;
            if cv == 0 {
                row += 1;
                continue;
            }
            let ch = ((cv - 1) & 63) as usize;
            if cv & 128 != 0 {
                masks[ch] = b.u8(at);
                at += 1;
            }
            let mask = masks[ch];
            let mut cell = Cell::EMPTY;
            if mask & 1 != 0 {
                let n = b.u8(at);
                at += 1;
                last[ch].note = match n {
                    255 => NOTE_OFF,
                    254 => NOTE_CUT,
                    0..=119 => n as i16,
                    _ => NOTE_FADE,
                };
                cell.note = last[ch].note;
            }
            if mask & 2 != 0 {
                last[ch].inst = b.u8(at) as u16;
                at += 1;
                cell.inst = last[ch].inst;
            }
            if mask & 4 != 0 {
                last[ch].vol = volume(b.u8(at));
                at += 1;
                cell.vol = last[ch].vol;
            }
            if mask & 8 != 0 {
                last_raw[ch] = (b.u8(at), b.u8(at + 1));
                at += 2;
                cell.fx = s3m::effect(last_raw[ch].0, last_raw[ch].1, true);
            }
            if mask & 16 != 0 {
                cell.note = last[ch].note;
            }
            if mask & 32 != 0 {
                cell.inst = last[ch].inst;
            }
            if mask & 64 != 0 {
                cell.vol = last[ch].vol;
            }
            if mask & 128 != 0 {
                cell.fx = s3m::effect(last_raw[ch].0, last_raw[ch].1, true);
            }
            channels = channels.max(ch + 1);
            cells.push((row, ch, cell));
        }
        patterns.push((rows, cells));
    }
    let patterns = patterns
        .into_iter()
        .map(|(rows, cells)| {
            let mut pat = Pattern::empty(rows, channels);
            for (row, ch, cell) in cells {
                pat.cells[row * channels + ch] = cell;
            }
            pat
        })
        .collect();
    let pans: Vec<u8> = (0..channels)
        .map(|c| {
            let v = b.u8(0x40 + c) & 0x7F;
            if !stereo || v > 64 { 0x80 } else { (v as u16 * 255 / 64) as u8 }
        })
        .collect();
    let channel_volumes = (0..channels).map(|c| b.u8(0x80 + c).min(64) as f32 / 64.0).collect();
    let mut message = String::new();
    if special & 1 != 0 && msg_len > 0 {
        message = b.slice(msg_at, msg_len).iter().map(|&c| if c == 13 { '\n' } else if c < 32 && c != 10 { ' ' } else { c as char }).collect::<String>();
        message = message.trim_end_matches('\0').trim().to_string();
    }
    let names = instruments.iter().map(|i| i.name.clone()).chain(samples.iter().map(|s| s.name.clone())).filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join("\n");
    if !names.is_empty() {
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str(&names);
    }
    let tracker = match cwt >> 12 {
        1 => "Impulse Tracker".to_string(),
        5 => "OpenMPT".to_string(),
        6 => "Schism Tracker".to_string(),
        _ => format!("IT {:x}", cwt),
    };
    Ok(Module {
        kind: Kind::It,
        title,
        tracker,
        channels,
        orders,
        restart: 0,
        patterns,
        instruments,
        samples,
        speed,
        tempo,
        global_volume: global * (mix * 2.0).clamp(0.25, 1.0),
        linear: flags & 8 != 0,
        pans,
        channel_volumes,
        rows_per_beat: (rows_per_beat > 0).then_some(rows_per_beat),
        message,
        s3m_slides: true,
        it_old_effects: flags & 16 != 0,
        it_compat_gxx: flags & 32 != 0,
    })
}
