// LIVEICE: the edited file. run.py first saves a version whose `parse` calls `first_end` before
// it exists (refused, E0425), then the version that adds `first_end` above `parse` (expression
// ids of everything below shift). `handle` and `run` are not edited and run from the start; the
// closures nested in handle's `or_else` closure are first built after the edits, on the worker
// thread, as in the user's Amp (its writer thread was inside next_song during the edits).
use std::sync::mpsc::Receiver;

pub fn parse(answer: &str) -> Result<(String, usize), String> {
    let start = answer.find('{').ok_or("no object")?;
    let end = answer.rfind('}').ok_or("no object")?;
    let body = &answer[start + 1..end];
    let text = |key: &str| body.split(',').find_map(|kv| kv.split_once(':').filter(|(k, _)| k.trim().trim_matches('"') == key).map(|(_, v)| v.trim().trim_matches('"').to_string()));
    let number = |key: &str| match text(key).map(|v| v.parse::<u32>()) {
        Some(Ok(n)) => Some(n),
        _ => None,
    };
    let title = text("title").filter(|t| !t.is_empty()).ok_or("no title")?;
    let steps = number("steps").map(|s| s as usize).unwrap_or(0);
    Ok((title, end + steps))
}

fn handle(n: u64, go: bool, cancelled: &dyn Fn() -> bool) -> Result<Option<String>, String> {
    if cancelled() {
        return Err("cancelled".into());
    }
    // the closure given to or_else and the ones nested in it first run once `go` exists
    let answer = (!go).then(String::new).or_else(|| {
        Some(format!("x {{\"title\": \"T{}\", \"steps\": 8}} {{\"title\": \"Z\"}}", n % 3))
            .filter(|a| a.contains('{'))
            .map(|a| a.trim().to_string())
    });
    let answer = answer.ok_or_else(|| "no answer".to_string())?;
    if !go {
        return Ok(None);
    }
    let (title, end) = parse(&answer).map_err(|e| format!("parse: {e}"))?;
    let label = Some(title).map(|t| format!("{t}:{end}")).filter(|t| !t.is_empty());
    let mut seen = Vec::new();
    let mut note = |s: &str| seen.push(s.len());
    note(&answer);
    Ok(Some(label.unwrap_or_default() + &"".repeat(seen.len())))
}

pub fn run(rx: Receiver<(u64, bool)>) {
    while let Ok((n, go)) = rx.recv() {
        let at = n;
        let cancelled = || at == u64::MAX;
        match handle(n, go, &cancelled) {
            Ok(Some(s)) => println!("out song {} {}", n, s),
            Ok(None) => {}
            Err(e) => println!("out failed {} {}", n, e),
        }
    }
}
