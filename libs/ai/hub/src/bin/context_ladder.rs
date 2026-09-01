//! Context ladder — the rung where a long conversation goes insane, and the
//! subsystem that did it.
//!
//! ONE COMMAND:
//!
//! ```text
//! cargo run --release --bin context-ladder -- http://10.0.0.165:8123
//! ```
//!
//! **The report this exists for.** A conversation on Qwen3.8-27B "goes insane
//! at larger contexts — like `1.1.1.1.1..4.23.23.4.234.24` kinda output", and
//! crucially "not immediately but after a while". Qwen3.8-27B is
//! architecturally long-context and upstream serves it far past 100k, so this
//! is a bug in OUR stack, not a property of the model. The gradual onset points
//! away from static position math — that would break any long prompt on its
//! first token — and towards ACCUMULATED state: recurrent carry and checkpoints
//! across resumes, a prefix-cache append chain drifting from what a cold ingest
//! would have produced, or compaction splicing the same history twice.
//!
//! **The instrument.** At each rung N (tokens of context) it runs TWO arms, and
//! the pairing IS the diagnosis:
//!
//! - **`fresh`** — ONE cold request carrying ~N tokens of synthetic history, on
//!   a session marker never used before. No resume, no compaction, no carried
//!   state: pure decode at depth. If this degrades, the fault is in the arena /
//!   position / attention-window math and it does not need a second turn to
//!   show up.
//! - **`grown`** — the SAME conversation content, delivered as many real turns
//!   on ONE session, each reply appended the way a client stores it, until the
//!   history reaches ~N tokens. This exercises resume, carry state, and
//!   compaction. If `grown` degrades where `fresh` at the same LENGTH is clean,
//!   the fault lives in the incremental path, and the two arms have named it.
//!
//! Every rung ends with one fixed probe question whose answer is scored
//! mechanically (see the threshold block below), including a canary fact
//! planted at the very start of the history — because "still coherent" and
//! "coherent but amnesiac" are different bugs and a repetition score alone
//! cannot tell them apart.
//!
//! Exits non-zero naming the first failing rung and arm, so this is a standing
//! regression gate for every future serving change and not only a report.
//!
//! **What a "session" is on this wire.** `POST /generate` carries no
//! conversation handle: the whole transcript rides in every body, and the box
//! decides warmth with a literal text-prefix test against what its lane already
//! holds (`libs/asset/ai/src/llm_backend.rs`, `PrefixCache::classify`). So a
//! session id here is a unique marker appended to the system prompt — two
//! markers make two prompts that can never extend each other, and a fresh
//! marker per run makes a cold arm genuinely cold instead of accidentally warm
//! off the previous run.

use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Scoring thresholds
// ---------------------------------------------------------------------------
//
// Every bar here is set where a HUMAN would already have called the output
// broken, not where a statistician would call it unusual. A gate that fires on
// ordinary variance gets ignored, and an ignored gate is worse than none.

/// N-gram widths checked for immediate repetition. 1 catches "the the the",
/// 2-4 catch the phrase loops a degenerating decoder falls into, 6 and 8 catch
/// a whole clause repeating — which reads as fluent English right up until you
/// notice it is the same sentence eleven times.
const NGRAM_SIZES: [usize; 6] = [1, 2, 3, 4, 6, 8];
/// Consecutive repeats of the same n-gram that mean the decoder is looping.
/// Real prose does repeat adjacently — refrains, emphasis, enumerations — but
/// it does not do it six times in a row without an intervening word.
const MAX_NGRAM_RUN: usize = 6;
/// Fraction of the reply's words sitting inside SOME immediately-repeated
/// n-gram. A reply can carry one legitimate refrain; a reply that is more than
/// a third repeats is not answering the question any more.
const MAX_REPEAT_COVERAGE: f64 = 0.35;
/// Where numeric babble starts being a SHAPE worth printing: a run of at least
/// this many characters drawn only from digits, dot, comma, pipe and
/// whitespace — the `1.1.1.1.1..4.23.23.4.234.24` signature from the report.
/// Reported at this length, not failed: a table or a long figure can reach it.
const BABBLE_REPORT_RUN: usize = 40;
/// Where it cannot be a legitimate figure any more. Eighty characters with no
/// letter in them is not an answer to a question about a person's name.
const MAX_BABBLE_RUN: usize = 80;
/// Share of the reply's characters that are not alphabetic. English prose with
/// spaces, punctuation and the odd number lands near 0.22; markdown tables and
/// code push it to the high 0.30s. Past this it is punctuation soup — the
/// reported babble measured 0.71.
const MAX_NONALPHA: f64 = 0.45;
/// A reply shorter than this cannot be scored honestly — there is not enough
/// text for a repeat rate to mean anything — so it is reported as its own
/// failure rather than passing by being too small to fail.
const MIN_REPLY_CHARS: usize = 24;

// ---------------------------------------------------------------------------
// Box facts and budgets
// ---------------------------------------------------------------------------

/// The canary, planted at the very start of the history and asked for at the
/// end. Distinctive on purpose: a model that has lost its context cannot guess
/// it, and a model that still holds it cannot answer with anything else.
const CANARY_NAME: &str = "Alder Vance";
const CANARY_FACT: &str = "Before anything else, record this and keep it: the ferryman's name is \
     Alder Vance, he keeps his tally in a green ledger, and he only crosses at slack water.";
/// The probe. Fixed across rungs and arms — a scored comparison between rungs
/// is only a comparison if the question is the same one.
const PROBE: &str = "Stop reading the log for a moment and answer from memory. What is the \
     ferryman's name? Give the name first, then two or three sentences about who he is and what \
     he keeps. Plain prose, no lists.";

/// The body size we BELIEVE this service refuses above, used only to decide
/// whether an empty-bodied transport failure was a refusal or a network fault.
/// It is never used to skip a rung on its own: the real cap is learned by
/// observing an actual refusal (see [`BodyCap`]), because a hardcoded guess
/// that drifts from the deployed service turns a measurement into a rumour.
const ASSUMED_BODY_CAP_BYTES: usize = 64 * 1024;

/// Tokens kept clear of the box's advertised `context_per_slot`, on top of the
/// reply budget: the service reserves its own decode headroom (512) and the
/// probe turn plus the rendered ChatML scaffolding needs room after that.
/// A rung that lands inside this band would compact rather than degenerate,
/// which is a different finding and must not be reported as this one.
const CEILING_HEADROOM_BASE: u64 = 1024;

/// Turns a `grown` arm aims to take on its way to the rung. Enough that resume,
/// carry and (at deep rungs) compaction all get exercised more than once;
/// few enough that a 100k rung does not take an afternoon.
const GROWN_TURNS_TARGET: usize = 8;
/// Hard stop, so a rung that refuses to converge ends as a reported cap rather
/// than an unbounded run.
const GROWN_TURNS_MAX: usize = 40;
/// Reply budget for a growth turn. The growth turns exist to move the history
/// forward, not to be read, so they are kept short — the depth comes from the
/// chunks the client sends, which is the part this tool controls exactly.
const GROWTH_REPLY_TOKENS: u32 = 200;

/// Poll interval on `/job/{id}`.
const POLL: Duration = Duration::from_millis(250);
/// Socket read budget for a single HTTP exchange. `POST /generate` returns as
/// soon as the job is queued and `GET /job` is instant, so this only has to
/// cover a slow box, not a whole prefill.
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
/// Socket write budget. A 400 KB body over a fleet LAN is nothing; a stall
/// here is the server having shut the socket down on us.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// Wall budget for one turn, from its prompt depth.
///
/// A measured cold 34k-token prefill takes ~75 s on the live box (~2.2 s per
/// 1000 tokens), and prefill is the dominant term at every rung this tool
/// visits. The allowance below is roughly 3x that, plus a decode allowance, plus
/// a fixed floor for queueing behind another lane — generous on purpose,
/// because a timeout reported as a degeneration would be a false accusation.
fn turn_budget(prompt_tokens: u64, reply_tokens: u32) -> Duration {
    let prefill = Duration::from_secs_f64(prompt_tokens as f64 / 1000.0 * 6.0);
    let decode = Duration::from_secs_f64(reply_tokens as f64 / 4.0);
    Duration::from_secs(90) + prefill + decode
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(base) = args.next() else { usage() };
    if base.starts_with("--") {
        usage()
    }
    let base = base.trim_end_matches('/').to_string();

    let mut rungs: Vec<u64> = vec![4000, 8000, 16000, 32000, 64000, 100000];
    let mut model = "qwen3.8-27b".to_string();
    let mut reply_tokens: u32 = 400;
    let mut arms: Vec<Arm> = vec![Arm::Fresh, Arm::Grown];
    let mut session_prefix = "ladder".to_string();
    let mut seed: u64 = 20260821;

    let rest: Vec<String> = args.collect();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--rungs" => {
                let Some(list) = rest.get(index + 1) else { usage() };
                rungs = list
                    .split(',')
                    .filter_map(|part| part.trim().parse::<u64>().ok())
                    .collect();
                if rungs.is_empty() {
                    eprintln!("--rungs needs a comma separated list of token counts");
                    std::process::exit(2);
                }
                index += 2;
            }
            "--model" => {
                model = rest.get(index + 1).cloned().unwrap_or(model);
                index += 2;
            }
            "--reply-tokens" => {
                reply_tokens = rest
                    .get(index + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(reply_tokens);
                index += 2;
            }
            "--arms" => {
                let Some(list) = rest.get(index + 1) else { usage() };
                arms = Vec::new();
                for part in list.split(',') {
                    match part.trim() {
                        "fresh" => arms.push(Arm::Fresh),
                        "grown" => arms.push(Arm::Grown),
                        other => {
                            eprintln!("unknown arm {other} (expected fresh or grown)");
                            std::process::exit(2);
                        }
                    }
                }
                if arms.is_empty() {
                    usage()
                }
                index += 2;
            }
            "--session-prefix" => {
                session_prefix = rest.get(index + 1).cloned().unwrap_or(session_prefix);
                index += 2;
            }
            "--seed" => {
                seed = rest.get(index + 1).and_then(|v| v.parse().ok()).unwrap_or(seed);
                index += 2;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    rungs.sort_unstable();
    rungs.dedup();

    // A nonce nobody has used before, so `fresh` is cold by construction: the
    // box's prefix test is literal text, and a session marker reused from an
    // earlier run of this same tool would resume that run's lane and quietly
    // turn the cold arm into a warm one.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    println!(
        "context-ladder {base}\n  model={model}  reply-tokens={reply_tokens}  \
         arms={}  session-prefix={session_prefix}  seed={seed}  run={nonce}\n  rungs={}",
        arms.iter().map(Arm::name).collect::<Vec<_>>().join(","),
        rungs.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
    );

    // The box's advertised ceiling, read BEFORE anything is sent. A rung above
    // `context_per_slot` does not degenerate, it compacts or overflows — a
    // different finding, and reporting it as this one would send whoever reads
    // the output hunting the wrong subsystem.
    let ceiling = read_ceiling(&base);
    match &ceiling {
        Ok(Some(lanes)) => println!(
            "  box lanes: model={} slots_total={} context_per_slot={} — rungs above \
             {} are skipped, not failed",
            lanes.model,
            lanes.slots_total,
            lanes.context_per_slot,
            lanes
                .context_per_slot
                .saturating_sub(CEILING_HEADROOM_BASE + reply_tokens as u64),
        ),
        Ok(None) => println!(
            "  box advertises no lanes block — no ceiling to respect, every rung is attempted"
        ),
        Err(e) => {
            eprintln!("FAIL: cannot read {base}/health: {e}");
            eprintln!("      the ladder was not run at all");
            std::process::exit(3);
        }
    }
    let usable_ceiling = ceiling.ok().flatten().map(|lanes| {
        lanes
            .context_per_slot
            .saturating_sub(CEILING_HEADROOM_BASE + reply_tokens as u64)
    });

    println!();
    let mut cap = BodyCap::default();
    let mut outcomes: Vec<ArmOutcome> = Vec::new();
    let mut skipped: Vec<u64> = Vec::new();

    for rung in &rungs {
        let rung = *rung;
        if let Some(limit) = usable_ceiling {
            if rung > limit {
                println!(
                    "rung {rung:>7}  SKIPPED — over the box's advertised ceiling ({limit} usable \
                     tokens per slot). A rung above the ceiling reports session context \
                     overflow or compacts; that is a different finding, not a degeneration."
                );
                skipped.push(rung);
                continue;
            }
        }
        for arm in &arms {
            let arm = *arm;
            println!("rung {rung:>7}  arm={:<5} running...", arm.name());
            let outcome = run_arm(
                &base,
                &model,
                arm,
                rung,
                reply_tokens,
                seed,
                &session_prefix,
                nonce,
                usable_ceiling,
                &mut cap,
            );
            print_row(&outcome);
            outcomes.push(outcome);
        }
    }

    println!();
    println!("{}", cap.report());
    std::process::exit(summarise(&outcomes, &skipped));
}

fn usage() -> ! {
    eprintln!(
        "usage: context-ladder <http://box:8123> [--rungs 4000,8000,16000,32000,64000,100000] \
         [--model ID] [--reply-tokens 400] [--arms fresh,grown] [--session-prefix NAME] \
         [--seed N]\n\
         \n\
         exit 0 = every rung that could be run was clean\n\
         exit 1 = a rung degenerated (the line names the first rung and arm)\n\
         exit 2 = bad arguments\n\
         exit 3 = the ladder could not be run (box unreachable, or a turn errored)"
    );
    std::process::exit(2)
}

// ---------------------------------------------------------------------------
// Arms
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arm {
    /// One cold request carrying the whole history. Pure decode at depth.
    Fresh,
    /// The same content grown turn by turn on one session. Resume, carry,
    /// compaction.
    Grown,
}

impl Arm {
    fn name(&self) -> &'static str {
        match self {
            Arm::Fresh => "fresh",
            Arm::Grown => "grown",
        }
    }
}

/// What one (rung, arm) produced.
struct ArmOutcome {
    arm: Arm,
    rung: u64,
    /// Estimated depth the arm actually reached. Below the rung when a body cap
    /// stopped the growth short, and the row says so rather than pretending.
    reached_tokens: u64,
    /// Requests the arm made, probe included.
    turns: usize,
    /// The probe turn's facts, when the probe ran.
    facts: Option<TurnFacts>,
    /// The probe reply's score, when there was a reply to score.
    score: Option<Score>,
    /// Why this arm produced no score, in the operator's words.
    note: Option<String>,
    /// Set when the box or the transport failed — a different exit code from a
    /// degeneration, because "the model broke" and "the box was down" send you
    /// to different places.
    error: Option<String>,
}

impl ArmOutcome {
    fn degenerate(&self) -> bool {
        self.score.as_ref().is_some_and(Score::degenerate)
    }
    fn clean(&self) -> bool {
        self.score.as_ref().is_some_and(|s| !s.degenerate())
    }
}

#[allow(clippy::too_many_arguments)]
fn run_arm(
    base: &str,
    model: &str,
    arm: Arm,
    rung: u64,
    reply_tokens: u32,
    seed: u64,
    session_prefix: &str,
    nonce: u64,
    usable_ceiling: Option<u64>,
    cap: &mut BodyCap,
) -> ArmOutcome {
    // Both arms draw their history from the SAME seeded stream, so "the same
    // conversation content" is literally true: chunk k of `fresh` is chunk k of
    // `grown`, byte for byte.
    let mut prose = Prose::new(seed ^ rung);
    let session = format!("{session_prefix}-{}-{rung}-{nonce}", arm.name());
    let system = system_prompt(&session);
    // Aim to reach the rung in a fixed number of turns, so every rung
    // exercises resume and compaction the same number of times and the rungs
    // stay comparable to each other. The floor keeps small rungs from becoming
    // one-turn conversations, which would not exercise the incremental path at
    // all.
    let chunk_bytes = ((rung as usize * 4) / GROWN_TURNS_TARGET).max(600);

    match arm {
        Arm::Fresh => run_fresh(
            base,
            model,
            rung,
            reply_tokens,
            &system,
            &mut prose,
            chunk_bytes,
            usable_ceiling,
            cap,
        ),
        Arm::Grown => run_grown(
            base,
            model,
            rung,
            reply_tokens,
            &system,
            &mut prose,
            chunk_bytes,
            usable_ceiling,
            cap,
        ),
    }
}

/// ONE cold request carrying ~N tokens. No resume, no carry, no compaction.
#[allow(clippy::too_many_arguments)]
fn run_fresh(
    base: &str,
    model: &str,
    rung: u64,
    reply_tokens: u32,
    system: &str,
    prose: &mut Prose,
    chunk_bytes: usize,
    usable_ceiling: Option<u64>,
    cap: &mut BodyCap,
) -> ArmOutcome {
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut first = true;
    while est_tokens(transcript_bytes(system, &messages)) < rung {
        let chunk = prose.chunk(chunk_bytes, first);
        first = false;
        messages.push(("user".to_string(), chunk));
        // The scripted acknowledgement carries the shape a real client stores —
        // reasoning stripped, closing tag restored — so the two arms' bodies
        // differ in HOW the assistant turns got there, not in what they look
        // like on the wire. Anything else would make the comparison unfair.
        messages.push(("assistant".to_string(), stored_reply(&prose.acknowledgement())));
        if messages.len() > GROWN_TURNS_MAX * 2 {
            break;
        }
    }
    let reached = est_tokens(transcript_bytes(system, &messages));
    messages.push(("user".to_string(), PROBE.to_string()));

    let body = request_json(model, system, &messages, reply_tokens);
    if cap.would_refuse(body.len()) {
        return ArmOutcome {
            arm: Arm::Fresh,
            rung,
            reached_tokens: reached,
            turns: 0,
            facts: None,
            score: None,
            note: Some(format!(
                "not sendable (body cap): one body would be {}, and {} was already refused",
                human_bytes(body.len()),
                human_bytes(cap.smallest_refused.unwrap_or(0))
            )),
            error: None,
        };
    }

    match run_turn(base, &body, reply_tokens, est_tokens(body.len())) {
        Ok(facts) => {
            cap.saw_accepted(body.len());
            let compacted = compaction_likely(&facts, usable_ceiling, reached);
            let score = score_reply(&facts.text, compacted);
            ArmOutcome {
                arm: Arm::Fresh,
                rung,
                reached_tokens: reached,
                turns: 1,
                facts: Some(facts),
                score: Some(score),
                note: None,
                error: None,
            }
        }
        Err(Refusal::BodyCap(bytes)) => {
            cap.saw_refused(bytes);
            ArmOutcome {
                arm: Arm::Fresh,
                rung,
                reached_tokens: reached,
                turns: 0,
                facts: None,
                score: None,
                note: Some(format!(
                    "not sendable (body cap): the service refused a {} body",
                    human_bytes(bytes)
                )),
                error: None,
            }
        }
        Err(Refusal::Box(why)) => ArmOutcome {
            arm: Arm::Fresh,
            rung,
            reached_tokens: reached,
            turns: 1,
            facts: None,
            score: None,
            note: None,
            error: Some(why),
        },
    }
}

/// The same content, grown turn by turn on ONE session, each reply appended the
/// way a client stores it. Resume, carry state, compaction — everything the
/// cold arm skips.
#[allow(clippy::too_many_arguments)]
fn run_grown(
    base: &str,
    model: &str,
    rung: u64,
    reply_tokens: u32,
    system: &str,
    prose: &mut Prose,
    chunk_bytes: usize,
    usable_ceiling: Option<u64>,
    cap: &mut BodyCap,
) -> ArmOutcome {
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut turns = 0usize;
    let mut note: Option<String> = None;
    let mut first = true;

    while est_tokens(transcript_bytes(system, &messages)) < rung && turns < GROWN_TURNS_MAX {
        let chunk = prose.chunk(chunk_bytes, first);
        first = false;
        let mut candidate = messages.clone();
        candidate.push(("user".to_string(), chunk.clone()));
        let body = request_json(model, system, &candidate, GROWTH_REPLY_TOKENS);
        if cap.would_refuse(body.len()) {
            note = Some(format!(
                "capped at ~{} tok: the next body would be {}, over the observed refusal at {}",
                est_tokens(transcript_bytes(system, &messages)),
                human_bytes(body.len()),
                human_bytes(cap.smallest_refused.unwrap_or(0))
            ));
            break;
        }
        turns += 1;
        match run_turn(base, &body, GROWTH_REPLY_TOKENS, est_tokens(body.len())) {
            Ok(facts) => {
                cap.saw_accepted(body.len());
                messages.push(("user".to_string(), chunk));
                messages.push(("assistant".to_string(), stored_reply(&facts.text)));
                println!(
                    "    grown turn {turns}: ~{} tok, ingested {} {}, {}",
                    est_tokens(transcript_bytes(system, &messages)),
                    facts
                        .prefix_ingested
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into()),
                    warmth_word(&facts),
                    facts.stage,
                );
            }
            Err(Refusal::BodyCap(bytes)) => {
                cap.saw_refused(bytes);
                turns -= 1;
                note = Some(format!(
                    "capped at ~{} tok: the service refused a {} body",
                    est_tokens(transcript_bytes(system, &messages)),
                    human_bytes(bytes)
                ));
                break;
            }
            Err(Refusal::Box(why)) => {
                return ArmOutcome {
                    arm: Arm::Grown,
                    rung,
                    reached_tokens: est_tokens(transcript_bytes(system, &messages)),
                    turns,
                    facts: None,
                    score: None,
                    note,
                    error: Some(format!("growth turn {turns}: {why}")),
                }
            }
        }
    }

    let reached = est_tokens(transcript_bytes(system, &messages));
    if messages.is_empty() {
        return ArmOutcome {
            arm: Arm::Grown,
            rung,
            reached_tokens: 0,
            turns,
            facts: None,
            score: None,
            note: note.or_else(|| Some("no turn was sendable at all".to_string())),
            error: None,
        };
    }

    messages.push(("user".to_string(), PROBE.to_string()));
    let body = request_json(model, system, &messages, reply_tokens);
    if cap.would_refuse(body.len()) {
        return ArmOutcome {
            arm: Arm::Grown,
            rung,
            reached_tokens: reached,
            turns,
            facts: None,
            score: None,
            note: Some(format!(
                "reached ~{reached} tok but the probe body ({}) is over the observed cap",
                human_bytes(body.len())
            )),
            error: None,
        };
    }
    turns += 1;
    match run_turn(base, &body, reply_tokens, est_tokens(body.len())) {
        Ok(facts) => {
            cap.saw_accepted(body.len());
            let compacted = compaction_likely(&facts, usable_ceiling, reached);
            let score = score_reply(&facts.text, compacted);
            ArmOutcome {
                arm: Arm::Grown,
                rung,
                reached_tokens: reached,
                turns,
                facts: Some(facts),
                score: Some(score),
                note,
                error: None,
            }
        }
        Err(Refusal::BodyCap(bytes)) => {
            cap.saw_refused(bytes);
            ArmOutcome {
                arm: Arm::Grown,
                rung,
                reached_tokens: reached,
                turns,
                facts: None,
                score: None,
                note: Some(format!(
                    "probe not sendable (body cap): the service refused a {} body",
                    human_bytes(bytes)
                )),
                error: None,
            }
        }
        Err(Refusal::Box(why)) => ArmOutcome {
            arm: Arm::Grown,
            rung,
            reached_tokens: reached,
            turns,
            facts: None,
            score: None,
            note,
            error: Some(format!("probe: {why}")),
        },
    }
}

/// Did this turn's history get compacted on the way in?
///
/// The lane path drops oldest turns silently (it only eprintlns on the box), so
/// there are two signals and both are used: the single-session path puts
/// "(context full)" in its stage string, and any prompt whose depth exceeds the
/// usable ceiling MUST have been compacted to fit. When it was, a lost canary is
/// the expected outcome of a working compactor, not amnesia — and scoring it as
/// a failure would accuse the wrong subsystem.
fn compaction_likely(facts: &TurnFacts, usable_ceiling: Option<u64>, depth: u64) -> bool {
    if facts.saw_context_full {
        return true;
    }
    usable_ceiling.is_some_and(|limit| depth > limit)
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Canary {
    /// The full name came back.
    Hit,
    /// Half the name came back — enough to say the context is there but frayed.
    Partial,
    Miss,
}

impl Canary {
    fn word(&self) -> &'static str {
        match self {
            Canary::Hit => "HIT",
            Canary::Partial => "PART",
            Canary::Miss => "MISS",
        }
    }
}

struct Score {
    /// Which half of the reply was graded. Qwen3.8 runs an open think block, so
    /// a tight token budget can produce reasoning and no visible text at all —
    /// and babble shows up in reasoning first. Grade what exists, and SAY which.
    scored_from: &'static str,
    chars: usize,
    words: usize,
    best_run: usize,
    best_n: usize,
    coverage: f64,
    babble_run: usize,
    nonalpha: f64,
    canary: Canary,
    compacted: bool,
    /// Empty means clean. Each entry is one tripped threshold, in the form the
    /// FAIL line prints.
    reasons: Vec<String>,
}

impl Score {
    fn degenerate(&self) -> bool {
        !self.reasons.is_empty()
    }
    /// The compact form the verdict line carries: only the metrics that tripped.
    fn brief(&self) -> String {
        self.reasons.join(", ")
    }
}

fn score_reply(text: &str, compacted: bool) -> Score {
    let (think, visible) = split_reply(text);
    let (graded, scored_from) = if visible.trim().len() >= MIN_REPLY_CHARS {
        (visible.trim(), "visible")
    } else if think.trim().len() >= MIN_REPLY_CHARS {
        // No readable answer, but there IS reasoning. Grade it and label it:
        // a think block that has turned into digit soup is the same bug, seen
        // one stage earlier.
        (think.trim(), "think-only")
    } else {
        (visible.trim(), "visible")
    };

    let words: Vec<String> = graded
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let (best_run, best_n, coverage) = scan_repeats(&words);
    let babble_run = longest_babble_run(graded);
    let nonalpha = nonalpha_share(graded);
    let canary = canary_state(graded);

    let mut reasons = Vec::new();
    if graded.chars().count() < MIN_REPLY_CHARS {
        reasons.push(format!("no reply ({} chars)", graded.chars().count()));
    }
    if best_run >= MAX_NGRAM_RUN {
        reasons.push(format!("ngram-repeat x{best_run}"));
    }
    if coverage > MAX_REPEAT_COVERAGE {
        reasons.push(format!("repeat-coverage {coverage:.2}"));
    }
    if babble_run >= MAX_BABBLE_RUN {
        reasons.push(format!("numeric-babble {babble_run} chars"));
    }
    if nonalpha > MAX_NONALPHA {
        reasons.push(format!("nonalpha {nonalpha:.2}"));
    }
    // Amnesia is a REAL failure — a conversation that cannot recall its own
    // opening line has lost its context — but only when the box was not
    // legitimately asked to drop it. Compaction dropping the oldest turn is the
    // compactor working, and this tool must not report it as this bug.
    if canary == Canary::Miss && !compacted {
        reasons.push("canary lost".to_string());
    }

    Score {
        scored_from,
        chars: graded.chars().count(),
        words: words.len(),
        best_run,
        best_n,
        coverage,
        babble_run: if babble_run >= BABBLE_REPORT_RUN { babble_run } else { 0 },
        nonalpha,
        canary,
        compacted,
        reasons,
    }
}

/// Longest immediately-repeated n-gram run, and the share of the reply inside
/// SOME repeat.
///
/// "Immediately repeated" is the whole point: `A B A B A B` is a decoder in a
/// loop, while `A B ... A B` fifty words apart is a person making the same
/// point twice. Only adjacency is counted, at every width in [`NGRAM_SIZES`],
/// and the coverage marks are the union across widths — so a reply that loops
/// at two scales is not scored as if it looped at one.
fn scan_repeats(words: &[String]) -> (usize, usize, f64) {
    if words.is_empty() {
        return (1, 0, 0.0);
    }
    let mut marked = vec![false; words.len()];
    let mut best_run = 1usize;
    let mut best_n = 0usize;
    for &n in NGRAM_SIZES.iter() {
        if words.len() < 2 * n {
            continue;
        }
        let mut i = 0usize;
        while i + 2 * n <= words.len() {
            let mut reps = 1usize;
            let mut j = i;
            while j + 2 * n <= words.len() && words[j..j + n] == words[j + n..j + 2 * n] {
                reps += 1;
                j += n;
            }
            if reps > 1 {
                for mark in marked.iter_mut().take(j + n).skip(i) {
                    *mark = true;
                }
                if reps > best_run || (reps == best_run && n > best_n) {
                    best_run = reps;
                    best_n = n;
                }
                i = j + n;
            } else {
                i += 1;
            }
        }
    }
    let covered = marked.iter().filter(|m| **m).count();
    (best_run, best_n, covered as f64 / words.len() as f64)
}

/// Longest run of characters drawn only from digits, dot, comma, pipe and
/// whitespace — the shape of `1.1.1.1.1..4.23.23.4.234.24`.
///
/// Deliberately character-level and not word-level: that string is ONE
/// whitespace token, so no n-gram measure sees it at all. The two measures
/// catch different failures and neither one subsumes the other.
fn longest_babble_run(text: &str) -> usize {
    let mut best = 0usize;
    let mut run = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == ',' || ch == '|' || ch.is_whitespace() {
            run += 1;
            if run > best {
                best = run;
            }
        } else {
            run = 0;
        }
    }
    best
}

fn nonalpha_share(text: &str) -> f64 {
    let total = text.chars().count();
    if total == 0 {
        return 1.0;
    }
    let alpha = text.chars().filter(|c| c.is_alphabetic()).count();
    (total - alpha) as f64 / total as f64
}

fn canary_state(text: &str) -> Canary {
    let lower = text.to_ascii_lowercase();
    if lower.contains(&CANARY_NAME.to_ascii_lowercase()) {
        Canary::Hit
    } else if lower.contains("vance") || lower.contains("alder") {
        Canary::Partial
    } else {
        Canary::Miss
    }
}

/// The think block and the readable answer, split the way a chat client does.
fn split_reply(text: &str) -> (&str, &str) {
    if let Some(at) = text.find("</think>") {
        let think = text[..at].trim_start_matches("<think>");
        (think, &text[at + "</think>".len()..])
    } else if let Some(at) = text.find("<think>") {
        (&text[at + "<think>".len()..], "")
    } else {
        ("", text)
    }
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

fn warmth_word(facts: &TurnFacts) -> &'static str {
    match facts.prefix_resumed {
        Some(true) => "WARM",
        Some(false) => "cold",
        None => "warmth-unreported",
    }
}

fn print_row(outcome: &ArmOutcome) {
    let head = format!(
        "rung {:>7}  arm={:<5} ~{:>6} tok in {:>2} turn(s)",
        outcome.rung,
        outcome.arm.name(),
        outcome.reached_tokens,
        outcome.turns,
    );
    let Some(score) = &outcome.score else {
        let why = outcome
            .error
            .clone()
            .or_else(|| outcome.note.clone())
            .unwrap_or_else(|| "no reply".to_string());
        println!("{head}  --  {why}");
        return;
    };
    let facts = outcome.facts.as_ref();
    let serving = match facts {
        Some(f) => format!(
            "ingested {} {}  think {} visible {}  stage {:?}",
            f.prefix_ingested
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into()),
            warmth_word(f),
            f.think_tokens,
            f.visible_tokens,
            f.stage,
        ),
        None => "serving facts unreported".to_string(),
    };
    println!(
        "{head}  {serving}\n           scored {} ({} words / {} chars)  repeat x{}{}  cov {:.2}  \
         babble {}  nonalpha {:.2}  canary {}{}  ->  {}",
        score.scored_from,
        score.words,
        score.chars,
        score.best_run,
        if score.best_n > 0 { format!("(n={})", score.best_n) } else { String::new() },
        score.coverage,
        score.babble_run,
        score.nonalpha,
        score.canary.word(),
        if score.compacted { " (compacted)" } else { "" },
        if score.degenerate() {
            format!("DEGENERATE [{}]", score.brief())
        } else {
            "clean".to_string()
        },
    );
    if let Some(note) = &outcome.note {
        println!("           note: {note}");
    }
}

/// The verdict, the discriminator, and the exit code.
fn summarise(outcomes: &[ArmOutcome], skipped: &[u64]) -> i32 {
    if !skipped.is_empty() {
        println!(
            "SKIPPED (over the box's advertised ceiling, not failures): {}",
            skipped.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
        );
    }

    // The discriminator: at every rung both arms actually scored, do they
    // agree? Divergence names the subsystem; agreement names a different one.
    let mut rungs: Vec<u64> = outcomes.iter().map(|o| o.rung).collect();
    rungs.sort_unstable();
    rungs.dedup();
    let mut divergence: Option<u64> = None;
    let mut both_bad: Option<u64> = None;
    let mut compared = 0usize;
    for rung in &rungs {
        let fresh = outcomes.iter().find(|o| o.rung == *rung && o.arm == Arm::Fresh);
        let grown = outcomes.iter().find(|o| o.rung == *rung && o.arm == Arm::Grown);
        let (Some(fresh), Some(grown)) = (fresh, grown) else { continue };
        if fresh.score.is_none() || grown.score.is_none() {
            continue;
        }
        compared += 1;
        if grown.degenerate() && fresh.clean() && divergence.is_none() {
            divergence = Some(*rung);
        }
        if fresh.degenerate() && grown.degenerate() && both_bad.is_none() {
            both_bad = Some(*rung);
        }
    }
    let discriminator = if compared == 0 {
        "DISCRIMINATOR: not established — no rung had both arms produce a scored reply, so \
         nothing here separates the incremental path from depth itself."
            .to_string()
    } else if let Some(rung) = divergence {
        format!(
            "DISCRIMINATOR: the arms DIVERGE at rung {rung} — grown degenerated where fresh at \
             the same length was clean. Depth alone is fine; the incremental path (resume, \
             carry state, prefix-cache append, compaction splicing) is the suspect."
        )
    } else if let Some(rung) = both_bad {
        format!(
            "DISCRIMINATOR: the arms AGREE at rung {rung} — a single cold request at that depth \
             degenerated too, so this is not the incremental path. Look at the arena, the \
             position math and the attention window."
        )
    } else {
        format!(
            "DISCRIMINATOR: no divergence — fresh and grown agreed at all {compared} rung(s) \
             where both scored, and both stayed clean."
        )
    };

    // First failing (rung, arm), in ladder order.
    let first_bad = outcomes.iter().find(|o| o.degenerate());
    if let Some(bad) = first_bad {
        let score = bad.score.as_ref().expect("degenerate implies a score");
        let other = outcomes
            .iter()
            .find(|o| o.rung == bad.rung && o.arm != bad.arm);
        let tail = match other {
            Some(other) if other.clean() && bad.arm == Arm::Grown => {
                " while arm=fresh at the same length was clean — the incremental path is the \
                 suspect"
                    .to_string()
            }
            Some(other) if other.clean() && bad.arm == Arm::Fresh => {
                " while arm=grown at the same length was clean — a cold request degenerating \
                 where a grown one does not points at the one-shot prefill path"
                    .to_string()
            }
            Some(other) if other.degenerate() => {
                format!(" and arm={} degenerated at the same rung too", other.arm.name())
            }
            _ => String::new(),
        };
        println!("{discriminator}");
        eprintln!(
            "FAIL: rung {} arm={} degenerated ({}){tail}",
            bad.rung,
            bad.arm.name(),
            score.brief(),
        );
        return 1;
    }

    if let Some(broken) = outcomes.iter().find(|o| o.error.is_some()) {
        println!("{discriminator}");
        eprintln!(
            "FAIL: rung {} arm={} could not be run: {}",
            broken.rung,
            broken.arm.name(),
            broken.error.clone().unwrap_or_default(),
        );
        eprintln!("      that is a box or transport fault, not a degeneration");
        return 3;
    }

    println!("{discriminator}");
    let scored = outcomes.iter().filter(|o| o.score.is_some()).count();
    println!(
        "PASS: {scored} of {} (rung, arm) runs produced a scored reply and none degenerated",
        outcomes.len()
    );
    0
}

// ---------------------------------------------------------------------------
// The synthetic history
// ---------------------------------------------------------------------------

/// A deterministic prose source.
///
/// **The filler must NOT repeat itself.** This tool measures repetition in the
/// model's output; a history built by pasting the same sentence a thousand
/// times teaches the model to repeat and would score its own filler as the
/// bug. Every sentence here is drawn from a large enough product of pools
/// (subjects x verbs x objects x places x adjectives x templates, well over
/// 10^7 distinct sentences) that a 100k-token history never says the same thing
/// twice, while an LCG keeps it byte-identical between runs and between arms —
/// which is what makes `fresh` and `grown` a fair comparison at all.
struct Prose {
    rng: Lcg,
    entry: u32,
}

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        // Any non-degenerate seed works; the odd constant keeps a zero seed
        // from producing a zero stream.
        Lcg(seed ^ 0x9e3779b97f4a7c15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // High bits: the low bits of an LCG have short periods.
        self.0 >> 17
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a>(&mut self, pool: &'a [&'a str]) -> &'a str {
        pool[self.below(pool.len())]
    }
}

const SUBJECTS: [&str; 28] = [
    "harbourmaster", "cooper", "signal keeper", "grain factor", "tide clerk", "map binder",
    "rope walker", "sail mender", "kiln warden", "night porter", "well digger", "fen guide",
    "bell founder", "ice cutter", "salt reeve", "orchard warden", "chart cutter", "lamp trimmer",
    "quarry master", "ferry hand", "weir keeper", "drover", "glass blower", "toll wife",
    "marsh reeve", "wheelwright", "pilot boat crew", "ledger clerk",
];

const VERBS: [&str; 28] = [
    "carried", "counted", "hid", "mended", "refused", "traded", "buried", "measured", "loaded",
    "returned", "borrowed", "mislaid", "repainted", "weighed", "argued over", "signed for",
    "abandoned", "recovered", "sold on", "hauled", "sheltered", "copied out", "burned",
    "rewrote", "lent", "guarded", "swapped", "catalogued",
];

const OBJECTS: [&str; 28] = [
    "brass tally", "grain sack", "chart roll", "sounding line", "cracked bell", "oil lamp",
    "iron key", "rope coil", "cargo book", "lead weight", "wax seal", "spare rudder",
    "salt barrel", "tide table", "signal flag", "hand cart", "milled plank", "copper kettle",
    "spool of twine", "lantern glass", "worn saddle", "bundle of reeds", "dyed cloth",
    "flint box", "sheaf of receipts", "cracked millstone", "coil of chain", "boxed compass",
];

const PLACES: [&str; 24] = [
    "the low crossing", "Sedge Reach", "the north quay", "Bellwater", "the toll house",
    "Cold Harbour", "the drying sheds", "Mirefield", "the pilot steps", "Gullet Bend",
    "the flood meadow", "Thornwick", "the old weighbridge", "Ashen Lock", "the boat yard",
    "Harrow Point", "the cut above the weir", "Saltmere", "the winter store", "Corn Landing",
    "the ropewalk", "Fennel Row", "the outer mole", "Kestrel Stair",
];

const ADJECTIVES: [&str; 24] = [
    "patient", "hurried", "unlucky", "stubborn", "borrowed", "half-mended", "second", "quiet",
    "damp", "unpaid", "early", "distant", "narrow", "rusted", "cheerful", "unmarked", "spare",
    "crooked", "watchful", "shortened", "well-kept", "forgotten", "expensive", "younger",
];

const CONNECTIVES: [&str; 16] = [
    "That same week,", "By the following tide,", "Some time after,", "Against the usual order,",
    "For reasons nobody wrote down,", "Late in the season,", "Once the water dropped,",
    "Before the accounts closed,", "In the same hand,", "Well after dark,",
    "Under the old arrangement,", "Between the two crossings,", "On the strength of a promise,",
    "Where the path narrows,", "Just before the frost,", "With the ledger still open,",
];

impl Prose {
    fn new(seed: u64) -> Self {
        Prose { rng: Lcg::new(seed), entry: 1 }
    }

    fn sentence(&mut self) -> String {
        let subject = self.rng.pick(&SUBJECTS).to_string();
        let verb = self.rng.pick(&VERBS).to_string();
        let object = self.rng.pick(&OBJECTS).to_string();
        let place = self.rng.pick(&PLACES).to_string();
        let adjective = self.rng.pick(&ADJECTIVES).to_string();
        let connective = self.rng.pick(&CONNECTIVES).to_string();
        match self.rng.below(6) {
            0 => format!("The {adjective} {subject} {verb} the {object} near {place}."),
            1 => format!("{connective} the {subject} {verb} a {adjective} {object} at {place}."),
            2 => format!(
                "At {place} a {subject} {verb} the {object}, and the {adjective} one stayed behind."
            ),
            3 => format!(
                "Nobody at {place} now remembers whether the {subject} {verb} the {adjective} \
                 {object}."
            ),
            4 => format!(
                "{connective} a {adjective} {object} was left at {place} for the {subject} who \
                 {verb} it."
            ),
            _ => format!(
                "The {subject} of {place} {verb} the {object}, which the {adjective} log records \
                 without further comment."
            ),
        }
    }

    /// One user turn's worth of history, roughly `target_bytes` long.
    ///
    /// The FIRST chunk carries the canary in its opening line: the fact has to
    /// be as far from the probe as the rung is deep, or recalling it says
    /// nothing about how much context survived.
    fn chunk(&mut self, target_bytes: usize, first: bool) -> String {
        let mut out = String::with_capacity(target_bytes + 256);
        if first {
            out.push_str(CANARY_FACT);
            out.push_str("\n\n");
        }
        out.push_str(&format!("Log entry {}. ", self.entry));
        self.entry += 1;
        while out.len() < target_bytes {
            out.push_str(&self.sentence());
            out.push(' ');
        }
        out.push_str(
            "\nAcknowledge this entry in one short sentence and name one detail from it.",
        );
        out
    }

    /// The scripted assistant turn the `fresh` arm uses in place of a real
    /// reply, so both arms' transcripts have the same shape and the same
    /// length at the same rung.
    fn acknowledgement(&mut self) -> String {
        let object = self.rng.pick(&OBJECTS).to_string();
        let place = self.rng.pick(&PLACES).to_string();
        format!("Noted. The entry mentions the {object} at {place}; I have it.")
    }
}

/// The system prompt, carrying the session marker.
///
/// The marker is what makes a session a session on a wire that has no session
/// field, so it must be present and it must be unique. It sits at the end
/// because the box's reuse test is a literal text prefix: two prompts that
/// differ anywhere before the first message can never extend each other, which
/// is exactly the isolation the `fresh` arm needs.
fn system_prompt(session: &str) -> String {
    format!(
        "You are reading a long river-trade log and answering questions about it. Answer from \
         what the conversation contains. Reasoning effort is set to low. Keep your thinking \
         brief and focused, moving directly to the conclusion without unnecessary elaboration. \
         Session marker: {session}"
    )
}

/// What a real client stores after a turn: reasoning removed, the closing tag
/// put back, because the service opens a think block on every assistant turn it
/// renders (`libs/asset/ai/src/protocol.rs`, `history_think_prefill`). This is
/// the shape production actually sends, and it is precisely the shape in which
/// the stored history can drift from what the lane's KV holds — so the `grown`
/// arm must use it or it is not testing the reported bug.
fn stored_reply(text: &str) -> String {
    let visible = text.rsplit("</think>").next().unwrap_or(text).trim_start();
    format!("\n</think>\n\n{visible}")
}

/// Bytes of a rendered transcript, near enough for a token estimate.
fn transcript_bytes(system: &str, messages: &[(String, String)]) -> usize {
    // ~24 bytes of ChatML scaffolding per turn (`<|im_start|>role\n` +
    // `<|im_end|>\n`), plus the system block's own wrapper.
    system.len()
        + 40
        + messages
            .iter()
            .map(|(role, text)| text.len() + role.len() + 24)
            .sum::<usize>()
}

/// Four bytes to a token — the same crude estimate `chat_bench` uses, and good
/// enough for choosing a rung. The box's own `prefix_ingested` is reported on
/// every row beside it, so the honest number is always visible next to the
/// approximation.
fn est_tokens(bytes: usize) -> u64 {
    (bytes / 4) as u64
}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

// ---------------------------------------------------------------------------
// The body cap, learned rather than assumed
// ---------------------------------------------------------------------------

/// What this run has OBSERVED about the service's request body limit.
///
/// The limit is a deployment fact, not a constant of the code: it moves with
/// the service version and with anything sitting in front of it. So it is
/// bracketed from evidence — the largest body that was accepted and the
/// smallest that was refused — and only the bracket is ever reported.
/// [`ASSUMED_BODY_CAP_BYTES`] appears exactly once, to decide whether an
/// empty-bodied transport failure on a large body was a refusal or a network
/// fault, and never to skip a rung that has not actually been refused.
#[derive(Default)]
struct BodyCap {
    largest_ok: usize,
    smallest_refused: Option<usize>,
}

impl BodyCap {
    fn saw_accepted(&mut self, bytes: usize) {
        if bytes > self.largest_ok {
            self.largest_ok = bytes;
        }
    }
    fn saw_refused(&mut self, bytes: usize) {
        self.smallest_refused = Some(match self.smallest_refused {
            Some(prev) => prev.min(bytes),
            None => bytes,
        });
    }
    /// Only true once a refusal has actually been seen at or below this size.
    fn would_refuse(&self, bytes: usize) -> bool {
        self.smallest_refused.is_some_and(|refused| bytes >= refused)
    }
    fn report(&self) -> String {
        match self.smallest_refused {
            Some(refused) => format!(
                "body cap: between {} (accepted) and {} (refused)",
                human_bytes(self.largest_ok),
                human_bytes(refused)
            ),
            None if self.largest_ok > 0 => format!(
                "body cap: not reached — the largest body this run sent was {} and it was accepted",
                human_bytes(self.largest_ok)
            ),
            None => "body cap: nothing was sent".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Talking to the box
// ---------------------------------------------------------------------------

struct Lanes {
    model: String,
    slots_total: u64,
    context_per_slot: u64,
}

fn read_ceiling(base: &str) -> Result<Option<Lanes>, String> {
    let health = http(&format!("{base}/health"), None)?;
    if health.status != 200 {
        return Err(format!("HTTP {} from /health", health.status));
    }
    // The lanes block is the only place these keys live, so a whole-body scan
    // cannot pick them up from anywhere else.
    let Some(context_per_slot) = field_u64(&health.body, "context_per_slot") else {
        return Ok(None);
    };
    Ok(Some(Lanes {
        model: field_str(&health.body, "model").unwrap_or_else(|| "?".into()),
        slots_total: field_u64(&health.body, "slots_total").unwrap_or(1),
        context_per_slot,
    }))
}

/// What one turn reported about itself.
struct TurnFacts {
    text: String,
    stage: String,
    /// True when any stage string this turn published said "(context full)" —
    /// the single-session path's compaction signal.
    saw_context_full: bool,
    prefix_ingested: Option<u64>,
    prefix_resumed: Option<bool>,
    think_tokens: u64,
    visible_tokens: u64,
    #[allow(dead_code)]
    wall: Duration,
}

enum Refusal {
    /// The service refused the body itself. Carries the size that was refused.
    BodyCap(usize),
    /// Everything else: a job error, a timeout, a dead box.
    Box(String),
}

fn run_turn(
    base: &str,
    body: &str,
    reply_tokens: u32,
    prompt_tokens: u64,
) -> Result<TurnFacts, Refusal> {
    let started = Instant::now();
    let job = submit(base, body)?;
    let budget = turn_budget(prompt_tokens, reply_tokens);

    // Assigned on every path that can reach the break below; never read before
    // one of them does.
    let mut stage = String::new();
    let mut saw_context_full = false;
    let last = loop {
        if started.elapsed() > budget {
            return Err(Refusal::Box(format!(
                "no answer in {:.0}s at ~{prompt_tokens} prompt tokens (last stage {stage:?})",
                budget.as_secs_f64()
            )));
        }
        std::thread::sleep(POLL);
        let status = match http(&format!("{base}/job/{job}"), None) {
            Ok(reply) => reply.body,
            // A single dropped poll is not a failed turn; the budget above is
            // what ends a turn that is genuinely stuck.
            Err(_) => continue,
        };
        if let Some(now) = field_str(&status, "stage") {
            if now.contains("context full") {
                saw_context_full = true;
            }
            stage = now;
        }
        let state = field_str(&status, "state");
        match state.as_deref() {
            Some("done") => break status,
            Some("error") | Some("failed") => {
                return Err(Refusal::Box(
                    field_str(&status, "error").unwrap_or_else(|| "job failed".into()),
                ))
            }
            Some("cancelled") => return Err(Refusal::Box("job cancelled".into())),
            _ => {}
        }
    };

    let text = field_str(&last, "partial_text").unwrap_or_default();
    Ok(TurnFacts {
        text,
        stage,
        saw_context_full,
        prefix_ingested: field_u64(&last, "prefix_ingested"),
        prefix_resumed: field_bool(&last, "prefix_resumed"),
        think_tokens: field_u64(&last, "think_tokens").unwrap_or(0),
        visible_tokens: field_u64(&last, "visible_tokens").unwrap_or(0),
        wall: started.elapsed(),
    })
}

/// Submit one `/generate` and classify anything that is not a job id.
///
/// A service that refuses an oversized body writes a bare status line and shuts
/// the socket down, so the refusal reaches a client in one of two shapes: a
/// non-2xx response with an EMPTY body, or a write that fails with a broken
/// pipe and nothing to read. Both are recognised here, and only here — the rest
/// of the tool sees `Refusal::BodyCap` and reports the rung honestly instead of
/// calling a refused request a degenerate model.
fn submit(base: &str, body: &str) -> Result<String, Refusal> {
    match http_post(&format!("{base}/generate"), body) {
        Ok(reply) => {
            if let Some(job) = field_str(&reply.body, "job_id") {
                return Ok(job);
            }
            if reply.status >= 400 && reply.body.trim().is_empty() {
                return Err(Refusal::BodyCap(body.len()));
            }
            Err(Refusal::Box(
                field_str(&reply.body, "error").unwrap_or_else(|| {
                    format!("HTTP {} with no job id: {}", reply.status, truncate(&reply.body, 200))
                }),
            ))
        }
        Err(transport) => {
            if body.len() > ASSUMED_BODY_CAP_BYTES {
                Err(Refusal::BodyCap(body.len()))
            } else {
                Err(Refusal::Box(transport))
            }
        }
    }
}

fn request_json(
    model: &str,
    system: &str,
    messages: &[(String, String)],
    max_tokens: u32,
) -> String {
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
    // Sampling is pinned to what the serving path the user complained about
    // actually uses, seed included: this must reproduce their conditions, not
    // a quieter greedy variant that hides a sampling-shaped bug.
    out.push_str(&format!(
        "],\"max_tokens\":{max_tokens},\"temperature\":0.7,\"seed\":7}}"
    ));
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

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

// --- the smallest HTTP that does the job -----------------------------------
//
// Dependency-free for the same reason `chat_bench` is: a gate that needs a
// client library built before it can say the box is broken is a gate nobody
// runs. The one thing it does that a library would hide is keep the STATUS
// LINE, because an empty body under a 500 is how this service says "that
// request was too big" and the difference matters here.

struct Reply {
    status: u16,
    body: String,
}

fn http_post(url: &str, body: &str) -> Result<Reply, String> {
    http(url, Some(body))
}

fn http(url: &str, body: Option<&str>) -> Result<Reply, String> {
    use std::io::{Read, Write};
    let rest = url.strip_prefix("http://").ok_or("only http:// urls")?;
    let (host_port, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let mut stream = std::net::TcpStream::connect(host_port)
        .map_err(|e| format!("connect {host_port}: {e}"))?;
    stream
        .set_read_timeout(Some(HTTP_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let head = match body {
        Some(body) => format!(
            "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ),
        None => format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"),
    };
    // Head and body go separately, so a server that rejects on Content-Length
    // alone breaks the BODY write — which is the signal `submit` reads.
    let write = stream.write_all(head.as_bytes()).and_then(|_| match body {
        Some(body) => stream.write_all(body.as_bytes()),
        None => Ok(()),
    });
    let write_error = write.err().map(|e| format!("write: {e}"));

    let mut raw = Vec::new();
    let read_error = stream.read_to_end(&mut raw).err().map(|e| format!("read: {e}"));
    if raw.is_empty() {
        return Err(write_error
            .or(read_error)
            .unwrap_or_else(|| "empty response".to_string()));
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = match text.find("\r\n\r\n") {
        Some(at) => text[at + 4..].to_string(),
        None => String::new(),
    };
    Ok(Reply { status, body })
}

// --- just enough JSON to read the fields this tool needs ---------------------

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
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
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

// ---------------------------------------------------------------------------
// The scorers score what they claim to score
// ---------------------------------------------------------------------------
//
// These are the tests that matter for a gate: if the measure is wrong, every
// verdict it prints is wrong, and a verdict about someone else's subsystem is
// expensive to be wrong about.

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(|w| w.to_ascii_lowercase()).collect()
    }

    #[test]
    fn ordinary_prose_is_not_a_repeat() {
        let text = "Alder Vance is the ferryman named at the start of the log. He keeps his \
                    tally in a green ledger and crosses only at slack water.";
        let (run, _, coverage) = scan_repeats(&words(text));
        assert!(run < MAX_NGRAM_RUN, "clean prose scored a run of {run}");
        assert!(coverage <= MAX_REPEAT_COVERAGE, "clean prose scored {coverage} coverage");
        assert!(nonalpha_share(text) < MAX_NONALPHA);
        // Prose does contain short digit/punctuation/space runs (". " is one);
        // what matters is that nothing reaches the length worth reporting.
        assert!(
            longest_babble_run(text) < BABBLE_REPORT_RUN,
            "clean prose measured a {}-char babble run",
            longest_babble_run(text)
        );
        assert_eq!(canary_state(text), Canary::Hit);
        assert!(!score_reply(text, false).degenerate(), "a good answer must score clean");
    }

    #[test]
    fn a_looping_decoder_trips_the_ngram_bar() {
        let text = "the ferryman the ferryman the ferryman the ferryman the ferryman \
                    the ferryman the ferryman";
        let (run, n, coverage) = scan_repeats(&words(text));
        assert!(run >= MAX_NGRAM_RUN, "a seven-fold loop scored only x{run} (n={n})");
        assert!(coverage > MAX_REPEAT_COVERAGE);
    }

    #[test]
    fn the_reported_babble_shape_is_caught_by_the_character_measure() {
        // The exact shape from the report, padded to the length a real reply
        // reaches. It is ONE whitespace token, so the n-gram measure cannot see
        // it — which is why both measures exist.
        let babble = "1.1.1.1.1..4.23.23.4.234.24".repeat(6);
        assert!(
            longest_babble_run(&babble) >= MAX_BABBLE_RUN,
            "babble run measured {}",
            longest_babble_run(&babble)
        );
        assert!(nonalpha_share(&babble) > MAX_NONALPHA);
        let score = score_reply(&babble, false);
        assert!(score.degenerate(), "the reported shape scored clean");
    }

    #[test]
    fn a_lost_canary_is_amnesia_unless_the_history_was_compacted() {
        let reply = "The log does not say who the ferryman was, and I have no record of a name \
                     anywhere in what I can still see of this conversation.";
        assert_eq!(canary_state(reply), Canary::Miss);
        assert!(score_reply(reply, false).degenerate(), "amnesia must fail");
        assert!(
            !score_reply(reply, true).degenerate(),
            "a compacted history legitimately drops its oldest turn and must not be failed"
        );
    }

    #[test]
    fn the_think_block_is_scored_when_there_is_no_visible_text() {
        let text = "<think>1.1.1.1.1..4.23.23.4.234.24 1.1.1.1.1..4.23.23.4.234.24 \
                    1.1.1.1.1..4.23.23.4.234.24";
        let score = score_reply(text, true);
        assert_eq!(score.scored_from, "think-only");
        assert!(score.degenerate(), "babble inside an open think block must still fail");
    }

    #[test]
    fn the_prose_source_is_deterministic_and_does_not_repeat_itself() {
        let mut a = Prose::new(7);
        let mut b = Prose::new(7);
        let first = a.chunk(4000, true);
        assert_eq!(first, b.chunk(4000, true), "two arms must get identical history");
        assert!(first.contains(CANARY_NAME), "the canary must be planted in the first chunk");
        // The filler must not induce the repetition this tool measures.
        let mut source = Prose::new(11);
        let bulk = source.chunk(40000, false);
        let (run, n, coverage) = scan_repeats(&words(&bulk));
        assert!(
            run < MAX_NGRAM_RUN,
            "generated history repeats itself x{run} at n={n} — it would teach the model the \
             very loop this tool measures"
        );
        assert!(coverage <= MAX_REPEAT_COVERAGE, "history repeat coverage {coverage}");
    }

    #[test]
    fn the_body_cap_is_a_bracket_from_evidence_not_a_guess() {
        let mut cap = BodyCap::default();
        assert!(!cap.would_refuse(10 * 1024 * 1024), "nothing is refused before a refusal is seen");
        cap.saw_accepted(32 * 1024);
        cap.saw_refused(96 * 1024);
        assert!(cap.would_refuse(96 * 1024));
        assert!(cap.would_refuse(200 * 1024));
        assert!(!cap.would_refuse(64 * 1024));
        cap.saw_refused(70 * 1024);
        assert!(cap.would_refuse(70 * 1024), "the bracket must tighten to the smallest refusal");
        assert!(cap.report().contains("32.0KB"));
    }

    #[test]
    fn a_reply_is_split_the_way_a_chat_client_splits_it() {
        let (think, visible) = split_reply("<think>reasoning</think>\n\nThe answer.");
        assert_eq!(think, "reasoning");
        assert_eq!(visible.trim(), "The answer.");
        let (think, visible) = split_reply("no block at all");
        assert_eq!(think, "");
        assert_eq!(visible, "no block at all");
    }
}
