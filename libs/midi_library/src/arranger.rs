//! The arranger: the second stage of an import. The mechanical import
//! ([`crate::import`]) is faithful but reads like a dump: a loop per MIDI
//! track holding every note of the song. The arranger is the music model
//! (Opus, through Stage's chat provider, as the Batch composers are): it
//! reads the song as the text transcription ([`crate::roll`]) and the
//! mechanical Score's instruments and mix, and writes the song again as an
//! idiomatic Score: named sections, loops per section that say what
//! repeats (patterns, phrases, chord loops) instead of thousands of notes,
//! split tracks merged, drums on the kit's names, the production presets by
//! role, the mix chain, short comments.
//!
//! Then the check: the arranged Score is played back into an arrangement
//! ([`crate::roll::from_score`]) and compared with the mechanical import on
//! the sixteenth grid ([`compare`]): the tune, the bass, the drums and the
//! harmony. A Score that does not load, fails while playing or drifts from
//! the song goes back to the model with what is wrong, bar by bar, up to
//! [`TURNS`] turns; the best one is kept.

use crate::arrangement::{Arrangement, Part};
use crate::gm::Role;
use crate::roll::{self, Detail};
use makepad_music_iter::gen::Writer;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;

/// Model turns at most (the first writes, the others repair).
pub const TURNS: usize = 3;
/// The similarity an arrangement must reach to be kept without a repair.
pub const GOOD: f64 = 0.85;

/// How closely an arrangement follows the song, 0..1 per aspect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Similarity {
    /// The tune's notes found (pitch and sixteenth) in the arrangement.
    pub melody: f64,
    /// The bass line: notes in common (F1 of pitch and sixteenth).
    pub bass: f64,
    /// The drums: hits in common (F1 of drum row and sixteenth).
    pub drums: f64,
    /// The harmony: pitch classes per beat (cosine, averaged).
    pub harmony: f64,
    pub overall: f64,
    /// The bars that drift most: (bar, aspect, score).
    pub worst: Vec<(usize, &'static str, f64)>,
}

impl Similarity {
    pub fn line(&self) -> String {
        format!("similarity {:.2} (melody {:.2}, bass {:.2}, drums {:.2}, harmony {:.2})", self.overall, self.melody, self.bass, self.drums, self.harmony)
    }
}

type Key = (i64, i32);

fn refs(v: &[Part]) -> Vec<&Part> {
    v.iter().collect()
}

fn steps(parts: &[&Part], drums: bool) -> HashSet<Key> {
    let mut out = HashSet::new();
    for p in parts {
        for n in &p.notes {
            let step = (n.beat * roll::STEPS).round() as i64;
            let key = if drums { roll_drum(n.key) } else { n.key.round() as i32 };
            out.insert((step, key));
        }
    }
    out
}

/// A drum note by its row (close pieces are one row, as the text shows).
fn roll_drum(key: f32) -> i32 {
    match key.round() as i32 {
        35 => 36,
        40 => 38,
        41 => 43,
        48 => 47,
        52 | 55 | 57 => 49,
        59 => 51,
        k => k,
    }
}

fn f1(a: &HashSet<Key>, b: &HashSet<Key>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let common = a.intersection(b).count() as f64;
    2.0 * common / (a.len() + b.len()).max(1) as f64
}

fn recall(want: &HashSet<Key>, have: &HashSet<Key>) -> f64 {
    if want.is_empty() {
        return 1.0;
    }
    want.intersection(have).count() as f64 / want.len() as f64
}

fn chroma(parts: &[&Part], beats: usize) -> Vec<[f64; 12]> {
    let mut out = vec![[0f64; 12]; beats];
    for p in parts {
        for n in &p.notes {
            let from = n.beat.max(0.0);
            let to = n.beat + n.dur.max(0.1);
            let mut b = from.floor() as usize;
            while (b as f64) < to && b < beats {
                let overlap = to.min(b as f64 + 1.0) - from.max(b as f64);
                if overlap > 0.0 {
                    out[b][n.key.round().rem_euclid(12.0) as usize] += overlap;
                }
                b += 1;
            }
        }
    }
    out
}

fn cos(a: &[f64; 12], b: &[f64; 12]) -> Option<f64> {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    (na > 0.0 || nb > 0.0).then(|| if na > 0.0 && nb > 0.0 { dot / (na * nb) } else { 0.0 })
}

/// How closely `arranged` follows `song` (the mechanical import).
pub fn compare(song: &Arrangement, arranged: &Arrangement) -> Similarity {
    let pick = |a: &Arrangement, f: &dyn Fn(&Part) -> bool| -> Vec<Part> { a.parts.iter().filter(|p| f(p)).cloned().collect() };
    let song_melody = pick(song, &|p| p.melody);
    let song_bass = pick(song, &|p| p.role == Role::Bass && !p.melody);
    let song_drums = pick(song, &|p| p.role == Role::Drums);
    let song_pitched = pick(song, &|p| p.role != Role::Drums);
    let arr_bass = pick(arranged, &|p| p.role == Role::Bass && !p.melody);
    let arr_drums = pick(arranged, &|p| p.role == Role::Drums);
    let arr_pitched = pick(arranged, &|p| p.role != Role::Drums);
    // The tune may be voiced by any arranged part: it is found among all
    // of them. The bass, by the arranged parts low enough to be a bass
    // (a part the model named otherwise still counts).
    let low: Vec<Part> = arr_pitched.iter().filter(|p| p.role == Role::Bass || p.notes.iter().map(|n| n.key as f64).sum::<f64>() / p.notes.len().max(1) as f64 <= 52.0).cloned().collect();
    let melody = recall(&steps(&refs(&song_melody), false), &steps(&refs(&arr_pitched), false));
    let bass = f1(&steps(&refs(&song_bass), false), &steps(&refs(if arr_bass.is_empty() { &low } else { &arr_bass }), false));
    let drums = f1(&steps(&refs(&song_drums), true), &steps(&refs(&arr_drums), true));
    let beats = song.beats().ceil() as usize;
    let (a, b) = (chroma(&refs(&song_pitched), beats), chroma(&refs(&arr_pitched), beats));
    let sims: Vec<f64> = a.iter().zip(&b).filter_map(|(x, y)| cos(x, y)).collect();
    let harmony = sims.iter().sum::<f64>() / sims.len().max(1) as f64;
    let mut weights = Vec::new();
    if !song_melody.is_empty() {
        weights.push((melody, 0.35));
    }
    if !song_bass.is_empty() {
        weights.push((bass, 0.25));
    }
    if !song_drums.is_empty() {
        weights.push((drums, 0.2));
    }
    weights.push((harmony, 0.2));
    let total: f64 = weights.iter().map(|w| w.1).sum();
    let overall = weights.iter().map(|w| w.0 * w.1).sum::<f64>() / total;
    // Bar by bar, where it drifts.
    let mut worst = Vec::new();
    for bar in 0..song.bar_count() {
        let (from, to) = ((song.bars[bar] * roll::STEPS).round() as i64, (song.bars[bar + 1] * roll::STEPS).round() as i64);
        let within = |s: &HashSet<Key>| s.iter().filter(|k| k.0 >= from && k.0 < to).copied().collect::<HashSet<Key>>();
        for (aspect, want, have, melodic) in [
            ("melody", &song_melody, &arr_pitched, true),
            ("bass", &song_bass, if arr_bass.is_empty() { &low } else { &arr_bass }, false),
            ("drums", &song_drums, &arr_drums, false),
        ] {
            let drum = aspect == "drums";
            let (w, h) = (within(&steps(&refs(want), drum)), within(&steps(&refs(have), drum)));
            if w.len() >= 2 {
                let score = if melodic { recall(&w, &h) } else { f1(&w, &h) };
                if score < 0.6 {
                    worst.push((bar + 1, aspect, score));
                }
            }
        }
    }
    worst.sort_by(|a, b| a.2.total_cmp(&b.2));
    worst.truncate(24);
    Similarity { melody, bass, drums, harmony, overall, worst }
}

const SYSTEM: &str = "You are the arranger of a song importer. A song was imported from MIDI mechanically: every note of every track, faithful but unreadable. You write it again as an idiomatic, readable Score (the Splash document described in the guide below) that plays the same song with the sound of the original record: the same tune, bass line, drum grooves, harmony, tempo, meter and form.\n\nThe notes:\n- Sections: one per section of the form (the names and bar counts given), in an arrangement; the last section ends the song (`next: @stop`).\n- Loops say what repeats. A groove or riff that repeats every 1, 2 or 4 bars is a loop of that length with a `hits:` pattern or loop code; a section plays the loops it lists. Write a new loop where the music changes, not a copy per bar. A part that never repeats (a tune through a verse) is a note list or code for that section.\n- Faithful: every note of the tune, the bass and the drums as transcribed (pitch, sixteenth, length); keep the chord voicings' registers. Merge doubled or split tracks (two drum tracks into one kit; a part doubled at the octave into one loop that plays both).\n\nThe sound (as important as the notes):\n- First write a sound brief for each part, from the song's era and production as you know the record: one line in plain words, never a product or brand name (\\\"early-80s synth-pop: a sparse, round mono synth bass; a thin, crisp drum machine with a tight snare; a bright lead with a slow filter; little reverb\\\"). Put it as a comment right above the part's instrument: `/// sound bass: ...`.\n- Then choose the instrument that matches the brief. The General MIDI SoundFont is the default and sounds best: `Instrument{kind: @soundfont program: n}` for melody, bass, keys, strings, brass, leads and pads (take the program the mechanical import names, from the MIDI file, unless the brief clearly wants another GM program), and a GM drum kit for the drums: `Instrument{kind: @soundfont bank: 128 program: k}` with k = 0 standard, 8 room, 16 power, 24 electronic, 25 analog drum machine, 32 jazz, 40 brush, 48 orchestra. Use a synthesized patch (`Synth{preset: ...}`, `Drums{kit: ...}`) only where the brief clearly calls for a synth sound that no GM program gives and the patch fits it.\n- Shape each sound to its brief with the mix tools: Eq, Highpass/Lowpass, Comp, Drive, Chorus, sends to a room, plate or hall return (dry where the record is dry), levels by amp. The mix: no muddy low-mids (high-pass what does not need lows, scoop 250-500 Hz on pads and keys), no harsh 2-5 kHz, the tune clearly on top, then the master chain from the guide's Mixing and mastering section.\n- Short comments: what each section and loop is (\\\"verse: the riff over Am F C G\\\").\n- Credits: keep the Score's `rights:` exactly as given. Keep note options (`[beat note dur vel, {bend: ... vib: ...}]`) and instruments written as their own nodes (a `Sampler{src: asset(...)}`, a chip `Synth{...}` of a tracker or chip song) exactly as the mechanical import has them.\n\nReply with the whole Score in one ```splash code block and nothing after it.\n\n";

/// The arranger's system text: its brief and the composer guide.
pub fn system() -> String {
    format!("{SYSTEM}{}", makepad_music_iter::context::score_guide())
}

/// The first message: the song as text, the mechanical Score's head (its
/// instruments, parts and rights; not its note lists).
pub fn prompt(song: &Arrangement, mechanical: &str) -> String {
    let text = roll::to_text(song, &Detail::default());
    let mut head = String::new();
    for line in mechanical.lines() {
        let t = line.trim_start();
        if t.starts_with("let ") && (t.contains("_sound = ") || t.starts_with("let clock")) || t.starts_with("/// ") && t.contains(": track") || t.starts_with("rights:") {
            head.push_str(line);
            head.push('\n');
        }
    }
    format!(
        "Arrange this song as a Score.\n\nThe song as text (the form, the parts, and per bar the chords and each part on a sixteenth grid; `7-8 = 5-6` repeats bars):\n```text\n{text}```\n\nThe mechanical import's instruments (a part per MIDI track) and its rights line, to keep:\n```splash\n{head}```\n"
    )
}

/// A repair request for a Score that loaded but drifts.
fn drift_note(sim: &Similarity) -> String {
    let bars: Vec<String> = sim.worst.iter().map(|(bar, aspect, score)| format!("bar {bar} {aspect} {:.0}%", score * 100.0)).collect();
    format!(
        "That Score plays, but it drifts from the song: {}. The bars that differ most: {}. Compare those bars with the text and fix them (notes, sixteenths, which loops the sections play), then send the whole Score again in the same format.",
        sim.line(),
        bars.join(", ")
    )
}

pub struct Arranged {
    pub source: String,
    pub similarity: Similarity,
    pub turns: usize,
}

/// Run the arranger on `song` (the mechanical import, `mechanical` its
/// Score): up to [`TURNS`] turns; the best Score that loads and plays.
pub fn arrange(writer: &dyn Writer, song: &Arrangement, mechanical: &str, cancel: &AtomicBool, progress: &dyn Fn(&str)) -> Result<Arranged, String> {
    let system = system();
    let mut turns = vec![prompt(song, mechanical)];
    let mut best: Option<Arranged> = None;
    for turn in 1..=TURNS {
        progress(&format!("arranging (turn {turn})"));
        let reply = writer.chat(&system, &turns, cancel)?;
        turns.push(reply.clone());
        let Some(source) = splash_block(&reply) else {
            turns.push("Your reply had no ```splash block with a whole Score. Send the whole Score in the format asked.".into());
            continue;
        };
        let problem = match makepad_score_live::model::load(&source) {
            Err(d) => format!("The Score does not load:\n{}\nFix it and send the whole Score again.", d.iter().take(20).map(|d| d.to_string()).collect::<Vec<_>>().join("\n")),
            Ok(_) => match roll::from_score(&source) {
                Err(e) => format!("The Score does not play: {e}. Fix it and send the whole Score again."),
                Ok(arranged) => {
                    let similarity = compare(song, &arranged);
                    progress(&format!("turn {turn}: {}", similarity.line()));
                    let good = similarity.overall >= GOOD;
                    let note = drift_note(&similarity);
                    if best.as_ref().is_none_or(|b| similarity.overall > b.similarity.overall) {
                        best = Some(Arranged { source: source.clone(), similarity, turns: turn });
                    }
                    if good {
                        break;
                    }
                    note
                }
            },
        };
        turns.push(problem);
    }
    let mut best = best.ok_or_else(|| "the arranger wrote no Score that plays".to_string())?;
    // The sound: each part alone and the mix, measured, against the briefs.
    for turn in 1..=SOUND_TURNS {
        progress(&format!("listening to the parts (sound turn {turn})"));
        let report = match sound_report(&best.source) {
            Ok(report) => report,
            Err(e) => {
                progress(&format!("sound check skipped: {e}"));
                break;
            }
        };
        progress(&report);
        turns.push(best.source.clone());
        turns.push(format!("{report}\nThat is how your Score sounds now (the parts rendered alone, then the mix). Judge each part against your sound brief for it: brightness (centroid), where its energy sits (bands), how fast it attacks and how long it rings, and its level against the others. If every part matches its brief and the mix has no problem, reply exactly SOUND OK. Otherwise fix the sounds (GM programs and kits, patches, Eq, filters, sends, levels), keep every note, and send the whole Score again."));
        let reply = writer.chat(&system, &turns, cancel)?;
        turns.push(reply.clone());
        if reply.trim().starts_with("SOUND OK") {
            break;
        }
        let Some(source) = splash_block(&reply) else { break };
        let Ok(arranged) = makepad_score_live::model::load(&source).map_err(|_| ()).and_then(|_| roll::from_score(&source).map_err(|_| ())) else {
            progress("the sound revision does not load; kept the one before");
            break;
        };
        let similarity = compare(song, &arranged);
        progress(&format!("sound turn {turn}: {}", similarity.line()));
        // A sound revision may not lose the song.
        if similarity.overall + 0.03 >= best.similarity.overall {
            best = Arranged { source, similarity, turns: best.turns + 1 };
        } else {
            break;
        }
    }
    Ok(best)
}

/// Sound turns at most after the notes.
pub const SOUND_TURNS: usize = 2;

/// Each part alone and the mix, measured over 40 s from the first chorus
/// (else the start), with the mix's problems: the text the model judges.
pub fn sound_report(source: &str) -> Result<String, String> {
    let doc = makepad_score_live::model::load(source).map_err(|d| d.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n"))?.doc;
    let per_bar = doc.beats_per_bar();
    let mut beat = 0.0;
    let mut start = 0.0;
    for name in &doc.arrange {
        let Some(s) = doc.sections.iter().find(|s| &s.name == name) else { continue };
        if s.name.contains("chorus") {
            start = beat;
            break;
        }
        beat += s.bars * per_bar;
    }
    let (stems, mix) = crate::sound::stems(source, start, 40.0)?;
    let mut out = format!("Measured over 40 s from beat {start}:\n");
    for (name, m) in &stems {
        out.push_str(&format!("- {name}: {}\n", m.line()));
    }
    out.push_str(&format!("- the mix: {}\n", mix.line()));
    let problems = crate::sound::mix_problems(&mix, &stems);
    if problems.is_empty() {
        out.push_str("Mix: no mud, no harshness, every part audible.\n");
    } else {
        out.push_str(&format!("Mix problems: {}.\n", problems.join("; ")));
    }
    Ok(out)
}

/// The Score in a reply's ```splash block (the last one with `Score{`).
pub fn splash_block(reply: &str) -> Option<String> {
    let mut found = None;
    let mut rest = reply;
    while let Some(at) = rest.find("```") {
        let after = &rest[at + 3..];
        let body_start = after.find('\n').map_or(after.len(), |n| n + 1);
        let Some(end) = after[body_start..].find("```") else { break };
        let body = &after[body_start..body_start + end];
        if body.contains("Score{") {
            found = Some(body.trim().to_string() + "\n");
        }
        rest = &after[body_start + end + 3..];
    }
    found
}

/// Per role, the number of notes, for a report.
pub fn census(a: &Arrangement) -> HashMap<&'static str, usize> {
    let mut out = HashMap::new();
    for p in &a.parts {
        *out.entry(if p.melody { "melody" } else { p.role.name() }).or_insert(0) += p.notes.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted arranger: it returns the mechanical import itself (a
    /// Score that is the song), so the check scores it as the song.
    struct Echo(String);
    impl Writer for Echo {
        fn chat(&self, _: &str, _: &[String], _: &AtomicBool) -> Result<String, String> {
            Ok(format!("Here it is.\n```splash\n{}```\n", self.0))
        }
        fn label(&self) -> String {
            "echo".into()
        }
    }

    #[test]
    fn the_song_itself_scores_as_the_song_and_a_wrong_one_does_not() {
        let bytes = crate::analyse::tests::band();
        let imported = crate::import::import(&bytes, &crate::import::About { title: "T".into(), ..Default::default() }, &crate::import::Options::default()).unwrap();
        let out = arrange(&Echo(imported.source.clone()), &imported.arrangement, &imported.source, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(out.similarity.overall > 0.99, "{:?}", out.similarity);
        assert!(prompt(&imported.arrangement, &imported.source).contains("bass_sound = "));
        // A song with its bass moved up a tone drifts, bar by bar.
        let mut wrong = imported.arrangement.clone();
        for p in wrong.parts.iter_mut().filter(|p| p.role == Role::Bass) {
            for n in &mut p.notes {
                n.key += 2.0;
            }
        }
        let sim = compare(&imported.arrangement, &wrong);
        assert!(sim.bass < 0.1 && sim.overall < GOOD && sim.worst.iter().any(|w| w.1 == "bass"), "{sim:?}");
    }
}
