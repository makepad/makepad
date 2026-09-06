//! Session engine tests with a scripted provider and a deterministic tool
//! executor. These mocks exist ONLY here: the engine under test cannot
//! construct a provider itself (no fallback is even expressible — it holds
//! one `Box<dyn ChatProvider>` for its whole life).

use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_asset_chat::session::{CancelFlag, ExecCtx, SendRefusal, Session, ToolExecutor};
use makepad_asset_chat::tools::ContentToolCall;
use makepad_asset_chat::wire::{
    AttachmentBinding, ChatEventBody, ProviderAvailability, ProviderKind, ServingFacts,
    ToolOutcome, MAX_DELTA_BYTES, MAX_MESSAGE_BYTES, MAX_PROGRESS_EVENTS, MAX_TOOL_JSON_BYTES,
    MAX_TOOL_ROUNDS,
};
use makepad_asset_client::json::{self, Value};
use makepad_asset_data::AssetRevisionId;
use std::cell::RefCell;
use std::rc::Rc;

fn rev(byte: u8) -> AssetRevisionId {
    AssetRevisionId::from_bytes([byte; 32])
}

/// Scripted provider: each `begin_turn` shifts the next event script;
/// records every turn input for assertions.
struct Scripted {
    kind: ProviderKind,
    available: ProviderAvailability,
    scripts: Vec<Vec<ProviderEvent>>,
    pending: Vec<ProviderEvent>,
    pub turns: Rc<RefCell<Vec<TurnInput>>>,
    pub continuations: Rc<RefCell<Vec<(String, String)>>>,
    cancelled: Rc<RefCell<u32>>,
    begin_fails: u32,
}

impl Scripted {
    fn new(scripts: Vec<Vec<ProviderEvent>>) -> Scripted {
        Scripted {
            kind: ProviderKind::FleetQwen,
            available: ProviderAvailability::Available {
                model: "scripted".into(),
                detail: "test".into(),
            },
            scripts,
            pending: Vec::new(),
            turns: Rc::new(RefCell::new(Vec::new())),
            continuations: Rc::new(RefCell::new(Vec::new())),
            cancelled: Rc::new(RefCell::new(0)),
            begin_fails: 0,
        }
    }
}

impl ChatProvider for Scripted {
    fn kind(&self) -> ProviderKind {
        self.kind
    }
    fn availability(&mut self) -> ProviderAvailability {
        self.available.clone()
    }
    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        self.turns.borrow_mut().push(input.clone());
        if self.begin_fails > 0 {
            self.begin_fails -= 1;
            return Err("provider start failed".to_string());
        }
        if self.scripts.is_empty() {
            return Err("script exhausted".to_string());
        }
        self.pending = self.scripts.remove(0);
        Ok(())
    }
    fn poll(&mut self) -> Vec<ProviderEvent> {
        std::mem::take(&mut self.pending)
    }
    fn cancel(&mut self) {
        *self.cancelled.borrow_mut() += 1;
        self.pending.clear();
    }
    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        self.continuations.borrow_mut().push((call_id.to_string(), output.to_string()));
        if self.scripts.is_empty() {
            return Err("script exhausted".to_string());
        }
        self.pending = self.scripts.remove(0);
        Ok(())
    }
}

/// Deterministic executor: programmed outcome per tool name, records calls.
struct Recorder {
    outcome: ToolOutcome,
    calls: Rc<RefCell<Vec<ContentToolCall>>>,
    seen_known: Rc<RefCell<Vec<std::collections::HashSet<AssetRevisionId>>>>,
    progress_ticks: u16,
}

impl Recorder {
    fn new(outcome: ToolOutcome) -> Recorder {
        Recorder {
            outcome,
            calls: Rc::new(RefCell::new(Vec::new())),
            seen_known: Rc::new(RefCell::new(Vec::new())),
            progress_ticks: 0,
        }
    }
}

impl ToolExecutor for Recorder {
    fn capability_doc(&mut self) -> String {
        "Registered operations (test): mesh.from_image.v1".to_string()
    }
    fn execute(
        &mut self,
        call: &ContentToolCall,
        ctx: &ExecCtx,
        progress: &mut dyn FnMut(u16, &str),
        _cancel: &CancelFlag,
    ) -> ToolOutcome {
        self.calls.borrow_mut().push(call.clone());
        self.seen_known.borrow_mut().push(ctx.known.clone());
        for i in 0..self.progress_ticks {
            progress(((i + 1) as u32 * 1000 / self.progress_ticks.max(1) as u32) as u16, "working");
        }
        self.outcome.clone()
    }
}

fn tool_line(name: &str, args: Value) -> String {
    format!(
        "<<tool>>{}",
        json::obj(vec![("name", json::s(name)), ("args", args)]).to_json()
    )
}

struct RoundLimited {
    recorder: Recorder,
    limit: u32,
    client: bool,
}
impl ToolExecutor for RoundLimited {
    fn capability_doc(&mut self) -> String { self.recorder.capability_doc() }
    fn max_tool_rounds(&self) -> u32 { self.limit }
    fn client_executes(&mut self, _: &ContentToolCall) -> bool { self.client }
    fn execute(&mut self, call: &ContentToolCall, ctx: &ExecCtx, progress: &mut dyn FnMut(u16, &str), cancel: &CancelFlag) -> ToolOutcome {
        self.recorder.execute(call, ctx, progress, cancel)
    }
}
fn scripted_call(kind: ProviderKind, index: usize, name: &str, args: Value) -> Vec<ProviderEvent> {
    if kind.uses_native_tools() {
        vec![ProviderEvent::FunctionCall { call_id: format!("call_{index}"), name: name.replace('.', "_"), arguments: args.to_json() }]
    } else {
        vec![ProviderEvent::Done { text: tool_line(name, args) }]
    }
}

/// Real Session continuation with binary tool outputs. Scripted deliberately
/// retains ChatProvider's default image refusal, on both textual/native lanes.
struct ImageRecorder {
    recorder: Recorder,
    images: Vec<makepad_asset_chat::provider::ToolImage>,
}
impl ToolExecutor for ImageRecorder {
    fn capability_doc(&mut self) -> String { self.recorder.capability_doc() }
    fn tool_definitions(&mut self) -> Vec<makepad_asset_chat::tools::ToolDef> {
        makepad_asset_chat::tools::sandbox_definitions().into_iter()
            .filter(|definition| definition.name == "model.jobs").collect()
    }
    fn take_tool_images(&mut self) -> Vec<makepad_asset_chat::provider::ToolImage> {
        std::mem::take(&mut self.images)
    }
    fn execute(&mut self, call: &ContentToolCall, ctx: &ExecCtx, progress: &mut dyn FnMut(u16, &str), cancel: &CancelFlag) -> ToolOutcome {
        self.recorder.execute(call, ctx, progress, cancel)
    }
}

fn review_result() -> Value {
    json::obj(vec![
        ("state", json::s("reviewed")),
        ("job", json::s("review_car")),
        ("result", json::obj(vec![
            ("document", json::s("car")),
            ("checks", json::obj(vec![("triangles", Value::Int(36)), ("manifold", Value::Bool(true)),
                ("self_intersections", Value::Int(0))])),
            ("images", Value::Arr(vec![json::obj(vec![("id", json::s("review_front")), ("view", json::s("front"))])])),
        ])),
    ])
}

fn assert_retained_review(outcome: &ToolOutcome) {
    let ToolOutcome::Ok { value } = outcome else { panic!("successful checks were replaced: {outcome:?}"); };
    let original = review_result();
    assert_eq!(value.get("state"), original.get("state"));
    assert_eq!(value.get("job"), original.get("job"));
    let nested = value.get("result").unwrap();
    for key in ["document", "checks", "images"] {
        assert_eq!(nested.get(key), original.get("result").unwrap().get(key), "lost {key}");
    }
    for scope in [value, nested] {
        assert_eq!(scope.get("images_delivered"), Some(&Value::Bool(false)));
        assert!(scope.get("image_delivery_error").and_then(Value::as_str).is_some_and(|reason| !reason.is_empty()));
    }
}

#[test]
fn refused_review_images_preserve_checks_in_the_actual_session_continuation() {
    use makepad_asset_chat::wire::ChatRole;
    for kind in [ProviderKind::FleetQwen, ProviderKind::OpenAi] {
        let args = json::obj(vec![("job", json::s("review_car")), ("wait_ms", Value::Int(0))]);
        let mut provider = Scripted::new(vec![
            scripted_call(kind, 0, "model.jobs", args),
            vec![ProviderEvent::Done { text: "The structural checks are available; the images were not delivered.".into() }],
        ]);
        provider.kind = kind;
        let turns = provider.turns.clone();
        let continuations = provider.continuations.clone();
        let mut exec = ImageRecorder {
            recorder: Recorder::new(ToolOutcome::Ok { value: review_result() }),
            // Default refusal depends only on a nonempty image vector; this
            // test exercises delivery, not a provider's PNG decoding.
            images: vec![makepad_asset_chat::provider::ToolImage { label: "front".into(),
                png: std::sync::Arc::from(&b"binary-review-image-fixture"[..]) }],
        };
        let mut session = Session::new("review-image-refusal", Box::new(provider));
        session.send("check the model geometry and rendered front", &[], &mut exec).unwrap();
        session.pump(&mut exec);
        session.pump(&mut exec);
        assert!(session.is_idle());
        assert_eq!(exec.recorder.calls.borrow().len(), 1);
        assert_eq!(exec.recorder.calls.borrow()[0].name(), "model.jobs");
        assert!(exec.images.is_empty());
        let events = session.drain_events();
        let results = events.iter().filter_map(|event| match &event.body {
            ChatEventBody::ToolResult { outcome, .. } => Some(outcome),
            ChatEventBody::Error { code, message } => panic!("unexpected {kind:?} error {code}: {message}"),
            _ => None,
        }).collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        assert_retained_review(results[0]);
        let output = if kind.uses_native_tools() {
            assert_eq!(continuations.borrow().len(), 1);
            assert_eq!(continuations.borrow()[0].0, "call_0");
            continuations.borrow()[0].1.clone()
        } else {
            assert_eq!(turns.borrow().len(), 2);
            turns.borrow()[1].messages.iter().find(|message| message.role == ChatRole::Tool).unwrap().text.clone()
        };
        assert!(!output.contains("binary-review-image-fixture"), "pixels must stay outside JSON");
        let value = json::parse(output.as_bytes()).unwrap();
        let continued = ToolOutcome::decode(&value).unwrap();
        assert_retained_review(&continued);
        assert_eq!(&continued, results[0]);
        let ToolOutcome::Ok { value } = continued else { unreachable!() };
        assert_eq!(value.get("image_delivery_error").and_then(Value::as_str),
            Some("this chat provider cannot receive rendered tool images"));
    }
}

#[test]
fn repeated_image_delivery_annotations_are_bounded_and_keep_nested_checks() {
    let mut outcome = ToolOutcome::Ok { value: review_result() };
    let reason = "🧶".repeat(1_000);
    outcome = outcome.image_delivery_failed(&reason);
    let once = outcome.encode().to_json();
    for _ in 0..20 { outcome = outcome.image_delivery_failed(&reason); }
    assert_eq!(outcome.encode().to_json(), once, "repeated failures must replace their annotation");
    assert_retained_review(&outcome);
    assert!(once.len() < MAX_TOOL_JSON_BYTES);
    assert!(outcome.validate().is_ok());
    let ToolOutcome::Ok { value } = &outcome else { unreachable!() };
    for scope in [value, value.get("result").unwrap()] {
        let Value::Obj(fields) = scope else { panic!("review object"); };
        for key in ["images_delivered", "image_delivery_error"] {
            assert_eq!(fields.iter().filter(|(name, _)| name == key).count(), 1);
        }
        assert!(scope.get("image_delivery_error").unwrap().as_str().unwrap().chars().count() <= 240);
    }
    let mut already_delivered = review_result();
    let Value::Obj(fields) = &mut already_delivered else { unreachable!() };
    fields.push(("images_delivered".into(), Value::Bool(true)));
    let Value::Obj(nested) = &mut fields.iter_mut().find(|(key, _)| key == "result").unwrap().1 else { unreachable!() };
    nested.push(("images_delivered".into(), Value::Bool(true)));
    assert_retained_review(&ToolOutcome::Ok { value: already_delivered }.image_delivery_failed("review image expired"));
    let failed = ToolOutcome::Failed { message: "geometry compilation failed".into() };
    assert_eq!(failed.clone().image_delivery_failed("no image"), failed,
        "an existing failed result must not be promoted to success");
}

// Match the current model review shape. Many object diagnostics legitimately
// approach the response bound; names remain within the 96-byte model name cap.
fn near_limit_review(headroom: usize, preview_metadata: bool, unknown_field: bool) -> Value {
    let make = |errors: Vec<Value>| {
        let head = json::obj(vec![("generation", json::s("1")), ("content", json::s("0".repeat(64)))]);
        let mut result = vec![
            ("document", json::s("car")), ("head", head.clone()),
            ("checks", json::obj(vec![("head", head), ("valid", Value::Bool(false)),
                ("closed_objects", Value::Int(120)), ("open_objects", Value::Int(0)), ("errors", Value::Arr(errors))])),
            ("motion_samples", Value::Arr(vec![json::s("steering left")])),
            ("motion_errors", Value::Arr(vec![json::obj(vec![("sample", json::s("steering right")),
                ("error", json::s("sample compilation failed"))])])),
            ("motion_scope", json::s("Sampled poses; not a continuous collision certificate.")),
            ("wheel_visual", Value::Arr(vec![])),
        ];
        if preview_metadata {
            result.extend([
                ("views", json::s("rest: front three-quarter,rear,underside,front; motion sheet follows motion_samples, row-major; same exact head, PBR renderer")),
                ("images_available", Value::Bool(true)),
            ]);
        }
        if unknown_field { result.push(("unrecognized_context", json::s("must preserve ".repeat(40)))); }
        json::obj(vec![("job", json::s("review_car")), ("state", json::s("reviewed")), ("result", json::obj(result))])
    };
    let mut errors = (0..120).map(|index| json::obj(vec![
        ("object", json::s(format!("part_{index:03}"))),
        ("orientation_errors", Value::Int(1)), ("intersections", Value::Int(0)),
    ])).collect::<Vec<_>>();
    let base = ToolOutcome::Ok { value: make(errors.clone()) }.encode().to_json().len();
    let target = MAX_TOOL_JSON_BYTES - headroom;
    let mut remaining = target.checked_sub(base).unwrap();
    for error in &mut errors {
        let Value::Obj(fields) = error else { unreachable!() };
        let Value::Str(name) = &mut fields.iter_mut().find(|(key, _)| key == "object").unwrap().1 else { unreachable!() };
        let amount = remaining.min(96 - name.len());
        name.push_str(&"x".repeat(amount)); remaining -= amount;
    }
    assert_eq!(remaining, 0, "fixture diagnostic names cannot fill the requested size");
    let value = make(errors);
    let outcome = ToolOutcome::Ok { value: value.clone() };
    assert_eq!(outcome.encode().to_json().len(), target);
    assert!(outcome.validate().is_ok(), "fixture must be admitted before annotation");
    value
}

#[test]
fn near_limit_review_continuation_compacts_only_declared_image_metadata() {
    use makepad_asset_chat::wire::ChatRole;
    for kind in [ProviderKind::FleetQwen, ProviderKind::OpenAi] {
        let original = near_limit_review(64, true, true);
        let args = json::obj(vec![("job", json::s("review_car")), ("wait_ms", Value::Int(0))]);
        let mut provider = Scripted::new(vec![scripted_call(kind, 0, "model.jobs", args),
            vec![ProviderEvent::Done { text: "The structural checks remain available; image delivery failed.".into() }]]);
        provider.kind = kind;
        let turns = provider.turns.clone(); let continuations = provider.continuations.clone();
        let mut exec = ImageRecorder { recorder: Recorder::new(ToolOutcome::Ok { value: original.clone() }),
            images: vec![makepad_asset_chat::provider::ToolImage { label: "front".into(),
                png: std::sync::Arc::from(&b"binary-review-image-fixture"[..]) }] };
        let mut session = Session::new("near-limit-review", Box::new(provider));
        session.send("review all model objects", &[], &mut exec).unwrap();
        session.pump(&mut exec); session.pump(&mut exec);
        assert!(session.is_idle());
        let output = if kind.uses_native_tools() { continuations.borrow()[0].1.clone() } else {
            turns.borrow()[1].messages.iter().find(|message| message.role == ChatRole::Tool).unwrap().text.clone()
        };
        assert!(output.len() <= MAX_TOOL_JSON_BYTES);
        let continued = ToolOutcome::decode(&json::parse(output.as_bytes()).unwrap()).unwrap();
        let ToolOutcome::Ok { value } = &continued else { panic!("lost review checks: {continued:?}"); };
        let nested = value.get("result").unwrap();
        for (key, prior) in match original.get("result").unwrap() { Value::Obj(fields) => fields, _ => unreachable!() } {
            if ["views", "images_available"].contains(&key.as_str()) { assert!(nested.get(key).is_none()); }
            else { assert_eq!(nested.get(key), Some(prior), "changed diagnostic or unknown field {key}"); }
        }
        assert_eq!(value.get("image_delivery_omitted_fields"), Some(&Value::Arr(vec![json::s("result.views"), json::s("result.images_available")])));
        for scope in [value, nested] { assert_eq!(scope.get("images_delivered"), Some(&Value::Bool(false))); }
        assert_eq!(continued.clone().image_delivery_failed("this chat provider cannot receive rendered tool images"), continued,
            "repeated delivery failure must preserve the bounded omission notice");
        let mut result_seen = false;
        for event in session.drain_events() { match event.body {
            ChatEventBody::ToolResult { outcome, .. } => { assert_eq!(outcome, continued); result_seen = true; },
            ChatEventBody::Error { code, message } => panic!("{code}: {message}"),
            _ => {},
        }}
        assert!(result_seen);
    }
}

#[test]
fn image_warning_shortens_new_reason_before_metadata_and_never_silently_cuts_checks() {
    let original = near_limit_review(256, true, true);
    let result = ToolOutcome::Ok { value: original.clone() }.image_delivery_failed(&"🧶".repeat(1_000));
    let ToolOutcome::Ok { value } = result else { panic!("short delivery reason should fit"); };
    assert!(value.get("image_delivery_omitted_fields").is_none());
    for (key, prior) in match original.get("result").unwrap() { Value::Obj(fields) => fields, _ => unreachable!() } {
        assert_eq!(value.get("result").unwrap().get(key), Some(prior));
    }
    assert_eq!(value.get("image_delivery_error").and_then(Value::as_str), Some("unavailable"));
    // Even an unfamiliar large field is not expendable. When the remaining
    // diagnostics plus the explicit warning cannot fit, fail visibly instead
    // of presenting an incomplete successful review.
    let result = ToolOutcome::Ok { value: near_limit_review(8, false, true) }.image_delivery_failed("image store expired");
    assert!(result.validate().is_ok());
    assert!(matches!(result, ToolOutcome::Failed { message } if message.contains("byte budget") && message.contains("No partial checks")));
}

#[test]
fn extended_modeling_rounds_reach_parked_publish_and_spawn_on_both_lanes() {
    for kind in [ProviderKind::CodexCli, ProviderKind::OpenAi] {
        let document = || json::obj(vec![("document", json::s("car"))]);
        let head = || json::obj(vec![("generation", json::s("0")), ("content", json::s("0".repeat(64)))]);
        let mut calls = ["model.workflow", "model.operations", "model.surface", "model.scene", "model.vehicle"]
            .into_iter().map(|topic| ("world.api", json::obj(vec![("query", json::s(topic))]))).collect::<Vec<_>>();
        calls.push(("model.open", document()));
        for index in 0..6 {
            calls.push(("model.apply", json::obj(vec![("document", json::s("car")), ("request_id", json::s(format!("part_{index}"))),
                ("expected", head()), ("operations", Value::Arr(vec![json::obj(vec![("op", json::s("cube")), ("object", json::s(format!("part_{index}"))), ("size", Value::Arr(vec![Value::Int(1); 3]))])]))])));
            calls.push(("model.jobs", json::obj(vec![("job", json::s(format!("edit_{index}"))), ("wait_ms", Value::Int(10_000))])));
        }
        calls.push(("model.publish", json::obj(vec![("document", json::s("car")), ("title", json::s("Original old car")),
            ("alias", json::s("gen/models/original-old-car")), ("expected", head()), ("expected_alias", json::s("absent"))])));
        calls.push(("model.jobs", json::obj(vec![("job", json::s("publish_car")), ("wait_ms", Value::Int(10_000))])));
        calls.push(("world.spawn", json::obj(vec![("model", json::s("gen/models/original-old-car")), ("form", json::s("car"))])));
        assert!(calls.len() > MAX_TOOL_ROUNDS as usize);
        let mut scripts = calls.iter().enumerate().map(|(i, (name, args))| scripted_call(kind, i, name, args.clone())).collect::<Vec<_>>();
        scripts.push(vec![ProviderEvent::Done { text: "The original car is placed.".into() }]);
        let mut provider = Scripted::new(scripts); provider.kind = kind;
        let mut exec = RoundLimited { recorder: Recorder::new(ToolOutcome::Ok { value: Value::Null }), limit: 48, client: true };
        let mut session = Session::new("modeling", Box::new(provider));
        session.send("model me an old car", &[], &mut exec).unwrap();
        assert_eq!(session.tool_round_limit(), 48);
        // Executor changes cannot enlarge/reduce an already admitted turn.
        exec.limit = 1;
        let mut observed = Vec::new();
        let mut published = false;
        let mut placed = false;
        for _ in 0..60 {
            session.pump(&mut exec);
            for event in session.drain_events() {
                if let ChatEventBody::ToolCall { id, name, args } = event.body {
                    assert_eq!(session.awaiting_client_tool(), Some(id.as_str()));
                    assert_eq!(name, calls[observed.len()].0);
                    let value = if name == "model.jobs" && args.get("job").and_then(Value::as_str) == Some("publish_car") {
                        published = true;
                        json::obj(vec![("state", json::s("published")), ("result", json::obj(vec![("alias", json::s("gen/models/original-old-car")), ("placeable_now", Value::Bool(true))]))])
                    } else if name == "world.spawn" {
                        assert!(published, "placement follows installed publication");
                        placed = true;
                        json::obj(vec![("committed", Value::Bool(true))])
                    } else { json::obj(vec![("ok", Value::Bool(true))]) };
                    observed.push(name);
                    session.provide_client_outcome(&id, ToolOutcome::Ok { value }, &mut exec).unwrap();
                } else if let ChatEventBody::Error { code, message } = event.body {
                    panic!("{kind:?}: {code}: {message}");
                }
            }
            if session.is_idle() { break; }
        }
        assert!(session.is_idle() && published && placed);
        assert_eq!(observed.len(), calls.len());
        assert_eq!(observed.last().map(String::as_str), Some("world.spawn"));
    }
}

#[test]
fn configurable_round_limit_is_finite_history_bounded_and_fail_closed() {
    use makepad_asset_chat::wire::{ChatMessage, ChatRole, MAX_MESSAGES, MAX_CONFIGURABLE_TOOL_ROUNDS};
    for kind in [ProviderKind::CodexCli, ProviderKind::OpenAi] {
        for (requested, old_rows) in [(48, 0), (u32::MAX, 0), (48, 64), (0, 0)] {
            let args = json::obj(vec![("document", json::s("car"))]);
            let scripts = (0..130).map(|i| scripted_call(kind, i, "model.inspect", args.clone())).collect();
            let mut provider = Scripted::new(scripts); provider.kind = kind;
            let history = (0..old_rows).map(|_| ChatMessage::new(ChatRole::User, "previous turn")).collect();
            let mut session = Session::resume(makepad_asset_chat::session::SessionId::generate(), "bounded", Box::new(provider), history, 1, None);
            let mut exec = RoundLimited { recorder: Recorder::new(ToolOutcome::Ok { value: json::obj(vec![]) }), limit: requested, client: false };
            session.send("inspect", &[], &mut exec).unwrap();
            let expected = requested.min(MAX_CONFIGURABLE_TOOL_ROUNDS).min(((MAX_MESSAGES - old_rows - 3) / 2) as u32);
            assert_eq!(session.tool_round_limit(), expected);
            let mut last = None;
            for _ in 0..130 {
                session.pump(&mut exec);
                for event in session.drain_events() { last = Some(event.body); }
                if session.is_idle() { break; }
            }
            assert!(session.is_idle());
            assert_eq!(exec.recorder.calls.borrow().len(), expected as usize);
            assert!(session.history().len() <= MAX_MESSAGES);
            if kind.uses_native_tools() || expected == 0 {
                assert!(matches!(last, Some(ChatEventBody::Error { code, .. }) if code == "tool_budget"));
            } else {
                assert!(matches!(last, Some(ChatEventBody::Done)));
            }
        }
    }
}

/// Serving facts ride out on the delta they describe — and on the LAST
/// chunk of a split, because they describe the END of that text.
#[test]
fn serving_facts_ride_on_the_delta_they_describe() {
    let facts = ServingFacts { gen_tokens: 64, lanes_active: Some(1), slots_total: Some(4), ..Default::default() };
    let big = "a".repeat(MAX_DELTA_BYTES + 16);
    let provider = Scripted::new(vec![vec![
        ProviderEvent::Delta("before".into()),
        ProviderEvent::Serving(facts),
        ProviderEvent::Delta(big.clone()),
        ProviderEvent::Done { text: format!("before{big}") },
    ]]);
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));
    session.send("hi", &[], &mut exec).unwrap();
    session.pump(&mut exec);

    let carried: Vec<Option<ServingFacts>> = session
        .drain_events()
        .iter()
        .filter_map(|e| match &e.body {
            ChatEventBody::Delta { serving, .. } => Some(*serving),
            _ => None,
        })
        .collect();
    assert_eq!(carried.len(), 3, "one delta, then a split one: {carried:?}");
    assert_eq!(carried[0], None, "facts that had not arrived yet are not invented");
    assert_eq!(carried[1], None, "the middle of a split says nothing");
    assert_eq!(carried[2], Some(facts));
}

#[test]
fn plain_turn_streams_and_completes_in_order() {
    let provider = Scripted::new(vec![vec![
        ProviderEvent::Delta("Hel".into()),
        ProviderEvent::Delta("lo".into()),
        ProviderEvent::Done { text: "Hello".into() },
    ]]);
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));

    session.send("hi", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    let events = session.drain_events();

    // seq is monotonic from 0 and the order is delta, delta, done.
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2]);
    assert!(matches!(&events[0].body, ChatEventBody::Delta { text, .. } if text == "Hel"));
    assert!(matches!(&events[1].body, ChatEventBody::Delta { text, .. } if text == "lo"));
    assert!(matches!(events[2].body, ChatEventBody::Done));
    assert!(session.is_idle());

    // The system prompt carried the executor's live capability text.
    assert!(turns.borrow()[0].system.contains("mesh.from_image.v1"));
}

#[test]
fn unavailable_provider_refuses_send_no_fallback_no_events() {
    let mut provider = Scripted::new(vec![]);
    provider.available =
        ProviderAvailability::Unavailable { reason: "no chat capability on fleet".into() };
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));

    let refusal = session.send("hi", &[], &mut exec).unwrap_err();
    assert_eq!(
        refusal,
        SendRefusal::ProviderUnavailable { reason: "no chat capability on fleet".into() }
    );
    // Honest refusal: nothing streamed, nothing started, nothing rerouted.
    assert!(session.drain_events().is_empty());
    assert!(turns.borrow().is_empty());
    assert_eq!(session.provider_kind(), ProviderKind::FleetQwen);
}

#[test]
fn busy_session_refuses_second_send() {
    let provider = Scripted::new(vec![vec![ProviderEvent::Delta("...".into())]]);
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));
    session.send("first", &[], &mut exec).unwrap();
    assert_eq!(session.send("second", &[], &mut exec).unwrap_err(), SendRefusal::Busy);
}

#[test]
fn tool_round_trip_events_history_and_followup_turn() {
    let call_args = json::obj(vec![("query", json::s("neon")), ("limit", Value::Int(3))]);
    let provider = Scripted::new(vec![
        vec![ProviderEvent::Done {
            text: format!("Let me look.\n{}", tool_line("asset.search", call_args)),
        }],
        vec![ProviderEvent::Done { text: "Found nothing interesting.".into() }],
    ]);
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("hits", Value::Arr(vec![]))]),
    });
    exec.progress_ticks = 2;
    let calls = exec.calls.clone();
    let mut session = Session::new("prin_test", Box::new(provider));

    session.send("find neon stuff", &[], &mut exec).unwrap();
    session.pump(&mut exec); // turn 1: tool call + execution + follow-up begins
    session.pump(&mut exec); // turn 2: final text
    let events = session.drain_events();

    let kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match &e.body {
            ChatEventBody::Delta { .. } => "delta",
            ChatEventBody::ToolCall { .. } => "tool_call",
            ChatEventBody::ToolProgress { .. } => "tool_progress",
            ChatEventBody::ToolResult { .. } => "tool_result",
            ChatEventBody::Done => "done",
            ChatEventBody::Cancelled => "cancelled",
            ChatEventBody::Error { .. } => "error",
        })
        .collect();
    assert_eq!(kinds, vec!["tool_call", "tool_progress", "tool_progress", "tool_result", "done"]);

    // The executor received the typed call.
    assert!(matches!(
        &calls.borrow()[0],
        ContentToolCall::AssetSearch { query, limit } if query == "neon" && *limit == 3
    ));

    // The follow-up turn carried the tool result as a Tool-role message.
    let followup = &turns.borrow()[1];
    let tool_msg = followup
        .messages
        .iter()
        .find(|m| m.role == makepad_asset_chat::wire::ChatRole::Tool)
        .expect("tool message in follow-up");
    assert!(tool_msg.text.contains("\"outcome\":\"ok\""));
    assert!(session.is_idle());
}

#[test]
fn malformed_tool_line_is_refused_back_to_model() {
    let provider = Scripted::new(vec![
        vec![ProviderEvent::Done { text: "<<tool>>{broken".into() }],
        vec![ProviderEvent::Done { text: "Sorry, retrying properly.".into() }],
    ]);
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));

    session.send("go", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    session.pump(&mut exec);

    // No executor call happened; the refusal went back as the tool message.
    assert!(exec.calls.borrow().is_empty());
    let followup = &turns.borrow()[1];
    let tool_msg = followup
        .messages
        .iter()
        .find(|m| m.role == makepad_asset_chat::wire::ChatRole::Tool)
        .unwrap();
    assert!(tool_msg.text.contains("refused"));
}

#[test]
fn leaked_level_source_is_refused_back_not_final() {
    // 2026-08-27 dog-shop regression: the model hit the token cap while
    // printing a whole interior as PLAIN TEXT (no tool call). That must
    // spend a corrective round, not end the turn as a final answer.
    let leak = "Here is the shop:
game.sky({top: #111})
game.box({pos: vec3(0,0,0), size: vec3(1,1,1)})
game.box({pos: vec3(1,0,0), size: vec3(1,1,1)})
game.box({pos: vec3(2,0,0), size: vec3(1,1,1)})
game.box({pos: vec3(3,0.9,-2.0), size: vec3(0.25,0.3,0.2), body:";
    let provider = Scripted::new(vec![
        vec![ProviderEvent::Done { text: leak.into() }],
        vec![ProviderEvent::Done { text: "Calling the tool properly now.".into() }],
    ]);
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));

    session.send("furnish the shop", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    session.pump(&mut exec);

    // Nothing executed; the model got a corrective tool message naming the fix.
    assert!(exec.calls.borrow().is_empty());
    let followup = &turns.borrow()[1];
    let tool_msg = followup
        .messages
        .iter()
        .find(|m| m.role == makepad_asset_chat::wire::ChatRole::Tool)
        .unwrap();
    assert!(tool_msg.text.contains("plain text"), "{}", tool_msg.text);
    assert!(tool_msg.text.contains("add_addon"), "{}", tool_msg.text);
}

#[test]
fn attachments_bind_known_revisions_and_tool_results_extend_them() {
    let input = rev(0x33);
    let derived = rev(0x44);
    let provider = Scripted::new(vec![
        vec![ProviderEvent::Done {
            text: tool_line("operation.get", json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))])),
        }],
        vec![ProviderEvent::Done { text: "done".into() }],
    ]);
    let mut exec = Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("result_revision", json::s(derived.to_string()))]),
    });
    let mut session = Session::new("prin_test", Box::new(provider));

    session
        .send(
            "derive from this",
            &[AttachmentBinding { revision: input, role: "source".into() }],
            &mut exec,
        )
        .unwrap();
    assert!(session.known_revisions().contains(&input));
    assert!(!session.known_revisions().contains(&derived));

    session.pump(&mut exec);
    // The tool result's revision became chainable.
    assert!(session.known_revisions().contains(&derived));
}

#[test]
fn refused_oversized_attachments_do_not_authorize_revisions() {
    let leaked = rev(0xAB);
    let provider = Scripted::new(vec![vec![ProviderEvent::Done { text: "ok".into() }]]);
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));
    let refusal = session
        .send(
            &"x".repeat(MAX_MESSAGE_BYTES),
            &[AttachmentBinding { revision: leaked, role: "source".into() }],
            &mut exec,
        )
        .unwrap_err();
    assert_eq!(refusal, SendRefusal::TooLarge { what: "message" });
    assert!(!session.known_revisions().contains(&leaked));
    assert!(session.is_idle());
}

#[test]
fn refused_provider_start_does_not_authorize_revisions() {
    let leaked = rev(0xCD);
    let ok_rev = rev(0x11);
    let mut provider = Scripted::new(vec![
        vec![ProviderEvent::Done {
            text: tool_line(
                "operation.get",
                json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))]),
            ),
        }],
        vec![ProviderEvent::Done { text: "done".into() }],
    ]);
    provider.begin_fails = 1;
    let mut exec = Recorder::new(ToolOutcome::Ok { value: json::obj(vec![("ok", json::s("1"))]) });
    let seen = exec.seen_known.clone();
    let mut session = Session::new("prin_test", Box::new(provider));

    let refusal = session
        .send(
            "go",
            &[AttachmentBinding { revision: leaked, role: "source".into() }],
            &mut exec,
        )
        .unwrap_err();
    assert!(matches!(refusal, SendRefusal::ProviderError { .. }), "{refusal:?}");
    assert!(!session.known_revisions().contains(&leaked));
    assert!(session.is_idle());

    session
        .send(
            "retry without the leaked attach",
            &[AttachmentBinding { revision: ok_rev, role: "source".into() }],
            &mut exec,
        )
        .unwrap();
    session.pump(&mut exec);
    assert!(session.known_revisions().contains(&ok_rev));
    assert!(!session.known_revisions().contains(&leaked));
    let last_known = seen.borrow().last().cloned().expect("tool saw known");
    assert!(last_known.contains(&ok_rev));
    assert!(!last_known.contains(&leaked), "refused revision must not be a transform input");
}

#[test]
fn cancel_mid_stream_emits_cancelled_and_idles() {
    let provider = Scripted::new(vec![vec![ProviderEvent::Delta("stream".into())]]);
    let cancelled = provider.cancelled.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));

    session.send("go", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    session.cancel();
    let events = session.drain_events();
    assert!(matches!(events.last().unwrap().body, ChatEventBody::Cancelled));
    assert!(session.is_idle());
    assert_eq!(*cancelled.borrow(), 1);

    // Cancel when idle is a no-op, not an event.
    session.cancel();
    assert!(session.drain_events().is_empty());
}

#[test]
fn tool_round_budget_degrades_gracefully_on_the_textual_lane() {
    // A provider that answers EVERY turn with another tool call. The
    // textual lane must NOT hard-kill the turn at the budget: the model
    // gets one final completion round (with a nudge in history) and any
    // tool line it emits there is cut off, not executed — the turn ends
    // in Done, never a dead session.
    let scripts: Vec<Vec<ProviderEvent>> = (0..MAX_TOOL_ROUNDS + 2)
        .map(|_| {
            vec![ProviderEvent::Done {
                text: tool_line("operation.get", json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))])),
            }]
        })
        .collect();
    let provider = Scripted::new(scripts);
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));

    session.send("loop forever", &[], &mut exec).unwrap();
    for _ in 0..MAX_TOOL_ROUNDS + 4 {
        session.pump(&mut exec);
    }
    let events = session.drain_events();
    let last = events.last().unwrap();
    assert!(
        matches!(&last.body, ChatEventBody::Done),
        "the budget must end the turn gracefully, got {:?}",
        last.body
    );
    assert!(session.is_idle());
    assert!(!session.is_sealed(), "a budgeted turn is not a dead session");
    // Exactly the budget executed; the final round's tool line did not.
    assert_eq!(exec.calls.borrow().len(), MAX_TOOL_ROUNDS as usize);
    // The final provider turn saw the nudge in its history.
    let final_input = turns.borrow().last().cloned().unwrap();
    assert!(
        final_input.messages.iter().any(|m| m.text.contains("tool budget reached")),
        "the final round must carry the budget nudge"
    );
}

/// Qwen keeps the textual marker contract; native providers do not.
/// Validated tool DTOs stay byte-equivalent when the same call is executed.
#[test]
fn qwen_marker_and_native_prompt_split_with_equivalent_tool_dtos() {
    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let input_rev = rev(0x66);
    let call_args = json::obj(vec![
        ("kind", json::s("mesh.from_image.v1")),
        (
            "inputs",
            Value::Arr(vec![json::obj(vec![
                ("slot", json::s("image")),
                ("asset", json::s(asset.to_string())),
                ("revision", json::s(input_rev.to_string())),
                ("role", json::s("texture")),
            ])]),
        ),
        ("params", json::obj(vec![("seed", Value::Int(3))])),
    ]);

    let mut qwen = Scripted::new(vec![
        vec![ProviderEvent::Done { text: tool_line("operation.create", call_args.clone()) }],
        vec![ProviderEvent::Done { text: "done".into() }],
    ]);
    qwen.kind = ProviderKind::FleetQwen;
    let qwen_turns = qwen.turns.clone();
    let mut qwen_exec = Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))]),
    });
    let qwen_calls = qwen_exec.calls.clone();
    let mut qwen_session = Session::new("prin_parity", Box::new(qwen));
    qwen_session
        .send(
            "make a mesh",
            &[AttachmentBinding { revision: input_rev, role: "image".into() }],
            &mut qwen_exec,
        )
        .unwrap();
    qwen_session.pump(&mut qwen_exec);
    qwen_session.pump(&mut qwen_exec);

    let mut native = Scripted::new(vec![
        vec![
            ProviderEvent::Delta("working".into()),
            ProviderEvent::FunctionCall {
                call_id: "call_create_1".into(),
                name: "operation_create".into(),
                arguments: call_args.to_json(),
            },
        ],
        vec![ProviderEvent::Delta("done".into()), ProviderEvent::Done { text: "done".into() }],
    ]);
    native.kind = ProviderKind::OpenAi;
    let native_turns = native.turns.clone();
    let native_conts = native.continuations.clone();
    let mut native_exec = Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))]),
    });
    let native_calls = native_exec.calls.clone();
    let mut native_session = Session::new("prin_parity", Box::new(native));
    native_session
        .send(
            "make a mesh",
            &[AttachmentBinding { revision: input_rev, role: "image".into() }],
            &mut native_exec,
        )
        .unwrap();
    native_session.pump(&mut native_exec);
    native_session.pump(&mut native_exec);

    let qwen_system = qwen_turns.borrow()[0].system.clone();
    let native_system = native_turns.borrow()[0].system.clone();
    assert!(qwen_system.contains("<<tool>>") || qwen_system.contains("<tool_call>"), "qwen must advertise a supported textual tool contract");
    assert!(!native_system.contains("<<tool>>"), "native prompt must not mention the marker");
    assert!(!native_system.contains("<tool_call>"), "native prompt must not teach textual tool blocks");
    assert!(native_system.contains("asset_search"));
    assert_eq!(
        encode_calls(&qwen_calls.borrow()),
        encode_calls(&native_calls.borrow())
    );
    assert_eq!(native_conts.borrow().len(), 1);
    assert_eq!(qwen_session.provider_kind().slug(), "fleet-qwen");
    assert_eq!(native_session.provider_kind().slug(), "openai");
}

fn encode_calls(calls: &[ContentToolCall]) -> Vec<String> {
    calls
        .iter()
        .map(|c| {
            format!(
                "{}:{}",
                c.name(),
                makepad_asset_chat::tools::encode_args(c).to_json()
            )
        })
        .collect()
}

#[test]
fn native_tool_executes_and_continues_exactly_once() {
    let args = json::obj(vec![("query", json::s("neon")), ("limit", Value::Int(3))]);
    let mut provider = Scripted::new(vec![
        vec![
            ProviderEvent::Delta("Let me look.".into()),
            ProviderEvent::FunctionCall {
                call_id: "call_search_1".into(),
                name: "asset_search".into(),
                arguments: args.to_json(),
            },
        ],
        vec![
            ProviderEvent::Delta("Found nothing interesting.".into()),
            ProviderEvent::Done { text: "Found nothing interesting.".into() },
        ],
    ]);
    provider.kind = ProviderKind::Grok;
    let conts = provider.continuations.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("hits", Value::Arr(vec![]))]),
    });
    exec.progress_ticks = 1;
    let calls = exec.calls.clone();
    let mut session = Session::new("prin_native", Box::new(provider));

    session.send("find neon", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    session.pump(&mut exec);
    let events = session.drain_events();

    let kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match &e.body {
            ChatEventBody::Delta { .. } => "delta",
            ChatEventBody::ToolCall { .. } => "tool_call",
            ChatEventBody::ToolProgress { .. } => "tool_progress",
            ChatEventBody::ToolResult { .. } => "tool_result",
            ChatEventBody::Done => "done",
            ChatEventBody::Cancelled => "cancelled",
            ChatEventBody::Error { .. } => "error",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["delta", "tool_call", "tool_progress", "tool_result", "delta", "done"]
    );
    let tool_calls: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.body, ChatEventBody::ToolCall { .. }))
        .collect();
    let tool_results: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.body, ChatEventBody::ToolResult { .. }))
        .collect();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_results.len(), 1);
    assert!(matches!(
        &events[1].body,
        ChatEventBody::ToolCall { id, name, .. } if id == "call_search_1" && name == "asset.search"
    ));
    assert_eq!(calls.borrow().len(), 1);
    assert!(matches!(
        &calls.borrow()[0],
        ContentToolCall::AssetSearch { query, limit } if query == "neon" && *limit == 3
    ));
    assert_eq!(conts.borrow().len(), 1);
    assert_eq!(conts.borrow()[0].0, "call_search_1");
    assert!(conts.borrow()[0].1.contains("\"outcome\":\"ok\""));
    assert_eq!(session.provider_kind().slug(), "grok");
    assert!(session.is_idle());
}

#[test]
fn native_malformed_args_are_refused_continuation() {
    let mut provider = Scripted::new(vec![
        vec![ProviderEvent::FunctionCall {
            call_id: "call_bad".into(),
            name: "asset_search".into(),
            arguments: "not-json".into(),
        }],
        vec![ProviderEvent::Done { text: "ok".into() }],
    ]);
    provider.kind = ProviderKind::OpenAi;
    let conts = provider.continuations.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_native", Box::new(provider));
    session.send("go", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    session.pump(&mut exec);
    assert!(exec.calls.borrow().is_empty());
    assert_eq!(conts.borrow().len(), 1);
    assert_eq!(conts.borrow()[0].0, "call_bad");
    assert!(conts.borrow()[0].1.contains("refused"));
    let events = session.drain_events();
    assert!(events.iter().any(|e| matches!(
        &e.body,
        ChatEventBody::ToolResult { outcome: ToolOutcome::Refused { .. }, .. }
    )));
}

#[test]
fn native_tool_round_budget_terminates() {
    let scripts: Vec<Vec<ProviderEvent>> = (0..MAX_TOOL_ROUNDS + 2)
        .map(|i| {
            vec![ProviderEvent::FunctionCall {
                call_id: format!("call_{i}"),
                name: "operation_get".into(),
                arguments: r#"{"operation":"op_00000000000000000000000000000000"}"#.into(),
            }]
        })
        .collect();
    let mut provider = Scripted::new(scripts);
    provider.kind = ProviderKind::OpenAi;
    let conts = provider.continuations.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_native", Box::new(provider));
    session.send("loop forever", &[], &mut exec).unwrap();
    for _ in 0..MAX_TOOL_ROUNDS + 2 {
        session.pump(&mut exec);
    }
    let events = session.drain_events();
    let last = events.last().unwrap();
    assert!(
        matches!(&last.body, ChatEventBody::Error { code, .. } if code == "tool_budget"),
        "expected tool_budget, got {:?}",
        last.body
    );
    assert!(session.is_idle());
    assert_eq!(exec.calls.borrow().len(), MAX_TOOL_ROUNDS as usize);
    assert_eq!(conts.borrow().len(), (MAX_TOOL_ROUNDS - 1) as usize);
}

#[test]
fn native_continue_error_keeps_session_idle_and_history_intact() {
    let mut provider = Scripted::new(vec![
        vec![ProviderEvent::FunctionCall {
            call_id: "call_1".into(),
            name: "asset_search".into(),
            arguments: r#"{"query":"x"}"#.into(),
        }],
        vec![ProviderEvent::Error("boom".into())],
        vec![ProviderEvent::Done { text: "ok".into() }],
    ]);
    provider.kind = ProviderKind::OpenAi;
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_native", Box::new(provider));
    session.send("go", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    session.pump(&mut exec);
    assert!(session.is_idle());
    let events = session.drain_events();
    assert!(events.iter().any(|e| matches!(e.body, ChatEventBody::Error { .. })));
    let refusal = session.send("again", &[], &mut exec).unwrap_err();
    assert!(
        matches!(refusal, SendRefusal::Sealed { .. }),
        "expected sealed session, got {refusal:?}"
    );
    assert_eq!(turns.borrow().len(), 1);
}

#[test]
fn executed_mutation_plus_continuation_error_never_runs_again() {
    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let input_rev = rev(0x66);
    let call_args = json::obj(vec![
        ("kind", json::s("mesh.from_image.v1")),
        (
            "inputs",
            Value::Arr(vec![json::obj(vec![
                ("slot", json::s("image")),
                ("asset", json::s(asset.to_string())),
                ("revision", json::s(input_rev.to_string())),
                ("role", json::s("texture")),
            ])]),
        ),
    ]);
    let mut provider = Scripted::new(vec![
        vec![ProviderEvent::FunctionCall {
            call_id: "call_create_1".into(),
            name: "operation_create".into(),
            arguments: call_args.to_json(),
        }],
        vec![ProviderEvent::Error("continuation failed".into())],
        vec![ProviderEvent::Done { text: "should never run".into() }],
    ]);
    provider.kind = ProviderKind::OpenAi;
    let turns = provider.turns.clone();
    let mut exec = Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))]),
    });
    let calls = exec.calls.clone();
    let mut session = Session::new("prin_mut", Box::new(provider));
    session
        .send(
            "make a mesh",
            &[AttachmentBinding { revision: input_rev, role: "image".into() }],
            &mut exec,
        )
        .unwrap();
    session.pump(&mut exec);
    session.pump(&mut exec);
    assert!(session.is_idle());
    assert_eq!(calls.borrow().len(), 1);
    assert!(matches!(calls.borrow()[0], ContentToolCall::OperationCreate { .. }));
    let refusal = session.send("try again", &[], &mut exec).unwrap_err();
    assert!(matches!(refusal, SendRefusal::Sealed { .. }), "{refusal:?}");
    session.pump(&mut exec);
    assert_eq!(calls.borrow().len(), 1);
    assert_eq!(turns.borrow().len(), 1);
}

#[test]
fn executed_mutation_plus_continuation_cancel_never_runs_again() {
    let asset = makepad_asset_data::AssetId::from_bytes([9; 16]);
    let input_rev = rev(0x77);
    let call_args = json::obj(vec![
        ("kind", json::s("mesh.from_image.v1")),
        (
            "inputs",
            Value::Arr(vec![json::obj(vec![
                ("slot", json::s("image")),
                ("asset", json::s(asset.to_string())),
                ("revision", json::s(input_rev.to_string())),
                ("role", json::s("texture")),
            ])]),
        ),
    ]);
    let mut provider = Scripted::new(vec![
        vec![ProviderEvent::FunctionCall {
            call_id: "call_create_2".into(),
            name: "operation_create".into(),
            arguments: call_args.to_json(),
        }],
        vec![ProviderEvent::Delta("continuing".into())],
        vec![ProviderEvent::Done { text: "should never run".into() }],
    ]);
    provider.kind = ProviderKind::Grok;
    let mut exec = Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))]),
    });
    let calls = exec.calls.clone();
    let mut session = Session::new("prin_mut", Box::new(provider));
    session
        .send(
            "make a mesh",
            &[AttachmentBinding { revision: input_rev, role: "image".into() }],
            &mut exec,
        )
        .unwrap();
    session.pump(&mut exec);
    assert_eq!(calls.borrow().len(), 1);
    session.cancel();
    assert!(session.is_idle());
    let refusal = session.send("retry", &[], &mut exec).unwrap_err();
    assert!(matches!(refusal, SendRefusal::Sealed { .. }), "{refusal:?}");
    session.pump(&mut exec);
    assert_eq!(calls.borrow().len(), 1);
}

#[test]
fn provider_slugs_are_stable() {
    for (kind, slug) in [
        (ProviderKind::FleetQwen, "fleet-qwen"),
        (ProviderKind::OpenAi, "openai"),
        (ProviderKind::Grok, "grok"),
        (ProviderKind::ClaudeCli, "claude-cli"),
        (ProviderKind::CodexCli, "codex-cli"),
        (ProviderKind::GrokCli, "grok-cli"),
    ] {
        let mut provider = Scripted::new(vec![]);
        provider.kind = kind;
        let session = Session::new("p", Box::new(provider));
        assert_eq!(session.provider_kind().slug(), slug);
    }
}

#[test]
fn codex_json_fixture_runs_world_tool_continuation_and_stays_map_scoped() {
    struct GameRecorder(Recorder);
    impl ToolExecutor for GameRecorder {
        fn capability_doc(&mut self) -> String { "Village world tools".into() }
        fn tool_definitions(&mut self) -> Vec<makepad_asset_chat::tools::ToolDef> {
            makepad_asset_chat::tools::sandbox_definitions()
        }
        fn client_executes(&mut self, call: &ContentToolCall) -> bool {
            matches!(call, ContentToolCall::WorldGetPlan)
        }
        fn execute(&mut self, _: &ContentToolCall, _: &ExecCtx,
            _: &mut dyn FnMut(u16, &str), _: &CancelFlag) -> ToolOutcome {
            panic!("world tools belong to the game client");
        }
    }
    // Real Codex JSON parser -> ordinary Session -> ordinary typed world
    // tool execution. Only the external model and game result are fixtures.
    fn reply(text: &str) -> Vec<ProviderEvent> {
        use makepad_asset_chat::codex_cli::{parse_line, ParseState};
        let item = json::obj(vec![("type", json::s("item.completed")),
            ("item", json::obj(vec![("type", json::s("agent_message")), ("text", json::s(text))]))]);
        let mut state = ParseState::default();
        let (mut events, _) = parse_line(&item, &mut state);
        events.extend(parse_line(&json::obj(vec![("type", json::s("turn.completed"))]), &mut state).0);
        events
    }
    let mut provider = Scripted::new(vec![
        reply(&tool_line("world.get_plan", Value::Obj(vec![]))),
        reply("The village plan is revision 17."),
        vec![ProviderEvent::Delta("working".into())],
    ]);
    provider.kind = ProviderKind::CodexCli;
    let turns = provider.turns.clone();
    let cancelled = provider.cancelled.clone();
    let mut exec = GameRecorder(Recorder::new(ToolOutcome::Ok {
        value: json::obj(vec![("revision", Value::Int(17)), ("title", json::s("Village"))]),
    }));
    let mut session = Session::new("village", Box::new(provider));
    session.send("inspect Village", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    assert!(session.drain_events().iter().any(|event| matches!(
        &event.body, ChatEventBody::ToolCall { name, .. } if name == "world.get_plan")));
    session.provide_client_outcome("tc_1_1", exec.0.outcome.clone(), &mut exec).unwrap();
    session.pump(&mut exec);
    assert!(session.is_idle());
    assert!(exec.0.calls.borrow().is_empty());
    assert_eq!(turns.borrow().len(), 2);
    assert!(turns.borrow()[1].messages.iter().any(|message|
        message.text.contains("revision") && message.text.contains("17")));
    session.send("continue Village", &[], &mut exec).unwrap();
    session.cancel();
    assert!(session.is_idle());
    assert_eq!(*cancelled.borrow(), 1);

    let mut next = Scripted::new(vec![reply("Desert is a new map.")]);
    next.kind = ProviderKind::CodexCli;
    let next_turns = next.turns.clone();
    let mut next_session = Session::new("desert", Box::new(next));
    next_session.send("inspect Desert", &[], &mut exec).unwrap();
    next_session.pump(&mut exec);
    assert!(next_turns.borrow()[0].messages.iter().all(|message| !message.text.contains("Village")));
}

#[test]
fn progress_callbacks_are_bounded() {
    let provider = Scripted::new(vec![
        vec![ProviderEvent::Done {
            text: tool_line(
                "operation.get",
                json::obj(vec![("operation", json::s("op_00000000000000000000000000000000"))]),
            ),
        }],
        vec![ProviderEvent::Done { text: "done".into() }],
    ]);
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    exec.progress_ticks = (MAX_PROGRESS_EVENTS as u16).saturating_add(40);
    let mut session = Session::new("prin_test", Box::new(provider));
    session.send("go", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    let progress = session
        .drain_events()
        .into_iter()
        .filter(|e| matches!(e.body, ChatEventBody::ToolProgress { .. }))
        .count();
    assert_eq!(progress, MAX_PROGRESS_EVENTS);
}

#[test]
fn native_oversize_arguments_are_refused_before_json_parse() {
    let mut provider = Scripted::new(vec![
        vec![ProviderEvent::FunctionCall {
            call_id: "call_big".into(),
            name: "asset_search".into(),
            arguments: format!("{{\"query\":\"{}\"}}", "x".repeat(MAX_TOOL_JSON_BYTES)),
        }],
        vec![ProviderEvent::Done { text: "ok".into() }],
    ]);
    provider.kind = ProviderKind::OpenAi;
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_native", Box::new(provider));
    session.send("go", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    assert!(exec.calls.borrow().is_empty());
    let events = session.drain_events();
    assert!(events.iter().any(|e| matches!(
        &e.body,
        ChatEventBody::ToolResult { outcome: ToolOutcome::Refused { what }, .. } if what.contains("too large")
    )));
}

#[test]
fn session_ids_are_unique_and_parseable() {
    let a = Session::new("p", Box::new(Scripted::new(vec![])));
    let b = Session::new("p", Box::new(Scripted::new(vec![])));
    assert_ne!(a.id().as_str(), b.id().as_str());
    assert!(makepad_asset_chat::session::SessionId::parse(a.id().as_str()).is_some());
    assert!(makepad_asset_chat::session::SessionId::parse("chat_zz").is_none());
    // Origin keeps principal locally; session id is the dispatch scope.
    assert_eq!(a.origin().principal, "p");
    assert_eq!(a.origin().session.as_str(), a.id().as_str());
}


/// A turn spends its opening inside the model's think block. If that reasoning
/// is not streamed as text there is no delta for the serving facts to ride on
/// — and the client would see nothing during precisely the wait it most wants
/// explained, with its rate readout frozen at whatever the last text carried.
#[test]
fn serving_facts_reach_the_client_even_when_no_text_does() {
    let facts = ServingFacts {
        gen_tokens: 24,
        think_tokens: Some(24),
        ..Default::default()
    };
    // A poll that reports progress and NO text: the box is generating, the
    // user can read none of it yet.
    let provider = Scripted::new(vec![vec![ProviderEvent::Serving(facts)]]);
    let mut exec = Recorder::new(ToolOutcome::Ok { value: Value::Obj(vec![]) });
    let mut session = Session::new("prin_test", Box::new(provider));
    session.send("hi", &[], &mut exec).unwrap();
    session.pump(&mut exec);

    let deltas: Vec<(String, Option<ServingFacts>)> = session
        .drain_events()
        .iter()
        .filter_map(|e| match &e.body {
            ChatEventBody::Delta { text, serving } => Some((text.clone(), *serving)),
            _ => None,
        })
        .collect();
    let (text, serving) = deltas
        .last()
        .expect("a silent phase must still report the facts");
    assert_eq!(text, "", "carried on an EMPTY delta, which appends nothing");
    let serving = serving.expect("the facts are the whole point of the event");
    assert_eq!(serving.gen_tokens, 24);
    assert_eq!(serving.think_tokens, Some(24));
}
