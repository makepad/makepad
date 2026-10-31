//! Making a panel with an agent: the one driver every host uses (Synth's
//! score, Stage's SCORE page, a Story scene, a Story cut).
//!
//! 1. The host lists what its document can bind ([`crate::listing`], or
//!    its value sites) and splits the lines into parts ([`split`]): each
//!    part is written by its own short conversation, at once.
//! 2. The host starts the conversations its own way (its model, its warm
//!    sessions) and hands their answers in as [`Piece`]s; [`Job`] gathers
//!    them and says how far they are ([`Job::progress`]).
//! 3. Each answer's top-level records are checked on their own
//!    ([`modules_of`]: a record that does not run is left out, the rest
//!    are kept), put together ([`assemble`]) and written ([`crate::with_let`]),
//!    one undo step.
//! 4. The panel's bindings are read back ([`bindings`]) and checked against
//!    the document ([`range_issues`]); what is wrong may go back to an agent
//!    once, the host's choice.
//!
//! Main-loop cost: a drain of each part's channel per pump; the
//! conversations run off the main thread.

use makepad_widgets::Cx;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// What a part's conversation sends.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// More of its answer.
    Text(String),
    /// Its answer is complete.
    Done,
    Failed(String),
}

/// One part being written.
pub struct Part {
    rx: Receiver<Piece>,
    pub text: String,
    pub done: bool,
    pub failed: Option<String>,
}

/// The parts of one make, as they come. Dropped, its writers stop.
pub struct Job {
    pub parts: Vec<Part>,
    pub started: f64,
    cancel: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One part's conversation: its text in, its answer out as it comes
/// (`piece`), until it ends (or `cancel` is set).
pub type Write = Arc<dyn Fn(String, &AtomicBool, &mut dyn FnMut(&str)) -> Result<(), String> + Send + Sync>;

impl Job {
    /// A make whose parts answer on these channels (in part order); the
    /// host's writers watch `cancel`.
    pub fn new(parts: Vec<Receiver<Piece>>, started: f64, cancel: Arc<AtomicBool>) -> Job {
        Job { parts: parts.into_iter().map(|rx| Part { rx, text: String::new(), done: false, failed: None }).collect(), started, cancel }
    }

    /// Each text written by `write` on a thread of its own, at once.
    pub fn spawn(texts: Vec<String>, started: f64, write: Write) -> Job {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut parts = Vec::new();
        for text in texts {
            let (tx, rx) = channel();
            let (write, cancel) = (write.clone(), cancel.clone());
            std::thread::spawn(move || {
                let mut piece = |t: &str| {
                    let _ = tx.send(Piece::Text(t.to_string()));
                };
                let end = match write(text, &cancel, &mut piece) {
                    Ok(()) => Piece::Done,
                    Err(e) => Piece::Failed(e),
                };
                let _ = tx.send(end);
            });
            parts.push(rx);
        }
        Job::new(parts, started, cancel)
    }

    /// Take what the parts sent since; whether every part is in.
    pub fn pump(&mut self) -> bool {
        for part in &mut self.parts {
            while let Ok(piece) = part.rx.try_recv() {
                match piece {
                    Piece::Text(t) => part.text.push_str(&t),
                    Piece::Done => part.done = true,
                    Piece::Failed(e) => {
                        part.failed = Some(e);
                        part.done = true;
                    }
                }
            }
            // A conversation that went away without a word failed.
            if !part.done && matches!(part.rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)) {
                part.failed = Some("the writer stopped".into());
                part.done = true;
            }
        }
        self.parts.iter().all(|p| p.done)
    }

    /// The parts in, of all, and the module titles written so far.
    pub fn progress(&self) -> (usize, usize, Vec<String>) {
        let done = self.parts.iter().filter(|p| p.done).count();
        let titles = self.parts.iter().flat_map(|p| titles(&p.text)).collect();
        (done, self.parts.len(), titles)
    }

    /// Every part's records that run, in part order, and what was left out
    /// (a part that failed, a record that does not run). `first`: the
    /// first word a panel record starts with (`Panel`, `SynthTweaks`).
    pub fn modules(&self, cx: &mut Cx, first: &str) -> (Vec<String>, Vec<String>) {
        let (mut kept, mut dropped) = (Vec::new(), Vec::new());
        for part in &self.parts {
            if let Some(e) = &part.failed {
                dropped.push(e.clone());
                continue;
            }
            let (k, d) = modules_of(cx, &part.text, first);
            kept.extend(k);
            dropped.extend(d);
        }
        (kept, dropped)
    }
}

/// The `title: "…"`s of a text, in order.
pub fn titles(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("title: \"") {
        rest = &rest[at + 8..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
        }
    }
    out
}

/// Groups (each a block of lines: an instrument's paths) put into at most
/// `k` parts of about the same size, a group never split.
pub fn split_groups(groups: &[String], k: usize) -> Vec<String> {
    let k = k.max(1).min(groups.len().max(1));
    let total: usize = groups.iter().map(|g| g.len()).sum();
    let target = total.div_ceil(k);
    let mut parts = vec![String::new()];
    for g in groups {
        if parts.last().is_some_and(|p| p.len() >= target) && parts.len() < k {
            parts.push(String::new());
        }
        parts.last_mut().expect("one part at least").push_str(g);
    }
    parts
}

/// Lines split into at most `k` parts of about the same size, a new part
/// starting only where `group` (a line's group: its node, its `let`)
/// changes.
pub fn split(lines: &[&str], group: &dyn Fn(&str) -> String, k: usize) -> Vec<String> {
    let k = k.max(1).min(lines.len().max(1));
    let total: usize = lines.iter().map(|l| l.len() + 1).sum();
    let target = total.div_ceil(k);
    let mut parts = vec![String::new()];
    let mut last = None;
    for line in lines {
        let g = group(line);
        if parts.last().is_some_and(|p| p.len() >= target) && last.as_ref() != Some(&g) && parts.len() < k {
            parts.push(String::new());
        }
        let part = parts.last_mut().expect("one part at least");
        part.push_str(line);
        part.push('\n');
        last = Some(g);
    }
    parts
}

/// How many writers for `paths` bindable paths: one per `per_part`, within `range`.
pub fn part_count(paths: usize, per_part: usize, range: (usize, usize)) -> usize {
    paths.div_ceil(per_part.max(1)).clamp(range.0, range.1)
}

/// The line a person's brief adds to every part.
pub fn steer(brief: Option<&str>) -> String {
    brief.map(str::trim).filter(|b| !b.is_empty()).map(|b| format!("\nThe person asks for this panel: {b}. Every part follows it.")).unwrap_or_default()
}

/// The top-level records of an answer that run, each checked on its own,
/// and the ones left out (with why). Code fences and words before the
/// first record are not the panel's.
pub fn modules_of(cx: &mut Cx, text: &str, first: &str) -> (Vec<String>, Vec<String>) {
    let body: String = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect::<Vec<_>>().join("\n");
    let Some(at) = body.find(first) else { return (Vec::new(), Vec::new()) };
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    for record in records(body[at..].trim()) {
        let head: String = record.lines().next().unwrap_or("").trim().chars().take(60).collect();
        match makepad_widgets::splash::validate_splash_body(cx, record, false).first() {
            Some(e) => dropped.push(format!("{head} ({e})")),
            None => kept.push(record.to_string()),
        }
    }
    (kept, dropped)
}

/// The top-level records of a panel's text (`PanelModule{…}`, `PanelHeader{}`).
pub fn records(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut quoted) = (0i32, None, false);
    let mut prev = '\0';
    for (i, c) in body.char_indices() {
        match c {
            '"' if prev != '\\' => quoted = !quoted,
            '{' if !quoted => {
                if depth == 0 && start.is_none() {
                    start = Some(body[..i].rfind(|c: char| c.is_whitespace()).map_or(0, |w| w + 1));
                }
                depth += 1;
            }
            '}' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start.take() {
                        out.push(body[s..=i].trim());
                    }
                }
            }
            _ => {}
        }
        prev = c;
    }
    out
}

/// The whole `let <name> = fn() { View{…} }`: `head` (fixed widgets, a line
/// each) then the modules.
pub fn assemble(name: &str, head: &[&str], modules: &[String]) -> String {
    let mut body: Vec<String> = head.iter().map(|h| format!("        {h}")).collect();
    body.extend(modules.iter().map(|m| m.lines().map(|l| format!("        {l}")).collect::<Vec<_>>().join("\n")));
    format!("let {name} = fn() {{\n    View{{\n        flow: Down width: Fill spacing: 10\n{}\n    }}\n}}", body.join("\n"))
}

/// A control's binding as written on its line.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// The widget (`PanelSlider`).
    pub widget: String,
    pub path: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// A macro's target (`targets: "PATH from to, …"`) rather than a control's `path:`.
    pub target: bool,
    pub when: Option<String>,
}

/// `name: "text"` or `name: word` on a widget's line.
pub fn field(line: &str, name: &str) -> Option<String> {
    let at = line.find(&format!("{name}: "))? + name.len() + 2;
    let rest = &line[at..];
    Some(match rest.strip_prefix('"') {
        Some(quoted) => quoted[..quoted.find('"')?].to_string(),
        None => rest.split([' ', '}']).next()?.to_string(),
    })
}

/// A number as a panel or a document writes it (`0.5`, `-3`, `120ms`).
pub fn number(text: &str) -> Option<f64> {
    let t = text.trim();
    let (sign, t) = match t.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, t),
    };
    t.split(|c: char| !(c.is_ascii_digit() || c == '.')).next().filter(|d| !d.is_empty()).and_then(|d| d.parse::<f64>().ok()).map(|v| sign * v)
}

/// Every binding of a panel's body, line by line (a macro's targets each
/// one binding; a target's path may hold spaces: `a + b:X 0 1`).
pub fn bindings(body: &str) -> Vec<Binding> {
    let mut out = Vec::new();
    for line in body.lines().map(str::trim) {
        let widget = line.split('{').next().unwrap_or("").trim().to_string();
        let when = field(line, "when").filter(|w| !w.trim().is_empty());
        if let Some(targets) = field(line, "targets") {
            for target in targets.split(',') {
                let words: Vec<&str> = target.split_whitespace().collect();
                let (path, min, max) = match words.len() {
                    0 => continue,
                    1 | 2 => (words[0].to_string(), None, None),
                    n => (words[..n - 2].join(" "), number(words[n - 2]), number(words[n - 1])),
                };
                out.push(Binding { widget: widget.clone(), path, min, max, target: true, when: when.clone() });
            }
            continue;
        }
        if let Some(path) = field(line, "path") {
            let min = field(line, "min").as_deref().and_then(number);
            let max = field(line, "max").as_deref().and_then(number);
            out.push(Binding { widget, path, min, max, target: false, when });
        }
    }
    out
}

/// What is wrong with a panel's bindings against its document: a path
/// that does not resolve, a range that does not hold the value written
/// there. `value`: a path's value as written (a number, or `None`), or why
/// it does not resolve.
pub fn range_issues(bindings: &[Binding], value: &dyn Fn(&str) -> Result<Option<f64>, String>) -> Vec<String> {
    let mut issues = Vec::new();
    for b in bindings {
        match value(&b.path) {
            Err(why) => issues.push(format!("{} `{}`: {why}", b.widget, b.path)),
            Ok(Some(v)) if !b.target => {
                if let (Some(min), Some(max)) = (b.min, b.max) {
                    if v < min.min(max) || v > max.max(min) {
                        issues.push(format!("{} `{}`: its range {min}..{max} does not hold the value written there, {v} (a control shows and sets the value itself, not an offset)", b.widget, b.path));
                    }
                }
            }
            Ok(_) => {}
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_at_group_changes() {
        let lines = ["a.x", "a.y", "b.x", "b.y", "c.x", "c.y"];
        let group = |l: &str| l.split('.').next().unwrap_or("").to_string();
        let parts = split(&lines, &group, 3);
        assert_eq!(parts, vec!["a.x\na.y\n", "b.x\nb.y\n", "c.x\nc.y\n"]);
        assert_eq!(split(&lines, &group, 1).len(), 1);
        assert_eq!(part_count(40, 16, (3, 6)), 3);
        assert_eq!(part_count(200, 16, (3, 6)), 6);
        let groups: Vec<String> = ["a\na\n", "b\n", "c\nc\nc\n"].iter().map(|g| g.to_string()).collect();
        assert_eq!(split_groups(&groups, 2), vec!["a\na\nb\n", "c\nc\nc\n"]);
    }

    #[test]
    fn records_and_titles_are_read() {
        let text = "PanelModule{title: \"Mood\" a: \"}\"}\nPanelHeader{}";
        assert_eq!(records(text), vec!["PanelModule{title: \"Mood\" a: \"}\"}", "PanelHeader{}"]);
        assert_eq!(titles(text), vec!["Mood"]);
    }

    #[test]
    fn bindings_and_their_issues() {
        let body = "PanelSlider{path: \"A.x\" min: 0 max: 1}\nPanelMacro{label: \"E\" targets: \"A.x 0 2, p + q:B 1 3\"}\nPanelSlider{path: \"A.gone\"}\nPanelSlider{path: \"A.y\" min: 0 max: 1 when: \"chorus\"}";
        let b = bindings(body);
        assert_eq!(b.len(), 5);
        assert_eq!(b[2].path, "p + q:B");
        assert_eq!((b[2].min, b[2].max), (Some(1.0), Some(3.0)));
        assert_eq!(b[4].when.as_deref(), Some("chorus"));
        let issues = range_issues(&b, &|p| match p {
            "A.x" => Ok(Some(0.5)),
            "A.y" => Ok(Some(4.0)),
            "p + q:B" => Ok(None),
            _ => Err("no such value".into()),
        });
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].contains("A.gone"));
        assert!(issues[1].contains("A.y"));
        assert_eq!(number("-120ms"), Some(-120.0));
    }

    #[test]
    fn a_panel_is_assembled() {
        let code = assemble("cut_panel", &["PanelHeader{}"], &["PanelModule{title: \"A\"\n    PanelSlider{path: \"x\"}\n}".into()]);
        assert!(code.starts_with("let cut_panel = fn() {\n    View{\n"));
        assert!(code.contains("        PanelHeader{}\n        PanelModule{title: \"A\"\n            PanelSlider"));
    }
}
