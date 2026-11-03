//! The JIT's error channel, shared by the loader and devhub. A recovery is another
//! record for the same load, never another hot load or an application test failure.

use crate::json_str;
use makepad_strict_json::Value;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Verified,
    Refused,
    Reverted,
    Panic,
    StaleFrame,
    Restarted,
    RestartFailed,
    FirstFrameTimeout,
    NoWindow,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Refused => "refused",
            Self::Reverted => "reverted",
            Self::Panic => "panic",
            Self::StaleFrame => "stale_frame",
            Self::Restarted => "restarted",
            Self::RestartFailed => "restart_failed",
            Self::FirstFrameTimeout => "first_frame_timeout",
            Self::NoWindow => "no_window",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "verified" => Self::Verified,
            "refused" => Self::Refused,
            "reverted" => Self::Reverted,
            "panic" => Self::Panic,
            "stale_frame" => Self::StaleFrame,
            "restarted" => Self::Restarted,
            "restart_failed" => Self::RestartFailed,
            "first_frame_timeout" => Self::FirstFrameTimeout,
            "no_window" => Self::NoWindow,
            _ => return None,
        })
    }

    pub fn failure(self) -> bool {
        matches!(self, Self::Refused | Self::Reverted | Self::Panic | Self::StaleFrame)
    }
}

/// One changed source unit. Paths are relative to the source workspace. Keeping
/// whole changed files makes derives and imports reproducible without a tree dump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceUnit {
    pub path: String,
    pub before: String,
    pub after: String,
}

impl SourceUnit {
    /// One exact replacement hunk, with unchanged prefix and suffix omitted.
    pub fn diff(&self) -> String {
        let a: Vec<_> = self.before.split_inclusive('\n').collect();
        let b: Vec<_> = self.after.split_inclusive('\n').collect();
        let start = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
        let end = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(a, b)| a == b).count();
        let (removed, added) = (a.len() - start - end, b.len() - start - end);
        let mut out = format!("--- a/{}\n+++ b/{}\n@@ -{},{} +{},{} @@\n", self.path, self.path,
            start + usize::from(removed != 0), removed, start + usize::from(added != 0), added);
        for (prefix, lines) in [('-', &a[start..a.len() - end]), ('+', &b[start..b.len() - end])] {
            for line in lines {
                out.push(prefix);
                out.push_str(line);
                if !line.ends_with('\n') { out.push_str("\n\\ No newline at end of file\n"); }
            }
        }
        out
    }

    fn json(&self) -> String {
        format!("{{\"path\":{},\"before\":{},\"after\":{},\"diff\":{}}}", json_str(&self.path), json_str(&self.before), json_str(&self.after), json_str(&self.diff()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: String,
    pub load: String,
    pub at_ms: u64,
    pub app: String,
    pub node: String,
    pub pid: u32,
    pub outcome: Outcome,
    pub reason: String,
    /// The failure this restart settles (empty for a load verdict).
    pub failure: String,
    pub units: Vec<SourceUnit>,
    pub logs: Vec<String>,
}

fn string(v: &Value, key: &str) -> Option<String> { Some(v.get(key)?.as_str()?.to_string()) }

impl Record {
    pub fn to_json(&self) -> String {
        format!("{{\"schema\":1,\"channel\":\"jit\",\"id\":{},\"load\":{},\"at_ms\":{},\"app\":{},\"node\":{},\"pid\":{},\"outcome\":{},\"reason\":{},\"cause\":{},\"failure\":{},\"units\":[{}],\"logs\":[{}]}}",
            json_str(&self.id), json_str(&self.load), self.at_ms, json_str(&self.app), json_str(&self.node), self.pid,
            json_str(self.outcome.name()), json_str(&self.reason), json_str(&cause_key(&self.reason)), json_str(&self.failure),
            self.units.iter().map(SourceUnit::json).collect::<Vec<_>>().join(","), self.logs.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","))
    }

    pub fn parse(json: &str) -> Option<Self> {
        let v = makepad_strict_json::parse(json.as_bytes()).ok()?;
        if v.get("schema")?.as_u64()? != 1 || v.get("channel")?.as_str()? != "jit" { return None; }
        Some(Self {
            id: string(&v, "id")?, load: string(&v, "load")?, at_ms: v.get("at_ms")?.as_u64()?,
            app: string(&v, "app")?, node: string(&v, "node")?, pid: v.get("pid")?.as_u64()?.try_into().ok()?,
            outcome: Outcome::parse(v.get("outcome")?.as_str()?)?, reason: string(&v, "reason")?, failure: string(&v, "failure")?,
            units: v.get("units")?.as_arr()?.iter().map(|u| Some(SourceUnit { path: string(u, "path")?, before: string(u, "before")?, after: string(u, "after")? })).collect::<Option<_>>()?,
            logs: v.get("logs")?.as_arr()?.iter().map(|s| s.as_str().map(str::to_string)).collect::<Option<_>>()?,
        })
    }
}

/// Strip locations, addresses and numeric instance identities, preserving names
/// and error wording. This is normalization of diagnostic data, not dispatch.
pub fn cause_key(reason: &str) -> String {
    reason.split_whitespace().map(|word| {
        let bare = word.trim_matches(['`', '\'', '"', '(', ')', ',', ';']);
        if bare.contains('/') || bare.contains('\\') || bare.contains(".rs:") || bare.ends_with(".rs") {
            return "<source>".to_string();
        }
        if bare.starts_with("0x") && bare[2..].chars().all(|c| c.is_ascii_hexdigit()) {
            return "<address>".to_string();
        }
        let mut out = String::new();
        let mut number = false;
        for c in word.chars() {
            if c.is_ascii_digit() {
                if !number { out.push('#'); }
                number = true;
            } else {
                number = false;
                out.extend(c.to_lowercase());
            }
        }
        out
    }).collect::<Vec<_>>().join(" ")
}

/// An infrastructure result has its own exit code; it must not become a failed
/// test assertion or a successful application run.
pub const JIT_ERROR_EXIT: i32 = 86;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Tests { code: i32 },
    JitError { record: String },
}

impl Verdict {
    /// The one text boundary: node commands expose their result through output
    /// and an exit code. Dispatch elsewhere uses the typed verdict.
    pub fn from_output(code: i32, output: &str) -> Option<Self> {
        if code != JIT_ERROR_EXIT { return Some(Self::Tests { code }); }
        let record = output.lines().rev().find_map(|line| line.strip_prefix("verdict: jit-error (not your change) record="))?;
        if record.is_empty() || record.chars().any(char::is_whitespace) { return None; }
        Some(Self::JitError { record: record.into() })
    }

    pub fn line(&self) -> String {
        match self {
            Self::Tests { code: 0 } => "verdict: pass".into(),
            Self::Tests { .. } => "verdict: fail".into(),
            Self::JitError { record } => format!("verdict: jit-error (not your change) record={record}"),
        }
    }
    pub fn code(&self) -> i32 {
        match self { Self::Tests { code } => *code, Self::JitError { .. } => JIT_ERROR_EXIT }
    }
}

pub fn home() -> PathBuf { makepad_network::fleet::makepad_home().join("devhub") }
pub fn outbox() -> PathBuf { home().join("hot-outbox") }

/// Worker-only durable handoff. Devhub imports each file into its ledger before
/// removing it, including records left by a short-lived test or an exec restart.
pub fn persist(record: &Record) -> std::io::Result<()> {
    use std::io::Write;
    if !record.id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid hot record id"));
    }
    let dir = outbox();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", record.id));
    let temp = dir.join(format!("{}.new", record.id));
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(record.to_json().as_bytes())?;
    file.sync_all()?;
    std::fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causes_ignore_locations_and_instances() {
        assert_eq!(cause_key("fn `fmt` removed at src/lib.rs:1:1; domain 12"), cause_key("fn `fmt` removed at other/derive.rs:902:4; domain 98"));
        assert_ne!(cause_key("fn `fmt` removed"), cause_key("impl `` removed"));
        assert_eq!(cause_key("fault at 0x1a2b"), cause_key("fault at 0xfeed"));
        assert_eq!(cause_key("failed at lib.rs:1:1"), cause_key("failed at lib.rs:234:90"));
    }

    #[test]
    fn infrastructure_verdict_is_distinct_from_test_failure() {
        let v = Verdict::JitError { record: "hot.7.2".into() };
        assert_eq!(v.line(), "verdict: jit-error (not your change) record=hot.7.2");
        assert_eq!(v.code(), JIT_ERROR_EXIT);
        assert_eq!(Verdict::from_output(JIT_ERROR_EXIT, &v.line()), Some(v.clone()));
        assert_eq!(Verdict::from_output(JIT_ERROR_EXIT, "test result: FAILED"), None);
        assert_eq!(Verdict::from_output(0, &v.line()), Some(Verdict::Tests { code: 0 }));
        assert_ne!(Verdict::Tests { code: 101 }.code(), v.code());
    }
}
