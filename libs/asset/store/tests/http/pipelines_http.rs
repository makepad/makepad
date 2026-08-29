//! Pipelines over HTTP: one declared graph enqueued up front, advancing
//! through the dependency gate and the claim-time splice with nobody
//! watching; derived state and weighted progress; cancel from anywhere;
//! `on_fail: skip`; and the `pipeline.finished` event.

mod common;

use common::*;
use makepad_asset_store::json::Value;

fn setup() -> (TestServer, Client, Client, Client) {
    let ts = start_server("pipelines");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let enq = principal_with(&mut admin, &[("job_enqueue", "demo"), ("job_cancel", "demo")]);
    let wrk = principal_with(&mut admin, &[("job_worker", "demo")]);
    let enqueuer = ts.control(Some(&enq));
    let worker = ts.control(Some(&wrk));
    (ts, admin, enqueuer, worker)
}

fn from_stage(stage: &str, field: &str) -> Value {
    jobj(vec![("$from_stage", jstr(stage)), ("field", jstr(field))])
}

fn stage(name: &str, kind: &str, weight: i64, body: Value) -> Value {
    jobj(vec![
        ("name", jstr(name)),
        ("kind", jstr(kind)),
        ("weight", Value::Int(weight)),
        ("body", body),
    ])
}

fn create(client: &mut Client, title: &str, prompt: &str, stages: Vec<Value>) -> String {
    let r = client.post_json(
        "/v1/pipelines",
        &jobj(vec![
            ("namespace", jstr("demo")),
            ("title", jstr(title)),
            ("prompt", jstr(prompt)),
            ("stages", Value::Arr(stages)),
        ]),
    );
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    r.str_field("pipeline")
}

fn claim(worker: &mut Client) -> Value {
    let r = worker.post_json("/v1/worker/claim", &jobj(vec![("lease_ms", Value::Int(60_000))]));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn detail(client: &mut Client, pipeline: &str) -> Value {
    let r = client.get(&format!("/v1/pipelines/{pipeline}"));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

fn stages_of(detail: &Value) -> Vec<Value> {
    detail.get("stages").and_then(Value::as_arr).expect("stages").to_vec()
}

fn field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn permille(detail: &Value) -> i64 {
    detail.get("permille").and_then(Value::as_i64).expect("permille")
}

/// The demo chain: expand → image → video, weights 5/15/80, every input
/// after the first spliced from the stage before it.
fn dream_stages() -> Vec<Value> {
    vec![
        stage("expand", "text.expand", 5, jobj(vec![("prompt", jstr("a city at night"))])),
        stage(
            "image",
            "image.generate",
            15,
            jobj(vec![("prompt", from_stage("expand", "text"))]),
        ),
        stage(
            "video",
            "video.generate",
            80,
            jobj(vec![
                ("prompt", jstr("animate it")),
                ("source_revision", from_stage("image", "revision")),
            ]),
        ),
    ]
}

fn succeed(worker: &mut Client, job: &str, result: Value) {
    let r = worker.post_json(
        "/v1/worker/succeed",
        &jobj(vec![("job", jstr(job)), ("result", result)]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
}

fn heartbeat(worker: &mut Client, job: &str, permille: i64, note: &str) -> u16 {
    worker
        .post_json(
            "/v1/worker/heartbeat",
            &jobj(vec![
                ("job", jstr(job)),
                ("extend_ms", Value::Int(60_000)),
                ("progress", jobj(vec![
                    ("permille", Value::Int(permille)),
                    ("note", jstr(note)),
                ])),
            ]),
        )
        .status
}

/// The whole point: a three-stage run declared in ONE request advances by
/// itself — the dependency gate orders it, the claim-time splice carries
/// each stage's product into the next, and the aggregate bar tells the
/// truth at every step. No client sits between the stages.
#[test]
fn a_declared_pipeline_advances_through_the_splice_with_nobody_watching() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let cursor = enqueuer.get("/v1/events?ev=5").str_field("cursor");
    let pipeline = create(&mut enqueuer, "DREAM", "a city at night", dream_stages());

    // Spawn IS visibility: the record and all three pending stage jobs
    // exist before anything has been claimed.
    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "state"), "running");
    assert_eq!(permille(&d), 0);
    assert_eq!(field(&d, "current_stage"), "expand");
    let stages = stages_of(&d);
    assert_eq!(stages.len(), 3);
    for stage in &stages {
        assert_eq!(field(stage, "state"), "pending");
    }
    // The author-friendly `$from_stage` was rewritten into the wire form
    // the claim-time splice understands, against the minted job ids.
    let image_prompt = stages[1].get("declared").unwrap().get("prompt").unwrap().clone();
    assert_eq!(
        image_prompt.get("$from").and_then(Value::as_str),
        stages[0].get("job").and_then(Value::as_str)
    );
    assert!(image_prompt.get("$from_stage").is_none());

    // Stage one runs. The bar is its weight's share of its own progress:
    // 5/100 of 50% is 25‰.
    let claimed = claim(&mut worker);
    let expand = claimed.get("job").and_then(Value::as_str).unwrap().to_string();
    assert_eq!(claimed.get("kind").and_then(Value::as_str), Some("text.expand"));
    assert_eq!(heartbeat(&mut worker, &expand, 500, "expanding"), 200);
    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(permille(&d), 25);
    assert_eq!(field(&d, "current_stage"), "expand");
    succeed(&mut worker, &expand, jobj(vec![("text", jstr("neon rain over a city at night"))]));
    // A completed stage contributes its WHOLE weight, always.
    assert_eq!(permille(&detail(&mut enqueuer, &pipeline)), 50);

    // Stage two claims with the expander's answer already in its body.
    let claimed = claim(&mut worker);
    let image = claimed.get("job").and_then(Value::as_str).unwrap().to_string();
    assert_eq!(
        claimed.get("body").unwrap().get("prompt").and_then(Value::as_str),
        Some("neon rain over a city at night")
    );
    succeed(&mut worker, &image, jobj(vec![("revision", jstr("arev_still7"))]));
    assert_eq!(permille(&detail(&mut enqueuer, &pipeline)), 200);

    // Stage three the same, through a differently named field.
    let claimed = claim(&mut worker);
    let video = claimed.get("job").and_then(Value::as_str).unwrap().to_string();
    assert_eq!(
        claimed.get("body").unwrap().get("source_revision").and_then(Value::as_str),
        Some("arev_still7")
    );
    assert_eq!(
        claimed.get("body").unwrap().get("prompt").and_then(Value::as_str),
        Some("animate it")
    );
    succeed(&mut worker, &video, jobj(vec![("revision", jstr("arev_clip9"))]));

    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "state"), "succeeded");
    assert_eq!(permille(&d), 1000);
    assert!(d.get("finished_ms").and_then(Value::as_i64).unwrap_or(0) > 0);
    let stages = stages_of(&d);
    assert_eq!(
        stages[2].get("result").unwrap().get("body").unwrap().get("revision").and_then(Value::as_str),
        Some("arev_clip9")
    );
    assert_eq!(stages[0].get("attempts").and_then(Value::as_i64), Some(1));

    // The completion EVENT — what a grid listens to, instead of guessing
    // from coincidental publishes.
    let r = enqueuer.get(&format!("/v1/events?ev=5&cursor={cursor}"));
    assert_eq!(r.status, 200);
    let events = r.json().get("events").and_then(Value::as_arr).unwrap().to_vec();
    let finished: Vec<&Value> = events
        .iter()
        .filter(|e| e.get("kind").and_then(Value::as_str) == Some("pipeline.finished"))
        .collect();
    assert_eq!(finished.len(), 1, "exactly one finish announcement");
    assert_eq!(finished[0].get("pipeline").and_then(Value::as_str), Some(pipeline.as_str()));
    assert_eq!(finished[0].get("pipeline_state").and_then(Value::as_str), Some("succeeded"));
    assert_eq!(finished[0].get("ns").and_then(Value::as_str), Some("demo"));

    // A subscriber on the older vocabulary is not lied to: it advances over
    // the sequence and receives no event it would have to invent a meaning
    // for.
    let r = enqueuer.get(&format!("/v1/events?ev=4&cursor={cursor}"));
    let events = r.json().get("events").and_then(Value::as_arr).unwrap().to_vec();
    assert!(events
        .iter()
        .all(|e| e.get("kind").and_then(Value::as_str) != Some("pipeline.finished")));

    // The listing renders the same run without a second request per stage.
    let r = enqueuer.get("/v1/pipelines");
    assert_eq!(r.status, 200);
    let rows = r.json().get("pipelines").and_then(Value::as_arr).unwrap().to_vec();
    assert_eq!(rows.len(), 1);
    assert_eq!(field(&rows[0], "title"), "DREAM");
    assert_eq!(field(&rows[0], "prompt"), "a city at night");
    assert_eq!(field(&rows[0], "state"), "succeeded");
    assert_eq!(rows[0].get("permille").and_then(Value::as_i64), Some(1000));
    assert_eq!(rows[0].get("stages").and_then(Value::as_i64), Some(3));
    // …and `state=active` drops what is no longer running.
    let r = enqueuer.get("/v1/pipelines?state=active");
    assert!(r.json().get("pipelines").and_then(Value::as_arr).unwrap().is_empty());
}

/// A run is inspectable from the instant it is spawned — before any box has
/// seen it. Every stage carries the body it was DECLARED with, which is the
/// answer to "what is this thing about to send".
#[test]
fn a_spawned_pipeline_is_inspectable_before_anything_runs() {
    let (_ts, _admin, mut enqueuer, _worker) = setup();
    let pipeline = create(&mut enqueuer, "Create", "a brass telescope", dream_stages());
    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "prompt"), "a brass telescope");
    let stages = stages_of(&d);
    assert_eq!(
        stages[0].get("declared").unwrap().get("prompt").and_then(Value::as_str),
        Some("a city at night")
    );
    for (index, stage) in stages.iter().enumerate() {
        assert_eq!(field(stage, "state"), "pending");
        assert_eq!(stage.get("seq").and_then(Value::as_i64), Some(index as i64));
        assert_eq!(stage.get("attempts").and_then(Value::as_i64), Some(0));
        assert!(stage.get("declared").is_some(), "a pending stage still shows what it will send");
        assert!(stage.get("records").is_none(), "nothing was sent yet");
        assert!(stage.get("result").is_none());
        assert!(stage.get("job").and_then(Value::as_str).unwrap().starts_with("job_"));
    }
    assert_eq!(stages[0].get("weight").and_then(Value::as_i64), Some(5));
    assert_eq!(field(&stages[0], "on_fail"), "fail");
    let r = enqueuer.get("/v1/pipelines?state=active");
    assert_eq!(r.json().get("pipelines").and_then(Value::as_arr).unwrap().len(), 1);
}

/// Cancel reaches the whole run from anywhere — the spawner does not have
/// to be alive. The running stage loses its lease (so its worker's next
/// report is refused and it drops the node dispatch), the tail never runs,
/// and what already succeeded is KEPT, marked as succeeded inside a
/// cancelled pipeline.
#[test]
fn cancelling_a_pipeline_stops_the_tail_and_keeps_what_finished() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let cursor = enqueuer.get("/v1/events?ev=5").str_field("cursor");
    let pipeline = create(&mut enqueuer, "DREAM", "a city at night", dream_stages());

    let expand = claim(&mut worker).get("job").and_then(Value::as_str).unwrap().to_string();
    succeed(&mut worker, &expand, jobj(vec![("text", jstr("neon rain"))]));
    let image = claim(&mut worker).get("job").and_then(Value::as_str).unwrap().to_string();
    assert_eq!(heartbeat(&mut worker, &image, 600, "rendering"), 200);

    let r = enqueuer.post_json(&format!("/v1/pipelines/{pipeline}/cancel"), &jobj(vec![]));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json().get("cancelled").and_then(Value::as_i64), Some(2));
    assert_eq!(r.str_field("state"), "cancelled");

    // The worker learns on its next report: the lease is gone, so it can
    // never overwrite the outcome — this is the same chain that kills the
    // node-side dispatch.
    assert_eq!(heartbeat(&mut worker, &image, 700, "rendering"), 409);
    // No orphaned stage: nothing is left claimable.
    assert!(claim(&mut worker).get("job").unwrap().as_str().is_none());

    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "state"), "cancelled");
    let stages = stages_of(&d);
    assert_eq!(field(&stages[0], "state"), "succeeded");
    assert_eq!(
        stages[0].get("result").unwrap().get("body").unwrap().get("text").and_then(Value::as_str),
        Some("neon rain"),
        "a finished stage keeps its product inside a cancelled run"
    );
    assert_eq!(field(&stages[1], "state"), "cancelled");
    assert_eq!(field(&stages[2], "state"), "cancelled");
    // 5/100 of a whole stage plus 15/100 of a stage frozen at 60%.
    assert_eq!(permille(&d), 50 + 90);

    let r = enqueuer.get(&format!("/v1/events?ev=5&cursor={cursor}"));
    let events = r.json().get("events").and_then(Value::as_arr).unwrap().to_vec();
    let finished: Vec<&Value> = events
        .iter()
        .filter(|e| e.get("kind").and_then(Value::as_str) == Some("pipeline.finished"))
        .collect();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].get("pipeline_state").and_then(Value::as_str), Some("cancelled"));
}

/// `on_fail: skip` is the never-lose-a-run law, structurally: the expander
/// failing must not doom the picture. The stage reads skipped, the stages
/// after it stop waiting for it, and every reference to its result becomes
/// the words the person actually typed.
#[test]
fn a_skipped_stage_falls_back_to_the_prompt_the_person_typed() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let pipeline = create(
        &mut enqueuer,
        "DREAM",
        "a lighthouse at dusk",
        vec![
            jobj(vec![
                ("name", jstr("expand")),
                ("kind", jstr("text.expand")),
                ("weight", Value::Int(5)),
                ("on_fail", jstr("skip")),
                ("body", jobj(vec![("prompt", jstr("a lighthouse at dusk"))])),
            ]),
            stage(
                "image",
                "image.generate",
                95,
                jobj(vec![("prompt", from_stage("expand", "text"))]),
            ),
        ],
    );

    let expand = claim(&mut worker).get("job").and_then(Value::as_str).unwrap().to_string();
    let r = worker.post_json(
        "/v1/worker/fail",
        &jobj(vec![
            ("job", jstr(expand.clone())),
            ("error", jobj(vec![("error", jstr("the model refused"))])),
        ]),
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("state"), "failed");

    // The pipeline is NOT failed: a skipped stage counts as done, and the
    // bar carries its whole weight.
    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "state"), "running");
    assert_eq!(permille(&d), 50);
    let stages = stages_of(&d);
    assert_eq!(field(&stages[0], "state"), "failed");
    assert_eq!(stages[0].get("skipped").and_then(Value::as_bool), Some(true));
    // The dependent's body was rewritten to the recorded prompt — visible
    // in what it will send, before it sends it.
    assert_eq!(
        stages[1].get("declared").unwrap().get("prompt").and_then(Value::as_str),
        Some("a lighthouse at dusk")
    );

    // …and it really does run, with that prompt.
    let claimed = claim(&mut worker);
    let image = claimed.get("job").and_then(Value::as_str).unwrap().to_string();
    assert_eq!(
        claimed.get("body").unwrap().get("prompt").and_then(Value::as_str),
        Some("a lighthouse at dusk")
    );
    succeed(&mut worker, &image, jobj(vec![("revision", jstr("arev_lighthouse"))]));
    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "state"), "succeeded");
    assert_eq!(permille(&d), 1000);
}

/// A hard failure fails the pipeline THE MOMENT it happens — dependents are
/// doomed lazily, at some later claim, and a person watching a dead run may
/// not be made to wait for a worker to poll.
#[test]
fn a_failed_stage_fails_the_pipeline_before_its_dependents_are_doomed() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let cursor = enqueuer.get("/v1/events?ev=5").str_field("cursor");
    let pipeline = create(&mut enqueuer, "Create", "a brass telescope", dream_stages());

    let expand = claim(&mut worker).get("job").and_then(Value::as_str).unwrap().to_string();
    assert_eq!(heartbeat(&mut worker, &expand, 400, "expanding"), 200);
    let r = worker.post_json(
        "/v1/worker/fail",
        &jobj(vec![
            ("job", jstr(expand.clone())),
            ("error", jobj(vec![("error", jstr("box unreachable"))])),
        ]),
    );
    assert_eq!(r.status, 200);

    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "state"), "failed");
    assert_eq!(field(&d, "current_stage"), "expand", "the card names the failure point");
    // The bar freezes where the work stopped: 5/100 of 40%.
    assert_eq!(permille(&d), 20);
    let stages = stages_of(&d);
    assert_eq!(field(&stages[0], "state"), "failed");
    assert_eq!(stages[0].get("skipped").and_then(Value::as_bool), Some(false));
    assert_eq!(field(&stages[1], "state"), "pending", "not doomed yet, and it does not matter");
    assert_eq!(
        stages[0].get("result").unwrap().get("body").unwrap().get("error").and_then(Value::as_str),
        Some("box unreachable")
    );

    let r = enqueuer.get(&format!("/v1/events?ev=5&cursor={cursor}"));
    let events = r.json().get("events").and_then(Value::as_arr).unwrap().to_vec();
    let finished: Vec<&Value> = events
        .iter()
        .filter(|e| e.get("kind").and_then(Value::as_str) == Some("pipeline.finished"))
        .collect();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].get("pipeline_state").and_then(Value::as_str), Some("failed"));

    // The doomed tail never reaches a box, and the pipeline's answer does
    // not change when it is finally cancelled.
    assert!(claim(&mut worker).get("job").unwrap().as_str().is_none());
    let d = detail(&mut enqueuer, &pipeline);
    assert_eq!(field(&d, "state"), "failed");
    assert_eq!(field(&stages_of(&d)[1], "state"), "cancelled");
    // Still exactly one announcement.
    let r = enqueuer.get(&format!("/v1/events?ev=5&cursor={cursor}"));
    let events = r.json().get("events").and_then(Value::as_arr).unwrap().to_vec();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.get("kind").and_then(Value::as_str) == Some("pipeline.finished"))
            .count(),
        1
    );
}

/// Everything a create request may not be. Each of these is refused BEFORE
/// a single job exists — a pipeline half in the queue is the one outcome
/// this route may never produce.
#[test]
fn create_refuses_what_it_cannot_run() {
    let (ts, mut admin, mut enqueuer, _worker) = setup();
    let post = |c: &mut Client, v: Value| c.post_json("/v1/pipelines", &v).status;
    let base = |stages: Vec<Value>| {
        jobj(vec![
            ("namespace", jstr("demo")),
            ("title", jstr("t")),
            ("prompt", jstr("p")),
            ("stages", Value::Arr(stages)),
        ])
    };
    let plain = |name: &str| stage(name, "image.generate", 10, jobj(vec![]));

    // Budgets: at most eight stages, weights 1..=1000, a title that is a
    // label and not a document.
    let nine: Vec<Value> = (0..9).map(|i| plain(&format!("s{i}"))).collect();
    assert_eq!(post(&mut enqueuer, base(nine)), 413);
    let eight: Vec<Value> = (0..8).map(|i| plain(&format!("s{i}"))).collect();
    assert_eq!(post(&mut enqueuer, base(eight)), 201);
    assert_eq!(post(&mut enqueuer, base(vec![])), 400);
    assert_eq!(post(&mut enqueuer, base(vec![stage("a", "image.generate", 0, jobj(vec![]))])), 400);
    assert_eq!(
        post(&mut enqueuer, base(vec![stage("a", "image.generate", 1001, jobj(vec![]))])),
        400
    );
    assert_eq!(
        post(&mut enqueuer, base(vec![stage("a", "image.generate", 1000, jobj(vec![]))])),
        201
    );
    let long_title = jobj(vec![
        ("namespace", jstr("demo")),
        ("title", jstr("t".repeat(201))),
        ("stages", Value::Arr(vec![plain("a")])),
    ]);
    assert_eq!(post(&mut enqueuer, long_title), 400);
    let ok_title = jobj(vec![
        ("namespace", jstr("demo")),
        ("title", jstr("t".repeat(200))),
        ("stages", Value::Arr(vec![plain("a")])),
    ]);
    assert_eq!(post(&mut enqueuer, ok_title), 201);

    // Shapes: duplicate names, a dep naming a later stage, a `$from_stage`
    // this stage never waits for, a malformed reference.
    assert_eq!(post(&mut enqueuer, base(vec![plain("a"), plain("a")])), 400);
    let forward = jobj(vec![
        ("name", jstr("a")),
        ("kind", jstr("image.generate")),
        ("deps", Value::Arr(vec![jstr("b")])),
    ]);
    assert_eq!(post(&mut enqueuer, base(vec![forward, plain("b")])), 400);
    let unwaited = jobj(vec![
        ("name", jstr("b")),
        ("kind", jstr("video.generate")),
        ("deps", Value::Arr(vec![])),
        ("body", jobj(vec![("prompt", from_stage("a", "text"))])),
    ]);
    assert_eq!(post(&mut enqueuer, base(vec![plain("a"), unwaited])), 400);
    let malformed = jobj(vec![
        ("name", jstr("b")),
        ("kind", jstr("video.generate")),
        ("body", jobj(vec![("prompt", jobj(vec![("$from_stage", jstr("a"))]))])),
    ]);
    assert_eq!(post(&mut enqueuer, base(vec![plain("a"), malformed])), 400);
    assert_eq!(post(&mut enqueuer, base(vec![plain("Bad Name")])), 400);
    assert_eq!(post(&mut enqueuer, base(vec![stage("a", "Bad Kind", 10, jobj(vec![]))])), 400);

    // `on_fail: skip` without a prompt has nothing to fall back TO.
    let skip_no_prompt = jobj(vec![
        ("namespace", jstr("demo")),
        ("title", jstr("t")),
        ("stages", Value::Arr(vec![jobj(vec![
            ("name", jstr("a")),
            ("kind", jstr("text.expand")),
            ("on_fail", jstr("skip")),
        ])])),
    ]);
    assert_eq!(post(&mut enqueuer, skip_no_prompt), 400);

    // Authorization is the job queue's, unchanged.
    let mut anon = ts.control(None);
    assert_eq!(post(&mut anon, base(vec![plain("a")])), 401);
    let stranger = principal_with(&mut admin, &[("job_enqueue", "elsewhere")]);
    let mut stranger = ts.control(Some(&stranger));
    assert_eq!(post(&mut stranger, base(vec![plain("a")])), 403);
    // …and a pipeline id nobody may see is the same 404 an unknown one is.
    let mine = create(&mut enqueuer, "mine", "p", vec![plain("a")]);
    assert_eq!(stranger.get(&format!("/v1/pipelines/{mine}")).status, 404);
    // Cancel refuses exactly as `POST /v1/jobs/<id>/cancel` does: the
    // capability, on the namespace, and its refusal spelling.
    assert_eq!(
        stranger.post_json(&format!("/v1/pipelines/{mine}/cancel"), &jobj(vec![])).status,
        403
    );
    assert_eq!(enqueuer.get("/v1/pipelines/pipe_nothex").status, 400);
    assert_eq!(enqueuer.get("/v1/pipelines?state=sideways").status, 400);
    assert_eq!(enqueuer.get("/v1/pipelines?ns=elsewhere").status, 403);
}
