//! XM: FastTracker 2's Extended Module. Instruments with a key map,
//! volume and pan envelopes and a fadeout; delta-coded 8- or 16-bit
//! samples; linear or Amiga frequency slides.

use super::{protracker, Bytes, Cell, Envelope, Fx, Instrument, Kind, Loop, Module, Pattern, Sample, Vol, NOTE_NONE, NOTE_OFF};

fn effect(fx: u8, param: u8) -> Fx {
    let (x, y) = (param >> 4, param & 15);
    match fx {
        0..=15 => match (fx, x) {
            (14, 1) => Fx::FinePortaUp(y),
            (14, 2) => Fx::FinePortaDown(y),
            _ => protracker::effect(fx, param),
        },
        16 => Fx::GlobalVolume(param.min(64)),
        17 => Fx::GlobalVolSlide(param),
        20 => Fx::KeyOff(param),
        21 => Fx::EnvelopePosition(param),
        25 => Fx::PanSlide(param),
        27 => Fx::MultiRetrig(param),
        29 => Fx::Tremor(param),
        33 => match x {
            1 => Fx::ExtraFinePortaUp(y),
            2 => Fx::ExtraFinePortaDown(y),
            _ => Fx::None,
        },
        _ => Fx::None,
    }
}

fn volume(v: u8) -> (Vol, Fx) {
    let y = v & 15;
    match v >> 4 {
        1..=4 => (Vol::Set(v - 0x10), Fx::None),
        5 if v == 0x50 => (Vol::Set(64), Fx::None),
        6 => (Vol::Slide(-(y as i8)), Fx::None),
        7 => (Vol::Slide(y as i8), Fx::None),
        8 => (Vol::FineSlide(-(y as i8)), Fx::None),
        9 => (Vol::FineSlide(y as i8), Fx::None),
        10 => (Vol::VibratoSpeed(y), Fx::None),
        11 => (Vol::VibratoDepth(y), Fx::None),
        12 => (Vol::Pan(y * 4 + 2), Fx::None),
        13 => (Vol::PanSlide(-(y as i8)), Fx::None),
        14 => (Vol::PanSlide(y as i8), Fx::None),
        15 => (Vol::TonePorta(y << 4), Fx::None),
        _ => (Vol::None, Fx::None),
    }
}

fn envelope(b: &Bytes, at: usize, count: u8, sustain: u8, loop_start: u8, loop_end: u8, kind: u8) -> Option<Envelope> {
    if kind & 1 == 0 || count == 0 {
        return None;
    }
    let n = (count as usize).min(12);
    let points: Vec<(u16, f32)> = (0..n).map(|i| (b.u16(at + i * 4), b.u16(at + i * 4 + 2).min(64) as f32 / 64.0)).collect();
    let ok = |i: u8| (i as usize) < n;
    Some(Envelope {
        points,
        sustain: (kind & 2 != 0 && ok(sustain)).then_some((sustain as usize, sustain as usize)),
        looped: (kind & 4 != 0 && ok(loop_start) && ok(loop_end) && loop_end >= loop_start).then_some((loop_start as usize, loop_end as usize)),
    })
}

pub fn read(bytes: &[u8]) -> Result<Module, String> {
    let b = Bytes(bytes);
    if bytes.len() < 80 {
        return Err("XM: the file is too short".into());
    }
    let title = b.text(17, 20);
    let tracker = b.text(38, 20);
    let header = b.u32(60) as usize;
    let song_len = b.u16(64) as usize;
    let restart = b.u16(66) as usize;
    let channels = (b.u16(68) as usize).clamp(1, 64);
    let pat_n = b.u16(70) as usize;
    let inst_n = b.u16(72) as usize;
    let linear = b.u16(74) & 1 != 0;
    let speed = (b.u16(76).clamp(1, 255)) as u8;
    let tempo = (b.u16(78).clamp(32, 255)) as u8;
    let orders: Vec<u16> = (0..song_len.min(256)).map(|i| b.u8(80 + i) as u16).collect();
    let mut at = 60 + header;
    let mut patterns = Vec::with_capacity(pat_n);
    for _ in 0..pat_n {
        let head = b.u32(at) as usize;
        let rows = (b.u16(at + 5) as usize).clamp(1, 256);
        let packed = b.u16(at + 7) as usize;
        let mut pat = Pattern::empty(rows, channels);
        let mut p = at + head;
        let end = (p + packed).min(bytes.len());
        let mut i = 0;
        while p < end && i < rows * channels {
            let first = b.u8(p);
            let (mut note, mut inst, mut vol, mut fx, mut param) = (0u8, 0u8, 0u8, 0u8, 0u8);
            if first & 0x80 != 0 {
                p += 1;
                if first & 1 != 0 { note = b.u8(p); p += 1; }
                if first & 2 != 0 { inst = b.u8(p); p += 1; }
                if first & 4 != 0 { vol = b.u8(p); p += 1; }
                if first & 8 != 0 { fx = b.u8(p); p += 1; }
                if first & 16 != 0 { param = b.u8(p); p += 1; }
            } else {
                note = first;
                inst = b.u8(p + 1);
                vol = b.u8(p + 2);
                fx = b.u8(p + 3);
                param = b.u8(p + 4);
                p += 5;
            }
            let (v, _) = volume(vol);
            pat.cells[i] = Cell {
                note: match note {
                    0 => NOTE_NONE,
                    97 => NOTE_OFF,
                    n if n < 97 => n as i16 + 11,
                    _ => NOTE_NONE,
                },
                inst: inst as u16,
                vol: v,
                fx: effect(fx, param),
                fx2: Fx::None,
            };
            i += 1;
        }
        patterns.push(pat);
        at += head + packed;
    }
    let mut instruments = Vec::with_capacity(inst_n);
    let mut samples: Vec<Sample> = Vec::new();
    for _ in 0..inst_n {
        let size = (b.u32(at) as usize).max(29);
        let name = b.text(at + 4, 22);
        let count = b.u16(at + 27) as usize;
        if count == 0 {
            instruments.push(Instrument { name, keymap: vec![None; 128], volume_env: None, pan_env: None, fadeout: 0.0, global_volume: 1.0, pan: None, nna: 0 });
            at += size;
            continue;
        }
        let sample_header = (b.u32(at + 29) as usize).max(40);
        let keys: Vec<u8> = (0..96).map(|k| b.u8(at + 33 + k)).collect();
        let vol_env = envelope(&b, at + 129, b.u8(at + 225), b.u8(at + 227), b.u8(at + 228), b.u8(at + 229), b.u8(at + 233));
        let pan_env = envelope(&b, at + 177, b.u8(at + 226), b.u8(at + 230), b.u8(at + 231), b.u8(at + 232), b.u8(at + 234));
        let fadeout = b.u16(at + 239) as f32 / 65536.0;
        at += size;
        let base = samples.len();
        let mut heads = Vec::with_capacity(count);
        for k in 0..count {
            let h = at + k * sample_header;
            heads.push((b.u32(h) as usize, b.u32(h + 4) as usize, b.u32(h + 8) as usize, b.u8(h + 12), b.u8(h + 13) as i8, b.u8(h + 14), b.u8(h + 15), b.u8(h + 16) as i8, b.text(h + 18, 22)));
        }
        at += count * sample_header;
        for (len, ls, ll, vol, finetune, kind, pan, rel, sname) in heads {
            let wide = kind & 16 != 0;
            let data = b.slice(at, len);
            at += len;
            let mut s = Sample::silent(sname);
            if wide {
                let mut acc: i16 = 0;
                s.left = data.chunks_exact(2).map(|c| {
                    acc = acc.wrapping_add(i16::from_le_bytes([c[0], c[1]]));
                    acc as f32 / 32768.0
                }).collect();
            } else {
                let mut acc: i8 = 0;
                s.left = data.iter().map(|&v| {
                    acc = acc.wrapping_add(v as i8);
                    acc as f32 / 128.0
                }).collect();
            }
            let unit = if wide { 2 } else { 1 };
            let (ls, ll) = (ls / unit, ll / unit);
            if kind & 3 != 0 && ll > 1 {
                let to = (ls + ll).min(s.left.len());
                if to > ls + 1 {
                    s.looped = Some(Loop { from: ls, to, pingpong: kind & 3 == 2 });
                }
            }
            s.volume = vol.min(64) as f32 / 64.0;
            s.pan = Some(pan);
            s.rate = 8363.0 * 2f64.powf((rel as f64 + finetune as f64 / 128.0) / 12.0);
            super::unroll_pingpong(&mut s);
            samples.push(s);
        }
        let keymap = (0..128)
            .map(|midi: usize| {
                let k = midi.checked_sub(12).filter(|k| *k < 96).map_or(keys[0], |k| keys[k]) as usize;
                (k < count).then_some((base + k, midi as u8))
            })
            .collect();
        instruments.push(Instrument { name, keymap, volume_env: vol_env, pan_env, fadeout, global_volume: 1.0, pan: None, nna: 0 });
    }
    let message = instruments.iter().map(|i| i.name.clone()).chain(samples.iter().map(|s| s.name.clone())).filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join("\n");
    Ok(Module {
        kind: Kind::Xm,
        title,
        tracker,
        channels,
        orders,
        restart,
        patterns,
        instruments,
        samples,
        speed,
        tempo,
        global_volume: 1.0,
        linear,
        pans: vec![0x80; channels],
        channel_volumes: vec![1.0; channels],
        rows_per_beat: None,
        message,
        s3m_slides: false,
        it_old_effects: false,
        it_compat_gxx: false,
    })
}
