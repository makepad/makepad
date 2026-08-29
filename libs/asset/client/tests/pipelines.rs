//! Declared pipelines over the real wire: one POST enqueues a whole run,
//! one GET draws it, one POST stops it.
//!
//! The fixture is a REAL TCP server that speaks the store's pipeline routes
//! byte for byte (`libs/asset/store/src/host/routes_control.rs`, exercised by
//! its own `tests/http/pipelines_http.rs`) — including the derived state and
//! the weighted aggregate, which it computes independently so the client's
//! local `aggregate_permille` has something to be checked against. Real
//! process coverage of the store belongs to the store crate; a clean-checkout
//! `cargo test` here must not need a prebuilt server binary.

mod common;

use common::{write_error, write_json_resp, ParsedRequest, RawServer};
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    default_stage_weight, stage_ref, Api, ApiEndpoints, ClientError, HttpLimits, JobStateDto,
    PipelineId, PipelineStageSpec, PipelineStateDto, StageOnFailDto, DEFAULT_STAGE_WEIGHTS,
    NEUTRAL_STAGE_WEIGHT,
};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const NOW: u64 = 1_756_400_000_000;
const PIPELINE: &str = "pipe_0123456789abcdef0123456789abcdef";

fn fast_limits() -> HttpLimits {
    HttpLimits {
        connect_timeout_ms: 2_000,
        read_timeout_ms: 1_000,
        write_timeout_ms: 2_000,
        head_deadline_ms: 2_000,
        body_deadline_ms: 2_000,
    }
}

fn api_at(addr: std::net::SocketAddr) -> Api {
    Api::new(ApiEndpoints { control: addr, data: addr }, fast_limits(), None).unwrap()
}

/// A RawServer that answers every request with one canned JSON document.
fn canned(status: u16, body: Value) -> RawServer {
    RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
        write_json_resp(stream, status, &body);
    }))
}

// ---------------------------------------------------------------------------
// the fixture store: the pipeline routes, as the server writes them
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FixStage {
    name: String,
    kind: String,
    job: String,
    weight: u16,
    on_fail: String,
    /// The body as enqueued (`$from_stage` already rewritten to `$from`).
    declared: Value,
    state: String,
    attempts: u32,
    progress: Option<(u16, String)>,
    records: Vec<Value>,
    result: Option<Value>,
}

#[derive(Default)]
struct FixState {
    /// Every request the fixture served, so a test can assert the exact
    /// document the client SENT, not just what it did with the answer.
    seen: Vec<ParsedRequest>,
    created: bool,
    title: String,
    prompt: String,
    namespace: String,
    stages: Vec<FixStage>,
}

impl FixState {
    /// The derived state, verbatim from the store's `pipeline_derive`:
    /// failed the moment a non-skipped stage fails (without waiting for the
    /// dependents to doom), else running, else cancelled, else succeeded.
    fn derive(&self) -> (&'static str, u64, Option<usize>) {
        let failed_at = self
            .stages
            .iter()
            .position(|s| s.state == "failed" && s.on_fail != "skip");
        let state = if failed_at.is_some() {
            "failed"
        } else if self.stages.iter().any(|s| s.state == "pending" || s.state == "running") {
            "running"
        } else if self.stages.iter().any(|s| s.state == "cancelled") {
            "cancelled"
        } else {
            "succeeded"
        };
        let mut total = 0u64;
        let mut done = 0u64;
        for st in &self.stages {
            let d = if st.state == "succeeded" || (st.state == "failed" && st.on_fail == "skip") {
                1000
            } else if st.state == "pending" {
                0
            } else {
                st.progress.as_ref().map(|(p, _)| *p as u64).unwrap_or(0)
            };
            total += st.weight as u64;
            done += st.weight as u64 * d;
        }
        let permille = if total == 0 { 0 } else { (done / total).min(1000) };
        let current = failed_at
            .or_else(|| self.stages.iter().position(|s| s.state == "running"))
            .or_else(|| self.stages.iter().position(|s| s.state == "pending"))
            .or_else(|| self.stages.len().checked_sub(1));
        (state, permille, current)
    }

    fn row(&self) -> Value {
        let (state, permille, current) = self.derive();
        let mut pairs = vec![
            ("pipeline", s(PIPELINE)),
            ("namespace", s(self.namespace.clone())),
            ("title", s(self.title.clone())),
            ("state", s(state)),
            ("permille", Value::Int(permille as i64)),
            ("stages", Value::Int(self.stages.len() as i64)),
            ("enqueued_by", s("prin_00000000000000000000000000000001")),
            ("created_ms", Value::Int(NOW as i64)),
        ];
        if !self.prompt.is_empty() {
            pairs.push(("prompt", s(self.prompt.chars().take(256).collect::<String>())));
        }
        if let Some(cur) = current.and_then(|at| self.stages.get(at)) {
            pairs.push(("current_stage", s(cur.name.clone())));
            if let Some((_, note)) = &cur.progress {
                if !note.is_empty() {
                    pairs.push(("note", s(note.clone())));
                }
            }
        }
        if state != "running" {
            pairs.push(("finished_ms", Value::Int(NOW as i64 + 90_000)));
        }
        obj(pairs)
    }

    fn detail(&self) -> Value {
        let (state, permille, current) = self.derive();
        let mut pairs = vec![
            ("pipeline", s(PIPELINE)),
            ("namespace", s(self.namespace.clone())),
            ("title", s(self.title.clone())),
            ("state", s(state)),
            ("permille", Value::Int(permille as i64)),
            ("enqueued_by", s("prin_00000000000000000000000000000001")),
            ("created_ms", Value::Int(NOW as i64)),
        ];
        if !self.prompt.is_empty() {
            pairs.push(("prompt", s(self.prompt.clone())));
        }
        if let Some(cur) = current.and_then(|at| self.stages.get(at)) {
            pairs.push(("current_stage", s(cur.name.clone())));
        }
        if state != "running" {
            pairs.push(("finished_ms", Value::Int(NOW as i64 + 90_000)));
        }
        let mut out = Vec::new();
        for (seq, st) in self.stages.iter().enumerate() {
            let mut sp = vec![
                ("name", s(st.name.clone())),
                ("seq", Value::Int(seq as i64)),
                ("job", s(st.job.clone())),
                ("kind", s(st.kind.clone())),
                ("state", s(st.state.clone())),
                ("skipped", Value::Bool(st.state == "failed" && st.on_fail == "skip")),
                ("weight", Value::Int(st.weight as i64)),
                ("on_fail", s(st.on_fail.clone())),
                ("attempts", Value::Int(st.attempts as i64)),
            ];
            if let Some((permille, note)) = &st.progress {
                sp.push((
                    "progress",
                    obj(vec![
                        ("permille", Value::Int(*permille as i64)),
                        ("note", s(note.clone())),
                        ("updated_ms", Value::Int(NOW as i64 + 1_000)),
                    ]),
                ));
            }
            sp.push(("declared", st.declared.clone()));
            if !st.records.is_empty() {
                sp.push(("records", Value::Arr(st.records.clone())));
            }
            if let Some(result) = &st.result {
                sp.push(("result", result.clone()));
            }
            out.push(obj(sp));
        }
        pairs.push(("stages", Value::Arr(out)));
        obj(pairs)
    }
}

struct PipeFixture {
    server: RawServer,
    state: Arc<Mutex<FixState>>,
}

impl PipeFixture {
    fn start() -> PipeFixture {
        let state = Arc::new(Mutex::new(FixState::default()));
        let handler_state = state.clone();
        let server = RawServer::start(Arc::new(move |req: ParsedRequest, stream: &mut TcpStream| {
            let mut st = handler_state.lock().unwrap();
            let segs = req.segs();
            let method = req.method.clone();
            st.seen.push(req.clone());
            let last = st.seen.len() - 1;
            match (method.as_str(), segs.as_slice()) {
                ("POST", [v1, pipelines]) if v1 == "v1" && pipelines == "pipelines" => {
                    let body =
                        makepad_asset_client::json::parse(&st.seen[last].body).expect("json body");
                    create(&mut st, &body);
                    let stages = st
                        .stages
                        .iter()
                        .map(|x| {
                            obj(vec![("name", s(x.name.clone())), ("job", s(x.job.clone()))])
                        })
                        .collect();
                    write_json_resp(
                        stream,
                        201,
                        &obj(vec![
                            ("pipeline", s(PIPELINE)),
                            ("stages", Value::Arr(stages)),
                        ]),
                    );
                }
                ("GET", [v1, pipelines]) if v1 == "v1" && pipelines == "pipelines" => {
                    let active_only = st.seen[last].query_get("state").as_deref() == Some("active");
                    let rows = if !st.created || (active_only && st.derive().0 != "running") {
                        Vec::new()
                    } else {
                        vec![st.row()]
                    };
                    write_json_resp(stream, 200, &obj(vec![("pipelines", Value::Arr(rows))]));
                }
                ("GET", [v1, pipelines, id]) if v1 == "v1" && pipelines == "pipelines" => {
                    if !st.created || id != PIPELINE {
                        write_error(stream, 404, "no such pipeline");
                        return;
                    }
                    write_json_resp(stream, 200, &st.detail());
                }
                ("POST", [v1, pipelines, id, cancel])
                    if v1 == "v1" && pipelines == "pipelines" && cancel == "cancel" =>
                {
                    if !st.created || id != PIPELINE {
                        write_error(stream, 404, "no such pipeline");
                        return;
                    }
                    let mut cancelled = 0i64;
                    for stage in st.stages.iter_mut() {
                        if stage.state == "pending" || stage.state == "running" {
                            stage.state = "cancelled".to_string();
                            cancelled += 1;
                        }
                    }
                    let state = st.derive().0;
                    write_json_resp(
                        stream,
                        200,
                        &obj(vec![
                            ("pipeline", s(PIPELINE)),
                            ("cancelled", Value::Int(cancelled)),
                            ("state", s(state)),
                        ]),
                    );
                }
                _ => write_error(stream, 404, "no route"),
            }
        }));
        PipeFixture { server, state }
    }

    fn api(&self) -> Api {
        api_at(self.server.addr)
    }

    fn requests(&self) -> usize {
        self.state.lock().unwrap().seen.len()
    }

    /// The body of the create request, as the server parsed it.
    fn create_body(&self) -> Value {
        let st = self.state.lock().unwrap();
        let req = st
            .seen
            .iter()
            .find(|r| r.method == "POST" && r.target == "/v1/pipelines")
            .expect("a create request");
        makepad_asset_client::json::parse(&req.body).expect("create body")
    }

    fn set(&self, stage: &str, f: impl FnOnce(&mut FixStage)) {
        let mut st = self.state.lock().unwrap();
        let at = st.stages.iter().position(|s| s.name == stage).expect("stage");
        f(&mut st.stages[at]);
    }
}

/// The create route: mint a job id per stage, rewrite `$from_stage` into the
/// wire `$from` form, keep the declared bodies.
fn create(st: &mut FixState, body: &Value) {
    st.created = true;
    st.namespace = body.get("namespace").and_then(Value::as_str).unwrap_or("").to_string();
    st.title = body.get("title").and_then(Value::as_str).unwrap_or("").to_string();
    st.prompt = body.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
    let stages = body.get("stages").and_then(Value::as_arr).expect("stages").to_vec();
    let names: Vec<String> = stages
        .iter()
        .map(|x| x.get("name").and_then(Value::as_str).unwrap_or("").to_string())
        .collect();
    let jobs: Vec<String> = (0..stages.len())
        .map(|i| format!("job_{:032x}", i + 1))
        .collect();
    st.stages = stages
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut declared = x.get("body").cloned().unwrap_or(Value::Obj(Vec::new()));
            rewrite(&mut declared, &names, &jobs);
            FixStage {
                name: names[i].clone(),
                kind: x.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
                job: jobs[i].clone(),
                weight: x.get("weight").and_then(Value::as_u64).unwrap_or(10) as u16,
                on_fail: x
                    .get("on_fail")
                    .and_then(Value::as_str)
                    .unwrap_or("fail")
                    .to_string(),
                declared,
                state: "pending".to_string(),
                attempts: 0,
                progress: None,
                records: Vec::new(),
                result: None,
            }
        })
        .collect();
}

fn rewrite(v: &mut Value, names: &[String], jobs: &[String]) {
    match v {
        Value::Obj(pairs) => {
            if let Some(stage) = pairs
                .iter()
                .find(|(k, _)| k == "$from_stage")
                .and_then(|(_, val)| val.as_str())
                .map(str::to_string)
            {
                let field = pairs
                    .iter()
                    .find(|(k, _)| k == "field")
                    .and_then(|(_, val)| val.as_str())
                    .unwrap_or("")
                    .to_string();
                let at = names.iter().position(|n| *n == stage).expect("named stage");
                *v = obj(vec![("$from", s(jobs[at].clone())), ("field", s(field))]);
                return;
            }
            for (_, val) in pairs.iter_mut() {
                rewrite(val, names, jobs);
            }
        }
        Value::Arr(items) => {
            for item in items.iter_mut() {
                rewrite(item, names, jobs);
            }
        }
        _ => {}
    }
}

/// The DREAM chain of §5.7: expand (skippable) → image → video, every input
/// after the first spliced from the stage before it.
fn dream_stages() -> Vec<PipelineStageSpec> {
    vec![
        PipelineStageSpec::new(
            "expand",
            "text.expand",
            obj(vec![("prompt", s("80s new wave about leaving the city"))]),
        )
        .on_fail_skip(),
        PipelineStageSpec::new(
            "image",
            "image.generate",
            obj(vec![("prompt", stage_ref("expand", "text"))]),
        ),
        PipelineStageSpec::new(
            "video",
            "video.generate",
            obj(vec![
                ("prompt", stage_ref("expand", "text")),
                (
                    "inputs",
                    Value::Arr(vec![obj(vec![
                        ("name", s("last_frame")),
                        ("source_revision", stage_ref("image", "revision")),
                    ])]),
                ),
            ]),
        )
        .with_deps(["expand", "image"]),
    ]
}

fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

// ---------------------------------------------------------------------------
// the round trip
// ---------------------------------------------------------------------------

/// One request declares the whole run; from that instant it is visible,
/// inspectable and cancellable, and the bar is the same number on both sides.
#[test]
fn a_declared_run_is_created_inspected_and_cancelled() {
    let fx = PipeFixture::start();
    let api = fx.api();
    let created = api
        .create_pipeline(
            "gen",
            "DREAM",
            "80s new wave about leaving the city",
            &dream_stages(),
        )
        .expect("create");

    // --- what went ON THE WIRE, key by key.
    let sent = fx.create_body();
    assert_eq!(field(&sent, "namespace"), "gen");
    assert_eq!(field(&sent, "title"), "DREAM");
    assert_eq!(field(&sent, "prompt"), "80s new wave about leaving the city");
    let stages = sent.get("stages").and_then(Value::as_arr).expect("stages").to_vec();
    assert_eq!(stages.len(), 3);
    assert_eq!(field(&stages[0], "name"), "expand");
    assert_eq!(field(&stages[0], "kind"), "text.expand");
    assert_eq!(field(&stages[0], "on_fail"), "skip");
    // Undeclared weights come from the ONE table.
    assert_eq!(stages[0].get("weight").and_then(Value::as_u64), Some(5));
    assert_eq!(stages[1].get("weight").and_then(Value::as_u64), Some(15));
    assert_eq!(stages[2].get("weight").and_then(Value::as_u64), Some(70));
    assert_eq!(field(&stages[1], "on_fail"), "fail");
    // A stage that declares no deps sends none: the server's default (the
    // stage before it) is the shape this client relies on.
    assert!(stages[1].get("deps").is_none());
    assert_eq!(
        stages[2].get("deps").and_then(Value::as_arr).map(|d| d.to_vec()),
        Some(vec![s("expand"), s("image")])
    );
    // The author-friendly reference travels as written; the server rewrites.
    let image_prompt = stages[1].get("body").and_then(|b| b.get("prompt")).cloned().unwrap();
    assert_eq!(field(&image_prompt, "$from_stage"), "expand");
    assert_eq!(field(&image_prompt, "field"), "text");

    // --- the answer names one job per declared stage, in order.
    assert_eq!(created.pipeline, PipelineId::parse(PIPELINE).unwrap());
    let names: Vec<&str> = created.stages.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["expand", "image", "video"]);
    let image_job = created.job_of("image").expect("image job");

    // --- t=0: nothing has run, everything is already inspectable.
    let at_spawn = api.pipeline_detail(&created.pipeline).expect("detail");
    assert_eq!(at_spawn.state, PipelineStateDto::Running);
    assert_eq!(at_spawn.permille, 0);
    assert_eq!(at_spawn.title, "DREAM");
    assert_eq!(at_spawn.prompt, "80s new wave about leaving the city");
    assert_eq!(at_spawn.current_stage.as_deref(), Some("expand"));
    assert_eq!(at_spawn.stages.len(), 3);
    for (seq, stage) in at_spawn.stages.iter().enumerate() {
        assert_eq!(stage.seq as usize, seq);
        assert_eq!(stage.state, JobStateDto::Pending);
        assert_eq!(stage.attempts, 0);
        assert!(stage.records.is_empty());
        assert!(stage.result.is_none());
        assert!(stage.declared.is_some(), "a pending stage shows what it WILL send");
        assert_eq!(stage.done_permille(), 0);
    }
    assert_eq!(at_spawn.stage("expand").unwrap().on_fail, StageOnFailDto::Skip);
    assert_eq!(at_spawn.stage("image").unwrap().job, image_job);
    // The declared body of a pending stage carries the rewritten reference —
    // this is what "inspectable before dispatch" means.
    let declared = at_spawn.stage("image").unwrap().declared.clone().unwrap();
    let prompt_ref = declared.get("prompt").cloned().unwrap();
    assert_eq!(field(&prompt_ref, "$from"), "job_00000000000000000000000000000001");
    assert_eq!(field(&prompt_ref, "field"), "text");
    assert_eq!(at_spawn.aggregate_permille(), at_spawn.permille);

    // --- the expander finishes, the image stage is halfway.
    fx.set("expand", |st| {
        st.state = "succeeded".into();
        st.attempts = 1;
        st.progress = Some((1000, "done".into()));
        st.records = vec![obj(vec![
            ("name", s("text.expand")),
            ("recorded_ms", Value::Int(NOW as i64 + 500)),
            (
                "record",
                obj(vec![
                    ("model", s("qwen3-8b")),
                    ("at", s(".165")),
                    ("prompt", s("expand this:\n80s new wave")),
                ]),
            ),
        ])];
        st.result = Some(obj(vec![
            ("outcome", s("succeeded")),
            ("attempt", Value::Int(1)),
            ("recorded_ms", Value::Int(NOW as i64 + 600)),
            ("body", obj(vec![("text", s("a rain-slick highway at dusk"))])),
        ]));
    });
    fx.set("image", |st| {
        st.state = "running".into();
        st.attempts = 1;
        st.progress = Some((500, "@.166 queued behind 1 run".into()));
    });

    let mid = api.pipeline_detail(&created.pipeline).expect("detail");
    // 5×1000 + 15×500 = 12500 over 90 = 138‰.
    assert_eq!(mid.permille, 138);
    assert_eq!(mid.aggregate_permille(), mid.permille);
    assert_eq!(mid.percent(), 13);
    assert_eq!(mid.current_stage.as_deref(), Some("image"));
    assert_eq!(mid.current().map(|c| c.kind.as_str()), Some("image.generate"));
    let expand = mid.stage("expand").unwrap();
    assert_eq!(expand.state, JobStateDto::Succeeded);
    assert!(!expand.skipped);
    assert_eq!(expand.done_permille(), 1000);
    assert_eq!(expand.records.len(), 1);
    assert_eq!(expand.records[0].model, "qwen3-8b");
    assert_eq!(expand.records[0].at, ".165");
    // The exact text the model was handed survives its line breaks.
    assert_eq!(expand.records[0].prompt, "expand this:\n80s new wave");
    let result = expand.result.as_ref().expect("result");
    assert_eq!(result.outcome, "succeeded");
    assert_eq!(field(&result.body, "text"), "a rain-slick highway at dusk");
    let image = mid.stage("image").unwrap();
    assert_eq!(image.progress.as_ref().map(|p| p.permille), Some(500));
    assert_eq!(
        image.progress.as_ref().map(|p| p.note.as_str()),
        Some("@.166 queued behind 1 run")
    );

    // --- the listing draws the same run, in one request.
    let rows = api.list_pipelines(Some("gen"), true, 50).expect("list");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.pipeline, created.pipeline);
    assert_eq!(row.title, "DREAM");
    assert_eq!(row.state, PipelineStateDto::Running);
    assert_eq!(row.permille, 138);
    assert_eq!(row.stages, 3);
    assert_eq!(row.current_stage.as_deref(), Some("image"));
    assert_eq!(row.note, "@.166 queued behind 1 run");
    assert_eq!(row.prompt.as_deref(), Some("80s new wave about leaving the city"));
    assert!(row.finished_ms.is_none());

    // --- cancel from anywhere; what finished is kept.
    let cancel = api.cancel_pipeline(&created.pipeline).expect("cancel");
    assert_eq!(cancel.pipeline, created.pipeline);
    assert_eq!(cancel.cancelled, 2, "the running stage and the pending one");
    assert_eq!(cancel.state, PipelineStateDto::Cancelled);
    let after = api.pipeline_detail(&created.pipeline).expect("detail");
    assert_eq!(after.state, PipelineStateDto::Cancelled);
    assert_eq!(after.stage("expand").unwrap().state, JobStateDto::Succeeded);
    assert!(after.stage("expand").unwrap().result.is_some(), "partial results are KEPT");
    assert_eq!(after.stage("image").unwrap().state, JobStateDto::Cancelled);
    // The bar freezes where the work stopped, and the two sides agree.
    assert_eq!(after.permille, 138);
    assert_eq!(after.aggregate_permille(), after.permille);
    assert!(after.finished_ms.is_some());
    // An active-only listing drops a run that is no longer moving.
    assert!(api.list_pipelines(Some("gen"), true, 50).unwrap().is_empty());
    assert_eq!(api.list_pipelines(None, false, 50).unwrap().len(), 1);
}

/// A skipped expander keeps the run alive: the stage reads failed+skipped,
/// contributes its whole weight, and the pipeline is still running.
#[test]
fn a_skipped_stage_keeps_the_run_alive_and_the_bar_honest() {
    let fx = PipeFixture::start();
    let api = fx.api();
    let created = api
        .create_pipeline("gen", "DREAM", "a city at night", &dream_stages())
        .expect("create");
    fx.set("expand", |st| {
        st.state = "failed".into();
        st.attempts = 1;
        st.progress = Some((200, "the model refused".into()));
    });
    let detail = api.pipeline_detail(&created.pipeline).expect("detail");
    let expand = detail.stage("expand").unwrap();
    assert_eq!(expand.state, JobStateDto::Failed);
    assert!(expand.skipped, "failed + on_fail:skip IS skipped");
    assert_eq!(expand.done_permille(), 1000);
    assert_eq!(detail.state, PipelineStateDto::Running);
    assert_eq!(detail.permille, 55, "5/90 of the run is behind it");
    assert_eq!(detail.aggregate_permille(), detail.permille);

    // A stage that fails WITHOUT skip fails the whole run, at that stage,
    // while its dependents are still formally pending.
    fx.set("image", |st| {
        st.state = "failed".into();
        st.progress = Some((732, "failed at publishing".into()));
    });
    let detail = api.pipeline_detail(&created.pipeline).expect("detail");
    assert_eq!(detail.state, PipelineStateDto::Failed);
    assert_eq!(detail.current_stage.as_deref(), Some("image"));
    assert_eq!(detail.stage("video").unwrap().state, JobStateDto::Pending);
    // 5×1000 + 15×732 = 15980 over 90 = 177‰, frozen where it stopped.
    assert_eq!(detail.permille, 177);
    assert_eq!(detail.aggregate_permille(), detail.permille);
}

/// The weights table is ONE table, and a declared weight always wins.
#[test]
fn the_default_weights_are_one_shared_table() {
    assert_eq!(default_stage_weight("text.expand"), 5);
    assert_eq!(default_stage_weight("image.generate"), 15);
    assert_eq!(default_stage_weight("image.upscale"), 25);
    assert_eq!(default_stage_weight("video.generate"), 70);
    assert_eq!(default_stage_weight("video.enhance"), 25);
    assert_eq!(default_stage_weight("music.generate"), 60);
    assert_eq!(default_stage_weight("mesh.generate"), 40);
    // Anything else weighs what the server would have given it.
    assert_eq!(default_stage_weight("annotate.asset"), NEUTRAL_STAGE_WEIGHT);
    assert_eq!(default_stage_weight(""), NEUTRAL_STAGE_WEIGHT);
    assert_eq!(NEUTRAL_STAGE_WEIGHT, 10);
    for (kind, weight) in DEFAULT_STAGE_WEIGHTS {
        assert!((1..=1000).contains(weight), "{kind} weight is declarable");
        assert_eq!(default_stage_weight(kind), *weight);
    }
    let spec = PipelineStageSpec::new("video", "video.generate", obj(vec![]));
    assert_eq!(spec.weight(), 70);
    assert_eq!(spec.clone().with_weight(3).weight(), 3);
}

/// Everything a pipeline cannot be is refused HERE, before a socket opens.
#[test]
fn a_run_that_cannot_be_declared_never_becomes_a_request() {
    let fx = PipeFixture::start();
    let api = fx.api();
    let one = |body: Value| vec![PipelineStageSpec::new("a", "image.generate", body)];
    let ok_body = || obj(vec![("prompt", s("x"))]);

    let bad: Vec<(&str, ClientError)> = vec![
        (
            "empty namespace",
            api.create_pipeline("", "T", "p", &one(ok_body())).unwrap_err(),
        ),
        (
            "empty title",
            api.create_pipeline("gen", "", "p", &one(ok_body())).unwrap_err(),
        ),
        (
            "control character in title",
            api.create_pipeline("gen", "D\u{7}M", "p", &one(ok_body())).unwrap_err(),
        ),
        (
            "over-long title",
            api.create_pipeline("gen", &"t".repeat(201), "p", &one(ok_body())).unwrap_err(),
        ),
        (
            "over-long prompt",
            api.create_pipeline("gen", "T", &"p".repeat(4001), &one(ok_body())).unwrap_err(),
        ),
        ("no stages", api.create_pipeline("gen", "T", "p", &[]).unwrap_err()),
        (
            "body that is not an object",
            api.create_pipeline("gen", "T", "p", &one(s("just text"))).unwrap_err(),
        ),
    ];
    for (what, err) in bad {
        assert!(matches!(err, ClientError::InvalidInput { .. }), "{what}: {err:?}");
    }

    // Nine stages: one past the graph's depth budget.
    let many: Vec<PipelineStageSpec> = (0..9)
        .map(|i| PipelineStageSpec::new(format!("s{i}"), "image.generate", ok_body()))
        .collect();
    assert!(matches!(
        api.create_pipeline("gen", "T", "p", &many).unwrap_err(),
        ClientError::InvalidInput { what: "too many pipeline stages" }
    ));

    // A duplicate stage name would make a reference ambiguous.
    let dup = vec![
        PipelineStageSpec::new("a", "image.generate", ok_body()),
        PipelineStageSpec::new("a", "image.generate", ok_body()),
    ];
    assert!(matches!(
        api.create_pipeline("gen", "T", "p", &dup).unwrap_err(),
        ClientError::InvalidInput { what: "duplicate stage name" }
    ));

    // Deps are backward-only, so a cycle is unrepresentable.
    let forward = vec![
        PipelineStageSpec::new("a", "image.generate", ok_body()).with_deps(["b"]),
        PipelineStageSpec::new("b", "image.generate", ok_body()),
    ];
    assert!(matches!(
        api.create_pipeline("gen", "T", "p", &forward).unwrap_err(),
        ClientError::InvalidInput { what: "stage dep names no earlier stage" }
    ));

    // A reference may only read a result this stage waited for.
    let stray = vec![
        PipelineStageSpec::new("a", "text.expand", ok_body()),
        PipelineStageSpec::new("b", "image.generate", ok_body()),
        PipelineStageSpec::new(
            "c",
            "video.generate",
            obj(vec![("prompt", stage_ref("a", "text"))]),
        ),
    ];
    assert!(matches!(
        api.create_pipeline("gen", "T", "p", &stray).unwrap_err(),
        ClientError::InvalidInput { what: "stage reference names no dependency" }
    ));
    // …and the same declaration is fine once it says it waits for that stage.
    let mut fixed = stray;
    fixed[2] = fixed[2].clone().with_deps(["a", "b"]);
    assert!(api.create_pipeline("gen", "T", "p", &fixed).is_ok());

    // `on_fail: skip` with nothing to fall back to would splice an empty
    // prompt into the rest of the run.
    let skip = vec![PipelineStageSpec::new("a", "text.expand", ok_body()).on_fail_skip()];
    assert!(matches!(
        api.create_pipeline("gen", "T", "", &skip).unwrap_err(),
        ClientError::InvalidInput { what: "on_fail skip needs a prompt" }
    ));

    // Names, kinds and weights are the job contract's, checked locally.
    for spec in [
        PipelineStageSpec::new("Bad Name", "image.generate", ok_body()),
        PipelineStageSpec::new("a", "IMAGE.GENERATE", ok_body()),
        PipelineStageSpec::new("a", "image.generate", ok_body()).with_weight(0),
        PipelineStageSpec::new("a", "image.generate", ok_body()).with_weight(1001),
        PipelineStageSpec::new("a", "image.generate", ok_body()).with_max_attempts(0),
    ] {
        assert!(spec.to_value().is_err(), "{} / {}", spec.name, spec.kind);
        assert!(api.create_pipeline("gen", "T", "p", &[spec]).is_err());
    }

    // Listing limits are the server's, refused before the round trip.
    assert!(api.list_pipelines(None, true, 0).is_err());
    assert!(api.list_pipelines(None, true, 201).is_err());
    assert!(api.list_pipelines(Some("bad ns"), true, 10).is_err());

    // Exactly one of those declarations was legal, so exactly one request
    // was ever made.
    assert_eq!(fx.requests(), 1, "local refusals never reach the network");
}

/// A pipeline document this client cannot fully believe is refused whole,
/// never half-rendered.
#[test]
fn a_hostile_pipeline_document_is_refused() {
    let pipeline = PipelineId::parse(PIPELINE).unwrap();
    let stage = |over: Vec<(&str, Value)>| {
        let mut pairs = vec![
            ("name", s("expand")),
            ("seq", Value::Int(0)),
            ("job", s("job_00000000000000000000000000000001")),
            ("kind", s("text.expand")),
            ("state", s("running")),
            ("skipped", Value::Bool(false)),
            ("weight", Value::Int(5)),
            ("on_fail", s("fail")),
            ("attempts", Value::Int(1)),
        ];
        for (k, v) in over {
            match pairs.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => pairs.push((k, v)),
            }
        }
        obj(pairs)
    };
    let detail = |over: Vec<(&str, Value)>, stages: Vec<Value>| {
        let mut pairs = vec![
            ("pipeline", s(PIPELINE)),
            ("namespace", s("gen")),
            ("title", s("DREAM")),
            ("state", s("running")),
            ("permille", Value::Int(500)),
            ("created_ms", Value::Int(NOW as i64)),
        ];
        for (k, v) in over {
            match pairs.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => pairs.push((k, v)),
            }
        }
        pairs.push(("stages", Value::Arr(stages)));
        obj(pairs)
    };

    let cases: Vec<(&str, Value)> = vec![
        ("a bar over 1000", detail(vec![("permille", Value::Int(1001))], vec![stage(vec![])])),
        (
            "a state word this build cannot interpret",
            detail(vec![("state", s("stalled"))], vec![stage(vec![])]),
        ),
        ("no stages at all", detail(vec![], vec![])),
        (
            "a stage state this build cannot interpret",
            detail(vec![], vec![stage(vec![("state", s("paused"))])]),
        ),
        (
            "an on_fail policy this build cannot interpret",
            detail(vec![], vec![stage(vec![("on_fail", s("retry"))])]),
        ),
        (
            "stages out of declaration order",
            detail(
                vec![],
                vec![stage(vec![("seq", Value::Int(3))]), stage(vec![("seq", Value::Int(1))])],
            ),
        ),
        (
            "a weightless stage",
            detail(vec![], vec![stage(vec![("weight", Value::Int(0))])]),
        ),
        (
            "an over-weighted stage",
            detail(vec![], vec![stage(vec![("weight", Value::Int(1001))])]),
        ),
        (
            "a malformed stage job id",
            detail(vec![], vec![stage(vec![("job", s("job_nope"))])]),
        ),
        (
            "a declared body that is not a body",
            detail(vec![], vec![stage(vec![("declared", s("a string"))])]),
        ),
        (
            "a stage progress bar over 1000",
            detail(
                vec![],
                vec![stage(vec![(
                    "progress",
                    obj(vec![("permille", Value::Int(2000)), ("note", s("x"))]),
                )])],
            ),
        ),
        (
            "someone else's pipeline",
            detail(
                vec![("pipeline", s("pipe_ffffffffffffffffffffffffffffffff"))],
                vec![stage(vec![])],
            ),
        ),
    ];
    for (what, body) in cases {
        let server = canned(200, body);
        let err = api_at(server.addr).pipeline_detail(&pipeline).unwrap_err();
        assert!(matches!(err, ClientError::Protocol { .. }), "{what}: {err:?}");
    }

    // The cancel answer must be about the run that was cancelled.
    let server = canned(
        200,
        obj(vec![
            ("pipeline", s("pipe_ffffffffffffffffffffffffffffffff")),
            ("cancelled", Value::Int(2)),
            ("state", s("cancelled")),
        ]),
    );
    assert!(matches!(
        api_at(server.addr).cancel_pipeline(&pipeline).unwrap_err(),
        ClientError::Protocol { what: "pipeline cancel id mismatch" }
    ));

    // A create answer must name every declared stage, in order.
    let server = canned(
        201,
        obj(vec![
            ("pipeline", s(PIPELINE)),
            (
                "stages",
                Value::Arr(vec![obj(vec![
                    ("name", s("elsewhere")),
                    ("job", s("job_00000000000000000000000000000001")),
                ])]),
            ),
        ]),
    );
    let one = vec![PipelineStageSpec::new("a", "image.generate", obj(vec![]))];
    assert!(matches!(
        api_at(server.addr).create_pipeline("gen", "T", "p", &one).unwrap_err(),
        ClientError::Protocol { what: "created pipeline stage mismatch" }
    ));

    // A row page with a state this build cannot interpret refuses whole.
    let server = canned(
        200,
        obj(vec![(
            "pipelines",
            Value::Arr(vec![obj(vec![
                ("pipeline", s(PIPELINE)),
                ("namespace", s("gen")),
                ("title", s("DREAM")),
                ("state", s("stalled")),
                ("permille", Value::Int(500)),
                ("stages", Value::Int(3)),
                ("created_ms", Value::Int(NOW as i64)),
            ])]),
        )]),
    );
    assert!(matches!(
        api_at(server.addr).list_pipelines(None, true, 50).unwrap_err(),
        ClientError::Protocol { .. }
    ));
}

/// Version skew is said out loud: a new client on an old server must never
/// quietly fall back to invisible client-side chaining.
#[test]
fn an_old_server_without_pipelines_says_so() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let server = RawServer::start(Arc::new(move |_req, stream: &mut TcpStream| {
        seen.fetch_add(1, Ordering::Relaxed);
        write_error(stream, 404, "no route");
    }));
    let api = api_at(server.addr);
    let one = vec![PipelineStageSpec::new("a", "image.generate", obj(vec![]))];
    assert_eq!(
        api.create_pipeline("gen", "T", "p", &one).unwrap_err(),
        ClientError::NotFound { what: "pipeline routes" }
    );
    assert_eq!(
        api.list_pipelines(None, true, 50).unwrap_err(),
        ClientError::NotFound { what: "pipeline routes" }
    );
    // An addressed route's 404 is the ordinary "no such run, or not yours".
    assert_eq!(
        api.pipeline_detail(&PipelineId::parse(PIPELINE).unwrap()).unwrap_err(),
        ClientError::NotFound { what: "pipeline" }
    );
    assert_eq!(
        api.cancel_pipeline(&PipelineId::parse(PIPELINE).unwrap()).unwrap_err(),
        ClientError::NotFound { what: "pipeline" }
    );
    assert_eq!(hits.load(Ordering::Relaxed), 4);
}

/// The id spelling is the transport's, and nothing else parses as one.
#[test]
fn a_pipeline_id_is_exactly_its_transport_spelling() {
    let id = PipelineId::parse(PIPELINE).expect("id");
    assert_eq!(id.to_string(), PIPELINE);
    for bad in [
        "pipe_0123456789ABCDEF0123456789abcdef", // uppercase hex
        "pipe_0123456789abcdef0123456789abcde",  // short
        "pipe_0123456789abcdef0123456789abcdef0", // long
        "job_0123456789abcdef0123456789abcdef",  // a job is not a pipeline
        "0123456789abcdef0123456789abcdef",      // unprefixed
        "",
    ] {
        assert!(PipelineId::parse(bad).is_none(), "{bad}");
    }
}

/// The request path is the route the server actually serves.
#[test]
fn the_listing_asks_for_exactly_what_it_wants() {
    let fx = PipeFixture::start();
    let api = fx.api();
    api.list_pipelines(Some("gen"), true, 25).expect("list");
    api.list_pipelines(None, false, 200).expect("list");
    let st = fx.state.lock().unwrap();
    assert_eq!(st.seen[0].target, "/v1/pipelines?limit=25&ns=gen&state=active");
    assert_eq!(st.seen[1].target, "/v1/pipelines?limit=200&state=all");
}

/// A pipeline read is one request, and it carries everything a card needs.
#[test]
fn one_request_draws_the_whole_card() {
    let fx = PipeFixture::start();
    let api = fx.api();
    let created = api
        .create_pipeline("gen", "DREAM", "a city at night", &dream_stages())
        .expect("create");
    let before = fx.requests();
    let detail = api.pipeline_detail(&created.pipeline).expect("detail");
    assert_eq!(fx.requests(), before + 1, "one request per card, not one per stage");
    assert_eq!(detail.stages.len(), 3);
    assert!(detail.stages.iter().all(|st| !st.name.is_empty() && !st.kind.is_empty()));
}
