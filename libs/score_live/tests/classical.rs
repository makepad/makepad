//! The classical piano pieces in `examples/classical`: Bernd Krueger's
//! performances (piano-midi.de, CC BY-SA 3.0 DE; the MIDI files ship with
//! Makepad's Score app) transcribed into score code by `score::midi`.
//!
//! - `classics_round_trip`: each file is what the transcriber writes today,
//!   and plays its performance: every note starts and ends within a
//!   millisecond of the file, the pedal moves with it.
//! - `transcribe_classics` (ignored): writes the files again.
//! - `render_classics` (ignored): renders them through the modelled grand
//!   to 48 kHz WAVs in `$CLASSICS_WAV_DIR`, printing their levels
//!   (`$CLASSICS_ONLY` picks pieces by name, `$CLASSICS_MASTER` sets the
//!   master gain).

use makepad_score_live::midi::{self, Piece, SectionSpec, Spelling, VoiceSpec};
use std::path::PathBuf;

fn performances() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/score/resources/performances")
}

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/classical")
}

fn voice(name: &'static str, tracks: &[usize], about: &'static str) -> VoiceSpec {
    VoiceSpec { name, tracks: tracks.to_vec(), about }
}

fn section(name: &'static str, measure: usize, about: &'static str) -> SectionSpec {
    SectionSpec { name, measure, about, spelling: None }
}

fn spelled(name: &'static str, measure: usize, about: &'static str, spelling: Spelling) -> SectionSpec {
    SectionSpec { name, measure, about, spelling: Some(spelling) }
}

fn hands() -> Vec<VoiceSpec> {
    vec![voice("right_hand", &[1], "The right hand."), voice("left_hand", &[2], "The left hand.")]
}

/// The pieces: (MIDI file stem, what the transcription needs).
fn pieces() -> Vec<(&'static str, Piece)> {
    let credit = midi::PIANO_MIDI_DE.to_vec();
    vec![
        ("bach-wtc1-prelude1", Piece {
            label: "Prelude and Fugue in C major",
            composer: "Johann Sebastian Bach",
            work: "The Well-Tempered Clavier, Book I: Prelude and Fugue No. 1 in C major, BWV 846",
            about: vec![
                "The prelude is one broken-chord figure, played twice in every bar, walking",
                "through a chain of harmonies: away from C, to G, home, down a chromatic slope,",
                "then over a long G and a long C in the bass. The fugue that follows has four",
                "voices on one subject, entering alto, soprano, tenor, bass, then answering",
                "each other ever closer (stretto). Bars are counted per movement.",
            ],
            voices: vec![
                voice("right_hand", &[1], "Prelude, right hand: the broken chord's upper notes, twice a bar."),
                voice("left_hand", &[2], "Prelude, left hand: the bass note and the tied second note."),
                voice("soprano", &[3], "Fugue, soprano: enters second, in bar 2."),
                voice("alto", &[4], "Fugue, alto: states the subject first."),
                voice("tenor", &[5], "Fugue, tenor: enters in bar 4."),
                voice("bass", &[6], "Fugue, bass: enters last, in bar 5."),
            ],
            sections: vec![
                section("opening", 0, "Prelude: C major stated, I, ii7, V7, I"),
                section("to_the_dominant", 4, "toward G major"),
                section("back_home", 11, "through D minor back to C, then F"),
                section("chromatic", 19, "a chromatic descent over a falling bass"),
                section("dominant_pedal", 23, "over a held G in the bass"),
                section("tonic_pedal", 31, "over C: the last harmonies and the final chord"),
                section("fugue_exposition", 35, "Fugue: the subject in alto, soprano, tenor and bass"),
                section("fugue_strettos", 42, "the subject in close strettos through A minor, D minor and G"),
                section("fugue_coda", 58, "over a C pedal to the end"),
            ],
            spelling: Spelling::Sharps,
            numbering: vec![(0, 1), (35, 1)],
            credit: credit.clone(),
            rights: Some(midi::piano_midi_de_rights()),
        }),
        ("beethoven-fur-elise", Piece {
            label: "Für Elise",
            composer: "Ludwig van Beethoven",
            work: "Bagatelle No. 25 in A minor, WoO 59, \"Für Elise\"",
            about: vec![
                "A rondo, A B A C A: the famous theme (A) frames two episodes, a lighter one",
                "in F and C major (B) and a stormy one over a drumming A in the bass (C).",
                "Bars are counted as played, with the repeats written out; bar 0 is the pickup.",
            ],
            voices: hands(),
            sections: vec![
                section("theme", 0, "A: the theme over A minor and E, twice"),
                section("theme_answer", 16, "A continued: the answer in C and G, the theme, and both again"),
                section("episode", 44, "B: in F major and C major, then E and D-sharp call the theme back"),
                section("theme_again", 59, "A: the theme and its answer"),
                section("storm", 81, "C: over a drumming A, diminished chords and a darkening line"),
                section("way_back", 99, "arpeggios rise and a chromatic line falls back to the theme"),
                section("theme_last", 104, "A: the theme and its answer a last time"),
            ],
            spelling: Spelling::Sharps,
            numbering: vec![(0, 0)],
            credit: credit.clone(),
            rights: Some(midi::piano_midi_de_rights()),
        }),
        ("beethoven-moonlight-1", Piece {
            label: "Moonlight Sonata, I",
            composer: "Ludwig van Beethoven",
            work: "Piano Sonata No. 14 in C-sharp minor, Op. 27 No. 2, \"Moonlight\": I. Adagio sostenuto",
            about: vec![
                "Three layers throughout: a slow melody, the triplets that never stop, and a",
                "bass in octaves. A sonata form in miniature: the theme in C-sharp minor, a",
                "second one in B minor, a development that ends in a long wait over G-sharp,",
                "the return, and a coda with the theme deep in the bass. Pedal as marked:",
                "\"senza sordino\", the dampers lifted through every harmony.",
            ],
            voices: vec![
                voice("melody", &[1], "The melody (right hand, on top)."),
                voice("triplets", &[2], "The triplets (right hand, under the melody)."),
                voice("bass", &[3], "The bass octaves (left hand)."),
            ],
            sections: vec![
                section("introduction", 0, "the triplets alone over the octave bass"),
                section("first_theme", 4, "the dotted melody enters in C-sharp minor"),
                section("transition", 9, "through E major toward B minor"),
                section("second_theme", 14, "in B minor, the melody answered in the bass"),
                section("development", 22, "the theme wanders into F-sharp minor"),
                section("dominant_pedal", 27, "the triplets alone over G-sharp octaves, rising and falling"),
                section("recapitulation", 41, "the theme returns, then the second theme in C-sharp minor"),
                section("coda", 59, "the theme low in the bass over the last triplets"),
            ],
            spelling: Spelling::Sharps,
            numbering: vec![(0, 1)],
            credit: credit.clone(),
            rights: Some(midi::piano_midi_de_rights()),
        }),
        ("chopin-nocturne-op27-2", Piece {
            label: "Nocturne in D-flat",
            composer: "Frédéric Chopin",
            work: "Nocturne in D-flat major, Op. 27 No. 2",
            about: vec![
                "A song over wide left-hand arpeggios: the theme three times, ever more",
                "ornamented, with restless passages in thirds and sixths between, a cadenza",
                "(bar 52, one 6/4 bar) and a long coda over a D-flat pedal.",
            ],
            voices: hands(),
            sections: vec![
                section("introduction", 0, "the left hand's arpeggios alone"),
                section("theme", 1, "the singing theme in D-flat"),
                section("agitato", 9, "con anima: the theme in thirds and sixths, restless and darkening"),
                section("theme_again", 25, "the theme again, more ornate"),
                section("climb", 33, "rising in sequence through A major to a climax"),
                section("theme_last", 45, "the theme, a cadenza, and the last climax"),
                section("coda", 61, "over a D-flat pedal, the theme's echoes in sixths"),
            ],
            spelling: Spelling::Flats,
            numbering: vec![(0, 1)],
            credit: credit.clone(),
            rights: Some(midi::piano_midi_de_rights()),
        }),
        ("chopin-raindrop-prelude", Piece {
            label: "Raindrop Prelude",
            composer: "Frédéric Chopin",
            work: "Prelude in D-flat major, Op. 28 No. 15, \"Raindrop\"",
            about: vec![
                "One note never stops: A-flat, the raindrop, repeating in eighths. Around it a",
                "calm melody in D-flat (A); then, heard as G-sharp, it hammers under a dark",
                "chorale in C-sharp minor that swells twice to fortissimo (B); the melody",
                "returns briefly and the drops fall still.",
            ],
            voices: hands(),
            sections: vec![
                section("theme", 0, "Sostenuto: the melody over the steady A-flats"),
                section("theme_middle", 8, "into B-flat minor and back"),
                section("theme_return", 19, "the melody again, and a short cadenza"),
                spelled("chorale", 27, "C-sharp minor: the chorale in the bass under repeated G-sharps, swelling to ff", Spelling::Sharps),
                spelled("chorale_again", 43, "the chorale again, to a second fortissimo", Spelling::Sharps),
                spelled("chorale_high", 59, "the chorale above; the storm subsides", Spelling::Sharps),
                section("theme_home", 75, "the melody returns in D-flat"),
                section("coda", 81, "a last phrase; the drops fall still"),
            ],
            spelling: Spelling::Flats,
            numbering: vec![(0, 1)],
            credit: credit.clone(),
            rights: Some(midi::piano_midi_de_rights()),
        }),
        ("debussy-clair-de-lune", Piece {
            label: "Clair de lune",
            composer: "Claude Debussy",
            work: "Suite bergamasque, L. 75: III. Clair de lune",
            about: vec![
                "Andante très expressif, pianissimo: a melody in thirds and sixths that floats",
                "across the 9/8 bar, then a middle part over flowing arpeggios that rises to one",
                "climax, and the theme again above the arpeggios. The time is free: the",
                "performer's tempo moves on nearly every beat.",
            ],
            voices: hands(),
            sections: vec![
                section("theme", 0, "pp con sordina: the melody in thirds and sixths"),
                section("rubato", 14, "Tempo rubato: a new phrase over a rocking bass, rising twice"),
                section("un_poco_mosso", 26, "Un poco mosso: the melody over flowing arpeggios"),
                section("climax", 36, "En animant: the climax in E major"),
                section("calmato", 42, "Calmato: the arpeggios settle"),
                section("theme_return", 50, "Tempo I: the theme above arpeggios"),
                section("coda", 58, "Morendo: the last phrases fade to the final chord"),
            ],
            spelling: Spelling::Flats,
            numbering: vec![(0, 1)],
            credit: credit.clone(),
            rights: Some(midi::piano_midi_de_rights()),
        }),
        ("liszt-liebestraum", Piece {
            label: "Liebestraum",
            composer: "Franz Liszt",
            work: "Liebesträume, S. 541: No. 3 in A-flat major, \"O lieb, so lang du lieben kannst\"",
            about: vec![
                "A song without words in three verses, each wider than the last, joined by two",
                "cadenzas: the melody sits in the middle of the texture with the arpeggios",
                "shared by both hands around it. Bar 0 is the pickup; the cadenza bars are",
                "longer (12/4 and 8/4), so some sections are fractional 6/4 bars.",
            ],
            voices: hands(),
            sections: vec![
                section("first_verse", 0, "Poco allegro con affetto: the song in A-flat, in the tenor"),
                section("first_cadenza", 23, "the first cadenza"),
                spelled("second_verse", 27, "the song in B major, higher, turning to C", Spelling::Sharps),
                section("climax", 37, "stretto: octaves over sweeping arpeggios to the climax"),
                section("second_cadenza", 60, "the second cadenza, falling from the top"),
                section("last_verse", 63, "the song a last time, tender, under high echoes"),
                section("coda", 79, "the harmony fades through distant keys to A-flat"),
            ],
            spelling: Spelling::Flats,
            numbering: vec![(0, 0)],
            credit: credit.clone(),
            rights: Some(midi::piano_midi_de_rights()),
        }),
        ("schumann-traumerei", Piece {
            label: "Träumerei",
            composer: "Robert Schumann",
            work: "Kinderszenen, Op. 15: No. 7, Träumerei",
            about: vec![
                "One four-bar phrase, rising from the pickup and falling back, heard six times",
                "in new harmonies; the voices move under it like a chorale. Bars are counted as",
                "played, with the first half's repeat written out; bar 0 is the pickup.",
            ],
            voices: hands(),
            sections: vec![
                section("theme", 0, "the phrase and its answer"),
                section("theme_repeat", 9, "the first half again"),
                section("middle", 17, "through G minor and D minor, the phrase climbing"),
                section("theme_return", 25, "the theme returns and closes in F, slowing"),
            ],
            spelling: Spelling::Flats,
            numbering: vec![(0, 0)],
            credit,
            rights: Some(midi::piano_midi_de_rights()),
        }),
    ]
}

fn read_midi(stem: &str) -> Option<Vec<u8>> {
    std::fs::read(performances().join(format!("{stem}.mid"))).ok()
}

#[test]
#[ignore]
fn transcribe_classics() {
    std::fs::create_dir_all(examples()).unwrap();
    for (stem, piece) in pieces() {
        let bytes = read_midi(stem).expect("the performances are in apps/score/resources/performances");
        let source = midi::transcribe(&bytes, &piece).unwrap_or_else(|e| panic!("{stem}: {e}"));
        std::fs::write(examples().join(format!("{stem}.score.splash")), &source).unwrap();
        let c = midi::check(&bytes, &piece, &source).unwrap_or_else(|e| panic!("{stem}: {e}"));
        println!("{stem}: {} notes, {} pedal moves, {:.1} s, onset {:.3} ms, release {:.3} ms, pedal {:.3} ms, velocity {}", c.notes, c.pedal_moves, c.seconds, c.onset * 1e3, c.release * 1e3, c.pedal * 1e3, c.velocity);
    }
}

#[test]
fn classics_round_trip() {
    for (stem, piece) in pieces() {
        let source = std::fs::read_to_string(examples().join(format!("{stem}.score.splash"))).unwrap_or_else(|e| panic!("{stem}: {e}"));
        makepad_score_live::load(&source).unwrap_or_else(|d| panic!("{stem}: {d:?}"));
        // The performances ship with Makepad; a checkout without them only
        // loads the scores.
        let Some(bytes) = read_midi(stem) else { continue };
        assert_eq!(midi::transcribe(&bytes, &piece).unwrap(), source, "{stem} is not what the transcriber writes: run transcribe_classics");
        let c = midi::check(&bytes, &piece, &source).unwrap_or_else(|e| panic!("{stem}: {e}"));
        assert!(c.onset < 1e-3 && c.release < 1e-3 && c.pedal < 1e-3, "{stem}: {c:?}");
        assert!(c.velocity <= 1 && c.pedal_moves > 0, "{stem}: {c:?}");
    }
}

#[test]
#[ignore]
fn render_classics() {
    let dir = PathBuf::from(std::env::var("CLASSICS_WAV_DIR").expect("CLASSICS_WAV_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let only = std::env::var("CLASSICS_ONLY").ok();
    for (stem, _) in pieces() {
        if only.as_deref().is_some_and(|o| !stem.contains(o)) {
            continue;
        }
        let mut source = std::fs::read_to_string(examples().join(format!("{stem}.score.splash"))).unwrap();
        // `CLASSICS_MASTER=0.5` renders 6 dB down: how far the master
        // limiter was working.
        if let Ok(master) = std::env::var("CLASSICS_MASTER") {
            source = source.replace("Score{label:", &format!("Score{{master: {master} label:"));
        }
        let t = std::time::Instant::now();
        let r = makepad_score_live::render(&source, &makepad_score_live::RenderOpts::default(), &makepad_score_live::NoSamples).unwrap_or_else(|d| panic!("{stem}: {d:?}"));
        let peak = r.left.iter().chain(&r.right).fold(0.0f32, |m, x| m.max(x.abs()));
        let clipped = r.left.iter().chain(&r.right).filter(|x| x.abs() >= 0.999).count();
        let rms = (r.left.iter().chain(&r.right).map(|x| (*x as f64).powi(2)).sum::<f64>() / (2 * r.left.len()) as f64).sqrt();
        // The loudest second, and how many seconds are near silence.
        let second = r.rate as usize;
        let windows: Vec<f64> = r.left.chunks(second).zip(r.right.chunks(second)).map(|(l, rr)| {
            (l.iter().chain(rr).map(|x| (*x as f64).powi(2)).sum::<f64>() / (l.len() + rr.len()) as f64).sqrt()
        }).collect();
        let loudest = windows.iter().cloned().fold(0.0, f64::max);
        let silent = windows.iter().filter(|w| **w < 1e-4).count();
        let db = |x: f64| 20.0 * x.max(1e-12).log10();
        let wav = makepad_audio_synth::wav::stereo_wav_16(&r.left, &r.right, r.rate as u32);
        std::fs::write(dir.join(format!("{stem}.wav")), wav).unwrap();
        println!(
            "{stem}: {:.1} s, rendered in {:.1} s, peak {:.1} dBFS ({} clipped), rms {:.1} dBFS, loudest second {:.1} dBFS, {} near-silent seconds, {} notes",
            r.seconds(), t.elapsed().as_secs_f64(), db(peak as f64), clipped, db(rms), db(loudest), silent, r.played.len()
        );
    }
}
