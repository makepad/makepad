//! Where the score is: its arrangement once through (or a score of loops'
//! longest cycle) with its tempo map, for the caption's position as
//! bar.beat and time.

use makepad_score_live::render::{self, Timing};
use makepad_score_live::ScoreDoc;

/// One section of the arrangement, in beats from its start.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub name: String,
    pub start: f64,
    pub len: f64,
}

/// The arrangement once through: its sections and its tempo map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrangement {
    pub beats_per_bar: f64,
    pub bands: Vec<Band>,
    timing: Option<Timing>,
    /// A loop's cycle, not an arrangement: it repeats, and a click seeks
    /// (there are no sections to loop).
    pub cycle: bool,
}

impl Arrangement {
    /// A score of loops (no arrangement): its longest loop's cycle of
    /// `beats`, one segment per bar, repeating.
    pub fn cycle(doc: &ScoreDoc, beats: f64) -> Arrangement {
        let beats_per_bar = doc.beats_per_bar().max(1.0);
        let bars = (beats / beats_per_bar).ceil().max(1.0) as usize;
        let bands = (0..bars).map(|i| Band { name: format!("{}", i + 1), start: i as f64 * beats_per_bar, len: beats_per_bar }).collect();
        let timing = Timing { bpm: render::transport_bpm(doc), beats_per_bar, seconds: 0.0, sections: Vec::new(), tempo: Vec::new() };
        Arrangement { beats_per_bar, bands, timing: Some(timing), cycle: true }
    }

    pub fn of(doc: &ScoreDoc) -> Arrangement {
        let beats_per_bar = doc.beats_per_bar().max(1.0);
        let mut start = 0.0;
        let bands = doc.arrange.iter().filter_map(|name| doc.sections.iter().find(|s| &s.name == name)).map(|s| {
            let band = Band { name: s.name.clone(), start, len: s.bars * beats_per_bar };
            start += band.len;
            band
        }).collect();
        let timing = Timing { bpm: render::transport_bpm(doc), beats_per_bar, seconds: 0.0, sections: Vec::new(), tempo: render::arrangement_tempo(doc) };
        Arrangement { beats_per_bar, bands, timing: Some(timing), cycle: false }
    }

    /// Beats once through.
    pub fn beats(&self) -> f64 {
        self.bands.last().map_or(0.0, |b| b.start + b.len)
    }

    /// Where a transport beat is in the arrangement when nothing jumped:
    /// once through, then the last section repeating (a live score never
    /// falls silent).
    pub fn fold(&self, beat: f64) -> f64 {
        let total = self.beats();
        if self.cycle && total > 0.0 {
            return beat.max(0.0) % total;
        }
        match self.bands.last() {
            Some(last) if beat >= total && last.len > 0.0 => last.start + (beat - total) % last.len,
            _ => beat.max(0.0),
        }
    }

    /// Seconds from the start to `beat` on the score's tempo map (as the
    /// render places it).
    pub fn seconds_at(&self, beat: f64) -> f64 {
        self.timing.as_ref().map_or(0.0, |t| t.seconds_at(beat))
    }

}

/// `beat` (from the start) as bar.beat, both from 1.
pub fn bar_beat(beat: f64, beats_per_bar: f64) -> String {
    let bpb = beats_per_bar.max(1.0);
    let beat = beat.max(0.0);
    let bar = (beat / bpb).floor();
    format!("{}.{}", bar as u64 + 1, ((beat - bar * bpb).floor() as u64 + 1).min(bpb as u64))
}

/// Seconds as m:ss.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIECE: &str = r#"use mod.score.*
let clock = Clock{bpm: 120 sig: [4 4]}
let kit = Drums{kit: @analog_hybrid}
let kick = Loop{play: kit hits: "x..."}
let intro = Section{bars: 4 loops: [kick]}
let drop = Section{bars: 8 loops: [kick] bpm: 60}
Score{label: "t" clock kit kick intro drop arrange: [intro drop intro]}
"#;

    fn piece() -> Arrangement {
        Arrangement::of(&ScoreDoc::load(PIECE).expect("the piece loads"))
    }

    #[test]
    fn bands_follow_the_arrangement() {
        let a = piece();
        let bands: Vec<(&str, f64, f64)> = a.bands.iter().map(|b| (b.name.as_str(), b.start, b.len)).collect();
        assert_eq!(bands, vec![("intro", 0.0, 16.0), ("drop", 16.0, 32.0), ("intro", 48.0, 16.0)]);
        assert_eq!(a.beats(), 64.0);
        // Past the end the last section repeats.
        assert_eq!(a.fold(64.0 + 5.0), 48.0 + 5.0);
        assert_eq!(a.fold(10.0), 10.0);
    }

    #[test]
    fn position_text_and_time() {
        let a = piece();
        assert_eq!(bar_beat(0.0, 4.0), "1.1");
        assert_eq!(bar_beat(18.5, 4.0), "5.3");
        // 16 beats at 120, then the drop at 60 bpm: 8 s + 2 beats = 10 s.
        assert!((a.seconds_at(16.0) - 8.0).abs() < 1e-6);
        assert!((a.seconds_at(18.0) - 10.0).abs() < 1e-6);
        assert_eq!(clock(70.9), "1:10");
    }

    #[test]
    fn a_score_of_loops_scrubs_its_longest_cycle() {
        let doc = ScoreDoc::load(PIECE).expect("the piece loads");
        let c = Arrangement::cycle(&doc, 14.0);
        assert!(c.cycle);
        assert_eq!(c.bands.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["1", "2", "3", "4"]);
        assert_eq!(c.beats(), 16.0);
        assert_eq!(c.fold(37.0), 5.0);
        assert!((c.seconds_at(8.0) - 4.0).abs() < 1e-6);
    }

}
