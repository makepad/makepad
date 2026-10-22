//! `makepad inspect <app> pf <query> [s=3] [save=<x.mkpf>] [base=<y.mkpf>]` and `makepad inspect
//! file=<x.mkpf> pf <query> [base=<y.mkpf>]`: the profiler's query engine (makepad-prof-stream
//! `ask`) for agents. Live, the client attaches to the app's `prof` stream for `s` seconds, builds
//! the profile in memory (and writes it to `save=` as a recording) and answers on it; on a file it
//! answers on the recording. `base=` names the baseline recording a `diff` compares against.

use makepad_prof_stream::{ask, Profile};

fn split(words: &[String]) -> (String, Vec<(String, String)>) {
    // the client's own options leave the query
    let mut q = Vec::new();
    let mut own = Vec::new();
    for w in words {
        match w.split_once('=') {
            Some((k, v)) if matches!(k, "s" | "save" | "base" | "hz" | "ev") => own.push((k.to_string(), v.to_string())),
            _ => q.push(w.clone()),
        }
    }
    (q.join(" "), own)
}

fn opt<'a>(own: &'a [(String, String)], k: &str) -> Option<&'a str> {
    own.iter().rev().find(|x| x.0 == k).map(|x| x.1.as_str())
}

fn load(path: &str) -> Result<Profile, String> {
    let b = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    Profile::decode(&b)
}

fn answer(p: &Profile, query: &str, own: &[(String, String)]) -> i32 {
    let base = match opt(own, "base") {
        Some(f) => match load(f) {
            Ok(b) => Some(b),
            Err(e) => {
                println!("{e}");
                return 1;
            }
        },
        None => None,
    };
    let q = if query.trim().is_empty() { "summary" } else { query };
    println!("{}", ask::ask(p, q, base.as_ref()).text());
    0
}

/// `file=<x.mkpf> pf <query>`.
pub fn file_main(path: &str, words: &[String]) -> i32 {
    let w: Vec<String> = words.iter().skip_while(|w| *w == "pf").cloned().collect();
    let (query, own) = split(&w);
    match load(path) {
        Ok(p) => answer(&p, &query, &own),
        Err(e) => {
            println!("{e}");
            1
        }
    }
}

/// `<app> pf <query>`: records the live stream for `s` seconds (default 3), then answers.
pub fn live_main(app: &super::client::App, words: &[String]) -> i32 {
    let (query, own) = split(words);
    let secs: f64 = opt(&own, "s").and_then(|v| v.parse().ok()).unwrap_or(3.0);
    let client = match makepad_remote::Client::connect(&app.socket) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("makepad inspect: {} (pid {}): {:?}", app.app, app.pid, e);
            return 2;
        }
    };
    // hz=N: the stream samples CPU stacks too (`top fn`); `top fn` alone asks for 1000 Hz
    let hz = opt(&own, "hz").map(str::to_string).or_else(|| query.split_whitespace().nth(1).filter(|w| *w == "fn" || *w == "samples").map(|_| "1000".to_string()));
    let open_args = match (hz, opt(&own, "ev")) {
        (Some(h), Some(e)) => format!("hz={} ev={}", h, e),
        (Some(h), None) => format!("hz={}", h),
        _ => String::new(),
    };
    let ch = match client.open("prof", &open_args) {
        Ok(c) => c,
        Err(e) => {
            println!("the app does not serve the profiler stream (an older build?): {e:?}");
            return 2;
        }
    };
    let mut p = Profile::new();
    let mut raw: Vec<u8> = Vec::new();
    let keep = opt(&own, "save").is_some();
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs.max(0.1));
    loop {
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match ch.recv_timeout(left) {
            Ok(makepad_remote::Event::Data(d)) => {
                if keep {
                    raw.extend_from_slice(&d);
                }
                p.feed(&d);
            }
            Ok(makepad_remote::Event::Closed(_, why)) => {
                eprintln!("makepad inspect: the stream closed: {why}");
                break;
            }
            Ok(_) => {}
            Err(true) => break,
            Err(false) => break,
        }
    }
    drop(ch);
    if let Some(path) = opt(&own, "save") {
        if let Err(e) = std::fs::write(path, &raw) {
            println!("cannot write {path}: {e}");
            return 1;
        }
        eprintln!("saved {} ({} bytes)", path, raw.len());
    }
    answer(&p, &query, &own)
}
