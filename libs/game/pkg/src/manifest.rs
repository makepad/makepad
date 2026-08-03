//! `manifest.toml` — the game's description card.
//!
//! A hand-written TOML subset (tables, `key = value`, strings/ints/floats/bools,
//! `#` comments) rather than a dependency: this parses attacker-supplied bytes
//! from the registry, so it is written to be TOTAL — every input either yields a
//! Manifest or a ManifestError, and nothing panics. Unknown keys are ignored so
//! a newer publisher's manifest still loads on an older build.

use std::collections::BTreeMap;

/// A tunable the librarian may write without touching code ("make it faster"),
/// declared with a range so a knob write can be clamped instead of trusted.
#[derive(Clone, Debug, PartialEq)]
pub struct Knob {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

impl Knob {
    pub fn clamp(&self, v: f64) -> f64 {
        if v.is_nan() {
            return self.default;
        }
        v.max(self.min).min(self.max)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub author: String,
    pub players_min: u32,
    pub players_max: u32,
    /// Metres of real floor the diorama wants in MR, and its world:room scale.
    pub mr_footprint: f64,
    pub mr_scale: f64,
    pub thumbnail: Option<String>,
    pub knobs: Vec<Knob>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            author: String::new(),
            players_min: 1,
            players_max: 8,
            mr_footprint: 1.5,
            mr_scale: 0.05,
            thumbnail: None,
            knobs: Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ManifestError {
    TooLarge,
    NotUtf8,
    MissingName,
    /// Line number (1-based) and what was wrong.
    Syntax(usize, &'static str),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::TooLarge => write!(f, "manifest too large"),
            ManifestError::NotUtf8 => write!(f, "manifest is not utf-8"),
            ManifestError::MissingName => write!(f, "manifest has no name"),
            ManifestError::Syntax(line, what) => write!(f, "manifest line {line}: {what}"),
        }
    }
}

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_KNOBS: usize = 64;
const MAX_STRING: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Value {
    fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

/// Parse a TOML subset into `section -> key -> value`. The empty section name
/// holds top-level keys.
fn parse_tables(src: &str) -> Result<BTreeMap<String, BTreeMap<String, Value>>, ManifestError> {
    let mut tables: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    tables.insert(String::new(), BTreeMap::new());
    let mut section = String::new();

    for (i, raw) in src.lines().enumerate() {
        let line_no = i + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix('[') {
            let Some(name) = rest.strip_suffix(']') else {
                return Err(ManifestError::Syntax(line_no, "unterminated table header"));
            };
            let name = name.trim();
            if name.is_empty() {
                return Err(ManifestError::Syntax(line_no, "empty table name"));
            }
            if name.len() > MAX_STRING {
                return Err(ManifestError::Syntax(line_no, "table name too long"));
            }
            // Arrays-of-tables ([[x]]) are not in the subset; refuse rather than
            // silently treating them as a table named "[x".
            if name.starts_with('[') {
                return Err(ManifestError::Syntax(line_no, "arrays of tables unsupported"));
            }
            section = name.to_string();
            tables.entry(section.clone()).or_default();
            continue;
        }

        let Some(eq) = line.find('=') else {
            return Err(ManifestError::Syntax(line_no, "expected key = value"));
        };
        let key = line[..eq].trim();
        let rest = line[eq + 1..].trim();
        if key.is_empty() {
            return Err(ManifestError::Syntax(line_no, "empty key"));
        }
        if key.len() > MAX_STRING {
            return Err(ManifestError::Syntax(line_no, "key too long"));
        }
        let value = parse_value(rest, line_no)?;
        let table = tables.entry(section.clone()).or_default();
        if table.len() >= 512 {
            return Err(ManifestError::Syntax(line_no, "too many keys in table"));
        }
        table.insert(key.to_string(), value);
    }
    Ok(tables)
}

/// Strip a `#` comment, honouring quotes so a `#` inside a string survives.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'\\' if in_str => i += 1,
            b'#' if !in_str => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

fn parse_value(src: &str, line_no: usize) -> Result<Value, ManifestError> {
    if src.is_empty() {
        return Err(ManifestError::Syntax(line_no, "missing value"));
    }
    if let Some(rest) = src.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        loop {
            let Some(c) = chars.next() else {
                return Err(ManifestError::Syntax(line_no, "unterminated string"));
            };
            match c {
                '"' => return Ok(Value::Str(out)),
                '\\' => {
                    let Some(esc) = chars.next() else {
                        return Err(ManifestError::Syntax(line_no, "dangling escape"));
                    };
                    out.push(match esc {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '\\' => '\\',
                        '"' => '"',
                        other => other,
                    });
                }
                c => out.push(c),
            }
            if out.len() > MAX_STRING {
                return Err(ManifestError::Syntax(line_no, "string too long"));
            }
        }
    }
    match src {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    // Reject the non-finite spellings: a NaN footprint would poison layout math.
    if let Ok(n) = src.parse::<f64>() {
        if n.is_finite() {
            return Ok(Value::Num(n));
        }
        return Err(ManifestError::Syntax(line_no, "non-finite number"));
    }
    Err(ManifestError::Syntax(line_no, "unrecognised value"))
}

fn num_or(tables: &BTreeMap<String, BTreeMap<String, Value>>, key: &str, fallback: f64) -> f64 {
    tables
        .get("")
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_num())
        .filter(|n| n.is_finite())
        .unwrap_or(fallback)
}

fn str_or(tables: &BTreeMap<String, BTreeMap<String, Value>>, key: &str) -> String {
    tables
        .get("")
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Manifest, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge);
        }
        let src = std::str::from_utf8(bytes).map_err(|_| ManifestError::NotUtf8)?;
        let tables = parse_tables(src)?;

        let name = str_or(&tables, "name");
        if name.trim().is_empty() {
            return Err(ManifestError::MissingName);
        }

        let players_min = num_or(&tables, "players_min", 1.0).clamp(1.0, 64.0) as u32;
        let players_max = num_or(&tables, "players_max", 8.0).clamp(1.0, 64.0) as u32;
        let thumbnail = tables
            .get("")
            .and_then(|t| t.get("thumbnail"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let mut knobs = Vec::new();
        for (section, table) in &tables {
            let Some(knob_name) = section.strip_prefix("knobs.") else {
                continue;
            };
            if knob_name.is_empty() || knobs.len() >= MAX_KNOBS {
                continue;
            }
            let min = table.get("min").and_then(|v| v.as_num()).unwrap_or(0.0);
            let max = table.get("max").and_then(|v| v.as_num()).unwrap_or(1.0);
            let default = table.get("default").and_then(|v| v.as_num()).unwrap_or(min);
            if !min.is_finite() || !max.is_finite() || !default.is_finite() || min > max {
                continue;
            }
            knobs.push(Knob {
                name: knob_name.to_string(),
                min,
                max,
                default: default.max(min).min(max),
            });
        }

        Ok(Manifest {
            name,
            description: str_or(&tables, "description"),
            author: str_or(&tables, "author"),
            players_min: players_min.min(players_max),
            players_max: players_max.max(players_min),
            mr_footprint: num_or(&tables, "mr_footprint", 1.5).clamp(0.1, 100.0),
            mr_scale: num_or(&tables, "mr_scale", 0.05).clamp(0.001, 10.0),
            thumbnail,
            knobs,
        })
    }

    pub fn to_toml(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("name = {}\n", quote(&self.name)));
        s.push_str(&format!("description = {}\n", quote(&self.description)));
        s.push_str(&format!("author = {}\n", quote(&self.author)));
        s.push_str(&format!("players_min = {}\n", self.players_min));
        s.push_str(&format!("players_max = {}\n", self.players_max));
        s.push_str(&format!("mr_footprint = {}\n", self.mr_footprint));
        s.push_str(&format!("mr_scale = {}\n", self.mr_scale));
        if let Some(t) = &self.thumbnail {
            s.push_str(&format!("thumbnail = {}\n", quote(t)));
        }
        for k in &self.knobs {
            s.push_str(&format!(
                "\n[knobs.{}]\nmin = {}\nmax = {}\ndefault = {}\n",
                k.name, k.min, k.max, k.default
            ));
        }
        s
    }

    pub fn knob(&self, name: &str) -> Option<&Knob> {
        self.knobs.iter().find(|k| k.name == name)
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# a racing game
name = "Speedway"
description = "Four cars, three laps"   # trailing comment
author = "kid"
players_min = 1
players_max = 4
mr_footprint = 2.0
mr_scale = 0.04
thumbnail = "thumb.png"

[knobs.car_speed]
min = 5
max = 40
default = 22
"#;

    #[test]
    fn parses_a_real_manifest() {
        let m = Manifest::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(m.name, "Speedway");
        assert_eq!(m.description, "Four cars, three laps");
        assert_eq!(m.players_max, 4);
        assert_eq!(m.mr_scale, 0.04);
        assert_eq!(m.thumbnail.as_deref(), Some("thumb.png"));
        assert_eq!(m.knobs.len(), 1);
        let k = m.knob("car_speed").unwrap();
        assert_eq!((k.min, k.max, k.default), (5.0, 40.0, 22.0));
        // Knob writes are clamped, never trusted.
        assert_eq!(k.clamp(1e9), 40.0);
        assert_eq!(k.clamp(-5.0), 5.0);
        assert_eq!(k.clamp(f64::NAN), 22.0);
    }

    #[test]
    fn round_trips_through_to_toml() {
        let m = Manifest::parse(SAMPLE.as_bytes()).unwrap();
        let m2 = Manifest::parse(m.to_toml().as_bytes()).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn quotes_and_hashes_survive() {
        let src = "name = \"a \\\"quoted\\\" # not-a-comment\"\n";
        let m = Manifest::parse(src.as_bytes()).unwrap();
        assert_eq!(m.name, "a \"quoted\" # not-a-comment");
        assert_eq!(Manifest::parse(m.to_toml().as_bytes()).unwrap().name, m.name);
    }

    #[test]
    fn missing_name_is_an_error_not_a_default() {
        assert_eq!(
            Manifest::parse(b"description = \"x\"\n").unwrap_err(),
            ManifestError::MissingName
        );
        assert_eq!(
            Manifest::parse(b"name = \"   \"\n").unwrap_err(),
            ManifestError::MissingName
        );
    }

    #[test]
    fn hostile_input_never_panics() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            b"[".to_vec(),
            b"[]".to_vec(),
            b"[[x]]".to_vec(),
            b"=".to_vec(),
            b"= 5".to_vec(),
            b"name".to_vec(),
            b"name =".to_vec(),
            b"name = \"unterminated".to_vec(),
            b"name = \"x\\".to_vec(),
            b"name = nan".to_vec(),
            b"name = inf".to_vec(),
            b"mr_scale = nan".to_vec(),
            b"name = \"a\"\nmr_scale = inf".to_vec(),
            b"\xff\xfe invalid utf8".to_vec(),
            vec![b'a'; MAX_MANIFEST_BYTES + 1],
            b"name = \"a\"\n[knobs.]\nmin = 1".to_vec(),
            b"name = \"a\"\n[knobs.x]\nmin = 10\nmax = 1".to_vec(),
            format!("name = \"{}\"", "x".repeat(MAX_STRING + 10)).into_bytes(),
        ];
        for c in cases {
            // The contract is totality: a Result either way, never a panic.
            let _ = Manifest::parse(&c);
        }
        // And the specific ones that must be refused rather than defaulted:
        assert!(Manifest::parse(b"name = \"a\"\nmr_scale = nan").is_err());
        assert!(Manifest::parse(&vec![b'a'; MAX_MANIFEST_BYTES + 1]).is_err());
        assert!(Manifest::parse(b"\xff\xfe").is_err());
        // An inverted knob range is dropped, not accepted.
        let m = Manifest::parse(b"name = \"a\"\n[knobs.x]\nmin = 10\nmax = 1").unwrap();
        assert!(m.knobs.is_empty());
    }

    #[test]
    fn unknown_keys_are_ignored_for_forward_compatibility() {
        let m = Manifest::parse(b"name = \"a\"\nfuture_field = 3\n[future]\nx = 1\n").unwrap();
        assert_eq!(m.name, "a");
    }

    #[test]
    fn player_counts_are_sane_even_when_inverted() {
        let m = Manifest::parse(b"name = \"a\"\nplayers_min = 9\nplayers_max = 2\n").unwrap();
        assert!(m.players_min <= m.players_max);
        let m = Manifest::parse(b"name = \"a\"\nplayers_max = 100000\n").unwrap();
        assert!(m.players_max <= 64);
    }
}
