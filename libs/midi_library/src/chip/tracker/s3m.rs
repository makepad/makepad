//! S3M: Scream Tracker 3. Up to 32 channels, packed patterns of 64 rows,
//! 8- or 16-bit samples (unsigned in most files), effects by letter.

use super::{Bytes, Cell, Fx, Instrument, Kind, Loop, Module, Pattern, Sample, Vol, NOTE_CUT, NOTE_NONE, ORDER_END, ORDER_SKIP};

/// An S3M/IT effect letter (1 = A) and its parameter.
pub fn effect(fx: u8, param: u8, it: bool) -> Fx {
    let (x, y) = (param >> 4, param & 15);
    match fx {
        1 => Fx::Speed(param),
        2 => Fx::Jump(param),
        // S3M writes the row in decimal digits; IT in plain hex.
        3 => Fx::Break(if it { param } else { x * 10 + y }),
        4 => Fx::S3mVolSlide(param),
        5 => Fx::S3mPortaDown(param),
        6 => Fx::S3mPortaUp(param),
        7 => Fx::TonePorta(param),
        8 => Fx::Vibrato(x, y),
        9 => Fx::Tremor(param),
        10 => Fx::Arpeggio(param),
        11 => Fx::VibratoVolSlide(param),
        12 => Fx::TonePortaVolSlide(param),
        13 => Fx::ChannelVolume(param.min(64)),
        14 => Fx::ChannelVolSlide(param),
        15 => Fx::Offset(param),
        16 => Fx::PanSlide(param),
        17 => Fx::MultiRetrig(param),
        18 => Fx::Tremolo(x, y),
        19 => match x {
            2 => Fx::Finetune(y),
            7 if it => Fx::NoteAction(y),
            8 => Fx::Pan(y * 17),
            10 => Fx::HighOffset(y),
            11 => Fx::PatternLoop(y),
            12 => Fx::NoteCut(y),
            13 => Fx::NoteDelay(y),
            14 => Fx::PatternDelay(y),
            _ => Fx::None,
        },
        20 if it && param < 0x10 => Fx::TempoSlide(-(param as i8)),
        20 if it && param < 0x20 => Fx::TempoSlide((param & 15) as i8),
        20 if param >= 32 => Fx::Tempo(param),
        21 => Fx::FineVibrato(x, y),
        22 => Fx::GlobalVolume(if it { (param.min(128) as u16 / 2) as u8 } else { param.min(64) }),
        23 => Fx::GlobalVolSlide(param),
        // S3M pans 0..0x80; IT 0..0xFF.
        24 => Fx::Pan(if it { param } else { (param.min(0x80) as u16 * 255 / 128) as u8 }),
        _ => Fx::None,
    }
}

pub fn read(bytes: &[u8]) -> Result<Module, String> {
    let b = Bytes(bytes);
    if bytes.len() < 0x60 {
        return Err("S3M: the file is too short".into());
    }
    let title = b.text(0, 28);
    let orders_n = b.u16(0x20) as usize;
    let inst_n = b.u16(0x22) as usize;
    let pat_n = b.u16(0x24) as usize;
    let flags = b.u16(0x26);
    let version = b.u16(0x28);
    let signed = b.u16(0x2A) == 1;
    let global = b.u8(0x30).min(64) as f32 / 64.0;
    let speed = b.u8(0x31).max(1);
    let tempo = b.u8(0x32).max(32);
    let master = b.u8(0x33);
    let stereo = master & 0x80 != 0;
    let default_pans = b.u8(0x35) == 0xFC;
    // Channels: the settings say which are on and on which side.
    let settings: Vec<u8> = (0..32).map(|i| b.u8(0x40 + i)).collect();
    let used: Vec<usize> = (0..32).filter(|&i| settings[i] < 16).collect();
    let channels = used.last().map_or(0, |&c| c + 1).max(1);
    let mut pans: Vec<u8> = (0..channels).map(|c| if !stereo { 0x80 } else if settings[c] & 15 < 8 { 0x30 } else { 0xD0 }).collect();
    let mut at = 0x60;
    let orders: Vec<u16> = (0..orders_n)
        .map(|i| match b.u8(at + i) {
            254 => ORDER_SKIP,
            255 => ORDER_END,
            p => p as u16,
        })
        .collect();
    at += orders_n;
    let inst_ptrs: Vec<usize> = (0..inst_n).map(|i| b.u16(at + i * 2) as usize * 16).collect();
    at += inst_n * 2;
    let pat_ptrs: Vec<usize> = (0..pat_n).map(|i| b.u16(at + i * 2) as usize * 16).collect();
    at += pat_n * 2;
    if default_pans {
        for (c, pan) in pans.iter_mut().enumerate() {
            let v = b.u8(at + c);
            if v & 0x20 != 0 {
                *pan = ((v & 15) as u16 * 255 / 15) as u8;
            }
        }
    }
    let mut samples = Vec::with_capacity(inst_n);
    for &p in &inst_ptrs {
        let kind = b.u8(p);
        let name = b.text(p + 0x30, 28);
        let mut s = Sample::silent(name);
        if kind != 1 {
            samples.push(s);
            continue;
        }
        let ptr = ((b.u8(p + 0x0D) as usize) << 16 | b.u16(p + 0x0E) as usize) * 16;
        let len = b.u32(p + 0x10) as usize;
        let ls = b.u32(p + 0x14) as usize;
        let le = b.u32(p + 0x18) as usize;
        s.volume = b.u8(p + 0x1C).min(64) as f32 / 64.0;
        let packed = b.u8(p + 0x1E);
        let sflags = b.u8(p + 0x1F);
        let c2spd = b.u32(p + 0x20).max(1);
        s.rate = c2spd as f64;
        let stereo_sample = sflags & 2 != 0;
        let wide = sflags & 4 != 0;
        if packed == 0 {
            let width = if wide { 2 } else { 1 };
            let frames = len;
            let read_ch = |offset: usize| -> Vec<f32> {
                (0..frames)
                    .map(|i| {
                        let at = ptr + offset + i * width;
                        if wide {
                            let v = b.u16(at);
                            if signed { v as i16 as f32 / 32768.0 } else { (v as f32 - 32768.0) / 32768.0 }
                        } else {
                            let v = b.u8(at);
                            if signed { v as i8 as f32 / 128.0 } else { (v as f32 - 128.0) / 128.0 }
                        }
                    })
                    .collect()
            };
            let available = bytes.len().saturating_sub(ptr) / width;
            let frames_ok = frames.min(if stereo_sample { available / 2 } else { available });
            s.left = read_ch(0);
            s.left.truncate(frames_ok);
            if stereo_sample {
                let mut r = read_ch(frames * width);
                r.truncate(frames_ok);
                s.right = Some(r);
            }
        }
        if sflags & 1 != 0 && le > ls + 1 {
            s.looped = Some(Loop { from: ls.min(s.left.len()), to: le.min(s.left.len()), pingpong: false });
            if s.looped.is_some_and(|l| l.to <= l.from + 1) {
                s.looped = None;
            }
        }
        samples.push(s);
    }
    let mut patterns = Vec::with_capacity(pat_n);
    for &p in &pat_ptrs {
        let mut pat = Pattern::empty(64, channels);
        if p == 0 {
            patterns.push(pat);
            continue;
        }
        let len = b.u16(p) as usize;
        let end = (p + len).min(bytes.len());
        let mut at = p + 2;
        let mut row = 0;
        while row < 64 && at < end {
            let what = b.u8(at);
            at += 1;
            if what == 0 {
                row += 1;
                continue;
            }
            let ch = (what & 31) as usize;
            let mut cell = Cell::EMPTY;
            if what & 32 != 0 {
                let note = b.u8(at);
                let inst = b.u8(at + 1);
                at += 2;
                cell.note = match note {
                    255 => NOTE_NONE,
                    254 => NOTE_CUT,
                    n => ((n >> 4) as i16) * 12 + (n & 15) as i16 + 12,
                };
                cell.inst = inst as u16;
            }
            if what & 64 != 0 {
                cell.vol = Vol::Set(b.u8(at).min(64));
                at += 1;
            }
            if what & 128 != 0 {
                cell.fx = effect(b.u8(at), b.u8(at + 1), false);
                at += 2;
            }
            if ch < channels {
                pat.cells[row * channels + ch] = cell;
            }
        }
        patterns.push(pat);
    }
    let instruments = samples.iter().enumerate().map(|(i, s)| Instrument::of_sample(s.name.clone(), i)).collect();
    let message = samples.iter().map(|s| s.name.clone()).filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join("\n");
    let tracker = match version >> 12 {
        1 => "Scream Tracker 3",
        2 => "Imago Orpheus",
        3 => "Impulse Tracker",
        4 => "Schism Tracker",
        5 => "OpenMPT",
        _ => "S3M",
    };
    let _ = flags;
    Ok(Module {
        kind: Kind::S3m,
        title,
        tracker: tracker.into(),
        channels,
        orders,
        restart: 0,
        patterns,
        instruments,
        samples,
        speed,
        tempo,
        global_volume: global,
        linear: false,
        pans,
        channel_volumes: vec![1.0; channels],
        rows_per_beat: None,
        message,
        s3m_slides: true,
        it_old_effects: false,
        it_compat_gxx: false,
    })
}
