//! Machine diagnostics. Codes are an append-only public protocol; never reuse a code.
use std::sync::atomic::{AtomicBool, Ordering};
use relax_session::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity { Error, Warning, Note }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub message: String,
    pub replacement: Option<(String, u32, u32, u32, u32, String)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub phase: &'static str,
    pub notes: Vec<String>,
    pub suggestion: Option<Suggestion>,
    pub item: Option<String>,
}

pub const CODES: &[(&str, &str)] = &[
    ("RB0001", "borrow checking failure"),
    ("RC0001", "code generation failure"),
    ("RC0002", "internal compiler error"),
    ("RK0001", "link failure"),
    ("RL0001", "unsupported lowering operation"),
    ("RL0002", "unknown declarative prototype"),
    ("RL0003", "field lacks a derived trait implementation"),
    ("RM0001", "cannot find macro"),
    ("RM0002", "derive or attribute expansion failure"),
    ("RM0003", "macro expansion failure"),
    ("RP0001", "source parse failure"),
    ("RR0001", "unresolved import"),
    ("RR0002", "resolution failure"),
    ("RT0001", "type checking failure"),
    ("RT0002", "type mismatch"),
    ("RT0003", "type inference failure"),
    ("RT0004", "missing field"),
    ("RT0005", "missing associated item"),
    ("RT0006", "unresolved type or value"),
    ("RT0007", "unsatisfied trait bound"),
    ("RT0008", "invalid operator operand"),
    ("RT0009", "invalid pattern"),
    ("RT0010", "invalid control flow"),
    ("RT0011", "nonconstant array length"),
    ("RT0012", "unsupported expression"),
    ("RT0013", "value is not callable"),
    ("RU0001", "check setup or execution failure"),
];

pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""), '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"'); out
}

impl Diagnostic {
    pub fn new(phase: &'static str, file: impl Into<String>, line: u32, col: u32, message: impl Into<String>) -> Self {
        let mut message = message.into();
        let ecode = message.as_bytes().windows(5).enumerate().find(|(i,s)| s[0] == b'E' && s[1..].iter().all(u8::is_ascii_digit) && (*i == 0 || !message.as_bytes()[i-1].is_ascii_alphanumeric()) && message.as_bytes().get(i+5).map_or(true, |c| !c.is_ascii_alphanumeric())).map(|(i,_)|i);
        let code = if let Some(i) = ecode {
            let code = message[i..i+5].to_string();
            if i == 0 { message = message[5..].trim_start_matches(':').trim_start().to_string(); }
            code
        } else {
            match () {
                _ if message.contains("internal compiler error") => "RC0002",
                _ if message.starts_with("RR005 ") => "RL0002",
                _ if phase == "lower" && message.starts_with("derive(") && message.contains("field type") => "RL0003",
                _ if message.contains("unresolved import") => "RR0001",
                _ if message.contains("cannot find macro") => "RM0001",
                _ if message.contains("derive(") && message.contains("failed") => "RM0002",
                _ if phase == "typeck" && message.starts_with("mismatched types") => "RT0002",
                _ if phase == "typeck" && (message.contains("cannot infer") || message.contains("not fully known")) => "RT0003",
                _ if phase == "typeck" && message.starts_with("no field") => "RT0004",
                _ if phase == "typeck" && (message.starts_with("no associated item") || message.starts_with("no item") || message.starts_with("no such trait item")) => "RT0005",
                _ if phase == "typeck" && (message.contains("unresolved") || message.starts_with("cannot find") || message.starts_with("cannot resolve")) => "RT0006",
                _ if phase == "typeck" && (message.contains("no impl") || message.contains("no bound") || message.contains("impl")) => "RT0007",
                _ if phase == "typeck" && (["cannot negate", "cannot deref", "cannot index", "cannot compare", "no binary operator", "`?`"].iter().any(|p|message.starts_with(p))) => "RT0008",
                _ if phase == "typeck" && message.contains("pattern") => "RT0009",
                _ if phase == "typeck" && (message.contains("outside of a loop") || message.contains("`await`")) => "RT0010",
                _ if phase == "typeck" && message.starts_with("array repeat count") => "RT0011",
                _ if phase == "typeck" && message.starts_with("unsupported expression") => "RT0012",
                _ if phase == "typeck" && message.contains("not callable") => "RT0013",
                _ => match phase { "parse" => "RP0001", "resolve" => "RR0002", "macro" => "RM0003", "borrowck" => "RB0001", "lower" => "RL0001", "codegen" => "RC0001", "link" => "RK0001", "run" => "RU0001", _ => "RT0001" },
            }.to_string()
        };
        let mut notes = Vec::new();
        let mut suggestion = None;
        for marker in [" -- help: ", "; help: ", "\nhelp: ", " -- consider ", "; consider ", " -- import it: ", " -- it exists as ", " -- annotate", " -- add ", " -- use "] {
            if let Some(at) = message.find(marker) {
                let skip = if marker.contains("help:") { marker.len() } else if marker.starts_with(" -- ") { 4 } else { 2 };
                suggestion = Some(Suggestion { message: message[at + skip..].to_string(), replacement: None });
                message.truncate(at); break;
            }
        }
        if let Some(at) = message.find(" -- ") {
            notes.push(message[at+4..].to_string()); message.truncate(at);
        }
        if let Some(at) = message.find("\nnote: ") {
            notes.push(message[at + 7..].to_string()); message.truncate(at);
        }
        Self { severity: Severity::Error, code, message, file: file.into(), line, col, end_line: line, end_col: col, phase, notes, suggestion, item: None }
    }
    pub fn to_json(&self) -> String {
        let suggestion = self.suggestion.as_ref().map_or("null".to_string(), |s| {
            let replacement = s.replacement.as_ref().map_or("null".to_string(), |(f,l,c,el,ec,t)| format!("[{},{},{},{},{},{}]", quote(f), l,c,el,ec,quote(t)));
            format!("{{\"message\":{},\"replacement\":{}}}", quote(&s.message), replacement)
        });
        format!("{{\"severity\":{},\"code\":{},\"message\":{},\"file\":{},\"line\":{},\"col\":{},\"end_line\":{},\"end_col\":{},\"phase\":{},\"notes\":[{}],\"suggestion\":{}{}}}",
            quote(match self.severity { Severity::Error => "error", Severity::Warning => "warning", Severity::Note => "note" }), quote(&self.code), quote(&self.message), quote(&self.file), self.line,self.col,self.end_line,self.end_col,quote(self.phase),self.notes.iter().map(|n|quote(n)).collect::<Vec<_>>().join(","),suggestion,self.item.as_ref().map_or(String::new(), |item| format!(",\"item\":{}", quote(item))))
    }
}

// The check session records structured data alongside the existing human error stream. Keys
// select only errors retained after retries/root filtering; speculative errors are not emitted.
static ENABLED: AtomicBool = AtomicBool::new(false);
static RECORDS: Mutex<Option<std::collections::HashMap<String, Diagnostic>>> = Mutex::new(None);
pub fn enable() { EMITTED.store(0, Ordering::Relaxed); ENABLED.store(true, Ordering::Relaxed); *RECORDS.lock().unwrap() = Some(Default::default()); }
pub fn enabled() -> bool { ENABLED.load(Ordering::Relaxed) }
pub fn record(human: String, d: Diagnostic) -> String {
    if enabled() { RECORDS.lock().unwrap().get_or_insert_with(Default::default).insert(human.clone(), d); }
    human
}
pub fn recorded(human: &str) -> Option<Diagnostic> { RECORDS.lock().unwrap().as_ref()?.get(human).cloned() }

/// Only legacy string-only producers use this fallback. Scan separators, so paths may contain ':';
/// diagnostics without a location retain an unknown (zero) span until the caller supplies one.
pub fn from_legacy(text: &str, phase: &'static str) -> Diagnostic {
    if let Some(d) = recorded(text) { return d; }
    for (i, _) in text.match_indices(':') {
        let Some((line, rest)) = text[i+1..].split_once(':') else { continue };
        let Ok(line) = line.parse::<u32>() else { continue };
        if let Some((col, msg)) = rest.split_once(':') {
            if let Ok(col) = col.parse::<u32>() { return Diagnostic::new(phase, &text[..i], line, col, msg.trim_start()); }
        }
        return Diagnostic::new(phase, &text[..i], line, 0, rest.trim_start());
    }
    Diagnostic::new(phase, "", 0, 0, text)
}

/// Levenshtein distance, bounded to two edits, with deterministic lexical tie breaking.
pub fn closest<'a>(name: &str, names: impl Iterator<Item=&'a str>) -> Option<String> {
    let a: Vec<char> = name.chars().collect();
    if a.len() > 128 { return None; }
    names.filter_map(|s| {
        let b: Vec<char> = s.chars().collect();
        if a.len().abs_diff(b.len()) > 2 || s == name { return None; }
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, x) in a.iter().enumerate() {
            let mut prev = row[0]; row[0] = i+1;
            for (j,y) in b.iter().enumerate() {
                let old = row[j+1]; row[j+1] = (row[j]+1).min(old+1).min(prev + usize::from(x != y)); prev = old;
            }
            if *row.iter().min().unwrap() > 2 { return None; }
        }
        (row[b.len()] <= 2).then_some((row[b.len()], s))
    }).min().map(|(_,s)|s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn encoding() {
        assert_eq!(quote("a\"\\\n\t\r\0é"), "\"a\\\"\\\\\\n\\t\\r\\u0000é\"");
        let d = Diagnostic::new("typeck", "a.rs", 2, 3, "E0412: unknown");
        assert_eq!(d.to_json(), "{\"severity\":\"error\",\"code\":\"E0412\",\"message\":\"unknown\",\"file\":\"a.rs\",\"line\":2,\"col\":3,\"end_line\":2,\"end_col\":3,\"phase\":\"typeck\",\"notes\":[],\"suggestion\":null}");
    }
    #[test] fn code_table() {
        for pair in CODES.windows(2) { assert!(pair[0].0 < pair[1].0); }
        let titles: std::collections::HashSet<_> = CODES.iter().map(|x|x.1).collect();
        assert_eq!(titles.len(), CODES.len());
    }
    #[test] fn fallback_and_distance() {
        let d = from_legacy("C:/a.rs:12:7: E0425: missing: x", "typeck");
        assert_eq!((d.file.as_str(),d.line,d.col,d.code.as_str()), ("C:/a.rs",12,7,"E0425"));
        assert_eq!(closest("Thign", ["Thing", "Other"].into_iter()), Some("Thing".into()));
        assert_eq!(closest("abc", ["xyz"].into_iter()), None);
    }
}

static EMITTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn emit(d: &Diagnostic) {
    if d.severity == Severity::Error { EMITTED.fetch_add(1, Ordering::Relaxed); }
    println!("{}", d.to_json());
}
pub fn emitted_errors() -> usize { EMITTED.load(Ordering::Relaxed) }
