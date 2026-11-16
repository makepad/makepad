use makepad_devhub::ask::{Asks, Question};
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
}

impl Answer {
    fn line(&self) -> String {
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

#[derive(Default)]
struct Shared {
    latest: Mutex<Option<Snapshot>>,
    results: Mutex<Vec<AnswerResult>>,
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
    answers: mpsc::SyncSender<Answer>,
}

impl Source {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        let demo = (std::env::var("AIRES_DEMO").as_deref() == Ok("1")).then(demo);
        let shared = Arc::new(Shared::default());
        let (answers, receive) = mpsc::sync_channel::<Answer>(16);
        let (out, store) = (shared.clone(), demo.clone());
        spawner.spawn_worker(ThreadOptions::default(), move || {
            let mut connection = Connection::new(&store);
            while let Ok(answer) = receive.recv() {
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
        Ok(Self { shared, answers })
    }

    pub fn take(&self) -> (Option<Snapshot>, Vec<AnswerResult>) {
        let latest = self.shared.latest.lock().unwrap().take();
        let results = std::mem::take(&mut *self.shared.results.lock().unwrap());
        (latest, results)
    }

    pub fn answer(&self, answer: Answer) -> Result<(), String> {
        self.answers.try_send(answer).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => "busy, try again".into(),
            mpsc::TrySendError::Disconnected(_) => "answer worker stopped".into(),
        })
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

fn demo_judge() -> Judged {
    Judged { source: Provenance::Tray, chain: "demo".into(), why: String::new() }
}

pub fn demo() -> Arc<Asks> {
    let store = Asks::memory();
    for line in [
        r#"new {"from":"review","title":"Which rollout should we use?","decided":"The review and checks passed.","why":"The change is ready for a first rollout.","blocked":"Choosing the rollout.","options":["Land it","Try it with one team","Wait for the next release"],"recommend":"Try it with one team","context":"The change keeps the existing settings and saved work.\nA small rollout lets us gather feedback before everyone sees it. We can return to the previous version without changing anyone's data.\nThe broader rollout would save a second deployment, but gives us less time to learn from the first users.\nPlease choose the rollout and add any conditions in your answer."}"#,
        r#"new {"from":"design","title":"How should the empty inbox read?","why":"The panel needs an empty state.","options":["No questions","All caught up"]}"#,
        r#"answer {"id":"Q2","ask":true,"text":"Will it also explain a lost connection?"}"#,
        r#"reply {"id":"Q2","text":"Yes, a connection error gets its own line and preserves the cards."}"#,
        r#"answer {"id":"Q2","ask":true,"text":"Can you show that wording before we choose?"}"#,
        r#"new {"from":"layout","title":"Keep the panel narrow?","decided":"Text wraps at the panel width.","options":["Keep it compact","Make it wider"],"recommend":"Keep it compact"}"#,
        r#"answer {"id":"Q3","choice":1,"text":"Keep the longer context behind Show all."}"#,
    ] {
        let mut out = Vec::new();
        store.serve(line, &demo_judge, &mut out);
        debug_assert!(out.iter().all(|line| !line.contains("\"err\"")), "demo request failed: {out:?}");
    }
    store
}

#[cfg(test)]
pub fn demo_questions() -> Vec<Question> {
    let values = Connection::Demo(demo()).request("follow {\"rev\":0}").unwrap();
    snapshot(values).unwrap().1
}
