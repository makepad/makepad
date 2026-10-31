//! The build: songs from the sources, filtered and deduplicated, split by
//! song, turned into task examples and written as JSONL shards with the
//! manifest (datasets, licences, credits), the song list and the stats.
//!
//! The output folder (never in a checkout, never shipped):
//!
//! ```text
//! manifest.json          what was read, under which licences, how it was split
//! stats.json             songs in/kept/dropped by reason, examples and tokens per task
//! songs.tsv              every song: its split, or why it was dropped
//! train/<task>-0000.jsonl  val/...  test/...
//! ```

use crate::prep::{self, Signature, Split};
use crate::source::{Dataset, SongRef, Source};
use crate::tasks::{self, Example, Limits, Task};
use crate::tokens::{self, Tally, Tokenizer};
use makepad_strict_json::{obj, s, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub const FORMAT: &str = "makepad-music-corpus/1";

#[derive(Clone, Debug)]
pub struct Opts {
    pub out: PathBuf,
    pub min_quality: u8,
    pub meters: Vec<String>,
    pub min_bars: usize,
    pub threads: usize,
    /// Per mille of song identities in val and test.
    pub val: u32,
    pub test: u32,
    /// Two versions of one song this alike (tune and bass bars) are one.
    pub near_dup: f64,
    pub limits: Limits,
    pub shard_lines: usize,
    /// Only this many songs (a deterministic sample), for a quick build.
    pub max_songs: Option<usize>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            out: PathBuf::from("music_corpus"),
            min_quality: 70,
            meters: ["4/4", "3/4", "2/4", "6/8", "12/8", "2/2", "6/4"].iter().map(|m| m.to_string()).collect(),
            min_bars: 8,
            threads: 8,
            val: 50,
            test: 50,
            near_dup: 0.5,
            limits: Limits::default(),
            shard_lines: 4000,
            max_songs: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub songs_in: usize,
    pub kept: usize,
    pub dropped: BTreeMap<String, usize>,
    pub songs_per_split: BTreeMap<Split, usize>,
    pub example_drops: BTreeMap<(Task, String), usize>,
    pub tally: BTreeMap<(Split, Task), Tally>,
    pub seconds: f64,
}

/// What became of a song.
#[derive(Clone, Debug)]
enum Fate {
    Dropped(String),
    Kept(Split),
}

struct Song {
    r: SongRef,
    identity: String,
    fate: Option<Fate>,
    bars: usize,
    sig: Signature,
}

/// Run `f` on every index on `threads` threads, a panic counted as an
/// error.
fn par_map<T: Send, F: Fn(usize) -> T + Sync>(n: usize, threads: usize, f: F) -> Vec<Option<T>> {
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    std::thread::scope(|sc| {
        for _ in 0..threads.max(1) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(i))).ok();
                *results[i].lock().unwrap() = r;
            });
        }
    });
    results.into_iter().map(|m| m.into_inner().unwrap()).collect()
}

struct Shards {
    dir: PathBuf,
    per: usize,
    open: HashMap<(Split, Task), (std::io::BufWriter<std::fs::File>, usize, usize)>,
}

impl Shards {
    fn write(&mut self, split: Split, task: Task, line: &str) -> Result<(), String> {
        let key = (split, task);
        let need_new = self.open.get(&key).is_none_or(|o| o.1 >= self.per);
        if need_new {
            let shard = self.open.get(&key).map_or(0, |o| o.2 + 1);
            let dir = self.dir.join(split.name());
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{}-{shard:04}.jsonl", task.name()));
            let f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            self.open.insert(key, (std::io::BufWriter::new(f), 0, shard));
        }
        let o = self.open.get_mut(&key).unwrap();
        o.0.write_all(line.as_bytes()).and_then(|_| o.0.write_all(b"\n")).map_err(|e| e.to_string())?;
        o.1 += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        for o in self.open.values_mut() {
            o.0.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn record(ex: &Example, song: &SongRef, split: Split, ds: &Dataset, roles: &str) -> String {
    let want = obj(vec![
        ("key", s(ex.want.key.clone())),
        ("meter", s(ex.want.meter.clone())),
        ("bars", Value::Int(ex.want.bars as i64)),
        ("from_bar", Value::Int(ex.want.from_bar as i64)),
        ("part", s(ex.want.part.clone())),
        ("role", s(ex.want.role.clone())),
    ]);
    let meta = obj(vec![
        ("genre", s(song.genres.first().cloned().unwrap_or_default())),
        ("decade", song.decade().map_or(Value::Null, |d| Value::Int(d as i64))),
        ("tags", Value::Arr(tasks::style_tags(song).into_iter().map(s).collect())),
        ("meter", s(song.meter.clone())),
        ("roles", s(roles)),
        ("quality", song.quality.map_or(Value::Null, |q| Value::Int(q as i64))),
    ]);
    obj(vec![
        ("id", s(ex.id.clone())),
        ("task", s(ex.task.name())),
        ("split", s(split.name())),
        ("source", s(ds.id.clone())),
        ("licence", s(song.licence.clone())),
        ("song", s(song.id.clone())),
        ("bars", Value::Arr(vec![Value::Int(ex.bars.0 as i64), Value::Int(ex.bars.1 as i64)])),
        ("transpose", Value::Int(ex.shift as i64)),
        ("tempo_scale", Value::F64(ex.tempo_scale)),
        ("want", want),
        ("meta", meta),
        ("tokens", obj(vec![("total", Value::Int(ex.tokens as i64)), ("target", Value::Int(ex.target_tokens as i64))])),
        (
            "messages",
            Value::Arr(vec![
                obj(vec![("role", s("system")), ("content", s(tasks::SYSTEM))]),
                obj(vec![("role", s("user")), ("content", s(ex.user.clone()))]),
                obj(vec![("role", s("assistant")), ("content", s(ex.assistant.clone()))]),
            ]),
        ),
    ])
    .to_json()
}

/// Build the corpus into `opts.out`.
pub fn build(sources: &[Box<dyn Source>], tok: &Tokenizer, opts: &Opts, log: &dyn Fn(&str)) -> Result<Summary, String> {
    let t0 = std::time::Instant::now();
    let mut sum = Summary::default();
    std::fs::create_dir_all(&opts.out).map_err(|e| format!("{}: {e}", opts.out.display()))?;
    for split in Split::ALL {
        let dir = opts.out.join(split.name());
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
    }
    let datasets: Vec<Dataset> = sources.iter().map(|s| s.dataset()).collect();
    let mut songs: Vec<Song> = Vec::new();
    for (i, src) in sources.iter().enumerate() {
        let list = src.songs(i)?;
        log(&format!("{}: {} songs", datasets[i].id, list.len()));
        songs.extend(list.into_iter().map(|r| Song { identity: prep::identity(&r.artist, &r.title), r, fate: None, bars: 0, sig: Signature::default() }));
    }
    songs.sort_by(|a, b| a.r.id.cmp(&b.r.id));
    sum.songs_in = songs.len();
    let drop = |s: &mut Song, why: &str| s.fate = Some(Fate::Dropped(why.into()));

    // Metadata: quality, meter, the exact file.
    let mut order: Vec<usize> = (0..songs.len()).collect();
    order.sort_by(|&a, &b| songs[b].r.quality.cmp(&songs[a].r.quality).then(songs[a].r.id.cmp(&songs[b].r.id)));
    let mut seen_files: HashSet<String> = HashSet::new();
    for &i in &order {
        let s = &mut songs[i];
        if s.r.quality.is_some_and(|q| q < opts.min_quality) {
            drop(s, "low_quality");
        } else if !s.r.meter.is_empty() && !opts.meters.contains(&s.r.meter) {
            drop(s, "odd_meter");
        } else if !seen_files.insert(s.r.hash.clone()) {
            drop(s, "duplicate_file");
        }
    }
    if let Some(n) = opts.max_songs {
        let mut live: Vec<usize> = (0..songs.len()).filter(|&i| songs[i].fate.is_none()).collect();
        live.sort_by_key(|&i| prep::hash_str(&["sample", &songs[i].r.id]));
        for &i in live.iter().skip(n) {
            drop(&mut songs[i], "not_sampled");
        }
    }

    // Import every candidate: its bars and duplicate signatures.
    let live: Vec<usize> = (0..songs.len()).filter(|&i| songs[i].fate.is_none()).collect();
    log(&format!("importing {} songs on {} threads", live.len(), opts.threads));
    let refs: Vec<&SongRef> = live.iter().map(|&i| &songs[i].r).collect();
    let min_bars = opts.min_bars;
    let read = par_map(refs.len(), opts.threads, |k| -> Result<(usize, Signature), String> {
        let r = refs[k];
        let a = sources[r.source].arrangement(r).map_err(|_| "import_failed".to_string())?;
        if a.parts.iter().all(|p| p.notes.is_empty()) {
            return Err("no_notes".into());
        }
        if a.bar_count() < min_bars {
            return Err("too_short".into());
        }
        Ok((a.bar_count(), prep::signature(&a)))
    });
    for (k, r) in read.into_iter().enumerate() {
        let s = &mut songs[live[k]];
        match r {
            None => drop(s, "importer_panic"),
            Some(Err(e)) => drop(s, &e),
            Some(Ok((bars, sig))) => {
                s.bars = bars;
                s.sig = sig;
            }
        }
    }

    // Near duplicates: the same music under another name, or a version of
    // the same song this alike. The best-rated version stays.
    let mut seen_songs: HashSet<u64> = HashSet::new();
    let mut kept_by_identity: HashMap<String, Vec<usize>> = HashMap::new();
    for &i in &order {
        if songs[i].fate.is_some() {
            continue;
        }
        if !seen_songs.insert(songs[i].sig.song) {
            drop(&mut songs[i], "near_duplicate");
            continue;
        }
        let same = kept_by_identity.get(&songs[i].identity).is_some_and(|v| v.iter().any(|&j| prep::jaccard(&songs[i].sig.core, &songs[j].sig.core) >= opts.near_dup));
        if same {
            drop(&mut songs[i], "near_duplicate_version");
            continue;
        }
        kept_by_identity.entry(songs[i].identity.clone()).or_default().push(i);
        let split = prep::split_of(&songs[i].identity, opts.val, opts.test);
        songs[i].fate = Some(Fate::Kept(split));
    }
    for s in &songs {
        match &s.fate {
            Some(Fate::Kept(split)) => {
                sum.kept += 1;
                *sum.songs_per_split.entry(*split).or_default() += 1;
            }
            Some(Fate::Dropped(why)) => *sum.dropped.entry(why.clone()).or_default() += 1,
            None => {}
        }
    }
    log(&format!("{} songs kept of {} ({:.0} s)", sum.kept, sum.songs_in, t0.elapsed().as_secs_f64()));

    // The examples, a batch of songs at a time, written in song order.
    let overhead = tasks::overhead(tok);
    let kept: Vec<(usize, Split)> = songs.iter().enumerate().filter_map(|(i, s)| if let Some(Fate::Kept(sp)) = s.fate { Some((i, sp)) } else { None }).collect();
    let mut shards = Shards { dir: opts.out.clone(), per: opts.shard_lines, open: HashMap::new() };
    let batch = 256;
    let mut done = 0;
    for chunk in kept.chunks(batch) {
        let made = par_map(chunk.len(), opts.threads, |k| -> Result<(Vec<String>, Vec<(Task, usize, usize)>, Vec<tasks::Drop>), String> {
            let (i, split) = chunk[k];
            let r = &songs[i].r;
            let a = sources[r.source].arrangement(r)?;
            let built = tasks::build(&a, r, split, &opts.limits, tok, overhead);
            let mut roles: Vec<&str> = a.parts.iter().map(|p| p.role.name()).collect();
            roles.sort();
            roles.dedup();
            let roles = roles.join(",");
            let lines = built.examples.iter().map(|ex| record(ex, r, split, &datasets[r.source], &roles)).collect();
            let counts = built.examples.iter().map(|ex| (ex.task, ex.tokens, ex.target_tokens)).collect();
            Ok((lines, counts, built.dropped))
        });
        for (k, m) in made.into_iter().enumerate() {
            let (_, split) = chunk[k];
            match m {
                Some(Ok((lines, counts, drops))) => {
                    for (line, (task, total, target)) in lines.iter().zip(counts) {
                        shards.write(split, task, line)?;
                        sum.tally.entry((split, task)).or_default().add(total, target);
                    }
                    for (why, task) in drops {
                        *sum.example_drops.entry((task, why.to_string())).or_default() += 1;
                    }
                }
                Some(Err(_)) => *sum.example_drops.entry((Task::Describe, "import_failed".into())).or_default() += 1,
                None => *sum.example_drops.entry((Task::Describe, "panic".into())).or_default() += 1,
            }
        }
        done += chunk.len();
        let examples: u64 = sum.tally.values().map(|t| t.examples).sum();
        let toks: u64 = sum.tally.values().map(|t| t.tokens).sum();
        log(&format!("{done}/{} songs, {examples} examples, {:.1} M tokens ({:.0} s)", kept.len(), toks as f64 / 1e6, t0.elapsed().as_secs_f64()));
    }
    shards.finish()?;
    sum.seconds = t0.elapsed().as_secs_f64();

    // The song list, the manifest and the stats.
    let mut tsv = String::from("id\tsource\tlicence\tidentity\tfate\tquality\tbars\thash\tpath\n");
    for s in &songs {
        let fate = match &s.fate {
            Some(Fate::Kept(sp)) => sp.name().to_string(),
            Some(Fate::Dropped(w)) => format!("dropped:{w}"),
            None => "-".into(),
        };
        tsv.push_str(&format!("{}\t{}\t{}\t{}\t{fate}\t{}\t{}\t{}\t{}\n", s.r.id, datasets[s.r.source].id, s.r.licence, s.identity, s.r.quality.map_or("-".into(), |q| q.to_string()), s.bars, s.r.hash, s.r.path));
    }
    std::fs::write(opts.out.join("songs.tsv"), tsv).map_err(|e| e.to_string())?;
    std::fs::write(opts.out.join("manifest.json"), manifest(&datasets, tok, opts).to_json()).map_err(|e| e.to_string())?;
    std::fs::write(opts.out.join("stats.json"), stats_json(&sum).to_json()).map_err(|e| e.to_string())?;
    Ok(sum)
}

fn manifest(datasets: &[Dataset], tok: &Tokenizer, opts: &Opts) -> Value {
    let l = &opts.limits;
    obj(vec![
        ("format", s(FORMAT)),
        ("notice", s("Built on this machine from the libraries its owner downloaded and their own files. Training data for personal use: never redistribute it; the songs keep their owners' rights and every record names its source and licence.")),
        (
            "datasets",
            Value::Arr(datasets.iter().map(|d| obj(vec![("id", s(d.id.clone())), ("name", s(d.name.clone())), ("licence", s(d.licence.clone())), ("credit", s(d.credit.clone())), ("notice", s(d.notice.clone())), ("location", s(d.location.clone()))])).collect()),
        ),
        ("tokenizer", obj(vec![("file", s(tok.file.clone())), ("pieces", Value::Int(tok.pieces() as i64)), ("chat", s("qwen im_start/im_end, assistant turn opens with an empty think block"))])),
        ("context", Value::Int(l.context as i64)),
        ("split", obj(vec![("by", s("song identity: normalised artist|title, FNV-1a 64 % 1000")), ("val_per_mille", Value::Int(opts.val as i64)), ("test_per_mille", Value::Int(opts.test as i64))])),
        (
            "filter",
            obj(vec![
                ("min_quality", Value::Int(opts.min_quality as i64)),
                ("meters", Value::Arr(opts.meters.iter().cloned().map(s).collect())),
                ("min_bars", Value::Int(opts.min_bars as i64)),
                ("near_dup_jaccard", Value::F64(opts.near_dup)),
            ]),
        ),
        (
            "tasks",
            obj(vec![
                ("describe", obj(vec![("per_song", Value::Int(l.describe as i64)), ("bars", Value::Arr(l.describe_bars.iter().map(|b| Value::Int(*b as i64)).collect()))])),
                ("continue", obj(vec![("per_song", Value::Int(l.cont as i64)), ("bars", Value::Arr(l.cont_bars.iter().map(|(k, m)| s(format!("{k}+{m}"))).collect()))])),
                ("infill", obj(vec![("per_song", Value::Int(l.infill as i64)), ("bars", Value::Arr(l.infill_bars.iter().map(|b| Value::Int(*b as i64)).collect()))])),
                ("roll_to_score", obj(vec![("per_song", Value::Int(l.score as i64)), ("bars", Value::Arr(l.score_bars.iter().map(|b| Value::Int(*b as i64)).collect()))])),
                ("score_to_roll", obj(vec![("per_song", Value::Int(l.score as i64)), ("bars", Value::Arr(l.score_bars.iter().map(|b| Value::Int(*b as i64)).collect()))])),
                ("train_augment_copies", Value::Int(l.augment as i64)),
            ]),
        ),
    ])
}

fn stats_json(sum: &Summary) -> Value {
    let mut tasks_v = Vec::new();
    for split in Split::ALL {
        let mut per = Vec::new();
        for task in Task::ALL {
            let t = sum.tally.get(&(split, task)).cloned().unwrap_or_default();
            per.push((
                task.name().to_string(),
                obj(vec![
                    ("examples", Value::Int(t.examples as i64)),
                    ("tokens", Value::Int(t.tokens as i64)),
                    ("target", Value::Int(t.target as i64)),
                    ("p50", Value::Int(t.quantile(0.5) as i64)),
                    ("p90", Value::Int(t.quantile(0.9) as i64)),
                    ("max", Value::Int(t.quantile(1.0) as i64)),
                    ("histogram", Value::Arr(t.histogram().into_iter().map(|c| Value::Int(c as i64)).collect())),
                ]),
            ));
        }
        tasks_v.push((split.name().to_string(), Value::Obj(per)));
    }
    let mut drops: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();
    for ((task, why), n) in &sum.example_drops {
        drops.entry(task.name().to_string()).or_default().push((why.clone(), Value::Int(*n as i64)));
    }
    obj(vec![
        ("songs_in", Value::Int(sum.songs_in as i64)),
        ("songs_kept", Value::Int(sum.kept as i64)),
        ("songs_dropped", Value::Obj(sum.dropped.iter().map(|(k, v)| (k.clone(), Value::Int(*v as i64))).collect())),
        ("songs_per_split", Value::Obj(sum.songs_per_split.iter().map(|(k, v)| (k.name().to_string(), Value::Int(*v as i64))).collect())),
        ("examples_dropped", Value::Obj(drops.into_iter().map(|(k, v)| (k, Value::Obj(v))).collect())),
        ("histogram_edges", Value::Arr(tokens::EDGES.iter().map(|e| Value::Int(*e as i64)).collect())),
        ("tasks", Value::Obj(tasks_v)),
        ("seconds", Value::F64((sum.seconds * 10.0).round() / 10.0)),
    ])
}

/// The summary printed after a build or by `stats`.
pub fn print_stats(dir: &Path) -> Result<String, String> {
    let text = std::fs::read(dir.join("stats.json")).map_err(|e| format!("{}: {e}", dir.join("stats.json").display()))?;
    let v = makepad_strict_json::parse(&text).map_err(|e| format!("stats.json: {e}"))?;
    let int = |v: Option<&Value>| v.and_then(|x| x.as_i64()).unwrap_or(0);
    let mut w = String::new();
    w.push_str(&format!("songs: {} in, {} kept\n", int(v.get("songs_in")), int(v.get("songs_kept"))));
    if let Some(Value::Obj(d)) = v.get("songs_dropped") {
        for (k, n) in d {
            w.push_str(&format!("  dropped {k}: {}\n", n.as_i64().unwrap_or(0)));
        }
    }
    if let Some(Value::Obj(d)) = v.get("songs_per_split") {
        w.push_str(&format!("  split: {}\n", d.iter().map(|(k, n)| format!("{k} {}", n.as_i64().unwrap_or(0))).collect::<Vec<_>>().join(", ")));
    }
    let edges: Vec<i64> = v.get("histogram_edges").and_then(|e| e.as_arr()).map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default();
    w.push_str(&format!("{:<6} {:<14} {:>9} {:>12} {:>12} {:>6} {:>6} {:>6}  histogram (<= {})\n", "split", "task", "examples", "tokens", "target", "p50", "p90", "max", edges.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("/")));
    let mut grand = (0i64, 0i64, 0i64);
    if let Some(Value::Obj(splits)) = v.get("tasks") {
        for (split, tasks) in splits {
            let mut tot = (0i64, 0i64, 0i64);
            if let Value::Obj(tasks) = tasks {
                for (task, t) in tasks {
                    let hist: Vec<String> = t.get("histogram").and_then(|h| h.as_arr()).map(|a| a.iter().map(|x| x.as_i64().unwrap_or(0).to_string()).collect()).unwrap_or_default();
                    let (e, tk, tg) = (int(t.get("examples")), int(t.get("tokens")), int(t.get("target")));
                    tot = (tot.0 + e, tot.1 + tk, tot.2 + tg);
                    w.push_str(&format!("{split:<6} {task:<14} {e:>9} {tk:>12} {tg:>12} {:>6} {:>6} {:>6}  {}\n", int(t.get("p50")), int(t.get("p90")), int(t.get("max")), hist.join(" ")));
                }
            }
            w.push_str(&format!("{split:<6} {:<14} {:>9} {:>12} {:>12}\n", "all", tot.0, tot.1, tot.2));
            grand = (grand.0 + tot.0, grand.1 + tot.1, grand.2 + tot.2);
        }
    }
    w.push_str(&format!("total: {} examples, {:.1} M tokens ({:.1} M trained on)\n", grand.0, grand.1 as f64 / 1e6, grand.2 as f64 / 1e6));
    if let Some(Value::Obj(d)) = v.get("examples_dropped") {
        for (task, reasons) in d {
            if let Value::Obj(r) = reasons {
                w.push_str(&format!("  {task} windows dropped: {}\n", r.iter().map(|(k, n)| format!("{k} {}", n.as_i64().unwrap_or(0))).collect::<Vec<_>>().join(", ")));
            }
        }
    }
    Ok(w)
}
