//! Render a Score to a mix and stems and print its scorecard; with a
//! previous scorecard, say whether the new version is kept.
//!
//!     music_iter <song.splash> <out dir> [--prev <vNN.card.tsv>] [--lufs -10] [--no-stems]
//!
//! Writes `<out dir>/<name>.wav`, `<name>.card.tsv` and the stems as
//! `<out dir>/stems/<role>.wav` (only the latest stems are kept).
//!
//! The batch loop, headless (the SCORE page's Batch mode does the same):
//!
//!     music_iter batch "<prompt>" [--style a,b] [--seconds 30] [--n 5]
//!         writes and renders n songs at once into batches/bNNN
//!     music_iter recompose <batch dir>
//!         writes that batch's concept cards again into a new batch
//!     music_iter learn [<batch dir>]
//!         learns from the batch's ratings.tsv (default: the newest batch)

use makepad_score_live::RenderOpts;
use makepad_music_iter::{card, compare, render, render_song, Card};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    match args.first().map(String::as_str) {
        Some("batch") => return batch(&args, &flag),
        Some("learn") => return learn(args.get(1)),
        Some("recompose") => return recompose(args.get(1).expect("music_iter recompose <batch dir>")),
        _ => {}
    }
    let (Some(src_path), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("music_iter <song.splash> <out dir> [--prev <card.tsv>] [--lufs -10] [--no-stems]");
        std::process::exit(2);
    };
    let src = std::fs::read_to_string(src_path).expect("read the score");
    let out = Path::new(out);
    std::fs::create_dir_all(out.join("stems")).expect("out dir");
    let name = Path::new(src_path).file_stem().unwrap().to_string_lossy().trim_end_matches(".score").to_string();
    let song = match render_song(&src, &RenderOpts::default(), !args.iter().any(|a| a == "--no-stems")) {
        Ok(song) => song,
        Err(e) => {
            eprintln!("does not render: {e}");
            std::process::exit(1);
        }
    };
    for e in &song.mix.errors {
        eprintln!("loop error: {e:?}");
    }
    std::fs::write(out.join(format!("{name}.wav")), render::wav(song.mix.rate, &song.mix.left, &song.mix.right)).expect("write wav");
    for stem in &song.stems {
        std::fs::write(out.join("stems").join(format!("{}.wav", stem.role.name())), render::wav(song.mix.rate, &stem.left, &stem.right)).expect("write stem");
    }
    let opts = card::Opts { target_lufs: flag("--lufs").and_then(|v| v.parse().ok()).unwrap_or(-10.0) };
    let card = card::score(&song, &opts);
    std::fs::write(out.join(format!("{name}.card.tsv")), card.to_tsv()).expect("write card");
    print!("{}", card.report());
    if let Some(prev) = flag("--prev") {
        let prev = Card::from_tsv(&std::fs::read_to_string(prev).expect("read the previous card"));
        let v = compare(&prev, &card);
        println!("against the previous: {:+.1}; {}", v.delta, if v.keep { "KEEP".to_string() } else if v.regressions.is_empty() { "REJECT (no gain)".to_string() } else { format!("REJECT (regresses {})", v.regressions.iter().map(|(k, a, b)| format!("{k} {a:.0}->{b:.0}")).collect::<Vec<_>>().join(", ")) });
    }
}

fn recompose(dir: &str) {
    use makepad_music_iter::batch::Batch;
    use makepad_music_iter::gen::Concept;
    let old = Batch::open(Path::new(dir)).expect("no batch there");
    let cards: Vec<Concept> = old.songs.iter().filter(|e| !e.concept.is_empty()).map(|e| Concept { title: e.concept.lines().find_map(|l| l.strip_prefix("TITLE:")).unwrap_or("").trim().to_string(), card: e.concept.clone() }).collect();
    assert!(!cards.is_empty(), "the batch has no concept cards");
    let (batch, run) = Batch::start_with(std::sync::Arc::new(makepad_music_iter::gen::ClaudeCode::default()), old.request.clone(), cards.len(), Some(cards)).unwrap_or_else(|e| panic!("{e}"));
    follow(batch, run);
}

fn batch(args: &[String], flag: &dyn Fn(&str) -> Option<String>) {
    use makepad_music_iter::batch::Batch;
    use makepad_music_iter::gen::{ClaudeCode, Request};
    let request = Request {
        prompt: args.get(1).cloned().unwrap_or_default(),
        markers: flag("--style").map(|s| s.split(',').map(|m| m.trim().to_string()).collect()).unwrap_or_default(),
        seconds: flag("--seconds").and_then(|v| v.parse().ok()).unwrap_or(30),
    };
    let n = flag("--n").and_then(|v| v.parse().ok()).unwrap_or(5);
    let (batch, run) = Batch::start(std::sync::Arc::new(ClaudeCode::default()), request, n).unwrap_or_else(|e| panic!("{e}"));
    follow(batch, run);
}

fn follow(mut batch: makepad_music_iter::batch::Batch, mut run: makepad_music_iter::batch::Run) {
    use makepad_music_iter::batch::State;
    let n = batch.songs.len();
    println!("batch b{:03} in {}", batch.id, batch.dir.display());
    let mut shown = vec![State::Waiting; n];
    while !run.done() {
        run.poll(&mut batch);
        for (i, e) in batch.songs.iter().enumerate() {
            if e.state != shown[i] {
                shown[i] = e.state.clone();
                match &e.state {
                    State::Ready => println!("s{} ready: \"{}\" {:.0} s, card {}", i + 1, e.title, e.seconds, e.card.as_ref().map_or(String::new(), Card::compact)),
                    other => println!("s{}: {other:?}", i + 1),
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    batch.save_ratings();
    makepad_music_iter::batch::append_log(&batch, None);
}

fn learn(dir: Option<&String>) {
    use makepad_music_iter::batch::{self, Batch};
    let batch = match dir {
        Some(dir) => Batch::open(Path::new(dir)),
        None => Batch::latest(),
    }
    .expect("no batch");
    match batch::learn(&makepad_music_iter::gen::ClaudeCode::default(), &batch, &std::sync::atomic::AtomicBool::new(false)) {
        Ok(l) => {
            println!("{}", l.changes);
            for r in l.added() {
                println!("+ {r}");
            }
            for r in l.removed() {
                println!("- {r}");
            }
        }
        Err(e) => eprintln!("{e}"),
    }
}
