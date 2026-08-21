//! Two lanes, both speculating: the gate, and the aggregate.
//!
//! `llama-slot-probe` gates the batched path with speculation ON but never
//! reaches a batched speculative ROUND — its gate 6 timeline runs one lane
//! session-native and the rest unspeculated. This one drives the shipping
//! `LaneExecutor` in the only shape that exercises the fused round: two or more
//! lanes, all decoding, all drafting, one verify batch serving all of them.
//!
//! **The gate is neighbour invariance, and it has to be.** B.14 of
//! `feasibility.md` closed the cross-width form: on the shipped Q8_1 mat-vec
//! route, width dependence of ~0.17 logits is structural, so a lane at batch
//! width 2 cannot be expected to reproduce itself at width 1, and a gate that
//! demanded it would be measuring the quantiser. What CAN be demanded is that
//! at a FIXED width timeline, a lane's stream does not depend on what its
//! neighbour is saying: the Q8_1 quantiser is per column, so a different
//! neighbour changes other columns and not this lane's. Any difference is
//! cross-lane bleed and nothing else.
//!
//! Both runs therefore submit every job BEFORE the first step. Prefill takes
//! priority over decode in the scheduler and runs lane by lane, so no decode
//! step happens until every lane is ingested — which makes the decode window
//! pure width-N in both runs however long the prompts are.
//!
//! **Every gate here refuses to pass vacuously.** A run that fell back to the
//! unspeculated step, or whose neighbour produced the same stream both times,
//! or whose window was too short to mean anything, FAILS rather than passes.
//! Five silent-corruption bugs in this lane were caught by that discipline and
//! none of them by an assertion that was reached.
//!
//! ```text
//! llama-lane-spec-probe <model.gguf> [--lanes N] [--spec N] [--context N]
//!                                    [--steps N] [--measure]
//! ```

use std::collections::HashMap;

use makepad_ai_llm::{
    LaneEvent, LaneExecutor, LaneRequest, LaneScheduler, LlamaModel, LlamaSamplingParams,
    LlamaSession, LlamaSessionConfig, LlamaVocab,
};

/// Graph activations grow with the slot count: wider batches mean wider
/// intermediates, and the arena the attention mask spans grows with it.
fn activation_reserve(slots: u32) -> usize {
    (512 + 192 * slots.max(1) as usize) << 20
}

/// Decode steps the gate compares over, and the fewest it will accept.
///
/// A window shorter than this has not exercised enough rounds to catch a bleed
/// that takes a few tokens to show, so it fails instead of passing thin.
const GATE_STEPS: usize = 24;
const MIN_GATE_TOKENS: usize = 12;

fn build_session(
    model: &LlamaModel,
    slots: u32,
    per_slot_context: u32,
    spec: usize,
) -> Result<LlamaSession, String> {
    LlamaSession::from_model(
        model,
        LlamaSessionConfig {
            max_context: Some(per_slot_context),
            max_sequences: slots,
            extra_activation_bytes: activation_reserve(slots),
            spec_draft_max: spec,
            ..LlamaSessionConfig::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// One run's observations.
struct Run {
    /// Tokens each job produced, in order.
    streams: HashMap<u64, Vec<i32>>,
    /// For job 1 only: how many tokens it had produced at the end of each step.
    lane_one_by_step: Vec<usize>,
    /// The step at which the batch stopped being `width` lanes wide.
    width_held_for: usize,
    /// Batched speculative rounds the executor ran, and the shape of the last.
    spec_rounds: u64,
    last_shape: Option<(usize, usize)>,
    /// Wall time and token count of the decode window, for the measurement.
    decode_seconds: f64,
    decode_tokens: usize,
}

/// Submit every job, prefill them all, then decode `steps` steps.
///
/// The submit-then-prefill order is what pins the width timeline: the scheduler
/// prefers prefill over decode and does one lane at a time, so every lane is
/// ingested before the first decode step whatever the prompts weigh.
///
/// Takes the session by value and gives it back, so a sweep pays for the
/// weights once. `reset` first, which clears the WHOLE device state — every
/// slot's rows, the recurrent arena and the carry tensor — so each run starts
/// from the same zeros the last one did.
fn run_lanes(
    mut session: LlamaSession,
    vocab: &LlamaVocab,
    prompts: &[String],
    steps: usize,
    budget: usize,
) -> Result<(LlamaSession, Run), String> {
    let width = prompts.len();
    session.reset().map_err(|e| format!("reset: {e}"))?;
    let table = session.new_slot_table().map_err(|e| e.to_string())?;
    let prefill_chunk = session.config().prefill_batch_size;
    let scheduler = LaneScheduler::new(table, 8).with_prefill_chunk(prefill_chunk);
    // Greedy, so a divergence is the scheduler's or the kernels' and never the
    // RNG's. Speculation stays on: rejection sampling at temperature 0 reduces
    // to "accept iff the draft is the argmax", which is deterministic.
    let params = LlamaSamplingParams {
        temperature: 0.0,
        ..LlamaSamplingParams::default()
    };
    let mut exec = LaneExecutor::new(session, scheduler, params);

    for (index, prompt) in prompts.iter().enumerate() {
        let tokens = vocab
            .tokenize(prompt, true, true)
            .map_err(|e| e.to_string())?;
        exec.scheduler()
            .submit(LaneRequest {
                job: index as u64 + 1,
                session: format!("spec-gate-{}", index + 1),
                prompt_tokens: tokens,
                reset_first: true,
                max_new: budget,
                sampling: params,
            })
            .map_err(|r| format!("submit refused job {}", r.job))?;
    }

    let mut run = Run {
        streams: HashMap::new(),
        lane_one_by_step: Vec::new(),
        width_held_for: 0,
        spec_rounds: 0,
        last_shape: None,
        decode_seconds: 0.0,
        decode_tokens: 0,
    };
    let record = |events: Vec<LaneEvent>, streams: &mut HashMap<u64, Vec<i32>>| -> usize {
        let mut produced = 0;
        for event in events {
            if let LaneEvent::Token { job, token, .. } = event {
                streams.entry(job).or_default().push(token);
                produced += 1;
            }
        }
        produced
    };

    // Prefill, untimed: every lane, to completion.
    //
    // The loop condition is `is_idle`, which counts PENDING work as well as
    // claimed lanes. Anything narrower is false on the first iteration —
    // nothing is admitted until `step` calls `admit_pending`, so a condition
    // written against lane occupancy skips the loop entirely and reports a run
    // that never decoded.
    let mut guard = 0usize;
    while !exec.is_idle() {
        // The first token produced is the signal that prefill is over: a
        // prefill step emits nothing until the chunk that finishes a prompt,
        // and the scheduler decodes nothing while any lane still needs one.
        let events = exec.step()?;
        let produced = record(events, &mut run.streams);
        if produced > 0 {
            run.lane_one_by_step
                .push(run.streams.get(&1).map(|s| s.len()).unwrap_or(0));
            run.width_held_for = 1;
            break;
        }
        guard += 1;
        if guard > 4096 {
            return Err("prefill never finished in 4096 steps".to_string());
        }
    }
    if run.width_held_for == 0 {
        return Err("no lane produced a token; the run never reached decode".to_string());
    }

    // Decode window, timed, and only while the batch is the width it started
    // at. A lane retiring narrows the batch, and past that point the remaining
    // lanes are running a different kernel shape — comparing across it would be
    // the cross-width question B.14 already closed.
    let started = std::time::Instant::now();
    let mut tokens = 0usize;
    for _ in 1..steps {
        if exec.scheduler().lanes_active() != width {
            break;
        }
        let events = exec.step()?;
        tokens += record(events, &mut run.streams);
        run.lane_one_by_step
            .push(run.streams.get(&1).map(|s| s.len()).unwrap_or(0));
        run.width_held_for += 1;
    }
    run.decode_seconds = started.elapsed().as_secs_f64();
    run.decode_tokens = tokens;
    run.spec_rounds = exec.batched_spec_rounds();
    run.last_shape = exec.last_batched_spec();
    Ok((exec.into_session(), run))
}

/// THE GATE: two lanes both speculating, and lane 1 blind to its neighbour.
fn neighbour_invariance(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    per_slot_context: u32,
    spec: usize,
    width: usize,
) -> Result<(), String> {
    if spec == 0 {
        return Err(
            "needs --spec N: with speculation off there is no batched round to gate, and \n  \
             passing here would certify the unspeculated path a second time"
                .to_string(),
        );
    }
    let mut a_prompts = vec![lane_prompt(0)];
    let mut b_prompts = vec![lane_prompt(0)];
    for index in 1..width {
        a_prompts.push(lane_prompt(index));
        b_prompts.push(alt_prompt(index));
    }

    let session = build_session(model, slots, per_slot_context, spec)?;
    let (session, a) = run_lanes(session, vocab, &a_prompts, GATE_STEPS, 512)?;
    let (_session, b) = run_lanes(session, vocab, &b_prompts, GATE_STEPS, 512)?;

    // NON-VACUITY, before any comparison. Each of these has a way of being
    // silently untrue, and a gate that compared two unspeculated runs would
    // pass while proving nothing about the path it names.
    for (name, run) in [("A", &a), ("B", &b)] {
        if run.spec_rounds == 0 {
            return Err(format!(
                "run {name} ran no batched speculative round at all — the executor took the \n  \
                 plain step, so this gate would have certified the wrong path"
            ));
        }
        match run.last_shape {
            Some((w, depth)) if w == width && depth > 0 => {}
            other => {
                return Err(format!(
                    "run {name} last ran a batched round of shape {other:?}, not {width} lanes \n  \
                     at a non-zero depth"
                ))
            }
        }
    }

    // The comparison window: steps both runs held the full width for. Past the
    // first retirement the batch narrows and the kernels change shape.
    let window = a.width_held_for.min(b.width_held_for);
    let lane_a = a.streams.get(&1).cloned().unwrap_or_default();
    let lane_b = b.streams.get(&1).cloned().unwrap_or_default();
    let count_a = a.lane_one_by_step.get(window - 1).copied().unwrap_or(0);
    let count_b = b.lane_one_by_step.get(window - 1).copied().unwrap_or(0);
    let compare = count_a.min(count_b).min(lane_a.len()).min(lane_b.len());
    if compare < MIN_GATE_TOKENS {
        return Err(format!(
            "only {compare} tokens of lane 1 fell inside a window both runs held {width} lanes \n  \
             for ({window} steps); fewer than {MIN_GATE_TOKENS} is too thin to mean anything"
        ));
    }
    if lane_a[..compare] != lane_b[..compare] {
        let at = lane_a
            .iter()
            .zip(&lane_b)
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        return Err(format!(
            "lane 1 changed at token {at} when only its NEIGHBOURS changed — cross-lane bleed\n  \
             with A: {:?}\n  with B: {:?}",
            &lane_a[..compare],
            &lane_b[..compare]
        ));
    }

    // And the experiment has to have been live: if the neighbours produced the
    // same stream both times, nothing varied and the invariance is trivial.
    let mut varied = false;
    for job in 2..=width as u64 {
        let n_a = a.streams.get(&job).cloned().unwrap_or_default();
        let n_b = b.streams.get(&job).cloned().unwrap_or_default();
        if n_a != n_b {
            varied = true;
        }
    }
    if !varied {
        return Err(
            "every neighbour produced the same stream in both runs, so the experiment did \n  \
             not vary and lane 1 being unchanged proves nothing"
                .to_string(),
        );
    }

    let (_, depth) = a.last_shape.expect("checked above");
    println!("  {width} lanes, uniform draft depth {depth}, {} rounds", a.spec_rounds);
    println!("  window {window} steps, {compare} tokens of lane 1 compared");
    println!("  neighbours differ between runs (experiment is live)");
    Ok(())
}

/// The aggregate: what N lanes with speculation are worth against the no-spec
/// floor, on the same box, the same fill and the same prompts.
fn measure(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    per_slot_context: u32,
    spec: usize,
    steps: usize,
) -> Result<(), String> {
    let mut session = Some(build_session(model, slots, per_slot_context, spec)?);
    println!(
        "{:>6}  {:>7}  {:>8}  {:>9}  {:>11}  {:>11}",
        "lanes", "depth", "steps", "tokens", "per lane", "aggregate"
    );
    for width in 1..=slots as usize {
        let prompts: Vec<String> = (0..width).map(lane_prompt).collect();
        // Every rung of the depth ladder that fits, plus 0 for the floor. The
        // modelled choice is one of these, and the point of measuring is to
        // find out whether the model picked the right one on THIS box.
        //
        // Width 1 is not on the ladder at all: a sole lane runs the SESSION-
        // NATIVE path at the session's own draft depth, and the batched depth
        // knob has nothing to say about it. It is measured anyway, because it
        // is the number that must not have moved.
        let ladder = if width == 1 {
            0
        } else {
            makepad_ai_llm::slots::draft_depth_for(width, spec)
        };
        for depth in 0..=ladder {
            std::env::set_var("MAKEPAD_LLAMA_BATCH_SPEC_DEPTH", depth.to_string());
            // The solo path decodes a whole chunk per step, so it needs far
            // fewer steps to time honestly than a batched one does.
            let want = if width == 1 { steps.min(8) } else { steps };
            let (returned, run) = run_lanes(
                session.take().expect("session"),
                vocab,
                &prompts,
                want,
                4096,
            )?;
            session = Some(returned);
            if run.width_held_for < 8.min(want) {
                return Err(format!(
                    "the {width}-lane run held its width for only {} steps, which is too few \n  \
                     to time",
                    run.width_held_for
                ));
            }
            if width > 1 && depth > 0 && run.spec_rounds == 0 {
                return Err(format!(
                    "the {width}-lane run at depth {depth} ran no batched speculative round, so \n  \
                     this row is the unspeculated rate wearing a depth label"
                ));
            }
            let aggregate = run.decode_tokens as f64 / run.decode_seconds;
            println!(
                "{:>6}  {:>7}  {:>8}  {:>9}  {:>11.1}  {:>11.1}",
                width,
                if width == 1 {
                    format!("solo {spec}")
                } else {
                    depth.to_string()
                },
                run.width_held_for,
                run.decode_tokens,
                aggregate / width as f64,
                aggregate
            );
        }
    }
    std::env::remove_var("MAKEPAD_LLAMA_BATCH_SPEC_DEPTH");
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!(
            "usage: llama-lane-spec-probe <model.gguf> [--lanes N] [--spec N] [--context N] \
             [--steps N] [--measure]"
        );
        std::process::exit(2);
    });
    let mut lanes = 4u32;
    let mut width = 2usize;
    let mut spec = 3usize;
    let mut context = 8192u32;
    let mut steps = 48usize;
    let mut measure_only = false;
    let rest: Vec<String> = args.collect();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--lanes" => {
                lanes = rest[index + 1].parse().unwrap_or(lanes);
                index += 2;
            }
            "--width" => {
                width = rest[index + 1].parse().unwrap_or(width);
                index += 2;
            }
            "--spec" => {
                spec = rest[index + 1].parse().unwrap_or(spec);
                index += 2;
            }
            "--context" => {
                context = rest[index + 1].parse().unwrap_or(context);
                index += 2;
            }
            "--steps" => {
                steps = rest[index + 1].parse().unwrap_or(steps);
                index += 2;
            }
            "--measure" => {
                measure_only = true;
                index += 1;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    let model = LlamaModel::load(std::path::Path::new(&path)).unwrap_or_else(|e| {
        eprintln!("load {path}: {e}");
        std::process::exit(1);
    });
    let vocab = LlamaVocab::from_model(&model).unwrap_or_else(|e| {
        eprintln!("vocab: {e}");
        std::process::exit(1);
    });

    if measure_only {
        if let Err(e) = measure(&model, &vocab, lanes, context, spec, steps) {
            eprintln!("FAIL (measure): {e}");
            std::process::exit(1);
        }
        return;
    }

    eprintln!("gate: {width} of {lanes} lanes, {context} context each, --spec {spec}");
    match neighbour_invariance(&model, &vocab, lanes, context, spec, width) {
        Ok(()) => println!("PASS: every lane speculated, and none of them heard its neighbour"),
        Err(e) => {
            eprintln!("FAIL (batched speculation): {e}");
            std::process::exit(1);
        }
    }
}

/// A real conversation's worth of context, plus a lane-specific question.
///
/// Real length on purpose. The lane's own record says it twice: a gate that
/// exercises a path with toy inputs has not exercised it, and every bug that
/// reached a player was a path that worked at thirty tokens and broke at eight
/// thousand. It also keeps the ACCEPTANCE honest — a repetitive synthetic body
/// is trivially predictable, so a draft head would look far better on it than
/// on anything a user types.
fn lane_prompt(index: usize) -> String {
    format!("{CONTEXT}\n\n{}", QUESTIONS[index % QUESTIONS.len()])
}

/// The same length and shape of context, different content — so a neighbour
/// varies without the width timeline varying.
fn alt_prompt(index: usize) -> String {
    format!("{ALT_CONTEXT}\n\n{}", ALT_QUESTIONS[index % ALT_QUESTIONS.len()])
}

const QUESTIONS: &[&str] = &[
    "Given all of that, explain in your own words why the verify batch dominates a \
     speculative round on this hardware, and what would have to change for the draft \
     chain to become the expensive half instead.",
    "Given all of that, explain why heterogeneous draft depths cannot be expressed in a \
     single fused verify batch, and describe what padding one would actually cost.",
    "Given all of that, explain why a conversation resuming a parked lane must extend the \
     lane's entire history rather than any prefix of it.",
    "Given all of that, explain what a slot's recurrent state row is for and why two \
     conversations must never share one.",
];

const ALT_QUESTIONS: &[&str] = &[
    "Given all of that, describe how you would decide whether a new soil mix is working, \
     and which measurements you would trust over which observations.",
    "Given all of that, explain why the second year of a hedgerow matters more than the \
     first, and what a grower should be doing during it.",
    "Given all of that, describe the trade-off between planting density and eventual yield \
     in the terms the passage sets out.",
    "Given all of that, explain what the passage means by a season being 'late' and why \
     that is not the same as being cold.",
];

/// ~1,400 words of real, non-repetitive technical prose. The subject is
/// deliberately the system under test: it is text this model will not find
/// trivially predictable, and it is true, so a reply to it can be read for
/// sense by whoever runs the gate.
const CONTEXT: &str = "\
You are reading the design notes for a batched inference server built around a hybrid \
attention and linear-recurrence language model. The notes are written for somebody who \
already knows what a transformer is but has not thought carefully about what happens when \
one machine serves several conversations at once, so they start from the memory system and \
work outward from there.\n\n\
The first thing to understand is that decoding a single token is not compute bound. The \
arithmetic a decode step performs is a handful of matrix-vector products, and a \
matrix-vector product reads its whole matrix and does almost nothing with each element. On \
a modern accelerator the multiply units finish long before the memory system has delivered \
the next block of weights, so the step takes as long as it takes to stream the parameters \
through the chip once. That single fact is responsible for almost every design decision \
that follows. If a step is going to read every weight anyway, then serving two \
conversations in that same step is very nearly free: the weights are read once and used \
twice. Serving them in two consecutive steps reads everything twice and takes twice as \
long. Batching is not an optimisation here; it is the difference between using the machine \
and idling it.\n\n\
The cache is what makes batching awkward. Attention layers remember every token they have \
seen as a pair of vectors per layer, and those pairs have to stay addressable for as long \
as the conversation lives. Serving several conversations means several such histories, and \
they cannot simply be concatenated, because each conversation must attend to its own past \
and nobody else's. The usual answer is to give the cache a sequence dimension and to carry, \
for every token in a batch, the index of the row it writes and the range of rows it is \
allowed to read. The masking is the interesting part. A token is allowed to see rows from \
the base of its own conversation's region up to and including its own position, and \
everything else is driven to negative infinity before the softmax. Get the lower bound \
wrong and a conversation reads the tail of whoever occupied those rows before it, which \
does not crash and does not warn: the reply is fluent and is about a conversation that \
never happened.\n\n\
The recurrent layers are harder, and they are harder in a way that is easy to miss. An \
attention cache is addressable. If a step turns out to have been wrong, the rows it wrote \
can simply be overwritten, because nothing downstream has folded them into anything. A \
linear recurrence has no such property. Its state is a running summary of every token so \
far, updated in place, and there is no operation that removes a token from it. You cannot \
truncate a recurrent state to a prefix. You cannot rewind it. The only way back to an \
earlier state is to have kept a copy of it. That single asymmetry decides how speculative \
decoding has to work on a hybrid model, and it decides what a conversation is allowed to \
do when it returns to a machine that still holds its state.\n\n\
Speculative decoding is the technique of guessing several tokens cheaply and then checking \
them all at once. A small draft head proposes a short chain of continuations; the full \
model then runs a single forward pass over the whole chain, producing a distribution for \
every position in it; and a rejection test decides how much of the chain survives. The \
economics are simple. The verify pass costs roughly what one ordinary decode step costs, \
because it is still bounded by reading the weights, but it can commit several tokens \
instead of one. If the draft head is accurate enough, the average tokens per pass rises and \
the effective rate rises with it. If it is not, the extra columns are paid for and thrown \
away, and speculation makes the machine slower than not speculating at all. There is no \
middle ground where it is harmlessly neutral: every drafted column has a cost.\n\n\
For the recurrent layers, the verify pass has to solve the rewind problem in advance. It \
does this by checkpointing: as the recurrent scan walks the chain of candidate tokens, it \
writes the state after each one into its own row. When the rejection test decides that \
three of five candidates survived, the next step does not need to undo anything. It simply \
resumes from the checkpoint belonging to the third token. Committing is a change of which \
row the next scan reads. That is elegant, and it is also a trap for whoever wires it up, \
because the row a conversation resumes from is now a piece of per-conversation state that \
moves on its own every round. Anything that reads it out of a stale copy will decode \
against the state of a different number of tokens, fluently and without complaint.\n\n\
Now combine the two ideas, because that combination is the point of the whole exercise. If \
one verify pass can serve several candidate tokens, and one batched step can serve several \
conversations, then one verify pass ought to be able to serve several conversations' \
candidate chains at once. It can, with one constraint that turns out to be load bearing: \
the batch has to be rectangular. The recurrent scan reshapes its input by sequence, so \
every conversation in a single fused pass must contribute the same number of tokens. \
Conversations that want different draft depths cannot share a pass. Padding the shallow \
ones costs exactly the column that padding was meant to save, and giving each conversation \
its own narrow pass throws away the shared weight read that made batching worthwhile in the \
first place. So a step picks one depth for everybody.\n\n\
That constraint has a consequence for how the depth may be chosen. The obvious scheme is to \
give each conversation the depth its recent acceptance rate justifies, handing columns from \
those whose guesses keep missing to those whose guesses land. Under a rectangular batch that \
scheme cannot be implemented, and the version of it that can — take some average and apply \
it to everybody — has a subtler problem. It makes one conversation's output depend on \
another conversation's behaviour. Not its content, but its timing and its luck. That is \
precisely the kind of coupling that makes a system impossible to test, because the only \
property a batched decoder can really be gated on is that a conversation's output does not \
change when its neighbours change. A depth chosen from the neighbours' statistics breaks \
that property deliberately, and then no gate can tell the deliberate breakage apart from a \
genuine bug.\n\n\
Which brings us to what can be tested at all. It would be pleasant if speculation were \
transparent: if turning it on produced exactly the tokens that turning it off produces. On \
this hardware it does not, and the reason is not a bug. The quantised matrix-vector kernel \
quantises its activations per column, in blocks, and the block statistics depend on how many \
columns are in flight. Any difference at all, however small, gets pushed through a rounding \
step whose output jumps by a fixed amount once the input crosses a bucket boundary. The \
amplifier saturates: shrinking the cause does not shrink the effect, it only changes which \
values flip. So identical output across batch widths is not available at any price worth \
paying, and the gate has to be shaped differently — hold the width fixed, change only who \
the neighbours are, and demand that the conversation under test does not notice.";

/// The same weight of prose on an unrelated subject, so a neighbour lane can be
/// varied without varying anything else about the run.
const ALT_CONTEXT: &str = "\
These are working notes from four seasons of trying to establish a mixed hedgerow on thin \
chalk soil, written for somebody who has planted a garden before but has never had to think \
about what happens underneath it. They start with the ground and work upward, because \
almost every mistake recorded here was a mistake about the ground that only became visible \
in the leaves.\n\n\
Thin soil over chalk drains fast and holds very little. Water that falls on it in March is \
gone by April, and the nutrients that were dissolved in that water are gone with it. This \
is not a deficiency that can be corrected by feeding, because feeding puts soluble material \
into a medium that has no capacity to retain solubles. What the ground lacks is structure: \
something that holds water against gravity long enough for roots to reach it, and holds \
minerals against rain long enough for roots to take them up. Structure is built by organic \
matter and by the fungal networks that colonise it, and both of those take years rather \
than weeks. The first practical consequence is that the correct time to improve a hedgerow's \
soil is two seasons before the hedgerow is planted, and the second is that anybody who \
skips that step will spend the next four years compensating for it.\n\n\
The choice of species matters less than the choice of stock. Bare-root whips lifted in the \
dormant season and planted within a few days establish far better than container-grown \
plants of the same species and twice the price, because a container-grown root system has \
spent its life in a medium with different physics and has to learn the new one. A whip has \
no such habit to unlearn. It also has almost no reserves, which is why the first season \
after planting is not about growth at all. A whip that puts on very little top growth in its \
first summer and simply holds its leaves is doing exactly what it should: it is building \
root. One that flushes hard and then wilts in July has usually been planted into a hole \
that was dug, filled with compost, and thereby turned into a bucket — a pocket of pleasant \
material surrounded by chalk, which the roots circle inside rather than leaving.\n\n\
The second year is where a hedge is won or lost, and it is the year most growers stop paying \
attention. The plants look established. They are not. Their roots have reached the edge of \
the disturbed ground and are meeting undisturbed chalk for the first time, and whether they \
push through it or turn back depends almost entirely on how much competition they are facing \
at the surface. Grass is the enemy here, not weeds in the ornamental sense. Grass roots \
occupy exactly the depth band a young woody plant needs, they are active early in the \
season, and they are extremely efficient at taking water before anything else gets it. A \
metre of clear ground around each whip, maintained through the second summer, is worth more \
than any amount of feeding, and mulch that suppresses grass is worth more than mulch that \
feeds.\n\n\
Density is the decision people get wrong most often, and they get it wrong in a way that \
feels generous. A hedge planted at five plants per metre in a double staggered row looks \
immediately like a hedge and is, for the first three years, denser and more satisfying than \
one planted at three. By year six the dense planting is thinner at the base, because the \
plants have competed each other into drawing upward for light instead of branching outward, \
and a hedge that is thin at the base is not doing the job a hedge exists to do. The \
generous-feeling decision produces the worse result, and it produces it slowly enough that \
the cause is easy to miss.\n\n\
Cutting is the same trade in a different form. A newly planted whip that is cut hard in its \
first winter looks brutalised, and produces two or three shoots from near the ground the \
following spring instead of one leader. Those low shoots are the entire future structure of \
the hedge. A whip left uncut grows a single stem, and a hedge of single stems is a row of \
small trees with gaps underneath. The cut is the difference between a hedge and a line of \
saplings, and it has to happen before the plant has invested in the shape you are about to \
remove.\n\n\
Season timing deserves a note of its own, because the word 'late' is used for two different \
things. A late season in the sense that matters is one where the soil warms slowly, and \
soil temperature is what governs root activity and therefore what governs whether a plant \
can supply the leaves it is about to open. A cold spring with warm ground is not late; the \
plants come away normally and simply look reluctant. A mild spring with cold, wet ground is \
late in the way that hurts: the air tells the buds to open, the roots cannot yet supply \
them, and the plant spends reserves it does not have. This is why a mild wet March is more \
dangerous to new planting than a hard dry one, and why measuring soil temperature at spade \
depth tells you more about what to expect than any amount of watching the weather.\n\n\
On judging whether an intervention worked: trust measurements that are taken at the same \
time of year, in the same place, by the same method, over several years, and distrust \
everything else. Growth in a single season is dominated by that season's rainfall, so a \
comparison between one year and the next is mostly a comparison of two weather patterns. \
The measurements that have been worth taking here are base diameter at a fixed height, the \
number of shoots below knee height, and the date of leaf fall — that last one being a good \
proxy for whether a plant went into winter in surplus or in deficit. Height is the \
measurement everybody takes and the least informative one available.";
