//! The score's tools (`score_*`) as the chat's model calls them: reading and
//! changing the score are answered at once; listening and rendering run on
//! their own threads and answer when done. `score_guide`, `score_check`,
//! `score_listen` and `instrument_search` are the Score library's own
//! (`makepad_score_live::tools`); this shell gives them the open score.

use super::Desk;
use crate::library::{self, Document};
use makepad_score_live::{source, templates, tools, NoSamples, RenderOpts};
use makepad_strict_json::{self as json, Value as Json};
use makepad_widgets::*;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

/// A tool's answer: now, or when job `id` is done.
pub enum Answer {
    Done(Result<Json, String>),
    Later(u64),
}

fn ok(value: Json) -> Answer {
    Answer::Done(Ok(value))
}

fn refused(message: impl Into<String>) -> Answer {
    Answer::Done(Err(message.into()))
}

fn num(value: f64) -> Json {
    Json::F64((value * 1000.0).round() / 1000.0)
}

fn text(value: impl Into<String>) -> Json {
    json::s(value)
}

fn f64_arg(args: &Json, key: &str) -> Option<f64> {
    match args.get(key) {
        Some(Json::F64(n)) => Some(*n),
        Some(Json::Int(n)) => Some(*n as f64),
        _ => None,
    }
}

type JobResult = Result<Json, String>;

/// Background work: renders and measurements.
#[derive(Default)]
pub struct Jobs {
    next: u64,
    tx: Option<Sender<(u64, JobResult)>>,
    rx: Option<Receiver<(u64, JobResult)>>,
    running: Vec<u64>,
    done: HashMap<u64, JobResult>,
    /// What the header says when a job ends (a render written).
    message: Option<String>,
    /// Samples and SoundFonts loaded since the last look.
    loaded: usize,
}

impl Jobs {
    pub fn spawn(&mut self, work: impl FnOnce() -> JobResult + Send + 'static) -> u64 {
        if self.tx.is_none() {
            let (tx, rx) = channel();
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        self.next += 1;
        let id = self.next;
        let Some(tx) = self.tx.clone() else { return id };
        self.running.push(id);
        let spawned = std::thread::Builder::new().name("synth-job".into()).spawn(move || {
            let _ = tx.send((id, work()));
        });
        if spawned.is_err() {
            self.running.retain(|r| *r != id);
            self.done.insert(id, Err("no thread for the job".into()));
        }
        id
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok((id, result)) = rx.try_recv() {
            self.running.retain(|r| *r != id);
            match &result {
                Ok(value) => {
                    if let Some(path) = value.get("path").and_then(Json::as_str) {
                        self.message = Some(format!("rendered to {path}"));
                    }
                    if let Some(n) = value.get("loaded").and_then(Json::as_u64) {
                        self.loaded += n as usize;
                    }
                    if let Some(Json::Arr(failed)) = value.get("failed") {
                        if let Some(first) = failed.first().and_then(Json::as_str) {
                            self.message = Some(first.to_string());
                        }
                    }
                }
                Err(error) => self.message = Some(error.clone()),
            }
            self.done.insert(id, result);
        }
    }

    pub fn busy(&self) -> bool {
        !self.running.is_empty()
    }

    /// How many samples and SoundFonts arrived since the last call.
    pub fn take_loaded(&mut self) -> usize {
        std::mem::take(&mut self.loaded)
    }

    pub fn finished_message(&mut self) -> Option<String> {
        self.message.take()
    }

    pub fn take(&mut self, id: u64) -> Option<JobResult> {
        self.poll();
        self.done.remove(&id)
    }
}

/// Render `src` to a WAV file: `path`, else the renders folder under its
/// title. Its timing comes back with it.
pub fn render_wav(src: &str, seconds: Option<f64>, seed: Option<u64>, path: Option<std::path::PathBuf>) -> JobResult {
    let rendered = makepad_score_live::render(src, &RenderOpts { rate: 48_000.0, seconds, seed, params: Vec::new(), start: None, held: Vec::new() }, &NoSamples)
        .map_err(|diags| diags.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("; "))?;
    let title = makepad_score_live::ScoreDoc::load(src).ok().map(|d| d.label).filter(|l| !l.trim().is_empty()).unwrap_or_else(|| "score".into());
    let path = path.unwrap_or_else(|| {
        let stem: String = title.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        crate::paths::renders_dir().join(format!("{stem}.wav"))
    });
    let wav = makepad_audio_synth::wav::stereo_wav_16(&rendered.left, &rendered.right, 48_000);
    std::fs::write(&path, wav).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json::obj(vec![
        ("path", text(path.to_string_lossy())),
        ("seconds", num(rendered.seconds())),
        ("bpm", num(rendered.bpm)),
        ("beats_per_bar", num(rendered.beats_per_bar)),
        ("sections", Json::Arr(rendered.sections.iter().enumerate().map(|(i, (beat, name))| {
            let end = rendered.sections.get(i + 1).map_or(rendered.seconds(), |(next, _)| rendered.seconds_at(*next));
            json::obj(vec![("beat", num(*beat)), ("seconds", num(rendered.seconds_at(*beat))), ("end_seconds", num(end)), ("name", text(name.clone()))])
        }).collect())),
    ]))
}

/// The longest sample decoded: a minute at 48 kHz.
const MAX_SAMPLE_FRAMES: usize = 48_000 * 60;
/// The largest SoundFont read.
const MAX_FONT_BYTES: u64 = 256 << 20;

/// The samples and SoundFonts a score names, read from the samples folder
/// (`asset("<file>", "file")`), decoded and registered under their names.
pub fn load_samples(assets: Vec<makepad_score_live::AssetRef>) -> JobResult {
    load_from(&crate::paths::samples_dir(), assets)
}

/// [`load_samples`] from the folder `dir`.
fn load_from(dir: &std::path::Path, assets: Vec<makepad_score_live::AssetRef>) -> JobResult {
    let (mut loaded, mut failed) = (0u64, Vec::new());
    for asset in assets {
        let got = (|| -> Result<(), String> {
            // A sample of a chip or tracker song opened before.
            if let Some(sample) = makepad_midi_library::chip::import::stored(&asset) {
                makepad_score_live::render::register_sample(&asset, sample);
                return Ok(());
            }
            if asset.rev != library::FILE_REV {
                return Err(format!("`asset(\"{}\", \"{}\")`: a sample here is a file of the samples folder, asset(\"<file>\", \"{}\")", asset.id, asset.rev, library::FILE_REV));
            }
            let path = dir.join(&asset.id);
            let len = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"sfbk") {
                if len > MAX_FONT_BYTES {
                    return Err(format!("{}: the SoundFont is {len} bytes", asset.id));
                }
                makepad_score_live::render::register_file(&asset, bytes.into());
                return Ok(());
            }
            let audio = makepad_audio_decode::decode_any(&bytes).map_err(|e| format!("{}: {e:?}", asset.id))?;
            let channels = audio.channels.max(1) as usize;
            let frames = audio.frames().min(MAX_SAMPLE_FRAMES);
            let pcm = &audio.pcm_interleaved_f32;
            let left: Vec<f32> = (0..frames).map(|i| pcm[i * channels]).collect();
            let right: Option<Vec<f32>> = (channels > 1).then(|| (0..frames).map(|i| pcm[i * channels + 1]).collect());
            let sample = makepad_score_player::Sample::new(left.into(), right.map(Into::into), audio.rate.max(1) as f32);
            makepad_score_live::render::register_sample(&asset, std::sync::Arc::new(sample));
            Ok(())
        })();
        match got {
            Ok(()) => loaded += 1,
            Err(error) => failed.push(text(error)),
        }
    }
    Ok(json::obj(vec![("loaded", Json::Int(loaded as i64)), ("failed", Json::Arr(failed))]))
}

/// What changed from `before` to `after`, as lines: the changed span between
/// the lines both keep at the start and the end, `context` lines around it,
/// `-` for a line that went and `+` for one that came (at most 80 lines).
pub fn changed_lines(before: &str, after: &str, context: usize) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    if head == a.len() && head == b.len() {
        return String::new();
    }
    let from = head.saturating_sub(context);
    let mut out = vec![format!("@@ line {} @@", from + 1)];
    out.extend(a[from..head].iter().map(|l| format!(" {l}")));
    out.extend(a[head..a.len() - tail].iter().map(|l| format!("-{l}")));
    out.extend(b[head..b.len() - tail].iter().map(|l| format!("+{l}")));
    out.extend(b[b.len() - tail..].iter().take(context).map(|l| format!(" {l}")));
    if out.len() > 80 {
        let more = out.len() - 80;
        out.truncate(80);
        out.push(format!("… {more} more lines"));
    }
    out.join("\n")
}

/// The problems that keep `source_text` from being written: the library's
/// `score_check` (its loops run for 8 bars).
pub fn check(source_text: &str) -> Vec<String> {
    tools::check_diags(source_text).iter().map(|d| d.to_string()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn changed_lines_show_the_changed_span_with_context() {
        let diff = super::changed_lines("a\nb\nc\nd\ne", "a\nb\nX\nd\ne", 1);
        assert_eq!(diff, "@@ line 2 @@\n b\n-c\n+X\n d");
        assert_eq!(super::changed_lines("a", "a", 2), "");
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("synth-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Export: a template rendered to a WAV file, with its timing.
    #[test]
    fn a_score_renders_to_a_wav_file() {
        let dir = temp_dir("render");
        let path = dir.join("techno.wav");
        let source = makepad_score_live::templates::template("techno").unwrap();
        let out = super::render_wav(source, Some(2.0), None, Some(path.clone())).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE");
        // 2 s of 48 kHz stereo 16-bit, give or take the tail.
        assert!(bytes.len() > 48_000 * 4, "{} bytes", bytes.len());
        assert!(out.get("seconds").and_then(|v| match v { super::Json::F64(f) => Some(*f), _ => None }).is_some_and(|s| s >= 2.0));
        std::fs::remove_file(&path).unwrap();
    }

    /// A sample named by file is decoded from the samples folder and
    /// registered: the score no longer misses it.
    #[test]
    fn a_sample_file_is_loaded_by_name() {
        let dir = temp_dir("samples");
        let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        std::fs::write(dir.join("synth_test_tone.wav"), makepad_audio_synth::wav::stereo_wav_16(&tone, &tone, 48_000)).unwrap();
        let asset = makepad_score_live::AssetRef { id: "synth_test_tone.wav".into(), rev: crate::library::FILE_REV.into() };
        let out = super::load_from(&dir, vec![asset.clone()]).unwrap();
        assert_eq!(out.get("loaded").and_then(super::Json::as_u64), Some(1), "{}", out.to_json());
        let wrong = makepad_score_live::AssetRef { id: "x".into(), rev: "7".into() };
        let out = super::load_from(&dir, vec![wrong]).unwrap();
        assert_eq!(out.get("loaded").and_then(super::Json::as_u64), Some(0));
        std::fs::remove_file(dir.join("synth_test_tone.wav")).unwrap();
    }
}

impl Desk {
    /// The open score for the model: its shape, its problems and (asked)
    /// its source.
    pub fn read_json(&self, with_source: bool) -> Json {
        let mut out = vec![("revision", Json::Int(self.revision as i64)), ("title", text(self.document.title.clone()))];
        if let Some(doc) = &self.doc {
            out.push(("bpm", num(doc.clock.bpm)));
            out.push(("sig", text(format!("{}/{}", doc.clock.sig.0, doc.clock.sig.1))));
            out.push(("seed", Json::Int(doc.clock.seed as i64)));
            out.push(("params", Json::Obj(doc.clock.params.iter().map(|(n, v)| (n.clone(), num(*v as f64))).collect())));
            out.push(("nodes", Json::Arr(doc.order.iter().map(|(n, t)| text(format!("{n}: {t}"))).collect())));
            out.push(("loops", Json::Arr(doc.loops.iter().map(|l| json::obj(vec![
                ("name", text(l.name.clone())),
                ("play", text(l.play.clone().unwrap_or_default())),
                ("beats", l.beats.map_or(Json::Null, num)),
                ("amp", num(l.amp as f64)),
                ("pan", num(l.pan as f64)),
                ("mute", Json::Bool(l.mute)),
                ("solo", Json::Bool(l.solo)),
                ("kind", text(if l.stinger.is_some() { "stinger" } else if l.code.is_some() { "code" } else { "pattern" })),
            ])).collect())));
            out.push(("sections", Json::Arr(doc.sections.iter().map(|s| text(format!("{}: {} bars [{}]", s.name, s.bars, s.loops.join(" ")))).collect())));
            out.push(("arrange", Json::Arr(doc.arrange.iter().map(|a| text(a.clone())).collect())));
        }
        out.push(("problems", Json::Arr(self.problems().into_iter().map(text).collect())));
        if with_source {
            out.push(("source", text(self.source.clone())));
        }
        json::obj(out)
    }

    fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = match makepad_score_live::ScoreDoc::load(&self.source) {
            Ok(_) => Vec::new(),
            Err(diags) => diags.iter().map(|d| d.to_string()).collect(),
        };
        for l in &self.status.loops {
            if let Some(e) = &l.error {
                out.push(e.to_string());
            }
        }
        out
    }

    /// Write `next` as the score (checked first; `base` is the revision it
    /// was made from).
    fn write(&mut self, cx: &mut Cx, ui: &WidgetRef, next: String, base: Option<i64>) -> Answer {
        if let Some(base) = base {
            if base != self.revision as i64 {
                return refused(format!("the score changed since revision {base} (it is {} now): read it again", self.revision));
            }
        }
        let problems = check(&next);
        if !problems.is_empty() {
            return refused(format!("the score does not load, nothing changed:\n{}", problems.join("\n")));
        }
        let before = self.source.clone();
        self.set_source(cx, ui, next);
        let diff = changed_lines(&before, &self.source, 2);
        ok(json::obj(vec![("revision", Json::Int(self.revision as i64)), ("diff", text(diff))]))
    }

    /// A tool of the score, run.
    pub fn tool(&mut self, cx: &mut Cx, ui: &WidgetRef, name: &str, args: &Json) -> Answer {
        let base = args.get("base_revision").and_then(Json::as_i64);
        match name {
            "score_guide" | "score_check" | "instrument_search" | "morph_params" | "tandem_params" | "tandem_patches" => match tools::call(name, args, Some(&self.source)) {
                Ok(value) => ok(value),
                Err(message) => refused(message),
            },
            "score_panel_guide" => ok(json::obj(vec![("guide", text(super::panel::guide()))])),
            "score_read" => ok(self.read_json(args.get("with_source").and_then(Json::as_bool) != Some(false))),
            "score_write" => match args.get("source").and_then(Json::as_str) {
                Some(next) => self.write(cx, ui, next.to_string(), base),
                None => refused("`source`: the whole score"),
            },
            "score_patch" => {
                let Some(Json::Arr(edits)) = args.get("edits") else { return refused("`edits`: [{old, new}]") };
                let mut next = self.source.clone();
                for (i, edit) in edits.iter().enumerate() {
                    let (Some(old), Some(new)) = (edit.get("old").and_then(Json::as_str), edit.get("new").and_then(Json::as_str)) else {
                        return refused(format!("edit {}: `old` and `new`", i + 1));
                    };
                    match next.matches(old).count() {
                        1 => next = next.replacen(old, new, 1),
                        0 => return refused(format!("edit {}: `old` is not in the score (read it again)", i + 1)),
                        n => return refused(format!("edit {}: `old` is in the score {n} times: give more of it", i + 1)),
                    }
                }
                self.write(cx, ui, next, base)
            }
            "score_new" => {
                let template = args.get("template").and_then(Json::as_str).unwrap_or("blank");
                let Some(mut next) = templates::template(template).map(str::to_string) else { return refused(format!("no template `{template}`")) };
                if let Some(bpm) = f64_arg(args, "bpm") {
                    if let Ok(s) = source::set_field(&next, "clock", "bpm", &source::fmt_num(bpm, 1)) {
                        next = source::apply(&next, &s);
                    }
                }
                if let Some(label) = args.get("label").and_then(Json::as_str) {
                    if let Ok(s) = source::set_field(&next, "", "label", &makepad_editor_panel::fmt_text(label)) {
                        next = source::apply(&next, &s);
                    }
                }
                let problems = check(&next);
                if !problems.is_empty() {
                    return refused(format!("the score does not load, nothing changed:\n{}", problems.join("\n")));
                }
                // A new score is a new document: the one open keeps its draft.
                let title = args.get("label").and_then(Json::as_str).filter(|label| !label.trim().is_empty()).unwrap_or("Untitled");
                self.open_document(cx, ui, Document::new(title), next);
                ok(json::obj(vec![("revision", Json::Int(self.revision as i64))]))
            }
            "score_play" => {
                let play = args.get("play").and_then(Json::as_bool).unwrap_or(true);
                if let Some(section) = args.get("section").and_then(Json::as_str) {
                    if let Some(live) = &self.live {
                        live.goto_section(section, 0.0);
                    }
                } else if let Some(bar) = f64_arg(args, "bar") {
                    let per_bar = self.doc.as_ref().map_or(4.0, |d| d.beats_per_bar());
                    self.seek(bar.max(0.0) * per_bar, 0.0);
                }
                self.transport(play);
                self.frame = cx.new_next_frame();
                ok(json::obj(vec![("playing", Json::Bool(play))]))
            }
            "score_status" => {
                use makepad_score_player::Meters;
                let status = &self.status;
                let meters = self.engine.meters.clone();
                let db = |x: f32| num(if x > 1e-6 { 20.0 * x.log10() as f64 } else { -90.0 });
                let bar = self.doc.as_ref().map_or(4.0, |d| d.beats_per_bar());
                let levels: Vec<(String, Json)> = status.loops.iter().map(|l| {
                    let (peak, rms) = meters.level(l.key).unwrap_or((0.0, 0.0));
                    (l.name.clone(), json::obj(vec![("peak_db", db(peak)), ("rms_db", db(rms))]))
                }).collect();
                let master = meters.level(crate::audio::MASTER_KEY).unwrap_or((0.0, 0.0));
                ok(json::obj(vec![
                    ("playing", Json::Bool(status.playing)),
                    ("bar", Json::Int((status.beat / bar).floor() as i64)),
                    ("beat_in_bar", num(status.beat - (status.beat / bar).floor() * bar)),
                    ("bpm", num(status.bpm)),
                    ("section", status.section.clone().map_or(Json::Null, text)),
                    ("loops", Json::Arr(status.loops.iter().map(|l| json::obj(vec![
                        ("name", text(l.name.clone())), ("cycle", Json::Int(l.cycle as i64)), ("beats", num(l.len)), ("resting", Json::Bool(l.resting)),
                        ("error", l.error.as_ref().map_or(Json::Null, |e| text(e.to_string()))),
                        ("log", Json::Arr(l.logs.iter().map(|line| text(line.clone())).collect())),
                    ])).collect())),
                    ("levels", Json::Obj(levels)),
                    ("master", json::obj(vec![("peak_db", db(master.0)), ("rms_db", db(master.1))])),
                    ("params", Json::Obj(status.params.iter().map(|(n, v)| (n.clone(), num(*v as f64))).collect())),
                    ("dropped_events", Json::Int(status.dropped as i64)),
                    ("audio_device", Json::Bool(self.engine.device_rate().is_some())),
                    ("problems", Json::Arr(self.problems().into_iter().map(text).collect())),
                ]))
            }
            "score_listen" => {
                let args = args.clone();
                let open = self.source.clone();
                Answer::Later(self.jobs.spawn(move || tools::listen_args(&args, Some(&open)).map(|(value, _)| value)))
            }
            "score_set" => {
                let Some(live) = &self.live else { return refused("the score is not loaded") };
                let mut done = Vec::new();
                if let (Some(param), Some(value)) = (args.get("param").and_then(Json::as_str), f64_arg(args, "value")) {
                    live.set_param(param, value as f32);
                    done.push(format!("{param} = {value}"));
                }
                if let Some(section) = args.get("section").and_then(Json::as_str) {
                    live.goto_section(section, -1.0);
                    done.push(format!("to section {section} on the next bar"));
                }
                if let Some(stinger) = args.get("trigger").and_then(Json::as_str) {
                    live.trigger(stinger);
                    done.push(format!("stinger {stinger}"));
                }
                ok(json::obj(vec![("done", Json::Arr(done.into_iter().map(text).collect()))]))
            }
            "score_render" => {
                let src = self.source.clone();
                let problems = check(&src);
                if !problems.is_empty() {
                    return refused(format!("the score does not load:\n{}", problems.join("\n")));
                }
                let seconds = f64_arg(args, "seconds");
                let seed = args.get("seed").and_then(Json::as_u64);
                let path = args.get("path").and_then(Json::as_str).filter(|p| !p.trim().is_empty()).map(std::path::PathBuf::from);
                Answer::Later(self.jobs.spawn(move || render_wav(&src, seconds, seed, path)))
            }
            "score_save" => {
                let name = args.get("name").and_then(Json::as_str).filter(|n| !n.trim().is_empty());
                match self.save(name) {
                    Ok(path) => ok(json::obj(vec![("path", text(path.to_string_lossy()))])),
                    Err(why) => refused(why),
                }
            }
            "score_library" => ok(json::obj(vec![("scores", Json::Arr(library::saved_scores().into_iter().map(|(title, path)| {
                json::obj(vec![("title", text(title)), ("name", text(path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()))])
            }).collect()))])),
            "score_open" => {
                let Some(name) = args.get("name").and_then(Json::as_str) else { return refused("`name`: a score of score_library") };
                let Some(path) = library::find(name) else { return refused(format!("no score `{name}` in the library")) };
                match Document::open_file(&path) {
                    Ok((document, source)) => {
                        self.open_document(cx, ui, document, source);
                        ok(json::obj(vec![("title", text(self.document.title.clone())), ("revision", Json::Int(self.revision as i64))]))
                    }
                    Err(why) => refused(why),
                }
            }
            "score_editor_make" => match self.make_panel(cx) {
                Ok(()) => ok(json::obj(vec![("asked", Json::Bool(true))])),
                Err(why) => refused(why),
            },
            "score_editor_write" => {
                let Some(code) = args.get("code").and_then(Json::as_str) else { return refused("`code`: the whole `let editor_panel = fn() { View{…} }`") };
                match super::panel::with_panel(&self.source, code) {
                    Ok(next) => self.write(cx, ui, next, None),
                    Err(error) => refused(error),
                }
            }
            "score_editor_paths" => ok(json::obj(vec![("paths", text(super::panel::score_listing(&self.source, self.doc.as_ref())))])),
            "score_panel_drive" => {
                let Some(path) = args.get("path").and_then(Json::as_str) else { return refused("`path`") };
                let values: Vec<f64> = match args.get("values") {
                    Some(Json::Arr(items)) => items.iter().filter_map(|v| match v {
                        Json::Int(n) => Some(*n as f64),
                        Json::F64(n) => Some(*n),
                        _ => None,
                    }).collect(),
                    _ => Vec::new(),
                };
                if values.is_empty() {
                    return refused("`values`: at least one number");
                }
                let hold = args.get("hold").and_then(Json::as_bool) == Some(true);
                let n = values.len();
                self.panel.drive(path, values, hold);
                self.frame = cx.new_next_frame();
                ok(json::obj(vec![("queued", Json::Int(n as i64))]))
            }
            "score_panel_status" => ok(self.panel_status_json()),
            "score_vary" => {
                use makepad_synth_panel::host::VaryAction;
                let Some(node) = args.get("voice").and_then(Json::as_str).map(str::to_string) else { return refused("`voice`: a loop of the score") };
                let card = || f64_arg(args, "card").map(|c| c.max(0.0) as usize);
                let action = match args.get("action").and_then(Json::as_str).unwrap_or("make") {
                    "make" => {
                        let (kind, ai) = match (args.get("kind").and_then(Json::as_str), args.get("ask").and_then(Json::as_str)) {
                            (_, Some(ask)) => (ask.to_string(), true),
                            (Some(kind), None) => (kind.to_string(), false),
                            (None, None) => ("variations".to_string(), false),
                        };
                        if let Some(bars) = args.get("bars").and_then(Json::as_arr) {
                            let bar = self.doc.as_ref().map_or(4.0, |d| d.beats_per_bar());
                            let n = |k: usize| bars.get(k).and_then(|v| match v { Json::Int(i) => Some(*i as f64), Json::F64(f) => Some(*f), _ => None });
                            if let (Some(a), Some(b)) = (n(0), n(1)) {
                                let _ = self.vary_action(cx, ui, &node, VaryAction::Region(Some(((a - 1.0).max(0.0) * bar, b * bar))));
                            }
                        }
                        VaryAction::Make { kind, ai, n: f64_arg(args, "n").map_or(4, |n| n.clamp(1.0, 8.0) as usize) }
                    }
                    "audition" => VaryAction::Audition(card()),
                    "keep" => VaryAction::Keep(card().unwrap_or(0)),
                    "apply" => VaryAction::Apply(card().unwrap_or(0)),
                    "merge" => VaryAction::Merge(match args.get("picks") { Some(Json::Arr(items)) => items.iter().filter_map(|v| match v { Json::Int(i) => Some(*i as usize), Json::F64(f) => Some(*f as usize), _ => None }).collect(), _ => Vec::new() }),
                    "clear" => VaryAction::Clear,
                    other => return refused(format!("no action `{other}`: make, audition, keep, apply, merge, clear")),
                };
                self.frame = cx.new_next_frame();
                match self.vary_action(cx, ui, &node, action) {
                    Ok(done) => ok(json::obj(vec![("done", text(done)), ("revision", Json::Int(self.revision as i64))])),
                    Err(error) => refused(error),
                }
            }
            "score_vary_status" => {
                let Some(node) = args.get("voice").and_then(Json::as_str) else { return refused("`voice`") };
                let view = self.vary.view(node);
                let voice = self.vary.voices.get(node);
                let cards: Vec<Json> = view.cards.iter().enumerate().map(|(i, c)| {
                    let part = voice.and_then(|v| if i == 0 { v.original.clone() } else { v.cards.get(i - 1).map(|c| c.1.clone()) });
                    json::obj(vec![("card", Json::Int(i as i64)), ("label", text(c.label.clone())), ("kept", Json::Bool(c.kept)), ("beats", part.as_ref().map_or(Json::Null, |p| num(p.beats))), ("first_cycle", part.as_ref().map_or(Json::Null, |p| Json::Int(p.first as i64))), ("notes", text(part.map(|p| p.text()).unwrap_or_default()))])
                }).collect();
                // What the composer plays of this voice now, and whether it
                // is the card playing.
                let played = self.variant_live.clone().unwrap_or_else(|| self.source.clone());
                let live_part = makepad_score_live::vary::part_of(&played, node).ok();
                let playing_part = view.playing.and_then(|i| voice.and_then(|v| if i == 0 { v.original.clone() } else { v.cards.get(i - 1).map(|c| c.1.clone()) }));
                ok(json::obj(vec![
                    ("cards", Json::Arr(cards)),
                    ("busy", view.busy.map_or(Json::Null, text)),
                    ("region_beats", view.region.map_or(Json::Null, |(a, b)| Json::Arr(vec![num(a), num(b)]))),
                    ("playing", view.playing.map_or(Json::Null, |p| Json::Int(p as i64))),
                    ("in_place", Json::Bool(self.variant_live.is_some())),
                    ("played_source", if args.get("with_source").and_then(Json::as_bool) == Some(true) { text(played.clone()) } else { Json::Null }),
                    ("live_notes", live_part.as_ref().map_or(Json::Null, |p| text(p.text()))),
                    ("live_beats", live_part.as_ref().map_or(Json::Null, |p| num(p.beats))),
                    ("live_is_card", match (&live_part, &playing_part) {
                        (Some(a), Some(b)) => Json::Bool(a.notes.len() == b.notes.len() && a.notes.iter().zip(&b.notes).all(|(x, y)| (x.beat - y.beat).abs() < 1e-3 && x.note == y.note && x.hit == y.hit)),
                        _ => Json::Null,
                    }),
                    ("ai_made", self.vary_ai_made.map_or(Json::Null, |(n, s)| json::obj(vec![("variants", Json::Int(n as i64)), ("seconds", num(s))]))),
                ]))
            }
            "score_generate" => {
                let (Some(node), Some(gen)) = (args.get("node").and_then(Json::as_str), args.get("generator").and_then(Json::as_str)) else { return refused("`node` and `generator`") };
                let choices: Vec<(String, String)> = match args.get("choices") {
                    Some(Json::Obj(pairs)) => pairs.iter().map(|(k, v)| (k.clone(), match v {
                        Json::Str(s) => s.clone(),
                        Json::Int(n) => n.to_string(),
                        Json::F64(n) => n.to_string(),
                        other => other.to_json(),
                    })).collect(),
                    _ => Vec::new(),
                };
                let write = args.get("write").and_then(Json::as_bool) == Some(true);
                match self.generate(cx, ui, node.trim(), gen.trim(), &choices, write) {
                    Ok(code) => ok(json::obj(vec![("written", Json::Bool(write)), ("edits", text(code)), ("revision", Json::Int(self.revision as i64))])),
                    Err(error) => refused(error),
                }
            }
            "score_batch" | "score_batch_status" | "score_rate" | "score_learn" | "score_batch_stop" => Answer::Done(self.batch_tool_call(cx, ui, name, args)),
            other => refused(format!("no score tool `{other}`")),
        }
    }
}
