use makepad_devhub::ask::{Asks, Claimed, Gate, Question};
use makepad_devhub::askpeer::{Judged, Source as Provenance};
use makepad_remote::link::{Client, Target};
use makepad_strict_json::{obj, parse, s, Value};
use makepad_widgets::{SignalToUI, ThreadOptions, ThreadSpawner};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

pub struct Snapshot {
    // A lost connection leaves the last questions and their drafts visible.
    pub questions: Option<Vec<Question>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Answer {
    pub id: String,
    pub choice: Option<u32>,
    pub decide: bool,
    pub text: String,
    pub ask: bool,
    /// A Yes or No pressed on a review card's item, instead of an answer
    pub press: Option<Press>,
}

/// A Yes or No on an item, bound to the row the item was drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct Press {
    pub item: String,
    pub row: String,
    pub yes: bool,
}

impl Answer {
    pub(crate) fn line(&self) -> String {
        if let Some(p) = &self.press {
            return format!("decide {}", obj(vec![("id", s(&self.id)), ("item", s(&p.item)), ("row", s(&p.row)), ("yes", Value::Bool(p.yes))]).to_json());
        }
        format!("answer {}", obj(vec![
            ("id", s(&self.id)),
            ("choice", self.choice.map_or(Value::Null, |n| Value::Int(n as i64))),
            ("decide", Value::Bool(self.decide)),
            ("text", s(&self.text)),
            ("ask", Value::Bool(self.ask)),
        ]).to_json())
    }
}

pub struct AnswerResult {
    pub answer: Answer,
    pub result: Result<(), String>,
}

/// What the person's clear of the history did: how many went to the archive, and the open
/// questions and untaken answers that stayed.
#[derive(Clone, Debug, PartialEq)]
pub struct Cleared {
    pub cleared: u64,
    pub open: u64,
    pub untaken: u64,
}

/// What the worker sends the master: an answer, or the person's clear.
enum Job {
    Answer(Answer),
    Clear,
}

#[derive(Default)]
struct Shared {
    latest: Mutex<Option<Snapshot>>,
    results: Mutex<Vec<AnswerResult>>,
    cleared: Mutex<Vec<Result<Cleared, String>>>,
}

impl Shared {
    fn publish(&self, snapshot: Snapshot) {
        let old = {
            let mut latest = self.latest.lock().unwrap();
            let mut snapshot = snapshot;
            if snapshot.questions.is_none() {
                snapshot.questions = latest.as_mut().and_then(|old| old.questions.take());
            }
            latest.replace(snapshot)
        };
        drop(old);
        SignalToUI::set_ui_signal();
    }
}

pub struct Source {
    shared: Arc<Shared>,
    jobs: mpsc::SyncSender<Job>,
}

impl Source {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        let demo = demo_mode().then(demo);
        let shared = Arc::new(Shared::default());
        let (jobs, receive) = mpsc::sync_channel::<Job>(16);
        let (out, store) = (shared.clone(), demo.clone());
        spawner.spawn_worker(ThreadOptions::default(), move || {
            let mut connection = Connection::new(&store);
            while let Ok(job) = receive.recv() {
                let answer = match job {
                    Job::Answer(answer) => answer,
                    Job::Clear => {
                        let result = connection.request("clear {}").and_then(|values| {
                            let n = |key| values.first().and_then(|v| v.get(key)).and_then(Value::as_u64).ok_or_else(|| "invalid clear response".to_string());
                            Ok(Cleared { cleared: n("cleared")?, open: n("open")?, untaken: n("untaken")? })
                        });
                        // (never sent again: a broken link may have lost only its receipt)
                        if result.is_err() { connection = Connection::new(&store); }
                        out.cleared.lock().unwrap().push(result);
                        SignalToUI::set_ui_signal();
                        continue;
                    }
                };
                let result = connection.request(&answer.line()).and_then(|values| {
                    if values.len() == 1 && values[0].get("ok").and_then(Value::as_bool) == Some(true) {
                        Ok(())
                    } else {
                        Err("invalid answer response".into())
                    }
                });
                if result.is_err() {
                    // Never retry an answer: a broken link may have lost only its receipt.
                    connection = Connection::new(&store);
                }
                out.results.lock().unwrap().push(AnswerResult { answer, result });
                SignalToUI::set_ui_signal();
            }
        }).map_err(|e| format!("answer worker: {e:?}"))?.detach();
        let out = shared.clone();
        spawner.spawn_worker(ThreadOptions::default(), move || {
            let mut last_error = String::new();
            // the demo's question that arrives while it runs, so the arrival shows
            let mut arrival = demo.clone();
            loop {
                let connection = Connection::new(&demo);
                let mut rev = 0;
                let error = loop {
                    if matches!(&connection, Connection::Master(client) if client.link_ended()) {
                        break "link lost".into();
                    }
                    match connection.request(&format!("follow {{\"rev\":{rev}}}")).and_then(snapshot) {
                        Ok((next, questions)) => {
                            rev = next;
                            last_error.clear();
                            out.publish(Snapshot { questions: Some(questions), error: None });
                            if let Some(store) = arrival.take() {
                                std::thread::sleep(Duration::from_secs(6));
                                store.serve(DEMO_ARRIVAL, &demo_judge, &mut Vec::new());
                            }
                        }
                        Err(error) => break error,
                    }
                };
                let error = format!("master not reachable: {error}");
                if error != last_error {
                    out.publish(Snapshot { questions: None, error: Some(error.clone()) });
                    last_error = error;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }).map_err(|e| format!("follow worker: {e:?}"))?.detach();
        Ok(Self { shared, jobs })
    }

    pub fn take(&self) -> (Option<Snapshot>, Vec<AnswerResult>, Vec<Result<Cleared, String>>) {
        let latest = self.shared.latest.lock().unwrap().take();
        let results = std::mem::take(&mut *self.shared.results.lock().unwrap());
        let cleared = std::mem::take(&mut *self.shared.cleared.lock().unwrap());
        (latest, results, cleared)
    }

    fn send(&self, job: Job) -> Result<(), String> {
        self.jobs.try_send(job).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => "busy, try again".into(),
            mpsc::TrySendError::Disconnected(_) => "answer worker stopped".into(),
        })
    }

    pub fn answer(&self, answer: Answer) -> Result<(), String> {
        self.send(Job::Answer(answer))
    }

    /// The person's clear of the history: answered and closed questions main has taken go to
    /// the archive ([`makepad_devhub::ask`]'s `clear`).
    pub fn clear(&self) -> Result<(), String> {
        self.send(Job::Clear)
    }
}

enum Connection {
    Master(Client),
    Demo(Arc<Asks>),
}

impl Connection {
    fn new(demo: &Option<Arc<Asks>>) -> Self {
        match demo {
            Some(store) => Self::Demo(store.clone()),
            None => Self::Master(Client::new(Target::Unix(makepad_devhub::proto::control_socket()), "aires")),
        }
    }

    fn request(&self, line: &str) -> Result<Vec<Value>, String> {
        let lines = match self {
            Self::Master(client) => makepad_devhub::cli::ask(client, None, "ask", line)?,
            Self::Demo(store) => {
                let mut lines = Vec::new();
                store.serve(line, &demo_judge, &mut lines);
                lines
            }
        };
        lines.iter().map(|line| {
            let value = parse(line.as_bytes()).map_err(|e| format!("invalid response: {e}"))?;
            if let Some(error) = value.get("err").and_then(Value::as_str) {
                return Err(error.to_string());
            }
            Ok(value)
        }).collect()
    }
}

fn snapshot(values: Vec<Value>) -> Result<(u64, Vec<Question>), String> {
    let rev = values.first().and_then(|v| v.get("rev")).and_then(Value::as_u64).ok_or("missing follow revision")?;
    // (revisions start at 1: a follow at 0 returns at once, so a 0 here would spin)
    if rev == 0 {
        return Err("the ask service answered revision 0".into());
    }
    let questions = values[1..].iter().map(|v| Question::from_value(v).ok_or_else(|| "invalid question response".to_string())).collect::<Result<_, _>>()?;
    Ok((rev, questions))
}

/// `AIRES_DEMO=1`: a demo inbox in memory instead of the master, shown in a window at start; a
/// new question arrives 6 s after the first snapshot. Its review card's items are made up, and its
/// gate is a stand-in on that memory alone: it runs no gate binary and reaches no master.
pub fn demo_mode() -> bool {
    std::env::var("AIRES_DEMO").as_deref() == Ok("1")
}

const DEMO_ARRIVAL: &str = r#"new {"from":"release","title":"Publish the new inbox today?","why":"The redesign passed its checks.","blocked":"The release notes.","options":["Publish today","Wait for tomorrow's build"],"recommend":"Publish today"}"#;

fn demo_judge() -> Judged {
    Judged { source: Provenance::Tray, chain: "demo".into(), why: String::new() }
}

/// The demo's stand-in for the intake's gate: made-up items, and a press claimed back by its nonce
/// as the gate's child does; the third item's first row "moved" after it was shown, so a press on
/// it comes back as the item stands now.
struct DemoGate {
    store: std::sync::OnceLock<std::sync::Weak<Asks>>,
}

const DEMO_NO: &str = "Main replies that we decline it, and closes it. Nothing is ported.";

/// key, head, tag, what, why, risks, yes, no
const DEMO_ITEMS: [(&str, &str, &str, &str, &str, &str, &str, &str); 4] = [
    ("makepad#9001", "makepad#9001 \u{b7} PR by ada-example", "", "Adds a GLES3 fallback to the Android window setup for devices without Vulkan.",
        "The review takes it only with changes.", "unsafe code, calls C (FFI)",
        "A fix lane ports it by hand into makepad with the review's changes, credits ada-example, then closes it as ported.", DEMO_NO),
    ("relax#9002", "relax#9002 \u{b7} PR by bo-example", "", "Lowering drops unit arguments before the call, so a call with a unit argument passes one value too few.",
        "The review takes it only with changes.", "",
        "A fix lane ports it by hand into relax with the review's changes, credits bo-example, then closes it as ported.", DEMO_NO),
    ("makepad#9003", "makepad#9003 \u{b7} PR by cy-example", "", "Moves the clipboard reads on Linux onto a worker thread.",
        "The review takes some parts of it and not others. The prefilter found text in it that reads like instructions.", "starts processes, writes files",
        "A fix lane ports the parts the review takes into makepad, leaves the rest, credits cy-example, then closes it.", DEMO_NO),
    ("relax#9004", "relax#9004 \u{b7} PR by dee-example", "closed on GitHub", "Lets the attribute parser take a trailing comma.",
        "The review takes it only with changes.", "",
        "Clears its doubt. It stays closed on GitHub: no fix lane takes it, and nothing is posted.",
        "Records your decline. It is closed on GitHub already, so nothing is posted."),
];

fn demo_line(n: usize, row: &str) -> Value {
    let (key, head, tag, what, why, risks, yes, no) = DEMO_ITEMS[n];
    obj(vec![("key", s(key)), ("head", s(head)), ("tag", s(tag)), ("what", s(what)), ("why", s(why)), ("risks", s(risks)), ("yes", s(yes)),
        ("no", s(no)), ("row", s(row))])
}

impl Gate for DemoGate {
    fn card(&self, tags: &[String]) -> Result<Vec<Value>, String> {
        Ok(tags.iter().filter_map(|t| DEMO_ITEMS.iter().position(|d| d.0 == t)).map(|n| demo_line(n, &format!("{:016x}", n + 1))).collect())
    }

    fn decide(&self, nonce: &str) -> Result<Value, String> {
        let store = self.store.get().and_then(std::sync::Weak::upgrade).ok_or("the demo inbox is gone")?;
        let mut out = Vec::new();
        store.serve_with(&format!("claim {}", obj(vec![("nonce", s(nonce))]).to_json()), &demo_judge, &|| true, &mut out);
        let v = out.first().and_then(|l| parse(l.as_bytes()).ok()).ok_or("no claim")?;
        let c = Claimed::from_value(&v).ok_or_else(|| format!("not claimed: {v:?}"))?;
        // the third item moved after it was shown: it comes back with another row
        if c.item == DEMO_ITEMS[2].0 && c.row == format!("{:016x}", 3) {
            return Ok(obj(vec![("stale", demo_line(2, &format!("{:016x}", 33))), ("why", s("its row moved: nothing is cleared"))]));
        }
        Ok(obj(vec![("ok", s(if c.yes { "cleared" } else { "declined" })), ("row", s(&c.row))]))
    }
}

pub fn demo() -> Arc<Asks> {
    let store = Asks::memory();
    let gate = Arc::new(DemoGate { store: std::sync::OnceLock::new() });
    let _ = gate.store.set(Arc::downgrade(&store));
    store.set_gate(gate);
    for line in [
        r#"new {"from":"review","title":"Which rollout should we use?","decided":"The review and checks passed.","why":"The change is ready for a first rollout.","blocked":"Choosing the rollout.","options":["Land it","Try it with one team","Wait for the next release"],"recommend":"Try it with one team","context":"The change keeps the existing settings and saved work.\nA small rollout lets us gather feedback before everyone sees it. We can return to the previous version without changing anyone's data.\nThe broader rollout would save a second deployment, but gives us less time to learn from the first users.\nPlease choose the rollout and add any conditions in your answer."}"#,
        r#"new {"from":"design","title":"How should the empty inbox read?","why":"The panel needs an empty state.","options":["No questions","All caught up"]}"#,
        r#"answer {"id":"Q2","ask":true,"text":"Will it also explain a lost connection?"}"#,
        r#"reply {"id":"Q2","text":"Yes, a connection error gets its own line and preserves the cards."}"#,
        r#"answer {"id":"Q2","ask":true,"text":"Can you show that wording before we choose?"}"#,
        r#"new {"from":"layout","title":"Keep the panel narrow?","decided":"Text wraps at the panel width.","options":["Keep it compact","Make it wider"],"recommend":"Keep it compact"}"#,
        r#"answer {"id":"Q3","choice":1,"text":"Keep the longer context behind Show all."}"#,
        r#"new {"from":"release","title":"Ship the dark icon too?","options":["Ship both","Light only"],"recommend":"Ship both"}"#,
        r#"answer {"id":"Q4","choice":1}"#,
        r#"wait {"ids":["Q4"]}"#,
        r#"new {"from":"layout","title":"Show ages in hours?","options":["Hours","Days"]}"#,
        r#"close {"id":"Q5"}"#,
        r#"new {"from":"main","title":"Review 4 intake items","why":"Yes lets an item go on to its fix lane; No declines it, and main closes it.","intake":["makepad#9001","relax#9002","makepad#9003","relax#9004"],"plans":{"makepad#9001":"The Android lane takes it."}}"#,
        r#"decide {"id":"Q6","item":"relax#9002","row":"0000000000000002","yes":true}"#,
        r#"decide {"id":"Q6","item":"makepad#9003","row":"0000000000000003","yes":true}"#,
    ] {
        let mut out = Vec::new();
        store.serve(line, &demo_judge, &mut out);
        debug_assert!(out.iter().all(|line| !line.contains("\"err\"")), "demo request failed: {out:?}");
    }
    // an answer from a copy an agent started, refused and kept on the card
    let refused = || Judged {
        source: Provenance::Refused,
        chain: "pid 4321 makepad-app-aires, from 1 launchd".into(),
        why: "this copy of aires was started by an agent: quit it from its menu-bar icon and open aires from Spotlight".into(),
    };
    store.serve(r#"answer {"id":"Q1","choice":2}"#, &refused, &mut Vec::new());
    store
}

#[cfg(test)]
pub fn demo_questions() -> Vec<Question> {
    let values = Connection::Demo(demo()).request("follow {\"rev\":0}").unwrap();
    snapshot(values).unwrap().1
}
