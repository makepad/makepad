//! Hearing a score as data, for an AI that cannot listen: levels per node,
//! the master's spectrum in octave bands with a verdict, onsets on the
//! sixteenth grid, and the notes each loop played. Measured on an offline
//! render, which is exactly what plays live.

use crate::build::Samples;
use crate::model::{self, Diag};
use crate::render::{render_with, RenderOpts};
use crate::theory;
use makepad_audio_synth::dsp::{Filter, FilterMode};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Level {
    pub peak_db: f32,
    pub rms_db: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heard {
    pub from_bar: u32,
    pub bars: u32,
    pub bpm: f64,
    pub master: Level,
    /// Per loop and FX, in document order.
    pub nodes: Vec<(String, Level)>,
    /// Octave band centre (Hz) and its level relative to the loudest band.
    pub bands: Vec<(f32, f32)>,
    pub balance: Vec<String>,
    /// Per bar, the sixteenths with an onset (`x`) or not (`.`).
    pub onsets: Vec<String>,
    /// Per loop: (beat within the window, note name or drum, velocity).
    pub notes: Vec<(String, Vec<(f64, String, f32)>)>,
    /// Per bar of the window: the master's RMS (dBFS) and brightness (the
    /// octave bands' energy centroid, Hz): how a held control moved it.
    pub per_bar: Vec<(f32, f32)>,
    pub warnings: Vec<String>,
}

pub const BANDS: [f32; 10] = [31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

fn db(x: f32) -> f32 {
    if x <= 1e-9 { -180.0 } else { 20.0 * x.log10() }
}

fn round1(x: f32) -> f32 {
    (x * 10.0).round() / 10.0
}

/// Hear bars `from_bar..from_bar + bars` with Clock `params` set and
/// fields `held` live from their moments ([`crate::held`]).
pub fn listen(source: &str, from_bar: u32, bars: u32, params: Vec<(String, f32)>, held: Vec<crate::held::Held>, samples: &dyn Samples) -> Result<Heard, Vec<Diag>> {
    let doc = model::ScoreDoc::load(source)?;
    let beats_per_bar = doc.beats_per_bar();
    let bpm = doc.clock.bpm;
    let seconds_per_beat = 60.0 / bpm;
    let bars = bars.clamp(1, 64);
    let start_s = from_bar as f64 * beats_per_bar * seconds_per_beat;
    let end_s = (from_bar + bars) as f64 * beats_per_bar * seconds_per_beat;
    let rate = 48_000.0f32;
    let (w0, w1) = ((start_s * rate as f64) as usize, (end_s * rate as f64) as usize);
    let names: Vec<String> = doc.loops.iter().map(|l| l.name.clone()).chain(doc.fx.iter().map(|f| f.name.clone())).collect();
    // Per node: peak, sum of squares of the chunk RMS, chunks.
    let mut acc = vec![(0.0f32, 0.0f64, 0usize); names.len()];
    let opts = RenderOpts { rate, seconds: Some(end_s), seed: None, params, start: None, held };
    let rendered = render_with(source, &opts, samples, &mut |frame, instance| {
        if frame <= w0 || frame > w1 + makepad_audio_engine::CHUNK {
            return;
        }
        for (i, name) in names.iter().enumerate() {
            let Some(probe) = instance.layout.probes.get(&format!("{}/{name}.peak", instance.name)) else { continue };
            let peak = instance.schedule.probes[*probe as usize];
            acc[i].0 = acc[i].0.max(peak);
            acc[i].1 += (peak * peak * 0.5) as f64;
            acc[i].2 += 1;
        }
    })?;
    let w1 = w1.min(rendered.left.len());
    let w0 = w0.min(w1);
    let mono: Vec<f32> = rendered.left[w0..w1].iter().zip(&rendered.right[w0..w1]).map(|(a, b)| (a + b) * 0.5).collect();
    let mut heard = Heard { from_bar, bars, bpm, ..Heard::default() };
    let (mut peak, mut ms) = (0.0f32, 0.0f64);
    for (l, r) in rendered.left[w0..w1].iter().zip(&rendered.right[w0..w1]) {
        peak = peak.max(l.abs()).max(r.abs());
        ms += ((l * l + r * r) * 0.5) as f64;
    }
    let rms = (ms / mono.len().max(1) as f64).sqrt() as f32;
    heard.master = Level { peak_db: round1(db(peak)), rms_db: round1(db(rms)) };
    for (i, name) in names.iter().enumerate() {
        let (peak, sum, n) = acc[i];
        heard.nodes.push((name.clone(), Level { peak_db: round1(db(peak)), rms_db: round1(db((sum / n.max(1) as f64).sqrt() as f32)) }));
    }
    // Octave bands: a band-pass per centre, the RMS of each.
    let mut levels = Vec::new();
    for hz in BANDS {
        if hz >= rate * 0.45 {
            continue;
        }
        let mut filter = Filter::new(FilterMode::Bandpass);
        filter.set(hz, 1.41, rate);
        let e: f64 = mono.iter().map(|x| {
            let y = filter.process(*x);
            (y * y) as f64
        }).sum();
        levels.push((hz, db((e / mono.len().max(1) as f64).sqrt() as f32)));
    }
    // Each bar: its level and the bands' energy centroid.
    let bar_frames = ((beats_per_bar * seconds_per_beat) * rate as f64) as usize;
    for chunk in mono.chunks(bar_frames.max(1)).filter(|c| c.len() * 2 > bar_frames) {
        let rms = (chunk.iter().map(|x| (x * x) as f64).sum::<f64>() / chunk.len() as f64).sqrt() as f32;
        let (mut weighted, mut total) = (0.0f64, 0.0f64);
        for hz in BANDS.iter().copied().filter(|hz| *hz < rate * 0.45) {
            let mut filter = Filter::new(FilterMode::Bandpass);
            filter.set(hz, 1.41, rate);
            let e: f64 = chunk.iter().map(|x| {
                let y = filter.process(*x);
                (y * y) as f64
            }).sum();
            weighted += e * hz as f64;
            total += e;
        }
        let centroid = if total > 1e-12 { (weighted / total) as f32 } else { 0.0 };
        heard.per_bar.push((round1(db(rms)), centroid.round()));
    }
    let top = levels.iter().map(|(_, l)| *l).fold(-180.0f32, f32::max);
    heard.bands = levels.iter().map(|(hz, l)| (*hz, round1(l - top))).collect();
    let band = |hz: f32| heard.bands.iter().find(|(h, _)| *h == hz).map_or(-60.0, |(_, l)| *l);
    let low = band(63.0).max(band(31.5));
    let lowmid = band(250.0).max(band(500.0));
    let presence = band(2000.0).max(band(4000.0));
    let air = band(8000.0).max(band(16000.0));
    if heard.master.rms_db < -50.0 {
        heard.balance.push("silent: nothing audible in this window".into());
    } else {
        if low < -24.0 {
            heard.balance.push(format!("thin: little low end (63 Hz band {low:.0} dB below the loudest)"));
        }
        if lowmid > low + 3.0 && lowmid > presence + 9.0 {
            heard.balance.push("muddy: 250-500 Hz dominates; high-pass pads and keys, lower mids".into());
        }
        if presence > -2.0 && presence > low + 6.0 {
            heard.balance.push("harsh: 2-4 kHz leads the mix; soften leads and hats".into());
        }
        if air < -36.0 {
            heard.balance.push("dull: almost nothing above 8 kHz; open filters or add hats/air".into());
        }
        if heard.balance.is_empty() {
            heard.balance.push("balanced".into());
        }
    }
    if peak >= 0.99 {
        heard.warnings.push("the master reaches the limiter's ceiling: lower loop amps or the master".into());
    }
    if heard.master.rms_db > -6.0 {
        heard.warnings.push("very loud: RMS above -6 dBFS".into());
    }
    // Onsets: a rise in short-time energy of the high-passed signal.
    let hop = 256usize;
    let mut hp = Filter::new(FilterMode::Highpass);
    hp.set(150.0, 0.707, rate);
    // A little from before the window, so an onset on its first frame rises.
    let pre = w0.min(8 * hop);
    let filtered: Vec<f32> = rendered.left[w0 - pre..w1].iter().zip(&rendered.right[w0 - pre..w1]).map(|(a, b)| hp.process((a + b) * 0.5)).collect();
    // Energy over 1024 frames every 256: long enough that a low note's
    // own cycle does not read as a rise.
    let energies: Vec<f32> = (0..filtered.len() / hop).map(|i| {
        let s = &filtered[i * hop..(i * hop + 4 * hop).min(filtered.len())];
        s.iter().map(|y| y * y).sum::<f32>() / s.len().max(1) as f32
    }).collect();
    let loudest = energies.iter().copied().fold(0.0f32, f32::max);
    let sixteenth_s = seconds_per_beat * 0.25;
    let steps_per_bar = (beats_per_bar * 4.0).round() as usize;
    let mut grid = vec![vec![false; steps_per_bar]; bars as usize];
    let mut last = usize::MAX;
    for i in 4..energies.len() {
        let before = energies[i - 4..i].iter().sum::<f32>() / 4.0;
        if energies[i] > before * 2.5 && energies[i] > loudest * 0.05 && (last == usize::MAX || i - last > 6) {
            last = i;
            // The rise shows once the attack is well inside the window.
            let t = ((i * hop + 3 * hop) as f64 - pre as f64) / rate as f64;
            if t < -sixteenth_s * 0.5 {
                continue;
            }
            let step = (t / sixteenth_s).round().max(0.0) as usize;
            let (bar, at) = (step / steps_per_bar.max(1), step % steps_per_bar.max(1));
            if bar < grid.len() {
                grid[bar][at] = true;
            }
        }
    }
    heard.onsets = grid.iter().map(|bar| bar.iter().map(|on| if *on { 'x' } else { '.' }).collect()).collect();
    let w_beat0 = from_bar as f64 * beats_per_bar;
    let w_beat1 = (from_bar + bars) as f64 * beats_per_bar;
    for l in &doc.loops {
        let notes: Vec<(f64, String, f32)> = rendered.played.iter().filter(|p| p.loop_name == l.name && p.beat >= w_beat0 - 1e-9 && p.beat < w_beat1)
            .map(|p| {
                let name = if p.hit { makepad_score_player::presets::DRUM_SLOTS.get(p.note as usize).copied().unwrap_or("?").to_string() } else { theory::note_name(p.note) };
                (p.beat - w_beat0, name, (p.vel * 100.0).round() / 100.0)
            }).collect();
        if notes.is_empty() {
            heard.warnings.push(format!("loop `{}` played nothing here", l.name));
        }
        heard.notes.push((l.name.clone(), notes));
    }
    Ok(heard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hearing_a_beat_finds_its_kicks_and_its_bass() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet kit = Drums{kit: @analog_hybrid}\nlet b = Synth{preset: @sub}\nlet kick = Loop{play: kit hits: \"x...x...x...x...\"}\nlet bass = Loop{play: b beats: 4 code: fn(i) { play(@a1, {dur: 3.5}) sleep(4) }}\nScore{clock kit b kick bass}\n";
        let heard = listen(src, 1, 2, Vec::new(), Vec::new(), &crate::NoSamples).unwrap();
        assert_eq!(heard.onsets.len(), 2);
        assert_eq!(&heard.onsets[0][..1], "x", "{:?}", heard.onsets);
        assert_eq!(heard.onsets[0].matches('x').count(), 4, "{:?}", heard.onsets);
        assert!(heard.master.rms_db > -40.0 && heard.master.peak_db <= 0.0, "{:?}", heard.master);
        assert!(heard.nodes.iter().all(|(_, l)| l.rms_db > -60.0), "{:?}", heard.nodes);
        assert!(heard.bands.iter().any(|(hz, l)| *hz <= 63.0 && *l > -12.0), "{:?}", heard.bands);
        let bass = &heard.notes.iter().find(|(n, _)| n == "bass").unwrap().1;
        assert_eq!(bass.len(), 2);
        assert_eq!(bass[0].1, "a1");
    }
}
