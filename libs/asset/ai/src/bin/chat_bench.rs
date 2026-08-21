//! Chat serving bench — the standing "is it slow right now?" check.
//!
//! ONE COMMAND, any time:
//!
//! ```text
//! cargo run --release --bin chat-bench -- http://10.0.0.165:8123
//! ```
//!
//! Add `--interleave` for the second arm (below). `--turns N` changes the
//! conversation length, `--model ID` the model.
//!
//! It exercises the SERVING path a game chat turn actually takes — a real
//! multi-turn conversation against a real box — and reports the four numbers
//! that decide whether chat feels fast, none of which a single tok/s can say:
//!
//! - **time to first VISIBLE text.** Qwen3.8 runs an open think block, so a
//!   turn generates its reasoning before one readable character. That wait is
//!   what a person calls "slow", and it is invisible to any measurement that
//!   counts tokens.
//! - **visible tok/s against total tok/s.** The box's rate and the user's rate
//!   are different numbers, and the gap is the think block.
//! - **cache warmth.** Turn 2 onward must RESUME: a warm turn ingests a
//!   handful of tokens where a cold one re-reads the whole conversation, and
//!   that difference is seconds the user feels but cannot attribute.
//! - **no overflow.** A conversation that outgrows its lane must compact and
//!   carry on, never surface a gguf-layer error.
//!
//! Exits non-zero with one line saying which of those failed, so it works as a
//! deploy gate and not only as a report.
//!
//! **The interleaving caveat**: a concurrent conversation on the same box will
//! take a lane, and before per-lane prefix caches landed it would also take
//! this bench's warmth. `--interleave` runs a SECOND conversation between this
//! one's turns and asserts BOTH stay warm — which is exactly the per-lane
//! acceptance test, and which fails by design on a box that predates them.

use std::time::{Duration, Instant};

/// Warm turns must ingest less than this fraction of their prompt. A resumed
/// turn ingests only the delta — a few dozen tokens against thousands — so the
/// bar is loose on purpose: it is catching "the whole history again", not
/// grading the cache.
const WARM_INGEST_FRACTION: f64 = 0.5;
/// Below this, something is wrong at a level worth failing over. Deliberately
/// far under any box's real rate: this is a floor, not a target, and a gate
/// that fails on ordinary variance gets ignored.
const MIN_VISIBLE_TOKENS_PER_SEC: f64 = 5.0;
/// A turn that takes longer than this has not "been slow", it has hung.
const TURN_TIMEOUT: Duration = Duration::from_secs(300);

fn main() {
    let mut args = std::env::args().skip(1);
    let base = args.next().unwrap_or_else(|| {
        eprintln!(
            "usage: chat-bench <http://box:8123> [--turns N] [--model ID] \
             [--interleave] [--as-a-game-client] [--long] [--system-words N]"
        );
        std::process::exit(2);
    });
    let base = base.trim_end_matches('/').to_string();
    let mut turns = 3usize;
    let mut model = "qwen3.8-27b".to_string();
    let mut interleave = false;
    // Behave like the real game client instead of echoing the reply back.
    let mut as_a_game_client = false;
    // Ask for long answers so the rate is decode-bound.
    let mut long = false;
    // Words of taught context. The default is deliberately game-sized: a
    // bench with a one-line prompt does not test prefill at all.
    let mut system_words = 2000usize;
    let rest: Vec<String> = args.collect();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--turns" => {
                turns = rest.get(index + 1).and_then(|v| v.parse().ok()).unwrap_or(turns);
                index += 2;
            }
            "--model" => {
                model = rest.get(index + 1).cloned().unwrap_or(model);
                index += 2;
            }
            "--interleave" => {
                interleave = true;
                index += 1;
            }
            "--as-a-game-client" => {
                as_a_game_client = true;
                index += 1;
            }
            "--long" => {
                long = true;
                index += 1;
            }
            "--system-words" => {
                system_words = rest
                    .get(index + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(system_words);
                index += 2;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    println!(
        "chat-bench {base}  model={model}  turns={turns}  interleave={interleave}  \
         game-client={as_a_game_client}  system~{system_words} words"
    );
    let mut failures: Vec<String> = Vec::new();
    let mut main_chat = Conversation::new("bench-primary", system_words, as_a_game_client, long);
    let mut other_chat =
        Conversation::new("bench-interleaved", system_words, as_a_game_client, long);

    for turn in 1..=turns {
        // The interleaved arm runs BETWEEN the primary's turns, which is the
        // shape that used to steal warmth: one shared cache belongs to
        // whoever spoke last.
        if interleave && turn > 1 {
            match run_turn(&base, &model, &mut other_chat, turn) {
                Ok(result) => report(&result, "  [interleaved]"),
                Err(e) => failures.push(format!("interleaved turn {turn}: {e}")),
            }
        }
        match run_turn(&base, &model, &mut main_chat, turn) {
            Ok(result) => {
                report(&result, "");
                failures.extend(check(&result, turn));
            }
            Err(e) => failures.push(format!("turn {turn}: {e}")),
        }
    }

    if failures.is_empty() {
        println!("PASS: conversation stayed warm and served at rate");
        return;
    }
    for failure in &failures {
        eprintln!("FAIL: {failure}");
    }
    std::process::exit(1);
}

/// One turn's outcome, in the units a person experiences.
struct TurnResult {
    turn: usize,
    /// Generated tokens per second between the first and last poll that carried
    /// any — the client meter's own arithmetic.
    meter: Option<f64>,
    /// Longest wall gap between two polls that carried tokens.
    biggest_gap: f64,
    /// How many polls carried tokens at all.
    deltas: usize,
    /// Wall time until the first character the user could READ — after the
    /// think block, not after the first token.
    first_visible: Option<Duration>,
    total: Duration,
    visible_tokens: u64,
    think_tokens: u64,
    prefix_ingested: Option<u64>,
    prefix_resumed: Option<bool>,
    prompt_tokens_estimate: u64,
    text: String,
}

impl TurnResult {
    fn visible_rate(&self) -> f64 {
        self.visible_tokens as f64 / self.total.as_secs_f64().max(1e-9)
    }
    fn total_rate(&self) -> f64 {
        (self.visible_tokens + self.think_tokens) as f64 / self.total.as_secs_f64().max(1e-9)
    }
}

fn report(result: &TurnResult, prefix: &str) {
    let warmth = match (result.prefix_resumed, result.prefix_ingested) {
        (Some(true), Some(n)) => format!("WARM (ingested {n})"),
        (Some(false), Some(n)) => format!("cold (ingested {n})"),
        _ => "warmth not reported".to_string(),
    };
    let ttfv = result
        .first_visible
        .map(|d| format!("{:.2}s", d.as_secs_f64()))
        .unwrap_or_else(|| "never".to_string());
    let meter = result
        .meter
        .map(|r| format!("{r:.1}"))
        .unwrap_or_else(|| "n/a".to_string());
    println!(
        "{prefix}turn {}: {warmth}  ttf-visible {ttfv}  total {:.2}s  \
         visible {:.1} tok/s ({} tok)  all {:.1} tok/s (think {})  \
         METER {meter} tok/s ({} deltas, worst gap {:.2}s)",
        result.turn,
        result.total.as_secs_f64(),
        result.visible_rate(),
        result.visible_tokens,
        result.total_rate(),
        result.think_tokens,
        result.deltas,
        result.biggest_gap,
    );
}

fn check(result: &TurnResult, turn: usize) -> Vec<String> {
    let mut out = Vec::new();
    if turn > 1 {
        match result.prefix_resumed {
            Some(true) => {
                if let Some(ingested) = result.prefix_ingested {
                    let limit =
                        (result.prompt_tokens_estimate as f64 * WARM_INGEST_FRACTION) as u64;
                    if ingested > limit.max(64) {
                        out.push(format!(
                            "turn {turn} claims warm but ingested {ingested} of ~{} prompt \
                             tokens — the append is not an append",
                            result.prompt_tokens_estimate
                        ));
                    }
                }
            }
            Some(false) => out.push(format!(
                "turn {turn} was COLD: the conversation's lane did not keep its cache, so \
                 the whole history was re-read"
            )),
            None => out.push(format!(
                "turn {turn} reported no warmth at all — the box predates the serving block, \
                 so this bench cannot gate it"
            )),
        }
    }
    if result.visible_tokens == 0 {
        out.push(format!("turn {turn} produced no visible text"));
    } else if result.visible_rate() < MIN_VISIBLE_TOKENS_PER_SEC {
        out.push(format!(
            "turn {turn} served {:.1} visible tok/s, under the {MIN_VISIBLE_TOKENS_PER_SEC} floor",
            result.visible_rate()
        ));
    }
    out
}

/// A system prompt long enough to be REAL.
///
/// A one-line prompt exercises none of what a game session does. The bug that
/// put "loading" on every one of a player's messages — a whole prompt in one
/// prefill graph, gigabytes of activations on a high lane — was invisible to
/// every gate that used thirty-token prompts, including this bench's first
/// version. Prefill cost scales with prompt length; a bench that never carries
/// a long one is not testing prefill.
const TAUGHT_CONTEXT: &str =
    "You are a helpful assistant inside a game world. Answer briefly. \
     Reference material follows, and you may use it. ";

fn long_system_prompt(words: usize) -> String {
    let mut out = String::from(TAUGHT_CONTEXT);
    // ~4 bytes a token, so this lands near `words` tokens.
    while out.split_whitespace().count() < words {
        out.push_str("The world contains objects, rules, places and people. ");
    }
    out
}

/// A growing chat, fed back verbatim — which is the only way warmth can hit:
/// the box matches a prompt that EXTENDS what its lane already holds, so an
/// approximation of the reply is a different conversation.
struct Conversation {
    id: &'static str,
    /// Carried per conversation so two of them are genuinely different
    /// prompts, not the same one twice.
    system: String,
    messages: Vec<(String, String)>,
    /// Store replies the way the game client stores them — reasoning removed —
    /// and carry tool results between turns.
    as_a_game_client: bool,
    /// Ask for long answers, so the measured rate is the decode rate.
    long_answers: bool,
}

impl Conversation {
    fn new(
        id: &'static str,
        system_words: usize,
        as_a_game_client: bool,
        long_answers: bool,
    ) -> Self {
        let mut system = long_system_prompt(system_words);
        // Two conversations must not share a prompt, or "both stayed warm"
        // could be one lane serving both.
        system.push_str(id);
        Self { id, system, messages: Vec::new(), as_a_game_client, long_answers }
    }

    fn next_question(&self) -> String {
        // A rate read off a 30-token reply is mostly HTTP and the poll
        // interval: at 120 ms granularity a turn that decodes in 300 ms
        // measures as whatever the poller happened to see. `--long` asks for
        // enough output that the number is the decode rate, which is the one
        // the user's meter shows.
        if self.long_answers {
            const LONG: [&str; 3] = [
                "Write a 60-line poem about a lighthouse keeper who is afraid of the dark.",
                "Write another 60 lines, same keeper, the morning after.",
                "Now 60 more, from the point of view of the light itself.",
            ];
            let asked = self
                .messages
                .iter()
                .filter(|(role, text)| role == "user" && !text.starts_with("<tool_response>"))
                .count();
            return LONG[asked % LONG.len()].to_string();
        }
        const QUESTIONS: [&str; 6] = [
            "In one sentence, what is a GPU?",
            "And what is a CPU?",
            "Which of the two suits graphics better?",
            "Why does that matter for a game?",
            "Name one thing that changes at high resolution.",
            "Summarise this conversation in one line.",
        ];
        let asked = self
            .messages
            .iter()
            .filter(|(role, text)| role == "user" && !text.starts_with("<tool_response>"))
            .count();
        QUESTIONS[asked % QUESTIONS.len()].to_string()
    }
}

fn run_turn(
    base: &str,
    model: &str,
    chat: &mut Conversation,
    turn: usize,
) -> Result<TurnResult, String> {
    let question = chat.next_question();
    chat.messages.push(("user".to_string(), question));
    let body = request_json(model, &chat.system, &chat.messages);
    // A rough token count, only used to size the warmth bar. Four bytes per
    // token is close enough for "did this re-read everything?".
    let prompt_tokens_estimate = (body.len() / 4) as u64;

    let started = Instant::now();
    let response = post(&format!("{base}/generate"), &body)?;
    let job = field_str(&response, "job_id")
        .ok_or_else(|| format!("no job id in {response}"))?;

    let mut first_visible = None;
    let mut last = String::new();
    // THE METER, measured the way the client computes it: generated-token
    // deltas over the wall time between the polls that carried them. A
    // service-side round rate says what the GPU did; this says what the person
    // watching the number sees, and the two came apart by a factor of two on
    // the live box.
    let mut meter_first: Option<(Instant, u64)> = None;
    let mut meter_last: Option<(Instant, u64)> = None;
    let mut meter_gaps: Vec<(f64, u64)> = Vec::new();
    loop {
        if started.elapsed() > TURN_TIMEOUT {
            return Err(format!("turn {turn} did not finish in {TURN_TIMEOUT:?}"));
        }
        std::thread::sleep(Duration::from_millis(120));
        let status = get(&format!("{base}/job/{job}"))?;
        let partial = field_str(&status, "partial_text").unwrap_or_default();
        // First VISIBLE text, not first token: everything before the think
        // block closes is reasoning the user never sees.
        if first_visible.is_none() {
            // A reply that never opens a think block is visible from its first
            // character. Waiting for `</think>` in that case reports "never"
            // for the fastest turns there are — which is how a brief-think run
            // came back reading like a broken one.
            let visible = if partial.contains("<think>") {
                partial.split("</think>").nth(1).unwrap_or("")
            } else {
                partial.as_str()
            };
            if !visible.trim().is_empty() {
                first_visible = Some(started.elapsed());
            }
        }
        if let Some(gen) = field_u64(&status, "gen_tokens") {
            let now = Instant::now();
            if meter_first.is_none() && gen > 0 {
                meter_first = Some((now, gen));
            }
            if let Some((prev_at, prev_gen)) = meter_last {
                if gen > prev_gen {
                    meter_gaps.push((now.duration_since(prev_at).as_secs_f64(), gen - prev_gen));
                }
            }
            meter_last = Some((now, gen));
        }
        last = status.clone();
        match field_str(&status, "state").as_deref() {
            Some("done") => break,
            Some("failed") | Some("error") => {
                return Err(field_str(&status, "error").unwrap_or_else(|| "failed".into()))
            }
            Some("cancelled") => return Err("cancelled".into()),
            _ => {}
        }
    }
    let total = started.elapsed();
    let text = field_str(&last, "partial_text").unwrap_or_default();
    // What the CLIENT stores, which is not always what the model wrote.
    //
    // `--as-a-game-client` reproduces `libs/asset/chat/src/qwen.rs`: reasoning
    // is stripped before the reply is kept, and the closing tag is put back on
    // the way out because the service opens a block on every assistant turn it
    // renders. Feeding the reply back VERBATIM — this bench's original
    // behaviour — is the one shape in which warmth could hit, so a bench that
    // only does that will report a warm box while every real conversation is
    // cold. That is exactly what happened.
    let stored = if chat.as_a_game_client {
        let visible = text.rsplit("</think>").next().unwrap_or(&text).trim_start();
        format!("\n</think>\n\n{visible}")
    } else {
        text.clone()
    };
    chat.messages.push(("assistant".to_string(), stored));
    // A game turn is a tool call and its result, not two lines of prose. The
    // client wraps tool outcomes as user-role `<tool_response>`, so that is
    // what the next prompt carries.
    if chat.as_a_game_client {
        chat.messages.push((
            "user".to_string(),
            format!("<tool_response>\nok, applied ({} chars)\n</tool_response>", text.len()),
        ));
    }

    // Sustained rate between the first delta that carried tokens and the last,
    // which is what a meter averaging over its window converges to. The
    // per-delta spread is reported too: a burst of 24 tokens every 270 ms and a
    // steady trickle read the same on average and feel nothing alike.
    let meter = match (meter_first, meter_last) {
        (Some((a, ga)), Some((b, gb))) if gb > ga && b > a => {
            Some((gb - ga) as f64 / b.duration_since(a).as_secs_f64())
        }
        _ => None,
    };
    let biggest_gap = meter_gaps
        .iter()
        .map(|(secs, _)| *secs)
        .fold(0.0f64, f64::max);
    let deltas = meter_gaps.len();
    Ok(TurnResult {
        turn,
        meter,
        biggest_gap,
        deltas,
        first_visible,
        total,
        visible_tokens: field_u64(&last, "visible_tokens").unwrap_or(0),
        think_tokens: field_u64(&last, "think_tokens").unwrap_or(0),
        prefix_ingested: field_u64(&last, "prefix_ingested"),
        prefix_resumed: field_bool(&last, "prefix_resumed"),
        prompt_tokens_estimate,
        text,
    })
}

fn request_json(model: &str, system: &str, messages: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str("{\"model\":\"");
    out.push_str(model);
    out.push_str("\",\"domain\":\"chat\",\"chat_system\":\"");
    out.push_str(&escape(system));
    out.push_str("\",\"chat_messages\":[");
    for (index, (role, text)) in messages.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"role\":\"");
        out.push_str(role);
        out.push_str("\",\"text\":\"");
        out.push_str(&escape(text));
        out.push_str("\"}");
    }
    out.push_str("],\"max_tokens\":1200,\"temperature\":0.7,\"seed\":7}");
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// --- the smallest HTTP that does the job -----------------------------------
//
// Deliberately dependency-free: a bench that has to be built before it can
// tell you the box is slow is a bench nobody runs.

fn post(url: &str, body: &str) -> Result<String, String> {
    http(url, Some(body))
}

fn get(url: &str) -> Result<String, String> {
    http(url, None)
}

fn http(url: &str, body: Option<&str>) -> Result<String, String> {
    use std::io::{Read, Write};
    let rest = url.strip_prefix("http://").ok_or("only http:// urls")?;
    let (host_port, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let mut stream = std::net::TcpStream::connect(host_port)
        .map_err(|e| format!("connect {host_port}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    let request = match body {
        Some(body) => format!(
            "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        None => format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"
        ),
    };
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let at = text
        .find("\r\n\r\n")
        .ok_or_else(|| "response has no body".to_string())?;
    Ok(text[at + 4..].to_string())
}

// --- just enough JSON to read the fields this bench needs -------------------

fn field_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let at = json.find(&needle)? + needle.len();
    let rest = json[at..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) =
                        u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                    {
                        out.push(c);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    Some(out)
}

fn field_u64(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\":");
    let at = json.find(&needle)? + needle.len();
    let rest = json[at..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

fn field_bool(json: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\":");
    let at = json.find(&needle)? + needle.len();
    let rest = json[at..].trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}
