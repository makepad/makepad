//! `makepad inspect file=<x.mkpf> rec replay [until=<s>] [show=1] [home=<dir>] [dry=1]`: turns
//! "the person saw it once" into a repro. The flight recorder's recording holds how the run was
//! started (args, folder, MAKEPAD_* environment, home), the first window's size and scale, the
//! script random seed and every input event with the frame it arrived in; the replay starts the
//! same program the same way, hidden (MAKEPAD_HIDE_WINDOWS, muted) and from the same build and
//! cache (the same home), and the program plays the input back from its start, each event at its
//! recorded frame (MAKEPAD_INPUT_REPLAY). The instance stays up at the end for inspection.
//!
//! Exact only when the recording reaches back to the run's start (input before the oldest record
//! is lost: the answer says so) and the program's logic follows its frames, the input and the
//! seeds (time read from the wall clock, threads racing, the network are not replayed).

use makepad_prof_stream::Profile;

const SEP: char = '\u{1f}';

pub fn file_main(path: &str, words: &[String]) -> i32 {
    let get = |k: &str| words.iter().find_map(|w| w.strip_prefix(k).and_then(|v| v.strip_prefix('='))).map(str::to_string);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            println!("cannot read {path}: {e}");
            return 1;
        }
    };
    let p = match Profile::decode(&bytes) {
        Ok(p) => p,
        Err(e) => {
            println!("{path}: {e}");
            return 1;
        }
    };
    let ev = |cat: &str| p.events.iter().find(|e| p.s(e.cat) == cat);
    let Some(args) = ev("run.args").map(|e| e.text.split(SEP).map(str::to_string).collect::<Vec<_>>()) else {
        println!("{path} has no run record (not a flight recorder recording, or an older one): cannot replay\nmore: makepad inspect file={path} pf brief");
        return 1;
    };
    let cwd = ev("run.cwd").map(|e| e.text.clone()).unwrap_or_default();
    let env: Vec<(String, String)> = ev("run.env").map(|e| e.text.split(SEP).filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()).unwrap_or_default();
    let home = get("home").or_else(|| ev("run.home").map(|e| e.text.clone()));
    let base = if p.info.start_mono > 0 { p.info.start_mono } else { p.t_min };
    let until = get("until").and_then(|v| v.trim_end_matches('s').parse::<f64>().ok());
    let until_t = until.map(|s| base + (s * 1e9) as u64);
    let inputs: Vec<&makepad_prof_stream::model::Input> = p.inputs.iter().filter(|i| until_t.map_or(true, |u| i.t <= u)).collect();
    // what the recording cannot give back
    let mut notes: Vec<String> = Vec::new();
    let first_frame = p.frames.first().map_or(0, |f| f.id);
    let first_input_frame = p.inputs.first().map_or(0, |i| i.frame);
    if first_frame > 2 {
        notes.push(format!(
            "the recording starts at frame {} ({} s into the run): input before it is not in the ring, so the replay starts fresh and may diverge before that point",
            first_frame,
            makepad_prof_stream::text::sig(p.t_min.saturating_sub(base) as f64 / 1e9)
        ));
    }
    if let Some(d) = p.dropped.iter().map(|d| d.1).max().filter(|&n| n > 0) {
        notes.push(format!("{d} records were overwritten while recording"));
    }
    let injected = inputs.iter().filter(|i| i.injected).count();
    if injected > 0 {
        notes.push(format!("{injected} of the events were injected by makepad remote (an agent), replayed as recorded"));
    }
    let seed = ev("seed.script").map(|e| e.a).filter(|&s| s != 0);
    let window = p.events.iter().find(|e| p.s(e.cat) == "window.geom" && e.a == 0).or_else(|| ev("window.geom")).map(|e| e.text.clone());
    // the input file the program plays back: `<ms> <frame since start> <command>`
    let mut txt = String::from("# makepad input v1: <ms since record start> <drawn frame since start> <command> (flight recorder replay)\n");
    let t0 = inputs.first().map_or(0, |i| i.t);
    for i in &inputs {
        txt.push_str(&format!("{:.1} {} {}\n", i.t.saturating_sub(t0) as f64 / 1e6, i.frame, i.command));
    }
    let input_path = format!("{}.input.txt", path.trim_end_matches(".mkpf"));
    if let Err(e) = std::fs::write(&input_path, &txt) {
        println!("cannot write {input_path}: {e}");
        return 1;
    }
    let exe = args.first().cloned().unwrap_or_default();
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args[1..]);
    if !cwd.is_empty() {
        cmd.current_dir(&cwd);
    }
    for (k, v) in &env {
        // the replay's own settings below win; a recorded replay's replay settings do not carry
        if !matches!(k.as_str(), "MAKEPAD_INPUT_REPLAY" | "MAKEPAD_FLIGHT_SEED" | "MAKEPAD_WINDOW" | "MAKEPAD_HIDE_WINDOWS") {
            cmd.env(k, v);
        }
    }
    if let Some(h) = &home {
        cmd.env("MAKEPAD_RUN_HOME", h);
    }
    cmd.env("MAKEPAD_INPUT_REPLAY", &input_path);
    if let Some(s) = seed {
        cmd.env("MAKEPAD_FLIGHT_SEED", s.to_string());
    }
    if let Some(w) = &window {
        cmd.env("MAKEPAD_WINDOW", w);
    }
    if get("show").as_deref() != Some("1") {
        cmd.env("MAKEPAD_HIDE_WINDOWS", "1");
        cmd.env("SANDBOX_MUTE", "1");
    }
    let what = format!(
        "{} input events (frames {}..{}{}) into `{}`{}{}",
        inputs.len(),
        inputs.first().map_or(0, |i| i.frame),
        inputs.last().map_or(0, |i| i.frame),
        until.map_or(String::new(), |u| format!(", until {u} s")),
        args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
        seed.map_or(String::new(), |s| format!(", script seed {s}")),
        window.as_ref().map_or(String::new(), |w| format!(", window {w}"))
    );
    if get("dry").as_deref() == Some("1") {
        println!("would replay {what}");
        println!("command: {} {}", exe, args[1..].join(" "));
        println!("input: {input_path}");
        for n in &notes {
            println!("note: {n}");
        }
        return 0;
    }
    let log = format!("{}.replay.log", path.trim_end_matches(".mkpf"));
    match std::fs::File::create(&log) {
        Ok(f) => {
            if let Ok(f2) = f.try_clone() {
                cmd.stdout(f2);
            }
            cmd.stderr(f);
        }
        Err(_) => {
            cmd.stdout(std::process::Stdio::null());
        }
    }
    cmd.stdin(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            println!("replaying {what}: pid {} (hidden; it stays up after the last event)", child.id());
            println!("input: {input_path}  log: {log}");
            for n in &notes {
                println!("note: {n}");
            }
            if first_input_frame == 0 && inputs.is_empty() {
                println!("note: the recording has no input: the replay only starts the program");
            }
            println!("more: makepad inspect {} input status  makepad inspect {} events  makepad inspect {} rec save  makepad inspect {} dump", child.id(), child.id(), child.id(), child.id());
            0
        }
        Err(e) => {
            println!("cannot start {exe}: {e}");
            1
        }
    }
}
