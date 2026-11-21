//! Questions main files for the user, with durable answers and their socket peer's provenance.
//! A take records delivery to main; it does not change what the tray shows.
//!
//! A review card is a question with items: a Yes and a No per item, the press the decision. Main's
//! own items (`--item`) record the user's answer per item, judged as an answer is. An item in doubt
//! in the intake (`--intake`) comes from the gate binary ([`Gate`], gate.rs), which writes the
//! lines and the row id the press binds to; its press is the tray's alone, and the gate decides it:
//! the master hands its child a single-use nonce and the child claims the press back (`claim`,
//! served to the master's own child alone), so nothing but a press judged the tray's clears or
//! declines an item, and only the row the card showed.

use crate::askpeer::{Judged, Source};
use crate::proto::{hub_record, wall_ms, Record, Sev};
use makepad_remote::json_str;
use makepad_strict_json::{obj, parse, s, Value};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Open,
    Waiting,
    Answered,
    Closed,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Waiting => "waiting",
            Self::Answered => "answered",
            Self::Closed => "closed",
        }
    }

    pub fn to_value(self) -> Value { s(self.word()) }

    pub fn from_value(v: &Value) -> Option<Self> {
        Some(match v.as_str()? {
            "open" => Self::Open,
            "waiting" => Self::Waiting,
            "answered" => Self::Answered,
            "closed" => Self::Closed,
            _ => return None,
        })
    }

    fn group(self) -> u8 {
        match self { Self::Open | Self::Waiting => 0, Self::Answered => 1, Self::Closed => 2 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub id: String,
    pub from: String,
    pub title: String,
    pub decided: String,
    pub why: String,
    pub options: Vec<String>,
    pub recommend: String,
    pub blocked: String,
    pub context: String,
    pub created_ms: u64,
    pub state: State,
    pub thread: Vec<Msg>,
    pub refused: Vec<Refused>,
    /// A review card's items, a Yes and a No each (empty: a question with options)
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Msg {
    pub seq: u32,
    pub at_ms: u64,
    pub by: String,
    pub kind: String,
    pub choice: Option<u32>,
    pub decide: bool,
    pub text: String,
    pub source: String,
    pub chain: String,
    pub taken: bool,
    /// The item a Yes or No (kind `yes` / `no`) was pressed on
    pub item: String,
    /// The item as the press left it: decided, or (a row that moved) as it stands now
    pub card: Option<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Refused {
    pub at_ms: u64,
    pub chain: String,
    pub why: String,
    /// The item a refused press was on (empty: the question's answer)
    pub item: String,
}

/// One item of a review card: its lines, what Yes and No do, and where it stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// `makepad#1491` (the gate's), or `1`, `2`, ... (main's own)
    pub key: String,
    pub head: String,
    /// A short tag beside the head (the gate's `closed on GitHub`), empty for none
    pub tag: String,
    pub what: String,
    pub why: String,
    pub risks: String,
    pub yes: String,
    pub no: String,
    /// Main's line under Yes: who takes it (`--plan`)
    pub plan: String,
    /// The row id the gate wrote: a press binds to it (empty: main's own item)
    pub row: String,
    /// From the gate binary: pressed in the tray alone, and decided by the gate
    pub gate: bool,
    /// `yes` or `no` once decided, else empty
    pub decided: String,
    /// What the last press did, or why the item is shown anew
    pub note: String,
    pub at_ms: u64,
}

/// A field of an item at most, in characters.
const ITEM_FIELD_MAX: usize = 2000;

impl Item {
    pub fn to_value(&self) -> Value {
        let mut v = obj(vec![
            ("key", s(&self.key)), ("head", s(&self.head)), ("what", s(&self.what)), ("why", s(&self.why)),
            ("risks", s(&self.risks)), ("yes", s(&self.yes)), ("no", s(&self.no)), ("plan", s(&self.plan)),
            ("row", s(&self.row)), ("gate", Value::Bool(self.gate)), ("decided", s(&self.decided)),
            ("note", s(&self.note)), ("at_ms", number(self.at_ms)),
        ]);
        if let (Value::Obj(fields), false) = (&mut v, self.tag.is_empty()) { fields.push(("tag".into(), s(&self.tag))); }
        v
    }

    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self {
            key: field(v, "key")?.into(), head: field(v, "head")?.into(), tag: field(v, "tag").unwrap_or("").into(), what: field(v, "what")?.into(),
            why: field(v, "why")?.into(), risks: field(v, "risks")?.into(), yes: field(v, "yes")?.into(),
            no: field(v, "no")?.into(), plan: field(v, "plan")?.into(), row: field(v, "row")?.into(),
            gate: v.get("gate")?.as_bool()?, decided: field(v, "decided")?.into(), note: field(v, "note")?.into(),
            at_ms: v.get("at_ms")?.as_u64()?,
        })
    }

    /// An item as the gate binary's card line gives it: every line a string within its cap.
    fn from_gate(v: &Value) -> Result<Self, String> {
        let take = |k: &str| -> Result<String, String> {
            let t = field(v, k).ok_or_else(|| format!("the gate's card line has no {k}"))?;
            if t.chars().count() > ITEM_FIELD_MAX { return Err(format!("the gate's card line's {k} is over {ITEM_FIELD_MAX} characters")); }
            Ok(t.to_string())
        };
        let row = take("row")?;
        if row.is_empty() || row.len() > 64 || !row.bytes().all(|b| b.is_ascii_hexdigit()) { return Err("the gate's card line's row is no row id".into()); }
        Ok(Self {
            key: take("key")?, head: take("head")?, tag: tag(field(v, "tag").unwrap_or(""))?, what: take("what")?, why: take("why")?,
            risks: take("risks")?, yes: take("yes")?, no: take("no")?, row, gate: true, ..Self::default()
        })
    }

    /// Main's own item (`--item`): its head, and any of the other lines.
    fn from_main(v: &Value, key: usize) -> Result<Self, String> {
        let take = |k: &str| -> Result<String, String> {
            let t = text_field(v, k)?;
            if t.chars().count() > ITEM_FIELD_MAX { return Err(format!("an item's {k} is over {ITEM_FIELD_MAX} characters")); }
            Ok(t)
        };
        let head = take("head")?;
        if head.trim().is_empty() { return Err("an item needs its head".into()); }
        Ok(Self {
            key: key.to_string(), head, tag: tag(&text_field(v, "tag")?)?, what: take("what")?, why: take("why")?,
            risks: take("risks")?, yes: take("yes")?, no: take("no")?, ..Self::default()
        })
    }
}

/// An item's tag: a few words on one line.
const ITEM_TAG_MAX: usize = 40;

fn tag(t: &str) -> Result<String, String> {
    if t.chars().count() > ITEM_TAG_MAX || t.contains(['\n', '\r']) { return Err(format!("an item's tag is a line of at most {ITEM_TAG_MAX} characters")); }
    Ok(t.to_string())
}

/// A press the master hands its gate child, which claims it by its nonce (`claim`).
#[derive(Clone, Debug, PartialEq)]
pub struct Claimed {
    /// the question
    pub id: String,
    pub item: String,
    /// the row the card showed
    pub row: String,
    pub yes: bool,
    /// the judge's word: `tray` for every press the master hands on
    pub source: String,
    pub chain: String,
}

impl Claimed {
    pub fn to_value(&self) -> Value {
        obj(vec![("id", s(&self.id)), ("item", s(&self.item)), ("row", s(&self.row)), ("yes", Value::Bool(self.yes)), ("source", s(&self.source)), ("chain", s(&self.chain))])
    }

    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self { id: field(v, "id")?.into(), item: field(v, "item")?.into(), row: field(v, "row")?.into(), yes: v.get("yes")?.as_bool()?, source: field(v, "source")?.into(), chain: field(v, "chain")?.into() })
    }
}

/// The gate's child's side of a press: the press the master holds under `nonce` (once).
pub fn claim(nonce: &str) -> Result<Claimed, String> {
    let c = crate::cli::control()?;
    let lines = crate::cli::ask(&c, None, "ask", &format!("claim {}", obj(vec![("nonce", s(nonce))]).to_json()))?;
    let v = parse(lines.first().ok_or("no response")?.as_bytes()).map_err(|e| format!("invalid response: {e}"))?;
    if let Some(e) = field(&v, "err") { return Err(e.to_string()); }
    Claimed::from_value(&v).ok_or_else(|| "invalid claim response".to_string())
}

/// The intake's gate binary as a review card needs it (gate.rs runs it; tests give their own).
pub trait Gate: Send + Sync {
    /// The card's lines for `tags` (`["--all"]`: every item in doubt): `{key, head, what, why,
    /// risks, yes, no, row}` each, or `{skip, why}`.
    fn card(&self, tags: &[String]) -> Result<Vec<Value>, String>;
    /// Runs the gate's `decide` with `nonce` on its stdin: its answer, `{ok, row}`,
    /// `{stale: <card line>, why}` or `{err}`.
    fn decide(&self, nonce: &str) -> Result<Value, String>;
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a str> { v.get(key)?.as_str() }
fn number(n: u64) -> Value { Value::Int(n as i64) }
fn choice_value(choice: Option<u32>) -> Value { choice.map_or(Value::Null, |n| number(n as u64)) }
fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_arr()?.iter().map(|v| v.as_str().map(str::to_string)).collect()
}
fn choice(v: &Value) -> Option<Option<u32>> {
    if v.is_null() { Some(None) } else { Some(Some(u32::try_from(v.as_u64()?).ok()?)) }
}

impl Question {
    pub fn to_value(&self) -> Value {
        let mut v = obj(vec![
            ("id", s(&self.id)), ("from", s(&self.from)), ("title", s(&self.title)),
            ("decided", s(&self.decided)), ("why", s(&self.why)),
            ("options", Value::Arr(self.options.iter().map(s).collect())),
            ("recommend", s(&self.recommend)), ("blocked", s(&self.blocked)), ("context", s(&self.context)),
            ("created_ms", number(self.created_ms)), ("state", self.state.to_value()),
            ("thread", Value::Arr(self.thread.iter().map(Msg::to_value).collect())),
            ("refused", Value::Arr(self.refused.iter().map(Refused::to_value).collect())),
        ]);
        // (a question with options keeps its line as it was)
        if let (Value::Obj(fields), false) = (&mut v, self.items.is_empty()) {
            fields.push(("items".into(), Value::Arr(self.items.iter().map(Item::to_value).collect())));
        }
        v
    }

    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self {
            id: field(v, "id")?.into(), from: field(v, "from")?.into(), title: field(v, "title")?.into(),
            decided: field(v, "decided")?.into(), why: field(v, "why")?.into(),
            options: strings(v.get("options")?)?, recommend: field(v, "recommend")?.into(),
            blocked: field(v, "blocked")?.into(), context: field(v, "context")?.into(),
            created_ms: v.get("created_ms")?.as_u64()?, state: State::from_value(v.get("state")?)?,
            thread: v.get("thread")?.as_arr()?.iter().map(Msg::from_value).collect::<Option<_>>()?,
            refused: v.get("refused")?.as_arr()?.iter().map(Refused::from_value).collect::<Option<_>>()?,
            items: match v.get("items") { None => Vec::new(), Some(items) => items.as_arr()?.iter().map(Item::from_value).collect::<Option<_>>()? },
        })
    }

    fn last_ms(&self) -> u64 {
        self.thread.iter().map(|m| m.at_ms).chain(self.refused.iter().map(|r| r.at_ms))
            .fold(self.created_ms, u64::max)
    }

    fn summary(&self) -> Value {
        obj(vec![
            ("id", s(&self.id)), ("from", s(&self.from)), ("title", s(&self.title)),
            ("state", self.state.to_value()), ("created_ms", number(self.created_ms)),
            ("last_ms", number(self.last_ms())), ("msgs", number(self.thread.len() as u64)),
            ("refused", number(self.refused.len() as u64)),
        ])
    }

    fn choice_text(&self, m: &Msg) -> &str {
        m.choice.and_then(|n| n.checked_sub(1)).and_then(|n| self.options.get(n as usize))
            .map_or("", String::as_str)
    }

    fn delivery(&self, m: &Msg) -> Value {
        obj(vec![
            ("id", s(&self.id)), ("title", s(&self.title)), ("seq", number(m.seq as u64)),
            ("kind", s(&m.kind)), ("choice", choice_value(m.choice)), ("choice_text", s(self.choice_text(m))),
            ("decide", Value::Bool(m.decide)), ("text", s(&m.text)), ("source", s(&m.source)),
            ("chain", s(&m.chain)), ("at_ms", number(m.at_ms)), ("item", s(&m.item)),
        ])
    }
}

impl Msg {
    pub fn to_value(&self) -> Value {
        let mut v = obj(vec![
            ("seq", number(self.seq as u64)), ("at_ms", number(self.at_ms)), ("by", s(&self.by)),
            ("kind", s(&self.kind)), ("choice", choice_value(self.choice)), ("decide", Value::Bool(self.decide)),
            ("text", s(&self.text)), ("source", s(&self.source)), ("chain", s(&self.chain)),
            ("taken", Value::Bool(self.taken)),
        ]);
        // (a message on no item keeps its line as it was)
        if let Value::Obj(fields) = &mut v {
            if !self.item.is_empty() { fields.push(("item".into(), s(&self.item))); }
            if let Some(card) = &self.card { fields.push(("card".into(), card.to_value())); }
        }
        v
    }

    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self {
            seq: u32::try_from(v.get("seq")?.as_u64()?).ok()?, at_ms: v.get("at_ms")?.as_u64()?,
            by: field(v, "by")?.into(), kind: field(v, "kind")?.into(), choice: choice(v.get("choice")?)?,
            decide: v.get("decide")?.as_bool()?, text: field(v, "text")?.into(),
            source: field(v, "source")?.into(), chain: field(v, "chain")?.into(), taken: v.get("taken")?.as_bool()?,
            item: field(v, "item").unwrap_or("").into(),
            card: match v.get("card") { None => None, Some(card) => Some(Item::from_value(card)?) },
        })
    }
}

impl Refused {
    pub fn to_value(&self) -> Value {
        let mut fields = vec![("at_ms", number(self.at_ms)), ("chain", s(&self.chain)), ("why", s(&self.why))];
        if !self.item.is_empty() { fields.push(("item", s(&self.item))); }
        obj(fields)
    }

    pub fn from_value(v: &Value) -> Option<Self> {
        Some(Self { at_ms: v.get("at_ms")?.as_u64()?, chain: field(v, "chain")?.into(), why: field(v, "why")?.into(), item: field(v, "item").unwrap_or("").into() })
    }
}

/// Live changes and replay use the same transitions. No message means close.
fn next_state(state: State, message: Option<&Msg>) -> Result<State, String> {
    let Some(m) = message else { return Ok(State::Closed) };
    if state == State::Closed { return Err("the question is closed".into()); }
    match (m.by.as_str(), m.kind.as_str()) {
        ("user", "answer") => Ok(State::Answered),
        ("user", "ask") => Ok(State::Waiting),
        ("main", "reply") => Ok(if state == State::Waiting { State::Open } else { state }),
        // main's reply that gives the turn back whatever the state: an answer that was a question
        // (`ask reply --reopen`) left the reply under the answered ones, where the user did not
        // find it (Q27, Q28, 11-18)
        ("main", "reopen") => Ok(State::Open),
        _ => Err("unknown message author or kind".into()),
    }
}

/// A message into its question, live and in replay alike: the state's transition ([`next_state`]),
/// the item a press left (its `card`), and a review card's turn: a press that decides its last
/// open item answers it, and the user's answer in words leaves it theirs while any item is open.
fn apply(q: &mut Question, m: Msg) -> Result<(), String> {
    let mut state = if m.item.is_empty() { next_state(q.state, Some(&m))? } else { q.state };
    if let Some(card) = &m.card {
        let slot = q.items.iter_mut().find(|i| i.key == card.key).ok_or_else(|| format!("{} has no item {}", q.id, card.key))?;
        *slot = card.clone();
    }
    if !q.items.is_empty() && m.by == "user" && state != State::Closed {
        let all = q.items.iter().all(|i| !i.decided.is_empty());
        if !m.item.is_empty() && all && state != State::Answered { state = State::Answered; }
        if m.kind == "answer" && !all { state = State::Open; }
    }
    q.state = state;
    q.thread.push(m);
    Ok(())
}

pub struct Asks {
    path: Option<PathBuf>,
    machine: String,
    publish: Option<Publish>,
    st: Mutex<St>,
    changed: Condvar,
    /// The intake's gate binary, on the master with the private checkout ([`Asks::set_gate`])
    gate: OnceLock<Arc<dyn Gate>>,
}

#[derive(Default)]
struct St {
    /// The presses handed to the gate's child, by nonce, until it claims them (once)
    pending: HashMap<String, Claimed>,
    /// The items a press is on its way for: (question, item)
    deciding: HashSet<(String, String)>,
    qs: Vec<Question>,
    rev: u64,
    /// The highest id given, a cleared question's too: ids count on after a clear
    last_id: u64,
    /// The changes of the questions cleared to the archive: the revision rebuilt at open
    /// counts them, so it never goes back
    cleared: u64,
}

/// A question's durable changes: its filing, its messages and refusals, its close.
fn changes(q: &Question) -> u64 {
    1 + q.thread.len() as u64 + q.refused.len() as u64 + u64::from(q.state == State::Closed)
}

/// A user message main has not taken yet (`ask wait`): a clear keeps its question.
pub fn untaken(q: &Question) -> bool {
    q.thread.iter().any(|m| m.by == "user" && !m.taken)
}

/// What a clear left in aires' list and why, as a sentence's end: `` or `; 2 open and 1 answer
/// main has not taken stay`.
pub fn kept_words(open: u64, untaken: u64) -> String {
    let mut parts = Vec::new();
    if open > 0 { parts.push(format!("{open} open")); }
    if untaken > 0 { parts.push(format!("{untaken} {} main has not taken", if untaken == 1 { "answer" } else { "answers" })); }
    if parts.is_empty() { return String::new(); }
    format!("; {} {}", parts.join(" and "), if open + untaken == 1 { "stays" } else { "stay" })
}

impl St {
    fn index(&self, v: &Value) -> Result<usize, String> {
        let id = field(v, "id").filter(|s| !s.is_empty()).ok_or("id is required")?;
        self.qs.iter().position(|q| q.id == id).ok_or_else(|| format!("no question {id}"))
    }

    fn ordered(&self) -> Vec<&Question> {
        let mut qs: Vec<_> = self.qs.iter().collect();
        qs.sort_by_key(|q| (q.state.group(), std::cmp::Reverse(q.created_ms), std::cmp::Reverse(id_number(&q.id))));
        qs
    }

    fn replay(&mut self, v: &Value) {
        if field(v, "op") == Some("cleared") {
            self.last_id = self.last_id.max(v.get("last_id").and_then(Value::as_u64).unwrap_or(0));
            self.cleared = self.cleared.max(v.get("changes").and_then(Value::as_u64).unwrap_or(0));
            return;
        }
        if field(v, "op") == Some("q") {
            if let Some(q) = v.get("q").and_then(Question::from_value) {
                self.last_id = self.last_id.max(id_number(&q.id));
                if let Some(i) = self.qs.iter().position(|old| old.id == q.id) { self.qs[i] = q; }
                else { self.qs.push(q); }
            }
            return;
        }
        let Ok(i) = self.index(v) else { return };
        let q = &mut self.qs[i];
        match field(v, "op").unwrap_or("") {
            "msg" => if let Some(m) = v.get("m").and_then(Msg::from_value) {
                if m.seq as usize == q.thread.len() + 1 {
                    let mut after = q.clone();
                    if apply(&mut after, m).is_ok() { *q = after; }
                }
            },
            "take" => if let Some(seq) = v.get("seq").and_then(Value::as_u64) {
                if let Some(m) = q.thread.iter_mut().find(|m| m.seq as u64 == seq) { m.taken = true; }
            },
            "close" => q.state = next_state(q.state, None).unwrap(),
            "refused" => if let Some(r) = v.get("r").and_then(Refused::from_value) { q.refused.push(r); },
            _ => {},
        }
    }
}

fn id_number(id: &str) -> u64 { id.strip_prefix('Q').and_then(|s| s.parse().ok()).unwrap_or(0) }

impl Asks {
    pub fn memory() -> Arc<Self> {
        // revisions start at 1: a follow at 0 is a follower that has nothing yet
        Arc::new(Self { path: None, machine: String::new(), publish: None, st: Mutex::new(St { rev: 1, ..St::default() }), changed: Condvar::new(), gate: OnceLock::new() })
    }

    pub fn open(path: PathBuf, machine: &str, publish: Publish) -> Arc<Self> {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("cannot read questions at {}: {e}", path.display()),
        };
        let mut st = St::default();
        for line in bytes.split(|b| *b == b'\n') {
            if let Ok(v) = parse(line) { st.replay(&v); }
        }
        // Reconstruct the revision from durable changes so compaction does not reset followers;
        // revisions start at 1 (a follow at 0 gets the list at once, so an empty store at 0 would
        // answer its follower at once, again and again).
        st.rev = 1 + st.cleared + st.qs.iter().map(changes).sum::<u64>();
        let asks = Arc::new(Self { path: Some(path), machine: machine.into(), publish: Some(publish), st: Mutex::new(st), changed: Condvar::new(), gate: OnceLock::new() });
        asks.compact().unwrap_or_else(|e| panic!("cannot compact questions: {e}"));
        asks
    }

    /// The intake's gate binary, which makes and decides a review card's intake items (gate.rs).
    pub fn set_gate(&self, gate: Arc<dyn Gate>) {
        let _ = self.gate.set(gate);
    }

    fn compact(&self) -> std::io::Result<()> {
        self.rewrite(&self.st.lock().unwrap())
    }

    /// The store as one line per question, and first, once a clear moved some to the archive,
    /// the id count and changes they took along.
    fn rewrite(&self, st: &St) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(dir)?; }
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".new");
        let tmp = PathBuf::from(tmp);
        let mut f = std::fs::File::create(&tmp)?;
        let highest = st.qs.iter().map(|q| id_number(&q.id)).max().unwrap_or(0);
        if st.cleared > 0 || st.last_id > highest {
            writeln!(f, "{}", obj(vec![("op", s("cleared")), ("last_id", number(st.last_id)), ("changes", number(st.cleared))]).to_json())?;
        }
        for q in &st.qs {
            writeln!(f, "{}", obj(vec![("op", s("q")), ("q", q.to_value())]).to_json())?;
        }
        f.sync_data()?;
        std::fs::rename(tmp, path)
    }

    /// Where a clear keeps the questions it takes from the list: beside the store
    /// (`ask-archive.jsonl`), one line each, the question whole with when it was cleared.
    fn archive(&self) -> Option<PathBuf> {
        let path = self.path.as_ref()?;
        let stem = path.file_stem()?.to_str()?;
        Some(path.with_file_name(format!("{stem}-archive.jsonl")))
    }

    /// An archived question by its id (its last line: a clear cut short by a crash may have
    /// archived it once before), and when it was cleared.
    fn archived(&self, id: &str) -> Option<(Question, u64)> {
        let text = std::fs::read(self.archive()?).ok()?;
        text.split(|b| *b == b'\n').filter_map(|line| parse(line).ok()).filter_map(|v| {
            let q = v.get("q").and_then(Question::from_value)?;
            (q.id == id).then(|| (q, v.get("cleared_ms").and_then(Value::as_u64).unwrap_or(0)))
        }).last()
    }

    fn append(&self, v: Value) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(f, "{}", v.to_json())?;
            f.sync_data()
        };
        write().map_err(|e| format!("cannot save questions: {e}"))
    }

    fn changed(&self, st: &mut St) {
        st.rev += 1;
        self.changed.notify_all();
    }

    fn publish(&self, sev: Sev, line: String) {
        if let Some(publish) = &self.publish { publish(hub_record(&self.machine, "", 0, "ask", sev, line, String::new())); }
    }

    pub fn serve(&self, line: &str, judge: &dyn Fn() -> Judged, out: &mut Vec<String>) {
        self.serve_with(line, judge, &|| false, out)
    }

    /// [`Asks::serve`], with `child`: whether the asking process is the master's own child (the
    /// gate's, claiming a press).
    pub fn serve_with(&self, line: &str, judge: &dyn Fn() -> Judged, child: &dyn Fn() -> bool, out: &mut Vec<String>) {
        if let Err(e) = self.request(line, judge, child, out) { out.push(format!("{{\"err\":{}}}", json_str(&e))); }
    }

    fn request(&self, line: &str, judge: &dyn Fn() -> Judged, child: &dyn Fn() -> bool, out: &mut Vec<String>) -> Result<(), String> {
        let (verb, body) = line.trim().split_once(' ').ok_or("expected a verb and a JSON object")?;
        let v = parse(body.trim().as_bytes()).map_err(|e| format!("invalid JSON: {e}"))?;
        if !matches!(v, Value::Obj(_)) { return Err("expected a JSON object".into()); }
        match verb {
            "decide" => return self.decide(&v, judge, out),
            "claim" => return self.claim(&v, child, out),
            _ => {}
        }
        // (a card's intake items come from the gate binary, outside the lock)
        let filed = if verb == "new" { Some(self.file(&v)?) } else { None };
        let mut st = self.st.lock().unwrap();
        match verb {
            "new" => {
                let n = st.qs.iter().map(|q| id_number(&q.id)).fold(st.last_id, u64::max).checked_add(1).ok_or("question id overflow")?;
                let (mut q, skipped) = filed.ok_or("no question")?;
                q.id = format!("Q{n}");
                q.created_ms = wall_ms();
                self.append(obj(vec![("op", s("q")), ("q", q.to_value())]))?;
                let line = format!("ask: {} from {}: {}{}", q.id, q.from, q.title, if q.items.is_empty() { String::new() } else { format!(" ({} items)", q.items.len()) });
                out.push(obj(vec![("id", s(&q.id)), ("skipped", Value::Arr(skipped.iter().map(s).collect()))]).to_json());
                st.last_id = n;
                st.qs.push(q);
                self.changed(&mut st);
                drop(st);
                self.publish(Sev::Info, line);
            }
            "list" => {
                let open = flag(&v, "open")?;
                for q in st.ordered().into_iter().filter(|q| !open || q.state.group() == 0) { out.push(q.summary().to_json()); }
            }
            "show" => match st.index(&v) {
                Ok(i) => out.push(st.qs[i].to_value().to_json()),
                // a cleared question is in the archive, whole
                Err(e) => {
                    let (q, cleared_ms) = field(&v, "id").and_then(|id| self.archived(id)).ok_or(e)?;
                    let Value::Obj(mut fields) = q.to_value() else { unreachable!("a question is an object") };
                    fields.push(("cleared_ms".into(), number(cleared_ms)));
                    out.push(Value::Obj(fields).to_json());
                }
            },
            // The user's: answered and closed questions main has taken every message of leave the
            // list for the archive. Open ones and untaken answers stay. `dry`: the counts alone.
            "clear" => {
                let dry = flag(&v, "dry")?;
                let gone = |q: &Question| q.state.group() != 0 && !untaken(q);
                let n = st.qs.iter().filter(|q| gone(q)).count() as u64;
                let open = st.qs.iter().filter(|q| q.state.group() == 0).count() as u64;
                let kept = st.qs.iter().filter(|q| q.state.group() != 0 && untaken(q)).count() as u64;
                let said = obj(vec![("cleared", number(n)), ("open", number(open)), ("untaken", number(kept))]).to_json();
                if dry || n == 0 {
                    out.push(said);
                    return Ok(());
                }
                let judged = judge();
                if judged.source == Source::Refused {
                    drop(st);
                    self.publish(Sev::Warn, format!("ask: a clear of the list was refused: {} ({})", judged.why, judged.chain));
                    return Err(format!("refused: {}", judged.why));
                }
                let at = wall_ms();
                if let Some(archive) = self.archive() {
                    let lines: String = st.qs.iter().filter(|q| gone(q)).map(|q| obj(vec![("op", s("q")), ("q", q.to_value()), ("cleared_ms", number(at))]).to_json() + "\n").collect();
                    let write = || -> std::io::Result<()> {
                        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&archive)?;
                        f.write_all(lines.as_bytes())?;
                        f.sync_data()
                    };
                    write().map_err(|e| format!("cannot archive questions at {}: {e}", archive.display()))?;
                }
                let (cleared, kept_qs): (Vec<Question>, Vec<Question>) = std::mem::take(&mut st.qs).into_iter().partition(|q| gone(q));
                st.last_id = cleared.iter().chain(&kept_qs).map(|q| id_number(&q.id)).fold(st.last_id, u64::max);
                // (and the clear: the revision rebuilt at open is the one followers saw last)
                st.cleared += cleared.iter().map(changes).sum::<u64>() + 1;
                st.qs = kept_qs;
                // (archived first: a crash before this leaves them in both, the list's copy shown)
                self.rewrite(&st).map_err(|e| format!("cannot save questions: {e}"))?;
                self.changed(&mut st);
                drop(st);
                out.push(said);
                self.publish(Sev::Info, format!("ask: the user cleared {n} answered and closed questions to the archive ({}){}", judged.source.word(), kept_words(open, kept)));
            }
            "answer" | "reply" => {
                let i = st.index(&v)?;
                let q = &st.qs[i];
                if q.state == State::Closed { return Err(format!("{} is closed", q.id)); }
                let mut m = if verb == "answer" { answer_message(&v, Some(q.options.len()))? } else {
                    let text = required(&v, "text")?;
                    let kind = if flag(&v, "reopen")? { "reopen" } else { "reply" };
                    Msg { by: "main".into(), kind: kind.into(), text, source: "main".into(), ..Msg::default() }
                };
                next_state(q.state, Some(&m))?;
                let id = q.id.clone();
                if verb == "answer" {
                    let judged = judge();
                    if judged.source == Source::Refused { return self.refuse(st, i, "", judged.chain, judged.why); }
                    m.source = judged.source.word().into();
                    m.chain = judged.chain;
                }
                let line = match (verb, m.kind.as_str()) {
                    ("answer", kind) => Some(format!("ask: {id} {} ({})", if kind == "ask" { "asked a follow-up" } else { "answered" }, m.source)),
                    (_, "reopen") => Some(format!("ask: main reopened {id} with a reply: the user's turn again")),
                    _ => None,
                };
                let seq = self.record(&mut st, i, m)?;
                out.push(obj(vec![("ok", Value::Bool(true)), ("seq", number(seq as u64))]).to_json());
                drop(st);
                if let Some(line) = line { self.publish(Sev::Info, line); }
            }
            "close" => {
                let i = st.index(&v)?;
                if st.qs[i].state != State::Closed {
                    self.append(obj(vec![("op", s("close")), ("id", s(&st.qs[i].id)), ("at_ms", number(wall_ms()))]))?;
                    st.qs[i].state = next_state(st.qs[i].state, None)?;
                    self.changed(&mut st);
                }
                out.push("{\"ok\":true}".into());
            }
            "wait" => {
                let any = flag(&v, "any")?;
                let ids = match v.get("ids") { Some(ids) => strings(ids).ok_or("ids must be strings")?, None => Vec::new() };
                if any && !ids.is_empty() || !any && ids.is_empty() { return Err("wait needs ids or any:true".into()); }
                for id in &ids {
                    if !st.qs.iter().any(|q| q.id == *id) { return Err(format!("no question {id}")); }
                }
                loop {
                    if makepad_remote::lines::client_gone() { return Ok(()); }
                    let mut ready = Vec::new();
                    for (i, q) in st.qs.iter().enumerate().filter(|(_, q)| any || ids.contains(&q.id)) {
                        for (j, m) in q.thread.iter().enumerate().filter(|(_, m)| m.by == "user" && !m.taken) {
                            ready.push((m.at_ms, i, j));
                        }
                    }
                    ready.sort_unstable();
                    if !ready.is_empty() {
                        for (_, i, j) in ready {
                            let q = &mut st.qs[i];
                            self.append(obj(vec![("op", s("take")), ("id", s(&q.id)), ("seq", number(q.thread[j].seq as u64))]))?;
                            q.thread[j].taken = true;
                            out.push(q.delivery(&q.thread[j]).to_json());
                        }
                        return Ok(());
                    }
                    st = self.changed.wait_timeout(st, RECHECK).unwrap().0;
                }
            }
            "follow" => {
                let rev = v.get("rev").and_then(Value::as_u64).ok_or("rev must be a non-negative integer")?;
                loop {
                    if makepad_remote::lines::client_gone() { return Ok(()); }
                    // any other revision (also one from before a lost file) gets the whole list
                    if rev == 0 || st.rev != rev {
                        out.push(obj(vec![("rev", number(st.rev))]).to_json());
                        out.extend(st.ordered().into_iter().map(|q| q.to_value().to_json()));
                        return Ok(());
                    }
                    st = self.changed.wait_timeout(st, RECHECK).unwrap().0;
                }
            }
            _ => return Err("ask new | list | show | answer | decide | reply | close | clear | wait | follow | claim".into()),
        }
        Ok(())
    }

    /// A message into question `i`, appended first: its sequence, its time, and [`apply`].
    fn record(&self, st: &mut St, i: usize, mut m: Msg) -> Result<u32, String> {
        let q = &st.qs[i];
        m.seq = u32::try_from(q.thread.len()).ok().and_then(|n| n.checked_add(1)).ok_or("message sequence overflow")?;
        m.at_ms = wall_ms();
        let mut after = q.clone();
        apply(&mut after, m.clone())?;
        self.append(obj(vec![("op", s("msg")), ("id", s(&after.id)), ("m", m.to_value())]))?;
        st.qs[i] = after;
        self.changed(st);
        Ok(m.seq)
    }

    /// A refused answer or press, kept on its question (and its item) and said as a warning.
    fn refuse(&self, mut st: std::sync::MutexGuard<'_, St>, i: usize, item: &str, chain: String, why: String) -> Result<(), String> {
        let id = st.qs[i].id.clone();
        let r = Refused { at_ms: wall_ms(), chain, why, item: item.into() };
        self.append(obj(vec![("op", s("refused")), ("id", s(&id)), ("r", r.to_value())]))?;
        let line = match item {
            "" => format!("ask: an answer to {id} was refused: {} ({})", r.why, r.chain),
            item => format!("ask: a press on {item} of {id} was refused: {} ({})", r.why, r.chain),
        };
        let err = format!("refused: {}", r.why);
        st.qs[i].refused.push(r);
        self.changed(&mut st);
        drop(st);
        self.publish(Sev::Warn, line);
        Err(err)
    }

    /// A question to file (`new`): its card, with its items from the gate binary (`intake`: tags,
    /// or `--all`) or main's own (`items`), main's `plans` under their Yes, and the tags the gate
    /// skipped, each with why.
    fn file(&self, v: &Value) -> Result<(Question, Vec<String>), String> {
        let mut q = new_question(v)?;
        let intake = match v.get("intake") { None => None, Some(t) => Some(strings(t).ok_or("intake must be item names")?) };
        let own = match v.get("items") {
            None => Vec::new(),
            Some(list) => list.as_arr().ok_or("items must be a list")?.iter().enumerate().map(|(n, it)| Item::from_main(it, n + 1)).collect::<Result<Vec<_>, _>>()?,
        };
        let mut skipped = Vec::new();
        q.items = match intake {
            Some(_) if !own.is_empty() => return Err("a review card takes intake items or main's own, not both".into()),
            Some(tags) => {
                if tags.is_empty() { return Err("intake names no item".into()); }
                let gate = self.gate.get().ok_or("intake items are filed on the master with the private checkout: no gate binary here")?;
                let mut items: Vec<Item> = Vec::new();
                for line in gate.card(&tags)? {
                    match field(&line, "skip") {
                        Some(tag) => skipped.push(format!("{tag}: {}", field(&line, "why").unwrap_or("skipped"))),
                        None => {
                            let item = Item::from_gate(&line)?;
                            if items.iter().any(|i| i.key == item.key) { return Err(format!("the gate gave {} twice", item.key)); }
                            items.push(item);
                        }
                    }
                }
                if items.is_empty() {
                    return Err(format!("nothing to review: {}", if skipped.is_empty() { "no item is in doubt".to_string() } else { skipped.join("; ") }));
                }
                items
            }
            None => own,
        };
        match v.get("plans") {
            None => {}
            Some(Value::Obj(plans)) => for (key, text) in plans {
                let text = text.as_str().ok_or("a plan is text")?;
                let item = q.items.iter_mut().find(|i| i.key == *key).ok_or_else(|| format!("a plan names no item of the card: {key}"))?;
                item.plan = text.into();
            },
            Some(_) => return Err("plans must be an object of item: text".into()),
        }
        if !q.items.is_empty() && !q.options.is_empty() { return Err("a review card has items, not options".into()); }
        Ok((q, skipped))
    }

    /// A Yes or No pressed on a review card's item: `{id, item, row, yes}`, `row` the one the card
    /// showed. Main's own item takes the user's answer as `answer` does. An intake item takes the
    /// tray's press alone, on the row the card shows now, and the gate decides it: the master
    /// holds the press under a nonce its child claims (`claim`), and the child's answer is the
    /// item decided, or (a row that moved) the item as it stands now.
    fn decide(&self, v: &Value, judge: &dyn Fn() -> Judged, out: &mut Vec<String>) -> Result<(), String> {
        let key = required(v, "item")?;
        let row = text_field(v, "row")?;
        let yes = v.get("yes").and_then(Value::as_bool).ok_or("yes must be true or false")?;
        let word = if yes { "yes" } else { "no" };
        let mut st = self.st.lock().unwrap();
        let i = st.index(v)?;
        let q = &st.qs[i];
        let id = q.id.clone();
        if q.state == State::Closed { return Err(format!("{id} is closed")); }
        let item = q.items.iter().find(|it| it.key == key).cloned().ok_or_else(|| format!("{id} has no item {key}"))?;
        if !item.decided.is_empty() { return Err(format!("{key} is decided already ({})", item.decided)); }
        if st.deciding.contains(&(id.clone(), key.clone())) { return Err(format!("a press on {key} is on its way")); }
        let judged = judge();
        let refused = match judged.source {
            Source::Refused => Some(judged.why.clone()),
            Source::Cli if item.gate => Some(format!("an intake item is decided by a press in aires; at a terminal it is `makepad devhub intake doubt {key} clear|decline <row id>`, with the row id `intake ready` shows")),
            _ => None,
        };
        if let Some(why) = refused { return self.refuse(st, i, &key, judged.chain, why); }
        if row != item.row { return Err(format!("{key} is not the row you pressed on any more: the card changed, look at it again")); }
        if !item.gate {
            let card = Item { decided: word.into(), at_ms: wall_ms(), ..item };
            let m = Msg { by: "user".into(), kind: word.into(), source: judged.source.word().into(), chain: judged.chain, item: key.clone(), card: Some(card), ..Msg::default() };
            let source = m.source.clone();
            let seq = self.record(&mut st, i, m)?;
            drop(st);
            out.push(obj(vec![("ok", Value::Bool(true)), ("seq", number(seq as u64)), ("stale", Value::Bool(false))]).to_json());
            self.publish(Sev::Info, format!("ask: {id} {key}: the user said {word} ({source})"));
            return Ok(());
        }
        let gate = self.gate.get().cloned().ok_or("no gate binary here to decide an intake item")?;
        let nonce = nonce()?;
        st.pending.insert(nonce.clone(), Claimed { id: id.clone(), item: key.clone(), row: row.clone(), yes, source: judged.source.word().into(), chain: judged.chain.clone() });
        st.deciding.insert((id.clone(), key.clone()));
        drop(st);
        // (the gate's child claims the press on a connection of its own while this one waits)
        let answer = gate.decide(&nonce);
        let mut st = self.st.lock().unwrap();
        let unclaimed = st.pending.remove(&nonce).is_some();
        st.deciding.remove(&(id.clone(), key.clone()));
        let i = st.index(v)?;
        let done = if yes { "cleared" } else { "declined" };
        let card = match answer {
            Ok(_) if unclaimed => Err("the gate answered without claiming the press".to_string()),
            Ok(a) => match (field(&a, "err"), a.get("stale")) {
                (Some(e), _) => Err(e.to_string()),
                (None, Some(now)) => Item::from_gate(now).and_then(|fresh| if fresh.key == key { Ok(fresh) } else { Err(format!("the gate answered for {} on a press on {key}", fresh.key)) })
                    .map(|fresh| Item { plan: item.plan.clone(), note: format!("It changed after you saw it, so nothing was {done}. This is how it stands now."), at_ms: wall_ms(), ..fresh }),
                (None, None) if field(&a, "ok") == Some(done) && field(&a, "row") == Some(row.as_str()) => Ok(Item {
                    decided: word.into(),
                    note: if yes { "Cleared: it goes on to its fix lane." } else { "Declined: main replies and closes it." }.into(),
                    at_ms: wall_ms(),
                    ..item.clone()
                }),
                _ => Err("the gate's answer is off its form".into()),
            },
            Err(e) => Err(e),
        };
        let card = match card {
            Ok(card) => card,
            Err(e) => return self.refuse(st, i, &key, judged.chain, format!("Not {done}: {e}")),
        };
        let stale = card.decided.is_empty();
        let m = Msg { by: "user".into(), kind: word.into(), text: if stale { "stale".into() } else { done.into() }, source: judged.source.word().into(), chain: judged.chain, item: key.clone(), card: Some(card), ..Msg::default() };
        let seq = self.record(&mut st, i, m)?;
        drop(st);
        out.push(obj(vec![("ok", Value::Bool(true)), ("seq", number(seq as u64)), ("stale", Value::Bool(stale))]).to_json());
        self.publish(Sev::Info, match stale {
            true => format!("ask: {id} {key}: the user's {word} came on a row that moved since; nothing was {done}, the card shows it anew (tray)"),
            false => format!("ask: {id} {key}: the user's {word}, {done} (tray)"),
        });
        Ok(())
    }

    /// The gate's child claims the press the master holds under its nonce: once, and only from the
    /// master's own child.
    fn claim(&self, v: &Value, child: &dyn Fn() -> bool, out: &mut Vec<String>) -> Result<(), String> {
        let nonce = required(v, "nonce")?;
        if !child() {
            self.publish(Sev::Warn, "ask: a claim of a press from a process that is not the master's own child was refused".into());
            return Err("a press is claimed by the master's own child alone".into());
        }
        let c = self.st.lock().unwrap().pending.remove(&nonce).ok_or("no press waits under that nonce (each is claimed once)")?;
        out.push(c.to_value().to_json());
        Ok(())
    }
}

/// A press's nonce: 128 bits of the OS's randomness.
fn nonce() -> Result<String, String> {
    let mut b = [0u8; 16];
    makepad_network::tls::os_random(&mut b).map_err(|e| format!("no OS randomness for a press's nonce: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn required(v: &Value, key: &str) -> Result<String, String> {
    field(v, key).filter(|s| !s.trim().is_empty()).map(str::to_string).ok_or_else(|| format!("{key} is required"))
}

fn text_field(v: &Value, key: &str) -> Result<String, String> {
    match v.get(key) {
        None => Ok(String::new()),
        Some(v) => v.as_str().map(str::to_string).ok_or_else(|| format!("{key} must be text")),
    }
}

fn flag(v: &Value, key: &str) -> Result<bool, String> {
    match v.get(key) { None => Ok(false), Some(v) => v.as_bool().ok_or_else(|| format!("{key} must be a boolean")) }
}

fn new_question(v: &Value) -> Result<Question, String> {
    Ok(Question {
        id: String::new(), from: required(v, "from")?, title: required(v, "title")?,
        decided: text_field(v, "decided")?, why: text_field(v, "why")?,
        options: match v.get("options") { None => Vec::new(), Some(v) => strings(v).ok_or("options must be strings")? },
        recommend: text_field(v, "recommend")?, blocked: text_field(v, "blocked")?, context: text_field(v, "context")?,
        created_ms: 0, state: State::Open, thread: Vec::new(), refused: Vec::new(), items: Vec::new(),
    })
}

fn answer_message(v: &Value, options: Option<usize>) -> Result<Msg, String> {
    let choice = match v.get("choice") { None => None, Some(v) => choice(v).ok_or("choice must be an option number or null")? };
    let (decide, ask, text) = (flag(v, "decide")?, flag(v, "ask")?, text_field(v, "text")?);
    if ask {
        if text.trim().is_empty() || choice.is_some() || decide { return Err("a follow-up needs text and no choice or decide".into()); }
    } else {
        if choice.is_some_and(|n| n == 0 || options.is_some_and(|len| n as usize > len)) {
            return Err("choice must name an option in 1..=options.len()".into());
        }
        if choice.is_none() && !decide && text.trim().is_empty() { return Err("an answer needs a choice, decide or text".into()); }
    }
    Ok(Msg { by: "user".into(), kind: if ask { "ask" } else { "answer" }.into(), choice, decide, text, ..Msg::default() })
}

pub fn register(ep: &makepad_remote::endpoint::Endpoint, asks: Arc<Asks>) {
    ep.register("ask", 1, makepad_remote::lines::service("ask", Arc::new(move |line, out| {
        let pid = makepad_remote::lines::asked_on().map_or(0, |c| c.peer().pid);
        asks.serve_with(line, &|| crate::askpeer::judge(pid), &|| own_child(pid), out);
    })));
}

/// Whether `pid` is this process's own child (the gate's `decide`, which the master started), as
/// the OS says: a socket's credentials name the pid, no client can claim it.
fn own_child(pid: u32) -> bool {
    pid != 0 && makepad_remote::procs::process(pid).is_some_and(|p| p.ppid == std::process::id())
}

/// How long a `wait` or `follow` sleeps before it looks again unwoken: whether its client is
/// gone. A change wakes it at once ([`Asks::changed`]).
const RECHECK: Duration = Duration::from_secs(60);

pub const HELP: &str = "  ask new --from <who> --title <t>         file a question for the user (DEVHUB_AGENT=main only)
                                          [--decided <t>] [--why <t>] [--option <t>]... [--recommend <t>]
                                          [--blocked <t>] [--context <t> | --context-file <path>]
                                          a review card instead of options, a Yes and a No per item:
                                          [--intake <item>,... | --intake all]  the intake's items in
                                          doubt, their lines and rows from the gate binary; a press is
                                          the tray's alone, and its Yes clears, its No declines the row
                                          the card showed (main closes it after a No)
                                          [--item <head>... | --items-file <jsonl>]  main's own items,
                                          {head, what, why, risks, yes, no} a line; a press is the
                                          user's answer per item
                                          [--plan <item>=<text>]...  main's line under an item's Yes
  ask list [--open] [--json]               questions, open and waiting first, then answered and closed
  ask show <id> [--json]                  the question, its context, thread and refused answer attempts
  ask answer <id> [--choice <n> | --decide] [--text <t>] [--ask]
                                          the user's answer, typed at a terminal or pressed in aires;
                                          refused in an agent's environment. --ask asks a follow-up
  ask answer <id> --item <n> --yes|--no   the user's Yes or No on main's own item of a review card, at a
                                          terminal (an intake item's is pressed in aires, or typed as
                                          `intake doubt <item> clear|decline <row id>`)
  ask reply <id> [--reopen] <text...>     main's reply to the user's follow-up; --reopen (DEVHUB_AGENT=main
                                          only) gives the question back to the user whatever its state
                                          (an answer that was a question), unless it is closed
  ask close <id>                          close a question, keeping its thread
  ask clear                               the user's, after a confirm: answered and closed questions
                                          leave aires' list for the archive beside the store, where
                                          `ask show` still finds them; open questions and answers
                                          main has not taken stay. Refused in an agent's environment
  ask wait (--any | <id>...) [--json]     wait for untaken user messages and take them, oldest first";

fn argument<'a>(args: &[&'a str], i: &mut usize) -> Result<&'a str, String> {
    let flag = args[*i];
    *i += 1;
    args.get(*i).copied().filter(|v| !v.starts_with("--")).ok_or_else(|| format!("{flag} needs a value"))
}

/// Parse flags without I/O. The CLI resolves context_file to context before sending the line.
pub fn new_request(args: &[&str]) -> Result<String, String> {
    let mut fields = Vec::new();
    let (mut options, mut items, mut plans) = (Vec::new(), Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        let key = match args[i] {
            "--from" => "from", "--title" => "title", "--decided" => "decided", "--why" => "why",
            "--option" => "options", "--recommend" => "recommend", "--blocked" => "blocked",
            "--context" => "context", "--context-file" => "context_file",
            "--item" => "items", "--items-file" => "items_file", "--intake" => "intake", "--plan" => "plans",
            word => return Err(format!("unknown new argument {word}")),
        };
        let value = argument(args, &mut i)?;
        match key {
            "options" => options.push(s(value)),
            "items" => items.push(obj(vec![("head", s(value))])),
            "plans" => {
                let (item, text) = value.split_once('=').filter(|(k, t)| !k.is_empty() && !t.trim().is_empty()).ok_or("--plan is <item>=<text>")?;
                if plans.iter().any(|(k, _): &(String, Value)| k == item) { return Err(format!("the plan of {item} was given twice")); }
                plans.push((item.to_string(), s(text)));
            }
            _ => {
                if fields.iter().any(|(k, _)| *k == key) { return Err(format!("{key} was given twice")); }
                let value = match key {
                    "intake" if value == "all" => Value::Arr(vec![s("--all")]),
                    "intake" => Value::Arr(value.split(',').map(str::trim).filter(|t| !t.is_empty()).map(s).collect()),
                    _ => s(value),
                };
                fields.push((key, value));
            }
        }
        i += 1;
    }
    fields.push(("options", Value::Arr(options)));
    if !items.is_empty() { fields.push(("items", Value::Arr(items))); }
    let mut v = obj(fields);
    if !plans.is_empty() {
        if let Value::Obj(fields) = &mut v { fields.push(("plans".into(), Value::Obj(plans))); }
    }
    if v.get("context").is_some() && v.get("context_file").is_some() { return Err("use --context or --context-file, not both".into()); }
    let given = ["intake", "items", "items_file"].iter().filter(|k| v.get(k).is_some()).count();
    if given > 1 { return Err("use one of --intake, --item and --items-file".into()); }
    if given > 0 && v.get("options").and_then(Value::as_arr).is_some_and(|o| !o.is_empty()) { return Err("a review card has items, not options".into()); }
    if v.get("plans").is_some() && given == 0 { return Err("--plan names an item of a review card".into()); }
    new_question(&v)?;
    Ok(format!("new {}", v.to_json()))
}

/// `ask answer <id> --item <n> --yes|--no`: the user's Yes or No on main's own item, at a terminal.
pub fn decide_request(args: &[&str]) -> Result<String, String> {
    let id = args.first().filter(|id| !id.is_empty() && !id.starts_with('-')).ok_or("answer needs an id")?;
    let (mut item, mut yes) = (None, None);
    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "--item" if item.is_none() => item = Some(argument(args, &mut i)?),
            "--yes" | "--no" if yes.is_none() => yes = Some(args[i] == "--yes"),
            word => return Err(format!("unknown or repeated answer argument {word} (an item's answer is --item <n> --yes|--no)")),
        }
        i += 1;
    }
    let (Some(item), Some(yes)) = (item, yes) else { return Err("an item's answer is --item <n> --yes|--no".into()) };
    Ok(format!("decide {}", obj(vec![("id", s(*id)), ("item", s(item)), ("row", s("")), ("yes", Value::Bool(yes))]).to_json()))
}

pub fn answer_request(args: &[&str]) -> Result<String, String> {
    let id = args.first().filter(|id| !id.is_empty() && !id.starts_with('-')).ok_or("answer needs an id")?;
    let mut fields = vec![("id", s(*id))];
    let mut i = 1;
    while i < args.len() {
        let (key, value) = match args[i] {
            "--choice" => {
                let n = argument(args, &mut i)?.parse::<u32>().map_err(|_| "--choice needs an option number")?;
                ("choice", number(n as u64))
            }
            "--decide" => ("decide", Value::Bool(true)),
            "--ask" => ("ask", Value::Bool(true)),
            "--text" => ("text", s(argument(args, &mut i)?)),
            word => return Err(format!("unknown answer argument {word}")),
        };
        if fields.iter().any(|(k, _)| *k == key) { return Err(format!("{key} was given twice")); }
        fields.push((key, value));
        i += 1;
    }
    let v = obj(fields);
    if v.get("choice").is_some() && flag(&v, "decide")? { return Err("use --choice or --decide, not both".into()); }
    answer_message(&v, None)?;
    Ok(format!("answer {}", v.to_json()))
}

fn cli_request(a: &[&str]) -> Result<(String, bool), String> {
    match a {
        ["new", rest @ ..] => {
            if std::env::var("DEVHUB_AGENT").as_deref() != Ok("main") {
                return Err("only main files questions (DEVHUB_AGENT=main); a lane sends its question to main".into());
            }
            let line = new_request(rest)?;
            let mut v = parse(line[4..].as_bytes()).map_err(str::to_string)?;
            if let Some(path) = field(&v, "context_file") {
                let context = std::fs::read_to_string(path).map_err(|e| format!("cannot read context file {path}: {e}"))?;
                if let Value::Obj(fields) = &mut v {
                    fields.retain(|(key, _)| key != "context_file");
                    fields.push(("context".into(), s(context)));
                }
            }
            if let Some(path) = field(&v, "items_file") {
                let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read items file {path}: {e}"))?;
                let items = text.lines().filter(|l| !l.trim().is_empty()).enumerate()
                    .map(|(n, l)| parse(l.as_bytes()).map_err(|e| format!("{path}:{}: {e}", n + 1)))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Value::Obj(fields) = &mut v {
                    fields.retain(|(key, _)| key != "items_file");
                    fields.push(("items".into(), Value::Arr(items)));
                }
            }
            Ok((format!("new {}", v.to_json()), false))
        }
        ["answer", rest @ ..] => {
            crate::cli::user_at_terminal()?;
            if rest.contains(&"--item") { return Ok((decide_request(rest)?, false)); }
            Ok((answer_request(rest)?, false))
        }
        ["list", rest @ ..] if rest.iter().all(|a| matches!(*a, "--open" | "--json")) => {
            Ok((format!("list {}", obj(vec![("open", Value::Bool(rest.contains(&"--open")))]).to_json()), rest.contains(&"--json")))
        }
        ["show", id, rest @ ..] if rest.is_empty() || *rest == ["--json"] => {
            Ok((format!("show {}", obj(vec![("id", s(*id))]).to_json()), !rest.is_empty()))
        }
        ["reply", id, "--reopen", words @ ..] if !words.is_empty() && !words.join(" ").trim().is_empty() => {
            if std::env::var("DEVHUB_AGENT").as_deref() != Ok("main") {
                return Err("only main reopens a question (DEVHUB_AGENT=main); a lane sends its reply to main".into());
            }
            Ok((format!("reply {}", obj(vec![("id", s(*id)), ("text", s(words.join(" "))), ("reopen", Value::Bool(true))]).to_json()), false))
        }
        ["reply", id, words @ ..] if !words.is_empty() && !words.join(" ").trim().is_empty() => {
            Ok((format!("reply {}", obj(vec![("id", s(*id)), ("text", s(words.join(" ")))]).to_json()), false))
        }
        ["close", id] => Ok((format!("close {}", obj(vec![("id", s(*id))]).to_json()), false)),
        ["wait", rest @ ..] => {
            let mut ids = Vec::new();
            let mut any = false;
            let mut json = false;
            for word in rest {
                match *word {
                    "--any" => any = true,
                    "--json" => json = true,
                    word if word.starts_with('-') => return Err(format!("unknown wait argument {word}")),
                    id => ids.push(s(id)),
                }
            }
            if any && !ids.is_empty() || !any && ids.is_empty() { return Err("wait needs --any or question ids".into()); }
            let v = if any { obj(vec![("any", Value::Bool(true))]) } else { obj(vec![("ids", Value::Arr(ids))]) };
            Ok((format!("wait {}", v.to_json()), json))
        }
        _ => Err(format!("usage:\n{HELP}")),
    }
}

/// `ask clear`: the user's, at a terminal. It says what would go and what stays, and clears
/// once they say yes.
fn clear_cli() -> i32 {
    if let Err(e) = crate::cli::user_at_terminal() {
        eprintln!("makepad devhub ask clear: {e}");
        return 2;
    }
    let request = |line: &str| -> Result<(u64, u64, u64), String> {
        let c = crate::cli::control()?;
        let lines = crate::cli::ask(&c, None, "ask", line)?;
        let v = parse(lines.first().ok_or("no response")?.as_bytes()).map_err(|e| format!("invalid response: {e}"))?;
        if let Some(e) = field(&v, "err") { return Err(e.to_string()); }
        let n = |key| v.get(key).and_then(Value::as_u64).unwrap_or(0);
        Ok((n("cleared"), n("open"), n("untaken")))
    };
    let (n, open, untaken) = match request(r#"clear {"dry":true}"#) {
        Ok(counts) => counts,
        Err(e) => { eprintln!("makepad devhub ask clear: {e}"); return 2; }
    };
    if n == 0 {
        println!("nothing to clear{}", kept_words(open, untaken));
        return 0;
    }
    print!("clear {n} answered and closed question(s) from aires' list to the archive{}? [y/N] ", kept_words(open, untaken));
    let _ = std::io::stdout().flush();
    let mut yes = String::new();
    if std::io::stdin().read_line(&mut yes).is_err() || !matches!(yes.trim(), "y" | "Y" | "yes") {
        println!("not cleared");
        return 1;
    }
    match request("clear {}") {
        Ok((n, open, untaken)) => {
            println!("cleared {n}{}; `ask show <id>` finds them in the archive", kept_words(open, untaken));
            0
        }
        Err(e) => { eprintln!("makepad devhub ask clear: {e}"); 1 }
    }
}

pub fn cli(a: &[&str]) -> i32 {
    if a == ["clear"] { return clear_cli(); }
    let (line, json) = match cli_request(a) {
        Ok(r) => r,
        Err(e) => { eprintln!("makepad devhub ask: {e}"); return 2; }
    };
    let call = || {
        let c = crate::cli::control()?;
        crate::cli::ask(&c, None, "ask", &line)
    };
    let result = if a.first() == Some(&"wait") { crate::cli::retry_master(call) } else { call() };
    let lines = match result {
        Ok(lines) => lines,
        Err(e) => { eprintln!("makepad devhub ask: {e}"); return 2; }
    };
    let mut code = 0;
    for line in lines {
        let v = match parse(line.as_bytes()) {
            Ok(v) => v,
            Err(e) => { eprintln!("makepad devhub ask: invalid response: {e}"); return 2; }
        };
        if let Some(e) = field(&v, "err") {
            eprintln!("makepad devhub ask: {e}");
            code = 1;
        } else if json && a[0] == "show" {
            // (an intake item's review words are the user's to read, in aires)
            match Question::from_value(&v) {
                Some(mut q) => {
                    masked(&mut q);
                    let mut out = q.to_value();
                    if let (Value::Obj(fields), Some(at)) = (&mut out, v.get("cleared_ms")) { fields.push(("cleared_ms".into(), at.clone())); }
                    println!("{}", out.to_json());
                }
                None => { eprintln!("makepad devhub ask: invalid question response"); return 2; }
            }
        } else if json { println!("{line}"); }
        else {
            match a[0] {
                "new" => {
                    println!("{}", field(&v, "id").unwrap_or(""));
                    for skipped in v.get("skipped").and_then(Value::as_arr).into_iter().flatten().filter_map(Value::as_str) {
                        eprintln!("not on the card: {skipped}");
                    }
                }
                "list" => {
                    println!("{}  {:8}  from {}  {} msgs  {}", field(&v, "id").unwrap_or(""), field(&v, "state").unwrap_or(""),
                        field(&v, "from").unwrap_or(""), v.get("msgs").and_then(Value::as_u64).unwrap_or(0), field(&v, "title").unwrap_or(""));
                    let refused = v.get("refused").and_then(Value::as_u64).unwrap_or(0);
                    if refused > 0 { println!("  ({refused} refused answer attempts)"); }
                }
                "show" => match Question::from_value(&v) {
                    Some(mut q) => {
                        masked(&mut q);
                        print!("{}", card(&q));
                        if let Some(at) = v.get("cleared_ms").and_then(Value::as_u64) { println!("cleared from the list to the archive at {} UTC", time(at)); }
                    }
                    None => { eprintln!("makepad devhub ask: invalid question response"); return 2; }
                },
                "wait" if field(&v, "item").is_some_and(|i| !i.is_empty()) => println!("{} {} on {} from the user, source={} ({}): {}",
                    field(&v, "id").unwrap_or(""), field(&v, "kind").unwrap_or(""), field(&v, "item").unwrap_or(""),
                    field(&v, "source").unwrap_or(""), field(&v, "chain").unwrap_or(""), field(&v, "text").unwrap_or("")),
                "wait" => println!("{} {} from the user, source={} ({}): {}", field(&v, "id").unwrap_or(""),
                    if field(&v, "kind") == Some("ask") { "asked a follow-up" } else { "answer" },
                    field(&v, "source").unwrap_or(""), field(&v, "chain").unwrap_or(""),
                    answer_text(v.get("choice").and_then(choice).flatten(), field(&v, "choice_text").unwrap_or(""),
                        v.get("decide").and_then(Value::as_bool).unwrap_or(false), field(&v, "text").unwrap_or(""))),
                _ => println!("{line}"),
            }
        }
    }
    code
}

fn time(at_ms: u64) -> String {
    let secs = at_ms / 1000;
    format!("{:02}:{:02}:{:02}", secs / 3600 % 24, secs / 60 % 60, secs % 60)
}

fn answer_text(choice: Option<u32>, choice_text: &str, decide: bool, text: &str) -> String {
    let mut parts = Vec::new();
    if let Some(n) = choice { parts.push(format!("option {n} {}", json_str(choice_text))); }
    if decide { parts.push("you decide".into()); }
    if !text.is_empty() { parts.push(text.into()); }
    parts.join(" -- ")
}

/// What an agent's `ask show` prints of an intake item's review words: the user reads them in
/// aires, and they stay out of an agent's context by habit.
const IN_AIRES: &str = "(the review's words: in aires)";

fn masked(q: &mut Question) {
    let mask = |i: &mut Item| if i.gate && !i.what.is_empty() { i.what = IN_AIRES.into(); };
    q.items.iter_mut().for_each(mask);
    q.thread.iter_mut().filter_map(|m| m.card.as_mut()).for_each(mask);
}

fn card(q: &Question) -> String {
    use std::fmt::Write;
    let mut out = format!("{} [{}] from {}: {}\n", q.id, q.state.word(), q.from, q.title);
    for (label, text) in [("decided", &q.decided), ("why now", &q.why)] {
        if !text.is_empty() { let _ = writeln!(out, "{label}: {text}"); }
    }
    if !q.options.is_empty() {
        out.push_str("options:\n");
        for (i, text) in q.options.iter().enumerate() { let _ = writeln!(out, "  {}. {text}", i + 1); }
    }
    for (label, text) in [("recommend", &q.recommend), ("blocked", &q.blocked)] {
        if !text.is_empty() { let _ = writeln!(out, "{label}: {text}"); }
    }
    if !q.items.is_empty() {
        let decided = q.items.iter().filter(|i| !i.decided.is_empty()).count();
        let _ = writeln!(out, "items ({decided} of {} decided):", q.items.len());
        for i in &q.items {
            let state = if i.decided.is_empty() { "open".to_string() } else { i.decided.clone() };
            let tag = if i.tag.is_empty() { String::new() } else { format!(" ({})", i.tag) };
            let _ = writeln!(out, "  {} [{state}] {}{tag}", i.key, i.head);
            for (label, text) in [("what", &i.what), ("why", &i.why), ("risks", &i.risks), ("yes", &i.yes), ("plan", &i.plan), ("no", &i.no), ("note", &i.note)] {
                if !text.is_empty() { let _ = writeln!(out, "    {label}: {text}"); }
            }
        }
    }
    if !q.context.is_empty() {
        out.push_str("context:\n");
        for line in q.context.lines() { let _ = writeln!(out, "  {line}"); }
    }
    for m in &q.thread {
        let _ = write!(out, "#{} {} {} {}", m.seq, time(m.at_ms), m.by, m.kind);
        if m.by == "user" && !m.item.is_empty() {
            let _ = writeln!(out, " on {} source={} ({}): {}", m.item, m.source, m.chain, m.text);
        } else if m.by == "user" {
            let _ = writeln!(out, " source={} ({}): {}", m.source, m.chain, answer_text(m.choice, q.choice_text(m), m.decide, &m.text));
        } else { let _ = writeln!(out, ": {}", m.text); }
    }
    if !q.refused.is_empty() {
        let _ = writeln!(out, "refused answer attempts: {}", q.refused.len());
        for r in &q.refused { let _ = writeln!(out, "  {} {}", time(r.at_ms), r.chain); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const NEW: &str = r#"new {"from":"lane","title":"Land the change?","decided":"Review passed","why":"Ready","options":["Land it","Wait"],"recommend":"Land it","blocked":"Landing","context":"First line\nSecond line"}"#;

    fn tray() -> Judged {
        Judged { source: Source::Tray, chain: "pid 4242 makepad-app-aires, from 1 launchd".into(), why: String::new() }
    }

    fn refused() -> Judged {
        Judged { source: Source::Refused, chain: "pid 901 makepad-devhub, from 899 zsh, from 1211 claude".into(), why: "an agent's environment".into() }
    }

    fn call(asks: &Asks, line: &str) -> Vec<Value> {
        let mut out = Vec::new();
        asks.serve(line, &tray, &mut out);
        out.iter().map(|s| parse(s.as_bytes()).expect("service responses are JSON")).collect()
    }

    fn question(asks: &Asks, id: &str) -> Question {
        let out = call(asks, &format!("show {{\"id\":{}}}", json_str(id)));
        Question::from_value(&out[0]).expect("show returns a full question")
    }

    fn state(asks: &Asks) -> State { question(asks, "Q1").state }

    fn blocked_call(asks: &Arc<Asks>, line: String) -> (std::thread::JoinHandle<()>, mpsc::Receiver<Vec<Value>>) {
        let asks = asks.clone();
        let (started, ready) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            started.send(()).unwrap();
            tx.send(call(&asks, &line)).unwrap();
        });
        ready.recv().expect("worker starts");
        (thread, rx)
    }

    #[test]
    fn lifecycle_and_persistence() {
        let dir = std::env::temp_dir().join(format!("devhub-ask-lifecycle-{}", std::process::id()));
        let path = dir.join("ask.jsonl");
        let publish: Publish = Arc::new(|_| {});
        let asks = Asks::open(path.clone(), "", publish.clone());
        assert_eq!(field(&call(&asks, NEW)[0], "id"), Some("Q1"), "first id");
        assert_eq!(state(&asks), State::Open, "new is the user's turn");
        call(&asks, r#"reply {"id":"Q1","text":"More context"}"#);
        assert_eq!(state(&asks), State::Open, "unsolicited reply keeps the state");
        let answer = call(&asks, r#"answer {"id":"Q1","choice":1,"text":"Please land"}"#);
        assert_eq!(answer[0].get("seq").and_then(Value::as_u64), Some(2), "message sequences include main replies");
        assert_eq!(state(&asks), State::Answered, "answer resolves an open question");
        assert_eq!(question(&asks, "Q1").thread[1].source, "tray", "the judge sets provenance");
        let replay = Asks::open(path.clone(), "", publish.clone());
        assert_eq!(state(&replay), State::Answered, "replay applies the answer transition");
        drop(replay);
        call(&asks, r#"answer {"id":"Q1","text":"What about the rollout?","ask":true}"#);
        assert_eq!(state(&asks), State::Waiting, "a follow-up gives main the turn");
        let replay = Asks::open(path.clone(), "", publish.clone());
        assert_eq!(state(&replay), State::Waiting, "replay applies the follow-up transition");
        drop(replay);
        call(&asks, r#"answer {"id":"Q1","decide":true}"#);
        assert_eq!(state(&asks), State::Answered, "an answer can supersede a follow-up");
        call(&asks, r#"answer {"id":"Q1","text":"Use your judgement"}"#);
        assert_eq!(state(&asks), State::Answered, "a later answer stays answered");
        call(&asks, r#"answer {"id":"Q1","ask":true,"text":"When?"}"#);
        call(&asks, r#"reply {"id":"Q1","text":"After review"}"#);
        assert_eq!(state(&asks), State::Open, "main's follow-up reply returns the turn to the user");
        let replay = Asks::open(path.clone(), "", publish.clone());
        assert_eq!(state(&replay), State::Open, "replay applies main's reply transition");
        drop(replay);
        // an answer that was a question: a plain reply leaves it answered, a reopen gives it back
        call(&asks, r#"answer {"id":"Q1","text":"Is that after the merge?"}"#);
        call(&asks, r#"reply {"id":"Q1","text":"Yes, after it"}"#);
        assert_eq!(state(&asks), State::Answered, "a plain reply keeps an answered question answered");
        call(&asks, r#"reply {"id":"Q1","text":"Yes, after it: which do you pick?","reopen":true}"#);
        assert_eq!(state(&asks), State::Open, "a reopen gives the turn back to the user");
        assert_eq!(question(&asks, "Q1").thread.last().map(|m| (m.by.as_str(), m.kind.as_str())), Some(("main", "reopen")));
        let replay = Asks::open(path.clone(), "", publish.clone());
        assert_eq!(state(&replay), State::Open, "replay applies the reopen transition");
        drop(replay);
        let mut out = Vec::new();
        asks.serve(r#"answer {"id":"Q1","text":"Forged"}"#, &refused, &mut out);
        let rev = asks.st.lock().unwrap().rev;
        let delivery = call(&asks, r#"wait {"ids":["Q1"]}"#);
        assert_eq!(delivery.len(), 6, "wait takes all user messages, never main's replies");
        assert_eq!(field(&delivery[0], "choice_text"), Some("Land it"), "wait includes the option's text");
        assert_eq!(asks.st.lock().unwrap().rev, rev, "taking does not change the tray revision");
        call(&asks, r#"close {"id":"Q1"}"#);
        assert_eq!(state(&asks), State::Closed, "close is final");
        let before = question(&asks, "Q1");
        for line in [r#"answer {"id":"Q1","text":"Too late"}"#, r#"reply {"id":"Q1","text":"Too late"}"#, r#"reply {"id":"Q1","text":"Too late","reopen":true}"#] {
            let mut out = Vec::new();
            asks.serve(line, &|| panic!("closed questions are rejected before judging"), &mut out);
            assert!(field(&parse(out[0].as_bytes()).unwrap(), "err").is_some(), "messages after close fail");
        }
        assert_eq!(question(&asks, "Q1"), before, "closed-message failures leave the question untouched");
        let rev = asks.st.lock().unwrap().rev;
        call(&asks, r#"close {"id":"Q1"}"#);
        assert_eq!(asks.st.lock().unwrap().rev, rev, "closing an already closed question changes nothing");
        let card = card(&before);
        assert!(card.contains("context:\n  First line\n  Second line\n"), "card indents context");
        assert!(card.contains("source=tray (pid 4242") && card.contains("option 1 \"Land it\" -- Please land"), "card shows user provenance and choice");
        assert!(card.contains("main reply: After review") && card.contains("refused answer attempts: 1"), "card shows replies and refusals");
        assert_eq!(time(50_591_000), "14:03:11", "thread times are UTC");
        drop(asks);
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"op\":\"msg\",\"broken\":\"\xff").unwrap();
        file.sync_data().unwrap();
        drop(file);
        let again = Asks::open(path.clone(), "", publish.clone());
        assert_eq!(question(&again, "Q1"), before, "replay keeps threads, taken flags and refusals despite a torn UTF-8 tail");
        assert_eq!(again.st.lock().unwrap().rev, rev, "replay preserves the tray revision");
        assert!(before.thread.iter().filter(|m| m.by == "user").all(|m| m.taken), "delivery marked user messages taken");
        assert_eq!(field(&call(&again, NEW)[0], "id"), Some("Q2"), "next id follows the highest stored id");
        call(&again, NEW);
        call(&again, r#"answer {"id":"Q3","text":"Yes"}"#);
        let all = call(&again, "list {}");
        assert_eq!(all.iter().map(|v| field(v, "id").unwrap()).collect::<Vec<_>>(), ["Q2", "Q3", "Q1"], "list groups open, answered and closed");
        assert_eq!(call(&again, r#"list {"open":true}"#).len(), 1, "open filter excludes answered and closed");
        let saved = again.st.lock().unwrap().qs.clone();
        drop(again);
        let compacted = Asks::open(path.clone(), "", publish.clone());
        assert_eq!(compacted.st.lock().unwrap().qs, saved, "compaction keeps all questions");
        drop(compacted);
        let compacted = Asks::open(path.clone(), "", publish);
        assert_eq!(compacted.st.lock().unwrap().qs, saved, "compacted question snapshots round trip");
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3, "compaction writes one line per question");
        drop(compacted);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The user's clear: answered and closed questions main took every message of go to the archive
    /// beside the store; open ones and answers main has not taken stay; an agent's is refused;
    /// `show` finds an archived question whole; ids count on, across a restart too.
    #[test]
    fn clear_archives_the_taken_and_keeps_open_and_untaken() {
        let dir = std::env::temp_dir().join(format!("devhub-ask-clear-{}", std::process::id()));
        let path = dir.join("ask.jsonl");
        let publish: Publish = Arc::new(|_| {});
        let asks = Asks::open(path.clone(), "", publish.clone());
        for _ in 0..4 { call(&asks, NEW); }
        call(&asks, r#"answer {"id":"Q2","choice":1}"#);
        call(&asks, r#"wait {"ids":["Q2"]}"#);
        call(&asks, r#"answer {"id":"Q3","text":"Wait for review"}"#);
        call(&asks, r#"close {"id":"Q4"}"#);
        let ids = |asks: &Asks| call(asks, "list {}").iter().map(|v| field(v, "id").unwrap().to_string()).collect::<Vec<_>>();
        let counts = |v: &Value| ["cleared", "open", "untaken"].map(|k| v.get(k).and_then(Value::as_u64).unwrap());
        assert_eq!(counts(&call(&asks, r#"clear {"dry":true}"#)[0]), [2, 1, 1], "Q2 (taken) and Q4 (closed) go; Q1 is open, Q3's answer untaken");
        assert_eq!(ids(&asks), ["Q1", "Q3", "Q2", "Q4"], "a dry run changes nothing");
        let mut out = Vec::new();
        asks.serve("clear {}", &refused, &mut out);
        assert!(field(&parse(out[0].as_bytes()).unwrap(), "err").is_some_and(|e| e.starts_with("refused")), "an agent's clear is refused");
        assert_eq!(ids(&asks), ["Q1", "Q3", "Q2", "Q4"], "and changes nothing");
        let before = question(&asks, "Q2");
        assert_eq!(counts(&call(&asks, "clear {}")[0]), [2, 1, 1]);
        assert_eq!(ids(&asks), ["Q1", "Q3"], "the open question and the untaken answer stay");
        let archived = call(&asks, r#"show {"id":"Q2"}"#);
        assert_eq!(Question::from_value(&archived[0]), Some(before), "show finds an archived question whole");
        assert!(archived[0].get("cleared_ms").and_then(Value::as_u64).is_some_and(|at| at > 0), "and says when it was cleared");
        assert_eq!(std::fs::read_to_string(dir.join("ask-archive.jsonl")).unwrap().lines().count(), 2, "the archive is beside the store");
        assert_eq!(field(&call(&asks, NEW)[0], "id"), Some("Q5"), "the next id follows the highest ever given, Q4 archived");
        call(&asks, r#"answer {"id":"Q5","decide":true}"#);
        call(&asks, r#"wait {"ids":["Q5"]}"#);
        assert_eq!(counts(&call(&asks, "clear {}")[0]), [1, 1, 1], "the newest question, answered and taken, goes too");
        let rev = asks.st.lock().unwrap().rev;
        assert_eq!(counts(&call(&asks, "clear {}")[0]), [0, 1, 1], "nothing more to clear");
        assert_eq!(asks.st.lock().unwrap().rev, rev, "and nothing changed");
        drop(asks);
        let again = Asks::open(path.clone(), "", publish);
        assert_eq!(again.st.lock().unwrap().rev, rev, "the revision rebuilt at open counts the cleared questions");
        assert_eq!(ids(&again), ["Q1", "Q3"]);
        assert!(question(&again, "Q4").state == State::Closed, "the archive outlives a restart");
        assert_eq!(field(&call(&again, NEW)[0], "id"), Some("Q6"), "ids count on after a restart, the highest archived");
        drop(again);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refusal_is_durable_visible_and_published() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = records.clone();
        let asks = Asks { path: None, machine: String::new(), publish: Some(Arc::new(move |r| captured.lock().unwrap().push(r))),
            st: Mutex::new(St::default()), changed: Condvar::new(), gate: OnceLock::new() };
        call(&asks, NEW);
        let rev = asks.st.lock().unwrap().rev;
        let judged = std::cell::Cell::new(0);
        let judge = || { judged.set(judged.get() + 1); refused() };
        let mut out = Vec::new();
        asks.serve(r#"answer {"id":"Q1","choice":1}"#, &judge, &mut out);
        let v = parse(out[0].as_bytes()).unwrap();
        assert_eq!(field(&v, "err"), Some("refused: an agent's environment"), "refusal is returned as an error");
        let q = question(&asks, "Q1");
        assert!(q.thread.is_empty(), "refusal adds no message");
        assert_eq!(q.refused.len(), 1, "refusal adds one attempt");
        assert_eq!(q.refused[0].chain, refused().chain, "refusal retains the process chain");
        assert_eq!(q.state, State::Open, "refusal keeps the user's turn");
        assert_eq!(asks.st.lock().unwrap().rev, rev + 1, "refusal bumps the tray revision");
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2, "new and refusal are published");
        assert_eq!(records[1].sev, Sev::Warn, "refusal publishes a warning");
        assert!(records[1].line.contains("an answer to Q1 was refused:") && records[1].line.contains(&refused().chain), "warning names the question and provenance");
        drop(records);
        for line in [
            r#"answer {"id":"Q9","text":"Yes"}"#, r#"answer {"id":"Q1","choice":0}"#,
            r#"answer {"id":"Q1","choice":3}"#, r#"answer {"id":"Q1","choice":1.5}"#,
            r#"answer {"id":"Q1","ask":true,"choice":1,"text":"Why?"}"#,
            r#"answer {"id":"Q1","ask":true,"decide":true,"text":"Why?"}"#,
            r#"answer {"id":"Q1","ask":true,"text":" "}"#, r#"answer {"id":"Q1"}"#,
        ] {
            let mut out = Vec::new();
            asks.serve(line, &judge, &mut out);
            assert!(field(&parse(out[0].as_bytes()).unwrap(), "err").is_some(), "invalid answer fails: {line}");
        }
        assert_eq!(judged.get(), 1, "only a valid answer calls the judge, once");
        assert_eq!(question(&asks, "Q1").refused.len(), 1, "validation failures add no refused attempts");
    }

    #[test]
    fn wait_takes_once_and_follow_tracks_visible_changes() {
        let asks = Asks::memory();
        call(&asks, NEW);
        let (worker, rx) = blocked_call(&asks, r#"wait {"any":true}"#.into());
        assert!(matches!(rx.recv_timeout(Duration::from_millis(20)), Err(mpsc::RecvTimeoutError::Timeout)), "wait blocks before an answer");
        call(&asks, r#"answer {"id":"Q1","choice":2,"text":"Until reviewed"}"#);
        // the change wakes it, not the recheck: a box under load took over 100 ms (11-16)
        let out = rx.recv_timeout(RECHECK / 2).expect("answer wakes wait");
        worker.join().unwrap();
        assert_eq!(out.len(), 1, "wait returns the one untaken message");
        assert_eq!(field(&out[0], "source"), Some("tray"), "wait includes source");
        assert_eq!(field(&out[0], "chain"), Some(tray().chain.as_str()), "wait includes process chain");
        assert_eq!(field(&out[0], "choice_text"), Some("Wait"), "wait resolves the chosen option");
        let (worker, rx) = blocked_call(&asks, r#"wait {"any":true}"#.into());
        assert!(matches!(rx.recv_timeout(Duration::from_millis(20)), Err(mpsc::RecvTimeoutError::Timeout)), "taken messages do not wake a second wait");
        call(&asks, NEW);
        assert!(matches!(rx.recv_timeout(Duration::from_millis(20)), Err(mpsc::RecvTimeoutError::Timeout)), "new questions alone do not satisfy wait");
        call(&asks, r#"answer {"id":"Q2","ask":true,"text":"Why now?"}"#);
        let out = rx.recv_timeout(RECHECK / 2).expect("another question's answer releases any wait");
        worker.join().unwrap();
        assert_eq!(out.len(), 1, "second wait does not redeliver the first answer");
        assert_eq!(field(&out[0], "id"), Some("Q2"), "any includes questions created while waiting");
        assert_eq!(field(&out[0], "kind"), Some("ask"), "follow-ups are user messages too");
        let empty = call(&Asks::memory(), r#"follow {"rev":0}"#);
        assert!(empty.len() == 1 && empty[0].get("rev").and_then(Value::as_u64) != Some(0), "an empty store gives its follower a revision to wait at");
        let unknown = call(&asks, r#"wait {"ids":["Q99"]}"#);
        assert_eq!(field(&unknown[0], "err"), Some("no question Q99"), "unknown ids fail immediately");
        let out = call(&asks, r#"follow {"rev":0}"#);
        assert_eq!(out.len(), 3, "initial follow returns a revision and all questions");
        assert_eq!(field(&out[1], "id"), Some("Q2"), "follow uses list order");
        assert!(Question::from_value(&out[1]).is_some(), "follow sends full questions");
        let rev = out[0].get("rev").and_then(Value::as_u64).unwrap();
        let (worker, rx) = blocked_call(&asks, format!("follow {{\"rev\":{rev}}}"));
        assert!(matches!(rx.recv_timeout(Duration::from_millis(20)), Err(mpsc::RecvTimeoutError::Timeout)), "follow waits at the current revision");
        call(&asks, r#"reply {"id":"Q2","text":"The review is complete"}"#);
        let out = rx.recv_timeout(RECHECK / 2).expect("reply wakes follow");
        worker.join().unwrap();
        assert_eq!(out[0].get("rev").and_then(Value::as_u64), Some(rev + 1), "follow reports the new revision");
        assert_eq!(Question::from_value(&out[1]).unwrap().state, State::Open, "follow snapshot includes the transition");
    }

    /// The gate as its binary does it: its card gives a line per tag (and skips `relax#9`), its
    /// decide claims the press back by its nonce, as the master's child, and answers as the test
    /// sets it.
    struct FakeGate {
        asks: OnceLock<std::sync::Weak<Asks>>,
        claimed: Mutex<Vec<(String, Claimed)>>,
        answer: Mutex<Box<dyn FnMut(&Claimed) -> Value + Send>>,
    }

    fn gate_line(tag: &str, row: &str) -> Value {
        let mut v = obj(vec![("key", s(tag)), ("head", s(format!("{tag} \u{b7} PR by ada-example"))), ("what", s("Adds a fallback.")),
            ("why", s("The review takes it only with changes.")), ("risks", s("unsafe code")), ("yes", s("A fix lane ports it.")),
            ("no", s("Main replies that we decline it, and closes it.")), ("row", s(row))]);
        if let (Value::Obj(fields), "relax#2") = (&mut v, tag) { fields.push(("tag".into(), s("closed on GitHub"))); }
        v
    }

    impl Gate for FakeGate {
        fn card(&self, tags: &[String]) -> Result<Vec<Value>, String> {
            Ok(tags.iter().map(|t| if t == "relax#9" { obj(vec![("skip", s(t)), ("why", s("not in doubt"))]) } else { gate_line(t, &format!("{:016x}", t.len() * 7 + t.as_bytes()[t.len() - 1] as usize)) }).collect())
        }

        fn decide(&self, nonce: &str) -> Result<Value, String> {
            let asks = self.asks.get().and_then(std::sync::Weak::upgrade).ok_or("no store")?;
            let mut out = Vec::new();
            asks.serve_with(&format!("claim {}", obj(vec![("nonce", s(nonce))]).to_json()), &|| panic!("a claim is not judged"), &|| true, &mut out);
            let v = parse(out[0].as_bytes()).unwrap();
            let Some(c) = Claimed::from_value(&v) else { return Ok(v) };
            self.claimed.lock().unwrap().push((nonce.to_string(), c.clone()));
            let mut answer = self.answer.lock().unwrap();
            Ok(answer.as_mut()(&c))
        }
    }

    /// A review card: the gate makes an intake card's items and the master holds each press for
    /// the gate's child to claim, once. A press judged the tray's reaches the gate with exactly the
    /// item and row the card showed; a cli or refused press, or one on a row the card no longer
    /// shows, never reaches it and is kept on the card; a row that moved comes back as the item
    /// stands now; the last decision answers the card. Main's own items take the user's answer as
    /// `answer` does, with no gate. All of it survives a restart.
    #[test]
    fn a_review_card_takes_a_yes_or_no_per_item_and_binds_each_to_its_row() {
        let dir = std::env::temp_dir().join(format!("devhub-ask-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("ask.jsonl");
        let publish: Publish = Arc::new(|_| {});
        let asks = Asks::open(path.clone(), "", publish.clone());
        let fake = Arc::new(FakeGate { asks: OnceLock::new(), claimed: Mutex::new(Vec::new()), answer: Mutex::new(Box::new(|c: &Claimed| obj(vec![("ok", s("cleared")), ("row", s(&c.row))]))) });
        let _ = fake.asks.set(Arc::downgrade(&asks));
        let cli = || Judged { source: Source::Cli, chain: "pid 900 makepad-devhub, from 899 zsh".into(), why: String::new() };
        let press = |judge: &dyn Fn() -> Judged, item: &str, row: &str, yes: bool| -> Value {
            let mut out = Vec::new();
            asks.serve(&format!("decide {}", obj(vec![("id", s("Q1")), ("item", s(item)), ("row", s(row)), ("yes", Value::Bool(yes))]).to_json()), judge, &mut out);
            parse(out[0].as_bytes()).unwrap()
        };
        let filed = call(&asks, r#"new {"from":"main","title":"Review 3 intake items","intake":["relax#1","relax#2","relax#9"],"plans":{"relax#1":"RELAXPR takes it"}}"#);
        assert!(field(&filed[0], "err").is_some_and(|e| e.contains("no gate binary")), "no gate, no intake card: {filed:?}");
        asks.set_gate(fake.clone());
        let filed = call(&asks, r#"new {"from":"main","title":"Review 3 intake items","intake":["relax#1","relax#2","relax#9"],"plans":{"relax#1":"RELAXPR takes it"}}"#);
        assert_eq!(field(&filed[0], "id"), Some("Q1"));
        assert_eq!(filed[0].get("skipped").and_then(Value::as_arr).map(|a| a.len()), Some(1), "the tag not in doubt is said, not carded");
        let q = question(&asks, "Q1");
        let rows: Vec<String> = q.items.iter().map(|i| i.row.clone()).collect();
        assert!(q.items.len() == 2 && q.items.iter().all(|i| i.gate && i.decided.is_empty()) && q.items[0].plan == "RELAXPR takes it", "two gate items, open, main's plan under the first");
        assert_eq!((q.items[0].tag.as_str(), q.items[1].tag.as_str()), ("", "closed on GitHub"), "the gate's tag rides beside its head");
        let mut long = gate_line("relax#5", "00000000000000aa");
        if let Value::Obj(fields) = &mut long { fields.push(("tag".into(), s("x".repeat(ITEM_TAG_MAX + 1)))); }
        assert!(Item::from_gate(&long).is_err(), "a tag is a few words, no more");
        let forged = call(&asks, r#"new {"from":"main","title":"Forged","items":[{"head":"relax#1","row":"0000000000000000","gate":true}]}"#);
        assert!(!question(&asks, field(&forged[0], "id").unwrap()).items[0].gate, "a client's items are never the gate's");

        // refused: the person's terminal (an intake item is the tray's), an agent's process, an old row
        let v = press(&cli, "relax#1", &rows[0], true);
        assert!(field(&v, "err").is_some_and(|e| e.contains("pressed in aires") || e.contains("press in aires")), "a cli press on an intake item: {v:?}");
        let v = press(&refused, "relax#1", &rows[0], true);
        assert_eq!(field(&v, "err"), Some("refused: an agent's environment"));
        let v = press(&tray, "relax#1", "0000000000000000", true);
        assert!(field(&v, "err").is_some_and(|e| e.contains("the card changed")), "a press on a row the card no longer shows: {v:?}");
        assert!(fake.claimed.lock().unwrap().is_empty(), "none of them reached the gate");
        let q = question(&asks, "Q1");
        assert_eq!(q.refused.iter().map(|r| r.item.as_str()).collect::<Vec<_>>(), ["relax#1", "relax#1"], "the refusals stay on the card, on their item");
        assert!(q.items.iter().all(|i| i.decided.is_empty()), "and decide nothing");

        // Yes, the tray's, on the row the card shows: the gate gets exactly that, claimed once
        let v = press(&tray, "relax#1", &rows[0], true);
        assert_eq!((v.get("ok").and_then(Value::as_bool), v.get("stale").and_then(Value::as_bool)), (Some(true), Some(false)), "{v:?}");
        let (nonce, c) = fake.claimed.lock().unwrap()[0].clone();
        assert_eq!(c, Claimed { id: "Q1".into(), item: "relax#1".into(), row: rows[0].clone(), yes: true, source: "tray".into(), chain: tray().chain }, "the gate gets the item and row the card showed");
        assert_eq!(nonce.len(), 32, "a 128-bit nonce");
        let mut out = Vec::new();
        asks.serve_with(&format!("claim {}", obj(vec![("nonce", s(&nonce))]).to_json()), &tray, &|| true, &mut out);
        assert!(out[0].contains("each is claimed once"), "a nonce is claimed once: {out:?}");
        let mut out = Vec::new();
        asks.serve(&format!("claim {}", obj(vec![("nonce", s(&nonce))]).to_json()), &tray, &mut out);
        assert!(out[0].contains("master's own child alone"), "and by the master's own child alone: {out:?}");
        let q = question(&asks, "Q1");
        assert_eq!((q.items[0].decided.as_str(), q.state), ("yes", State::Open), "decided yes; the card waits for its other item");
        assert!(field(&press(&tray, "relax#1", &rows[0], false), "err").is_some_and(|e| e.contains("decided already")), "a decided item takes no second press");

        // the row moved after the card was drawn: the item as it stands now, open again
        *fake.answer.lock().unwrap() = Box::new(|_: &Claimed| obj(vec![("stale", gate_line("relax#2", "00000000000000bb")), ("why", s("relax#2's row is not it now: nothing is cleared"))]));
        let v = press(&tray, "relax#2", &rows[1], true);
        assert_eq!(v.get("stale").and_then(Value::as_bool), Some(true));
        let item = question(&asks, "Q1").items[1].clone();
        assert!(item.row == "00000000000000bb" && item.decided.is_empty() && item.note.contains("changed after you saw it"), "the card shows the row now: {item:?}");
        assert!(field(&press(&tray, "relax#2", &rows[1], false), "err").is_some_and(|e| e.contains("the card changed")), "the old row binds to nothing now");
        // No on the row it shows now: declined, and the card is answered
        *fake.answer.lock().unwrap() = Box::new(|c: &Claimed| obj(vec![("ok", s("declined")), ("row", s(&c.row))]));
        assert_eq!(press(&tray, "relax#2", "00000000000000bb", false).get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(fake.claimed.lock().unwrap().last().map(|(_, c)| (c.yes, c.row.clone())), Some((false, "00000000000000bb".into())), "No reaches the gate as no, bound to the row shown");
        let q = question(&asks, "Q1");
        assert_eq!((q.items[1].decided.as_str(), q.state), ("no", State::Answered), "the last decision answers the card");
        let taken = call(&asks, r#"wait {"ids":["Q1"]}"#);
        let said: Vec<(&str, &str, &str)> = taken.iter().map(|m| (field(m, "kind").unwrap(), field(m, "item").unwrap(), field(m, "text").unwrap())).collect();
        assert_eq!(said, [("yes", "relax#1", "cleared"), ("yes", "relax#2", "stale"), ("no", "relax#2", "declined")], "main takes each press with its item and outcome");

        // the gate fails: nothing decided, said on the card; words on an open card keep it open
        call(&asks, r#"new {"from":"main","title":"One more","intake":["relax#3"]}"#);
        *fake.answer.lock().unwrap() = Box::new(|_: &Claimed| obj(vec![("err", s("not recorded: disk full"))]));
        let row = question(&asks, "Q3").items[0].row.clone();
        let mut out = Vec::new();
        asks.serve(&format!("decide {}", obj(vec![("id", s("Q3")), ("item", s("relax#3")), ("row", s(&row)), ("yes", Value::Bool(true))]).to_json()), &tray, &mut out);
        assert!(out[0].contains("Not cleared: not recorded: disk full"), "{out:?}");
        call(&asks, r#"answer {"id":"Q3","text":"Why is this one in doubt?"}"#);
        let q = question(&asks, "Q3");
        assert!(q.items[0].decided.is_empty() && q.state == State::Open, "an answer in words leaves a card with an open item the user's turn");

        // main's own items: the user's answer per item, from the terminal too, no gate
        let filed = call(&asks, r#"new {"from":"main","title":"Land these?","items":[{"head":"rc-a"},{"head":"rc-b","why":"its tests pass"}]}"#);
        let id = field(&filed[0], "id").unwrap().to_string();
        let claims = fake.claimed.lock().unwrap().len();
        for (judge, item, yes) in [(&cli as &dyn Fn() -> Judged, "1", true), (&tray, "2", false)] {
            let mut out = Vec::new();
            asks.serve(&format!("decide {}", obj(vec![("id", s(&id)), ("item", s(item)), ("row", s("")), ("yes", Value::Bool(yes))]).to_json()), judge, &mut out);
            assert!(out[0].contains("\"ok\":true"), "{out:?}");
        }
        let q = question(&asks, &id);
        assert_eq!((q.items.iter().map(|i| i.decided.as_str()).collect::<Vec<_>>(), q.state), (vec!["yes", "no"], State::Answered), "each item answered, the card with them");
        assert_eq!(q.thread.iter().map(|m| m.source.as_str()).collect::<Vec<_>>(), ["cli", "tray"], "with each answer's provenance");
        assert_eq!(fake.claimed.lock().unwrap().len(), claims, "and no gate");

        // all of it survives a restart, and the agent's view hides the review's words
        let saved = asks.st.lock().unwrap().qs.clone();
        drop(asks);
        let again = Asks::open(path.clone(), "", publish);
        assert_eq!(again.st.lock().unwrap().qs, saved, "replay rebuilds the cards, items and refusals");
        let mut q = saved[0].clone();
        masked(&mut q);
        assert!(q.items.iter().all(|i| i.what == IN_AIRES) && card(&q).contains("relax#2 [no]") && !card(&q).contains("Adds a fallback"), "ask show prints the review's words as in aires");
        drop(again);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn request_flags_are_pure_and_checked() {
        let line = new_request(&["--from", "lane", "--title", "A \"choice\"?", "--decided", "Done", "--why", "Ready",
            "--option", "One", "--option", "Two", "--recommend", "One", "--blocked", "Landing", "--context", "a\nb"]).unwrap();
        let v = parse(line.strip_prefix("new ").unwrap().as_bytes()).unwrap();
        let q = new_question(&v).unwrap();
        assert_eq!((q.from.as_str(), q.title.as_str(), q.decided.as_str(), q.why.as_str()), ("lane", "A \"choice\"?", "Done", "Ready"), "new parses text flags and escapes");
        assert_eq!(q.options, ["One", "Two"], "repeated options retain their order");
        assert_eq!((q.recommend.as_str(), q.blocked.as_str(), q.context.as_str()), ("One", "Landing", "a\nb"), "new carries the full card");
        let line = new_request(&["--from", "lane", "--title", "Question", "--context-file", "context.txt"]).unwrap();
        let v = parse(line.strip_prefix("new ").unwrap().as_bytes()).unwrap();
        assert_eq!(field(&v, "context_file"), Some("context.txt"), "pure parsing carries the context path without reading it");
        for args in [vec!["--from", "lane"], vec!["--title", "Title"], vec!["--from", "lane", "--title"],
            vec!["--from", "lane", "--title", "Title", "--context", "Text", "--context-file", "context.txt"]] {
            assert!(new_request(&args).is_err(), "bad new flags are rejected: {args:?}");
        }
        let line = answer_request(&["Q7", "--choice", "2", "--text", "Land \"now\""]).unwrap();
        let v = parse(line.strip_prefix("answer ").unwrap().as_bytes()).unwrap();
        let m = answer_message(&v, Some(2)).unwrap();
        assert_eq!(field(&v, "id"), Some("Q7"), "answer keeps the id");
        assert_eq!((m.choice, m.text.as_str(), m.kind.as_str()), (Some(2), "Land \"now\"", "answer"), "answer parses choice with text");
        for args in [vec![], vec!["Q7"], vec!["Q7", "--choice", "many"], vec!["Q7", "--choice", "0"],
            vec!["Q7", "--choice", "1", "--ask", "--text", "Why?"], vec!["Q7", "--choice", "1", "--decide"], vec!["Q7", "--ask"]] {
            assert!(answer_request(&args).is_err(), "bad answer flags are rejected: {args:?}");
        }
        for args in [vec!["Q7", "--decide"], vec!["Q7", "--text", "Yes"], vec!["Q7", "--ask", "--text", "Why?"]] {
            assert!(answer_request(&args).is_ok(), "valid answer mode is accepted: {args:?}");
        }
    }
}
