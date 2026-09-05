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
    assert!(qwen_system.contains("<<tool>>"), "qwen must keep the marker contract");
    assert!(!native_system.contains("<<tool>>"), "native prompt must not mention the marker");
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
