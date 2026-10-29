//! Round trips, source-preserving edits, the hot swap at a cycle boundary,
//! live equals offline, and determinism with a seed.

use crate::build::NoSamples;
use crate::compose::Composer;
use crate::render::{instantiate, render, transport_bpm, RenderOpts};
use crate::source;
use crate::{key, model};
use makepad_audio_engine::run::{run, NoExt};
use makepad_audio_engine::CHUNK;
use makepad_score_player::EventKind;
use makepad_flow::Literal;

/// The player's slice of a score (as `s`), read by member name.
struct Graph(makepad_flow::live::plan::OwnerSlice);

fn graph(doc: &model::ScoreDoc) -> Graph {
    Graph(crate::player::slice(doc, "s", crate::player::ClockMode::Own))
}

impl Graph {
    fn index(&self, name: &str) -> usize {
        self.0.nodes.iter().position(|n| n.id == format!("s/{name}")).unwrap_or_else(|| panic!("no node {name}"))
    }

    fn node(&self, name: &str) -> &makepad_flow::live::plan::LiveNode {
        &self.0.nodes[self.index(name)]
    }

    /// What is wired into `name`'s `port`, in entry order.
    fn wired(&self, name: &str, port: &str) -> Vec<String> {
        let input = self.node(name).input(port).unwrap_or_else(|| panic!("{name} has no input {port}"));
        input.sources.iter().map(|s| makepad_flow::live::plan::qualified(&s.from.node).trim_start_matches("s/").to_string()).collect()
    }

    fn param(&self, name: &str, param: &str) -> Literal {
        self.node(name).param(param).cloned().unwrap_or_else(|| panic!("{name} has no {param}"))
    }
}

const TECHNO: &str = r#"// A small techno loop.
use mod.score.*

let clock = Clock{bpm: 126 sig: [4 4] seed: 7}

let kit = Drums{kit: @analog_hybrid}
let acid = Synth{preset: @acid cutoff: 700 res: 6}
let pad = Synth{preset: @warm_pad}

let kick = Loop{play: kit hits: "x...x...x...x..." amp: 0.9}
let hats = Loop{play: kit beats: 4 amp: 0.5
    code: fn(i) {
        for s in 0..8 {
            sleep(0.25)
            hit(@hat, {amp: rand(0.4, 0.8)})
            sleep(0.25)
        }
    }
}
let line = Loop{play: acid beats: 4 amp: 0.6
    code: fn(i) {
        let notes = [@a1 @a1 @c2 @a1 @e2 @a1 @g1 @a2]
        for s in 0..16 {
            if one_in(4) == false { play(tick(notes), {cutoff: rand(300, 1400) dur: 0.2}) }
            sleep(0.25)
        }
    }
}
let chords = Loop{play: pad beats: 8 amp: 0.4
    code: fn(i) {
        play(chord(@a3, @m7), {dur: 3.8})
        sleep(4)
        play(chord(@f3, @maj7), {dur: 3.8})
        sleep(4)
    }
}
let echo = Delay{in: [line] time: 0.75 feedback: 0.4 mix: 0.3}
let verb = Reverb{in: [chords echo] room: 0.7 mix: 0.35}

Score{label: "Techno" clock kit acid pad kick hats line chords echo verb}
"#;

#[test]
fn a_score_loads_with_its_graph_and_code() {
    let doc = model::ScoreDoc::load(TECHNO).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.label, "Techno");
    assert_eq!(doc.clock.bpm, 126.0);
    assert_eq!(doc.loops.len(), 4);
    assert_eq!(doc.insts.len(), 3);
    assert_eq!(doc.fx.len(), 2);
    assert_eq!(doc.fx[1].inputs, ["chords", "echo"]);
    assert!(doc.loops[0].hits.is_some());
    assert!(doc.loops[2].code.is_some());
    let instance = instantiate(&doc, 48_000.0).unwrap();
    assert_eq!(instance.feeds.len(), 4);
    // The lowered graph has every loop and FX as a node of the engine.
    for name in ["kick", "hats", "line", "chords", "echo", "verb", "master"] {
        assert!(instance.layout.nodes.iter().any(|n| n.id == format!("{}/{name}", instance.name)), "{name}");
    }
}

#[test]
fn problems_are_reported_where_they_are() {
    let check = |bad: String, line: u32, needle: &str| {
        let diags = model::ScoreDoc::load(&bad).unwrap_err();
        assert!(diags.iter().any(|d| d.line == line && d.to_string().contains(needle)), "wanted `{needle}` on line {line}: {diags:?}");
    };
    check(TECHNO.replace("hits: \"x...x...x...x...\"", "hits: 4"), 10, "pattern string");
    check(TECHNO.replace("room: 0.7", "rooom: 0.7"), 38, "`rooom` is not a Reverb field");
    check(TECHNO.replace("in: [line]", "in: [lien]"), 37, "lien");
    assert!(model::ScoreDoc::load(&TECHNO.replace("seed: 7}", "seed: 7")).is_err());
    // A code error shows when the loop runs: the composer reports it.
    let loaded = model::load(&TECHNO.replace("hit(@hat,", "hit(@hta,")).unwrap();
    let mut composer = Composer::new(loaded);
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    let errors = composer.errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].node.as_deref(), Some("hats"));
    assert!(errors[0].message.contains("no drum `hta`"), "{errors:?}");
    assert_eq!(errors[0].line, 15);
}

#[test]
fn widget_edits_are_source_preserving_and_reload() {
    // Scrub the tempo, mute a loop, toggle a pattern step: every other byte
    // stays and the result loads.
    let splice = source::set_field(TECHNO, "clock", "bpm", "128").unwrap();
    let s = source::apply(TECHNO, &splice);
    assert_eq!(s, TECHNO.replacen("bpm: 126", "bpm: 128", 1));
    let s = source::apply(&s, &source::set_field(&s, "hats", "mute", "true").unwrap());
    let outline = source::outline(&s);
    let hits = outline.node("kick").unwrap().field("hits").unwrap().value;
    // The pattern's third step: character 2 inside the quotes.
    let at = hits.start + 1 + 2;
    let s = format!("{}x{}", &s[..at], &s[at + 1..]);
    let doc = model::ScoreDoc::load(&s).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.clock.bpm, 128.0);
    assert!(doc.loop_def("hats").unwrap().mute);
    assert_eq!(doc.loop_def("kick").unwrap().hits.as_ref().unwrap().steps[2], Some(0.8));
    // Undo the three edits by hand: nothing else changed.
    let undone = format!("{}.{}", &s[..at], &s[at + 1..]).replacen("bpm: 128", "bpm: 126", 1).replacen("\n    mute: true", "", 1);
    assert_eq!(undone, TECHNO);
    // A number literal under the pointer (drag to scrub).
    let at = s.find("0.75").unwrap() + 2;
    assert_eq!(source::number_at(&s, at).unwrap().text(&s), "0.75");
}

#[test]
fn a_seed_makes_a_render_repeat_and_another_seed_changes_it() {
    let opts = RenderOpts { seconds: Some(4.0), ..Default::default() };
    let a = render(TECHNO, &opts, &NoSamples).unwrap();
    let b = render(TECHNO, &opts, &NoSamples).unwrap();
    assert!(a.left.iter().any(|x| x.abs() > 0.05), "silent");
    assert_eq!(a.left, b.left);
    assert_eq!(a.right, b.right);
    let c = render(TECHNO, &RenderOpts { seed: Some(8), ..opts.clone() }, &NoSamples).unwrap();
    assert_ne!(a.left, c.left);
}

/// A live playback in one thread: the graph renders pass by pass on the
/// engine while the composer schedules ahead of the output by a lookahead,
/// the way Stage's composer thread does.
fn live(source: &str, seconds: f64, lookahead_beats: f64, swap: Option<(f64, &str)>) -> (Vec<f32>, Vec<(f64, f32, u64)>) {
    let loaded = model::load(source).unwrap();
    let mut instance = instantiate(&loaded.doc, 48_000.0).unwrap();
    let bpf = transport_bpm(&loaded.doc) / 60.0 / 48_000.0;
    let mut composer = Composer::new(loaded);
    let total = (seconds * 48_000.0) as usize;
    let mut out = Vec::with_capacity(total);
    let mut sent = Vec::new();
    let mut swap = swap;
    while out.len() < total {
        let now = out.len() as f64 * bpf;
        if let Some((at, next)) = swap {
            if now >= at {
                composer.reload(model::load(next).unwrap(), now + lookahead_beats);
                swap = None;
            }
        }
        let mut events = Vec::new();
        composer.pump(now + lookahead_beats, &mut events);
        for event in events {
            let note = match event.kind {
                EventKind::Hit(slot, _) => 100.0 + slot as f32,
                _ => event.note,
            };
            sent.push((event.beat, note, event.loop_key));
            let feed = &instance.feeds.iter().find(|(k, _)| *k == event.loop_key).unwrap().1;
            assert!(feed.send(event));
        }
        let n = CHUNK.min(total - out.len());
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        run(&mut instance.schedule, n, &mut |_, a, b| {
            l.copy_from_slice(&a[..n]);
            r.copy_from_slice(&b[..n]);
        }, &mut NoExt);
        out.extend(l.iter().zip(&r).map(|(a, b)| a + b));
    }
    (out, sent)
}

#[test]
fn live_playback_equals_the_offline_render() {
    let seconds = 6.0;
    let offline = render(TECHNO, &RenderOpts { seconds: Some(seconds), ..Default::default() }, &NoSamples).unwrap();
    let offline: Vec<f32> = offline.left.iter().zip(&offline.right).map(|(a, b)| a + b).collect();
    // A lookahead of a chunk and a bit: enough, since the composer runs
    // before every callback.
    // The composer runs a pass and a bit ahead, as Stage's does.
    let lookahead = ((CHUNK + 64) as f64 / 48_000.0) * 126.0 / 60.0;
    let (live, _) = live(TECHNO, seconds, lookahead, None);
    assert_eq!(live.len(), offline.len());
    let first = live.iter().zip(&offline).position(|(a, b)| a != b);
    assert_eq!(first, None, "live and offline differ from frame {first:?}");
}

const PULSE_A: &str = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet pulse = Loop{play: s beats: 4 code: fn(i) { for b in 0..4 { play(60, {dur: 0.25}) sleep(1) } }}\nScore{clock s pulse}\n";

#[test]
fn a_reevaluation_swaps_a_loop_at_its_next_cycle_without_drift() {
    let b = PULSE_A.replace("play(60,", "play(72,");
    // Re-evaluated halfway through the second cycle (beat 5.3).
    let (audio, sent) = live(PULSE_A, 9.0, 0.5, Some((5.3, &b)));
    let pulse = key("pulse");
    let notes: Vec<(f64, f32)> = sent.iter().filter(|(_, _, k)| *k == pulse).map(|(beat, note, _)| (*beat, *note)).collect();
    // Every note is on its beat, exactly; the old body finishes its cycle
    // (beats 4..7) and the new one starts at the boundary (beat 8).
    for (i, (beat, note)) in notes.iter().enumerate() {
        assert_eq!(*beat, i as f64, "drift at note {i}");
        assert_eq!(*note, if *beat < 8.0 { 60.0 } else { 72.0 }, "at beat {beat}");
    }
    assert!(notes.len() >= 17);
    // Up to the boundary the audio is the old score's, sample for sample.
    let old = render(PULSE_A, &RenderOpts { seconds: Some(4.0), ..Default::default() }, &NoSamples).unwrap();
    let boundary = 4 * 48_000;
    for i in 0..boundary {
        assert_eq!(audio[i], old.left[i] + old.right[i], "differs at {i} before the boundary");
    }
}

#[test]
fn sections_arrange_loops_in_time() {
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat]}\nScore{clock s pulse hat a b arrange: [a b]}");
    let r = render(&src, &RenderOpts::default(), &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(r.sections, vec![(0.0, "a".to_string()), (4.0, "b".to_string())]);
    assert!(r.played.iter().filter(|p| p.loop_name == "pulse").all(|p| p.beat < 4.0));
    assert!(r.played.iter().filter(|p| p.loop_name == "hat").all(|p| p.beat >= 4.0));
    // 2 bars at 120 plus a 2 s tail.
    assert!((r.seconds() - 6.0).abs() < 1e-3);
}

#[test]
fn sections_repeated_in_the_arrangement_play_on_to_what_follows() {
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat] next: @stop}\nScore{clock s pulse hat a b arrange: [a a b]}");
    let r = render(&src, &RenderOpts::default(), &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    // The repeat is one span of a; b follows it.
    assert_eq!(r.sections, vec![(0.0, "a".to_string()), (8.0, "b".to_string())]);
    assert!(r.played.iter().filter(|p| p.loop_name == "pulse").all(|p| p.beat < 8.0));
    assert!(r.played.iter().any(|p| p.loop_name == "hat" && p.beat >= 8.0));
}


#[test]
fn an_editor_panel_loads_and_is_ignored() {
    // The panel a desk runs beside the code: a function the score never calls.
    let panel = "let editor_panel = fn() { View{flow: Down PanelKnob{path: \"acid.cutoff\" min: 200 max: 4000} SynthEnvelope{node: \"acid\"}} }\n";
    let src = TECHNO.replace("let echo =", &format!("{panel}let echo ="));
    let doc = model::ScoreDoc::load(&src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.loops.len(), 4);
    assert!(!doc.order.iter().any(|(n, _)| n == "editor_panel"));
    let plain = render(TECHNO, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    let with = render(&src, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    assert_eq!(plain.left, with.left);
}

#[test]
fn ironfish_plays_a_loop_on_its_beats_and_repeats() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet lead = Ironfish{program: \"acid\" cutoff: 0.5 reverb: 0}\nlet line = Loop{play: lead beats: 2 code: fn(i) { play(@a2, {dur: 0.5}) sleep(1) play(@c3, {dur: 0.5}) sleep(1) }}\nScore{clock lead line}\n";
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    let a = render(src, &opts, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "Ironfish is silent (peak {peak})");
    // Nothing before the first beat's frame; sound right after it.
    assert!(a.left[..1].iter().all(|x| *x == 0.0));
    let b = render(src, &opts, &NoSamples).unwrap();
    assert_eq!(a.left, b.left);
    // A setting out of range is reported on its line.
    let bad = src.replace("cutoff: 0.5", "cutoff: 5");
    assert!(model::load(&bad).is_err());
}

#[test]
fn morph_plays_its_patch_and_reports_bad_fields() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet lead = Morph{preset: @wide_saw a_table: @formant a_pos: 0.3 lfo1_sync: @n8 mods: [[@lfo1 @a_pos 0.4] [@env2, @f1_cutoff, -0.3, @m1]] fx: [@delay @reverb] lfo2_shape: @drawn lfo2_points: [[0 0 0] [0.5 1 0.3] [1 0 0]] b_on: true b_table: @user b_frames: [[1 0 0.5] [1 0.3]]}\nlet line = Loop{play: lead beats: 2 code: fn(i) { play(@a2, {dur: 0.5}) sleep(1) play(@c3, {dur: 0.5}) sleep(1) }}\nScore{clock lead line}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let Some(model::InstKind::Morph { patch, .. }) = doc.insts.iter().find(|i| i.name == "lead").map(|i| &i.kind) else { panic!("not a Morph") };
    assert!(patch.contains("a_table=formant") && patch.contains("mod=lfo1,a_pos,0.4") && patch.contains("fx=delay,reverb"), "{patch}");
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    let a = render(src, &opts, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "Morph is silent (peak {peak})");
    // Deterministic.
    assert_eq!(a.left, render(src, &opts, &NoSamples).unwrap().left);
    // A typo, a bad choice and a bad mod row each come back with the reason.
    for (bad, says) in [("a_pos: 0.3", "a_poss: 0.3"), ("a_table: @formant", "a_table: @nope"), ("[@lfo1 @a_pos 0.4]", "[@lfo9 @a_pos 0.4]")] {
        let errs = model::load(&src.replace(bad, says)).err().unwrap_or_else(|| panic!("{says} loaded"));
        assert!(!errs.is_empty(), "{says}");
    }
    // The AI's tool lists the parameters.
    let out = crate::tools::call("morph_params", &makepad_strict_json::parse(br#"{"groups": ["osc_a"]}"#).unwrap(), None).unwrap();
    let params = out.get("params").and_then(|v| v.as_str()).unwrap_or("");
    assert!(params.contains("a_warp1") && !params.contains("b_pos"));
}

#[test]
fn shader_instruments_and_effects_play_in_the_graph() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet bass = Shader{lib: \"acid\" cutoff: 600 mono: 1}\nlet keys = Shader{lib: \"epiano\"}\nlet line = Loop{play: bass beats: 2 code: fn(i) { play(@a1, {dur: 0.4}) sleep(0.5) play(@c2, {dur: 0.4}) sleep(1.5) }}\nlet chords = Loop{play: keys beats: 2 code: fn(i) { play(chord(@a3, @m7), {dur: 1.5}) sleep(2) }}\nlet space = ShaderFx{in: [chords] lib: \"echo\" wet: 0.4}\nScore{clock bass keys line chords space}\n";
    let started = std::time::Instant::now();
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    eprintln!("load with two shaders compiled: {:?}", started.elapsed());
    assert!(doc.fx[0].shader.is_some());
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    let a = render(src, &opts, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "the shaders are silent (peak {peak})");
    assert!(a.left.iter().all(|x| x.is_finite()));
    let b = render(src, &opts, &NoSamples).unwrap();
    assert_eq!(a.left, b.left);
    // Problems show on their fields.
    let bad = src.replace("cutoff: 600", "cutof: 600");
    let diags = model::load(&bad).err().expect("an unknown param is reported");
    assert!(diags[0].message.contains("no param `cutof`"), "{diags:?}");
    let wrong = src.replace("lib: \"echo\"", "lib: \"acid\"");
    assert!(model::load(&wrong).is_err());
}

#[test]
fn a_voice_sings_its_lyrics_on_the_notes() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 100}\nlet vox = Voice{profile: @alto vibrato: 0.2}\nlet choir = Voice{profile: @tenor choir: 4 parts: 2}\nlet line = Loop{play: vox beats: 4 code: fn(i) { sleep(sing([@e4 @g4 @a4 @g4], \"a-ma-zing grace\", {dur: 1})) }}\nlet pad = Loop{play: choir beats: 4 code: fn(i) { sing([[@c4 @e4]], \"ooh\", {dur: 4}) sleep(4) }}\nScore{clock vox choir line pad}\n";
    let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    let sung: Vec<_> = events.iter().filter(|e| e.loop_key == key("line")).collect();
    assert_eq!(sung.len(), 4, "one syllable per note: {sung:?}");
    assert!(sung.iter().all(|e| matches!(e.kind, EventKind::Sung(0, _))));
    let parts: Vec<u8> = events.iter().filter_map(|e| match e.kind { EventKind::Sung(part, _) if e.loop_key == key("pad") => Some(part), _ => None }).collect();
    assert!(parts.contains(&0) && parts.contains(&1), "a chord sings in parts: {parts:?}");
    let lyrics: Vec<String> = composer.loops.iter().find(|l| l.name == "line").unwrap().last_lyrics.iter().map(|s| s.text.clone()).collect();
    assert_eq!(lyrics.join(" "), "a ma zing grace");
    let opts = RenderOpts { seconds: Some(3.0), ..RenderOpts::default() };
    let a = render(src, &opts, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01 && a.left.iter().all(|x| x.is_finite()), "the voices are silent (peak {peak})");
    assert_eq!(a.left, render(src, &opts, &NoSamples).unwrap().left);
    let bad = src.replace("@alto", "@diva");
    assert!(model::load(&bad).is_err());
}

#[test]
fn a_neural_voice_sings_its_phrases_rendered_ahead() {
    use makepad_score_player::sung::neural::{self, makepad_ai_sing as sing};
    // A tiny untrained voice stands in for the installed one.
    let path = std::env::temp_dir().join(format!("score-neural-test-{}.mksing", std::process::id()));
    sing::Cantor::random(sing::acoustic::AcousticConfig::tiny(), sing::vocoder::VocoderConfig::tiny(), 1).save(&path, sing::weights::Dtype::F32).unwrap();
    std::env::set_var("MAKEPAD_CANTOR_MODEL", &path);
    let line = "code: fn(i) { sleep(sing([@e4 @g4 @a4 @g4], \"a-ma-zing grace\", {dur: 1})) }";
    let src = format!("use mod.score.*\nlet clock = Clock{{bpm: 100}}\nlet vox = Voice{{engine: @neural}}\nlet alto = Voice{{profile: @alto}}\nlet line = Loop{{play: vox beats: 4 {line}}}\nlet dsp = Loop{{play: alto beats: 4 {line}}}\nScore{{clock vox alto line dsp}}\n");
    let mut composer = Composer::new(model::load(&src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(0.1, &mut events);
    let phrases: Vec<_> = events.iter().filter(|e| matches!(e.kind, EventKind::Phrase(_))).collect();
    assert_eq!(phrases.len(), 1, "one phrase per sung line, only on the neural voice: {phrases:?}");
    assert_eq!(phrases[0].loop_key, key("line"));
    // Its audio starts its lead before the first note: 0.3 s at 100 bpm.
    assert!((phrases[0].beat + 0.5).abs() < 1e-6, "phrase at beat {}", phrases[0].beat);
    assert_eq!(events.iter().filter(|e| matches!(e.kind, EventKind::Sung(..)) && e.loop_key == key("dsp")).count(), 4);
    // The neural loop is composed a second ahead of the others.
    events.clear();
    composer.pump(3.0, &mut events);
    assert!(events.iter().any(|e| e.loop_key == key("line") && e.beat >= 4.0), "the neural loop's next cycle is composed ahead");
    assert!(!events.iter().any(|e| e.loop_key == key("dsp") && e.beat >= 4.0));
    // Offline, the phrases render before they play; the same render twice is the same.
    let src = src.replace("Score{clock vox alto line dsp}", "Score{clock vox line}");
    let opts = RenderOpts { seconds: Some(3.0), ..RenderOpts::default() };
    let a = render(&src, &opts, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.0 && a.left.iter().all(|x| x.is_finite()), "the neural voice is silent");
    assert_eq!(a.left, render(&src, &opts, &NoSamples).unwrap().left);
    assert!(neural::shared().is_some());
    let _ = std::fs::remove_file(&path);
    assert!(model::load(&src.replace("@neural", "@opera")).is_err());
}

#[test]
fn game_music_headless_equals_the_offline_render_and_reports_its_beats() {
    use crate::game::{music, Clocking, MusicEvent};
    use makepad_audio_synth::Instrument;
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat]}\nScore{clock s pulse hat a b arrange: [a b]}");
    let seconds = 3.0;
    let offline = render(&src, &RenderOpts { seconds: Some(seconds), ..RenderOpts::default() }, &NoSamples).unwrap();
    let (mut handle, mut inst) = music(&src, 48_000.0, Clocking::Headless).unwrap_or_else(|d| panic!("{d:?}"));
    let total = (seconds * 48_000.0) as usize;
    let mut out = Vec::new();
    let mut events = Vec::new();
    let sizes = [512usize, 64, 1000, 256, 7];
    let (mut done, mut i) = (0, 0);
    while done < total {
        let n = sizes[i % sizes.len()].min(total - done);
        handle.advance(n);
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        inst.render(&mut l, &mut r);
        out.extend(l);
        events.extend(handle.poll());
        done += n;
        i += 1;
    }
    let first = out.iter().zip(&offline.left).position(|(a, b)| a != b);
    assert!(first.is_none() && out.len() == offline.left.len(), "the game's music is the offline render, whatever the blocks: first difference at {first:?} ({:?} vs {:?})", first.map(|i| out[i]), first.map(|i| offline.left[i]));
    let beats: Vec<u64> = events.iter().filter_map(|e| match e { MusicEvent::Beat { beat, .. } => Some(*beat), _ => None }).collect();
    assert_eq!(beats, (0..6).collect::<Vec<u64>>());
    let sections: Vec<&str> = events.iter().filter_map(|e| match e { MusicEvent::Section { name, .. } => Some(name.as_str()), _ => None }).collect();
    assert_eq!(sections, ["a", "b"]);
    // Parked, it goes silent and stops; resumed, it plays again.
    handle.park(0.01);
    let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
    for _ in 0..3 {
        handle.advance(4800);
        inst.render(&mut l, &mut r);
    }
    assert!(l.iter().all(|x| *x == 0.0) && !inst.is_active());
    assert!(!handle.beat().1);
    handle.resume(0.01);
    let mut heard = 0.0f32;
    for _ in 0..40 {
        handle.advance(4800);
        inst.render(&mut l, &mut r);
        heard = heard.max(l.iter().fold(0.0f32, |m, x| m.max(x.abs())));
    }
    assert!(heard > 0.01, "resumed music is silent");
}

#[test]
fn game_music_takes_a_new_graph_without_stopping() {
    use crate::game::{music, Clocking};
    use makepad_audio_synth::Instrument;
    let (mut handle, mut inst) = music(PULSE_A, 48_000.0, Clocking::Headless).unwrap();
    let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
    for _ in 0..10 {
        handle.advance(4800);
        inst.render(&mut l, &mut r);
    }
    // A new instrument and loop: a new graph, the pulse carried over.
    let next = PULSE_A.replace("Score{clock s pulse}", "let bell = Ironfish{program: \"glass\"}\nlet ring = Loop{play: bell beats: 4 code: fn(i) { play(@c5, {dur: 1}) sleep(4) }}\nScore{clock s bell pulse ring}");
    handle.set_source(&next).unwrap_or_else(|d| panic!("{d:?}"));
    let mut heard = 0.0f32;
    for _ in 0..20 {
        handle.advance(4800);
        inst.render(&mut l, &mut r);
        heard = heard.max(l.iter().fold(0.0f32, |m, x| m.max(x.abs())));
        handle.poll();
    }
    assert!(heard > 0.01);
    assert!(handle.set_source("let broken = ").is_err());
}

#[test]
fn game_music_plays_on_a_synth_bus_in_real_time() {
    use crate::game::{music, Clocking, MusicEvent};
    use makepad_audio_synth::bus::{Emitter, Group, SynthBus};
    let (mut handle, inst) = music(PULSE_A, 48_000.0, Clocking::Realtime).unwrap();
    let mut bus = SynthBus::new(48_000.0);
    assert!(bus.add_instrument(7, Box::new(inst), Emitter::FLAT, 1.0).is_ok());
    bus.set_instrument_group(7, Group::Music);
    // Paced like an audio callback: 10 ms blocks for 0.8 s.
    let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]);
    let (mut peak, mut beats) = (0.0f32, 0);
    let started = std::time::Instant::now();
    for block in 0..80 {
        l.fill(0.0);
        r.fill(0.0);
        bus.render(&mut l, &mut r);
        peak = peak.max(l.iter().fold(0.0f32, |m, x| m.max(x.abs())));
        beats += handle.poll().iter().filter(|e| matches!(e, MusicEvent::Beat { .. })).count();
        let due = std::time::Duration::from_millis(10 * (block + 1));
        if let Some(wait) = due.checked_sub(started.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    assert!(peak > 0.001, "the bus is silent");
    assert!(beats >= 1, "no beats reported");
    handle.set_param("nothing", 1.0);
}

#[test]
fn samplers_and_kit_slots_play_the_samples_the_host_gives() {
    use crate::build::Samples;
    use crate::model::AssetRef;
    use makepad_score_player::Sample;
    use std::sync::Arc;
    struct Tone;
    impl Samples for Tone {
        fn sample(&self, asset: &AssetRef) -> Option<Arc<Sample>> {
            let hz = if asset.id == "tone" { 440.0 } else { 90.0 };
            let left: Vec<f32> = (0..24_000).map(|i| (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * (1.0 - i as f32 / 24_000.0)).collect();
            Some(Arc::new(Sample::new(left.into(), None, 48_000.0)))
        }
    }
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet keys = Sampler{src: asset(\"tone\", \"1\") root: @a4}\nlet kit = Drums{kit: @analog_hybrid samples: {kick: asset(\"boom\", \"2\")}}\nlet line = Loop{play: keys beats: 2 code: fn(i) { play(@a4, {dur: 0.5}) sleep(2) }}\nlet beat = Loop{play: kit hits: \"x...\"}\nScore{clock keys kit line beat}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.dependencies().len(), 2);
    let opts = RenderOpts { seconds: Some(1.0), ..RenderOpts::default() };
    let silent = render(src.replace("\"tone\"", "\"missing\"").replace("\"boom\"", "\"gone\"").as_str(), &opts, &NoSamples).unwrap();
    let played = render(src, &opts, &Tone).unwrap();
    let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak(&played.left) > 0.05);
    // Without the samples, the sampler is silent and the kick is the recipe's.
    assert_ne!(played.left, silent.left);
}

#[test]
fn a_voice_sings_with_the_library_singer_the_host_gives() {
    use crate::build::Samples;
    use crate::model::AssetRef;
    use makepad_score_player::sung::VoiceProfile;
    use makepad_score_player::Sample;
    use std::sync::Arc;
    struct Singers;
    impl Samples for Singers {
        fn sample(&self, _asset: &AssetRef) -> Option<Arc<Sample>> {
            None
        }
        fn singer(&self, asset: &AssetRef) -> Option<Arc<VoiceProfile>> {
            (asset.id == "singer_t1").then(|| Arc::new(VoiceProfile::named("bass").unwrap()))
        }
    }
    let src = |id: &str| format!("use mod.score.*\nlet clock = Clock{{bpm: 100}}\nlet vox = Voice{{profile: asset(\"{id}\", \"r1\")}}\nlet line = Loop{{play: vox beats: 2 code: fn(i) {{ sleep(sing([@e3 @g3], \"la la\", {{dur: 1}})) }}}}\nScore{{clock vox line}}\n");
    let doc = model::ScoreDoc::load(&src("singer_t1")).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.singers(), vec![AssetRef { id: "singer_t1".into(), rev: "r1".into() }]);
    assert_eq!(doc.dependencies().len(), 1);
    assert!(doc.samples().is_empty());
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    // Not registered yet: the built-in alto sings in its place.
    let waiting = render(&src("singer_t2"), &opts, &NoSamples).unwrap();
    assert_eq!(crate::render::missing_singers(&model::ScoreDoc::load(&src("singer_t2")).unwrap()).len(), 1);
    let sung = render(&src("singer_t1"), &opts, &Singers).unwrap();
    assert!(crate::render::missing_singers(&doc).is_empty());
    let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak(&waiting.left) > 0.01 && peak(&sung.left) > 0.01);
    assert_ne!(waiting.left, sung.left);
    // A name that is neither built in nor an asset is reported.
    assert!(model::ScoreDoc::load(&src("x").replace("asset(\"x\", \"r1\")", "@choirboy")).is_err());
}

#[test]
fn timing_reads_the_sections_without_rendering() {
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat]}\nScore{clock s pulse hat a b arrange: [a b]}");
    let t = crate::render::timing(&src, None).unwrap();
    assert_eq!((t.bpm, t.beats_per_bar, t.seconds), (120.0, 4.0, 6.0));
    assert_eq!(t.sections, vec![("a".to_string(), 0.0, 2.0), ("b".to_string(), 2.0, 6.0)]);
}

#[test]
fn a_loop_following_a_param_fades_with_it() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 params: {intensity: 0.0}}\nlet s = Synth{preset: @pluck}\nlet layer = Loop{play: s beats: 1 follow: {param: @intensity from: 0.2 to: 0.6} code: fn(i) { play(60) sleep(1) }}\nScore{clock s layer}\n";
    let vels = |value: f32| {
        let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
        composer.set_param("intensity", value);
        let mut events = Vec::new();
        composer.pump(2.0, &mut events);
        events.iter().map(|e| e.vel).collect::<Vec<f32>>()
    };
    assert!(vels(0.0).is_empty());
    assert!(vels(0.4).iter().all(|v| (v - 0.5).abs() < 1e-5), "{:?}", vels(0.4));
    assert!(vels(1.0).iter().all(|v| (v - 1.0).abs() < 1e-5));
}

/// A tiny SoundFont: programs 0 and 40 (bank 0), each a looped sine.
fn tiny_sf2() -> Vec<u8> {
    fn u16s(o: &mut Vec<u8>, v: u16) { o.extend_from_slice(&v.to_le_bytes()); }
    fn u32s(o: &mut Vec<u8>, v: u32) { o.extend_from_slice(&v.to_le_bytes()); }
    fn name(o: &mut Vec<u8>, t: &str) { let mut b = t.as_bytes().to_vec(); b.resize(20, 0); o.extend_from_slice(&b); }
    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut o = id.to_vec();
        u32s(&mut o, data.len() as u32);
        o.extend_from_slice(data);
        if data.len() & 1 != 0 { o.push(0); }
        o
    }
    fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut d = kind.to_vec();
        for c in chunks { d.extend_from_slice(c); }
        chunk(b"LIST", &d)
    }
    let points = 2000u32;
    let mut smpl = Vec::new();
    for i in 0..points {
        let v = ((i as f32 * std::f32::consts::TAU * 261.63 / 44_100.0).sin() * 12_000.0) as i16;
        smpl.extend_from_slice(&v.to_le_bytes());
    }
    smpl.resize(smpl.len() + 92, 0);
    let (mut phdr, mut pbag, mut pgen, mut inst, mut ibag, mut igen, mut shdr) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for (i, program) in [(0u16, 0u16), (1, 40)] {
        name(&mut phdr, "P"); u16s(&mut phdr, program); u16s(&mut phdr, 0); u16s(&mut phdr, i); u32s(&mut phdr, 0); u32s(&mut phdr, 0); u32s(&mut phdr, 0);
        u16s(&mut pbag, i); u16s(&mut pbag, 0);
        u16s(&mut pgen, 41); u16s(&mut pgen, 0);
    }
    name(&mut phdr, "EOP"); u16s(&mut phdr, 0); u16s(&mut phdr, 0); u16s(&mut phdr, 2); u32s(&mut phdr, 0); u32s(&mut phdr, 0); u32s(&mut phdr, 0);
    u16s(&mut pbag, 2); u16s(&mut pbag, 0);
    u16s(&mut pgen, 0); u16s(&mut pgen, 0);
    name(&mut inst, "I"); u16s(&mut inst, 0);
    name(&mut inst, "EOI"); u16s(&mut inst, 1);
    u16s(&mut ibag, 0); u16s(&mut ibag, 0);
    u16s(&mut ibag, 2); u16s(&mut ibag, 0);
    u16s(&mut igen, 54); u16s(&mut igen, 1);
    u16s(&mut igen, 53); u16s(&mut igen, 0);
    u16s(&mut igen, 0); u16s(&mut igen, 0);
    for (label, start, end, ls, le) in [("S", 0, points, 169, 1855), ("EOS", 0, 0, 0, 0)] {
        name(&mut shdr, label); u32s(&mut shdr, start); u32s(&mut shdr, end); u32s(&mut shdr, ls); u32s(&mut shdr, le);
        u32s(&mut shdr, 44_100); shdr.push(60); shdr.push(0); u16s(&mut shdr, 0); u16s(&mut shdr, 1);
    }
    let terminal_mod = vec![0u8; 10];
    let info = list(b"INFO", &[chunk(b"ifil", &[2, 0, 4, 0]), chunk(b"INAM", b"tiny\0")]);
    let sdta = list(b"sdta", &[chunk(b"smpl", &smpl)]);
    let pdta = list(b"pdta", &[
        chunk(b"phdr", &phdr), chunk(b"pbag", &pbag), chunk(b"pmod", &terminal_mod), chunk(b"pgen", &pgen),
        chunk(b"inst", &inst), chunk(b"ibag", &ibag), chunk(b"imod", &terminal_mod), chunk(b"igen", &igen), chunk(b"shdr", &shdr),
    ]);
    let mut body = b"sfbk".to_vec();
    body.extend(info);
    body.extend(sdta);
    body.extend(pdta);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

struct Font;
impl crate::build::Samples for Font {
    fn sample(&self, _: &crate::model::AssetRef) -> Option<std::sync::Arc<makepad_score_player::Sample>> {
        None
    }
    fn file(&self, _: &crate::model::AssetRef) -> Option<std::sync::Arc<[u8]>> {
        Some(tiny_sf2().into())
    }
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

#[test]
fn native_instruments_play_in_a_score() {
    // The modelled acoustic kit, every GM piece a game band uses.
    let drums = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet kit = Instrument{kind: @drums}\nlet band = Loop{play: kit beats: 4 code: fn(i) { for d in [@kick @pedalhat @bell @tom1 @tom2 @tom3 @floortom @snare] { hit(d) sleep(0.5) } }}\nScore{clock kit band}\n";
    let a = render(drums, &RenderOpts { seconds: Some(2.0), ..RenderOpts::default() }, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(peak(&a.left) > 0.05, "the modelled kit is silent");
    assert!(a.left.iter().position(|x| x.abs() > 1e-6).is_some_and(|at| at < 240), "the first hit sounds within 5 ms");
    // The modelled piano, down to its lowest octave.
    let piano = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet grand = Instrument{kind: @piano}\nlet low = Loop{play: grand beats: 4 code: fn(i) { play([21 28 33], {dur: 2}) sleep(4) }}\nScore{clock grand low}\n";
    let p = render(piano, &RenderOpts { seconds: Some(1.5), ..RenderOpts::default() }, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(peak(&p.left) > 0.01 && p.left.iter().all(|x| x.is_finite()), "piano peak {}", peak(&p.left));
    // A SoundFont program from the library.
    let sf = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet gm = Instrument{kind: @soundfont src: asset(\"font\", \"1\") program: 40}\nlet line = Loop{play: gm beats: 2 code: fn(i) { play(@c4, {dur: 1}) sleep(2) }}\nScore{clock gm line}\n";
    let silent = render(&sf.replace("\"font\"", "\"never_registered\""), &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    assert_eq!(peak(&silent.left), 0.0, "an unloaded SoundFont is silent, not an error");
    let s = render(sf, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &Font).unwrap();
    assert!(peak(&s.left) > 0.01, "the SoundFont is silent");
    assert!(model::load(&drums.replace("@drums", "@kazoo")).is_err());
}

#[test]
fn a_note_list_streams_with_program_changes() {
    // Four thousand notes and a program change, as an imported song.
    let mut notes = String::from("[0 @program 0] ");
    for i in 0..4000 {
        if i == 2000 {
            notes.push_str("[500 @program 40] ");
        }
        notes.push_str(&format!("[{} {} 0.2 0.7] ", i as f64 * 0.25, 48 + (i * 7) % 24));
    }
    let src = format!("use mod.score.*\nlet clock = Clock{{bpm: 240}}\nlet gm = Instrument{{kind: @soundfont src: asset(\"font\", \"1\")}}\nlet song = Loop{{play: gm notes: [{notes}]}}\nScore{{clock gm song}}\n");
    let mut composer = Composer::new(model::load(&src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    assert!(events.len() <= 20, "streamed up to the horizon, not the whole song: {}", events.len());
    composer.pump(1000.0, &mut events);
    let programs: Vec<u8> = events.iter().filter_map(|e| match e.kind { EventKind::Program(p) => Some(p), _ => None }).collect();
    assert_eq!(programs, [0, 40]);
    assert_eq!(events.iter().filter(|e| e.kind == EventKind::Note).count(), 4000);
    // It renders through the graph, the feed never overflowing.
    let r = render(&src, &RenderOpts { seconds: Some(4.0), ..RenderOpts::default() }, &Font).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(peak(&r.left) > 0.01);
}

#[test]
fn feeds_are_freed_with_their_music() {
    use crate::game::{music, Clocking};
    use makepad_audio_engine::feed::registry_len;
    let make = |clocking| {
        let (handle, inst) = music(PULSE_A, 48_000.0, clocking).unwrap();
        drop(inst);
        drop(handle);
    };
    make(Clocking::Headless);
    make(Clocking::Realtime);
    render(PULSE_A, &RenderOpts { seconds: Some(0.1), ..RenderOpts::default() }, &NoSamples).unwrap();
    let before = registry_len();
    for _ in 0..20 {
        make(Clocking::Headless);
        make(Clocking::Realtime);
        render(PULSE_A, &RenderOpts { seconds: Some(0.1), ..RenderOpts::default() }, &NoSamples).unwrap();
    }
    // Other tests run beside this one and hold feeds while they render:
    // allow what they hold, never growth with our repeats (a leak here is
    // at least 60 loop feeds: three instances a round, twenty rounds).
    let after = registry_len();
    assert!(after.0 <= before.0 + 20 && after.1 <= before.1 + 8, "registry grew from {before:?} to {after:?}");
}

const TEMPO: &str = "use mod.score.*\nlet clock = Clock{bpm: 100 params: {mood: 0}}\nlet s = Synth{preset: @pluck}\nlet pulse = Loop{play: s beats: 1 code: fn(i) { play(60, {dur: 0.1}) sleep(1) }}\nlet slow = Section{bars: 1 loops: [pulse]}\nlet fast = Section{bars: 1 loops: [pulse] bpm: 150}\nlet ramp = Section{bars: 2 loops: [pulse] bpm: 60 ramp: 4}\nScore{clock s pulse slow fast ramp arrange: [slow fast ramp]}\n";

fn onsets(v: &[f32]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut quiet = 0;
    for (i, x) in v.iter().enumerate() {
        if x.abs() > 1e-4 {
            if quiet > 2000 || out.is_empty() {
                out.push(i);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    out
}

#[test]
fn sections_change_the_tempo_sample_exactly() {
    let r = render(TEMPO, &RenderOpts { seconds: Some(9.0), ..RenderOpts::default() }, &NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
    // Each beat's note starts on the frame the tempo map puts that beat.
    // (Measured from the first note: the pluck's attack is the same each time.)
    let heard = onsets(&r.left);
    assert!(heard.len() >= 12, "{heard:?}");
    for (beat, at) in heard.iter().enumerate().take(12) {
        let want = (r.seconds_at(beat as f64) * 48_000.0).ceil() as usize + heard[0];
        assert!(at.abs_diff(want) <= 1, "beat {beat}: heard at {at}, the map says {want}");
    }
    // 4 beats at 100, 4 at 150, then down to 60.
    assert!((r.seconds_at(4.0) - 2.4).abs() < 1e-9);
    assert!((r.seconds_at(8.0) - 4.0).abs() < 1e-9);
    let t = crate::render::timing(TEMPO, Some(9.0)).unwrap();
    assert_eq!(t.sections[1].0, "fast");
    assert!((t.sections[1].1 - 2.4).abs() < 1e-9 && (t.sections[2].1 - 4.0).abs() < 1e-9);
    // A game's headless music in odd blocks plays the same samples, and
    // its beat events follow the map.
    use crate::game::{music, Clocking, MusicEvent};
    use makepad_audio_synth::Instrument;
    let (mut handle, mut inst) = music(TEMPO, 48_000.0, Clocking::Headless).unwrap();
    let total = r.left.len();
    let (mut out, mut beats) = (Vec::new(), Vec::new());
    let (mut done, mut i) = (0, 0);
    while done < total {
        let n = [700usize, 64, 333, 2048][i % 4].min(total - done);
        handle.advance(n);
        let (mut l, mut rr) = (vec![0.0; n], vec![0.0; n]);
        inst.render(&mut l, &mut rr);
        out.extend(l);
        for e in handle.poll() {
            if let MusicEvent::Beat { beat, .. } = e {
                beats.push((beat, done));
            }
        }
        done += n;
        i += 1;
    }
    let first = out.iter().zip(&r.left).position(|(a, b)| a != b);
    assert!(first.is_none(), "the game's music differs from the offline render at {first:?}");
    assert_eq!(beats.iter().map(|b| b.0).take(10).collect::<Vec<_>>(), (0..10).collect::<Vec<u64>>());
}

#[test]
fn a_param_moves_the_tempo_and_sections_repeat() {
    let src = TEMPO.replace("params: {mood: 0}}", "params: {mood: 0} tempo: {param: @mood from: 60 to: 180 ramp: 2}}");
    let mut composer = Composer::new(model::load(&src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(2.5, &mut events);
    composer.set_param("mood", 1.0);
    assert_eq!(composer.tempo_out.last().map(|e| (e.beat, e.bpm, e.ramp)), Some((3.0, 180.0, 2.0)));
    // A section repeats itself until a goto, forward references resolve.
    let looping = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet pulse = Loop{play: s beats: 1 code: fn(i) { play(60) sleep(1) }}\nlet intro = Section{bars: 1 loops: [pulse] next: @calm}\nlet calm = Section{bars: 1 loops: [pulse] repeat: true}\nlet storm = Section{bars: 1 loops: [pulse] next: @self}\nScore{clock s pulse intro calm storm arrange: [intro calm storm]}\n";
    let mut c = Composer::new(model::load(looping).unwrap_or_else(|d| panic!("{d:?}")));
    assert_eq!(c.section_at(2.0).as_deref(), Some("intro"));
    assert_eq!(c.section_at(40.0).as_deref(), Some("calm"));
    c.goto_section("storm", 40.0, -1.0).unwrap();
    assert_eq!(c.section_at(100.0).as_deref(), Some("storm"));
    assert!(model::load(&looping.replace("next: @calm", "next: @nowhere")).is_err());
}

#[test]
fn an_id_in_arithmetic_is_an_error_not_silence() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet bad = Loop{play: s beats: 1 code: fn(i) { let c = [@c4 @e4] play(at(c, 0) + 12) sleep(1) }}\nlet good = Loop{play: s beats: 1 code: fn(i) { play(note(@c4) + floor(12.7) + max(0, -3) + abs(-0)) sleep(1) }}\nScore{clock s bad good}\n";
    let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(2.0, &mut events);
    let errors = composer.errors();
    assert!(errors.iter().any(|e| e.node.as_deref() == Some("bad") && e.message.contains("not a number")), "{errors:?}");
    assert!(events.iter().all(|e| e.note.is_finite()));
    assert!(events.iter().any(|e| e.note == 72.0), "{events:?}");
    let r = render(src, &RenderOpts { seconds: Some(3.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    assert!(peak(&r.left[96_000..]) > 0.01, "the rest of the music plays");
}

#[test]
fn sends_bars_and_sections() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 4 sends: {verb: 0.5} code: fn(i) { if section() == \"b\" { play(60 + section_bar()) } else { play(48 + bar()) } sleep(4) }}\nlet verb = Reverb{room: 0.9 mix: 1.0}\nlet one = Section{bars: 2 loops: [a]}\nlet b = Section{bars: 2 loops: [a]}\nScore{clock s a verb one b arrange: [one b]}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.loops[0].sends, vec![("verb".to_string(), 0.5)]);
    // The send is a Bus into the reverb; the loop still reaches the master.
    let g = graph(&doc);
    assert_eq!((g.node("a_send_verb").type_name.as_str(), g.wired("a_send_verb", "audio"), g.param("a_send_verb", "gain")), ("Bus", vec!["a".to_string()], Literal::Num(0.5)));
    assert_eq!(g.wired("verb", "audio"), ["a_send_verb"]);
    assert_eq!(g.wired("master", "audio"), ["a", "verb"]);
    // bar() counts from the piece's start; section_bar() within a section.
    let mut composer = Composer::new(model::load(src).unwrap());
    let mut events = Vec::new();
    composer.pump(16.0, &mut events);
    let notes: Vec<f32> = events.iter().map(|e| e.note).collect();
    assert_eq!(notes, [48.0, 49.0, 60.0, 61.0]);
    assert!(composer.errors().is_empty());
    // The tail follows the reverb, and a render reports loop errors.
    assert!(crate::render::tail_seconds(&doc) > 5.0);
    let broken = src.replace("play(48 + bar())", "play(48 + nope())");
    let r = render(&broken, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    assert!(!r.errors.is_empty());
    assert!(model::load(&src.replace("{verb: 0.5}", "{verbb: 0.5}")).err().unwrap()[0].message.contains("did you mean `verb`"));
}

#[test]
fn fx_settings_follow_params_and_sections_set_them() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 params: {build: 0}}\nlet s = Synth{preset: @supersaw}\nlet a = Loop{play: s beats: 1 code: fn(i) { play(48, {dur: 1}) sleep(1) }}\nlet sweep = Lowpass{in: [a] cutoff: {param: @build from: 200 to: 8000 ramp: 4} res: 1}\nlet low = Section{bars: 1 loops: [a]}\nlet up = Section{bars: 2 loops: [a] set: {build: 1}}\nScore{clock s a sweep low up arrange: [low up]}\n";
    let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
    // It starts where the param is, and the section moves it over 4 beats.
    assert_eq!(composer.automation_out.drain(..).map(|(fx, e)| (fx, e.beat, e.value, e.ramp)).collect::<Vec<_>>(), [("sweep".to_string(), 0.0, 200.0, 0.0)]);
    let mut events = Vec::new();
    composer.pump(8.0, &mut events);
    assert_eq!(composer.automation_out.iter().map(|(_, e)| (e.beat, e.value, e.ramp)).collect::<Vec<_>>(), [(4.0, 8000.0, 4.0)]);
    // In the render the high bands open up after the section starts.
    let r = render(src, &RenderOpts { seconds: Some(6.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    let hf = |from: usize, to: usize| r.left[from..to].windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / (to - from) as f32;
    assert!(hf(4 * 24_000, 5 * 24_000) > hf(24_000, 2 * 24_000) * 2.0, "the sweep did not open: {} vs {}", hf(24_000, 48_000), hf(96_000, 120_000));
    assert!(model::load(&src.replace("set: {build: 1}", "set: {buld: 1}")).is_err());
}

#[test]
fn the_loop_code_toolkit() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet chords = Loop{play: s beats: 4 code: fn(i) { put(@chord, chord(@a3, @minor)) log(rev([1 2 3])) sleep(4) }}\nlet lead = Loop{play: s beats: 4 humanize: {time: 0.02 vel: 0.2} code: fn(i) { let c = get(@chord) sleep(phrase([c[0] c[1]], [0.5 0.5])) sleep(arp(c, @updown, 0.25)) strum(c, 0.05) log(deg(3, @c4, @major)) log(snap(61, @c4, @major)) log(rot([1 2 3], 1)) sleep(1) }}\nScore{clock s chords lead}\n";
    let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    let lead: Vec<f32> = events.iter().filter(|e| e.loop_key == key("lead")).map(|e| e.note).collect();
    // phrase a3 c4, arp up-down a3 c4 e4 c4, strum a3 c4 e4.
    assert_eq!(lead, [57.0, 60.0, 57.0, 60.0, 64.0, 60.0, 57.0, 60.0, 64.0]);
    let logs = &composer.loops.iter().find(|l| l.name == "lead").unwrap().last_logs;
    assert_eq!(logs.iter().map(|l| l.split(": ").nth(1).unwrap().to_string()).collect::<Vec<_>>(), ["64", "60", "[2 3 1]"]);
    // Humanize moves notes off the grid, the same way every time.
    let beats: Vec<f64> = events.iter().filter(|e| e.loop_key == key("lead")).map(|e| e.beat).collect();
    assert!(beats.iter().any(|b| (b * 4.0 - (b * 4.0).round()).abs() > 1e-4));
    let mut again = Composer::new(model::load(src).unwrap());
    let mut events2 = Vec::new();
    again.pump(4.0, &mut events2);
    assert_eq!(events.iter().map(|e| e.beat).collect::<Vec<_>>(), events2.iter().map(|e| e.beat).collect::<Vec<_>>());
}

#[test]
fn a_playing_note_glides_its_pitch_level_and_cutoff() {
    // A fixed recipe (the presets' sound design may change; the glide may not).
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{recipe: \"saw freq=440 detune=12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=220 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.1 lp=3000\"}\nlet a = Loop{play: s beats: 4 code: fn(i) { let h = play(@a2, {dur: 4}) sleep(1) glide(h, {by: 12 cutoff: 6000}, 1) sleep(3) }}\nScore{clock s a}\n";
    let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    // play() hands out the handle the glide names.
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, EventKind::Glide);
    assert!(events[0].opts.handle != 0 && events[0].opts.handle == events[1].opts.handle);
    assert_eq!((events[1].beat, events[1].dur, events[1].opts.glide_from), (1.0, 1.0, 12.0));
    // The held note is an octave up once the glide is over.
    let r = render(src, &RenderOpts { seconds: Some(1.9), ..RenderOpts::default() }, &NoSamples).unwrap();
    let crossings = |from: f64, to: f64| {
        let s = &r.left[(from * 48_000.0) as usize..(to * 48_000.0) as usize];
        s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32
    };
    let (before, after) = (crossings(0.1, 0.45), crossings(1.1, 1.45));
    assert!(after > before * 1.6, "no octave glide: {before} -> {after} crossings");
    // A second play gives a new handle; a glide on none is ignored.
    let other = src.replace("glide(h,", "glide(0,");
    let mut composer = Composer::new(model::load(&other).unwrap());
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    assert_eq!(events.len(), 1);
}

#[test]
fn samples_slice_at_onsets_loop_reverse_fit_and_stretch() {
    use crate::build::Samples;
    use crate::model::AssetRef;
    use makepad_score_player::Sample;
    use std::sync::Arc;
    // Four 440 Hz hits a quarter second apart, each 0.1 s, in one second.
    fn hits() -> Vec<f32> {
        (0..48_000).map(|i| {
            let t = (i % 12_000) as f32 / 48_000.0;
            if t < 0.1 { (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * (1.0 - t * 5.0) } else { 0.0 }
        }).collect()
    }
    struct Hits;
    impl Samples for Hits {
        fn sample(&self, _: &AssetRef) -> Option<Arc<Sample>> {
            Some(Arc::new(Sample::new(hits().into(), None, 48_000.0)))
        }
    }
    let onsets = Sample::new(hits().into(), None, 48_000.0).onsets.to_vec();
    assert!(onsets.len() == 4 && onsets.iter().zip([0, 12_000, 24_000, 36_000]).all(|(g, w)| g.abs_diff(w) < 32), "{onsets:?}");
    // 120 bpm: a beat is half a second. `call` plays once at the start.
    let run = |call: &str| {
        let src = format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet s = Sampler{{src: asset(\"hits\", \"1\") root: @a4}}\nlet a = Loop{{play: s beats: 16 code: fn(i) {{ {call} sleep(16) }}}}\nScore{{clock s a}}\n");
        let r = render(&src, &RenderOpts { seconds: Some(2.4), ..RenderOpts::default() }, &Hits).unwrap_or_else(|d| panic!("{d:?}"));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        r.left
    };
    // Loud 10 ms windows, as times; and zero crossings per second between two times.
    let loud = |v: &[f32]| (0..v.len() / 480).filter(|w| v[w * 480..(w + 1) * 480].iter().any(|x| x.abs() > 0.02)).map(|w| w as f32 * 0.01).collect::<Vec<f32>>();
    let hz = |v: &[f32], a: f32, b: f32| {
        let s = &v[(a * 48_000.0) as usize..(b * 48_000.0) as usize];
        s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32 / 2.0 / (b - a)
    };
    let starts = |t: &[f32]| t.iter().enumerate().filter(|(i, x)| *i == 0 || **x - t[i - 1] > 0.05).map(|(_, x)| *x).collect::<Vec<f32>>();
    let near = |got: &[f32], want: &[f32]| got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 0.03);

    let plain = loud(&run("sample()"));
    assert!(near(&starts(&plain), &[0.0, 0.25, 0.5, 0.75]), "{:?}", starts(&plain));
    // The third onset alone.
    let third = loud(&run("sample(nil, {onset: 2})"));
    assert!(near(&starts(&third), &[0.0]) && *third.last().unwrap() < 0.12, "{third:?}");
    // Slice 1 of 4, looped for two beats: the hit every quarter second.
    let looped = loud(&run("sample(nil, {slice: 1 slices: 4 looped: true dur: 1.9})"));
    assert!(near(&starts(&looped), &[0.0, 0.25, 0.5, 0.75]), "{:?}", starts(&looped));
    // Backwards: the last hit's tail first, ending on the first hit.
    let back = loud(&run("sample(nil, {rate: -1})"));
    assert!((starts(&back)[0] - 0.15).abs() <= 0.03 && (back.last().unwrap() - 0.99).abs() <= 0.03, "{back:?}");
    // Fit to 4 beats (2 s): slower and lower. Stretch to 4 beats: slower, same pitch.
    let fit = run("sample(nil, {fit: 4})");
    assert!(near(&starts(&loud(&fit)), &[0.0, 0.5, 1.0, 1.5]), "{:?}", starts(&loud(&fit)));
    assert!((hz(&fit, 0.02, 0.15) - 220.0).abs() < 20.0, "{}", hz(&fit, 0.02, 0.15));
    let stretch = run("sample(nil, {stretch: 4})");
    let st = starts(&loud(&stretch));
    assert!(near(&st, &[0.0, 0.5, 1.0, 1.5]), "{st:?}");
    assert!((hz(&stretch, 0.52, 0.65) - 440.0).abs() < 30.0, "{}", hz(&stretch, 0.52, 0.65));
}

#[test]
fn a_loop_waits_for_another_loops_cue() {
    // The follower is written first; the loop that cues still runs first.
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet follow = Loop{play: s code: fn(i) { wait(@drop) play(60) sleep(1) }}\nlet lead = Loop{play: s beats: 4 code: fn(i) { sleep(1.5) cue(@drop) play(48) sleep(2.5) }}\nScore{clock s follow lead}\n";
    let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(12.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    let at = |note: f32| events.iter().filter(|e| e.note == note).map(|e| e.beat).collect::<Vec<f64>>();
    assert_eq!(at(48.0), [1.5, 5.5, 9.5]);
    // Its cycle from 10.5 waits for the cue the lead gives next (at 13.5).
    assert_eq!(at(60.0), [1.5, 5.5, 9.5, 13.5]);
    // Waiting for a cue nothing gives says so.
    let mut composer = Composer::new(model::load(&src.replace("wait(@drop)", "wait(@dorp)")).unwrap());
    composer.pump(4.0, &mut Vec::new());
    let errors = composer.errors();
    assert!(errors.iter().any(|e| e.message.contains("nothing cues @dorp") && e.message.contains("@drop")), "{errors:?}");
}

#[test]
fn fx_scopes_play_their_notes_through_their_own_effects() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 8 code: fn(i) {\n    play(72, {dur: 0.1})\n    fx(@reverb, {room: 0.95 mix: 1.0}, fn() {\n        play(60, {dur: 0.1})\n        fx(@echo, {time: 0.5}, fn() { play(64, {dur: 0.1}) })\n    })\n    sleep(8)\n}}\nScore{clock s a}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    use makepad_score_player::FxKind;
    assert_eq!(doc.loops[0].scopes, vec![vec![FxKind::Reverb], vec![FxKind::Reverb, FxKind::Delay]]);
    assert_eq!(doc.feed_names(), ["a", "a_fx1_in", "a_fx2_in"]);
    // Inner effects first; the outer reads its own notes and the inner.
    let g = graph(&doc);
    assert_eq!((g.node("a_fx2").type_name.as_str(), g.wired("a_fx2", "audio")), ("Delay", vec!["a_fx2_in".to_string()]));
    assert_eq!((g.node("a_fx1").type_name.as_str(), g.wired("a_fx1", "audio")), ("Reverb", vec!["a_fx1_in".to_string(), "a_fx2".to_string()]));
    assert!(g.index("a_fx2") < g.index("a_fx1"));
    assert_eq!(g.wired("master", "audio"), ["a", "a_fx1"]);
    // Each note on its node; the settings to each effect from the beat.
    let mut composer = Composer::new(model::load(src).unwrap());
    let mut events = Vec::new();
    composer.pump(8.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    let on = |note: f32| events.iter().find(|e| e.note == note).map(|e| e.loop_key);
    assert_eq!((on(72.0), on(60.0), on(64.0)), (Some(key("a")), Some(key("a_fx1_in")), Some(key("a_fx2_in"))));
    let sets: Vec<(String, u8, f32)> = composer.automation_out.iter().map(|(fx, e)| (fx.clone(), e.param, e.value)).collect();
    assert_eq!(sets, [("a_fx1".to_string(), 0, 0.95), ("a_fx1".to_string(), 1, 1.0), ("a_fx2".to_string(), 0, 0.5)]);
    // The reverb rings on after the notes; without fx() it does not.
    let tail = |src: &str| {
        let r = render(src, &RenderOpts { seconds: Some(3.0), ..RenderOpts::default() }, &NoSamples).unwrap();
        peak(&r.left[(2.0 * 48_000.0) as usize..])
    };
    let dry = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 8 code: fn(i) { play(72, {dur: 0.1}) play(60, {dur: 0.1}) play(64, {dur: 0.1}) sleep(8) }}\nScore{clock s a}\n";
    assert!(tail(src) > tail(dry) * 4.0 + 1e-4, "{} vs {}", tail(src), tail(dry));
    // An unknown effect is a load error.
    assert!(model::load(&src.replace("@echo", "@wobble")).err().unwrap()[0].message.contains("fx(@wobble): no such effect"));
}

#[test]
fn custom_envelopes_shape_a_notes_level_and_cutoff() {
    let env = makepad_score_player::Env { pts: { let mut p = [[0.0; 2]; 8]; p[0] = [0.0, 0.0]; p[1] = [1.0, 1.0]; p[2] = [2.0, 0.5]; p }, n: 3, curve: 1.0 };
    assert_eq!((env.level(0.5), env.level(1.5), env.level(9.0)), (0.5, 0.75, 0.5));
    let run = |opts: &str| {
        let src = format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet s = Synth{{recipe: \"saw freq=440 detune=12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=220 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.1 lp=3000\"}}\nlet a = Loop{{play: s beats: 8 code: fn(i) {{ play(@a2, {opts}) sleep(8) }}}}\nScore{{clock s a}}\n");
        render(&src, &RenderOpts { seconds: Some(1.6), ..RenderOpts::default() }, &NoSamples).unwrap_or_else(|d| panic!("{d:?}")).left
    };
    let rms = |v: &[f32], a: f32, b: f32| {
        let s = &v[(a * 48_000.0) as usize..(b * 48_000.0) as usize];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    // A swell: up over a beat, down to nothing by the second; the note
    // lasts as long as its envelope.
    let swell = run("{env: [[0 0] [1 1] [2 0]]}");
    let (start, top, end) = (rms(&swell, 0.0, 0.05), rms(&swell, 0.45, 0.55), rms(&swell, 1.1, 1.6));
    assert!(top > start * 4.0 && top > end * 20.0, "swell {start} {top} {end}");
    // A cutoff opening over two beats: brighter at the end.
    let hf = |v: &[f32], a: f32, b: f32| {
        let s = &v[(a * 48_000.0) as usize..(b * 48_000.0) as usize];
        s.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / s.iter().map(|x| x.abs()).sum::<f32>().max(1e-9)
    };
    let open = run("{dur: 4 cutoff_env: [0 150 2 9000]}");
    assert!(hf(&open, 0.8, 0.95) > hf(&open, 0.1, 0.25) * 2.0, "{} vs {}", hf(&open, 0.1, 0.25), hf(&open, 0.8, 0.95));
}

#[test]
fn midi_keys_play_loops_move_params_and_reach_loop_code_and_outboard_gear() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 params: {bright: 0} midi: {bright: 74}}\nlet s = Synth{preset: @pluck}\nlet gear = Instrument{kind: @midi channel: 2 port: \"IAC Bus 1\"}\nlet keys = Loop{play: s midi_in: true}\nlet arp = Loop{play: gear beats: 1 code: fn(i) { log(held()) log(cc(74)) sleep(arp(held(), @up, 0.25)) }}\nlet sweep = Lowpass{in: [keys] cutoff: {param: @bright from: 200 to: 8000}}\nScore{clock s gear keys arp sweep}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.loops[0].midi_in, Some(0));
    assert_eq!(doc.clock.midi, vec![("bright".to_string(), 74)]);
    let g = graph(&doc);
    assert_eq!((g.param("arp", "inst"), g.param("arp", "channel"), g.param("arp", "midi_port")), (Literal::Str("midi".into()), Literal::Num(2.0), Literal::Str("IAC Bus 1".into())));
    let mut composer = Composer::new(model::load(src).unwrap());
    composer.automation_out.clear();
    // Keys: played on the keyboard loop at once, held for the loop code.
    let mut now = Vec::new();
    composer.midi_in([0x90, 60, 100], 3.0, &mut now);
    composer.midi_in([0x90, 64, 127], 3.0, &mut now);
    composer.midi_in([0xb0, 74, 127], 3.0, &mut now);
    assert_eq!(now.iter().map(|e| (e.beat, e.loop_key, e.kind, e.note)).collect::<Vec<_>>(), [(3.0, key("keys"), EventKind::Note, 60.0), (3.0, key("keys"), EventKind::Note, 64.0)]);
    assert!(now[0].opts.handle != 0 && now[0].opts.handle != now[1].opts.handle);
    // The controller moved the param and the filter following it, now.
    assert_eq!(composer.params, [("bright".to_string(), 1.0)]);
    assert_eq!(composer.automation_out.iter().map(|(fx, e)| (fx.as_str(), e.beat, e.value)).collect::<Vec<_>>(), [("sweep", 3.0, 8000.0)]);
    let mut events = Vec::new();
    composer.pump(1.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    let logs = &composer.loops.iter().find(|l| l.name == "arp").unwrap().last_logs;
    assert_eq!(logs.iter().map(|l| l.split(": ").nth(1).unwrap().to_string()).collect::<Vec<_>>(), ["[60 64]", "1"]);
    assert_eq!(events.iter().filter(|e| e.loop_key == key("arp")).map(|e| (e.beat, e.note)).collect::<Vec<_>>(), [(0.0, 60.0), (0.25, 64.0)]);
    // A key let go ends its note.
    let mut off = Vec::new();
    composer.midi_in([0x80, 60, 0], 4.0, &mut off);
    assert_eq!((off[0].kind, off[0].opts.handle), (EventKind::Release, now[0].opts.handle));
    assert_eq!(composer.held.len(), 1);
    // Only a live score's MIDI loops reach the outputs.
    use makepad_audio_engine::feed::midi_out;
    midi_out::set_live("stage_score", true);
    assert!(midi_out::is_live("stage_score/arp") && !midi_out::is_live("stage_score_x/arp") && !midi_out::is_live("render_1/arp"));
    midi_out::set_live("stage_score", false);
    assert!(!midi_out::is_live("stage_score/arp"));
}

#[test]
fn osc_moves_params_reaches_loop_code_and_goes_out_on_its_beat() {
    use crate::osc::{decode, OscArg};
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 params: {wet: 0} osc_in: 9000 osc_out: \"127.0.0.1:57120\"}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 2 code: fn(i) { log(osc(\"/fader1\")) sleep(1) osc_send(\"/beat\", [i \"one\"]) osc_send(\"/x\", [], \"127.0.0.1:9001\") sleep(1) }}\nlet verb = Reverb{in: [a] mix: {param: @wet from: 0 to: 1}}\nScore{clock s a verb}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!((doc.clock.osc_in, doc.clock.osc_out.as_deref()), (Some(9000), Some("127.0.0.1:57120")));
    let mut composer = Composer::new(model::load(src).unwrap());
    composer.automation_out.clear();
    composer.osc_in("/fader1", vec![OscArg::Float(0.25)], 0.0);
    composer.osc_in("/param/wet", vec![OscArg::Int(1)], 0.5);
    assert_eq!(composer.params, [("wet".to_string(), 1.0)]);
    assert_eq!(composer.automation_out.iter().map(|(fx, e)| (fx.as_str(), e.beat, e.value)).collect::<Vec<_>>(), [("verb", 0.5, 1.0)]);
    let mut events = Vec::new();
    composer.pump(2.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    assert_eq!(composer.loops[0].last_logs[0].split(": ").nth(1), Some("0.25"));
    let sent: Vec<(f64, &str, String, Vec<OscArg>)> = composer.osc_out.iter().map(|(beat, to, packet)| {
        let (addr, args) = decode(packet).remove(0);
        (*beat, to.as_str(), addr, args)
    }).collect();
    assert_eq!(sent, [
        (1.0, "127.0.0.1:57120", "/beat".to_string(), vec![OscArg::Float(0.0), OscArg::Str("one".into())]),
        (1.0, "127.0.0.1:9001", "/x".to_string(), vec![]),
    ]);
    // With nowhere to send, the loop says so.
    let mut composer = Composer::new(model::load(&src.replace(" osc_out: \"127.0.0.1:57120\"", "")).unwrap());
    composer.pump(2.0, &mut Vec::new());
    assert!(composer.errors().iter().any(|e| e.message.contains("osc_send(\"/beat\"): where to?")), "{:?}", composer.errors());
}

#[test]
fn an_input_loop_passes_the_audio_input_through_its_fx() {
    use makepad_audio_engine::feed::audio_in;
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet mic = Instrument{kind: @input gain: 0.5}\nlet voice = Loop{play: mic}\nlet echo = Delay{in: [voice] time: 0.25 feedback: 0.3 mix: 0.3}\nScore{clock mic voice echo}\n";
    let loaded = model::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let g = graph(&loaded.doc);
    assert_eq!((g.param("voice", "inst"), g.param("voice", "gain")), (Literal::Str("input".into()), Literal::Num(0.5)));
    let mut instance = instantiate(&loaded.doc, 48_000.0).unwrap();
    let mut composer = Composer::new(loaded);
    composer.pump(4.0, &mut Vec::new());
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    // A 440 Hz tone arrives, a chunk ahead of each pass.
    let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5).collect();
    let mut out = Vec::new();
    let mut at = 0;
    while out.len() < 24_000 {
        audio_in::push(48_000.0, &tone[at..at + CHUNK], &tone[at..at + CHUNK]);
        at += CHUNK;
        run(&mut instance.schedule, CHUNK, &mut |_, a, _| out.extend_from_slice(&a[..CHUNK]), &mut NoExt);
    }
    let crossings = out[4_800..].windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32 / 2.0 / ((out.len() - 4_800) as f32 / 48_000.0);
    assert!(peak(&out[4_800..]) > 0.1 && (crossings - 440.0).abs() < 15.0, "peak {} at {crossings} Hz", peak(&out[4_800..]));
    // At another rate than the graph's it stays silent.
    audio_in::push(44_100.0, &tone[..CHUNK], &tone[..CHUNK]);
    let mut quiet = Vec::new();
    // (after the echo's tail)
    for _ in 0..400 {
        audio_in::push(44_100.0, &tone[..CHUNK], &tone[..CHUNK]);
        run(&mut instance.schedule, CHUNK, &mut |_, a, _| quiet.extend_from_slice(&a[..CHUNK]), &mut NoExt);
    }
    assert!(peak(&quiet[quiet.len() - 2 * CHUNK..]) < 1e-3);
}

#[test]
fn seek_and_loop_move_the_arrangement_on_the_bar_grid() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 4 code: fn(i) { if section() == \"x\" { play(60) } else if section() == \"y\" { play(62) } else { play(64) } sleep(4) }}\nlet x = Section{bars: 2 loops: [a]}\nlet y = Section{bars: 2 loops: [a]}\nlet z = Section{bars: 2 loops: [a]}\nScore{clock s a x y z arrange: [x y z]}\n";
    let notes = |composer: &mut Composer, until: f64| {
        let mut events = Vec::new();
        composer.pump(until, &mut events);
        events.iter().map(|e| (e.beat, e.note)).collect::<Vec<_>>()
    };
    // As arranged: x x y y z z.
    let mut c = Composer::new(model::load(src).unwrap());
    assert_eq!(notes(&mut c, 24.0), [(0.0, 60.0), (4.0, 60.0), (8.0, 62.0), (12.0, 62.0), (16.0, 64.0), (20.0, 64.0)]);
    assert_eq!((c.song_beat(10.0), c.song_beat(30.0)), (Some(10.0), None));
    // A seek to z's start from bar 2: z plays from there, and the song
    // position follows.
    let mut c = Composer::new(model::load(src).unwrap());
    assert_eq!(notes(&mut c, 4.0), [(0.0, 60.0)]);
    assert_eq!(c.seek(16.0, 3.0, -1.0), 4.0);
    assert_eq!(notes(&mut c, 12.0), [(4.0, 64.0), (8.0, 64.0)]);
    assert_eq!(c.song_beat(5.0), Some(17.0));
    // Mid-section: from z's second bar.
    let mut c = Composer::new(model::load(src).unwrap());
    c.seek(20.0, 0.0, 0.0);
    assert_eq!(notes(&mut c, 8.0), [(0.0, 64.0), (4.0, 64.0)]);
    assert_eq!((c.song_beat(2.0), c.song_beat(6.0)), (Some(22.0), None));
    // Looping y: x x y y y y …
    let mut c = Composer::new(model::load(src).unwrap());
    c.set_loop(Some((8.0, 16.0)));
    assert_eq!(notes(&mut c, 32.0), [(0.0, 60.0), (4.0, 60.0), (8.0, 62.0), (12.0, 62.0), (16.0, 62.0), (20.0, 62.0), (24.0, 62.0), (28.0, 62.0)]);
    assert_eq!(c.song_beat(21.0), Some(13.0));
    // A cycle crossing the loop end is cut there (an 8-beat loop).
    let long = src.replace("beats: 4 code: fn(i) {", "beats: 8 code: fn(i) { play(50) sleep(4)");
    let mut c = Composer::new(model::load(&long).unwrap());
    c.set_loop(Some((0.0, 4.0)));
    let got = notes(&mut c, 12.0);
    assert!(got.iter().all(|(b, _)| (b % 4.0).abs() < 1e-9) && got.iter().filter(|(_, n)| *n == 50.0).count() == 3, "{got:?}");
}

#[test]
fn a_note_lists_voices_mute_and_solo() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet p = Synth{preset: @pluck}\nlet rh = [[0 @c5 1] [1 @e5 1]]\nlet lh = [[0 @c3 2]]\nlet piece = Loop{play: p beats: 2 notes: [rh lh]}\nScore{clock p piece}\n";
    let notes = |src: &str| {
        let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
        doc.loops[0].notes.as_ref().unwrap().iter().map(|n| n.note).collect::<Vec<f32>>()
    };
    assert_eq!(notes(src), [72.0, 48.0, 76.0]);
    assert_eq!(notes(&src.replace("notes: [rh lh]", "notes: [rh lh] mute_voices: [@lh]")), [72.0, 76.0]);
    assert_eq!(notes(&src.replace("notes: [rh lh]", "notes: [rh lh] solo_voices: [@lh]")), [48.0]);
}

#[test]
fn a_seek_without_an_arrangement_resumes_every_loop_mid_cycle() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 8 code: fn(i) { for k in 0..8 { play(60 + k) sleep(1) } }}\nlet b = Loop{play: s beats: 4 code: fn(i) { play(40) sleep(4) }}\nScore{clock s a b}\n";
    let mut c = Composer::new(model::load(src).unwrap());
    let mut events = Vec::new();
    c.pump(4.0, &mut events);
    // From beat 4, jump to song beat 10: `a` is 2 beats into its cycle, `b` 2.
    c.seek(10.0, 4.0, 0.0);
    let mut events = Vec::new();
    c.pump(8.0, &mut events);
    let a: Vec<(f64, f32)> = events.iter().filter(|e| e.loop_key == key("a")).map(|e| (e.beat, e.note)).collect();
    let b: Vec<f64> = events.iter().filter(|e| e.loop_key == key("b")).map(|e| e.beat).collect();
    assert_eq!(a, [(4.0, 62.0), (5.0, 63.0), (6.0, 64.0), (7.0, 65.0), (8.0, 66.0), (9.0, 67.0)]);
    assert_eq!(b, [6.0]);
    assert_eq!(c.song_beat(5.0), Some(11.0));
}

#[test]
fn a_limiter_node_and_the_masters_ceiling() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @supersaw}\nlet a = Loop{play: s beats: 1 code: fn(i) { play([@c3 @e3 @g3 @c4], {dur: 1}) sleep(1) }}\nlet glue = Bus{in: [a] gain: 2}\nlet lim = Limiter{in: [glue] ceiling: -6 gain: 2}\nScore{clock s a glue lim ceiling: -1.5}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.ceiling, -1.5);
    let g = graph(&doc);
    assert_eq!((g.node("lim").type_name.as_str(), g.wired("lim", "audio"), g.param("lim", "ceiling"), g.param("lim", "gain")), ("Limiter", vec!["glue".to_string()], Literal::Num(-6.0), Literal::Num(2.0)));
    assert_eq!((g.param("master", "ceiling"), g.param("master", "gain")), (Literal::Num(-1.5), Literal::Num(1.0)));
    let r = render(src, &RenderOpts { seconds: Some(2.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    let limit = 10f32.powf(-6.0 / 20.0);
    assert!(peak(&r.left[4800..]) <= limit * 1.02, "{} > {limit}", peak(&r.left[4800..]));
}

#[test]
fn eq_width_and_a_keyed_comp_do_what_they_say() {
    use makepad_score_player::fx::{eq_response_db, FxKind, FxState, MAX_FX_PARAMS};
    let rate = 48_000.0;
    // The Eq's curve: a +6 dB peak at 1 kHz, a -12 dB low shelf at 100 Hz.
    let mut p = FxKind::Eq.defaults();
    p[0..4].copy_from_slice(&[2.0, 100.0, -12.0, 0.7]);
    p[4..8].copy_from_slice(&[1.0, 1000.0, 6.0, 1.0]);
    assert!((eq_response_db(&p, 1000.0, rate) - 6.0).abs() < 0.5, "{}", eq_response_db(&p, 1000.0, rate));
    assert!((eq_response_db(&p, 30.0, rate) + 12.0).abs() < 1.0, "{}", eq_response_db(&p, 30.0, rate));
    assert!(eq_response_db(&p, 10_000.0, rate).abs() < 0.5);
    // Processed: a 1 kHz tone comes out 6 dB up.
    let tone = |hz: f32, n: usize| (0..n).map(|i| (i as f32 * hz * std::f32::consts::TAU / rate).sin() * 0.25).collect::<Vec<f32>>();
    let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
    let mut eq = FxState::new(FxKind::Eq, rate);
    let (mut l, mut r) = (tone(1000.0, 9600), tone(1000.0, 9600));
    eq.process(FxKind::Eq, &p, &mut l, &mut r, rate, 0.5, 0.0, None);
    let db = 20.0 * (rms(&l[4800..]) / rms(&tone(1000.0, 9600)[4800..])).log10();
    assert!((db - 6.0).abs() < 0.5, "{db}");
    // Width 0 folds to mono; bass mono keeps the lows' side out.
    let mut w = FxState::new(FxKind::Width, rate);
    let mut wp = [0.0; MAX_FX_PARAMS];
    wp[..4].copy_from_slice(&[0.0, 0.0, 0.0, 0.0]);
    let (mut l, mut r) = (tone(440.0, 4800), vec![0.0; 4800]);
    w.process(FxKind::Width, &wp, &mut l, &mut r, rate, 0.5, 0.0, None);
    assert!(l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1e-6));
    wp[..4].copy_from_slice(&[1.0, 0.0, 0.0, 200.0]);
    let mut w = FxState::new(FxKind::Width, rate);
    let (mut l, mut r) = (tone(50.0, 48_000), tone(50.0, 48_000).iter().map(|x| -x).collect::<Vec<f32>>());
    w.process(FxKind::Width, &wp, &mut l, &mut r, rate, 0.5, 0.0, None);
    let side = l[24_000..].iter().zip(&r[24_000..]).map(|(a, b)| ((a - b) * 0.5).powi(2)).sum::<f32>().sqrt();
    let before = tone(50.0, 48_000)[24_000..].iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(side < before * 0.2, "{side} vs {before}");
    // A Comp keyed by a loud kick reduces a quiet pad; unkeyed it does not.
    let mut cp = FxKind::Comp.defaults();
    cp[..6].copy_from_slice(&[-20.0, 8.0, 0.0, 1.0, 100.0, 0.0]);
    let quiet = tone(220.0, 4800).iter().map(|x| x * 0.1).collect::<Vec<f32>>();
    let loud = tone(60.0, 4800).iter().map(|x| x * 3.0).collect::<Vec<f32>>();
    let mut c = FxState::new(FxKind::Comp, rate);
    let (mut l, mut r) = (quiet.clone(), quiet.clone());
    c.process(FxKind::Comp, &cp, &mut l, &mut r, rate, 0.5, 0.0, None);
    assert!(c.reduction_db() > -0.1);
    let mut c = FxState::new(FxKind::Comp, rate);
    let (mut l, mut r) = (quiet.clone(), quiet.clone());
    c.process(FxKind::Comp, &cp, &mut l, &mut r, rate, 0.5, 0.0, Some((&loud, &loud)));
    assert!(c.reduction_db() < -6.0 && rms(&l[2400..]) < rms(&quiet[2400..]) * 0.5, "{}", c.reduction_db());
    // In a score: bands, width and a keyed comp load and lower.
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @warm_pad}\nlet kit = Drums{kit: @analog_hybrid}\nlet pad = Loop{play: s beats: 4 code: fn(i) { play([@c3 @g3], {dur: 4}) sleep(4) }}\nlet kick = Loop{play: kit hits: \"x...\"}\nlet tone = Eq{in: [pad] b1: [@highpass 80] b2: [@peak, 900, -3, 1.4] b4: [@highshelf 9000 2]}\nlet wide = Width{in: [tone] width: 1.4 bass_mono: 150}\nlet glue = Comp{in: [wide] key: kick threshold: -24 ratio: 6 attack: 2 release: 180 knee: 3}\nScore{clock s kit pad kick tone wide glue}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let eq = doc.fx.iter().find(|f| f.name == "tone").unwrap();
    assert_eq!(&eq.params[..8], &[4.0, 80.0, 0.0, 0.7, 1.0, 900.0, -3.0, 1.4]);
    let g = graph(&doc);
    assert_eq!((g.wired("glue", "audio"), g.wired("glue", "key")), (vec!["wide".to_string()], vec!["kick".to_string()]));
    let r = render(src, &RenderOpts { seconds: Some(2.0), ..RenderOpts::default() }, &NoSamples).unwrap();
    assert!(peak(&r.left) > 0.01 && r.left.iter().all(|x| x.is_finite()));
    assert!(model::load(&src.replace("@peak, 900", "@bump, 900")).is_err());
}

/// A kit's per-slot gains and tunings reach its loop's node (a drum tuned
/// down plays at a pitch below 1).
#[test]
fn kit_slots_keep_their_gains_and_tunings() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet kit = Drums{kit: @analog_hybrid tune: {kick: -12 snare: 7} gain: {kick: 0.5}}\nlet beat = Loop{play: kit hits: \"x...\"}\nScore{clock kit beat}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let g = graph(&doc);
    let list = |param: &str| match g.param("beat", param) {
        Literal::Arr(items) => items.iter().take(2).cloned().collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    assert_eq!(list("kit_gain"), [Literal::Num(0.5), Literal::Num(1.0)]);
    assert_eq!(list("kit_pitch")[0], Literal::Num(0.5));
}

#[test]
fn list_notes_bend_vibrate_arpeggiate_and_samples_hold_their_loop() {
    use crate::build::Samples;
    use crate::model::AssetRef;
    use makepad_score_player::Sample;
    use std::sync::Arc;
    // Four looped cycles of a 400 Hz sine: without its loop it is over
    // in 10 ms; with it, it holds as long as the note.
    struct Cycle;
    impl Samples for Cycle {
        fn sample(&self, _asset: &AssetRef) -> Option<Arc<Sample>> {
            let left: Vec<f32> = (0..480).map(|i| (i as f32 * 400.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5).collect();
            Some(Arc::new(Sample::new(left.into(), None, 48_000.0).with_loop(0, 480)))
        }
    }
    let score = |opts: &str| format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet s = Sampler{{src: asset(\"cycle\", \"1\") root: @a4}}\nlet l = Loop{{play: s beats: 4 notes: [[0 @a4 2 1{opts}]]}}\nScore{{clock s l}}\n");
    let opts = RenderOpts { seconds: Some(1.0), ..RenderOpts::default() };
    // Rising zero crossings in 0.4..0.6 s: the pitch half a second in.
    let hz = |v: &[f32]| v[19_200..28_800].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32 / 0.2;
    let plain = render(&score(""), &opts, &Cycle).unwrap();
    assert!((hz(&plain.left) - 400.0).abs() < 15.0, "{}", hz(&plain.left));
    // The loop holds it: still sounding at 0.9 s.
    assert!(plain.left[43_000..43_500].iter().any(|x| x.abs() > 0.1));
    // bend: up an octave by beat 0.5 (0.25 s), held.
    let doc = model::ScoreDoc::load(&score(", {bend: [[0 0] [0.5 12]]}")).unwrap_or_else(|d| panic!("{d:?}"));
    let l = &doc.loops[0];
    assert_eq!(l.note_opts.len(), 1, "{:?}", l.notes);
    assert_eq!(l.note_opts[0].1.bend.n, 2);
    let bent = render(&score(", {bend: [[0 0] [0.5 12]]}"), &opts, &Cycle).unwrap();
    assert!((hz(&bent.left) - 800.0).abs() < 25.0, "{}", hz(&bent.left));
    // arp: [0 12] stepping every beat (0.5 s): an octave up from 0.5 s.
    let arp = render(&score(", {arp: [0 12] arp_step: 1}"), &opts, &Cycle).unwrap();
    assert!(hz(&arp.left[4_800..]) > 600.0);
    // vib: the pitch swings, but averages to the note.
    let vib = render(&score(", {vib: [5 1]}"), &opts, &Cycle).unwrap();
    assert!((hz(&vib.left) - 400.0).abs() < 40.0);
    assert_ne!(vib.left, plain.left);
    // A note's pan moves it from the loop's.
    let right = render(&score(", {pan: 1}"), &opts, &Cycle).unwrap();
    let energy = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>();
    assert!(energy(&right.right) > energy(&right.left) * 20.0);
}

/// `at(ring, i)` in loop code is the ring lookup, and a canvas position is a
/// plain `{x y}` record: the prelude once also defined `mod.score.at` as the
/// position constructor, which overwrote the ring lookup as soon as a module's
/// last assignment was stored.
#[test]
fn at_in_a_loop_is_the_ring_lookup_and_a_position_is_a_record() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck at: {x: 120 y: 80}}\nlet a = Loop{play: s beats: 1 code: fn(i) { play(at([@c4, @e4], 1)) play(at([@c4, @e4], 2)) sleep(1) }}\nScore{clock s a}\n";
    let doc = model::ScoreDoc::load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.positions, vec![("s".to_string(), (120.0, 80.0))]);
    let mut composer = Composer::new(model::load(src).unwrap_or_else(|d| panic!("{d:?}")));
    let mut events = Vec::new();
    composer.pump(2.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    assert!(events.iter().any(|e| e.note == 64.0) && events.iter().any(|e| e.note == 60.0), "{events:?}");
}

/// A root flow with a `Score` node on its transport plays its slot's score
/// sample for sample as the player on its own clock does; a new score in
/// the slot takes over at the next pass, and the host reads the player's
/// meters from the slot.
#[test]
fn a_score_node_in_a_root_flow_plays_as_the_player_does() {
    use crate::player::ClockMode;
    use makepad_audio_engine::feed::{automation_feed, loop_feed, tempo_feed};
    use makepad_audio_engine::schedule::{compile, AudioBudget};
    use makepad_flow::document::closure::Closure;
    use makepad_flow::document::Registry;
    use makepad_flow::graph::FlowVm;
    use std::sync::Arc;
    let rate = 48_000.0f32;
    let loaded = model::load(TECHNO).unwrap();
    let doc = loaded.doc.clone();
    let mut registry = Registry::with_builtins();
    makepad_audio_engine::nodes::register_nodes(&mut registry).unwrap();
    crate::node::register(&mut registry).unwrap();
    let slot = crate::node::slot("root_score");
    slot.set_doc(doc.clone()).unwrap();
    let root = format!(
        "use mod.flow.*\n\nlet t = Transport{{ bpm: {} playing: 1 }}\nlet music = Score{{ score: \"root_score\" clock: t.out(@clock) }}\nlet out = AudioOutput{{ enabled: 1 audio: music.out(@audio) }}\n\nFlow{{ label: \"Root\" t, music, out }}\n",
        transport_bpm(&doc)
    );
    let (_vm, flow) = FlowVm::load_with_registry(&root, "root.splash", &Closure { modules: Vec::new() }, &registry).unwrap_or_else(|e| panic!("{}", e.message));
    let lowered = makepad_flow::engine::lower::lower(&flow).unwrap();
    let plan = makepad_flow::live::plan::partition(&lowered, &registry, &|owner| owner == makepad_audio_engine::OWNER, "root", 1).unwrap();
    let slice = plan.slices.iter().find(|slice| slice.owner == makepad_audio_engine::OWNER).unwrap();
    let mut host = compile::<NoExt, ()>(slice, 1, 1, (1u64 << 32) | 48_000, None, &Arc::new(AudioBudget::default())).unwrap().schedule;
    let mut own = crate::player::prepare(&doc, rate, "own_score", ClockMode::Own, None, 1).unwrap();
    let mut composer = Composer::new(loaded);
    let reach = CHUNK as f64 * 400.0 / 60.0 / rate as f64;
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let mut beat = 0.0f64;
    let mut events = Vec::new();
    let mut pass = |composer: &mut Composer, beat: &mut f64, own: &mut crate::player::Player, host: &mut makepad_audio_engine::schedule::Schedule<(), ()>, a: &mut Vec<f32>, b: &mut Vec<f32>| {
        composer.pump(*beat + reach + 1e-9, &mut events);
        for change in composer.tempo_out.drain(..) {
            tempo_feed("own_score_clock").send(change);
            tempo_feed("t").send(change);
        }
        for (fx, event) in composer.automation_out.drain(..) {
            automation_feed(&format!("own_score/{fx}")).send(event);
            automation_feed(&format!("root_score/{fx}")).send(event);
        }
        composer.osc_out.clear();
        composer.history.clear();
        for event in events.drain(..) {
            let name = doc.feed_names().into_iter().find(|n| key(n) == event.loop_key).unwrap();
            loop_feed(&format!("own_score/{name}")).send(event);
            loop_feed(&format!("root_score/{name}")).send(event);
        }
        own.run(CHUNK, &mut |cue, l, _| if !cue { a.extend_from_slice(&l[..CHUNK]) });
        run(host, CHUNK, &mut |cue, l, _| if !cue { b.extend_from_slice(&l[..CHUNK]) }, &mut NoExt);
        *beat = own.schedule.clocks[0].tempo.end_beat(CHUNK);
    };
    while a.len() < 4 * 48_000 {
        pass(&mut composer, &mut beat, &mut own, &mut host, &mut a, &mut b);
    }
    assert_eq!(a.len(), b.len());
    assert!(a == b, "the node's render differs from the player's");
    assert!(a.iter().any(|x| x.abs() > 0.05), "silent");
    // The meters: the host reads the node's player through the slot.
    slot.pump().unwrap();
    assert_eq!(slot.playing(), 1);
    assert!(slot.read("root_score/kick", "peak").is_some_and(|peak| peak > 0.0));
    assert!(slot.scope("root_score/master").is_some_and(|bins| bins.len() == crate::player::SCOPE_BINS));
    // A new score: prepared now (the mailbox is empty), playing from the next pass on.
    slot.set_source(&TECHNO.replace("amp: 0.4", "amp: 0.3")).unwrap();
    pass(&mut composer, &mut beat, &mut own, &mut host, &mut a, &mut b);
    slot.pump().unwrap();
    assert_eq!(slot.playing(), 2);
    let tail = b.len() - CHUNK;
    assert!(b[tail..].iter().any(|x| x.abs() > 1e-4), "the swap keeps the music playing");
    makepad_audio_engine::feed::forget_instance("own_score");
}
