//! The vision judge on the AI fleet: one grab and one acceptance text go to the vision model on the
//! fastest hub that has it loaded ([`judges`]), and the answer comes back with its verdict. An
//! acceptance text is numbered questions, each ending in the answer that passes it ([`Acceptance`]);
//! it passes when the model's answers match them. The model sees the questions, not those answers.
//! The model stays loaded on that hub between grabs. A hub that fails is left out and the next one
//! asked. Shared by ci-leg and the devhub master; neither runs a local model.

use makepad_ai_hub::client::{ContentProvider, LocalService};
use makepad_ai_hub::fleet::{self, BoxSnapshot};
use makepad_ai_hub::job::{fleet_snapshots, generate, process_origin, FleetRouter, GenPick, GenRouter};
use makepad_ai_hub::protocol::GenerateRequestJson;
use makepad_ai_hub::registry::Domain;
use makepad_strict_json::{self as json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The CI box's model (its ci.toml `model`).
pub const MODEL: &str = "qwen3.5-4b-vision";
/// The answer's budget: a line per numbered question.
const MAX_TOKENS: u32 = 400;
/// Hubs asked for one grab before the judge gives up.
const ATTEMPTS: usize = 3;
/// With no hub able to take it now (they update and restart with landings), a grab waits this long.
const NO_HUB_WAIT: Duration = Duration::from_secs(180);
/// How long a box tries to reach any hub before it says it cannot.
const FLEET_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct Verdict {
    pub yes: bool,
    pub raw: String,
    pub secs: f64,
    /// the hub that answered
    pub hub: String,
    pub parse_error: bool,
    /// why it is no pass: the questions answered otherwise than expected, or what the reply lacks
    pub why: String,
}

impl Verdict {
    pub fn json(&self) -> Value {
        json::obj(vec![("yes", Value::Bool(self.yes)), ("raw", json::s(&self.raw)), ("seconds", Value::F64(self.secs)), ("hub", json::s(&self.hub)), ("parse_error", Value::Bool(self.parse_error)), ("why", json::s(&self.why))])
    }
}

/// The answer form the judge asks for after the questions of an [`Acceptance`].
const FORM: &str = "Answer each numbered question on its own line, in exactly this form: the question's number, a colon, YES or NO, a dash, then what you see. Write nothing else.";

/// Words that turn a question around. A question asks plainly about the thing itself and expects NO
/// when it must be absent: five smokes failed (11-03) on the model answering "NO - there is no
/// error panel" to "No error panel covers the window", a point that held.
const NEGATIONS: [&str; 7] = ["not", "no", "never", "none", "nothing", "without", "nor"];

/// One numbered question of an acceptance text and the answer that passes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub question: String,
    pub expect: bool,
}

/// An acceptance text of numbered questions, each ending in the answer that passes it:
/// `<what the picture is> 1. Is the whole picture one flat colour? expected NO 2. Does ...? expected YES`.
/// Quoted text ("no headset") is the picture's, not the question's.
#[derive(Clone, Debug, PartialEq)]
pub struct Acceptance {
    pub context: String,
    pub points: Vec<Point>,
}

impl Acceptance {
    /// The questions of `text`.
    pub fn parse(text: &str) -> Result<Acceptance, String> {
        let marks = expected_marks(text);
        if marks.is_empty() {
            return Err("no expected answers: an acceptance text asks numbered questions, each ending in `expected YES` or `expected NO`".into());
        }
        let mut context = "";
        let mut points = Vec::new();
        let mut from = 0;
        for (i, &(at, end, expect)) in marks.iter().enumerate() {
            let k = i + 1;
            let head = &text[from..at];
            let lead = format!("{k}. ");
            let question = if k == 1 {
                let q = head.rfind(&lead).ok_or("question 1 has no `1. ` before its expected answer")?;
                context = head[..q].trim();
                &head[q + lead.len()..]
            } else {
                let h = head.trim_start_matches(|c: char| c == '.' || c.is_whitespace());
                h.strip_prefix(&lead).ok_or_else(|| format!("question {k} does not start with `{lead}` after the expected answer of question {i}"))?
            };
            let question = question.trim();
            if question.is_empty() {
                return Err(format!("question {k} is empty"));
            }
            if let Some(word) = negation(question) {
                return Err(format!("question {k} asks with a negation ({word}): ask about the thing itself and expect NO when it must be absent"));
            }
            points.push(Point { question: question.into(), expect });
            from = end;
        }
        let tail = text[from..].trim_start_matches(|c: char| c == '.' || c.is_whitespace());
        if !tail.is_empty() {
            return Err(format!("text after the last expected answer: {tail}"));
        }
        Ok(Acceptance { context: context.into(), points })
    }

    /// What the model is asked: the context, each question on its own line without its expected
    /// answer, and the answer form.
    pub fn prompt(&self) -> String {
        let mut p = self.context.clone();
        for (i, point) in self.points.iter().enumerate() {
            p.push_str(&format!("\n{}. {}", i + 1, point.question));
        }
        format!("{}\n{FORM}", p.trim_start())
    }

    /// The reply's first answer to each question: YES or NO and what it says it sees.
    fn answers(&self, raw: &str) -> Vec<Option<(bool, String)>> {
        let mut said: Vec<Option<(bool, String)>> = vec![None; self.points.len()];
        for (k, yes, text) in raw.lines().filter_map(answer_line) {
            if let Some(slot) = k.checked_sub(1).and_then(|i| said.get_mut(i)) {
                slot.get_or_insert((yes, text));
            }
        }
        said
    }

    /// The numbers of the questions the reply leaves unanswered (a small model loops, or runs out
    /// of tokens, before the last): a judge asks those outright.
    pub fn unanswered(&self, raw: &str) -> Vec<usize> {
        self.answers(raw).iter().enumerate().filter(|(_, a)| a.is_none()).map(|(i, _)| i + 1).collect()
    }

    /// The questions the reply answers otherwise than expected (none: a pass); a question it leaves
    /// unanswered is an error.
    pub fn misses(&self, raw: &str) -> Result<Vec<String>, String> {
        let said = self.answers(raw);
        let word = |yes: bool| if yes { "YES" } else { "NO" };
        let mut misses = Vec::new();
        for (i, (point, answer)) in self.points.iter().zip(&said).enumerate() {
            let Some((yes, text)) = answer else { return Err(format!("question {} unanswered (YES or NO)", i + 1)) };
            if *yes != point.expect {
                misses.push(format!("{}. {} answered {}, expected {}: {text}", i + 1, point.question, word(*yes), word(point.expect)));
            }
        }
        Ok(misses)
    }
}

/// Where each `expected YES` / `expected NO` of a text is: (start, end, the answer).
fn expected_marks(text: &str) -> Vec<(usize, usize, bool)> {
    const MARK: &str = "expected ";
    let mut marks = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(MARK) {
        let at = from + i;
        let word = text[at + MARK.len()..].split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("");
        match word {
            "YES" => marks.push((at, at + MARK.len() + 3, true)),
            "NO" => marks.push((at, at + MARK.len() + 2, false)),
            _ => {}
        }
        from = at + MARK.len();
    }
    marks
}

/// The first word of a question (outside quotes) that turns it around.
fn negation(question: &str) -> Option<String> {
    let unquoted: Vec<&str> = question.split('"').step_by(2).collect();
    unquoted
        .join(" ")
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(str::to_lowercase)
        .find(|w| NEGATIONS.contains(&w.as_str()) || w.ends_with("n't"))
}

/// One answer line: its question's number, YES or NO, and what the model says it sees. The shapes
/// the model writes: `1: YES - text`, `1. YES - text`, `**1:** Yes, text`, `- Question 2: NO - text`,
/// and its own list number before the form's (`1. 1: YES-text`).
fn answer_line(line: &str) -> Option<(usize, bool, String)> {
    let mut l = line.trim().trim_start_matches(|c: char| matches!(c, '-' | '*' | '#' | '>') || c.is_whitespace());
    for word in ["Question", "question", "Point", "point"] {
        if let Some(rest) = l.strip_prefix(word) {
            l = rest.trim_start();
        }
    }
    let mut k = None;
    loop {
        let digits = l.len() - l.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            break;
        }
        k = l[..digits].parse::<usize>().ok();
        l = l[digits..].trim_start_matches(|c: char| matches!(c, ':' | '.' | ')' | '*' | '-') || c.is_whitespace());
    }
    let k = k?;
    let len = l.len() - l.trim_start_matches(|c: char| c.is_ascii_alphabetic()).len();
    let yes = match l[..len].to_ascii_uppercase().as_str() {
        "YES" => true,
        "NO" => false,
        _ => return None,
    };
    let text = l[len..].trim_start_matches(|c: char| matches!(c, '*' | '-' | ':' | ',' | '.' | '\u{2013}' | '\u{2014}') || c.is_whitespace());
    Some((k, yes, text.trim_end().to_string()))
}

/// The fleet router minus the hubs that failed this judge.
struct Avoiding {
    failed: Arc<Mutex<Vec<String>>>,
    /// the hub the last route picked
    picked: Arc<Mutex<Option<String>>>,
}

impl GenRouter for Avoiding {
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        FleetRouter::default().use_idle_admission(domain, request)
    }

    fn route(&self, domain: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
        let mut all = excluded.to_vec();
        all.extend(self.failed.lock().unwrap().iter().cloned());
        let pick = FleetRouter { installed_only: false }.route_snapshots(judges(fleet_snapshots(&all)), domain, request)?;
        *self.picked.lock().unwrap() = Some(pick.base_url.clone());
        Ok(pick)
    }
}

/// The hubs a grab goes to: those with the model loaded, else those with it ready (installed),
/// else every hub; of them, the fastest GPU's (the fleet's VRAM classes: a 96 GB card, then a
/// 32 GB card, ...). A model is never loaded just to pick a box while another hub has it loaded.
pub fn judges(snapshots: Vec<BoxSnapshot>) -> Vec<BoxSnapshot> {
    let state = |s: &BoxSnapshot| fleet::affinity(s, MODEL);
    let pool: Vec<BoxSnapshot> = [4, 3]
        .iter()
        .map(|want| snapshots.iter().filter(|s| state(s) == Some(*want)).cloned().collect::<Vec<_>>())
        .find(|v| !v.is_empty())
        .unwrap_or(snapshots);
    let best = pool.iter().map(fleet::gpu_rank).max().unwrap_or(0);
    pool.into_iter().filter(|s| fleet::gpu_rank(s) == best).collect()
}

pub struct Judge {
    failed: Arc<Mutex<Vec<String>>>,
    /// no fleet: every judgement says so (a failed step, not a pass)
    pub off: Option<String>,
}

impl Judge {
    /// Starts fleet discovery (hubs announce every 2 s; a first route waits for them).
    /// The caller must already have LAN access. Do not mutate process-wide environment here:
    /// devhub calls this from its shard workers while other threads are running.
    pub fn start() -> Judge {
        let d = makepad_ai_hub::discovery::start_listener();
        let off = match &d {
            makepad_ai_hub::discovery::Discovery::Unavailable { reason } => Some(reason.to_string()),
            _ => None,
        };
        Judge { failed: Arc::new(Mutex::new(Vec::new())), off }
    }

    /// Waits until a hub is heard (at most `secs`).
    pub fn wait_for_hubs(&self, secs: f64) -> usize {
        let t = Instant::now();
        loop {
            let n = makepad_ai_hub::discovery::start_listener().nodes().len();
            if n > 0 || t.elapsed().as_secs_f64() >= secs {
                return n;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Why this box cannot reach the fleet at all (its OS keeps this process off the LAN), after
    /// the time a fleet-wide hub restart takes; nothing when a hub answers.
    pub fn unreachable(&self) -> Option<String> {
        if let Some(why) = &self.off {
            return Some(format!("no AI fleet ({why})"));
        }
        let t = Instant::now();
        loop {
            let nodes = makepad_ai_hub::discovery::start_listener().nodes();
            if nodes.iter().any(|n| LocalService::new(&n.base_url).health().is_ok()) {
                return None;
            }
            if t.elapsed() >= FLEET_WAIT {
                return Some(if nodes.is_empty() { format!("no AI fleet hub heard in {} s", FLEET_WAIT.as_secs()) } else { format!("no hub answers{}", health_errors()) });
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// One grab judged against `prompt`.
    pub fn judge(&self, image: &Path, prompt: &str) -> Result<Verdict, String> {
        if let Some(why) = &self.off {
            return Err(format!("vision judge: no AI fleet ({why})"));
        }
        let ask = Acceptance::parse(prompt).map_err(|e| format!("acceptance text: {e}"))?;
        let prompt = ask.prompt();
        let bytes = std::fs::read(image).map_err(|e| format!("{}: {e}", image.display()))?;
        if self.wait_for_hubs(6.0) == 0 {
            return Err("vision judge: no AI fleet hub heard in 6 s".into());
        }
        let mut last = String::new();
        let asked = Instant::now();
        let mut hubs_failed = 0;
        while hubs_failed < ATTEMPTS {
            let request = GenerateRequestJson {
                model: MODEL.into(),
                prompt: Some(prompt.clone()),
                input_b64: Some(makepad_network::digest::base64_encode(&bytes)),
                input_content_type: Some("image/png".into()),
                max_tokens: Some(MAX_TOKENS),
                ..Default::default()
            };
            let t = Instant::now();
            let picked = Arc::new(Mutex::new(None));
            let router: Arc<dyn GenRouter> = Arc::new(Avoiding { failed: self.failed.clone(), picked: picked.clone() });
            match generate(router, process_origin("ci-judge"), Domain::Vision, request, makepad_ai_hub::job::Want::Outputs { media: 0, text: 1 }, &|| false, &mut |_, _| {}, Duration::from_millis(200)) {
                Ok(out) => {
                    let raw = out.text.unwrap_or_default();
                    let judged = ask.misses(&raw).map(|misses| (misses.is_empty(), misses.join("\n")));
                    let (yes, why, parse_error) = match judged {
                        Ok((yes, why)) => (yes, why, false),
                        Err(e) => (false, e, true),
                    };
                    return Ok(Verdict { yes, raw, secs: t.elapsed().as_secs_f64(), hub: out.node.clone(), parse_error, why });
                }
                Err(e) => {
                    last = e.to_string();
                    // the hub it went to is left out next time (a dead or failing hub); no pick: no hub
                    match picked.lock().unwrap().take() {
                        Some(hub) => {
                            last = format!("{hub}: {last}");
                            self.failed.lock().unwrap().push(hub);
                            hubs_failed += 1;
                        }
                        None if asked.elapsed() >= NO_HUB_WAIT => return Err(format!("vision judge: {last}{}", health_errors())),
                        None => std::thread::sleep(Duration::from_secs(10)),
                    }
                }
            }
        }
        Err(format!("vision judge: {last}"))
    }
}

/// Why each hub heard does not answer `/health` (what the router's "unavailable" leaves out).
fn health_errors() -> String {
    let errors: Vec<String> = makepad_ai_hub::discovery::start_listener().nodes().iter().filter_map(|n| LocalService::new(&n.base_url).health().err().map(|e| format!("{}: {e}", n.base_url))).collect();
    if errors.is_empty() {
        String::new()
    } else {
        format!(" (health: {})", errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_answer_is_compared_with_the_one_its_question_expects() {
        let text = "One window. 1. Is the whole picture one flat colour? expected NO 2. Is there a toolbar? expected YES 3. Does an error panel cover the window? expected NO";
        let ask = Acceptance::parse(text).unwrap();
        assert_eq!(ask.prompt(), format!("One window.\n1. Is the whole picture one flat colour?\n2. Is there a toolbar?\n3. Does an error panel cover the window?\n{FORM}"), "the model never sees the expected answers");
        // the 11-03 replies, answered to their questions
        assert_eq!(ask.misses("1: NO - greys and labels\n2: YES - a toolbar\n3: NO - there is no error panel covering the window."), Ok(vec![]));
        assert_eq!(ask.misses("1. 1: NO-grey\n2. 2: YES-a toolbar\n3. 3: NO-none"), Ok(vec![]), "its own list number before the form's");
        assert_eq!(ask.misses("**1:** No - grey\n* Question 2: **Yes**, a bar\n- 3 - no"), Ok(vec![]));
        assert_eq!(ask.misses("1: NO - grey\n2: YES - bar\n3: YES - a panic text\n3: NO - none"), Ok(vec!["3. Does an error panel cover the window? answered YES, expected NO: a panic text".to_string()]), "its first answer counts");
        assert_eq!(ask.misses("1: NO - grey\n3: NO - none"), Err("question 2 unanswered (YES or NO)".into()));
        assert_eq!(ask.unanswered("1: NO - grey\n3: NO - none"), vec![2], "what a judge asks outright");
        assert_eq!(ask.unanswered("1: NO - grey\n2: YES - bar\n3: NO"), Vec::<usize>::new());
        assert_eq!(ask.misses("1: center, YES - a window\n2: YES\n3: NO"), Err("question 1 unanswered (YES or NO)".into()), "an answer leads with YES or NO");
    }

    #[test]
    fn an_acceptance_text_asks_plainly_and_names_each_answer() {
        assert!(Acceptance::parse("Is it a calculator? Then write VERDICT: YES or VERDICT: NO").unwrap_err().starts_with("no expected answers"), "one form only");
        let quoted = Acceptance::parse("A dark window. 1. Is there a line of text with the words \"no headset\"? expected YES").unwrap();
        assert_eq!(quoted.points, vec![Point { question: "Is there a line of text with the words \"no headset\"?".into(), expect: true }]);
        assert_eq!(Acceptance::parse("1. No error panel covers the window? expected YES"), Err("question 1 asks with a negation (no): ask about the thing itself and expect NO when it must be absent".into()));
        assert!(Acceptance::parse("1. Isn't it blank? expected NO").unwrap_err().contains("(isn't)"));
        assert!(Acceptance::parse("1. Is it blank? expected NO 3. Is it red? expected NO").unwrap_err().starts_with("question 2 does not start with `2. `"));
        assert!(Acceptance::parse("1. Is it blank? expected NO Write nothing else.").unwrap_err().starts_with("text after the last expected answer"));
        assert!(Acceptance::parse("It runs as expected NOWhere else. Is it blank?").unwrap_err().starts_with("no expected answers"), "an expected answer is YES or NO, a whole word");
        assert_eq!(Acceptance::parse("Is it blank? expected NO"), Err("question 1 has no `1. ` before its expected answer".into()));
    }

    /// The string literals of a line of Splash, joined (`FRAME + "..."` gives the second; FRAME has
    /// no questions).
    fn literals(line: &str) -> String {
        let (mut out, mut inside, mut chars) = (String::new(), false, line.chars());
        while let Some(c) = chars.next() {
            match (inside, c) {
                (_, '"') => inside = !inside,
                (true, '\\') => out.extend(chars.next()),
                (true, c) => out.push(c),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn every_acceptance_text_in_the_tree_asks_plainly() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = vec![root.join("ci.splash")];
        for dir in ["apps", "tools/ci_leg"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap().flatten() {
                let p = entry.path();
                files.push(if p.extension().is_some_and(|x| x == "splash") { p } else { p.join("ci.splash") });
            }
        }
        // the commercial apps' too, where the commercial checkout is beside this one
        for entry in std::fs::read_dir(root.join("apps/commercial")).into_iter().flatten().flatten() {
            files.extend([entry.path().join("ci.splash"), entry.path().join("app/ci.splash")]);
        }
        let mut texts = 0;
        for file in files.iter().filter(|f| f.is_file()) {
            for line in std::fs::read_to_string(file).unwrap().lines().filter(|l| l.trim_start().starts_with("ci.accept(")) {
                match Acceptance::parse(&literals(line)) {
                    Ok(_) => texts += 1,
                    Err(e) => panic!("{}: {e}: {}", file.display(), line.trim()),
                }
            }
        }
        assert!(texts >= 30, "only {texts} acceptance texts found");
    }
}
