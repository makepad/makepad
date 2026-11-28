//! Round trips, source-preserving edits, the hot swap at a cycle boundary,
//! live equals offline, and determinism with a seed.

use crate::build::NoSamples;
use crate::compose::Composer;
use crate::render::{instantiate, render, transport_bpm, RenderOpts};
use crate::source;
use crate::{key, model};
use makepad_audio_engine::run::{run, NoExt};
use makepad_audio_engine::CHUNK;
use makepad_score_player::shader::FADE;
use makepad_score_player::EventKind;
use makepad_flow::Literal;

/// Prepare the ordinary slice in tests that exercise live players at a chosen rate.
fn prepare_score(doc: &model::ScoreDoc, rate: f32, name: &str, clocking: crate::player::ClockMode,
    base: Option<&makepad_audio_engine::schedule::Layout>, generation: u64, control: &crate::PrepareControl,
) -> Result<crate::player::Player, crate::LoadError> {
    let slice = crate::player::slice(doc, name, clocking);
    crate::player::prepare(doc, slice, rate, name, clocking, base, generation, control)
}

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
    let doc = model::ScoreDoc::test_load(TECHNO).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.label, "Techno");
    assert_eq!(doc.clock.bpm, 126.0);
    assert_eq!(doc.loops.len(), 4);
    assert_eq!(doc.insts.len(), 3);
    assert_eq!(doc.fx.len(), 2);
    assert_eq!(&*doc.fx[1].inputs, ["chords", "echo"]);
    assert!(doc.loops[0].hits.is_some());
    assert!(doc.loops[2].code.is_some());
    let instance = instantiate(&doc, 48_000.0, &crate::PrepareControl::default()).unwrap();
    assert_eq!(instance.feeds.len(), 4);
    // The lowered graph has every loop and FX as a node of the engine.
    for name in ["kick", "hats", "line", "chords", "echo", "verb", "master"] {
        assert!(instance.layout.nodes.iter().any(|n| n.id == format!("{}/{name}", instance.name)), "{name}");
    }
}

#[test]
fn problems_are_reported_where_they_are() {
    let check = |bad: String, line: u32, needle: &str| {
        let diags = model::ScoreDoc::test_load(&bad).unwrap_err();
        assert!(diags.iter().any(|d| d.line == line && d.to_string().contains(needle)), "wanted `{needle}` on line {line}: {diags:?}");
    };
    check(TECHNO.replace("hits: \"x...x...x...x...\"", "hits: 4"), 10, "pattern string");
    check(TECHNO.replace("room: 0.7", "rooom: 0.7"), 38, "`rooom` is not a Reverb field");
    check(TECHNO.replace("in: [line]", "in: [lien]"), 37, "lien");
    assert!(model::ScoreDoc::test_load(&TECHNO.replace("seed: 7}", "seed: 7")).is_err());
    // A code error shows when the loop runs: the composer reports it.
    let loaded = model::test_load(&TECHNO.replace("hit(@hat,", "hit(@hta,")).unwrap();
    let mut composer = Composer::new(loaded.into_evaluated());
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    let errors = composer.errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].node.as_deref(), Some("hats"));
    assert!(errors[0].message.contains("no drum `hta`"), "{errors:?}");
    assert_eq!(errors[0].line, 15);

    let control = crate::PrepareControl::default();
    for sliced in [false, true] {
        with_refused_score_concat(|source, line| {
            let result = if sliced {
                let slice = std::time::Duration::ZERO;
                let crate::LoadStep::Pending(mut pending) = crate::load_begin(source, &control, slice) else {
                    panic!("the score must suspend before its refused grow");
                };
                loop {
                    match pending.step(slice) {
                        crate::LoadStep::Pending(next) => pending = next,
                        crate::LoadStep::Done(result) => break result,
                    }
                }
            } else {
                crate::load_controlled(source, &control)
            };
            assert_eq!(result.err(), Some(crate::LoadError::Memory { line: Some(line) }),
                "the refused concat is attributed to its document line (sliced={sliced})");
        });
    }
    let unknown = crate::LoadError::Memory { line: None };
    assert_eq!(unknown.describe(|_| unreachable!()), "Evaluation ran out of memory",
        "a refusal with no document location does not invent a line");
}

#[test]
fn widget_edits_are_source_preserving_and_reload() {
    // Scrub the tempo, mute a loop, toggle a pattern step: every other byte
    // stays and the result loads.
    let splice = source::set_field(TECHNO, "clock", "bpm", "128").unwrap();
    let s = makepad_edit_values::apply_splices(TECHNO, 0, &[splice]).unwrap();
    assert_eq!(s, TECHNO.replacen("bpm: 126", "bpm: 128", 1));
    let s = makepad_edit_values::apply_splices(&s, 0, &[source::set_field(&s, "hats", "mute", "true").unwrap()]).unwrap();
    let outline = source::outline(&s);
    let hits = outline.node("kick").unwrap().field("hits").unwrap().value;
    // The pattern's third step: character 2 inside the quotes.
    let at = hits.start + 1 + 2;
    let s = format!("{}x{}", &s[..at], &s[at + 1..]);
    let doc = model::ScoreDoc::test_load(&s).unwrap_or_else(|d| panic!("{d:?}"));
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
    let a = render(TECHNO, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let b = render(TECHNO, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(a.left.iter().any(|x| x.abs() > 0.05), "silent");
    assert_eq!(a.left, b.left);
    assert_eq!(a.right, b.right);
    let c = render(TECHNO, &RenderOpts { seed: Some(8), ..opts.clone() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert_ne!(a.left, c.left);
}

/// A live playback in one thread: the graph renders pass by pass on the
/// engine while the composer schedules ahead of the output by a lookahead,
/// the way Stage's composer thread does.
fn live(source: &str, total: usize, lookahead_beats: f64, swap: Option<(f64, &str)>) -> (Vec<f32>, Vec<(f64, f32, u64)>) {
    let loaded = model::test_load(source).unwrap();
    let mut instance = instantiate(&loaded.doc, 48_000.0, &crate::PrepareControl::default()).unwrap();
    let bpf = transport_bpm(&loaded.doc) / 60.0 / 48_000.0;
    let mut composer = Composer::new(loaded.into_evaluated());
    let mut out = Vec::with_capacity(total);
    let mut sent = Vec::new();
    let mut swap = swap;
    while out.len() < total {
        let now = out.len() as f64 * bpf;
        if let Some((at, next)) = swap {
            if now >= at {
                composer.reload(model::test_load(next).unwrap().into_evaluated(), now + lookahead_beats);
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
    let offline = render(TECHNO, &RenderOpts { seconds: Some(seconds), ..Default::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let latency = offline.latency as usize;
    let offline: Vec<f32> = offline.left.iter().zip(&offline.right).map(|(a, b)| a + b).collect();
    // A lookahead of a chunk and a bit: enough, since the composer runs
    // before every callback.
    // The composer runs a pass and a bit ahead, as Stage's does.
    let lookahead = ((CHUNK + 64) as f64 / 48_000.0) * 126.0 / 60.0;
    let (live, _) = live(TECHNO, offline.len() + latency, lookahead, None);
    let live = &live[latency..];
    assert_eq!(live.len(), offline.len());
    let first = live.iter().zip(&offline).position(|(a, b)| a != b);
    assert_eq!(first, None, "live and offline differ from frame {first:?}");
}

const PULSE_A: &str = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet pulse = Loop{play: s beats: 4 code: fn(i) { for b in 0..4 { play(60, {dur: 0.25}) sleep(1) } }}\nScore{clock s pulse}\n";

#[test]
fn a_reevaluation_swaps_a_loop_at_its_next_cycle_without_drift() {
    let b = PULSE_A.replace("play(60,", "play(72,");
    // Re-evaluated halfway through the second cycle (beat 5.3).
    let (audio, sent) = live(PULSE_A, 9 * 48_000, 0.5, Some((5.3, &b)));
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
    let old = render(PULSE_A, &RenderOpts { seconds: Some(4.0), ..Default::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let boundary = 4 * 48_000;
    for i in 0..boundary {
        assert_eq!(audio[i + old.latency as usize], old.left[i] + old.right[i], "differs at {i} before the boundary");
    }
}

#[test]
fn sections_arrange_loops_in_time() {
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat]}\nScore{clock s pulse hat a b arrange: [a b]}");
    let r = render(&src, &RenderOpts::default(), &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(r.sections, vec![(0.0, "a".to_string()), (4.0, "b".to_string())]);
    assert!(r.played.iter().filter(|p| p.loop_name == "pulse").all(|p| p.beat < 4.0));
    assert!(r.played.iter().filter(|p| p.loop_name == "hat").all(|p| p.beat >= 4.0));
    // 2 bars at 120 plus a 2 s tail.
    assert!((r.seconds() - 6.0).abs() < 1e-3);
}

#[test]
fn sections_repeated_in_the_arrangement_play_on_to_what_follows() {
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat] next: @stop}\nScore{clock s pulse hat a b arrange: [a a b]}");
    let r = render(&src, &RenderOpts::default(), &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
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
    let doc = model::ScoreDoc::test_load(&src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.loops.len(), 4);
    assert!(!doc.order.iter().any(|(n, _)| n == "editor_panel"));
    let plain = render(TECHNO, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let with = render(&src, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert_eq!(plain.left, with.left);
}

#[test]
fn ironfish_plays_a_loop_on_its_beats_and_repeats() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet lead = Ironfish{program: \"acid\" cutoff: 0.5 reverb: 0}\nlet line = Loop{play: lead beats: 2 code: fn(i) { play(@a2, {dur: 0.5}) sleep(1) play(@c3, {dur: 0.5}) sleep(1) }}\nScore{clock lead line}\n";
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    let a = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "Ironfish is silent (peak {peak})");
    // Nothing before the first beat's frame; sound right after it.
    assert!(a.left[..1].iter().all(|x| *x == 0.0));
    let b = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert_eq!(a.left, b.left);
    // A setting out of range is reported on its line.
    let bad = src.replace("cutoff: 0.5", "cutoff: 5");
    assert!(model::test_load(&bad).is_err());
}

#[test]
fn morph_plays_its_patch_and_reports_bad_fields() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet lead = Morph{preset: @wide_saw a_table: @formant a_pos: 0.3 lfo1_sync: @n8 mods: [[@lfo1 @a_pos 0.4] [@env2, @f1_cutoff, -0.3, @m1]] fx: [@delay @reverb] lfo2_shape: @drawn lfo2_points: [[0 0 0] [0.5 1 0.3] [1 0 0]] b_on: true b_table: @user b_frames: [[1 0 0.5] [1 0.3]]}\nlet line = Loop{play: lead beats: 2 code: fn(i) { play(@a2, {dur: 0.5}) sleep(1) play(@c3, {dur: 0.5}) sleep(1) }}\nScore{clock lead line}\n";
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let Some(model::InstKind::Morph { patch, .. }) = doc.insts.iter().find(|i| i.name == "lead").map(|i| &i.kind) else { panic!("not a Morph") };
    assert!(patch.contains("a_table=formant") && patch.contains("mod=lfo1,a_pos,0.4") && patch.contains("fx=delay,reverb"), "{patch}");
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    let a = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01, "Morph is silent (peak {peak})");
    // Deterministic.
    assert_eq!(a.left, render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap().left);
    // A typo, a bad choice and a bad mod row each come back with the reason.
    for (bad, says) in [("a_pos: 0.3", "a_poss: 0.3"), ("a_table: @formant", "a_table: @nope"), ("[@lfo1 @a_pos 0.4]", "[@lfo9 @a_pos 0.4]")] {
        let errs = model::test_load(&src.replace(bad, says)).err().unwrap_or_else(|| panic!("{says} loaded"));
        assert!(!errs.is_empty(), "{says}");
    }
    // The AI's tool lists the parameters.
    let out = crate::tools::call("morph_params", &makepad_strict_json::parse(br#"{"groups": ["osc_a"]}"#).unwrap(), None, &crate::PrepareControl::default()).unwrap();
    let params = out.get("params").and_then(|v| v.as_str()).unwrap_or("");
    assert!(params.contains("a_warp1") && !params.contains("b_pos"));
}

#[test]
fn shader_instruments_and_effects_play_in_the_graph() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet bass = Shader{lib: \"acid\" cutoff: 600 mono: 1}\nlet keys = Shader{lib: \"epiano\"}\nlet line = Loop{play: bass beats: 2 code: fn(i) { play(@a1, {dur: 0.4}) sleep(0.5) play(@c2, {dur: 0.4}) sleep(1.5) }}\nlet chords = Loop{play: keys beats: 2 code: fn(i) { play(chord(@a3, @m7), {dur: 1.5}) sleep(2) }}\nlet space = ShaderFx{in: [chords] lib: \"echo\" wet: 0.4}\nScore{clock bass keys line chords space}\n";
    let started = std::time::Instant::now();
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    eprintln!("load with two shaders compiled: {:?}", started.elapsed());
    assert!(doc.fx[0].shader.is_some());
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    let a = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let rms = (a.left.iter().map(|x| x * x).sum::<f32>() / a.left.len().max(1) as f32).sqrt();
    eprintln!("shader score: peak {peak:.3} rms {rms:.4}");
    assert!(peak > 0.01, "the shaders are silent (peak {peak})");
    assert!(a.left.iter().all(|x| x.is_finite()));
    let b = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert_eq!(a.left, b.left);
    assert_eq!(a.right, b.right);
    let options = RenderOpts { seconds: Some(0.4), ..RenderOpts::default() };
    let gliding = src.replace("cutoff: 600 mono: 1", "mono: 1 glide: 0.06 steal: @quietest").replace("dur: 0.4", "dur: 0.75");
    let slid = render(&gliding, &options, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let repeat = render(&gliding, &options, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(slid.left.iter().any(|&x| x.abs() > 0.01));
    assert_eq!((slid.left, slid.right), (repeat.left, repeat.right), "the acid glide repeats exactly");
    let code = "mono()\nfn voice() { gate * 0.0625 }";
    let chords = format!("use mod.score.*\nlet clock = Clock{{}}\nlet keys = Shader{{code: {code:?}}}\nlet line = Loop{{play: keys beats: 2 notes: [[0 @c4 1 1] [0 @e4 1 1]]}}\nScore{{clock keys line}}");
    let options = RenderOpts { seconds: Some(0.02), ..Default::default() };
    let one = render(&chords, &options, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let two = render(&chords.replace("Shader{", "Shader{mono: 0 "), &options, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let one_peak = one.left.iter().copied().fold(0.0, f32::max);
    let two_peak = two.left.iter().copied().fold(0.0, f32::max);
    assert!(one_peak > 0.001 && (two_peak - one_peak * 2.0).abs() < 1e-5, "declared mono {one_peak}, explicit chord {two_peak}");
    for (field, setting) in [("steal", "steal: @newest"), ("legato", "legato: 2"), ("glide", "glide: -1")] {
        let errors = model::ScoreDoc::test_load(&chords.replace("Shader{", &format!("Shader{{{setting} "))).unwrap_err();
        assert!(errors.iter().any(|e| e.node.as_deref() == Some("keys") && e.message.contains(field)), "diagnostic on {field}: {errors:?}");
    }
    // Problems show on their fields.
    let bad = src.replace("cutoff: 600", "cutof: 600");
    let diags = model::test_load(&bad).err().expect("an unknown param is reported");
    assert!(diags[0].message.contains("no param `cutof`"), "{diags:?}");
    let wrong = src.replace("lib: \"echo\"", "lib: \"acid\"");
    assert!(model::test_load(&wrong).is_err());
}

// Named outputs write the new side first, then add the fading old side.
fn faded_output(frame: usize, edit: usize, old: f32, new: f32) -> f32 {
    let gain = frame.saturating_sub(edit).min(FADE as usize) as f32 / FADE as f32;
    new * gain + old * (1.0 - gain)
}

#[test]
fn wired_cutoff_matches_manual_block_targets_across_passes_and_edits() {
    use crate::player::ClockMode;
    use makepad_audio_engine::schedule::SPAN;
    use makepad_score_player::shader::{compile, library, ShaderFx};

    // The main signal varies within every block. Its stereo mean is also one
    // of two ordered cutoff sources; reading the first sample of a pass fails.
    let src = r#"use mod.score.*
let clock = Clock{}
let tone = ShaderFx{code: "var k = int(0) fn effect(l,r) { k = (k + 1) % 13; vec2(float(k) / 32.0, float(k) / -64.0) }"}
let modulator = ShaderControl{audio: tone code: "let audio = param(0,0,1,{smooth: 0}) let out = output(1) var hz = 400.0 fn block() { hz += 71.0; out = hz + 512.0 * audio }"}
let filter = ShaderFx{lib: "filter" in: tone cutoff: [Port{node: modulator name: @out} tone]}
Score{clock tone modulator filter master: 1}"#;
    let doc = model::ScoreDoc::test_load(src).unwrap();
    let filter = library("filter").unwrap();
    let shader = compile(filter).unwrap();
    let offline = render(src, &RenderOpts { seconds: Some(768.0 / 48000.0), ..Default::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    const MOD_EDIT: usize = 195;
    for (pass, edits) in [(256, false), (97, false), (1, false), (256, true), (97, true), (1, true)] {
        let mut player = prepare_score(&doc, 48000.0, "wired_reference", ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
        let mut manual = ShaderFx::new(shader.clone(), 48000.0);
        let produced = src.replace("71.0", "109.0");
        // A change of program with the same filter state and parameter names.
        let consumed = produced.replace("lib: \"filter\"", &format!("code: {:?}", filter.replace("let k = 1.0 / resonance", "let k = 1.25 / resonance")));
        let total = offline.left.len() + offline.latency as usize;
        let (mut old_hz, mut hz) = (400.0, 400.0);
        let (mut live_l, mut live_r) = (Vec::new(), Vec::new());
        let mut at = 0usize;
        while at < total {
            if edits && (at == MOD_EDIT || at == 401) {
                let next = model::ScoreDoc::test_load(if at == MOD_EDIT { &produced } else { &consumed }).unwrap();
                let mut fresh = prepare_score(&next, 48000.0, "wired_reference", ClockMode::Own, Some(&player.layout), at as u64, &crate::PrepareControl::default()).unwrap();
                fresh.schedule.take_over(&mut player.schedule);
                player = fresh;
                if at == 401 {
                    let mut next_filter = ShaderFx::new(next.shader("filter").unwrap().shader.clone(), 48000.0);
                    next_filter.swap_from(&mut manual);
                    manual = next_filter;
                }
            }
            let mut n = pass.min(total - at);
            if edits {
                for edge in [MOD_EDIT, 401] { if at < edge { n = n.min(edge - at); } }
            }
            player.run(n, &mut |cue, l, r| {
                if !cue { live_l.extend_from_slice(&l[..n]); live_r.extend_from_slice(&r[..n]); }
            });
            let span = player.layout.audio_probes["wired_reference/filter.audio"] as usize * SPAN;
            let (mut left, mut right) = ([0.0; CHUNK], [0.0; CHUNK]);
            let mut i = 0;
            while i < n {
                let frame = at + i;
                let len = (64 - frame % 64).min(n - i);
                let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
                for j in 0..len {
                    let k = ((frame + j + 1) % 13) as f32;
                    l[j] = k / 32.0;
                    r[j] = k / -64.0;
                }
                if frame % 64 == 0 {
                    old_hz += 71.0;
                    let mean = (l[0] + r[0]) * 0.5;
                    let old_out = old_hz + 512.0 * mean;
                    let out = if edits {
                        hz += if frame >= MOD_EDIT { 109.0 } else { 71.0 };
                        faded_output(frame, MOD_EDIT, old_out, hz + 512.0 * mean)
                    } else { old_out };
                    assert!(manual.set_param_by_name("cutoff", out + mean));
                }
                manual.render(&l[..len], &r[..len], &mut left[i..i+len], &mut right[i..i+len]);
                i += len;
            }
            for (offset, want) in [(0, &left), (CHUNK, &right)] {
                for i in 0..n {
                    assert_eq!(player.schedule.arena[span + offset + i].to_bits(), want[i].to_bits(),
                        "filter at frame {}, channel {}, pass {pass}, edits {edits}", at + i, offset / CHUNK);
                }
            }
            at += n;
        }
        assert!(!player.layout.probes.contains_key("wired_reference/modulator.peak"));
        assert!(!player.layout.audio_probes.contains_key("wired_reference/modulator.audio"));
        if !edits {
            for (channel, (live, export)) in [(&live_l, &offline.left), (&live_r, &offline.right)].into_iter().enumerate() {
                assert!(live[offline.latency as usize..].iter().zip(export).all(|(a, b)| a.to_bits() == b.to_bits()),
                    "export differs from live, channel {channel}, pass {pass}");
            }
        }
        makepad_audio_engine::feed::forget_instance("wired_reference");
    }

    let high = r#"use mod.score.*
let clock = Clock{}
let big = ShaderControl{code: "let out = output(1) fn block() { out = 3.0e38 }"}
let reader = ShaderFx{code: "let gain = param(0,0,1) fn effect(l,r) { gain }" gain: Port{node: big name: @out}}
Score{clock big reader}"#;
    let mut p = prepare_score(&model::ScoreDoc::test_load(high).unwrap(), 48000.0, "wired_mean", ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
    p.run(1, &mut |_, _, _| {});
    assert_eq!(p.schedule.arena[p.layout.audio_probes["wired_mean/reader.audio"] as usize * SPAN], 1.0,
        "a finite mono target above half f32::MAX must reach the param clamp");
    makepad_audio_engine::feed::forget_instance("wired_mean");

    let cycle = src.replace("Score{clock", "modulator.audio = filter\nScore{clock");
    assert!(model::ScoreDoc::test_load(&cycle).unwrap_err().iter().any(|d| d.message.contains("cycle") &&
        d.message.contains("modulator -> filter -> modulator")), "param wires must participate in cycle detection");
    let literal = src.replace("[Port{node: modulator name: @out} tone]", "700");
    let connected = crate::graph::connect_port(&literal, "modulator", "out", "filter", "cutoff", &crate::PrepareControl::default()).unwrap();
    assert!(connected.contains("cutoff: [Port{node: modulator name: @out}]"));
    let deleted = source::remove_node(&connected.replace("[Port{node: modulator name: @out}]", "Port{node: modulator name: @out}"), "modulator").unwrap();
    assert!(model::ScoreDoc::test_load(&deleted).is_ok(), "{deleted}");
    let graph = crate::graph::project(&doc);
    assert!(graph.edges.iter().any(|e| e.from == "modulator" && e.to_port == "cutoff"));
    assert!(!graph.edges.iter().any(|e| e.from == "modulator" && e.to == crate::graph::MASTER));
    for (text, node, field, message) in [
        (src.replace("Port{node: modulator name: @out}", "modulator"), "filter", "cutoff", "Port{"),
        (src.replace("ShaderControl{", "ShaderControl{in: tone "), "modulator", "in", "no main audio"),
    ] {
        let errors = model::ScoreDoc::test_load(&text).unwrap_err();
        assert!(errors.iter().any(|e| e.node.as_deref() == Some(node) && e.message.contains(field) && e.message.contains(message)), "{errors:?}");
    }
}

#[test]
fn native_param_wires_match_manual_blocks_and_export_through_edits() {
    use crate::player::ClockMode;
    use makepad_audio_engine::schedule::SPAN;
    use makepad_score_player::{fx::FxState, FxKind};

    const RATE: f32 = 8000.0;
    const ADD: usize = 101;
    const SWAP: usize = 369;
    const REMOVE: usize = 704;
    // Two ordered stereo sources drive cutoff. The second program exceeds the
    // upper cutoff limit; the first also goes below the minimum Delay time.
    let score = r#"use mod.score.*
let clock = Clock{bpm: 240}
let tone = ShaderFx{code: "var t = int(0) fn effect(l,r) { t += 1; let x = float(t % 13) / 32.0; vec2(x, -0.5*x) }"}
let cv = ShaderFx{code: "let hz = output(2) let bias = output(2) let dt = output(2) var t = int(0) fn effect(l,r) { let x = float(t % 11); let h = HERTZ; let d = BEATS; hz = vec2(0.0, 2.0*h); bias = vec2(25.0, 75.0); dt = vec2(2.0*d, 0.0); t += 1; 0.0 }"}
let lp = Lowpass{in: tone cutoff: CUTOFF}
let echo = Delay{in: lp time: TIME mix: 1}
Score{clock tone cv lp echo master: 1}"#;
    let source = |hz: &str, beats: &str, cutoff: &str, time: &str| {
        score.replace("HERTZ", hz).replace("BEATS", beats).replace("CUTOFF", cutoff).replace("TIME", time)
    };
    let cutoff = "[Port{node: cv name: @hz} Port{node: cv name: @bias}]";
    let time = "Port{node: cv name: @dt}";
    let old_hz = "100.0 + 81.0*x";
    let old_dt = "x / 64.0";
    let new_hz = "24000.0";
    let new_dt = "(12.0 + x) / 64.0";
    let docs = [
        source(old_hz, old_dt, "750", "0.125"),
        source(old_hz, old_dt, cutoff, time),
        source(new_hz, new_dt, cutoff, time),
        source(new_hz, new_dt, "900", "0.09375"),
    ].map(|src| model::ScoreDoc::test_load(&src).unwrap());
    // One uninterrupted offline score authors the equivalent target at each cut.
    // The extra 50 Hz comes from the bias output.
    let script = |first, old, new, last| format!(
        "if t < {ADD} {{ {first} }} else if t < {REMOVE} {{ ({new}) * g + ({old}) * (1.0 - g) }} else {{ {last} }}");
    let scripted = source(&script("700.0", old_hz, new_hz, "850.0"),
        &script("0.125", old_dt, new_dt, "0.09375"), cutoff, time)
        .replace("let h =", &format!("let g = clamp(float(t - {SWAP}) / {FADE}.0, 0.0, 1.0); let h ="));
    let offline = render(&scripted,
        &RenderOpts { rate: RATE, seconds: Some(1024.0 / f64::from(RATE)), ..Default::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let trim = offline.latency as usize;
    let total = trim + offline.left.len();
    // The reference calls process once per absolute block, independently of
    // host slicing. Only an authored edit inside a block makes an earlier cut.
    let wired_targets = |at: usize| {
        let x = (at % 11) as f32;
        // cv writes hz on the right and dt on the left. Each stereo pair is
        // mixed before the wire takes its mean, sums the bias and clamps.
        let hz_right = faded_output(at, SWAP, 2.0 * (100.0 + 81.0 * x), 2.0 * 24000.0);
        let dt_left = faded_output(at, SWAP, 2.0 * (x / 64.0), 2.0 * ((12.0 + x) / 64.0));
        ((0.5 * hz_right + 50.0).min(20000.0), (0.5 * dt_left).max(0.0625))
    };
    let reference = |remove| {
        let (lp, delay) = (FxKind::Lowpass, FxKind::Delay);
        let (mut filter, mut echo) = (FxState::new(lp, RATE), FxState::new(delay, RATE));
        let (mut fp, mut dp) = (lp.defaults(), delay.defaults());
        dp[2] = 1.0;
        let mut expected = [Vec::with_capacity(total), Vec::with_capacity(total)];
        let mut at = 0;
        while at < total {
            let mut len = (64 - at % 64).min(total - at);
            if at < remove { len = len.min(remove - at); }
            (fp[0], dp[0]) = if at >= remove {
                (900.0, 0.09375)
            } else if at < ADD {
                (750.0, 0.125)
            } else {
                wired_targets(at)
            };
            let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
            for j in 0..len {
                l[j] = ((at + j + 1) % 13) as f32 / 32.0;
                r[j] = -0.5 * l[j];
            }
            for ((state, kind, params), output) in [(&mut filter, lp, &fp), (&mut echo, delay, &dp)].into_iter().zip(&mut expected) {
                state.process(kind, params, &mut l[..len], &mut r[..len], RATE, 0.25, 0.0, None);
                output.extend((0..len).map(|j| [l[j].to_bits(), r[j].to_bits()]));
            }
            at += len;
        }
        expected
    };
    assert!(render_allocations::count(|| drop(std::hint::black_box(Box::new(1u8)))) > 0,
        "the allocation counter must observe an allocation on this thread");
    let aligned = reference(REMOVE);
    let early = reference(REMOVE - 3);
    for (pass, remove, expected) in [(256, REMOVE, &aligned), (97, REMOVE, &aligned), (1, REMOVE, &aligned), (97, REMOVE - 3, &early)] {
        let name = format!("native_wires_{pass}_{remove}");
        // Also exercise a hosted player with its caller's clock and a mid-block removal.
        let clocking = if remove == REMOVE { ClockMode::Own } else { ClockMode::Host };
        let mut player = prepare_score(&docs[0], RATE, &name, clocking, None, 1, &crate::PrepareControl::default()).unwrap();
        // Already running and queued automation from the old graph may not
        // return after the wire is removed.
        let feed = makepad_audio_engine::feed::automation_feed(&format!("{name}/lp"));
        for (beat, value) in [(0.0, 750.0), (0.4, 83.0)] {
            assert!(feed.send(makepad_audio_engine::feed::AutomationEvent { beat, value, param: 0, ramp: 0.0 }));
        }
        let mut frame = 0;
        while frame < total {
            if let Some(i) = [ADD, SWAP, remove].iter().position(|&edit| edit == frame) {
                let mut next = prepare_score(&docs[i + 1], RATE, &name, clocking, Some(&player.layout), i as u64 + 2, &crate::PrepareControl::default()).unwrap();
                next.schedule.take_over(&mut player.schedule);
                player = next;
                if frame != remove {
                    // A panel write between cuts cannot replace a held wire target.
                    let index = player.control("lp", "cutoff").unwrap();
                    player.schedule.controls.set_manual(index, 31.0);
                }
            }
            let end = [ADD, SWAP, remove, total].into_iter().find(|&at| at > frame).unwrap().min(frame + pass);
            let n = end - frame;
            let (mut out_l, mut out_r) = ([0.0; CHUNK], [0.0; CHUNK]);
            let allocations = render_allocations::count(|| match clocking {
                ClockMode::Own => player.run(n, &mut |cue, l, r| if !cue {
                    out_l[..n].copy_from_slice(&l[..n]);
                    out_r[..n].copy_from_slice(&r[..n]);
                }),
                ClockMode::Host => {
                    let per_frame = 4.0 / f64::from(RATE);
                    let start = frame as f64 * per_frame;
                    let clock = makepad_audio_engine::schedule::ClockChunk { start, per_frame, playing: true, bpm: 240.0,
                        tempo: makepad_score_player::Tempo::linear(start, per_frame), ..Default::default() };
                    player.run_on(&clock, &mut out_l[..n], &mut out_r[..n]);
                }
            });
            assert_eq!(allocations, 0, "native effect render allocated or freed");
            for (node, reference) in ["lp", "echo"].into_iter().zip(expected) {
                let span = player.layout.audio_probes[&format!("{name}/{node}.audio")] as usize * SPAN;
                for i in 0..n {
                    let actual = [player.schedule.arena[span + i], player.schedule.arena[span + CHUNK + i]];
                    assert_eq!(actual.map(f32::to_bits), reference[frame + i], "{node} frame {}, pass {pass}, removal {remove}", frame + i);
                }
            }
            if remove == REMOVE {
                for (ch, (live, export)) in [(&out_l, &offline.left), (&out_r, &offline.right)].into_iter().enumerate() {
                    for i in trim.saturating_sub(frame)..n {
                        assert_eq!(live[i].to_bits(), export[frame + i - trim].to_bits(), "export frame {}, channel {ch}, pass {pass}", frame + i);
                    }
                }
            }
            if frame >= SWAP && end > 384 && end <= remove {
                let index = player.control("lp", "cutoff").unwrap();
                let block = (end - 1) / 64 * 64;
                assert_eq!(player.schedule.controls.get(index), wired_targets(block).0, "held cutoff including upper clamp at {block}");
            }
            frame = end;
        }
        makepad_audio_engine::feed::forget_instance(&name);
    }
    check_native_effect_call_boundaries();
}

fn check_native_effect_call_boundaries() {
    use crate::player::{transport, ClockMode};
    use makepad_audio_engine::{feed, schedule::SPAN};
    use makepad_score_player::{fx::FxState, FxKind};

    const RATE: f32 = 8192.0;
    const TOTAL: usize = 1536;
    const CHANGE: usize = 256;
    const SWAP: usize = 347;
    const TEMPO: usize = 713;
    let nodes = ["duck", "room", "cross", "sweep"];
    let kinds = [FxKind::Duck, FxKind::Reverb, FxKind::DjFilter, FxKind::Lowpass];
    let score = r#"use mod.score.*
let clock = Clock{bpm: 240 params: {open: 0}}
let tone = ShaderFx{code: "var t = int(0) fn effect(l,r) { let x = float(t % 13 + 1) / 32.0; t += 1; vec2(x,-0.5*x) }"}
let cv = ShaderFx{code: "var t = int(0) let detector = output(2) let depth = output(1) let size = output(1) let pos = output(1) let q = output(1) fn effect(l,r) { detector = vec2(if t % 192 == 49 { 0.125 } else { 0.0 }, if t % 256 == 82 { -0.375 } else { 0.0 }); depth = 12.0; size = if t < 256 { 0.25 } else { 0.875 }; pos = if t < 256 { 0.125 } else { 0.875 }; q = 0.75; t += 1; 0.0 }"}
let duck = Duck{in: tone key: Port{node: cv name: @detector} depth: DEPTH}
let room = Reverb{in: tone room: SIZE}
let cross = DjFilter{in: tone position: POS}
let sweep = Lowpass{in: tone cutoff: {param: @open from: 200 to: 2248 ramp: 0.25} res: Q}
Score{clock tone cv duck room cross sweep}"#;

    // One reference for both wiring modes. Calls cover whole blocks, except
    // the filter crossing, where a one-sample call observes the fade threshold.
    let mut states = kinds.map(|kind| FxState::new(kind, RATE));
    let mut expected = vec![[[0u32; 2]; 4]; TOTAL];
    let mut side = 0.0;
    for at in (0..TOTAL).step_by(64) {
        let beat = (at.min(TEMPO) as f64 + at.saturating_sub(TEMPO) as f64 * 0.5) / 2048.0;
        let mut params = kinds.map(|kind| kind.defaults());
        params[0][0] = 12.0;
        params[1][0] = if at < CHANGE { 0.25 } else { 0.875 };
        params[2][0] = if at < CHANGE { 0.125 } else { 0.875 };
        let first = |b: f64| 200.0 + 2048.0 * ((b - 31.0 / 2048.0) / 0.25).clamp(0.0, 1.0) as f32;
        let turn = 355.0 / 2048.0;
        params[3][0] = if beat < turn { first(beat) } else {
            first(turn) + (1000.0 - first(turn)) * ((beat - turn) / 0.25).clamp(0.0, 1.0) as f32
        };
        params[3][1] = 0.75;
        for (k, state) in states.iter_mut().enumerate() {
            let mut l: [f32; 64] = std::array::from_fn(|i| (((at + i) % 13 + 1) as f32) / 32.0);
            let mut r = l.map(|x| -0.5 * x);
            let step = if kinds[k] == FxKind::DjFilter { 1 } else { 64 };
            for (l, r) in l.chunks_mut(step).zip(r.chunks_mut(step)) {
                state.process(kinds[k], &params[k], l, r, RATE, 0.25, side, None);
            }
            for i in 0..64 { expected[at + i][k] = [l[i].to_bits(), r[i].to_bits()]; }
        }
        // Only the previous complete block drives the ducker. The larger
        // pulse is negative, in the right channel, and on a different period.
        side = if (at..at + 64).any(|n| n % 256 == 82) { 0.375 * 2.5 }
            else if (at..at + 64).any(|n| n % 192 == 49) { 0.125 * 2.5 } else { 0.0 };
    }
    for wired in [false, true] {
        let docs = [0, 1, 2].map(|revision| {
            let mut source = score.to_string();
            let changed = revision != 0;
            if revision == 2 {
                source = source.replace("Score{clock tone", "let late = Duck{in: tone key: tone depth: 12}\nScore{clock tone late");
            }
            for (token, port, value) in [("DEPTH", "depth", "12"), ("SIZE", "size", if changed { "0.875" } else { "0.25" }),
                ("POS", "pos", if changed { "0.875" } else { "0.125" }), ("Q", "q", "0.75")] {
                source = source.replace(token, &if wired { format!("Port{{node: cv name: @{port}}}") } else { value.to_string() });
            }
            model::ScoreDoc::test_load(&source).unwrap()
        });
        for pass in [256, 97, 1] {
            let name = format!("effect_timing_{wired}_{pass}");
            let mut player = prepare_score(&docs[0], RATE, &name, ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
            let spans = |player: &crate::player::Player| nodes.map(|node| player.layout.audio_probes[&format!("{name}/{node}.audio")] as usize * SPAN);
            let mut outputs = spans(&player);
            let automation = feed::automation_feed(&format!("{name}/sweep"));
            for (frame, value) in [(31, 2248.0), (355, 1000.0)] {
                assert!(automation.send(feed::AutomationEvent { beat: frame as f64 / 2048.0, param: 0, value, ramp: 0.25 }));
            }
            assert!(feed::tempo_feed(&transport(&name)).send(feed::TempoEvent { beat: TEMPO as f64 / 2048.0, bpm: 120.0, ramp: 0.0, locate: false }));
            let mut at = 0;
            let mut mismatch = [None; 4];
            while at < TOTAL {
                if at == CHANGE || at == SWAP {
                    let revision = if at == CHANGE { 1 } else { 2 };
                    let mut next = prepare_score(&docs[revision], RATE, &name, ClockMode::Own, Some(&player.layout), revision as u64 + 1, &crate::PrepareControl::default()).unwrap();
                    next.schedule.take_over(&mut player.schedule);
                    player = next;
                    outputs = spans(&player);
                }
                let end = [CHANGE, SWAP, TOTAL].into_iter().find(|&stop| stop > at).unwrap().min(at + pass);
                assert_eq!(render_allocations::count(|| player.run(end - at, &mut |_, _, _| {})), 0);
                // Inserted at 347, its first complete key block is [384, 448).
                for frame in [384, 447, 448].into_iter().filter(|f| (at..end).contains(f)) {
                    let span = player.layout.audio_probes[&format!("{name}/late.audio")] as usize * SPAN;
                    let dry = (frame % 13 + 1) as f32 / 32.0;
                    let actual = player.schedule.arena[span + frame - at];
                    if frame < 448 { assert_eq!(actual, dry); } else { assert!(actual < dry); }
                }
                for (k, base) in outputs.into_iter().enumerate() {
                    for i in 0..end - at {
                        let actual = [player.schedule.arena[base + i], player.schedule.arena[base + CHUNK + i]].map(f32::to_bits);
                        if actual != expected[at + i][k] { mismatch[k].get_or_insert(at + i); }
                    }
                }
                at = end;
            }
            feed::forget_instance(&name);
            assert_eq!(mismatch, [None; 4], "first differing frame for {nodes:?}, pass {pass}, wired {wired}");
        }
    }
}

#[test]
fn engine_levels_delay_and_rest_follow_the_sample_clock() {
    use crate::player::{live_node, prepare, ClockMode};
    use makepad_audio_engine::{feed, nodes::descriptor, run::{rests, run_awake}, schedule::SPAN};
    use makepad_flow::live::plan::OwnerSlice;
    use makepad_score_player::{Event, NoteOpts, Sample};
    use std::sync::Arc;

    const RATE: usize = 8192;
    const WAKE: usize = 2 * 256 * 97;
    const END: usize = WAKE + 320;
    let id = |name: &str| format!("slicing/{name}");
    let node = |kind, name, fields: &[(&str, Literal)], wires: &[(&str, Vec<(String, &str)>)]|
        live_node(descriptor(kind).unwrap(), id(name), fields, wires);
    let from = |name| vec![(id(name), "audio")];
    let pad = |name, sample| node("Pads", name, &[("samples", Literal::Arr(vec![Literal::Str(id(sample))]))], &[]);
    let slice = OwnerSlice { owner: makepad_audio_engine::OWNER.into(), nodes: vec![
        node("Transport", "clock", &[("playing", Literal::Num(1.0)), ("bpm", Literal::Num(120.0))], &[]),
        pad("ready", "constant"), pad("pad", "audio"),
        node("Loop", "note", &[("inst", Literal::Str("sampler".into())), ("sample", Literal::Str(id("audio")))], &[("clock", vec![(id("clock"), "clock")])]),
        node("Filter", "filter", &[("cutoff", Literal::Num(70.0))], &[("audio", from("pad"))]),
        node("Gain", "level", &[("gain", Literal::Num(1.0))], &[("audio", from("filter"))]),
        node("StemMixer", "stems", &[], &[("vocals", from("level")), ("drums", from("note")), ("bass", from("pad")),
            ("other", from("filter")), ("available", vec![(id("ready"), "peak")])]),
        node("AudioMixer", "mix", &[("master", Literal::Num(1.0))], &[("inputs", vec![(id("stems"), "audio"), (id("note"), "audio")])]),
        node("Delay", "master", &[("time", Literal::Num(0.0625)), ("mix", Literal::Num(0.7)), ("feedback", Literal::Num(0.0))], &[("audio", from("mix"))]),
        node("AudioOutput", "out", &[("enabled", Literal::Num(1.0))], &[("audio", from("master"))]),
    ] };
    // The readiness probe is at unity before any signal enters the separated stems.
    let samples: Vec<_> = (0..6 * RATE).map(|i| if i < 640 { ((i % 17) as f32 - 8.0) / 32.0 } else { 1e-7 }).collect();
    for (name, pcm) in [("slicing/audio", samples), ("slicing/constant", vec![1.0; 6 * RATE])] {
        feed::samples::register(name, Arc::new(Sample::new(pcm.into(), None, RATE as f32)));
    }
    let doc = model::ScoreDoc::test_load(PULSE_A).unwrap();
    let mut reference = Vec::new();
    let mut sleep_frame = None;
    for (pass, export) in [(256, true), (256, false), (97, true), (97, false), (1, true), (1, false)] {
        // Use the export player's render entry and the live device's entry
        // with the same prepared graph, including nodes outside score syntax.
        let mut player = prepare(&doc, slice.clone(), RATE as f32, "slicing", ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
        let bank = player.schedule.controls.clone();
        let layout = player.layout.clone();
        let set = |name: &str, v| bank.set_manual(layout.controls[&id(name)].0, v);
        let watches = ["pad", "note", "filter", "level", "stems", "mix", "master"]
            .map(|id| player.layout.audio_probes[&format!("slicing/{id}.audio")] as usize * SPAN);
        let hit = feed::PadHit { pad: 0, gain: 1.0, stop: false };
        assert!(feed::pad_feed("slicing/ready").send(hit));
        let mut heard = vec![[0u32; 2]; END];
        let mut active = [false; 7];
        let mut slept = None;
        let mut frame = 0;
        while frame < END {
            match frame {
                256 => {
                    assert!(feed::pad_feed("slicing/pad").send(hit));
                    assert!(feed::loop_feed("slicing/note").send(Event { beat: 256.0 / 4096.0, loop_key: 1, kind: EventKind::Note,
                        note: 60.0, vel: 1.0, pan: 0.0, dur: 4.0, opts: NoteOpts::default() }));
                    // A tempo step at frame 349.5, well inside the 256-frame pass.
                    assert!(feed::tempo_feed("slicing/clock").send(feed::TempoEvent { beat: 349.5 / 4096.0, bpm: 60.0, ..Default::default() }));
                }
                461 => {
                    for (id, v) in [("filter.cutoff", 340.0), ("level.gain", 0.7), ("stems.gain_vocals", 0.3),
                        ("mix.master", 0.6), ("note.amp", 0.4), ("pad.amp", 0.8)] { set(id, v); }
                }
                1024 => set("clock.playing", 0.0),
                WAKE => set("pad.amp", 1.0),
                _ => {}
            }
            let edge = [256, 461, 1024, WAKE, END].into_iter().find(|&edge| edge > frame).unwrap();
            let n = (pass - frame % pass).min(edge - frame);
            let mut sink = |cue: bool, l: &[f32], r: &[f32]| {
                if !cue { for i in 0..n { heard[frame + i] = [l[i].to_bits(), r[i].to_bits()]; } }
            };
            let allocations = render_allocations::count(|| {
                if export { player.run(n, &mut sink); }
                else if !rests(&mut player.schedule, &NoExt) { run_awake(&mut player.schedule, n, &mut sink, &mut NoExt, true); }
            });
            assert_eq!(allocations, 0, "render allocated at {frame}, pass {pass}");
            if frame < 1024 {
                for (on, span) in active.iter_mut().zip(watches) {
                    *on |= player.schedule.arena[span..span + n].iter().any(|v| v.abs() > 0.001);
                }
            }
            if player.schedule.rest.asleep() { slept.get_or_insert(player.schedule.frame as usize); }
            frame += n;
        }
        assert!(active.iter().all(|&on| on), "every tested op must pass signal: {active:?}");
        let last_loud = heard[..WAKE].iter().rposition(|v| v.iter().any(|x| f32::from_bits(*x).abs() >= makepad_audio_engine::rest::SILENCE)).unwrap();
        let expected = (last_loud + 1 + 5 * RATE).next_multiple_of(64);
        assert_eq!(slept, Some(expected), "sleep at the first eligible block after the last loud frame");
        assert!(heard[expected - 1][0] != 0 && heard[expected..WAKE].iter().all(|v| *v == [0; 2]), "cut the nonzero quiet tail precisely");
        assert!(heard[WAKE][0] != 0, "wake on the first frame of the common pass");
        if reference.is_empty() { reference = heard; sleep_frame = slept; }
        else {
            assert_eq!(slept, sleep_frame);
            assert_eq!(heard.iter().zip(&reference).position(|(a,b)| a != b), None, "first different frame, pass {pass}, export {export}");
        }
        feed::forget_instance(&player.name);
    }
    feed::samples::forget("slicing/audio");
    feed::samples::forget("slicing/constant");
}

// Count only the rendering thread; other score tests can prepare graphs concurrently.
mod render_allocations {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! { static COUNT: Cell<Option<usize>> = const { Cell::new(None) }; }
    thread_local! { pub(super) static REFUSE_SIZE: Cell<Option<usize>> = const { Cell::new(None) }; }
    struct Counting;
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if REFUSE_SIZE.get() == Some(layout.size()) {
                REFUSE_SIZE.set(None);
                return std::ptr::null_mut();
            }
            COUNT.with(|n| { if let Some(v) = n.get() { n.set(Some(v + 1)); } });
            System.alloc(layout)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            COUNT.with(|n| { if let Some(v) = n.get() { n.set(Some(v + 1)); } });
            System.dealloc(ptr, layout)
        }
    }
    #[global_allocator]
    static ALLOCATOR: Counting = Counting;
    pub(super) fn count(f: impl FnOnce()) -> usize {
        COUNT.with(|n| n.set(Some(0)));
        f();
        COUNT.with(|n| n.replace(None).unwrap())
    }
}

/// Fail the exact-size reserve for the last concat, on this test's thread only.
pub(super) fn with_refused_score_concat(run: impl FnOnce(&str, u32)) {
    const DOUBLINGS: u32 = 13;
    let source = format!("use mod.score.*\nlet n = 0\nfor i in 0..256 {{ n += i }}\nlet text = \"x\"\nfor i in 0..{DOUBLINGS} {{ text = text + text }}\nlet end = text + \"!\"\nScore{{}}");
    let line = source.lines().position(|s| s.starts_with("let end")).unwrap() as u32 + 1;
    struct Disarm;
    impl Drop for Disarm {
        fn drop(&mut self) { render_allocations::REFUSE_SIZE.set(None); }
    }
    let _disarm = Disarm;
    render_allocations::REFUSE_SIZE.set(Some((1 << DOUBLINGS) + 1));
    run(&source, line);
    assert!(render_allocations::REFUSE_SIZE.get().is_none(), "the script reached the selected physical grow");
}

#[test]
fn instrument_wires_follow_the_block_through_live_edits_and_export() {
    use crate::player::ClockMode;
    use makepad_audio_engine::schedule::SPAN;
    use makepad_score_player::shader::{compile, ShaderInst};

    const SOURCE_EDIT: usize = 101;
    let src = r#"use mod.score.*
let clock = Clock{bpm: 120}
let src = ShaderControl{code: "let out = output(1) var blocks = int(0) var target = 0.0 fn block() { blocks += 1; target += 0.03125; out = target }"}
let pad = Shader{level: Port{node: src name: @out} code: "let level = param(0,0,1) let env = output(2) var tick = int(0) fn voice() { let v = level + float(tick % 7) / 128.0; env = vec2(v, -v * 0.5); tick += 1; 0.0 }"}
let line = Loop{play: pad beats: 4 notes: [[0 @c4 1 1]]}
let tap = ShaderFx{in: Port{node: line name: @env} code: "fn effect(l,r) { vec2(l,r) }"}
Score{clock src pad line tap master: 1}"#;
    let edit_source = src.replace("+= 0.03125", "+= 0.0625");
    let edit_voice = edit_source.replace("level + float", "level * 0.5 + float");
    // An offline score with both edits authored at their corresponding samples.
    // The instrument samples src.out at each absolute block start.
    let scripted = src.replace("target += 0.03125; out = target", &format!(
        "target += if blocks > 2 {{ 0.0625 }} else {{ 0.03125 }}; \
         let g = clamp(float((blocks - 1) * 64 - {SOURCE_EDIT}) / {FADE}.0, 0.0, 1.0); \
         out = target * g + (float(blocks) * 0.03125) * (1.0 - g)"))
        .replace("level + float", "(if tick >= 239 { level * 0.5 } else { level }) + float");
    let rate = 48000.0;
    let offline = render(&scripted, &RenderOpts { seconds: Some(512.0 / rate), ..Default::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let docs = [src, &edit_source, &edit_voice].map(|s| model::ScoreDoc::test_load(s).unwrap());
    let manual_code = docs[0].shader_def("pad").unwrap().code.replace("tick += 1; 0.0", "tick += 1; env");
    let manual_program = compile(&manual_code).unwrap();
    let next_program = compile(&manual_code.replace("level + float", "level * 0.5 + float")).unwrap();
    for pass in [256, 97, 1] {
        let name = format!("instrument_edits_{pass}");
        let mut host = prepare_score(&docs[0], rate as f32, &name, ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
        let mut events = Vec::new();
        Composer::new(model::test_load(src).unwrap().into_evaluated()).pump(0.01, &mut events);
        for event in events { assert!(host.feeds.iter().find(|(k, _)| *k == event.loop_key).unwrap().1.send(event)); }
        let mut manual = ShaderInst::new(manual_program.clone(), rate as f32);
        manual.note_on(60.0, 1.0, 0, 0);
        let (mut old_target, mut target) = (0.0, 0.0);
        let mut frame = 0;
        let trim = offline.latency as usize;
        let total = trim + offline.left.len();
        while frame < total {
            if frame == SOURCE_EDIT || frame == 239 {
                let rev = if frame == SOURCE_EDIT { 1 } else { 2 };
                let mut next = prepare_score(&docs[rev], rate as f32, &name, ClockMode::Own, Some(&host.layout), rev as u64 + 1, &crate::PrepareControl::default()).unwrap();
                next.schedule.take_over(&mut host.schedule);
                host = next;
                if rev == 2 {
                    let mut changed = ShaderInst::new(next_program.clone(), rate as f32);
                    changed.swap_from(&mut manual);
                    manual = changed;
                }
            }
            let end = [SOURCE_EDIT, 239, total].into_iter().find(|&i| i > frame).unwrap().min(frame + pass);
            let n = end - frame;
            let (mut l, mut r) = ([0.0; CHUNK], [0.0; CHUNK]);
            // Compare at the same device output, including its startup gate.
            host.run(n, &mut |cue, left, right| if !cue {
                l[..n].copy_from_slice(&left[..n]);
                r[..n].copy_from_slice(&right[..n]);
            });
            let (mut want_l, mut want_r) = ([0.0; CHUNK], [0.0; CHUNK]);
            let mut i = 0;
            while i < n {
                let at = frame + i;
                if at % 64 == 0 {
                    old_target += 0.03125;
                    target += if at >= SOURCE_EDIT { 0.0625 } else { 0.03125 };
                    assert!(manual.set_param_by_name("level", faded_output(at, SOURCE_EDIT, old_target, target)));
                }
                let stop = n.min(i + 64 - at % 64);
                manual.render(&mut want_l[i..stop], &mut want_r[i..stop]);
                i = stop;
            }
            let span = host.layout.audio_probes[&format!("{name}/tap.audio")] as usize * SPAN;
            for (channel, (actual, reference, exported)) in [(&l, &want_l, &offline.left), (&r, &want_r, &offline.right)].into_iter().enumerate() {
                for i in 0..n {
                    let at = frame + i;
                    assert_eq!(host.schedule.arena[span + channel * CHUNK + i].to_bits(), reference[i].to_bits(),
                        "manual target at {at}, channel {channel}, pass {pass}");
                    if at >= trim { assert_eq!(actual[i].to_bits(), exported[at-trim].to_bits(), "offline/live at {at}, channel {channel}, pass {pass}"); }
                }
            }
            frame = end;
        }
        makepad_audio_engine::feed::forget_instance(&name);
    }
}

#[test]
fn instrument_edges_report_their_authored_latency_and_endpoints() {
    use crate::graph::{connect_port, disconnect, project};
    use makepad_strict_json::{self as json, Value};
    let src = r#"use mod.score.*
let clock = Clock{bpm: 240 sig: [1 4]}
let duck_key = ShaderFx{code: "latency(38) fn effect(l,r) { 0.25 }"}
let pad = Shader{key: duck_key code: "let key = input(1) let env = output(1) fn voice() { env = key * velocity; 0.0 }"}
let bass_loop = Loop{play: pad beats: 1 code: fn(i) { play(60, {amp: 1 dur: 1}); fx(@bus, {}, fn() { play(62, {amp: 1 dur: 1}) }); sleep(1) }}
let accent = Stinger{play: pad notes: [[0 64 0.5 1]]}
let tap = ShaderFx{in: Port{node: bass_loop name: @env} code: "fn effect(l,r) { vec2(l,r) }"}
Score{clock duck_key pad bass_loop accent tap master: 1}"#;
    let dry = src.replace("latency(38)", "latency(0)");
    for (source, notes) in [(src, vec!["duck_key.audio -> bass_loop.key arrives 38 frames late"]), (&dry, vec![])] {
        let doc = model::ScoreDoc::test_load(source).unwrap();
        let player = instantiate(&doc, 48000.0, &crate::PrepareControl::default()).unwrap();
        assert_eq!(player.layout.plan_notes.len(), notes.len(), "one note per authored edge across all instances");
        for (note, &message) in player.layout.plan_notes.iter().zip(&notes) {
            assert_eq!((note.node.as_str(), note.port.as_str()), ("bass_loop", "key"));
            assert_eq!(note.source, "duck_key.audio");
            assert_eq!(note.frames, 38);
            assert_eq!(note.message, message);
        }
        makepad_audio_engine::feed::forget_instance(&player.name);
        let args = json::obj(vec![("source", json::s(source)), ("bars", Value::Int(1))]);
        let heard = crate::tools::call("score_listen", &args, None, &crate::PrepareControl::default()).unwrap();
        assert_eq!(heard.get("plan_notes"), Some(&Value::Arr(notes.into_iter().map(json::s).collect())), "{}", heard.to_json());
        assert!(heard.get("notes").and_then(|music| music.get("bass_loop")).is_some(), "played notes still have their own field");
    }
    let doc = model::ScoreDoc::test_load(src).unwrap();
    let g = graph(&doc);
    for name in ["bass_loop", "bass_loop_fx1_in", "accent"] { assert_eq!(g.wired(name, "key"), ["duck_key"]); }
    let opts = RenderOpts { seconds: Some(0.012), ..Default::default() };
    let heard = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let one = render(&src.replace("play(62, {amp: 1 dur: 1})", "sleep(1)"), &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(heard.left.iter().zip(&one.left).any(|(a,b)| a-b > 0.15), "the scope voice reaches the public envelope");
    // The loop's main pair, peak meter and clock can coexist with shader ports
    // of those names. A graph edit must write the authored port name back.
    for name in ["audio", "peak"] {
        let renamed = src.replace("env", name).replace("key: duck_key", "clock: duck_key")
            .replace("let key = input", "let clock = input").replace("= key * velocity", "= clock * velocity");
        let doc = model::ScoreDoc::test_load(&renamed).unwrap();
        let projected = project(&doc);
        let named = format!(":{name}");
        assert!(projected.edges.iter().any(|e| e.from == "bass_loop" && e.from_port == named && e.to == "tap"));
        let cleared = disconnect(&renamed, "tap", "in", Some(0)).unwrap();
        let edited = connect_port(&cleared, "bass_loop", &named, "tap", "in", &crate::PrepareControl::default()).unwrap();
        let played = render(&edited, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
        assert_eq!(played.left, heard.left, "named {name} through a graph edit");
    }
    let projected = project(&doc);
    assert!(projected.edges.iter().any(|e| e.from == "duck_key" && e.to == "pad" && e.to_port == "key"));
    assert!(projected.nodes.iter().any(|n| n.id == "bass_loop" && n.outputs.iter().any(|p| p.name == "env")));
    let cleared = disconnect(src, "pad", "key", Some(0)).unwrap();
    let reconnected = connect_port(&cleared, "duck_key", "audio", "pad", "key", &crate::PrepareControl::default()).unwrap();
    assert!(model::ScoreDoc::test_load(&reconnected).is_ok());
    assert!(model::ScoreDoc::test_load(&source::remove_node(&reconnected, "duck_key").unwrap()).is_ok());
    for (source, node, message) in [
        (src.replace("node: bass_loop", "node: pad"), "tap", "read the loop that plays it"),
        (src.replace("key: duck_key code:", "key: Port{node: duck_key name: @missing} code:"), "pad", "unknown output port"),
        (src.replace("let pad = Shader{", "let pad = Synth{"), "pad", "wire the port or param on a Shader instrument"),
        (src.replace("let pad = Shader{", "let pad = Synth{"), "tap", "read its main audio or play a Shader instrument"),
        (src.replace("let bass_loop = Loop{", "let bass_loop = Loop{key: duck_key "), "bass_loop", "wire `key` on the Shader definition"),
        (src.replace("Score{clock", "pad.key = tap\nScore{clock"), "tap", "cycle"),
    ] {
        let ds = model::ScoreDoc::test_load(&source).unwrap_err();
        assert!(ds.iter().any(|d| d.message.contains(message) && (d.node.as_deref()==Some(node) || d.message.contains(node))), "{ds:?}");
    }
}

#[test]
fn named_shader_wires_play_edit_and_diagnose_their_endpoints() {
    use crate::graph::{connect_port, disconnect, project};

    let score = r#"use mod.score.*
let clock = Clock{}
let pulse = Shader{code: "fn voice() { gate * 0.25 }"}
let kick = Loop{play: pulse beats: 1 notes: [[0 @c4 0.5 1]]}
let split = ShaderFx{in: [kick] code: "let low = output(2) fn effect(l,r) { low = vec2(l,r); 0.0 }"}
let comp = ShaderFx{in: [Port{node: split name: @low}] key: kick code: "let key = input(1) fn effect(l,r) { vec2(l,r) * key }"}
Score{clock pulse kick split comp}"#;
    let authored = model::read_document_controlled(score, &crate::PrepareControl::default()).unwrap();
    let comp = authored.doc.fx.iter().find(|fx| fx.name == "comp").unwrap();
    assert!(comp.sides.iter().any(|(name, wire)| name == "key" && wire.ports().eq([("kick", "audio")])),
        "a bare name on an input port is an authored wire");
    let doc = model::ScoreDoc::test_load(score).unwrap();
    let graph = project(&doc);
    assert!(graph.edges.iter().any(|e| e.from == "split" && e.from_port == "low" && e.to_port == "in"));
    let opts = RenderOpts { seconds: Some(0.02), ..RenderOpts::default() };
    let heard = render(score, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(heard.left.iter().any(|&x| x > 0.001), "both the named output and the key must reach comp");
    let silent = render(&score.replace(" key: kick", ""), &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(silent.left.iter().all(|&x| x == 0.0), "disconnecting the key silences the multiplier");

    // A delayed kick keys a separate bass loop. Delay compensation aligns
    // their onsets, and the muted key path stays out of the master mix.
    let music = r#"use mod.score.*
let clock = Clock{}
let tone = Shader{code: "fn voice() { gate * velocity }"}
let bass = Loop{play: tone beats: 1 notes: [[0 @c3 0.5 0.2]]}
let kick = Loop{play: tone beats: 1 notes: [[0 @c2 0.5 1]]}
let copy = ShaderFx{in: kick code: "fn effect(l,r) { vec2(l,r) }"}
let silent = ShaderFx{in: copy code: "fn effect(l,r) { 0.0 }"}
let comp = ShaderFx{in: bass key: copy lib: "keyed_comp" threshold: -32 attack: 0.1}
Score{clock tone bass kick copy silent comp master: 1}"#;
    let opts = RenderOpts { rate: 8192.0, seconds: Some(0.05), ..Default::default() };
    let direct = render(music, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let dry = render(&music.replace(" key: copy", ""), &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(dry.left[200] > 0.01 && direct.left[200] < dry.left[200] * 0.5, "a loud kick must reduce the bass gain");
    let delayed = music.replace("fn effect(l,r) { vec2(l,r) }",
        "latency(7) var hist = [0.0; 16] var w = int(0) fn effect(l,r) { hist[w] = l; let x = hist[w-7]; w += 1; x }");
    let late = render(&delayed, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert_eq!(late.latency, direct.latency + 7);
    assert_eq!((direct.left, direct.right), (late.left, late.right), "delaying only the kick must preserve the aligned stereo export");

    let deleted = source::remove_node(score, "split").unwrap();
    assert!(model::ScoreDoc::test_load(&deleted).is_ok() && deleted.contains("ShaderFx{in: [] key: kick"));
    let added = connect_port(score, "split", "low", "comp", "key", &crate::PrepareControl::default()).unwrap();
    let removed = disconnect(&added, "comp", "key", Some(0)).unwrap();
    let graph = project(&model::ScoreDoc::test_load(&removed).unwrap());
    assert!(graph.edges.iter().any(|e| e.from_port == "low" && e.to_port == "key"));
    assert!(!graph.edges.iter().any(|e| e.from == "kick" && e.to == "comp"));
    let added = connect_port(score, "kick", "audio", "comp", "in", &crate::PrepareControl::default()).unwrap();
    assert!(disconnect(&added, "comp", "in", Some(1)).unwrap().contains("in: [Port{node: split name: @low}]"));

    let held = score.replace("fn effect(l,r) { low = vec2(l,r);", "fn block() { low = vec2(1,1) } fn effect(l,r) {");
    assert!(render(&held, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap().left.iter().any(|&x| x > 0.001));
    let param = score.replace("let key = input(1)", "let key = param(0,0,1)");
    assert!(render(&param, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap().left.iter().any(|&x| x > 0.001));
    for (old, new, needle, reason) in [
        ("name: @low", "name: @gone", "split.gone", "unknown"),
        ("key: kick", "oops: kick", "oops", "unknown"),
        ("key: kick", "key: Port{node: kick name: @aux}", "kick.aux", "unknown output"),
        ("let key = input(1)", "let lib = input(1) let key = input(1)", "lib", "rename"),
        ("clock pulse kick split comp}", "clock pulse kick comp split}", "split", "comes after"),
    ] {
        let errors = model::ScoreDoc::test_load(&score.replace(old, new)).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains(needle) && e.message.contains(reason)), "{new}: {errors:?}");
    }
    // Objects must be defined before Splash can evaluate them; a node may
    // reference itself by being edited after construction.
    let cycle = score.replace("Score{clock", "split.in = [comp]\nScore{clock");
    let errors = model::ScoreDoc::test_load(&cycle).unwrap_err();
    assert!(errors.iter().any(|e| ["cycle", "split", "comp"].iter().all(|s| e.message.contains(s))), "{errors:?}");
}

#[test]
fn shader_node_oversample_controls_program_reuse_and_path_alignment() {
    use crate::player::{ClockMode, Player};
    use makepad_audio_engine::schedule::{NodeState, SPAN};

    let source = r#"use mod.score.*
let clock = Clock{}
let dry = ShaderFx{code: "var k = int(0) fn effect(l,r) { let a = TAU * float(k) / 8.0; k = (k + 3) % 8; vec2(sin(a), cos(a)) * 0.125 }"}
let wet = ShaderFx{in: [dry] code: "rate(4) fn effect(l,r) { vec2(l,r) }" oversample: 2}
let sum = ShaderFx{in: [dry wet] code: "fn effect(l,r) { vec2(l,r) }"}
Score{clock dry wet sum}"#;
    let load = |src: &str| model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let doc = load(source);
    let name = "shader_rate_score";
    let mut player = prepare_score(&doc, 48_000.0, name, ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
    let program = |player: &Player, latency| {
        let wet = player.layout.nodes.iter().find(|n| n.id == "shader_rate_score/wet").unwrap();
        assert_eq!(wet.own_latency, latency, "the wet node's own latency");
        let NodeState::ShaderFx(state) = &player.schedule.states[wet.state as usize] else { panic!("expected a shader effect") };
        state.fx.shader().clone()
    };
    assert_eq!(doc.shader("wet").unwrap().shader.latency(), 34, "score validation uses the node rate");
    let mut previous = program(&player, 34);

    // Compare the real sum to two delayed copies of its source, after filter startup.
    player.run(CHUNK, &mut |_, _, _| {});
    let dry = player.layout.audio_probes["shader_rate_score/dry.audio"] as usize * SPAN;
    let sum = player.layout.audio_probes["shader_rate_score/sum.audio"] as usize * SPAN;
    let mut error = 0.0;
    let mut power = 0.0;
    for offset in [0, CHUNK] {
        for i in 96..CHUNK {
            let reference = 2.0 * f64::from(player.schedule.arena[dry + offset + i - 34]);
            let observed = f64::from(player.schedule.arena[sum + offset + i]);
            error += (observed - reference).powi(2);
            power += reference * reference;
        }
    }
    let null_db = 10.0 * (error / power).log10();
    assert!(power > 0.0 && null_db < -70.0, "parallel 2x and dry paths null at {null_db:.2} dB");
    eprintln!("2x and dry score sum against two 34-frame delayed copies: {null_db:.2} dB");

    for (edit, latency, reuses) in [
        ("oversample: 2", 34, true),
        ("oversample: 1", 0, false),
        ("", 38, false),
        ("oversample: [4 @linear]", 0, false),
        ("oversample: [4 @filter @hold]", 18, false),
    ] {
        let next_doc = load(&source.replace("oversample: 2", edit));
        let mut next = prepare_score(&next_doc, 48_000.0, name, ClockMode::Own, Some(&player.layout), player.layout.generation + 1, &crate::PrepareControl::default()).unwrap();
        let shader = program(&next, latency);
        assert_eq!(previous.ptr_eq(&shader), reuses, "program reuse for '{edit}'");
        next.schedule.take_over(&mut player.schedule);
        assert!(shader.ptr_eq(&program(&next, latency)), "hot swap installs '{edit}'");
        player = next;
        previous = shader;
    }
    makepad_audio_engine::feed::forget_instance(name);
}

#[test]
fn shader_node_oversample_validates_modes_at_the_field() {
    use crate::player::ClockMode;

    let score = |ty, code: &str, field| format!("use mod.score.*\nlet clock = Clock{{}}\nlet tone = {ty}{{code: {code:?}\n    oversample: {field}}}\nScore{{clock tone}}");
    for (ty, code, value, reason) in [
        ("ShaderFx", "fn effect(l,r) { vec2(l,r) }", "3", "factor"),
        ("Shader", "fn voice() { gate }", "[4 @filter @hold]", "down only"),
        ("ShaderFx", "fn effect(l,r) { vec2(l,r) }", "[2 @unknown]", "conversion mode"),
        ("ShaderFx", "var os = Oversample2{} fn effect(l,r) { oversample(os,l,fn(u) { u }) }", "8", "reaches 16×"),
    ] {
        let src = score(ty, code, value);
        let errors = model::ScoreDoc::test_load(&src).unwrap_err();
        let (line, col) = source::line_col(&src, src.find("oversample:").unwrap());
        assert!(errors.iter().any(|e| e.node.as_deref() == Some("tone") && (e.line, e.col) == (line as u32, col as u32)
            && e.message.contains(reason)), "expected {reason} on the oversample field: {errors:?}");
    }

    let src = score("Shader", "rate(4, hold) fn voice() { gate }", "[8 @filter]")
        .replace("Score{clock tone}", "let line = Loop{play: tone beats: 1 notes: [[0 @c4 0.5 1]]}\nScore{clock tone line}");
    let doc = model::ScoreDoc::test_load(&src).unwrap();
    let player = prepare_score(&doc, 48_000.0, "shader_rate_voice", ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
    let instrument = player.layout.nodes.iter().find(|n| n.id == "shader_rate_voice/line").unwrap();
    assert_eq!(instrument.own_latency, 20, "the instrument uses the node's 8x down filter");
    makepad_audio_engine::feed::forget_instance("shader_rate_voice");
}

#[test]
fn editing_a_shader_module_updates_its_instrument_and_effect() {
    use crate::player::{ClockMode, Player};
    use makepad_audio_engine::schedule::NodeState;

    let source = r#"use mod.score.*
let clock = Clock{}
let voice = Shader{code: "use tone.*\nfn voice() { level(gate) }"}
let line = Loop{play: voice beats: 1 notes: [[0 @c4 0.5 1]]}
let fx = ShaderFx{in: [line] code: "use curve\nfn effect(l,r) { vec2(curve.gain(l), curve.gain(r)) }"}
let curve = ShaderModule{code: "use tone\nfn gain(x) { tone.level(x) }"}
let dry = ShaderFx{in: [fx] code: "fn effect(l,r) { vec2(l,r) }"}
Score{clock voice line fx dry curve tone := ShaderModule{code: "fn level(x) { x * 0.5 }"}}"#;
    let edited = source.replace("x * 0.5", "x * 0.25");
    let load = |src: &str| model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let before_doc = load(source);
    let after_doc = load(&edited);
    let mut before = prepare_score(&before_doc, 48000.0, "module_score", ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
    let mut after = prepare_score(&after_doc, 48000.0, "module_score", ClockMode::Own, Some(&before.layout), 2, &crate::PrepareControl::default()).unwrap();
    let changed: Vec<_> = before.slice.nodes.iter().zip(&after.slice.nodes).filter(|(a, b)| a.params != b.params)
        .map(|(n, _)| n.id.as_str()).collect();
    assert_eq!(changed, ["module_score/line", "module_score/fx"], "only importers change their fields");
    let program = |player: &mut Player, name: &str| {
        let node = player.layout.nodes.iter().find(|n| n.id == format!("module_score/{name}")).unwrap();
        match &mut player.schedule.states[node.state as usize] {
            NodeState::Loop(s) => s.engine.shader_mut().unwrap().shader().clone(),
            NodeState::ShaderFx(s) => s.fx.shader().clone(),
            _ => panic!("expected a shader"),
        }
    };
    for node in ["line", "fx", "dry"] {
        assert_eq!(program(&mut before, node).ptr_eq(&program(&mut after, node)), node == "dry", "compiled program for {node}");
    }
    let modes = load(&edited.replace("let voice = Shader{", "let voice = Shader{mono: 1 legato: 0 glide: 0.06 steal: @quietest "));
    let mut updated = prepare_score(&modes, 48000.0, "module_score", ClockMode::Own, Some(&after.layout), 3, &crate::PrepareControl::default()).unwrap();
    assert!(program(&mut updated, "line").ptr_eq(&program(&mut after, "line")), "voice settings reuse the compiled instrument");
    let next_fx = program(&mut after, "fx");
    after.schedule.take_over(&mut before.schedule);
    assert!(next_fx.ptr_eq(&program(&mut after, "fx")), "the edited effect takes over");

    let options = RenderOpts { seconds: Some(0.02), ..RenderOpts::default() };
    let a = render(source, &options, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let b = render(&edited, &options, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(peak(&a.left) > peak(&b.left) && peak(&b.left) > 0.001, "both versions play at their authored levels");
    let bad = source.replace("use tone.*", "use absent.*");
    let errors = model::ScoreDoc::test_load(&bad).unwrap_err();
    assert!(errors.iter().any(|d| d.node.as_deref() == Some("voice") && d.message.contains("no module `absent`")), "{errors:?}");
}

#[test]
fn shader_streams_follow_the_score_node_and_seed() {
    let source = r#"use mod.score.*
let clock = Clock{seed: 1}
let dust = Shader{code: "fn voice() { rand() * 0.1 }" seed: 0}
let west = Loop{play: dust beats: 1 notes: [[0 @c4 0.5 1]] pan: -1}
let east = Loop{play: dust beats: 1 notes: [[0 @c4 0.5 1]] pan: 1}
let fx = ShaderFx{in: [west east] code: "fn effect(l, r) { vec2(l, r) * (0.5 + instance) }" seed: 1}
Score{clock dust west east fx}"#;
    let play = |src: &str| {
        let opts = RenderOpts { seconds: Some(0.02), ..RenderOpts::default() };
        let r = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
        (r.left, r.right)
    };
    let first = play(source);
    assert!(peak(&first.0) > 0.001 && peak(&first.1) > 0.001, "both loops play");
    assert_ne!(first.0, first.1, "two loops on one instrument have independent streams");
    assert_eq!(play(source), first, "a fresh render plays the same samples");
    assert_ne!(play(&source.replace("seed: 0", "seed: 2")), first, "instrument seed");
    assert_ne!(play(&source.replace("}\" seed: 1", "}\" seed: 2")), first, "effect seed");
    let high_seed = play(&source.replace("Clock{seed: 1}", "Clock{seed: 4294967297}"));
    assert_ne!(high_seed, first, "high clock seed bits");
    let opts = RenderOpts { seconds: Some(0.02), seed: Some(4294967297), ..Default::default() };
    let overridden = render(source, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    for (actual, authored) in [(&overridden.left, &high_seed.0), (&overridden.right, &high_seed.1)] {
        assert!(actual.iter().map(|v| v.to_bits()).eq(authored.iter().map(|v| v.to_bits())),
            "a render seed override and the same authored clock seed produce identical sample bits");
    }
    let invalid = model::test_load(&source.replace("seed: 0", "seed: 1.5")).err().expect("fractional seed rejected");
    assert!(invalid.iter().any(|d| d.message.contains("seed")), "diagnostic names the seed");
}

#[test]
fn shader_choices_and_steps_reach_the_render() {
    let word = crate::PrepareControl::default();
    let code = "let wave = param(0, {choices: [square saw tri]})\nlet level = param(0, 0, 1, {step: 0.5})\nlet cutoff = param(1200, 20, 16000, {unit: hz})\nfn voice() { vec2(wave * level, 0) }";
    let score = |wave: &str| format!("use mod.score.*\nlet clock = Clock{{}}\nlet tone = Shader{{code: {code:?} wave: {wave} level: 0.7}}\nlet line = Loop{{play: tone beats: 1 notes: [[0 @c4 0.5 1]]}}\nScore{{clock tone line}}");
    let named = score("@saw");
    let doc = model::ScoreDoc::test_load(&named).unwrap();
    let shader = doc.shader("tone").expect("expected a shader");
    assert!(shader.params.contains(&("level".into(), 0.5)), "the score stores the snapped value");
    assert_eq!(doc.insts, model::ScoreDoc::test_load(&score("\"saw\"")).unwrap().insts, "quoted choices resolve to the same index");

    let computed = named.replace("let wave = ", "let half = 0.25 + 0.25; let wave = ")
        .replace("step: 0.5", "step: half");
    for source in [&named, &computed] {
        let before = model::SHADER_COMPILES.with(|n| n.get());
        let authored = model::read_document_controlled(source, &word).unwrap();
        let def = authored.doc.shader_def("tone").unwrap();
        assert!(def.overrides.contains(&("level".into(), model::Override::Num(0.7))));
        assert!(def.overrides.contains(&("wave".into(), model::Override::Choice("saw".into()))));
        assert_eq!(model::SHADER_COMPILES.with(|n| n.get()), before, "reading declarations compiles no shader");
        let checked = model::ScoreDoc::test_load(source).unwrap();
        assert_eq!(checked.shader("tone").unwrap().params, shader.params, "literal and computed steps snap identically");
    }

    let broken = "use mod.score.*\nlet clock = Clock{}\nlet ctl = ShaderControl{\n    code: \"let out = output(1) fn block() { out = undefined_body_value }\"\n}\nScore{clock ctl}";
    assert!(model::read_document_controlled(broken, &word).is_ok(), "a body type error is resolved only at load");
    let valid_loop = plays("Synth{preset: @pluck}", A_NOTE);
    let poisoned = valid_loop.replacen("Score{", "let unused = ShaderControl{code: \"let out = output(1) fn block() { out = unknown_name }\"}\nScore{unused ", 1);
    assert!(matches!(model::ScoreDoc::load_controlled(&poisoned, &crate::PrepareControl::default()), Err(crate::LoadError::Problems(_))));
    assert_eq!(crate::render::cycle_notes_controlled(&valid_loop, &word).unwrap(), crate::render::cycle_notes_controlled(&poisoned, &word).unwrap());
    let loop_name = model::read_document_controlled(&valid_loop, &word).unwrap().doc.loops[0].name.clone();
    assert_eq!(crate::vary::part_of_controlled(&valid_loop, &loop_name, &word).unwrap(), crate::vary::part_of_controlled(&poisoned, &loop_name, &word).unwrap());
    assert_eq!(crate::render::timing_controlled(&valid_loop, None, &word).unwrap(), crate::render::timing_controlled(&poisoned, None, &word).unwrap());
    let loop_error = |source: &str| {
        crate::render::loop_error_controlled(&source.replace("play(@c4,", "play(nope(),"), &loop_name, &word).unwrap()
            .expect("the loop itself fails when pumped")
    };
    assert_eq!(loop_error(&valid_loop), loop_error(&poisoned), "the unrelated shader cannot hide a loop error");
    let diag = |line, message: &str| model::Diag { line, col: 5, node: Some("ctl".into()), message: message.into() };
    let body = diag(4, "shader line 1: unknown name `undefined_body_value`");
    assert_eq!(model::ScoreDoc::test_load(broken).unwrap_err(), [body.clone()]);
    let both = broken.replace("\n}\n", "\n    in: []\n}\n");
    assert_eq!(model::ScoreDoc::test_load(&both).unwrap_err(), [
        body, diag(5, "a ShaderControl has no main audio input for `in`; wire a param instead"),
    ], "a failed shader and a structural error keep their positions and order");
    let malformed = broken.replace("undefined_body_value", "0x");
    assert_eq!(model::read_document_controlled(&malformed, &word).err(),
        Some(crate::LoadError::Problems(vec![diag(4, "shader line 1: bad hex number `0x`")])),
        "a malformed shader has no inventory and keeps its authored diagnostic and location");

    let labelled = valid_loop.replace("Score{", "Score{label: \"Live\" ");
    assert_eq!(model::evaluated_label_controlled(&labelled, &word).unwrap().as_deref(), Some("Live"));
    assert_eq!(model::evaluated_palette_controlled(&valid_loop, &word).unwrap(), Some((120.0, vec!["inst (synth)".into()])));

    let opts = RenderOpts { seconds: Some(0.05), ..Default::default() };
    let a = render(&named, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let b = render(&score("1"), &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(a.left.iter().any(|v| v.abs() > 0.01), "the selected wave is audible");
    assert_eq!((a.left, a.right), (b.left, b.right), "a named wave renders as its index");

    let bad = score("@missing").replace("level: 0.7", "cutoff: @low");
    let diags = model::ScoreDoc::test_load(&bad).unwrap_err();
    assert!(diags.iter().any(|d| d.message.contains("@square @saw @tri")), "{diags:?}");
    assert!(diags.iter().any(|d| d.message.contains("from 20 Hz to 16000 Hz")), "{diags:?}");
}

#[test]
fn a_voice_sings_its_lyrics_on_the_notes() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 100}\nlet vox = Voice{profile: @alto vibrato: 0.2}\nlet choir = Voice{profile: @tenor choir: 4 parts: 2}\nlet line = Loop{play: vox beats: 4 code: fn(i) { sleep(sing([@e4 @g4 @a4 @g4], \"a-ma-zing grace\", {dur: 1})) }}\nlet pad = Loop{play: choir beats: 4 code: fn(i) { sing([[@c4 @e4]], \"ooh\", {dur: 4}) sleep(4) }}\nScore{clock vox choir line pad}\n";
    let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
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
    let a = render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.01 && a.left.iter().all(|x| x.is_finite()), "the voices are silent (peak {peak})");
    assert_eq!(a.left, render(src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap().left);
    let bad = src.replace("@alto", "@diva");
    assert!(model::test_load(&bad).is_err());
}

#[cfg(feature = "neural")]
#[test]
fn a_neural_voice_sings_its_phrases_rendered_ahead() {
    use makepad_score_player::sung::neural::{self, makepad_ai_sing as sing};
    // A tiny untrained voice stands in for the installed one.
    let path = std::env::temp_dir().join(format!("score-neural-test-{}.mksing", std::process::id()));
    sing::Cantor::random(sing::acoustic::AcousticConfig::tiny(), sing::vocoder::VocoderConfig::tiny(), 1).save(&path, sing::weights::Dtype::F32).unwrap();
    std::env::set_var("MAKEPAD_CANTOR_MODEL", &path);
    let line = "code: fn(i) { sleep(sing([@e4 @g4 @a4 @g4], \"a-ma-zing grace\", {dur: 1})) }";
    let src = format!("use mod.score.*\nlet clock = Clock{{bpm: 100}}\nlet vox = Voice{{engine: @neural}}\nlet alto = Voice{{profile: @alto}}\nlet line = Loop{{play: vox beats: 4 {line}}}\nlet dsp = Loop{{play: alto beats: 4 {line}}}\nScore{{clock vox alto line dsp}}\n");
    let mut composer = Composer::new(model::test_load(&src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
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
    let a = render(&src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let peak = a.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.0 && a.left.iter().all(|x| x.is_finite()), "the neural voice is silent");
    assert_eq!(a.left, render(&src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap().left);
    assert!(neural::shared().is_some());
    let _ = std::fs::remove_file(&path);
    assert!(model::test_load(&src.replace("@neural", "@opera")).is_err());
}

#[test]
fn game_music_headless_equals_the_offline_render_and_reports_its_beats() {
    use crate::game::{music, Clocking, MusicEvent};
    use makepad_audio_synth::Instrument;
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat]}\nScore{clock s pulse hat a b arrange: [a b]}");
    let seconds = 3.0;
    let offline = render(&src, &RenderOpts { seconds: Some(seconds), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let (mut handle, mut inst) = music(&src, 48_000.0, Clocking::Headless, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
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
    let latency = offline.latency as usize;
    handle.advance(latency);
    let (mut l, mut r) = (vec![0.0; latency], vec![0.0; latency]);
    inst.render(&mut l, &mut r);
    out.extend(l);
    let out = &out[latency..];
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
    use crate::{LoadError, PrepareControl};
    use makepad_audio_synth::Instrument;
    use makepad_script::makepad_error_log::{set_log_tap, LogLevel};
    use std::sync::Mutex;

    let source = PULSE_A.replace("Synth{preset: @pluck}", r#"Shader{code: "fn voice() { gate * 0.1 }"}"#);
    let count = || model::SHADER_COMPILES.with(|n| n.get());
    let before = count();
    let start = |source: &str, control: &PrepareControl| music(source, 48_000.0, Clocking::Headless, control);
    let cancelled = PrepareControl::default();
    cancelled.cancel();
    assert!(matches!(start(&source, &cancelled), Err(LoadError::Cancelled)),
        "a pre-cancelled start returns neither music nor diagnostics");
    assert_eq!(count(), before, "a pre-cancelled start skips shader preparation");

    let (mut handle, mut inst) = start(&source, &PrepareControl::default()).expect("a finite score starts");
    assert_eq!(count(), before + 1, "the headless composer reuses the checked shader on start");
    let silent = "use mod.score.*\nlet clock = Clock{}\nScore{clock}";
    assert_eq!(handle.set_source(silent, &cancelled), Err(LoadError::Cancelled),
        "a pre-cancelled replacement returns no diagnostic");

    // Stop from a second thread once the document itself reports entry.
    {
        const MARK: &str = "game_music_document_entered";
        static REQUEST: Mutex<Option<PrepareControl>> = Mutex::new(None);
        fn tap(message: &str, _: LogLevel) {
            if message.contains(MARK) {
                let control = REQUEST.lock().unwrap().take().expect("one marker per request");
                assert!(control.script_running(), "the game document is registered on its caller's control");
                std::thread::spawn(move || control.cancel()).join().unwrap();
            }
        }
        struct ResetTap;
        impl Drop for ResetTap {
            fn drop(&mut self) {
                set_log_tap(None);
                REQUEST.lock().unwrap().take();
            }
        }
        let _lock = crate::live::TEST_LOG_TAP.lock().unwrap_or_else(|e| e.into_inner());
        set_log_tap(Some(tap));
        let _reset = ResetTap;
        let runaway = format!("mod.std.log({MARK:?})\nloop {{}}");
        for starting in [true, false] {
            let control = PrepareControl::default();
            *REQUEST.lock().unwrap() = Some(control.clone());
            let result = if starting { start(&runaway, &control).map(drop) } else { handle.set_source(&runaway, &control) };
            assert!(control.is_cancelled(), "the other thread stopped the running document (start={starting})");
            assert_eq!(result, Err(LoadError::Cancelled), "the runaway ends without diagnostics (start={starting})");
        }
    }
    let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
    let mut original_peak = 0.0f32;
    for _ in 0..10 {
        handle.advance(4800);
        inst.render(&mut l, &mut r);
        original_peak = original_peak.max(peak(&l));
    }
    assert!(original_peak > 0.0, "the original graph still plays after the cancelled replacements");
    // A new instrument and loop: a new graph, the pulse carried over.
    let next = source.replace("Score{clock s pulse}", "let bell = Ironfish{program: \"glass\"}\nlet ring = Loop{play: bell beats: 4 code: fn(i) { play(@c5, {dur: 1}) sleep(4) }}\nScore{clock s bell pulse ring}");
    let mut slot = crate::game::MusicSlot::default();
    let stale = slot.begin();
    let doc = model::ScoreDoc::load_controlled(&next, &stale).unwrap();
    let mut first = handle.base().prepare(next.as_str().into(), &doc, &stale).unwrap();
    let latest = slot.begin();
    assert!(stale.is_cancelled() && !latest.is_cancelled(), "only the replaced prepare is cancelled");
    assert_eq!(handle.post(&mut first), Err(crate::LoadError::Cancelled), "a completed obsolete graph cannot enter the swaps queue");
    let before = count();
    handle.set_source(&next, &latest).expect("a finite replacement loads");
    assert_eq!(count(), before + 1, "the headless composer reuses the replacement shader on reload");
    let mut heard = 0.0f32;
    for _ in 0..20 {
        handle.advance(4800);
        inst.render(&mut l, &mut r);
        heard = heard.max(l.iter().fold(0.0f32, |m, x| m.max(x.abs())));
        handle.poll();
    }
    assert!(heard > 0.01);
    assert!(matches!(handle.set_source("let broken = ", &slot.begin()), Err(LoadError::Problems(_))),
        "an invalid replacement still reports diagnostics");
}

#[test]
fn game_music_plays_on_a_synth_bus_in_real_time() {
    use crate::game::{music, Clocking, MusicEvent};
    use makepad_audio_synth::bus::{Emitter, Group, SynthBus};
    let (mut handle, inst) = music(PULSE_A, 48_000.0, Clocking::Realtime, &crate::PrepareControl::default()).unwrap();
    let mut bus = SynthBus::new(48_000.0);
    assert!(bus.add_instrument(7, Box::new(inst), Emitter::FLAT, 1.0, &mut |_| {}));
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
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.dependencies().len(), 2);
    let opts = RenderOpts { seconds: Some(1.0), ..RenderOpts::default() };
    let silent = render(src.replace("\"tone\"", "\"missing\"").replace("\"boom\"", "\"gone\"").as_str(), &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let played = render(src, &opts, &Tone, &crate::PrepareControl::default()).unwrap();
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
    let doc = model::ScoreDoc::test_load(&src("singer_t1")).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.singers(), vec![AssetRef { id: "singer_t1".into(), rev: "r1".into() }]);
    assert_eq!(doc.dependencies().len(), 1);
    assert!(doc.samples().is_empty());
    let opts = RenderOpts { seconds: Some(2.0), ..RenderOpts::default() };
    // Not registered yet: the built-in alto sings in its place.
    let waiting = render(&src("singer_t2"), &opts, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert_eq!(crate::render::missing_singers(&model::ScoreDoc::test_load(&src("singer_t2")).unwrap()).len(), 1);
    let sung = render(&src("singer_t1"), &opts, &Singers, &crate::PrepareControl::default()).unwrap();
    assert!(crate::render::missing_singers(&doc).is_empty());
    let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak(&waiting.left) > 0.01 && peak(&sung.left) > 0.01);
    assert_ne!(waiting.left, sung.left);
    // A name that is neither built in nor an asset is reported.
    assert!(model::ScoreDoc::test_load(&src("x").replace("asset(\"x\", \"r1\")", "@choirboy")).is_err());
}

#[test]
fn timing_reads_the_sections_without_rendering() {
    let src = PULSE_A.replace("Score{clock s pulse}", "let hat = Loop{hits: \"x.x.\" step: 0.5}\nlet a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 1 loops: [hat]}\nScore{clock s pulse hat a b arrange: [a b]}");
    let t = crate::render::timing_controlled(&src, None, &crate::PrepareControl::default()).unwrap();
    assert_eq!((t.bpm, t.beats_per_bar, t.seconds), (120.0, 4.0, 6.0));
    assert_eq!(t.sections, vec![("a".to_string(), 0.0, 2.0), ("b".to_string(), 2.0, 6.0)]);
}

#[test]
fn a_loop_following_a_param_fades_with_it() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 params: {intensity: 0.0}}\nlet s = Synth{preset: @pluck}\nlet layer = Loop{play: s beats: 1 follow: {param: @intensity from: 0.2 to: 0.6} code: fn(i) { play(60) sleep(1) }}\nScore{clock s layer}\n";
    let vels = |value: f32| {
        let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
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
    let a = render(drums, &RenderOpts { seconds: Some(2.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(peak(&a.left) > 0.05, "the modelled kit is silent");
    assert!(a.left.iter().position(|x| x.abs() > 1e-6).is_some_and(|at| at < 240), "the first hit sounds within 5 ms");
    // The modelled piano, down to its lowest octave.
    let piano = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet grand = Instrument{kind: @piano}\nlet low = Loop{play: grand beats: 4 code: fn(i) { play([21 28 33], {dur: 2}) sleep(4) }}\nScore{clock grand low}\n";
    let p = render(piano, &RenderOpts { seconds: Some(1.5), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(peak(&p.left) > 0.01 && p.left.iter().all(|x| x.is_finite()), "piano peak {}", peak(&p.left));
    // A SoundFont program from the library.
    let sf = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet gm = Instrument{kind: @soundfont src: asset(\"font\", \"1\") program: 40}\nlet line = Loop{play: gm beats: 2 code: fn(i) { play(@c4, {dur: 1}) sleep(2) }}\nScore{clock gm line}\n";
    let silent = render(&sf.replace("\"font\"", "\"never_registered\""), &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert_eq!(peak(&silent.left), 0.0, "an unloaded SoundFont is silent, not an error");
    let s = render(sf, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &Font, &crate::PrepareControl::default()).unwrap();
    assert!(peak(&s.left) > 0.01, "the SoundFont is silent");
    assert!(model::test_load(&drums.replace("@drums", "@kazoo")).is_err());
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
    let mut composer = Composer::new(model::test_load(&src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    assert!(events.len() <= 20, "streamed up to the horizon, not the whole song: {}", events.len());
    composer.pump(1000.0, &mut events);
    let programs: Vec<u8> = events.iter().filter_map(|e| match e.kind { EventKind::Program(p) => Some(p), _ => None }).collect();
    assert_eq!(programs, [0, 40]);
    assert_eq!(events.iter().filter(|e| e.kind == EventKind::Note).count(), 4000);
    // It renders through the graph, the feed never overflowing.
    let r = render(&src, &RenderOpts { seconds: Some(4.0), ..RenderOpts::default() }, &Font, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(peak(&r.left) > 0.01);
}

#[test]
fn feeds_are_freed_with_their_music() {
    use crate::game::{music, Clocking};
    use makepad_audio_engine::feed::registry_len;
    let make = |clocking| {
        let (handle, inst) = music(PULSE_A, 48_000.0, clocking, &crate::PrepareControl::default()).unwrap();
        drop(inst);
        drop(handle);
    };
    make(Clocking::Headless);
    make(Clocking::Realtime);
    render(PULSE_A, &RenderOpts { seconds: Some(0.1), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let before = registry_len();
    for _ in 0..20 {
        make(Clocking::Headless);
        make(Clocking::Realtime);
        render(PULSE_A, &RenderOpts { seconds: Some(0.1), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
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
    let r = render(TEMPO, &RenderOpts { seconds: Some(9.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let heard = onsets(&r.left);
    assert!(heard.len() >= 12, "{heard:?}");
    // The first note is under the output's startup fade at frame zero of the trimmed export.
    assert!(heard[0] < CHUNK, "beat zero first crossed the threshold at frame {}", heard[0]);
    for (beat, at) in heard.iter().take(12).enumerate().skip(1) {
        let want = (r.seconds_at(beat as f64) * 48_000.0).ceil() as usize;
        assert!(at.abs_diff(want) <= 1, "beat {beat}: heard at {at}, the map says {want}");
    }
    // 4 beats at 100, 4 at 150, then down to 60.
    assert!((r.seconds_at(4.0) - 2.4).abs() < 1e-9);
    assert!((r.seconds_at(8.0) - 4.0).abs() < 1e-9);
    let t = crate::render::timing_controlled(TEMPO, Some(9.0), &crate::PrepareControl::default()).unwrap();
    assert_eq!(t.sections[1].0, "fast");
    assert!((t.sections[1].1 - 2.4).abs() < 1e-9 && (t.sections[2].1 - 4.0).abs() < 1e-9);
    // A game's headless music in odd blocks plays the same samples, and
    // its beat events follow the map.
    use crate::game::{music, Clocking, MusicEvent};
    use makepad_audio_synth::Instrument;
    let (mut handle, mut inst) = music(TEMPO, 48_000.0, Clocking::Headless, &crate::PrepareControl::default()).unwrap();
    let latency = r.latency as usize;
    let total = r.left.len() + latency;
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
    let first = out[latency..].iter().zip(&r.left).position(|(a, b)| a != b);
    assert!(first.is_none(), "the game's music differs from the offline render at {first:?}");
    assert_eq!(beats.iter().map(|b| b.0).take(10).collect::<Vec<_>>(), (0..10).collect::<Vec<u64>>());
}

#[test]
fn a_param_moves_the_tempo_and_sections_repeat() {
    let src = TEMPO.replace("params: {mood: 0}}", "params: {mood: 0} tempo: {param: @mood from: 60 to: 180 ramp: 2}}");
    let mut composer = Composer::new(model::test_load(&src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    let mut events = Vec::new();
    composer.pump(2.5, &mut events);
    composer.set_param("mood", 1.0);
    assert_eq!(composer.tempo_out.last().map(|e| (e.beat, e.bpm, e.ramp)), Some((3.0, 180.0, 2.0)));
    // A section repeats itself until a goto, forward references resolve.
    let looping = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet pulse = Loop{play: s beats: 1 code: fn(i) { play(60) sleep(1) }}\nlet intro = Section{bars: 1 loops: [pulse] next: @calm}\nlet calm = Section{bars: 1 loops: [pulse] repeat: true}\nlet storm = Section{bars: 1 loops: [pulse] next: @self}\nScore{clock s pulse intro calm storm arrange: [intro calm storm]}\n";
    let mut c = Composer::new(model::test_load(looping).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    assert_eq!(c.section_at(2.0).as_deref(), Some("intro"));
    assert_eq!(c.section_at(40.0).as_deref(), Some("calm"));
    c.goto_section("storm", 40.0, -1.0).unwrap();
    assert_eq!(c.section_at(100.0).as_deref(), Some("storm"));
    assert!(model::test_load(&looping.replace("next: @calm", "next: @nowhere")).is_err());
}

#[test]
fn an_id_in_arithmetic_is_an_error_not_silence() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet bad = Loop{play: s beats: 1 code: fn(i) { let c = [@c4 @e4] play(at(c, 0) + 12) sleep(1) }}\nlet good = Loop{play: s beats: 1 code: fn(i) { play(note(@c4) + floor(12.7) + max(0, -3) + abs(-0)) sleep(1) }}\nScore{clock s bad good}\n";
    let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    let mut events = Vec::new();
    composer.pump(2.0, &mut events);
    let errors = composer.errors();
    assert!(errors.iter().any(|e| e.node.as_deref() == Some("bad") && e.message.contains("not a number")), "{errors:?}");
    assert!(events.iter().all(|e| e.note.is_finite()));
    assert!(events.iter().any(|e| e.note == 72.0), "{events:?}");
    let r = render(src, &RenderOpts { seconds: Some(3.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(peak(&r.left[96_000..]) > 0.01, "the rest of the music plays");
}

#[test]
fn sends_bars_and_sections() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 4 sends: {verb: 0.5} code: fn(i) { if section() == \"b\" { play(60 + section_bar()) } else { play(48 + bar()) } sleep(4) }}\nlet verb = Reverb{room: 0.9 mix: 1.0}\nlet one = Section{bars: 2 loops: [a]}\nlet b = Section{bars: 2 loops: [a]}\nScore{clock s a verb one b arrange: [one b]}\n";
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.loops[0].sends, vec![("verb".to_string(), 0.5)]);
    // The send is a Bus into the reverb; the loop still reaches the master.
    let g = graph(&doc);
    assert_eq!((g.node("a_send_verb").type_name.as_str(), g.wired("a_send_verb", "audio"), g.param("a_send_verb", "gain")), ("Bus", vec!["a".to_string()], Literal::Num(0.5)));
    assert_eq!(g.wired("verb", "audio"), ["a_send_verb"]);
    assert_eq!(g.wired("master", "audio"), ["a", "verb"]);
    // bar() counts from the piece's start; section_bar() within a section.
    let mut composer = Composer::new(model::test_load(src).unwrap().into_evaluated());
    let mut events = Vec::new();
    composer.pump(16.0, &mut events);
    let notes: Vec<f32> = events.iter().map(|e| e.note).collect();
    assert_eq!(notes, [48.0, 49.0, 60.0, 61.0]);
    assert!(composer.errors().is_empty());
    // The tail follows the reverb, and a render reports loop errors.
    assert!(crate::render::tail_seconds(&doc) > 5.0);
    let broken = src.replace("play(48 + bar())", "play(48 + nope())");
    let r = render(&broken, &RenderOpts { seconds: Some(1.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(!r.errors.is_empty());
    assert!(model::test_load(&src.replace("{verb: 0.5}", "{verbb: 0.5}")).err().unwrap()[0].message.contains("did you mean `verb`"));
}

#[test]
fn fx_settings_follow_params_and_sections_set_them() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 params: {build: 0}}\nlet s = Synth{preset: @supersaw}\nlet a = Loop{play: s beats: 1 code: fn(i) { play(48, {dur: 1}) sleep(1) }}\nlet sweep = Lowpass{in: [a] cutoff: {param: @build from: 200 to: 8000 ramp: 4} res: 1}\nlet low = Section{bars: 1 loops: [a]}\nlet up = Section{bars: 2 loops: [a] set: {build: 1}}\nScore{clock s a sweep low up arrange: [low up]}\n";
    let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    // It starts where the param is, and the section moves it over 4 beats.
    assert_eq!(composer.automation_out.drain(..).map(|(fx, e)| (fx, e.beat, e.value, e.ramp)).collect::<Vec<_>>(), [("sweep".to_string(), 0.0, 200.0, 0.0)]);
    let mut events = Vec::new();
    composer.pump(8.0, &mut events);
    assert_eq!(composer.automation_out.iter().map(|(_, e)| (e.beat, e.value, e.ramp)).collect::<Vec<_>>(), [(4.0, 8000.0, 4.0)]);
    // In the render the high bands open up after the section starts.
    let r = render(src, &RenderOpts { seconds: Some(6.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let hf = |from: usize, to: usize| r.left[from..to].windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / (to - from) as f32;
    assert!(hf(4 * 24_000, 5 * 24_000) > hf(24_000, 2 * 24_000) * 2.0, "the sweep did not open: {} vs {}", hf(24_000, 48_000), hf(96_000, 120_000));
    assert!(model::test_load(&src.replace("set: {build: 1}", "set: {buld: 1}")).is_err());
}

#[test]
fn the_loop_code_toolkit() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet chords = Loop{play: s beats: 4 code: fn(i) { put(@chord, chord(@a3, @minor)) log(rev([1 2 3])) sleep(4) }}\nlet lead = Loop{play: s beats: 4 humanize: {time: 0.02 vel: 0.2} code: fn(i) { let c = get(@chord) sleep(phrase([c[0] c[1]], [0.5 0.5])) sleep(arp(c, @updown, 0.25)) strum(c, 0.05) log(deg(3, @c4, @major)) log(snap(61, @c4, @major)) log(rot([1 2 3], 1)) sleep(1) }}\nScore{clock s chords lead}\n";
    let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
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
    let mut again = Composer::new(model::test_load(src).unwrap().into_evaluated());
    let mut events2 = Vec::new();
    again.pump(4.0, &mut events2);
    assert_eq!(events.iter().map(|e| e.beat).collect::<Vec<_>>(), events2.iter().map(|e| e.beat).collect::<Vec<_>>());
}

#[test]
fn a_playing_note_glides_its_pitch_level_and_cutoff() {
    // A fixed recipe (the presets' sound design may change; the glide may not).
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{recipe: \"saw freq=440 detune=12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=220 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.1 lp=3000\"}\nlet a = Loop{play: s beats: 4 code: fn(i) { let h = play(@a2, {dur: 4}) sleep(1) glide(h, {by: 12 cutoff: 6000}, 1) sleep(3) }}\nScore{clock s a}\n";
    let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    let mut events = Vec::new();
    composer.pump(4.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    // play() hands out the handle the glide names.
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, EventKind::Glide);
    assert!(events[0].opts.handle != 0 && events[0].opts.handle == events[1].opts.handle);
    assert_eq!((events[1].beat, events[1].dur, events[1].opts.glide_from), (1.0, 1.0, 12.0));
    // The held note is an octave up once the glide is over.
    let r = render(src, &RenderOpts { seconds: Some(1.9), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    let crossings = |from: f64, to: f64| {
        let s = &r.left[(from * 48_000.0) as usize..(to * 48_000.0) as usize];
        s.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32
    };
    let (before, after) = (crossings(0.1, 0.45), crossings(1.1, 1.45));
    assert!(after > before * 1.6, "no octave glide: {before} -> {after} crossings");
    // A second play gives a new handle; a glide on none is ignored.
    let other = src.replace("glide(h,", "glide(0,");
    let mut composer = Composer::new(model::test_load(&other).unwrap().into_evaluated());
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
        let r = render(&src, &RenderOpts { seconds: Some(2.4), ..RenderOpts::default() }, &Hits, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
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
    let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    let mut events = Vec::new();
    composer.pump(12.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    let at = |note: f32| events.iter().filter(|e| e.note == note).map(|e| e.beat).collect::<Vec<f64>>();
    assert_eq!(at(48.0), [1.5, 5.5, 9.5]);
    // Its cycle from 10.5 waits for the cue the lead gives next (at 13.5).
    assert_eq!(at(60.0), [1.5, 5.5, 9.5, 13.5]);
    // Waiting for a cue nothing gives says so.
    let mut composer = Composer::new(model::test_load(&src.replace("wait(@drop)", "wait(@dorp)")).unwrap().into_evaluated());
    composer.pump(4.0, &mut Vec::new());
    let errors = composer.errors();
    assert!(errors.iter().any(|e| e.message.contains("nothing cues @dorp") && e.message.contains("@drop")), "{errors:?}");
}

#[test]
fn fx_scopes_play_their_notes_through_their_own_effects() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 8 code: fn(i) {\n    play(72, {dur: 0.1})\n    fx(@reverb, {room: 0.95 mix: 1.0}, fn() {\n        play(60, {dur: 0.1})\n        fx(@echo, {time: 0.5}, fn() { play(64, {dur: 0.1}) })\n    })\n    sleep(8)\n}}\nScore{clock s a}\n";
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
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
    let mut composer = Composer::new(model::test_load(src).unwrap().into_evaluated());
    let mut events = Vec::new();
    composer.pump(8.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    let on = |note: f32| events.iter().find(|e| e.note == note).map(|e| e.loop_key);
    assert_eq!((on(72.0), on(60.0), on(64.0)), (Some(key("a")), Some(key("a_fx1_in")), Some(key("a_fx2_in"))));
    let sets: Vec<(String, u8, f32)> = composer.automation_out.iter().map(|(fx, e)| (fx.clone(), e.param, e.value)).collect();
    assert_eq!(sets, [("a_fx1".to_string(), 0, 0.95), ("a_fx1".to_string(), 1, 1.0), ("a_fx2".to_string(), 0, 0.5)]);
    // The reverb rings on after the notes; without fx() it does not.
    let tail = |src: &str| {
        let r = render(src, &RenderOpts { seconds: Some(3.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
        peak(&r.left[(2.0 * 48_000.0) as usize..])
    };
    let dry = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 8 code: fn(i) { play(72, {dur: 0.1}) play(60, {dur: 0.1}) play(64, {dur: 0.1}) sleep(8) }}\nScore{clock s a}\n";
    assert!(tail(src) > tail(dry) * 4.0 + 1e-4, "{} vs {}", tail(src), tail(dry));
    // An unknown effect is a load error.
    assert!(model::test_load(&src.replace("@echo", "@wobble")).err().unwrap()[0].message.contains("fx(@wobble): no such effect"));
}

#[test]
fn custom_envelopes_shape_a_notes_level_and_cutoff() {
    let env = makepad_score_player::Env { pts: { let mut p = [[0.0; 2]; 8]; p[0] = [0.0, 0.0]; p[1] = [1.0, 1.0]; p[2] = [2.0, 0.5]; p }, n: 3, curve: 1.0 };
    assert_eq!((env.level(0.5), env.level(1.5), env.level(9.0)), (0.5, 0.75, 0.5));
    let run = |opts: &str| {
        let src = format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet s = Synth{{recipe: \"saw freq=440 detune=12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-12 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=440 detune=-5 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.13 lp=5200; saw freq=220 a=0.01 d=0.4 s=0.8 r=0.3 gain=0.1 lp=3000\"}}\nlet a = Loop{{play: s beats: 8 code: fn(i) {{ play(@a2, {opts}) sleep(8) }}}}\nScore{{clock s a}}\n");
        render(&src, &RenderOpts { seconds: Some(1.6), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}")).left
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
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 params: {bright: 0} midi: {bright: 74}}\nlet s = Synth{preset: @pluck}\nlet gear = Instrument{kind: @midi channel: 2 port: \"IAC Bus 1\"}\nlet keys_in = MidiIn{device: \"any\" channel: 1}\nlet keys = Loop{play: s in: keys_in}\nlet arp = Loop{play: gear beats: 1 code: fn(i) { log(held()) log(cc(74)) sleep(arp(held(), @up, 0.25)) }}\nlet sweep = Lowpass{in: [keys] cutoff: {param: @bright from: 200 to: 8000}}\nScore{clock s gear keys_in keys arp sweep}\n";
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.loops[0].input.as_deref(), Some("keys_in"));
    assert_eq!(doc.clock.midi, vec![("bright".to_string(), 74)]);
    let g = graph(&doc);
    assert_eq!((g.param("arp", "inst"), g.param("arp", "channel"), g.param("arp", "midi_port")), (Literal::Str("midi".into()), Literal::Num(2.0), Literal::Str("IAC Bus 1".into())));
    // The keyboard loop plays its stream on the audio thread: device and
    // channel filters, no stages: bend range, per-note channels, master.
    assert_eq!((g.param("keys", "midi_device"), g.param("keys", "midi_channel")), (Literal::Str("any".into()), Literal::Num(1.0)));
    assert!(matches!(g.param("keys", "stream"), Literal::Arr(words) if words.len() == 3));
    assert!(matches!(g.param("arp", "stream"), Literal::Arr(words) if words.is_empty()));
    let mut composer = Composer::new(model::test_load(src).unwrap().into_evaluated());
    composer.automation_out.clear();
    // Keys: held for the loop code; the controller moves the param.
    composer.midi_in([0x90, 60, 100], 3.0);
    composer.midi_in([0x90, 64, 127], 3.0);
    composer.midi_in([0xb0, 74, 127], 3.0);
    // The controller moved the param and the filter following it, now.
    assert_eq!(composer.params, [("bright".to_string(), 1.0)]);
    assert_eq!(composer.automation_out.iter().map(|(fx, e)| (fx.as_str(), e.beat, e.value)).collect::<Vec<_>>(), [("sweep", 3.0, 8000.0)]);
    let mut events = Vec::new();
    composer.pump(1.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    let logs = &composer.loops.iter().find(|l| l.name == "arp").unwrap().last_logs;
    assert_eq!(logs.iter().map(|l| l.split(": ").nth(1).unwrap().to_string()).collect::<Vec<_>>(), ["[60 64]", "1"]);
    assert_eq!(events.iter().filter(|e| e.loop_key == key("arp")).map(|e| (e.beat, e.note)).collect::<Vec<_>>(), [(0.0, 60.0), (0.25, 64.0)]);
    assert!(events.iter().all(|e| e.loop_key != key("keys")), "the composer plays nothing on a keyboard loop");
    composer.midi_in([0x80, 60, 0], 4.0);
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
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!((doc.clock.osc_in, doc.clock.osc_out.as_deref()), (Some(9000), Some("127.0.0.1:57120")));
    let mut composer = Composer::new(model::test_load(src).unwrap().into_evaluated());
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
    let mut composer = Composer::new(model::test_load(&src.replace(" osc_out: \"127.0.0.1:57120\"", "")).unwrap().into_evaluated());
    composer.pump(2.0, &mut Vec::new());
    assert!(composer.errors().iter().any(|e| e.message.contains("osc_send(\"/beat\"): where to?")), "{:?}", composer.errors());
}

#[test]
fn an_input_loop_passes_the_audio_input_through_its_fx() {
    use makepad_audio_engine::feed::audio_in;
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet mic = Instrument{kind: @input gain: 0.5}\nlet voice = Loop{play: mic}\nlet echo = Delay{in: [voice] time: 0.25 feedback: 0.3 mix: 0.3}\nScore{clock mic voice echo}\n";
    let loaded = model::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let g = graph(&loaded.doc);
    assert_eq!((g.param("voice", "inst"), g.param("voice", "gain")), (Literal::Str("input".into()), Literal::Num(0.5)));
    let mut instance = instantiate(&loaded.doc, 48_000.0, &crate::PrepareControl::default()).unwrap();
    let mut composer = Composer::new(loaded.into_evaluated());
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
    let mut c = Composer::new(model::test_load(src).unwrap().into_evaluated());
    assert_eq!(notes(&mut c, 24.0), [(0.0, 60.0), (4.0, 60.0), (8.0, 62.0), (12.0, 62.0), (16.0, 64.0), (20.0, 64.0)]);
    assert_eq!((c.song_beat(10.0), c.song_beat(30.0)), (Some(10.0), None));
    // A seek to z's start from bar 2: z plays from there, and the song
    // position follows.
    let mut c = Composer::new(model::test_load(src).unwrap().into_evaluated());
    assert_eq!(notes(&mut c, 4.0), [(0.0, 60.0)]);
    assert_eq!(c.seek(16.0, 3.0, -1.0), 4.0);
    assert_eq!(notes(&mut c, 12.0), [(4.0, 64.0), (8.0, 64.0)]);
    assert_eq!(c.song_beat(5.0), Some(17.0));
    // Mid-section: from z's second bar.
    let mut c = Composer::new(model::test_load(src).unwrap().into_evaluated());
    c.seek(20.0, 0.0, 0.0);
    assert_eq!(notes(&mut c, 8.0), [(0.0, 64.0), (4.0, 64.0)]);
    assert_eq!((c.song_beat(2.0), c.song_beat(6.0)), (Some(22.0), None));
    // Looping y: x x y y y y …
    let mut c = Composer::new(model::test_load(src).unwrap().into_evaluated());
    c.set_loop(Some((8.0, 16.0)));
    assert_eq!(notes(&mut c, 32.0), [(0.0, 60.0), (4.0, 60.0), (8.0, 62.0), (12.0, 62.0), (16.0, 62.0), (20.0, 62.0), (24.0, 62.0), (28.0, 62.0)]);
    assert_eq!(c.song_beat(21.0), Some(13.0));
    // A cycle crossing the loop end is cut there (an 8-beat loop).
    let long = src.replace("beats: 4 code: fn(i) {", "beats: 8 code: fn(i) { play(50) sleep(4)");
    let mut c = Composer::new(model::test_load(&long).unwrap().into_evaluated());
    c.set_loop(Some((0.0, 4.0)));
    let got = notes(&mut c, 12.0);
    assert!(got.iter().all(|(b, _)| (b % 4.0).abs() < 1e-9) && got.iter().filter(|(_, n)| *n == 50.0).count() == 3, "{got:?}");
}

#[test]
fn a_note_lists_voices_mute_and_solo() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet p = Synth{preset: @pluck}\nlet rh = [[0 @c5 1] [1 @e5 1]]\nlet lh = [[0 @c3 2]]\nlet piece = Loop{play: p beats: 2 notes: [rh lh]}\nScore{clock p piece}\n";
    let notes = |src: &str| {
        let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
        doc.loops[0].notes.as_ref().unwrap().iter().map(|n| n.note).collect::<Vec<f32>>()
    };
    assert_eq!(notes(src), [72.0, 48.0, 76.0]);
    assert_eq!(notes(&src.replace("notes: [rh lh]", "notes: [rh lh] mute_voices: [@lh]")), [72.0, 76.0]);
    assert_eq!(notes(&src.replace("notes: [rh lh]", "notes: [rh lh] solo_voices: [@lh]")), [48.0]);
}

#[test]
fn a_seek_without_an_arrangement_resumes_every_loop_mid_cycle() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet a = Loop{play: s beats: 8 code: fn(i) { for k in 0..8 { play(60 + k) sleep(1) } }}\nlet b = Loop{play: s beats: 4 code: fn(i) { play(40) sleep(4) }}\nScore{clock s a b}\n";
    let mut c = Composer::new(model::test_load(src).unwrap().into_evaluated());
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
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.ceiling, -1.5);
    let g = graph(&doc);
    assert_eq!((g.node("lim").type_name.as_str(), g.wired("lim", "audio"), g.param("lim", "ceiling"), g.param("lim", "gain")), ("Limiter", vec!["glue".to_string()], Literal::Num(-6.0), Literal::Num(2.0)));
    assert_eq!((g.param("master", "ceiling"), g.param("master", "gain")), (Literal::Num(-1.5), Literal::Num(1.0)));
    let r = render(src, &RenderOpts { seconds: Some(2.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
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
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let eq = doc.fx.iter().find(|f| f.name == "tone").unwrap();
    assert_eq!(&eq.params[..8], &[4.0, 80.0, 0.0, 0.7, 1.0, 900.0, -3.0, 1.4]);
    let g = graph(&doc);
    assert_eq!((g.wired("glue", "audio"), g.wired("glue", "key")), (vec!["wide".to_string()], vec!["kick".to_string()]));
    let r = render(src, &RenderOpts { seconds: Some(2.0), ..RenderOpts::default() }, &NoSamples, &crate::PrepareControl::default()).unwrap();
    assert!(peak(&r.left) > 0.01 && r.left.iter().all(|x| x.is_finite()));
    assert!(model::test_load(&src.replace("@peak, 900", "@bump, 900")).is_err());
}

/// A kit's per-slot gains and tunings reach its loop's node (a drum tuned
/// down plays at a pitch below 1).
#[test]
fn kit_slots_keep_their_gains_and_tunings() {
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet kit = Drums{kit: @analog_hybrid tune: {kick: -12 snare: 7} gain: {kick: 0.5}}\nlet beat = Loop{play: kit hits: \"x...\"}\nScore{clock kit beat}\n";
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
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
    let plain = render(&score(""), &opts, &Cycle, &crate::PrepareControl::default()).unwrap();
    assert!((hz(&plain.left) - 400.0).abs() < 15.0, "{}", hz(&plain.left));
    // The loop holds it: still sounding at 0.9 s.
    assert!(plain.left[43_000..43_500].iter().any(|x| x.abs() > 0.1));
    // bend: up an octave by beat 0.5 (0.25 s), held.
    let doc = model::ScoreDoc::test_load(&score(", {bend: [[0 0] [0.5 12]]}")).unwrap_or_else(|d| panic!("{d:?}"));
    let l = &doc.loops[0];
    assert_eq!(l.note_opts.len(), 1, "{:?}", l.notes);
    assert_eq!(l.note_opts[0].1.bend.n, 2);
    let bent = render(&score(", {bend: [[0 0] [0.5 12]]}"), &opts, &Cycle, &crate::PrepareControl::default()).unwrap();
    assert!((hz(&bent.left) - 800.0).abs() < 25.0, "{}", hz(&bent.left));
    // arp: [0 12] stepping every beat (0.5 s): an octave up from 0.5 s.
    let arp = render(&score(", {arp: [0 12] arp_step: 1}"), &opts, &Cycle, &crate::PrepareControl::default()).unwrap();
    assert!(hz(&arp.left[4_800..]) > 600.0);
    // vib: the pitch swings, but averages to the note.
    let vib = render(&score(", {vib: [5 1]}"), &opts, &Cycle, &crate::PrepareControl::default()).unwrap();
    assert!((hz(&vib.left) - 400.0).abs() < 40.0);
    assert_ne!(vib.left, plain.left);
    // A note's pan moves it from the loop's.
    let right = render(&score(", {pan: 1}"), &opts, &Cycle, &crate::PrepareControl::default()).unwrap();
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
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(doc.positions, vec![("s".to_string(), (120.0, 80.0))]);
    let mut composer = Composer::new(model::test_load(src).unwrap_or_else(|d| panic!("{d:?}")).into_evaluated());
    let mut events = Vec::new();
    composer.pump(2.0, &mut events);
    assert!(composer.errors().is_empty(), "{:?}", composer.errors());
    assert!(events.iter().any(|e| e.note == 64.0) && events.iter().any(|e| e.note == 60.0), "{events:?}");
}

#[test]
fn playing_control_reads_follow_the_generation_and_release_to_the_authored_value() {
    use makepad_audio_engine::{feed::{automation_feed, AutomationEvent}, schedule::ClockChunk};
    const SLOT: &str = "control_readback";
    const WIRE: &str = "Port{node: cv name: @hz}";
    const FOLLOW: &str = "{param: @open from: 1 to: 8}";
    let source = format!(r#"use mod.score.*
let clock = Clock{{bpm: 120 params: {{open: 0}}}}
let cv = ShaderFx{{code: "let hz = output(1) fn effect(l,r) {{ hz = 4800.0; 0.0 }}"}}
let lp = Lowpass{{in: cv cutoff: {WIRE} res: {FOLLOW} mix: {{param: @open from: 0 to: 1}}}}
Score{{clock cv lp}}"#);
    let mut registry = makepad_flow::document::Registry::with_builtins();
    makepad_audio_engine::nodes::register_nodes(&mut registry).unwrap();
    crate::node::register(&mut registry).unwrap();
    let slot = crate::node::slot(SLOT);
    slot.set_source(&source, &crate::PrepareControl::default()).unwrap();
    let score = crate::player::live_node(&crate::node::descriptor(), "score".into(), &[("score", Literal::Str(SLOT.into()))], &[]);
    let mut node = makepad_audio_engine::nested::factory(crate::node::TYPE).unwrap()(&score, 48_000, 1, None).unwrap();
    let feed = automation_feed(&format!("{SLOT}/lp"));
    let param = makepad_score_player::FxKind::Lowpass.param_index("res").unwrap() as u8;
    assert!(feed.send(AutomationEvent { beat: 0.0, value: 5.0, param, ramp: 0.0 }));
    let mix = makepad_score_player::FxKind::Lowpass.param_index("mix").unwrap() as u8;
    assert!(feed.send(AutomationEvent { beat: 1.0, value: 7.0, param, ramp: 0.0 }));
    assert!(feed.send(AutomationEvent { beat: 1.0, value: 0.75, param: mix, ramp: 0.0 }));
    let mut block = |beat| {
        let clock = ClockChunk { start: beat, tempo: makepad_score_player::Tempo::linear(beat, 0.0), ..Default::default() };
        node.run(&clock, &mut [0.; 64], &mut [0.; 64]);
    };
    block(0.0);
    let reader = slot.playing_controls().unwrap();
    let cutoff = reader.get("lp.cutoff").unwrap();
    let res = reader.get("lp.res").unwrap();
    assert_eq!((cutoff.driver.as_deref(), cutoff.value()), (Some("cv.hz"), 4800.0));
    assert_eq!((res.driver.as_deref(), res.value()), (Some("automation (clock.open)"), 5.0));
    let unplugged = source.replace(WIRE, "900");
    slot.set_source(&unplugged, &crate::PrepareControl::default()).unwrap();
    assert_eq!(slot.playing_controls().unwrap().generation, reader.generation, "a pending player is inaudible");
    block(0.0);
    let reader = slot.playing_controls().unwrap();
    let cutoff = reader.get("lp.cutoff").unwrap();
    assert_eq!((cutoff.driver.as_deref(), cutoff.value()), (None, 900.0));
    assert_eq!(reader.get("lp.res").unwrap().value(), 5.0, "the continuing automation keeps its target");
    slot.set_source(&unplugged.replace(FOLLOW, "2"), &crate::PrepareControl::default()).unwrap();
    // Still in the feed at the swap, alongside an event already waiting in the effect.
    assert!(feed.send(AutomationEvent { beat: 1.0, value: 8.0, param, ramp: 0.0 }));
    block(1.0);
    let reader = slot.playing_controls().unwrap();
    let res = reader.get("lp.res").unwrap();
    assert_eq!(reader.get("lp.mix").unwrap().value(), 0.75, "the other binding keeps its pending event");
    assert_eq!((res.driver.as_deref(), res.value()), (None, 2.0), "removing the automation frees the knob at its authored value");
}

/// A runner defers the slot's prepare, coalesces edits while it is out,
/// and returns plan errors to the host's next pump.
#[test]
fn a_slot_runner_defers_prepares_and_keeps_the_latest_doc() {
    use makepad_audio_engine::schedule::ClockChunk;
    use std::sync::{Arc, Mutex};

    const SLOT: &str = "deferred_score_prepare";
    let mut registry = makepad_flow::document::Registry::with_builtins();
    makepad_audio_engine::nodes::register_nodes(&mut registry).unwrap();
    crate::node::register(&mut registry).unwrap();
    let slot = crate::node::slot(SLOT);
    let jobs = Arc::new(Mutex::new(Vec::<Box<dyn FnOnce() + Send>>::new()));
    let queued = jobs.clone();
    slot.set_runner(Arc::new(move |job| {
        queued.lock().unwrap().push(job);
        Ok(())
    }));
    let score = crate::player::live_node(&crate::node::descriptor(), "score".into(), &[("score", Literal::Str(SLOT.into()))], &[]);
    let mut node = makepad_audio_engine::nested::factory(crate::node::TYPE).unwrap()(&score, 48_000, 1, None).unwrap();
    let revision = |loops: &[&str], label: &str| {
        let mut source = String::from("use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\n");
        for name in loops {
            source.push_str(&format!("let {name} = Loop{{play: s beats: 1 notes: [[0 @c4 0.5 1]]}}\n"));
        }
        source.push_str(&format!("Score{{clock s {} label: {label:?}}}\n", loops.join(" ")));
        model::ScoreDoc::test_load(&source).unwrap()
    };
    slot.set_doc(revision(&["first"], "")).expect("submitting the first prepare succeeds");
    assert_eq!(jobs.lock().unwrap().len(), 1, "the first prepare waits in the runner");
    assert_eq!(slot.playing(), 0, "a deferred prepare starts no player");
    assert!(!node.running(), "the mailbox is empty until the runner executes the job");

    slot.set_doc(revision(&["middle"], "")).expect("an edit while preparing stays pending");
    let newest = revision(&["newest"], "");
    slot.set_doc(newest.clone()).expect("the newest edit replaces the pending document");
    assert_eq!(jobs.lock().unwrap().len(), 1, "two edits leave exactly one prepare out");
    let run_next = || {
        let job = jobs.lock().unwrap().pop().expect("one queued prepare");
        job();
    };
    run_next();
    assert!(!node.running(), "the superseded first prepare never posts");
    assert_eq!(slot.playing(), 0);
    slot.pump().expect("the superseded prepare leaves no diagnostic");
    assert_eq!(jobs.lock().unwrap().len(), 1, "completion dispatches only the newest edit");
    run_next();
    let (mut left, mut right) = ([0.0; CHUNK], [0.0; CHUNK]);
    node.run(&ClockChunk::default(), &mut left, &mut right);
    let playing = slot.playing();
    assert!(playing > 0);
    assert!(slot.read(&format!("{SLOT}/newest"), "peak").is_some(), "the posted player uses the newest document");
    assert!(slot.read(&format!("{SLOT}/middle"), "peak").is_none());
    slot.pump().unwrap();
    assert!(jobs.lock().unwrap().is_empty());

    let mut invalid = crate::player::slice(&newest, SLOT, crate::player::ClockMode::Host);
    let newest_id = format!("{SLOT}/newest");
    let playing_node = invalid.nodes.iter_mut().find(|n| n.id == newest_id).unwrap();
    let (_, amp) = playing_node.params.iter_mut().find(|(name, _)| name == "amp").unwrap();
    *amp = Literal::Num(-1.0);
    slot.set_slice(newest.clone(), invalid.clone()).unwrap();
    slot.set_doc(newest.clone()).unwrap();
    run_next();
    slot.pump().expect("the obsolete error is silent");
    node.run(&ClockChunk::default(), &mut left, &mut right);
    assert_eq!(slot.playing(), playing, "returning to the playing graph keeps its generation");
    assert!(jobs.lock().unwrap().is_empty(), "returning to the playing graph queues no replacement");

    slot.refresh().unwrap();
    slot.set_slice(newest.clone(), invalid.clone()).unwrap();
    run_next();
    slot.pump().expect("the superseded refresh is silent");
    run_next();
    let diags = slot.pump().expect_err("the latest invalid replacement reports its own error");
    assert!(diags.iter().any(|d| d.message.contains("audio control is outside its range")), "{diags:?}");
    node.run(&ClockChunk::default(), &mut left, &mut right);
    assert_eq!(slot.playing(), playing, "a failed replacement preserves the audible generation");
    slot.pump().unwrap();
    assert!(jobs.lock().unwrap().is_empty());

    slot.set_doc(newest.clone()).unwrap();
    let fresh = slot.prepare_fresh().unwrap().expect("a rewind prepared successfully");
    slot.set_slice(newest.clone(), invalid).unwrap();
    slot.post_fresh(fresh);
    run_next(); // the ordinary job superseded by the rewind
    assert!(jobs.lock().unwrap().is_empty(), "the edit waits for the rewind's mailbox");
    node.run(&ClockChunk::default(), &mut left, &mut right);
    let rewound = slot.playing();
    assert!(rewound > playing, "an invalid edit cannot withdraw the transport's rewind");
    slot.pump().unwrap();
    run_next();
    let diags = slot.pump().expect_err("the invalid edit reports its own plan error");
    assert!(diags.iter().any(|d| d.message.contains("audio control is outside its range")));
    node.run(&ClockChunk::default(), &mut left, &mut right);
    assert_eq!(slot.playing(), rewound);

    // A is playing. Rewind B, then edit back to A's graph before B posts.
    slot.set_doc(revision(&["newest", "only_in_b"], "")).unwrap();
    let fresh = slot.prepare_fresh().unwrap().unwrap();
    slot.set_doc(revision(&["newest"], " edited")).unwrap();
    slot.post_fresh(fresh);
    run_next(); // B's ordinary job
    let per_frame = 2.0 / 48_000.0;
    let mut clock = ClockChunk { playing: true, bpm: 120.0, per_frame,
        tempo: makepad_score_player::Tempo::linear(0.0, per_frame), ..Default::default() };
    let feed = makepad_audio_engine::feed::loop_feed(&format!("{SLOT}/newest"));
    assert!(feed.send(makepad_score_player::Event { beat: 0.0, loop_key: crate::key("newest"),
        kind: makepad_score_player::EventKind::Note, note: 60.0, vel: 1.0, pan: 0.0, dur: 2.0, opts: Default::default() }));
    node.run(&clock, &mut left, &mut right);
    let playing_b = slot.playing();
    assert!(playing_b > rewound && slot.read(&format!("{SLOT}/only_in_b"), "peak").is_some());
    assert!(left.iter().any(|v| v.abs() > 1e-6), "the rewind's player started the note");
    slot.pump().unwrap();
    run_next();
    clock.start = per_frame * CHUNK as f64;
    clock.tempo = makepad_score_player::Tempo::linear(clock.start, per_frame);
    node.run(&clock, &mut left, &mut right);
    assert!(slot.playing() > playing_b && slot.read(&format!("{SLOT}/only_in_b"), "peak").is_none(),
        "the edit back to A is compared with the B that actually posted");
    assert!(left.iter().any(|v| v.abs() > 1e-6), "the edit carries the sounding note from the rewound player");
    assert!(jobs.lock().unwrap().is_empty());

    // A resource arriving during a rewind needs a second prepare even for the same graph.
    let fresh = slot.prepare_fresh().unwrap().unwrap();
    slot.refresh().unwrap();
    slot.post_fresh(fresh);
    node.run(&clock, &mut left, &mut right);
    let before_refresh = slot.playing();
    slot.pump().unwrap();
    run_next();
    node.run(&clock, &mut left, &mut right);
    assert!(slot.playing() > before_refresh);

    let prior_rate = slot.playing();
    let old_rate_rewind = slot.prepare_fresh().unwrap().unwrap();
    let mut changed_rate = makepad_audio_engine::nested::factory(crate::node::TYPE).unwrap()(&score, 44_100, 2, None).unwrap();
    slot.post_fresh(old_rate_rewind);
    node.run(&ClockChunk::default(), &mut left, &mut right);
    assert_eq!(slot.playing(), prior_rate, "a rewind at the old device rate never posts");
    run_next();
    changed_rate.run(&ClockChunk::default(), &mut left, &mut right);
    assert!(slot.playing() > prior_rate);
    assert_eq!(slot.playing_layout().unwrap().rate, 44_100);
    let fresh = slot.prepare_fresh().unwrap().expect("rewind at the current rate");
    let before_rewind = slot.playing();
    slot.post_fresh(fresh);
    changed_rate.run(&ClockChunk::default(), &mut left, &mut right);
    assert!(slot.playing() > before_rewind, "the latest rewind posts normally");
    assert!(jobs.lock().unwrap().is_empty());

    // Only the new device rate doubles the recording to two frames. A document
    // load or the first device prepare therefore cannot enter the long loop.
    let source = r#"use mod.score.*
let clock = Clock{}
let fx = ShaderFx{code: "let src = sample(1) let t = [0.0;2] fn init(){ if len(src) == 2 { for i in 0..1000000000 { for j in 0..1024 { t[0] += float(j) } } } } fn effect(l,r){ vec2(t[0]) }" src: asset("slot_init_rate", "1")}
Score{clock fx}"#;
    let doc = model::ScoreDoc::test_load(source).unwrap();
    let asset = crate::player::sample_name(&doc.samples()[0]);
    use makepad_audio_engine::feed::samples;
    samples::register(&asset, Arc::new(makepad_score_player::Sample::new(vec![0.5].into(), None, 44_100.0)));
    slot.set_doc(doc).unwrap();
    run_next();
    changed_rate.run(&ClockChunk::default(), &mut left, &mut right);
    slot.pump().unwrap();
    assert!(!slot.initialising(), "completed initialization released the counter");
    let before = slot.playing();
    let mut doubled = makepad_audio_engine::nested::factory(crate::node::TYPE).unwrap()(&score, 88_200, 3, None).unwrap();
    let job = jobs.lock().unwrap().pop().unwrap();
    assert!(!slot.initialising(), "queueing a rate change is not initialization");
    let (done, finished) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || { job(); done.send(()).unwrap(); });
    let timeout = std::time::Duration::from_secs(5);
    let deadline = std::time::Instant::now() + timeout;
    while !slot.initialising() && !worker.is_finished() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let executing = slot.initialising();
    slot.set_doc(newest).unwrap();
    finished.recv_timeout(timeout).expect("the replacement cancelled the device-rate init");
    worker.join().unwrap();
    samples::forget(&asset);
    assert!(executing, "the slot observed initialization at the new device rate");
    assert!(!slot.initialising(), "the cancelled init released the counter");
    doubled.run(&ClockChunk::default(), &mut left, &mut right);
    assert_eq!(slot.playing(), before, "the superseded prepare never posted");
    run_next();
    doubled.run(&ClockChunk::default(), &mut left, &mut right);
    assert!(slot.playing() > before && slot.playing_layout().unwrap().rate == 88_200,
        "the new prepare has its own cancellation word");
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
    let loaded = model::test_load(TECHNO).unwrap();
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
    let mut own = prepare_score(&doc, rate, "own_score", ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
    let mut composer = Composer::new(loaded.into_evaluated());
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
    slot.set_source(&TECHNO.replace("amp: 0.4", "amp: 0.3"), &crate::PrepareControl::default()).unwrap();
    pass(&mut composer, &mut beat, &mut own, &mut host, &mut a, &mut b);
    slot.pump().unwrap();
    assert_eq!(slot.playing(), 2);
    let tail = b.len() - CHUNK;
    assert!(b[tail..].iter().any(|x| x.abs() > 1e-4), "the swap keeps the music playing");
    makepad_audio_engine::feed::forget_instance("own_score");
}

#[test]
fn nested_latency_nulls_on_first_play_and_after_each_child_swap() {
    use crate::player::{live_node, slice, ClockMode};
    use makepad_audio_engine::{nested, nodes, schedule::ClockChunk};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    const CHILD: &str = "latency_child";
    const PARENT: &str = "latency_parent";
    const SIGNAL: &str = "
        var k = int(0)
        fn effect(l, r) {
            k = (k + 1) % 31
            if k == 1 { vec2(0.25, -0.5) } else { vec2(0.0625, -0.125) }
        }";
    let source = format!("use mod.score.*\nlet clock = Clock{{}}\nlet dry = ShaderFx{{code: {SIGNAL:?}}}\n");
    let child_score = |delay| {
        let invert = format!("
            latency({delay})
            var dl = [0.0; 64]
            var dr = [0.0; 64]
            var w = int(0)
            fn effect(l, r) {{
                dl[w] = l
                dr[w] = r
                let v = vec2(dl[w - {delay}], dr[w - {delay}])
                w += 1
                -v
            }}");
        format!("{source}let wet = ShaderFx{{in: [dry] code: {invert:?}}}\nScore{{clock dry wet master: 1 ceiling: -0.8}}")
    };
    let mut registry = makepad_flow::document::Registry::with_builtins();
    nodes::register_nodes(&mut registry).unwrap();
    crate::node::register(&mut registry).unwrap();
    let score_node = |slot: &str| live_node(&crate::node::descriptor(), slot.into(), &[("score", Literal::Str(slot.into()))], &[]);
    let factory = nested::factory(crate::node::TYPE).unwrap();
    let child = crate::node::slot(CHILD);
    let nested = score_node(CHILD);
    let child_latency = factory(&nested, 48_000, 1, None).unwrap().latency();
    child.set_source(&child_score(37), &crate::PrepareControl::default()).expect("prepare the real child before the parent");

    // The parent exports the mixer directly, so we observe the sum on the
    // first pass without a second master limiter between it and the test.
    let doc = model::ScoreDoc::test_load(&format!("{source}Score{{clock dry}}")).unwrap();
    let mut graph = slice(&doc, PARENT, ClockMode::Host);
    let master = graph.nodes.pop().unwrap();
    assert_eq!(master.id, format!("{PARENT}/master"), "the mixer must replace the exported master");
    graph.nodes.push(nested);
    graph.nodes.push(live_node(nodes::descriptor("AudioMixer").unwrap(), master.id,
        &[("master", Literal::Num(1.0))],
        &[("inputs", vec![(format!("{PARENT}/dry"), "audio"), (CHILD.into(), "audio")])]));
    let parent = crate::node::slot(PARENT);
    let prepares = Arc::new(AtomicU32::new(0));
    let count = prepares.clone();
    parent.set_runner(Arc::new(move |job| {
        count.fetch_add(1, Ordering::Relaxed);
        job();
        Ok(())
    }));
    let mut output = factory(&score_node(PARENT), 48_000, 1, None).unwrap();
    parent.set_slice(doc, graph).expect("prepare the parent mixer");

    for (generation, delay, total) in [(1, 37, 165), (2, 0, 128), (3, 37, 165)] {
        if generation > 1 {
            child.set_source(&child_score(delay), &crate::PrepareControl::default()).expect("publish the child swap");
        }
        assert_eq!(child_latency.load(Ordering::Acquire), total, "published child total for shader latency {delay}");
        parent.pump().expect("the host must realign the playing parent on its first pump");
        assert_eq!(prepares.load(Ordering::Relaxed), generation, "parent prepare count after the first pump, generation {generation}");
        assert_eq!(output.latency().load(Ordering::Acquire), total, "parent must use the published child total on prepare {generation}");
        if generation > 1 {
            // The old and new pulses are six frames apart modulo 31. Their
            // jumps cannot coincide. The largest step is high minus low per
            // channel; these levels and the 1/256 fade weights are exact in f32.
            let until_null = total as usize + makepad_score_player::shader::FADE as usize;
            let mut previous = (0.0, 0.0); // Both channels nulled on the preceding pass.
            for at in (0..until_null).step_by(CHUNK) {
                let frames = CHUNK.min(until_null - at);
                let (mut left, mut right) = ([f32::NAN; CHUNK], [f32::NAN; CHUNK]);
                output.run(&ClockChunk::default(), &mut left[..frames], &mut right[..frames]);
                for i in 0..frames {
                    let step = ((left[i] - previous.0).abs(), (right[i] - previous.1).abs());
                    assert!(step.0 <= 0.25 - 0.0625 && step.1 <= 0.5 - 0.125,
                        "stereo pulse step {step:?} at swap frame {}, generation {generation}", at + i);
                    previous = (left[i], right[i]);
                }
            }
        }
        let first_start = if generation == 1 { total as usize } else { 0 };
        for (frames, start) in [(CHUNK, first_start), (97, 0)] {
            let (mut left, mut right) = ([f32::NAN; CHUNK], [f32::NAN; CHUNK]);
            output.run(&ClockChunk::default(), &mut left[..frames], &mut right[..frames]);
            for frame in start..frames {
                assert_eq!((left[frame], right[frame]), (0.0, 0.0), "stereo null at frame {frame} of the {frames}-frame pass, generation {generation}");
            }
        }
        let wet = child.scope(&format!("{CHILD}/master")).expect("read the child's post-limiter stereo signal");
        assert!(wet.iter().any(|bin| bin[0] == -0.25 && bin[3] == 0.5), "the limiter must pass the inverted stereo peaks at unity, generation {generation}");
        parent.pump().expect("pump an unchanged playing graph");
        assert_eq!(prepares.load(Ordering::Relaxed), generation, "an unchanged pump must leave the prepare count alone, generation {generation}");
    }
}

#[test]
fn export_and_live_midi_follow_the_playing_scores_latency() {
    use makepad_audio_engine::{feed::{loop_feed, midi_out}, nested, schedule::ClockChunk};

    const RATE: u32 = 8192;
    const FRAMES: usize = 513;
    const PORT: &str = "latency_out";
    let parts = r#"use mod.score.*
let clock = Clock{bpm: 120}
let tone = Shader{code: "fn voice() { vec2(gate * 0.125, gate * -0.25) }"}
let line = Loop{play: tone beats: 1 notes: [[0 60 0.0625 1]]}
let gear = Instrument{kind: @midi port: "latency_out" channel: 1}
let notes = Loop{play: gear beats: 1 notes: [[0 60 0.5 1] [0.03125 62 0.5 1]]}"#;
    let dry = format!("{parts}\nScore{{clock tone line gear notes master: 1}}");
    let copy = "latency(37)
        var dl = [0.0; 64]
        var dr = [0.0; 64]
        var w = int(0)
        fn effect(l, r) {
            dl[w] = l
            dr[w] = r
            let sample = vec2(dl[w - 37], dr[w - 37])
            w += 1
            sample
        }";
    let delayed = format!("{parts}\nlet copy = ShaderFx{{in: [line notes] code: {copy:?}}}\nScore{{clock tone line gear notes copy master: 1}}");
    let opts = RenderOpts { rate: RATE as f32, seconds: Some(FRAMES as f64 / RATE as f64), ..Default::default() };
    let a = render(&dry, &opts, &NoSamples, &crate::PrepareControl::default()).expect("render the direct path");
    let b = render(&delayed, &opts, &NoSamples, &crate::PrepareControl::default()).expect("render the delayed copy");
    assert_eq!((a.latency, b.latency), (128, 165), "each export reports the frames it trimmed");
    assert_eq!([a.left.len(), a.right.len(), b.left.len(), b.right.len()], [FRAMES; 4], "both exports keep the requested stereo length");
    assert!(a.left[0] > 0.0 && a.right[0] < 0.0 && b.left[0] > 0.0 && b.right[0] < 0.0, "the beat-zero note must sound at the first exported frame");

    let mut messages = Vec::new();
    let port = midi_out::port_hash(PORT);
    midi_out::take_due(f64::INFINITY, &mut messages);
    assert!(messages.iter().all(|m| m.port != port), "offline renders must send no MIDI");
    messages.clear();

    let loaded = model::test_load(&delayed).expect("load the same score for the host");
    let slot = crate::node::slot("latency_export");
    let mut registry = makepad_flow::document::Registry::with_builtins();
    makepad_audio_engine::nodes::register_nodes(&mut registry).unwrap();
    crate::node::register(&mut registry).unwrap();
    let score = crate::player::live_node(&crate::node::descriptor(), "score".into(), &[("score", Literal::Str(slot.name().into()))], &[]);
    midi_out::set_live(slot.name(), true);
    slot.set_doc(loaded.doc.clone()).expect("publish the score for its live slot");
    let mut live = nested::factory(crate::node::TYPE).unwrap()(&score, RATE, 1, None).expect("prepare the slot's player");
    midi_out::set_live(slot.name(), false);
    assert!(slot.playing_layout().is_none(), "a prepared player has no playing layout until its first pass");
    let mut events = Vec::new();
    Composer::new(loaded.into_evaluated()).pump(0.0625, &mut events);
    let feed = loop_feed("latency_export/notes");
    for event in events.into_iter().filter(|e| e.loop_key == key("notes")) {
        assert!(feed.send(event), "the live MIDI loop must receive its score events");
    }
    let per_frame = 2.0 / RATE as f64;
    let clock = ClockChunk { bpm: 120.0, playing: true, per_frame, tempo: makepad_score_player::Tempo::linear(0.0, per_frame), ..Default::default() };
    let (mut left, mut right) = ([0.0; CHUNK], [0.0; CHUNK]);
    let before = midi_out::now();
    live.run(&clock, &mut left, &mut right);
    let after = midi_out::now();
    let layout = slot.playing_layout().expect("the host can read the playing generation");
    assert_eq!(layout.latency, b.latency, "the watch and the export must report the same total");
    for (id, own, input) in [("latency_export/copy", 37, 0), ("latency_export/master", 128, 37)] {
        let node = layout.nodes.iter().find(|n| n.id == id).expect("the watched layout includes the output path");
        assert_eq!((node.own_latency, node.l_in), (own, input), "latency and aligned input frame for {id}");
    }
    midi_out::take_due(f64::INFINITY, &mut messages);
    messages.retain(|m| m.port == port);
    assert_eq!(messages.iter().map(|m| (m.data[0], m.data[1])).collect::<Vec<_>>(), [(0x90, 60), (0x90, 62)], "both live note-ons must reach the port in order");
    for (event, frame) in messages.iter().zip([0, 128]) {
        let offset = (b.latency + frame) as f64 / RATE as f64;
        assert!((before + offset..=after + offset).contains(&event.at), "MIDI frame {frame} must be due at its frame plus the score latency");
    }
    for i in 0..FRAMES {
        assert_eq!((a.left[i].to_bits(), a.right[i].to_bits()), (b.left[i].to_bits(), b.right[i].to_bits()),
            "exported stereo differs at frame {i}: direct {:?}, delayed {:?}", (a.left[i], a.right[i]), (b.left[i], b.right[i]));
    }
}

/// A player that plays live (it takes MIDI in), under a name of its own.
fn live_instance(doc: &model::ScoreDoc) -> crate::player::Player {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let name = format!("live_test_{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    makepad_audio_engine::feed::midi_out::set_live(&name, true);
    prepare_score(doc, 48_000.0, &name, crate::player::ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap()
}

/// The MIDI input ports the tests play on (one list for every test: the
/// port list is the process's).
fn test_ports() {
    makepad_audio_engine::feed::midi_in::set_ports(vec![(7, "Test Keys MIDI 1".into()), (8, "Other".into()), (9, "Pedal Keys".into())]);
}

/// Where notes start in `out` (frames after at least `gap` frames under
/// `floor`), and each one's pitch from its zero crossings (Hz).
fn note_onsets(out: &[f32], floor: f32, gap: usize) -> Vec<(usize, f32)> {
    let mut found = Vec::new();
    let mut quiet = gap;
    let mut i = 0;
    while i < out.len() {
        if out[i].abs() > floor && quiet >= gap {
            let end = (i + 2_900).min(out.len());
            let w = &out[i + 240..end];
            let crossings = w.windows(2).filter(|p| (p[0] < 0.0) != (p[1] < 0.0)).count() as f32;
            found.push((i, crossings / 2.0 / (w.len() as f32 / 48_000.0)));
            quiet = 0;
            i += 240;
            continue;
        }
        quiet = if out[i].abs() > floor { 0 } else { quiet + 1 };
        i += 1;
    }
    found
}

#[test]
fn midi_keys_through_an_arp_play_the_pattern_on_the_clock() {
    use makepad_audio_engine::feed::midi_in::{self, MidiMsg};
    // 120 bpm: a sixteenth is an eighth of a second, 6000 frames.
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet tone = Synth{osc: @sine attack: 0.001 decay: 0.1 sustain: 1 release: 0.002 cutoff: 16000}\nlet keys_in = MidiIn{device: \"Test Keys\" channel: 1 latency: 0}\nlet up = Transpose{in: keys_in semis: 12}\nlet arp = Arp{in: up pattern: @updown rate: 1/16 octaves: 1 gate: 0.5}\nlet lead = Loop{play: tone in: arp}\nScore{clock tone keys_in up arp lead}\n";
    let loaded = model::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    for (setting, master) in [("", Some(0)), ("master: 15", Some(15)), ("master: @none", None)] {
        let doc = model::ScoreDoc::test_load(&src.replace("MidiIn{", &format!("MidiIn{{mpe: true {setting} "))).unwrap();
        assert_eq!(doc.resolve_stream(&doc.loops[0]).unwrap().unwrap().def.master, master, "{setting}");
    }
    let stream = loaded.doc.resolve_stream(&loaded.doc.loops[0]).unwrap().unwrap();
    assert_eq!(stream.path, ["arp", "up", "keys_in"]);
    let mut instance = live_instance(&loaded.doc);
    test_ports();
    // A chord (c3 e3 g3, an octave up through Transpose) on the next pass's
    // first frame; another device's and another channel's keys are not taken.
    for data in [[0x90, 48, 100], [0x90, 52, 100], [0x90, 55, 100], [0x91, 50, 100]] {
        midi_in::push(MidiMsg { at: f64::NEG_INFINITY, port: 7, data });
    }
    midi_in::push(MidiMsg { at: f64::NEG_INFINITY, port: 8, data: [0x90, 70, 100] });
    let mut out = Vec::new();
    for pass in 0..(6 * 6000 / CHUNK + 1) {
        if pass == 5 * 6000 / CHUNK {
            // Let go before the sixth step: up-down c e g e c, then silence.
            for data in [[0x80, 48, 0], [0x80, 52, 0], [0x80, 55, 0]] {
                midi_in::push(MidiMsg { at: f64::NEG_INFINITY, port: 7, data });
            }
        }
        run(&mut instance.schedule, CHUNK, &mut |_, a, _| out.extend_from_slice(&a[..CHUNK]), &mut NoExt);
    }
    let notes = note_onsets(&out, 1e-6, 1_000);
    assert_eq!(notes.len(), 5, "{notes:?}");
    let first = notes[0].0;
    let latency = instance.schedule.latency as usize;
    // The first note is under the output's startup fade beginning at the score's latency.
    assert!((latency..latency + CHUNK).contains(&first), "first crossing at {first}, latency {latency}");
    for (k, (at, hz)) in notes.iter().enumerate() {
        if k > 0 {
            let grid_frame = latency + k * 6000;
            assert_eq!(*at, grid_frame + 1, "step {k}: the sine starts at zero on frame {grid_frame}");
        }
        let want = [261.63, 329.63, 392.0, 329.63, 261.63][k];
        assert!((hz - want).abs() < want * 0.03, "step {k}: {hz} Hz, want {want}");
    }
    // The gate: each note sounds half its step.
    let (on, off) = (peak(&out[first + 1_000..first + 2_900]), peak(&out[first + 4_500..first + 5_900]));
    assert!(on > 0.05 && off < 1e-3, "on {on} off {off} first {first}");
    makepad_audio_engine::feed::forget_instance(&instance.name);
}

#[test]
fn a_keyboard_plays_its_loop_while_the_transport_is_stopped_and_the_pedal_holds() {
    use makepad_audio_engine::feed::midi_in::{self, MidiMsg};
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet tone = Synth{osc: @sine attack: 0.001 decay: 0.1 sustain: 1 release: 0.002 cutoff: 16000}\nlet keys_in = MidiIn{device: \"Pedal Keys\" latency: 0}\nlet keys = Loop{play: tone in: keys_in}\nScore{clock tone keys_in keys}\n";
    let loaded = model::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let mut instance = live_instance(&loaded.doc);
    // A render (not live) takes no keys.
    let mut render = instantiate(&loaded.doc, 48_000.0, &crate::PrepareControl::default()).unwrap();
    // The transport stopped: the key still sounds.
    let playing = instance.layout.controls.get(&format!("{}.playing", crate::player::transport(&instance.name))).map(|(index, _)| *index).expect("the transport's playing control");
    instance.schedule.controls.set_manual(playing, 0.0);
    test_ports();
    let pass = |instance: &mut crate::player::Player, msgs: &[[u8; 3]]| {
        for data in msgs {
            midi_in::push(MidiMsg { at: f64::NEG_INFINITY, port: 9, data: *data });
        }
        let mut out = Vec::new();
        for _ in 0..40 {
            run(&mut instance.schedule, CHUNK, &mut |_, a, _| out.extend_from_slice(&a[..CHUNK]), &mut NoExt);
        }
        out
    };
    let held = pass(&mut instance, &[[0xb0, 64, 127], [0x90, 69, 100], [0x80, 69, 0]]);
    let mut quiet = Vec::new();
    for _ in 0..40 {
        run(&mut render.schedule, CHUNK, &mut |_, a, _| quiet.extend_from_slice(&a[..CHUNK]), &mut NoExt);
    }
    assert!(peak(&quiet) < 1e-6, "a render plays no keyboard");
    assert!(peak(&held[5_000..]) > 0.05, "the pedal holds the key let go");
    let up = pass(&mut instance, &[[0xb0, 64, 0]]);
    assert!(peak(&up[2_000..]) < 1e-3, "the pedal up lets it go");
    makepad_audio_engine::feed::forget_instance(&instance.name);
}

#[test]
fn a_held_document_evaluation_takes_precedence_in_the_desk_overlay() {
    use model::{LoadActivity::*, LoadProgress, LoadResult, RevisionLoads};
    use makepad_script::makepad_error_log::{set_log_tap, LogLevel};
    use std::sync::{mpsc::{channel, Sender}, Mutex};
    use std::time::Duration;

    const MARK: &str = "desk_overlay_document_entered";
    static ENTERED: Mutex<Option<Sender<Sender<()>>>> = Mutex::new(None);
    fn tap(message: &str, _: LogLevel) {
        if message.contains(MARK) {
            let notify = ENTERED.lock().unwrap().take();
            if let Some(notify) = notify {
                let (release, held) = channel();
                if notify.send(release).is_ok() { let _ = held.recv(); }
            }
        }
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) { set_log_tap(None); ENTERED.lock().unwrap().take(); }
    }
    let _lock = crate::live::TEST_LOG_TAP.lock().unwrap_or_else(|e| e.into_inner());
    let (notify, entered) = channel();
    *ENTERED.lock().unwrap() = Some(notify);
    set_log_tap(Some(tap));
    let _reset = Reset;

    let mut loads = RevisionLoads::default();
    loads.submit(1, &format!("use mod.score.*\nmod.std.log({MARK:?})\nlet c = Clock{{}}\nScore{{c}}"), drop);
    let mut progress = LoadProgress::default();
    let mut sample = |load: Option<&model::RevisionLoad>, slot, now| progress.sample(load, slot, now, 0.2);
    assert_eq!(sample(None, true, 0.0), None, "a short device-rate init stays hidden");
    assert_eq!(sample(None, true, 0.2), Some(Initialising), "a sustained init is visible at the delay");
    loads.start(|request| {
        assert!(!request.evaluating(), "queuing the revision does not mark its VM as running");
        std::thread::scope(|scope| {
            let entered = entered; // Disconnect the tap on unwind before the scoped worker is joined.
            let worker = scope.spawn(|| request.run());
            let release = entered.recv_timeout(Duration::from_secs(5)).expect("the revision is inside its document VM");
            assert!(request.evaluating(), "the held evaluation is visible through its revision");
            assert_eq!(sample(Some(&request), true, 0.5), Some(Initialising));
            assert_eq!(sample(Some(&request), true, 0.69), Some(Initialising), "a short evaluation cannot flash over an init");
            assert_eq!(sample(Some(&request), true, 0.71), Some(Evaluating), "the sustained evaluation takes precedence");
            drop(release);
            let result = worker.join().unwrap();
            assert!(matches!(result, LoadResult::Ready(..)), "the held revision finishes normally");
            assert!(!request.evaluating(), "the VM unregisters before the revision returns");
            assert_eq!(sample(Some(&request), true, 0.72), Some(Initialising), "the ongoing init is immediately visible again");
            assert_eq!(sample(None, false, 0.73), None, "the ordinary desk status returns");
        });
        Some(())
    });
}

#[test]
fn a_new_revision_cancels_a_load_already_reading_a_shader() {
    use std::cell::Cell;
    use crate::model::{LoadResult, RevisionLoads};
    use std::sync::mpsc::{sync_channel, Receiver};
    use std::time::Duration;

    let timeout = Duration::from_secs(5);
    let mut loads = RevisionLoads::<Receiver<LoadResult>>::default();
    let good = plays("Shader{code: \"fn voice() { gate * velocity }\"}", A_NOTE);
    let bad = good.replace("gate * velocity", "unknown_name");
    let cancelled = crate::PrepareControl::default();
    cancelled.cancel();
    assert!(matches!(model::load_controlled(&bad, &cancelled), Err(crate::LoadError::Cancelled)),
        "cancellation takes precedence even over a broken shader");
    assert!(matches!(model::read_document_controlled(&bad, &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(model::evaluated_label_controlled(&bad, &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(model::evaluated_palette_controlled(&bad, &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(crate::render::cycle_notes_controlled(&bad, &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(crate::render::loop_error_controlled(&bad, "line", &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(crate::render::phrase_notes_controlled(&bad, "line", 1.0, &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(crate::render::timing_controlled(&bad, None, &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(crate::vary::part_of_controlled(&bad, "line", &cancelled), Err(crate::LoadError::Cancelled)));
    let during = crate::PrepareControl::default();
    let worker = during.clone();
    model::ON_SHADER_COMPILE.with(|hook| hook.replace(Some(Box::new(move || worker.cancel()))));
    assert!(matches!(model::load_controlled(&good, &during), Err(crate::LoadError::Cancelled)),
        "a shader cancelled inside the compile produces no document or problems");
    assert!(loads.submit(1, &bad, |_| unreachable!()));
    let (started, inside) = sync_channel(1);
    let (resume, paused) = sync_channel(1);
    let (finished, result) = sync_channel(1);
    std::thread::scope(|scope| {
        loads.start(|request| {
            let (tx, rx) = sync_channel(1);
            scope.spawn(move || {
                model::ON_SHADER_COMPILE.with(|hook| hook.replace(Some(Box::new(move || {
                    started.send(()).unwrap();
                    paused.recv_timeout(timeout).expect("newer revision releases the worker");
                }))));
                let answer = request.run();
                finished.send(matches!(answer, LoadResult::Cancelled)).unwrap();
                let _ = tx.try_send(answer);
            });
            Some(rx)
        });
        inside.recv_timeout(timeout).expect("old load entered its shader compile pass");
        let mut cancelled_job = false;
        assert!(loads.submit(2, &good, |_| cancelled_job = true));
        assert!(cancelled_job, "superseding cancels the frame job too");
        resume.send(()).unwrap();
        assert!(result.recv_timeout(timeout).expect("old load returned"), "an obsolete error is silent");
    });
    assert!(loads.applied.is_none() && loads.diags.is_empty());

    // Both programs have an observable result and would run for trillions of
    // iterations. Wait for the compile-time execution itself before replacing it.
    for (entry, body) in [
        ("constant", "let c=slow() fn voice(){c}"),
        ("init", "let t=[0.0;2] fn init(){t[0]=slow()} fn voice(){t[0]}"),
    ] {
        let shader = format!("fn slow() {{ var x=0.0 for i in 0..1000000000 {{ for j in 0..1024 {{ x += float(j) }} }} x }} {body}");
        let source = good.replace("fn voice() { gate * velocity }", &shader);
        let mut revisions = RevisionLoads::<()>::default();
        revisions.submit(1, &source, drop);
        let mut started = None;
        revisions.start(|request| {
            let observer = request.clone();
            let (tx, rx) = sync_channel(1);
            let job = std::thread::spawn(move || tx.send(request.run()).unwrap());
            started = Some((observer, rx, job));
            Some(())
        });
        let (observer, rx, job) = started.unwrap();
        let deadline = std::time::Instant::now() + timeout;
        while !observer.executing() && !job.is_finished() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        let running = observer.executing();
        revisions.submit(2, &good, drop);
        let answer = rx.recv_timeout(timeout).expect("supersession stopped the obsolete shader");
        job.join().unwrap();
        assert!(running, "{entry}: the over-cap shader entered execution");
        assert!(matches!(answer, LoadResult::Cancelled), "{entry}: replacement discarded the whole load");
        assert!(!observer.executing(), "{entry}: the retired revision released its guard");
    }

    loads.start(|request| {
        let (tx, rx) = sync_channel(1);
        tx.try_send(request.run()).unwrap();
        Some(rx)
    });
    // The good result is buffered but has yet to be polled. A replacement
    // must discard it just as it would an in-flight failure.
    loads.submit(3, &good, drop);
    assert!(loads.take(|rx| rx.try_recv().ok()).is_none());
    loads.start(|request| {
        let (tx, rx) = sync_channel(1);
        tx.try_send(request.run()).unwrap();
        Some(rx)
    });
    let doc = loads.take(|rx| rx.try_recv().ok()).expect("newest valid revision applies");
    assert_eq!(loads.applied.as_ref().unwrap().revision, 3);
    assert!(loads.diags.is_empty());
    assert!(matches!(instantiate(&doc, 8000.0, &cancelled), Err(crate::LoadError::Cancelled)),
        "a cancelled cache miss publishes no prepared player or layout");
    let fresh = crate::PrepareControl::default();
    let ready = instantiate(&doc, 8000.0, &fresh).expect("a fresh prepare of the same shader succeeds");
    makepad_audio_engine::feed::forget_instance(&ready.name);
    let mut passes = 0;
    let opts = RenderOpts { rate: 8000.0, seconds: Some(0.1), ..Default::default() };
    let result = crate::render::render_with(&good, &opts, &NoSamples, &fresh, &mut |_, _| {
        passes += 1;
        fresh.cancel();
    });
    assert_eq!(passes, 1, "the next pass never executes");
    assert!(matches!(result, Err(crate::LoadError::Cancelled)), "no partial render escapes");
    let set = loads.shaders.as_ref().unwrap();
    assert!(matches!(set.evaluate_controlled(&bad, &cancelled), Err(crate::LoadError::Cancelled)));
    assert!(matches!(set.evaluate_rewrite(&bad, &cancelled), Err(crate::LoadError::Cancelled)),
        "cancellation does not request a fallback compile");
    let evaluated = set.evaluate_controlled(&good, &crate::PrepareControl::default()).unwrap();
    assert!(doc.shader("inst").unwrap().shader.ptr_eq(&evaluated.doc.shader("inst").unwrap().shader));
    let count = model::SHADER_COMPILES.with(|count| count.get());
    let Some(crate::LoadError::Problems(diags)) = set.evaluate_controlled(&bad, &crate::PrepareControl::default()).err() else {
        panic!("a changed definition cannot use the compiled set");
    };
    assert!(diags.iter().any(|d| d.message.contains("internal error")));
    assert_eq!(model::SHADER_COMPILES.with(|count| count.get()), count, "a mismatch never falls back to compilation");

    async fn rewrite_then_load(
        set: &model::CompiledShaders, source: &str, word: &crate::PrepareControl, loading: &Cell<bool>,
    ) -> Result<(crate::Loaded, crate::Loaded), crate::LoadError> {
        let rewrite = set.evaluate_rewrite_with_slice(source, word, Duration::ZERO).await?
            .expect("the rewrite preserves the shader definition");
        loading.set(true);
        let loaded = model::load_with_slice(&rewrite.doc.source, word, Duration::ZERO).await?;
        Ok((rewrite, loaded))
    }

    let rewritten = good.replace("let clock = Clock{bpm: 120}",
        "let n = 0\nfor i in 0..512 { n += 1 }\nlet clock = Clock{bpm: n / 4}");
    let word = crate::PrepareControl::default();
    let whole_rewrite = set.evaluate_rewrite(&rewritten, &word).unwrap().unwrap();
    let whole_load = crate::load_controlled(&whole_rewrite.doc.source, &word).unwrap();
    let loading = Cell::new(false);
    let mut reads = std::pin::pin!(rewrite_then_load(set, &rewritten, &word, &loading));
    assert!(poll_once(reads.as_mut()).is_pending() && !loading.get(), "read one yields before the load starts");
    let mut load_yielded = false;
    let (rewrite, loaded) = loop {
        if let std::task::Poll::Ready(done) = poll_once(reads.as_mut()) { break done.unwrap(); }
        load_yielded |= loading.get();
    };
    assert!(load_yielded, "the dependent load also yields");
    assert_eq!(rewrite.doc, whole_rewrite.doc, "the sliced rewrite matches the blocking rewrite");
    assert_eq!(rewrite.vm.root, whole_rewrite.vm.root, "the rewrite preserves the evaluated score");
    assert!(rewrite.doc.shader("inst").unwrap().shader.ptr_eq(&doc.shader("inst").unwrap().shader),
        "the sliced rewrite keeps the original compiled program");
    assert_eq!(loaded.doc, whole_load.doc, "the dependent load matches the blocking load");
    assert_eq!(loaded.vm.objects, whole_load.vm.objects, "the load preserves the evaluated nodes and their locations");
    assert_eq!(loaded.doc.warnings(), whole_load.doc.warnings(), "both loads return the same diagnostics");

    for cancel in [true, false] {
        let word = crate::PrepareControl::default();
        let loading = Cell::new(false);
        let mut reads = Box::pin(rewrite_then_load(set, "loop {}", &word, &loading));
        assert!(poll_once(reads.as_mut()).is_pending() && word.script_running(), "read one is suspended and registered");
        if cancel {
            word.cancel();
            assert!(matches!(poll_once(reads.as_mut()), std::task::Poll::Ready(Err(crate::LoadError::Cancelled))),
                "Stop in read one ends the chain without diagnostics");
            assert!(!word.script_running(), "the cancelled read unregisters before its future is dropped");
        }
        drop(reads);
        assert!(!word.script_running(), "dropping the suspended read releases the registration");
        assert!(!loading.get(), "read two never started");
    }
}

fn poll_once<F: std::future::Future>(future: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
}

#[test]
fn interleaved_evaluations_keep_their_vm_and_only_blocking_reads_hit_the_ui_guard() {
    use crate::vm::evaluate_with_slice;
    use makepad_core_util::ui_thread::{mark_ui_thread, violations};
    use std::{task::Poll, time::Duration};

    // The UI mark belongs to this thread for its lifetime.
    std::thread::spawn(|| {
        mark_ui_thread();
        let hits = || violations().into_iter().find(|(name, _)| *name == "a score evaluation").map_or(0, |(_, n)| n);
        let before = hits();
        let sources = ["let n = 0\nfor i in 0..512 { n += i }\nn", "let n = 1\nfor i in 0..512 { n += 2 }\nn"];
        let words = [crate::PrepareControl::default(), crate::PrepareControl::default()];
        let eval = |i: usize| evaluate_with_slice(sources[i], &words[i], Duration::ZERO);
        let whole = crate::drive(async {
            let first = crate::drive(eval(0)).unwrap();
            let second = eval(1).await.unwrap();
            [first, second]
        });
        assert_ne!(whole[0].root, whole[1].root, "the scripts have distinguishable results");
        assert_eq!(hits() - before, whole.len(), "nested blocking drivers count each evaluation once across all its polls");

        let after_drive = hits();
        let mut futures = [Box::pin(eval(0)), Box::pin(eval(1))];
        for future in &mut futures {
            assert!(poll_once(future.as_mut()).is_pending(), "each VM yields its first slice");
        }
        assert!(words.iter().all(|w| w.script_running()), "both VMs are suspended on this thread");
        let mut results = [None, None];
        while results.iter().any(Option::is_none) {
            for (future, result) in futures.iter_mut().zip(&mut results) {
                if result.is_none() {
                    if let Poll::Ready(done) = poll_once(future.as_mut()) { *result = Some(done.unwrap()); }
                }
            }
        }
        for (result, whole) in results.into_iter().zip(whole) {
            let sliced = result.unwrap();
            assert_eq!(sliced.root, whole.root, "each interleaved VM produces its own whole-run value");
            assert_eq!(sliced.errors, whole.errors, "interleaving preserves each VM's diagnostics");
        }
        assert_eq!(hits(), after_drive, "polling the same futures outside drive records no UI evaluation");
        words[0].cancel();
        assert!(matches!(crate::drive(eval(0)), Err(crate::LoadError::Cancelled)), "a pre-cancelled read stays cancelled");
        assert_eq!(hits(), after_drive, "a pre-cancelled blocking read records no UI evaluation");

        let mut page_job = std::pin::pin!(async { crate::vm::evaluate_controlled(sources[1], &words[1]) });
        assert!(matches!(poll_once(page_job.as_mut()), Poll::Ready(Ok(_))), "the sync reader finishes within the page poll");
        assert_eq!(hits(), after_drive + 1, "a sync reader inside a directly polled job is visible by name");
    }).join().expect("UI evaluation checks complete");
}

/// A rewind is a first start: through a host root like the apps', with the
/// composer on the root's clock, what plays after it (running or stopped) is
/// what played from the first start, sample for sample, though the old run
/// left voices, echoes and a later beat behind.
#[test]
fn a_rewind_sounds_as_the_first_start() {
    use crate::live::{ClockSource, LiveConfig, LiveScore};
    use crate::player::live_node;
    use makepad_audio_engine::nodes::descriptor;
    use makepad_audio_engine::schedule::{compile, AudioBudget, ClockChunk, Schedule};
    use makepad_flow::live::plan::OwnerSlice;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Clock {
        chunk: Mutex<ClockChunk>,
        beat: Mutex<f64>,
    }
    impl ClockSource for Clock {
        fn now(&self) -> Option<(f64, f64, bool)> {
            let chunk = *self.chunk.lock().unwrap();
            Some((*self.beat.lock().unwrap(), if chunk.bpm > 0.0 { chunk.bpm as f64 } else { 120.0 }, chunk.playing))
        }
        fn located(&self) -> u64 {
            self.chunk.lock().unwrap().located as u64
        }
    }

    const SLOT: &str = "rewind_test";
    const TRANSPORT: &str = "rewind_test_transport";
    const SCORE: &str = "rewind_test_score";
    let source = crate::templates::template("techno").unwrap().replace("Synth{preset: @stab}", "Shader{code: \"fn voice() { gate * velocity * 0.1 }\"}");
    let before = model::SHADER_COMPILES.with(|count| count.get());
    let doc = model::test_load(&source).unwrap().doc;
    let shaders = model::CompiledShaders::from_doc(&doc);
    let document_compiles = model::SHADER_COMPILES.with(|count| count.get());
    assert_eq!(document_compiles, before + 1, "the document contains one compiled shader");
    let mut registry = makepad_flow::document::Registry::with_builtins();
    makepad_audio_engine::nodes::register_nodes(&mut registry).unwrap();
    crate::node::register(&mut registry).unwrap();
    let transport = live_node(descriptor("Transport").unwrap(), TRANSPORT.into(), &[("bpm", Literal::Num(transport_bpm(&doc))), ("playing", Literal::Num(0.0))], &[]);
    let score = live_node(&crate::node::descriptor(), SCORE.into(), &[("score", Literal::Str(SLOT.into()))], &[("clock", vec![(TRANSPORT.to_string(), "clock")])]);
    let out = live_node(descriptor("AudioOutput").unwrap(), "out".into(), &[("enabled", Literal::Num(1.0))], &[("audio", vec![(SCORE.to_string(), "audio")])]);
    let slice = OwnerSlice { owner: makepad_audio_engine::OWNER.into(), nodes: vec![transport, score, out] };
    let compiled = compile::<NoExt, ()>(&slice, 1, 1, (1u64 << 32) | 48_000, None, &Arc::new(AudioBudget::default())).unwrap();
    let clock_index = *compiled.layout.transports.get(TRANSPORT).unwrap() as usize;
    let playing = compiled.layout.controls.get(&format!("{TRANSPORT}.playing")).unwrap().0;
    let mut root: Box<Schedule<(), ()>> = compiled.schedule;
    let slot = crate::node::slot(SLOT);
    slot.set_doc(doc).unwrap();
    let clock = Arc::new(Clock::default());
    let live = LiveScore::new_pumped(LiveConfig { instance: SLOT.into(), lookahead: 0.15, clock: clock.clone() }, TRANSPORT);
    let source: Arc<str> = source.into();
    assert!(live.try_set_prepared(source.clone(), shaders.clone(), 1));
    assert!(!live.try_set_prepared(source, shaders, 2), "only one source waits for the composer");
    live.pump();
    let set_playing = |root: &mut Schedule<(), ()>, on: bool| {
        root.controls.set_manual(playing, if on { 1.0 } else { 0.0 });
        clock.chunk.lock().unwrap().playing = on;
    };
    let pass = |root: &mut Schedule<(), ()>, out: &mut Vec<f32>| {
        live.pump();
        slot.pump().unwrap();
        run(root, CHUNK, &mut |cue, l, _| {
            if !cue {
                out.extend_from_slice(&l[..CHUNK]);
            }
        }, &mut NoExt);
        let chunk = root.clocks[clock_index];
        *clock.beat.lock().unwrap() = chunk.tempo.end_beat(CHUNK);
        *clock.chunk.lock().unwrap() = chunk;
    };
    let passes = 3 * 48_000 / CHUNK;

    // The first start.
    set_playing(&mut root, true);
    let mut first = Vec::new();
    for _ in 0..passes {
        pass(&mut root, &mut first);
    }
    let end = *clock.beat.lock().unwrap();
    let loudest = first.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    assert!(loudest > 0.05, "the score is silent ({loudest})");
    // From the second pass: the root's output gate opens over its first.
    let same = |what: &str, again: &[f32], from: usize| {
        assert_eq!(first.len(), again.len());
        let apart = first.iter().zip(again).enumerate().skip(from).find(|(_, (x, y))| (*x - *y).abs() >= 1e-6);
        assert!(apart.is_none(), "{what}: at (frame, (first start, now)) {apart:?}");
        assert!((*clock.beat.lock().unwrap() - end).abs() < 1e-6, "{what}: the beat counts from 0");
    };

    // Rewound while it plays, well into the piece.
    let mut scratch = Vec::new();
    for _ in 0..passes {
        pass(&mut root, &mut scratch);
    }
    let located = clock.located();
    live.rewind(TRANSPORT);
    let mut again = Vec::new();
    for _ in 0..passes {
        pass(&mut root, &mut again);
    }
    assert_eq!(clock.located(), located + 1);
    same("rewound playing", &again, CHUNK);

    // Rewound stopped: it stays stopped at 0, and the next play is the first start.
    set_playing(&mut root, false);
    for _ in 0..8 {
        pass(&mut root, &mut scratch);
    }
    live.rewind(TRANSPORT);
    for _ in 0..8 {
        pass(&mut root, &mut scratch);
        assert!(!clock.chunk.lock().unwrap().playing);
    }
    assert_eq!(*clock.beat.lock().unwrap(), 0.0, "stopped at the top");
    live.pump();
    assert_eq!(live.status().beat, 0.0);
    set_playing(&mut root, true);
    let mut again = Vec::new();
    for _ in 0..passes {
        pass(&mut root, &mut again);
    }
    // Its gain stages opened while it stood, as before a first play in an
    // app; the first start above opened them as it began.
    same("rewound stopped", &again, 8 * CHUNK);
    // A discontinuity drives restart_at even when the transport is playing.
    *clock.beat.lock().unwrap() += 32.0;
    live.pump();
    assert_eq!(model::SHADER_COMPILES.with(|count| count.get()), document_compiles,
        "first start, both rewinds and clock restart reuse the document's shader");
}

/// A score playing `ctor` as `inst` (`code` its loop's).
fn plays(ctor: &str, code: &str) -> String {
    format!("use mod.score.*\nlet clock = Clock{{bpm: 120}}\nlet inst = {ctor}\nlet line = Loop{{play: inst beats: 4 code: fn(i) {{ {code} }}}}\nScore{{clock inst line}}\n")
}

const A_NOTE: &str = "play(@c4, {dur: 0.5}) sleep(1)";

#[test]
fn every_node_type_a_score_reads_is_defined() {
    let fx = ["Lowpass", "Highpass", "Bandpass", "Delay", "Reverb", "Drive", "Crush", "Chorus", "Comp", "Duck", "Bus", "Limiter", "Eq", "Width", "Isolator", "DjFilter"];
    for ty in crate::instruments::INSTRUMENT_TYPES.iter().chain(&model::STREAM_TYPES).chain(&fx) {
        assert!(crate::vm::NODE_TYPES.contains(ty), "`{ty}` is not defined in a score");
        assert!(!crate::assist::fields_of(ty).is_empty() || matches!(*ty, "Bus"), "`{ty}` lists no fields");
    }
}

#[test]
fn every_instrument_type_loads_and_plays() {
    use crate::instruments::INSTRUMENT_TYPES;
    use makepad_score_player::Sample;
    use std::sync::Arc;
    struct Tone;
    impl crate::build::Samples for Tone {
        fn sample(&self, _asset: &model::AssetRef) -> Option<Arc<Sample>> {
            let left: Vec<f32> = (0..24_000).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin()).collect();
            Some(Arc::new(Sample::new(left.into(), None, 48_000.0)))
        }
    }
    for ty in INSTRUMENT_TYPES {
        let ctor = match ty {
            "Synth" => "Synth{preset: @pluck}",
            "Drums" => "Drums{kit: @analog_hybrid}",
            "Sampler" => "Sampler{src: asset(\"tone\", \"1\") root: @a4}",
            "Ironfish" => "Ironfish{}",
            "Morph" => "Morph{}",
            "Tandem" => "Tandem{}",
            "Shader" => "Shader{lib: @supersaw}",
            "Voice" => "Voice{profile: @alto}",
            "Instrument" => "Instrument{kind: @piano}",
            other => panic!("`{other}`: an instrument type this test does not play yet"),
        };
        let code = match ty {
            "Voice" => "sleep(sing([@c4 @e4], \"la la\", {dur: 0.5}))",
            "Drums" => "hit(@kick) sleep(1)",
            _ => A_NOTE,
        };
        let src = plays(ctor, code);
        let doc = model::ScoreDoc::test_load(&src).unwrap_or_else(|d| panic!("{ty}: {d:?}"));
        assert_eq!(crate::graph::project(&doc).nodes.iter().find(|n| n.id == "inst").map(|n| n.role), Some("inst"), "{ty}");
        let opts = RenderOpts { seconds: Some(2.0), seed: Some(1), ..RenderOpts::default() };
        let rendered = render(&src, &opts, &Tone, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{ty}: {d:?}"));
        assert!(peak(&rendered.left).max(peak(&rendered.right)) > 0.001, "{ty} is silent");
    }
}

#[test]
fn a_library_patch_plays_or_says_the_library_is_missing() {
    let src = plays("Tandem{patch: \"FX/Rather Low\"}", A_NOTE);
    let doc = model::ScoreDoc::test_load(&src).unwrap_or_else(|d| panic!("{d:?}"));
    let installed = crate::tandem_library::active().is_some();
    assert_eq!(doc.lacks_patch_library(), !installed, "the desk's install message follows the library");
    // The tools say so too, never a quiet substitute: a problem in
    // score_check, a warning in score_listen, "not installed" from tandem_patches.
    let says = |v: &makepad_strict_json::Value, key: &str| v.get(key).and_then(|p| p.as_arr()).is_some_and(|p| p.iter().any(|x| x.to_json().contains("FX/Rather Low") && x.to_json().contains("not installed")));
    let args = makepad_strict_json::obj(vec![("source", makepad_strict_json::s(src.clone())), ("bars", makepad_strict_json::Value::Int(1))]);
    let checked = crate::tools::call("score_check", &args, None, &crate::PrepareControl::default()).unwrap();
    assert_eq!(says(&checked, "problems"), !installed, "score_check: {}", checked.to_json());
    let heard = crate::tools::call("score_listen", &args, None, &crate::PrepareControl::default()).unwrap();
    assert_eq!(says(&heard, "warnings"), !installed, "score_listen: {}", heard.to_json());
    let listed = crate::tools::call("tandem_patches", &makepad_strict_json::Value::Obj(Vec::new()), None, &crate::PrepareControl::default()).unwrap();
    assert_eq!(listed.get("installed").and_then(|v| v.as_bool()), Some(installed));
    let opts = RenderOpts { seconds: Some(2.0), seed: Some(1), ..RenderOpts::default() };
    let rendered = render(&src, &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(peak(&rendered.left).max(peak(&rendered.right)) > 0.001, "silent (library installed: {installed})");
    // Installed, it is that patch: not the plain starting sound.
    if installed {
        let plain = render(&plays("Tandem{}", A_NOTE), &opts, &NoSamples, &crate::PrepareControl::default()).unwrap_or_else(|d| panic!("{d:?}"));
        assert_ne!(rendered.left, plain.left, "the patch plays the starting sound");
    }

    // The rest outlasts the capture limit. The final three seconds match the
    // catalogue's root note at velocity 100, held for 1.5 seconds.
    use makepad_strict_json::{self as json, Value};
    let sub = crate::palette::entries().into_iter().find(|e| e.id == "patch:sub").unwrap();
    let source = format!(
        "use mod.score.*\nlet clock = Clock{{bpm: 60 sig: [8 4]}}\nlet inst = {}\nlet line = Loop{{play: inst beats: 8 notes: [[5 {} 1.5 {}]]}}\nlet quiet = Bus{{}}\nScore{{clock inst line quiet}}\n",
        sub.code(), sub.root, 100.0 / 127.0,
    );
    let args = json::obj(vec![("source", json::s(source)), ("bars", Value::Int(1))]);
    let heard = crate::tools::call("score_listen", &args, None, &crate::PrepareControl::default()).unwrap();
    let nodes = heard.get("nodes").expect("score_listen rendered the one-note score");
    assert!(nodes.get("quiet").unwrap().get("measured").is_none(), "silent bus: {nodes:?}");
    let measured = nodes.get("line").unwrap().get("measured").expect("the sub note sounded");
    let number = |key| match measured.get(key) {
        Some(Value::Int(v)) => *v as f32,
        Some(Value::F64(v)) => *v as f32,
        value => panic!("expected a number for {key}, got {value:?}"),
    };
    let expected = sub.features.unwrap();
    assert!((number("attack_ms") - expected.attack_ms).abs() <= 5.0, "sub attack: {measured:?} vs {expected:?}");
    assert!((number("centroid_hz") - expected.centroid).abs() <= 0.1 * expected.centroid, "sub centroid: {measured:?} vs {expected:?}");
}

/// A shader in a playing score follows the song's own clock: at every
/// pass's first frame its `beat` is the transport's beat (to one f32 ulp)
/// and its `bar` that beat over the bar's length, through a tempo step and
/// a ramp that split passes. The transport plays throughout this check.
#[test]
fn a_shader_follows_the_transports_beat_and_bar() {
    use makepad_audio_engine::schedule::{Op, SPAN};
    let src = "use mod.score.*\nlet clock = Clock{bpm: 120 sig: [3 4]}\nlet s = Synth{preset: @pluck}\n\
        let pulse = Loop{play: s beats: 1 code: fn(i) { play(60, {dur: 0.1}) sleep(1) }}\n\
        let a = Section{bars: 1 loops: [pulse]}\nlet b = Section{bars: 2 loops: [pulse] bpm: 150}\nlet c = Section{bars: 2 loops: [pulse] bpm: 70 ramp: 2}\n\
        let meter = ShaderFx{code: \"fn effect(l, r) { vec2(beat, bar) }\"}\n\
        Score{clock s pulse meter a b c arrange: [a b c]}\n";
    let (mut passes, mut split_passes, mut start) = (0usize, 0usize, 0usize);
    let mut worst = 0.0f32;
    let opts = RenderOpts { seconds: Some(8.0), ..RenderOpts::default() };
    crate::render::render_with(src, &opts, &NoSamples, &crate::PrepareControl::default(), &mut |frame, player| {
        let schedule = &player.schedule;
        let out = schedule.ops.iter().find_map(|op| if let Op::ShaderFx { out, .. } = op { Some(*out as usize) } else { None }).expect("the score has its shader effect");
        let chunk = schedule.clocks.first().expect("the score has a clock");
        let (left, right) = (schedule.arena[out * SPAN], schedule.arena[out * SPAN + CHUNK]);
        let want = chunk.start as f32;
        let ulp = f32::from_bits(want.to_bits() + 1) - want;
        assert!((left - want).abs() <= ulp, "beat at the pass starting at frame {start}: shader {left}, transport {} ({} apart, one ulp is {ulp})", chunk.start, (left - want).abs());
        let bar = left / 3.0;
        assert!((right - bar).abs() <= f32::from_bits(bar.to_bits() + 1) - bar, "bar at the pass starting at frame {start}: shader {right}, beat / 3 is {bar}");
        worst = worst.max((left - want).abs() / ulp.max(f32::MIN_POSITIVE));
        passes += 1;
        split_passes += usize::from(chunk.time.count > 1);
        start = frame;
    }).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(passes > 1000, "the render ran {passes} passes");
    assert!(split_passes > 0, "a pass held a tempo change, so the shader's time was cut inside it");
    println!("worst beat error {worst} ulp over {passes} passes, {split_passes} with a tempo change");
}

#[test]
fn shader_effect_plays_a_recording_when_it_arrives() {
    use crate::player::{ClockMode, Player};
    use makepad_audio_engine::{feed::samples, schedule::{Op, SPAN}};
    use makepad_score_player::Sample;
    use std::sync::Arc;
    let src = r#"use mod.score.*
let clock = Clock{}
let fx = ShaderFx{code: "let src = sample(2)\nvar age = int(0)\nfn effect(l,r) { age += 1\n if age > 8 && len(src) == 4 { src[0] } else { vec2(0.0) } }" src: asset("shader_table_arrival", "1")}
Score{clock fx}"#;
    let bad = src.replace("asset(\"shader_table_arrival\", \"1\")", "2");
    let errors = model::ScoreDoc::test_load(&bad).unwrap_err();
    assert!(errors.iter().any(|e| e.message.contains("src") && e.message.contains("asset")), "{errors:?}");
    let doc = model::ScoreDoc::test_load(src).unwrap_or_else(|d| panic!("{d:?}"));
    let assets = doc.samples();
    assert_eq!(assets.len(), 1);
    let key = crate::player::sample_name(&assets[0]);
    samples::forget(&key);
    let mut before = prepare_score(&doc, 48000.0, "table_score", ClockMode::Own, None, 1, &crate::PrepareControl::default()).unwrap();
    const PASS: usize = 8;
    let pass = |player: &mut Player| -> [[f32; 2]; PASS] {
        player.run(PASS, &mut |_, _, _| {});
        let out = player.schedule.ops.iter().find_map(|op| if let Op::ShaderFx { out, .. } = op { Some(*out as usize) } else { None }).unwrap();
        std::array::from_fn(|i| [player.schedule.arena[out * SPAN + i], player.schedule.arena[out * SPAN + CHUNK + i]])
    };
    assert_eq!(pass(&mut before), [[0.0; 2]; PASS], "waiting for the recording");
    samples::register(&key, Arc::new(Sample::new(vec![0.5, 0.0].into(), Some(vec![-0.25, 0.0].into()), 24000.0)));
    let mut after = prepare_score(&doc, 48000.0, "table_score", ClockMode::Own, Some(&before.layout), 2, &crate::PrepareControl::default()).unwrap();
    after.schedule.take_over(&mut before.schedule);
    for at in (0..FADE as usize + PASS).step_by(PASS) {
        for (i, stereo) in pass(&mut after).into_iter().enumerate() {
            let k = at + i;
            let gain = k.min(FADE as usize) as f32 / FADE as f32;
            assert_eq!(stereo, [0.5 * gain, -0.25 * gain], "resampled stereo data and carried age at frame {k}");
        }
    }
    samples::forget(&key);
}

/// Prepare words start only at a request boundary. A helper allocating one would
/// detach its compiler or render from the request that is supposed to stop it.
#[test]
fn only_request_owners_create_prepare_controls() {
    use std::path::Path;

    const OWNERS: [(&str, &str); 13] = [
        ("score_live/src/palette/sounds.rs", "load"), // The trusted shipped palette is read once on first use from bundled sources.
        ("score_live/src/bin/score_palette.rs", "main"), // The palette CLI owns the audition.
        ("score_live/src/bin/neural_latency/native.rs", "run"), // The neural latency CLI owns its music request.
        ("midi_library/src/bin/midi_library.rs", "run"), // The MIDI CLI (also midi-text) owns the text or cover request.
        ("midi_library/src/tools.rs", "receive"), // The MCP reader owns the word until that call replies or the input closes.
        ("music_iter/src/bin/music_iter.rs", "main"), // The Music Iter CLI owns its song or batch.
        ("music_corpus/src/bin/music_corpus.rs", "build"), // The build command owns all its generated examples.
        ("music_corpus/src/bin/music_corpus.rs", "run_eval"), // The eval command owns all predictions and reference reads.
        ("score_live/src/live.rs", "try_set_prepared"), // The sender owns each accepted playing source, including an audition.
        ("score_live/src/model.rs", "submit"),       // RevisionLoads owns the pending source revision.
        ("score_live/src/node.rs", "new"),           // Slot owns the observer for its player generations.
        ("score_live/src/node.rs", "begin"),         // Host owns the pending player generation.
        ("score_live/src/game.rs", "begin"),         // MusicSlot owns its music request.
    ];

    let libs = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut seen = [false; OWNERS.len()];
    let mut violations = Vec::new();
    for name in ["score_live", "midi_library", "music_iter", "music_corpus"] {
        scan_control_sites(libs, &libs.join(name).join("src"), &OWNERS, &mut seen, &mut violations, "", |compact| {
            compact.contains(".renewed()") || compact.contains("PrepareControl::default(") || compact.contains("PrepareControl::new(")
                || (compact.contains("PrepareControl=") && compact.contains("Default::default("))
        });
    }
    for (owner, found) in OWNERS.iter().zip(seen) { assert!(found, "missing request owner {owner:?}"); }
    assert!(violations.is_empty(), "helper created a prepare control: {}", violations.join(", "));
}

fn scan_control_sites(root: &std::path::Path, dir: &std::path::Path, owners: &[(&str, &str)], seen: &mut [bool],
    violations: &mut Vec<String>, needle: &str, is_call: fn(&str) -> bool) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() { continue; }
        let path = entry.path();
        if kind.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with('.') && !matches!(name.as_ref(), "tests" | "target" | "node_modules" | "local") {
                scan_control_sites(root, &path, owners, seen, violations, needle, is_call);
            }
            continue;
        }
        let file = path.file_name().unwrap().to_str().unwrap();
        if !file.ends_with(".rs") || file == "tests.rs" || file.ends_with("_tests.rs") { continue; }
        let source = std::fs::read_to_string(&path).unwrap();
        if !source.contains(needle) { continue; }
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        let mut function = "";
        let mut test_item = None;
        let mut test_body = false;
        for (line_number, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[test]") {
                test_item.get_or_insert(indent);
            }
            if let Some(start) = test_item {
                if !test_body && trimmed.contains('{') { test_body = true; }
                if indent == start && (matches!(trimmed, "}" | "};") || (!test_body && trimmed.ends_with(';'))) {
                    test_item = None;
                    test_body = false;
                }
                continue;
            }
            let code = trimmed.split("//").next().unwrap();
            if let Some((_, declaration)) = code.split_once("fn ") {
                function = declaration.split(['(', '<', ' ']).next().unwrap_or("");
            }
            let compact: String = code.split_whitespace().collect();
            if !is_call(&compact) { continue; }
            if let Some(owner) = owners.iter().position(|site| *site == (relative.as_str(), function)) {
                seen[owner] = true;
            } else {
                violations.push(format!("{relative}:{} ({function})", line_number + 1));
            }
        }
    }
}

#[test]
fn only_batch_owners_set_instruction_budgets() {
    use std::path::Path;
    const OWNERS: &[(&str, &str)] = &[
        ("libs/music_corpus/src/corpus.rs", "build"), // One read per generated example.
        ("libs/music_corpus/src/eval.rs", "evaluate"), // Each prediction, reference and cover.
        ("libs/music_iter/src/batch.rs", "history"), // The director scans saved songs.
        ("libs/music_iter/src/batch.rs", "compose"), // One generated song and its revision.
        ("apps/apps/synth/src/library.rs", "titles"), // Titles for the library, open and instrument lists.
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let mut seen = vec![false; OWNERS.len()];
    let mut violations = Vec::new();
    // The crates that evaluate scores on a caller's control; the rest of the tree has no reason to.
    for dir in ["libs/score_live", "libs/midi_library", "libs/music_iter", "libs/music_corpus", "libs/flow", "apps/apps/synth"] {
        let dir = root.join(dir).join("src");
        if !dir.is_dir() { continue; }
        scan_control_sites(root, &dir, OWNERS, &mut seen, &mut violations, "with_instruction_budget", |code| {
            code.contains("with_instruction_budget") && !code.contains("fnwith_instruction_budget(")
        });
    }
    for ((file, function), found) in OWNERS.iter().zip(seen) {
        if file.starts_with("apps/apps/") && !root.join("apps/apps/synth").is_dir() { continue; }
        assert!(found, "budget owner missing: {file} ({function})");
    }
    assert!(violations.is_empty(), "instruction budget outside a batch owner: {}", violations.join(", "));
}
