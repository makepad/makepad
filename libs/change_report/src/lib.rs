//! Change reports: what someone changed in a Makepad app's source, told as
//! concepts (what changed and why), for sending to Makepad after they have
//! read it. The format is documented in `docs/agents/change-report.md`, with
//! its JSON schema beside it (`change-report.schema.json`).
//!
//! A report is a zip of at most [`MAX_ZIP_BYTES`] holding exactly
//! `report.json` (the list, checked here field by field), `REPORT.md` (the
//! same list for people; what the Builder shows before sending) and, only
//! when the person chose to include it, a trimmed `changes.diff`. Everything
//! is UTF-8 text. [`open`] checks a zip completely, the anonymisation scan
//! included, and is what both the Builder (before it sends) and the
//! server (when it receives) run; [`pack`] makes the zip from a folder
//! holding those files. Nothing here does I/O beyond reading that folder.

use makepad_strict_json::{self as json, Value};
use makepad_zip_file::{
    zip_read_central_directory, ZipMethod, ZipWriter, COMPRESS_METHOD_DEFLATED, COMPRESS_METHOD_UNCOMPRESSED,
};
use std::fs;
use std::io::Cursor;
use std::path::Path;

/// `report.json`'s "format" value, and its "version".
pub const FORMAT: &str = "makepad-change-report";
pub const VERSION: i64 = 1;

pub const REPORT_JSON: &str = "report.json";
pub const REPORT_MD: &str = "REPORT.md";
pub const CHANGES_DIFF: &str = "changes.diff";

/// The whole zip.
pub const MAX_ZIP_BYTES: usize = 256 * 1024;
/// Each file, unpacked.
pub const MAX_REPORT_JSON_BYTES: usize = 128 * 1024;
pub const MAX_REPORT_MD_BYTES: usize = 128 * 1024;
pub const MAX_DIFF_BYTES: usize = 1024 * 1024;

/// Changes in one report.
pub const MAX_CHANGES: usize = 100;
/// Characters in a change's fields.
pub const MAX_TITLE: usize = 120;
pub const MAX_WHAT: usize = 2000;
pub const MAX_WHY: usize = 1000;
pub const MAX_AREAS: usize = 16;
pub const MAX_AREA: usize = 80;
pub const MAX_SNIPPET: usize = 1500;
pub const MAX_SNIPPET_LINES: usize = 40;
pub const MAX_SUMMARY: usize = 2000;
pub const MAX_REDACTIONS: usize = 50;

/// What kind of change an item is.
pub const KINDS: &[&str] = &["fix", "feature", "tweak", "ui", "performance", "refactor", "docs", "other"];

/// Where reports go unless `MAKEPAD_FEEDBACK_URL` names the feedback
/// endpoint (reports then go to that URL + `/changes`).
pub const DEFAULT_ENDPOINT: &str = "https://makepad.nl/api/feedback/changes";
/// Marks the request as coming from a program, as app feedback does.
pub const FEEDBACK_HEADER: &str = "X-Makepad-Feedback";
/// The sender's address, only when they chose to include it.
pub const REPLY_TO_HEADER: &str = "X-Makepad-Reply-To";

/// The endpoint for a given `MAKEPAD_FEEDBACK_URL` value (None or empty:
/// the default).
pub fn endpoint(feedback_url: Option<&str>) -> String {
    match feedback_url.map(str::trim).filter(|url| !url.is_empty()) {
        Some(url) => format!("{}/changes", url.trim_end_matches('/')),
        None => DEFAULT_ENDPOINT.to_owned(),
    }
}

/// The report id from the server's answer, `{"ok":true,"id":N}`.
pub fn response_id(body: &[u8]) -> Result<i64, String> {
    let value = json::parse(body).map_err(|_| "The server's answer is not JSON".to_owned())?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    value.get("id").and_then(Value::as_i64).ok_or_else(|| "The server's answer has no report id".to_owned())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub language: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub title: String,
    pub kind: String,
    /// What changed, for a person using the app.
    pub what: String,
    /// Why it was changed.
    pub why: String,
    /// Screens, components or modules it touches, by name.
    pub areas: Vec<String>,
    pub snippet: Option<Snippet>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Redaction {
    /// What kind of thing was taken out ("email address", "file path").
    pub what: String,
    pub count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub app: String,
    /// The release the edits were made against.
    pub release: String,
    /// The commits of that release's repositories: (repository, commit).
    pub base: Vec<(String, String)>,
    pub summary: String,
    pub changes: Vec<Change>,
    pub redactions: Vec<Redaction>,
    /// Whether `changes.diff` is part of the report.
    pub diff: bool,
}

/// A checked report: its list, the text people read, the optional diff,
/// and the zip exactly as it is sent.
#[derive(Clone, Debug)]
pub struct Package {
    pub report: Report,
    pub markdown: String,
    pub diff: Option<String>,
    pub zip: Vec<u8>,
}

/// An app or release id: letters, digits, `-`, `_` and `.`, at most 128.
pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !value.starts_with('.')
}

fn problem<T>(message: impl Into<String>) -> Result<T, String> {
    Err(message.into())
}

/// Text of at most `max` characters, trimmed; `name` names it in errors.
fn text_field(value: Option<&Value>, name: &str, max: usize, required: bool) -> Result<String, String> {
    let text = match value {
        None | Some(Value::Null) if !required => return Ok(String::new()),
        None | Some(Value::Null) => return problem(format!("{name} is missing")),
        Some(value) => value.as_str().ok_or_else(|| format!("{name} must be text"))?,
    };
    let text = text.trim();
    if required && text.is_empty() {
        return problem(format!("{name} is empty"));
    }
    if text.chars().count() > max {
        return problem(format!("{name} is longer than {max} characters"));
    }
    if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')) {
        return problem(format!("{name} holds control characters"));
    }
    Ok(text.to_owned())
}

/// Only these keys: a report carries nothing its reader did not see listed.
fn known_keys(value: &Value, name: &str, keys: &[&str]) -> Result<(), String> {
    let Value::Obj(pairs) = value else { return problem(format!("{name} must be an object")) };
    match pairs.iter().find(|(key, _)| !keys.contains(&key.as_str())) {
        Some((key, _)) => problem(format!("{name} has an unknown field \"{key}\"")),
        None => Ok(()),
    }
}

/// Check `report.json` against the schema (version 1).
pub fn parse_report(bytes: &[u8]) -> Result<Report, String> {
    if bytes.len() > MAX_REPORT_JSON_BYTES {
        return problem("report.json is larger than 128 KB");
    }
    let value = json::parse(bytes).map_err(|error| format!("report.json is not valid JSON ({error})"))?;
    known_keys(&value, "report.json", &["format", "version", "app", "base", "summary", "changes", "redactions", "diff"])?;
    if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return problem(format!("report.json: \"format\" must be \"{FORMAT}\""));
    }
    if value.get("version").and_then(Value::as_i64) != Some(VERSION) {
        return problem(format!("report.json: \"version\" must be {VERSION}"));
    }
    let app = value.get("app").ok_or("report.json: \"app\" is missing")?;
    known_keys(app, "app", &["id", "release"])?;
    let id = text_field(app.get("id"), "app.id", 128, true)?;
    let release = text_field(app.get("release"), "app.release", 128, true)?;
    if !identifier(&id) || !identifier(&release) {
        return problem("app.id and app.release are identifiers (letters, digits, - _ .)");
    }
    let mut base = Vec::new();
    match value.get("base") {
        None | Some(Value::Null) => {}
        Some(list) => {
            let list = list.as_arr().ok_or("base must be a list")?;
            if list.len() > 8 {
                return problem("base lists at most 8 repositories");
            }
            for entry in list {
                known_keys(entry, "base entry", &["repository", "commit"])?;
                let repository = text_field(entry.get("repository"), "base.repository", 128, true)?;
                let commit = text_field(entry.get("commit"), "base.commit", 64, true)?;
                if !identifier(&repository) || commit.len() < 7 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return problem("base: a repository name and its commit (hexadecimal)");
                }
                base.push((repository, commit));
            }
        }
    }
    let summary = text_field(value.get("summary"), "summary", MAX_SUMMARY, false)?;
    let list = value.get("changes").and_then(Value::as_arr).ok_or("report.json: \"changes\" must be a list")?;
    if list.is_empty() {
        return problem("The report lists no changes");
    }
    if list.len() > MAX_CHANGES {
        return problem(format!("The report lists more than {MAX_CHANGES} changes"));
    }
    let mut changes = Vec::new();
    for (n, entry) in list.iter().enumerate() {
        let at = |field: &str| format!("changes[{n}].{field}");
        known_keys(entry, &format!("changes[{n}]"), &["title", "kind", "what", "why", "areas", "snippet"])?;
        let title = text_field(entry.get("title"), &at("title"), MAX_TITLE, true)?;
        if title.contains('\n') {
            return problem(format!("{} is one line", at("title")));
        }
        let kind = text_field(entry.get("kind"), &at("kind"), 32, true)?;
        if !KINDS.contains(&kind.as_str()) {
            return problem(format!("{} must be one of {}", at("kind"), KINDS.join(", ")));
        }
        let what = text_field(entry.get("what"), &at("what"), MAX_WHAT, true)?;
        let why = text_field(entry.get("why"), &at("why"), MAX_WHY, false)?;
        let mut areas = Vec::new();
        if let Some(list) = entry.get("areas").filter(|v| !v.is_null()) {
            let list = list.as_arr().ok_or_else(|| format!("{} must be a list", at("areas")))?;
            if list.len() > MAX_AREAS {
                return problem(format!("{} lists more than {MAX_AREAS} areas", at("areas")));
            }
            for area in list {
                let area = text_field(Some(area), &at("areas"), MAX_AREA, true)?;
                if area.contains('\n') {
                    return problem(format!("{} are one line each", at("areas")));
                }
                areas.push(area);
            }
        }
        let snippet = match entry.get("snippet") {
            None | Some(Value::Null) => None,
            Some(snippet) => {
                known_keys(snippet, &at("snippet"), &["language", "text"])?;
                let language = text_field(snippet.get("language"), &at("snippet.language"), 32, false)?;
                let text = text_field(snippet.get("text"), &at("snippet.text"), MAX_SNIPPET, true)?;
                if text.lines().count() > MAX_SNIPPET_LINES {
                    return problem(format!("{} is longer than {MAX_SNIPPET_LINES} lines", at("snippet.text")));
                }
                Some(Snippet { language, text })
            }
        };
        changes.push(Change { title, kind, what, why, areas, snippet });
    }
    let list = value.get("redactions").and_then(Value::as_arr).ok_or("report.json: \"redactions\" must be a list (empty when nothing was taken out)")?;
    if list.len() > MAX_REDACTIONS {
        return problem(format!("redactions lists more than {MAX_REDACTIONS} kinds"));
    }
    let mut redactions = Vec::new();
    for entry in list {
        known_keys(entry, "redactions entry", &["what", "count"])?;
        let what = text_field(entry.get("what"), "redactions.what", 120, true)?;
        let count = entry.get("count").and_then(Value::as_i64).filter(|n| (1..=100_000).contains(n)).ok_or("redactions.count must be a number from 1")?;
        redactions.push(Redaction { what, count });
    }
    let diff = value.get("diff").and_then(Value::as_bool).ok_or("report.json: \"diff\" must be true or false")?;
    Ok(Report { app: id, release, base, summary, changes, redactions, diff })
}

impl Report {
    /// The report as `report.json` (what [`parse_report`] reads back).
    pub fn to_json(&self) -> String {
        let text = |s: &str| json::s(s);
        let changes = self.changes.iter().map(|change| {
            let mut pairs = vec![("title", text(&change.title)), ("kind", text(&change.kind)), ("what", text(&change.what))];
            if !change.why.is_empty() {
                pairs.push(("why", text(&change.why)));
            }
            if !change.areas.is_empty() {
                pairs.push(("areas", Value::Arr(change.areas.iter().map(|a| text(a)).collect())));
            }
            if let Some(snippet) = &change.snippet {
                pairs.push(("snippet", json::obj(vec![("language", text(&snippet.language)), ("text", text(&snippet.text))])));
            }
            json::obj(pairs)
        });
        let mut pairs = vec![
            ("format", text(FORMAT)),
            ("version", Value::Int(VERSION)),
            ("app", json::obj(vec![("id", text(&self.app)), ("release", text(&self.release))])),
        ];
        if !self.base.is_empty() {
            pairs.push(("base", Value::Arr(self.base.iter().map(|(r, c)| json::obj(vec![("repository", text(r)), ("commit", text(c))])).collect())));
        }
        if !self.summary.is_empty() {
            pairs.push(("summary", text(&self.summary)));
        }
        pairs.push(("changes", Value::Arr(changes.collect())));
        pairs.push(("redactions", Value::Arr(self.redactions.iter().map(|r| json::obj(vec![("what", text(&r.what)), ("count", Value::Int(r.count))])).collect())));
        pairs.push(("diff", Value::Bool(self.diff)));
        json::obj(pairs).to_json()
    }

    /// The list as `REPORT.md`, in the layout the documentation shows. An
    /// agent may write its own; [`open`] only asks that every title is in it.
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# Changes to {} (release {})\n\n", self.app, self.release);
        if !self.summary.is_empty() {
            out.push_str(&format!("{}\n\n", self.summary));
        }
        for (n, change) in self.changes.iter().enumerate() {
            out.push_str(&format!("## {}. {} ({})\n\n{}\n\n", n + 1, change.title, change.kind, change.what));
            if !change.why.is_empty() {
                out.push_str(&format!("Why: {}\n\n", change.why));
            }
            if !change.areas.is_empty() {
                out.push_str(&format!("Areas: {}\n\n", change.areas.join(", ")));
            }
            if let Some(snippet) = &change.snippet {
                out.push_str(&format!("```{}\n{}\n```\n\n", snippet.language, snippet.text));
            }
        }
        out.push_str("## Taken out before sending\n\n");
        if self.redactions.is_empty() {
            out.push_str("Nothing needed taking out.\n");
        }
        for redaction in &self.redactions {
            out.push_str(&format!("- {} × {}\n", redaction.what, redaction.count));
        }
        out.push_str(if self.diff { "\nThe trimmed diff (changes.diff) is included.\n" } else { "\nNo code diff is included.\n" });
        out
    }
}

/// One thing the anonymisation scan found: in which file, on which line,
/// what kind of thing, and the text that matched (shown to the person
/// sending it, so they can find it; never logged).
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub what: &'static str,
    pub matched: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} line {}: {} ({})", self.file, self.line, self.what, self.matched)
    }
}

pub const EMAIL: &str = "email address";
pub const HOME_PATH: &str = "absolute or home folder path";
pub const SECRET: &str = "key, token or password";
pub const PRIVATE_HOST: &str = "private network address";
pub const URL_CREDENTIALS: &str = "credentials in a URL";

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

/// The run of `accept` characters around byte `at` of `line`: (start, end).
fn span(line: &str, at: usize, accept: impl Fn(char) -> bool) -> (usize, usize) {
    let start = line[..at].char_indices().rev().take_while(|(_, c)| accept(*c)).last().map_or(at, |(i, _)| i);
    let end = line[at..].char_indices().find(|(_, c)| !accept(*c)).map_or(line.len(), |(i, _)| at + i);
    (start, end)
}

/// Addresses nobody owns (RFC 2606), git's own user name, and Makepad's
/// public addresses, which appear in the sources themselves.
fn allowed_email(local: &str, domain: &str) -> bool {
    let domain = domain.to_ascii_lowercase();
    local == "git"
        || matches!(domain.as_str(), "example.com" | "example.org" | "example.net" | "makepad.nl")
        || [".example", ".invalid", ".test", ".localhost"].iter().any(|end| domain.ends_with(end))
}

fn emails(line: &str, found: &mut Vec<(&'static str, String)>) {
    let local_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');
    let domain_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    for (at, _) in line.match_indices('@') {
        let (start, _) = span(line, at, local_char);
        let end = line[at + 1..].char_indices().find(|(_, c)| !domain_char(*c)).map_or(line.len(), |(i, _)| at + 1 + i);
        let local = &line[start..at];
        let domain = line[at + 1..end].trim_end_matches('.');
        let tld = domain.rsplit('.').next().unwrap_or("");
        if local.is_empty() || !domain.contains('.') || tld.len() < 2 || !tld.chars().all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        if !allowed_email(local, domain) {
            found.push((EMAIL, format!("{local}@{domain}")));
        }
    }
}

fn home_paths(line: &str, found: &mut Vec<(&'static str, String)>) {
    let path_char = |c: char| !c.is_whitespace() && !matches!(c, '"' | '\'' | '`' | ')' | '(' | ',' | ';' | '<' | '>');
    for prefix in ["/Users/", "/home/", "/root/", "/Volumes/", "/mnt/", "/media/", "/private/var/", "/var/folders/", "/run/user/"] {
        for (at, _) in line.match_indices(prefix) {
            // A path begins the token (not `a/home/` inside a relative path).
            let before = line[..at].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')) {
                continue;
            }
            let rest = &line[at + prefix.len()..];
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).collect();
            if name.is_empty() || name == "Shared" {
                continue;
            }
            let (_, end) = span(line, at, path_char);
            found.push((HOME_PATH, line[at..end].to_owned()));
        }
    }
    // Windows: a drive letter and a folder (C:\Users\…, D:/work/…), and
    // network shares (\\server\share).
    let bytes = line.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        // `https://…` is not a drive: its letter is preceded by more
        // letters. In source the separator may be escaped (C:\\Users).
        let separators = bytes[i + 2..].iter().take(2).take_while(|b| matches!(b, b'\\' | b'/')).count();
        let drive = bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && separators > 0
            && bytes.get(i + 2 + separators).is_some_and(|b| b.is_ascii_alphanumeric())
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'));
        // A share: two backslashes, a server name, one backslash and more
        // (so the "\\n" of an escaped newline in source is not one).
        let share = bytes[i] == b'\\' && bytes[i + 1] == b'\\' && (i == 0 || bytes[i - 1] != b'\\') && {
            let name = bytes[i + 2..].iter().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.')).count();
            name > 0 && bytes.get(i + 2 + name) == Some(&b'\\') && bytes.get(i + 3 + name).is_some_and(|b| b.is_ascii_alphanumeric())
        };
        if drive || share {
            let (_, end) = span(line, i, path_char);
            found.push((HOME_PATH, line[i..end].to_owned()));
        }
    }
}

/// Token prefixes of well-known services, and the length the rest must reach.
const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("sk-", 20), ("sk_live_", 16), ("sk_test_", 16), ("rk_live_", 16), ("pk_live_", 16),
    ("ghp_", 30), ("gho_", 30), ("ghu_", 30), ("ghs_", 30), ("ghr_", 30), ("github_pat_", 30),
    ("glpat-", 20), ("xoxb-", 20), ("xoxp-", 20), ("xoxa-", 20), ("xapp-", 20),
    ("hf_", 30), ("npm_", 30), ("AKIA", 16), ("ASIA", 16), ("AIza", 30),
];

/// Names that hold secrets when a literal is assigned to them.
const SECRET_NAMES: &[&str] = &["api_key", "apikey", "api-key", "secret", "token", "password", "passwd", "credential", "private_key", "access_key", "auth_key"];

/// A placeholder, not a value: `[secret]`, `${VAR}`, `<token>`, `xxx`, `***`.
fn placeholder(value: &str) -> bool {
    value.starts_with('[') || value.starts_with("${") || value.starts_with('<') || value.starts_with('$') || value.starts_with('%')
        || value.chars().all(|c| matches!(c, 'x' | 'X' | '*' | '.' | '-' | '_'))
        || value.chars().all(|c| c.is_ascii_uppercase() || c == '_')
}

fn secrets(line: &str, found: &mut Vec<(&'static str, String)>) {
    if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
        found.push((SECRET, "-----BEGIN … PRIVATE KEY-----".into()));
    }
    for (prefix, length) in TOKEN_PREFIXES {
        for (at, _) in line.match_indices(prefix) {
            if line[..at].chars().next_back().is_some_and(token_char) {
                continue;
            }
            let rest: String = line[at + prefix.len()..].chars().take_while(|c| token_char(*c)).collect();
            if rest.len() >= *length {
                found.push((SECRET, format!("{prefix}{}…", &rest[..4.min(rest.len())])));
            }
        }
    }
    for (at, _) in line.match_indices("Bearer ") {
        let rest: String = line[at + 7..].chars().take_while(|c| token_char(*c) || matches!(c, '.' | '=' | '/' | '+')).collect();
        if rest.len() >= 20 && !placeholder(&rest) {
            found.push((SECRET, format!("Bearer {}…", &rest[..4])));
        }
    }
    // name = "literal" / name: "literal" / "name": "literal"
    let lower = line.to_ascii_lowercase();
    for name in SECRET_NAMES {
        for (at, _) in lower.match_indices(name) {
            let rest = &line[at + name.len()..];
            // The rest of the identifier, a closing quote, then = or :.
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_');
            let rest = rest.trim_start_matches(['"', '\'']).trim_start();
            let Some(rest) = rest.strip_prefix(['=', ':']) else { continue };
            let rest = rest.trim_start_matches(['=', ' ', '\t']);
            let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`')) else { continue };
            let value: String = rest[1..].chars().take_while(|c| *c != quote).collect();
            if value.len() >= 8 && !value.contains(' ') && !placeholder(&value) {
                found.push((SECRET, format!("{}=\"{}…\"", &line[at..at + name.len()], value.chars().take(3).collect::<String>())));
            }
        }
    }
}

fn private_ipv4(host: &str) -> bool {
    let parts: Vec<u8> = host.split('.').filter_map(|p| if p.len() <= 3 && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) { p.parse().ok() } else { None }).collect();
    if parts.len() != 4 || host.split('.').count() != 4 {
        return false;
    }
    matches!((parts[0], parts[1]), (10, _) | (192, 168) | (169, 254))
        || (parts[0] == 172 && (16..=31).contains(&parts[1]))
        || (parts[0] == 100 && (64..=127).contains(&parts[1]))
}

fn private_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    [".local", ".lan", ".internal", ".intranet", ".corp", ".home", ".localdomain", ".ts.net", ".home.arpa"].iter().any(|end| host.ends_with(end))
}

fn hosts(line: &str, found: &mut Vec<(&'static str, String)>) {
    for (at, _) in line.match_indices("://") {
        let rest = &line[at + 3..];
        let authority: String = rest.chars().take_while(|c| !matches!(c, '/' | '?' | '#' | ' ' | '"' | '\'' | '`' | ')' | '>')).collect();
        let host_port = match authority.rsplit_once('@') {
            Some((credentials, host)) => {
                if credentials.contains(':') && !credentials.ends_with(':') {
                    found.push((URL_CREDENTIALS, format!("…://{}:…@{host}", credentials.split(':').next().unwrap_or(""))));
                }
                host.to_owned()
            }
            None => authority,
        };
        let host = host_port.split(':').next().unwrap_or("");
        if private_ipv4(host) || private_name(host) {
            found.push((PRIVATE_HOST, host.to_owned()));
        }
    }
    // Private addresses outside URLs (ssh arch@10.0.0.2, a config value).
    for word in line.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        if private_ipv4(word) && !found.iter().any(|(_, m)| m == word) {
            found.push((PRIVATE_HOST, word.to_owned()));
        }
    }
}

/// The anonymisation scan of one text file: email addresses, absolute and
/// home folder paths, keys and tokens, credentials in URLs and private
/// network addresses. A safety net under the agent's own redaction, not a
/// replacement for it: names and personal details in prose cannot be found
/// this way.
pub fn scan(file: &str, text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let mut found = Vec::new();
        emails(line, &mut found);
        home_paths(line, &mut found);
        secrets(line, &mut found);
        hosts(line, &mut found);
        out.extend(found.into_iter().map(|(what, matched)| Finding { file: file.to_owned(), line: n + 1, what, matched }));
    }
    out
}

/// Text: UTF-8 without NUL or other control characters (tabs, newlines and
/// carriage returns allowed), so no binary content.
fn text_file(name: &str, bytes: Vec<u8>) -> Result<String, String> {
    let text = String::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8 text"))?;
    if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        return problem(format!("{name} holds binary data or control characters"));
    }
    Ok(text)
}

fn limit(name: &str) -> usize {
    match name {
        REPORT_JSON => MAX_REPORT_JSON_BYTES,
        REPORT_MD => MAX_REPORT_MD_BYTES,
        _ => MAX_DIFF_BYTES,
    }
}

/// Read and check a report zip: its size, exactly the allowed files as
/// UTF-8 text within their sizes, `report.json` against the schema,
/// `REPORT.md` naming every change, the diff present exactly when the
/// report says so and holding no binary patch, and the anonymisation scan
/// clean in all of them. The error lists every scan finding, one per line.
pub fn open(zip: &[u8]) -> Result<Package, String> {
    if zip.len() > MAX_ZIP_BYTES {
        return problem(format!("The report is {} KB; at most {} KB can be sent", zip.len().div_ceil(1024), MAX_ZIP_BYTES / 1024));
    }
    let mut cursor = Cursor::new(zip);
    let directory = zip_read_central_directory(&mut cursor).map_err(|_| "The report is not a zip file".to_owned())?;
    let mut files: Vec<(String, String)> = Vec::new();
    for header in &directory.file_headers {
        let name = header.file_name.as_str();
        if ![REPORT_JSON, REPORT_MD, CHANGES_DIFF].contains(&name) {
            return problem(format!("The report holds \"{name}\"; only {REPORT_JSON}, {REPORT_MD} and {CHANGES_DIFF} belong in it"));
        }
        if files.iter().any(|(known, _)| known == name) {
            return problem(format!("The report holds {name} twice"));
        }
        let size = header.uncompressed_size as usize;
        if size > limit(name) {
            return problem(format!("{name} is larger than {} KB", limit(name) / 1024));
        }
        if header.general_purpose_bit_flag & 1 != 0 {
            return problem(format!("{name} is encrypted"));
        }
        // The member's data follows its local header; the sizes come from
        // the central directory, so the output is bounded before inflating.
        cursor.set_position(header.relative_offset_of_local_header as u64);
        makepad_zip_file::LocalFileHeader::from_stream(&mut cursor).map_err(|_| format!("{name} is damaged"))?;
        let start = cursor.position() as usize;
        let data = zip.get(start..start + header.compressed_size as usize).ok_or_else(|| format!("{name} is damaged"))?;
        let bytes = match header.compression_method {
            COMPRESS_METHOD_UNCOMPRESSED if data.len() == size => data.to_vec(),
            COMPRESS_METHOD_DEFLATED => {
                let mut out = vec![0u8; size];
                match makepad_fast_inflate::deflate_decompress(data, &mut out) {
                    Ok((_, written)) if written == size => out,
                    _ => return problem(format!("{name} is damaged")),
                }
            }
            _ => return problem(format!("{name} uses a compression the report format does not")),
        };
        if makepad_fast_inflate::crc32(&bytes) != header.crc32 {
            return problem(format!("{name} is damaged (checksum)"));
        }
        files.push((name.to_owned(), text_file(name, bytes)?));
    }
    let file = |name: &str| files.iter().find(|(known, _)| known == name).map(|(_, text)| text.clone());
    let json_text = file(REPORT_JSON).ok_or("The report has no report.json")?;
    let markdown = file(REPORT_MD).ok_or("The report has no REPORT.md")?;
    let diff = file(CHANGES_DIFF);
    let report = parse_report(json_text.as_bytes())?;
    if markdown.trim().is_empty() {
        return problem("REPORT.md is empty");
    }
    if let Some(change) = report.changes.iter().find(|c| !markdown.contains(c.title.as_str())) {
        return problem(format!("REPORT.md does not list \"{}\"; it shows the same changes as report.json", change.title));
    }
    match (&diff, report.diff) {
        (Some(_), false) => return problem("changes.diff is included but report.json says \"diff\": false"),
        (None, true) => return problem("report.json says \"diff\": true but there is no changes.diff"),
        (Some(diff), true) if diff.contains("GIT binary patch") => return problem("changes.diff holds a binary patch; leave binary files out"),
        _ => {}
    }
    let mut findings = scan(REPORT_JSON, &json_text);
    findings.extend(scan(REPORT_MD, &markdown));
    if let Some(diff) = &diff {
        findings.extend(scan(CHANGES_DIFF, diff));
    }
    if !findings.is_empty() {
        let mut message = format!("The report still holds {} thing{} to take out:", findings.len(), if findings.len() == 1 { "" } else { "s" });
        for finding in &findings {
            message.push_str(&format!("\n  {finding}"));
        }
        return Err(message);
    }
    Ok(Package { report, markdown, diff, zip: zip.to_vec() })
}

/// Zip a report folder (`report.json`, `REPORT.md`, optionally
/// `changes.diff`; hidden files such as .DS_Store are ignored, anything
/// else is refused) and check the result with [`open`].
pub fn pack(folder: &Path) -> Result<Package, String> {
    let entries = fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if ![REPORT_JSON, REPORT_MD, CHANGES_DIFF].contains(&name.as_str()) || !entry.path().is_file() {
            return problem(format!("The report folder holds \"{name}\"; only {REPORT_JSON}, {REPORT_MD} and {CHANGES_DIFF} belong in it"));
        }
        names.push(name);
    }
    let mut zip = ZipWriter::new();
    for name in [REPORT_JSON, REPORT_MD, CHANGES_DIFF] {
        if !names.iter().any(|n| n == name) {
            continue;
        }
        let path = folder.join(name);
        let size = fs::metadata(&path).map_err(|e| format!("{name}: {e}"))?.len() as usize;
        if size > limit(name) {
            return problem(format!("{name} is larger than {} KB", limit(name) / 1024));
        }
        let bytes = fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
        zip.add(name, &bytes, ZipMethod::Deflate).map_err(|e| e.to_string())?;
    }
    open(&zip.finish().map_err(|e| e.to_string())?)
}

/// A report from a folder ([`pack`]) or a zip file ([`open`]).
pub fn load(path: &Path) -> Result<Package, String> {
    if path.is_dir() {
        return pack(path);
    }
    let size = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > MAX_ZIP_BYTES as u64 {
        return problem(format!("The report is {} KB; at most {} KB can be sent", size.div_ceil(1024), MAX_ZIP_BYTES / 1024));
    }
    open(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
}

#[cfg(test)]
mod tests;
