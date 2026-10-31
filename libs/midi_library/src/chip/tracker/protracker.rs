//! MOD: the Amiga's ProTracker format and its relatives (4 channels with
//! `M.K.`, the PC trackers' `6CHN`/`8CHN`/`xxCH`, the 15-sample
//! Soundtracker modules without a tag). Notes are Amiga periods; the
//! samples are 8-bit signed.

use super::{Bytes, Cell, Fx, Instrument, Kind, Loop, Module, Pattern, Sample, Vol, NOTE_NONE};

/// The channels the tag at 1080 says, or none for no known tag.
pub fn channels_of(bytes: &[u8]) -> Option<usize> {
    let tag = bytes.get(1080..1084)?;
    match tag {
        b"M.K." | b"M!K!" | b"M&K!" | b"N.T." | b"FLT4" | b"4CHN" | b"NSMS" | b"LARD" | b"PATT" => Some(4),
        b"FLT8" | b"CD81" | b"OKTA" | b"OCTA" => Some(8),
        b"2CHN" => Some(2),
        _ => {
            let digit = |b: u8| (b as char).to_digit(10).map(|d| d as usize);
            match (tag[2], tag[3]) {
                (b'C', b'H') | (b'C', b'N') => Some(digit(tag[0])? * 10 + digit(tag[1])?).filter(|n| (1..=32).contains(n)),
                _ if &tag[1..4] == b"CHN" => digit(tag[0]).filter(|n| (1..=9).contains(n)),
                _ if &tag[..3] == b"TDZ" => digit(tag[3]),
                _ => None,
            }
        }
    }
}

/// The note a period plays (C-2, period 428, is MIDI 60).
pub fn period_note(period: u16) -> i16 {
    if period == 0 {
        return NOTE_NONE;
    }
    (60.0 + 12.0 * (428.0 / period as f64).log2()).round().clamp(0.0, 127.0) as i16
}

/// A MOD effect.
pub fn effect(fx: u8, param: u8) -> Fx {
    let (x, y) = (param >> 4, param & 15);
    match fx {
        0 if param != 0 => Fx::Arpeggio(param),
        0 => Fx::None,
        1 => Fx::PortaUp(param),
        2 => Fx::PortaDown(param),
        3 => Fx::TonePorta(param),
        4 => Fx::Vibrato(x, y),
        5 => Fx::TonePortaVolSlide(param),
        6 => Fx::VibratoVolSlide(param),
        7 => Fx::Tremolo(x, y),
        8 => Fx::Pan(param),
        9 => Fx::Offset(param),
        10 => Fx::VolSlide(param),
        11 => Fx::Jump(param),
        12 => Fx::Volume(param.min(64)),
        // The row is written in decimal digits.
        13 => Fx::Break(x * 10 + y),
        14 => match x {
            1 => Fx::FinePortaUp(y),
            2 => Fx::FinePortaDown(y),
            5 => Fx::Finetune(y),
            6 => Fx::PatternLoop(y),
            8 => Fx::Pan(y * 17),
            9 => Fx::Retrig(y),
            10 => Fx::FineVolUp(y),
            11 => Fx::FineVolDown(y),
            12 => Fx::NoteCut(y),
            13 => Fx::NoteDelay(y),
            14 => Fx::PatternDelay(y),
            _ => Fx::None,
        },
        15 if param == 0 => Fx::None,
        15 if param < 32 => Fx::Speed(param),
        15 => Fx::Tempo(param),
        _ => Fx::None,
    }
}

pub fn read(bytes: &[u8]) -> Result<Module, String> {
    let b = Bytes(bytes);
    let (channels, samples_n, orders_at) = match channels_of(bytes) {
        Some(n) => (n, 31, 950),
        None => (4, 15, 470),
    };
    if bytes.len() < orders_at + 130 {
        return Err("MOD: the file is too short".into());
    }
    let title = b.text(0, 20);
    let mut samples = Vec::new();
    let mut lengths = Vec::new();
    for i in 0..samples_n {
        let at = 20 + i * 30;
        let name = b.text(at, 22);
        let len = b.u16be(at + 22) as usize * 2;
        let finetune = (b.u8(at + 24) & 15) as i8;
        let finetune = if finetune > 7 { finetune - 16 } else { finetune };
        let volume = b.u8(at + 25).min(64) as f32 / 64.0;
        let mut loop_start = b.u16be(at + 26) as usize * 2;
        let loop_len = b.u16be(at + 28) as usize * 2;
        // Soundtracker's loop start is in bytes.
        if samples_n == 15 {
            loop_start /= 2;
        }
        let looped = (loop_len > 2 && loop_start + loop_len <= len.max(loop_start + loop_len)).then_some(Loop { from: loop_start, to: (loop_start + loop_len).min(len.max(1)), pingpong: false });
        let mut s = Sample::silent(name);
        s.rate = 8363.0 * 2f64.powf(finetune as f64 / 96.0);
        s.volume = volume;
        s.looped = looped;
        samples.push(s);
        lengths.push(len);
    }
    let song_len = (b.u8(orders_at) as usize).clamp(1, 128);
    let restart = b.u8(orders_at + 1) as usize;
    let all_orders: Vec<u16> = (0..128).map(|i| b.u8(orders_at + 2 + i) as u16).collect();
    // Patterns stored: every order slot counts, played or not.
    let patterns_n = all_orders.iter().copied().max().unwrap_or(0) as usize + 1;
    let header = orders_at + 2 + 128 + if samples_n == 31 { 4 } else { 0 };
    let pattern_bytes = 64 * channels * 4;
    let mut patterns = Vec::with_capacity(patterns_n);
    for p in 0..patterns_n {
        let base = header + p * pattern_bytes;
        let mut pat = Pattern::empty(64, channels);
        for row in 0..64 {
            for ch in 0..channels {
                let at = base + (row * channels + ch) * 4;
                let (b0, b1, b2, b3) = (b.u8(at), b.u8(at + 1), b.u8(at + 2), b.u8(at + 3));
                let period = (((b0 & 15) as u16) << 8) | b1 as u16;
                let inst = ((b0 & 0xF0) | (b2 >> 4)) as u16;
                pat.cells[row * channels + ch] = Cell { note: period_note(period), inst, vol: Vol::None, fx: effect(b2 & 15, b3), fx2: Fx::None };
            }
        }
        patterns.push(pat);
    }
    let mut at = header + patterns_n * pattern_bytes;
    for (s, len) in samples.iter_mut().zip(lengths) {
        let data = b.slice(at, len);
        s.left = data.iter().map(|&v| v as i8 as f32 / 128.0).collect();
        at += len;
        if let Some(l) = s.looped.as_mut() {
            l.to = l.to.min(s.left.len());
            if l.to <= l.from + 2 {
                s.looped = None;
            }
        }
    }
    let instruments = samples.iter().enumerate().map(|(i, s)| Instrument::of_sample(s.name.clone(), i)).collect();
    // Amiga panning: left, right, right, left.
    let pans = (0..channels).map(|c| if matches!(c % 4, 0 | 3) { 0x40 } else { 0xC0 }).collect();
    let message = samples.iter().map(|s| s.name.clone()).filter(|n| !n.trim().is_empty()).collect::<Vec<_>>().join("\n");
    Ok(Module {
        kind: Kind::Mod,
        title,
        tracker: if samples_n == 15 { "Soundtracker".into() } else { String::from_utf8_lossy(b.slice(1080, 4)).into_owned() },
        channels,
        orders: all_orders[..song_len].to_vec(),
        restart: if restart < song_len { restart } else { 0 },
        patterns,
        instruments,
        samples,
        speed: 6,
        tempo: 125,
        global_volume: 1.0,
        linear: false,
        pans,
        channel_volumes: vec![1.0; channels],
        rows_per_beat: None,
        message,
        s3m_slides: false,
        it_old_effects: false,
        it_compat_gxx: false,
    })
}
