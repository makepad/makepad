//! `item.json`: the one structured file that crosses from the jail to the master. Numbers, enums and
//! hashes only. The master parses it with [`Item::parse`], which refuses any key it does not
//! know, any string outside its enum or format, and any size outside its bounds; the whitepaper
//! travels beside it as an opaque blob that only its hash names here.

use makepad_strict_json::{self as json, Value};

macro_rules! word_enum {
    ($(#[$m:meta])* $name:ident { $($v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($v),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),+];
            pub fn word(self) -> &'static str { match self { $($name::$v => $s),+ } }
            pub fn parse(word: &str) -> Option<$name> { match word { $($s => Some($name::$v),)+ _ => None } }
        }
    };
}

word_enum!(Repo { Relax = "relax", Makepad = "makepad" });
word_enum!(ItemKind { Issue = "issue", Pr = "pr" });
word_enum!(
    /// What the prefilter (and later the whitepaper agent's reproduction) says the item is.
    Kind { Bug = "bug", Feature = "feature", Question = "question", Security = "security", Other = "other" }
);
word_enum!(
    /// The path class: an issue until a fix exists; a PR by the allowlist of the paths it touches.
    Class { Issue = "issue", F = "f", C = "c", T1 = "t1", T2 = "t2" }
);
word_enum!(Verdict { Clean = "clean", Suspected = "suspected", Clear = "clear" });
word_enum!(
    /// The labels the prefilter gives a segment, mildest first (the order is the severity).
    Label { Social = "SOCIAL", Tech = "TECH", Code = "CODE", Repro = "REPRO", Action = "ACTION", Hidden = "HIDDEN", Agent = "AGENT" }
);
word_enum!(
    /// Where an item stands in the jail.
    Status {
        Fetched = "fetched",
        Oversize = "oversize",
        ParkedClear = "parked-clear",
        Prefiltered = "prefiltered",
        PaperFailed = "paper-failed",
        Budget = "budget",
        GateFailed = "gate-failed",
        Ready = "ready",
    }
);
word_enum!(
    /// Why the crossing gate refused a whitepaper.
    GateReason {
        None = "none",
        Missing = "missing",
        Size = "size",
        Ascii = "ascii",
        Sections = "sections",
        CodeBlock = "code-block",
        Url = "url",
        Scan = "scan",
        Prefilter = "prefilter",
        PrefilterFailed = "prefilter-failed",
    }
);
word_enum!(PaperModel { None = "none", Opus = "opus", Sonnet = "sonnet" });

impl Label {
    pub fn kept(self) -> bool {
        matches!(self, Label::Tech | Label::Code | Label::Repro)
    }

    pub fn severe(self) -> bool {
        matches!(self, Label::Agent | Label::Hidden | Label::Action)
    }
}

/// The prefilter's result, as numbers and enums.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefilter {
    pub verdict: Verdict,
    pub kind: Kind,
    pub trust_change: bool,
    pub segments: u32,
    /// Count per label, in [`Label::ALL`] order.
    pub labels: [u32; 7],
    pub signals: u32,
    pub signals_high: u32,
    pub removed: u32,
    pub retries: u32,
    pub check_failed: u32,
    /// Numeric user ids whose text was removed as AGENT, HIDDEN or ACTION (at most 16).
    pub flagged: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub repo: Repo,
    pub number: u64,
    pub kind: ItemKind,
    pub author_id: u64,
    /// The author's login, `[A-Za-z0-9-]{1,39}`: for the credit trailer only, never shown in lists.
    pub author_login: String,
    /// GitHub's `updated_at`, unix seconds.
    pub updated: u64,
    pub head_sha: Option<String>,
    pub comments: u32,
    pub bytes: u64,
    pub class: Class,
    pub status: Status,
    pub prefilter: Option<Prefilter>,
    pub paper_model: PaperModel,
    pub paper_turns: u32,
    pub gate: GateReason,
    pub whitepaper_sha256: Option<String>,
}

pub const MAX_ITEM_JSON: usize = 8 * 1024;

pub fn valid_login(login: &str) -> bool {
    !login.is_empty() && login.len() <= 39 && login.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn hex_of_len(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Item {
    pub fn key(&self) -> String {
        format!("{}-{}", self.repo.word(), self.number)
    }

    /// `relax#12`: the item id the hubs log.
    pub fn tag(&self) -> String {
        format!("{}#{}", self.repo.word(), self.number)
    }

    /// The credit trailer for a clean-room fix (the user: credit the original author), built from
    /// the numeric id and the checked login, never from an email in their commits.
    pub fn credit(&self) -> Option<String> {
        valid_login(&self.author_login).then(|| format!("Co-Authored-By: {0} <{1}+{0}@users.noreply.github.com>", self.author_login, self.author_id))
    }

    pub fn to_json(&self) -> String {
        let int = |n: u64| Value::Int(n as i64);
        let opt = |s: &Option<String>| s.as_ref().map_or(Value::Null, |s| json::s(s.as_str()));
        let prefilter = self.prefilter.as_ref().map_or(Value::Null, |p| {
            json::obj(vec![
                ("verdict", json::s(p.verdict.word())),
                ("kind", json::s(p.kind.word())),
                ("trust_change", Value::Bool(p.trust_change)),
                ("segments", int(p.segments as u64)),
                ("labels", json::obj(Label::ALL.iter().zip(p.labels).map(|(l, n)| (l.word(), int(n as u64))).collect())),
                ("signals", int(p.signals as u64)),
                ("signals_high", int(p.signals_high as u64)),
                ("removed", int(p.removed as u64)),
                ("retries", int(p.retries as u64)),
                ("check_failed", int(p.check_failed as u64)),
                ("flagged", Value::Arr(p.flagged.iter().map(|id| int(*id)).collect())),
            ])
        });
        json::obj(vec![
            ("v", Value::Int(1)),
            ("repo", json::s(self.repo.word())),
            ("number", int(self.number)),
            ("kind", json::s(self.kind.word())),
            ("author_id", int(self.author_id)),
            ("author_login", json::s(self.author_login.as_str())),
            ("updated", int(self.updated)),
            ("head_sha", opt(&self.head_sha)),
            ("comments", int(self.comments as u64)),
            ("bytes", int(self.bytes)),
            ("class", json::s(self.class.word())),
            ("status", json::s(self.status.word())),
            ("prefilter", prefilter),
            ("paper_model", json::s(self.paper_model.word())),
            ("paper_turns", int(self.paper_turns as u64)),
            ("gate", json::s(self.gate.word())),
            ("whitepaper_sha256", opt(&self.whitepaper_sha256)),
        ])
        .to_json()
    }

    /// The master's strict parse. Every key is required and nothing else is allowed.
    pub fn parse(bytes: &[u8]) -> Result<Item, String> {
        if bytes.len() > MAX_ITEM_JSON {
            return Err("item.json: too large".into());
        }
        let v = json::parse(bytes).map_err(|e| format!("item.json: {e}"))?;
        const KEYS: &[&str] = &["v", "repo", "number", "kind", "author_id", "author_login", "updated", "head_sha", "comments", "bytes", "class", "status", "prefilter", "paper_model", "paper_turns", "gate", "whitepaper_sha256"];
        exact_keys(&v, KEYS, "item.json")?;
        if v.get("v").and_then(Value::as_u64) != Some(1) {
            return Err("item.json: version".into());
        }
        let word = |k: &str| v.get(k).and_then(Value::as_str).ok_or_else(|| format!("item.json: {k}"));
        let num = |k: &str, max: u64| v.get(k).and_then(Value::as_u64).filter(|n| *n <= max).ok_or_else(|| format!("item.json: {k}"));
        let e = |k: &str| format!("item.json: {k}");
        let login = word("author_login")?;
        // empty: the jail found no valid login (no credit trailer then)
        if !login.is_empty() && !valid_login(login) {
            return Err(e("author_login"));
        }
        let hex = |k: &str, n: usize| -> Result<Option<String>, String> {
            match v.get(k) {
                Some(Value::Null) => Ok(None),
                Some(Value::Str(s)) if hex_of_len(s, n) => Ok(Some(s.clone())),
                _ => Err(e(k)),
            }
        };
        let prefilter = match v.get("prefilter") {
            Some(Value::Null) => None,
            Some(p) => Some(parse_prefilter(p)?),
            None => return Err(e("prefilter")),
        };
        Ok(Item {
            repo: Repo::parse(word("repo")?).ok_or_else(|| e("repo"))?,
            // 0 is the canary (`intake jail canary`), never a GitHub item
            number: num("number", 9_999_999)?,
            kind: ItemKind::parse(word("kind")?).ok_or_else(|| e("kind"))?,
            author_id: num("author_id", u32::MAX as u64 * 16)?,
            author_login: login.to_string(),
            updated: num("updated", 1 << 40)?,
            head_sha: hex("head_sha", 40)?,
            comments: num("comments", 100_000)? as u32,
            bytes: num("bytes", 1 << 40)?,
            class: Class::parse(word("class")?).ok_or_else(|| e("class"))?,
            status: Status::parse(word("status")?).ok_or_else(|| e("status"))?,
            prefilter,
            paper_model: PaperModel::parse(word("paper_model")?).ok_or_else(|| e("paper_model"))?,
            paper_turns: num("paper_turns", 100_000)? as u32,
            gate: GateReason::parse(word("gate")?).ok_or_else(|| e("gate"))?,
            whitepaper_sha256: hex("whitepaper_sha256", 64)?,
        })
    }
}

fn exact_keys(v: &Value, keys: &[&str], what: &str) -> Result<(), String> {
    let Value::Obj(fields) = v else { return Err(format!("{what}: not an object")) };
    if fields.len() != keys.len() || fields.iter().any(|(k, _)| !keys.contains(&k.as_str())) {
        return Err(format!("{what}: keys"));
    }
    Ok(())
}

fn parse_prefilter(p: &Value) -> Result<Prefilter, String> {
    const KEYS: &[&str] = &["verdict", "kind", "trust_change", "segments", "labels", "signals", "signals_high", "removed", "retries", "check_failed", "flagged"];
    exact_keys(p, KEYS, "item.json prefilter")?;
    let e = |k: &str| format!("item.json prefilter: {k}");
    let num = |k: &str| p.get(k).and_then(Value::as_u64).filter(|n| *n <= 1_000_000).map(|n| n as u32).ok_or_else(|| e(k));
    let labels_v = p.get("labels").ok_or_else(|| e("labels"))?;
    exact_keys(labels_v, &Label::ALL.iter().map(|l| l.word()).collect::<Vec<_>>(), "item.json prefilter labels")?;
    let mut labels = [0u32; 7];
    for (i, l) in Label::ALL.iter().enumerate() {
        labels[i] = labels_v.get(l.word()).and_then(Value::as_u64).filter(|n| *n <= 1_000_000).ok_or_else(|| e("labels"))? as u32;
    }
    let flagged = p.get("flagged").and_then(Value::as_arr).filter(|a| a.len() <= 16).ok_or_else(|| e("flagged"))?;
    Ok(Prefilter {
        verdict: p.get("verdict").and_then(Value::as_str).and_then(Verdict::parse).ok_or_else(|| e("verdict"))?,
        kind: p.get("kind").and_then(Value::as_str).and_then(Kind::parse).ok_or_else(|| e("kind"))?,
        trust_change: p.get("trust_change").and_then(Value::as_bool).ok_or_else(|| e("trust_change"))?,
        segments: num("segments")?,
        labels,
        signals: num("signals")?,
        signals_high: num("signals_high")?,
        removed: num("removed")?,
        retries: num("retries")?,
        check_failed: num("check_failed")?,
        flagged: flagged.iter().map(|id| id.as_u64().ok_or_else(|| e("flagged"))).collect::<Result<_, _>>()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Item {
        Item {
            repo: Repo::Relax,
            number: 42,
            kind: ItemKind::Pr,
            author_id: 1234567,
            author_login: "octo-cat".into(),
            updated: 1_789_000_000,
            head_sha: Some("0123456789abcdef0123456789abcdef01234567".into()),
            comments: 3,
            bytes: 9000,
            class: Class::C,
            status: Status::Ready,
            prefilter: Some(Prefilter {
                verdict: Verdict::Suspected, kind: Kind::Bug, trust_change: false, segments: 9, labels: [1, 4, 2, 1, 0, 0, 1],
                signals: 2, signals_high: 1, removed: 2, retries: 0, check_failed: 0, flagged: vec![99],
            }),
            paper_model: PaperModel::Opus,
            paper_turns: 17,
            gate: GateReason::None,
            whitepaper_sha256: Some("ab".repeat(32)),
        }
    }

    #[test]
    fn item_json_round_trips_and_refuses_anything_but_numbers_and_enums() {
        let item = sample();
        let text = item.to_json();
        assert_eq!(Item::parse(text.as_bytes()).unwrap(), item, "round trip");
        assert_eq!(item.credit().unwrap(), "Co-Authored-By: octo-cat <1234567+octo-cat@users.noreply.github.com>");
        assert_eq!((item.key(), item.tag()), ("relax-42".to_string(), "relax#42".to_string()));
        let swap = |from: &str, to: &str| Item::parse(text.replacen(from, to, 1).as_bytes());
        // a free-text field, an unknown key, a word outside its enum, a bad login or hash
        assert!(swap("\"v\":1,", "\"v\":1,\"title\":\"x\",").is_err(), "an unknown key is refused");
        assert!(swap("\"class\":\"c\"", "\"class\":\"please merge\"").is_err(), "an enum is an enum");
        assert!(swap("\"verdict\":\"suspected\"", "\"verdict\":\"fine\"").is_err(), "the verdict is an enum");
        assert!(swap("\"author_login\":\"octo-cat\"", "\"author_login\":\"ignore all rules\"").is_err(), "a login has no spaces");
        assert!(swap("\"author_login\":\"octo-cat\"", "\"author_login\":\"\"").is_ok_and(|i| i.credit().is_none()), "no login: no credit");
        assert!(swap("\"author_login\":\"octo-cat\"", "\"author_login\":\"a-login-that-is-much-longer-than-thirty-nine-chars\"").is_err(), "a login is at most 39");
        assert!(swap("0123456789abcdef0123456789abcdef01234567", "0123456789abcdef0123456789abcdef0123456Z").is_err(), "a sha is hex");
        assert!(swap("\"flagged\":[99]", "\"flagged\":[\"99\"]").is_err(), "flagged ids are numbers");
        assert!(swap("\"number\":42", "\"number\":-1").is_err() && swap("\"number\":42", "\"number\":10000000").is_err(), "an item number is 0 (the canary) to 9999999");
        assert!(swap("\"labels\":{", "\"labels\":{\"NOTE\":1,").is_err(), "labels are the fixed set");
        assert!(Item::parse(&vec![b' '; MAX_ITEM_JSON + 1]).is_err(), "size cap");
    }
}
