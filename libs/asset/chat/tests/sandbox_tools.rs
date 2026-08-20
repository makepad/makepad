//! The sandbox (game-client) tool extension: `assets.query`/`assets.schema`
//! and the `world.*` placement verbs. Parsing is the security boundary —
//! every wrong shape refuses — and the extension is only ADVERTISED when an
//! executor opts in via `ToolExecutor::tool_definitions`.

use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_asset_chat::session::{CancelFlag, ExecCtx, Session, ToolExecutor};
use makepad_asset_chat::tools::{
    canonical_from_api_name, definitions, encode_args, sandbox_definitions, ContentToolCall,
    ToolDef,
};
use makepad_asset_chat::wire::{ProviderAvailability, ProviderKind, ToolOutcome};
use makepad_asset_client::json::{self, Value};
use std::cell::RefCell;
use std::rc::Rc;

fn parse(name: &str, args: Value) -> Result<ContentToolCall, String> {
    ContentToolCall::parse(name, &args)
}

fn obj(json_text: &str) -> Value {
    json::parse(json_text.as_bytes()).expect("test json")
}

// ---------------------------------------------------------------- parsing

#[test]
fn assets_query_roundtrips_and_bounds_sql() {
    let call = parse("assets.query", obj(r#"{"sql": "SELECT 1"}"#)).unwrap();
    assert_eq!(call, ContentToolCall::AssetsQuery { sql: "SELECT 1".into() });
    assert_eq!(call.name(), "assets.query");
    let re = ContentToolCall::parse("assets.query", &encode_args(&call)).unwrap();
    assert_eq!(re, call);

    let big = format!(r#"{{"sql": "{}"}}"#, "S".repeat(5000));
    assert!(parse("assets.query", obj(&big)).is_err(), "oversized sql must refuse");
    assert!(parse("assets.query", obj(r#"{"sql": ""}"#)).is_err());
    assert!(parse("assets.query", obj(r#"{"sql": "SELECT 1", "x": 1}"#)).is_err());
    assert!(parse("assets.query", obj(r#"{}"#)).is_err());
}

#[test]
fn assets_schema_takes_no_arguments() {
    assert_eq!(parse("assets.schema", obj("{}")).unwrap(), ContentToolCall::AssetsSchema);
    assert!(parse("assets.schema", obj(r#"{"table": "x"}"#)).is_err());
}

#[test]
fn world_place_parses_a_batch_and_refuses_bad_shapes() {
    let call = parse(
        "world.place",
        obj(
            r#"{"items": [
                {"model": "kenney/props/fence", "pos": [4, 0, 2], "yaw_deg": 90, "scale": 1.5, "tag": "fence"},
                {"model": "doom/doom/worlds/doom1/e1m1", "pos": [0, 0, 0]}
            ]}"#,
        ),
    )
    .unwrap();
    let ContentToolCall::WorldPlace { items } = &call else {
        panic!("wrong variant");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].model, "kenney/props/fence");
    assert_eq!(items[0].pos, [4.0, 0.0, 2.0]);
    assert_eq!(items[0].yaw_deg, Some(90.0));
    assert_eq!(items[0].scale, Some(1.5));
    assert_eq!(items[0].tag.as_deref(), Some("fence"));
    assert_eq!(items[1].yaw_deg, None);
    let re = ContentToolCall::parse("world.place", &encode_args(&call)).unwrap();
    assert_eq!(re, call);

    // The model reference is spliced into game source as a string literal:
    // quotes, backslashes and newlines refuse instead of being escaped.
    for bad in [
        r#"{"items": [{"model": "a\"b", "pos": [0,0,0]}]}"#,
        r#"{"items": [{"model": "a\\b", "pos": [0,0,0]}]}"#,
        r#"{"items": [{"model": "a\nb", "pos": [0,0,0]}]}"#,
    ] {
        assert!(parse("world.place", obj(bad)).is_err(), "{bad}");
    }
    // Wrong positions.
    for bad in [
        r#"{"items": [{"model": "a", "pos": [0, 0]}]}"#,
        r#"{"items": [{"model": "a", "pos": [0, 0, 0, 0]}]}"#,
        r#"{"items": [{"model": "a", "pos": ["x", 0, 0]}]}"#,
        r#"{"items": [{"model": "a", "pos": [1e7, 0, 0]}]}"#,
        r#"{"items": [{"model": "a"}]}"#,
    ] {
        assert!(parse("world.place", obj(bad)).is_err(), "{bad}");
    }
    // Bounds and unknown fields.
    assert!(parse("world.place", obj(r#"{"items": []}"#)).is_err());
    assert!(parse("world.place", obj(r#"{"items": [{"model": "a", "pos": [0,0,0], "rot": 1}]}"#)).is_err());
    assert!(parse("world.place", obj(r#"{"items": [{"model": "a", "pos": [0,0,0], "scale": 0}]}"#)).is_err());
    assert!(parse("world.place", obj(r#"{"items": [{"model": "a", "pos": [0,0,0], "tag": "NOT OK"}]}"#)).is_err());
    let many: String = (0..65)
        .map(|_| r#"{"model": "a", "pos": [0,0,0]}"#)
        .collect::<Vec<_>>()
        .join(",");
    assert!(parse("world.place", obj(&format!(r#"{{"items": [{many}]}}"#))).is_err());
}

#[test]
fn world_remove_is_exactly_ids_or_tag() {
    let by_ids = parse("world.remove", obj(r#"{"ids": [3, 4]}"#)).unwrap();
    assert_eq!(by_ids, ContentToolCall::WorldRemove { ids: vec![3, 4], tag: None });
    let by_tag = parse("world.remove", obj(r#"{"tag": "fence"}"#)).unwrap();
    assert_eq!(by_tag, ContentToolCall::WorldRemove { ids: vec![], tag: Some("fence".into()) });
    for call in [&by_ids, &by_tag] {
        let re = ContentToolCall::parse("world.remove", &encode_args(call)).unwrap();
        assert_eq!(&re, call);
    }
    assert!(parse("world.remove", obj(r#"{}"#)).is_err(), "neither is refused");
    assert!(parse("world.remove", obj(r#"{"ids": [1], "tag": "x"}"#)).is_err(), "both is refused");
    assert!(parse("world.remove", obj(r#"{"ids": []}"#)).is_err());
    assert!(parse("world.remove", obj(r#"{"ids": [0]}"#)).is_err());
    assert!(parse("world.remove", obj(r#"{"ids": ["3"]}"#)).is_err());
}

#[test]
fn world_move_needs_id_plus_at_least_one_change() {
    let call = parse("world.move", obj(r#"{"id": 3, "pos": [6, 0, 2], "yaw_deg": 45}"#)).unwrap();
    assert_eq!(
        call,
        ContentToolCall::WorldMove {
            id: 3,
            pos: Some([6.0, 0.0, 2.0]),
            yaw_deg: Some(45.0),
            scale: None
        }
    );
    let re = ContentToolCall::parse("world.move", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    assert!(parse("world.move", obj(r#"{"id": 3}"#)).is_err());
    assert!(parse("world.move", obj(r#"{"pos": [0,0,0]}"#)).is_err());
    assert!(parse("world.move", obj(r#"{"id": 3, "scale": 2000}"#)).is_err());
}

#[test]
fn world_list_takes_no_arguments() {
    assert_eq!(parse("world.list", obj("{}")).unwrap(), ContentToolCall::WorldList);
    assert!(parse("world.list", obj(r#"{"tag": "x"}"#)).is_err());
}

// ------------------------------------------------------------ definitions

#[test]
fn world_source_tools_roundtrip_and_bound() {
    let call = parse(
        "world.set_source",
        obj(r#"{"source": "game.sky({})\ngame.terrain({size: 100, cells: 65, smooth: true})", "note": "v1"}"#),
    )
    .unwrap();
    let ContentToolCall::WorldSetSource { source, note } = &call else {
        panic!("wrong variant");
    };
    assert!(source.starts_with("game.sky"));
    assert_eq!(note.as_deref(), Some("v1"));
    let re = ContentToolCall::parse("world.set_source", &encode_args(&call)).unwrap();
    assert_eq!(re, call);

    let big = format!(r#"{{"source": "{}"}}"#, "g".repeat(13_000));
    assert!(parse("world.set_source", obj(&big)).is_err(), "oversized source refuses");
    assert!(parse("world.set_source", obj(r#"{}"#)).is_err());
    assert!(parse("world.set_source", obj(r#"{"source": "x", "mode": "y"}"#)).is_err());

    assert_eq!(
        parse("world.get_source", obj("{}")).unwrap(),
        ContentToolCall::WorldGetSource
    );
    assert!(parse("world.get_source", obj(r#"{"x": 1}"#)).is_err());

    // world.spawn: the content-add verb (§4.5 addon slice).
    let call = parse(
        "world.spawn",
        obj(r#"{"model": "kenney/car-kit/ambulance"}"#),
    )
    .unwrap();
    assert_eq!(
        call,
        ContentToolCall::WorldSpawn {
            model: "kenney/car-kit/ambulance".into(),
            pos: None,
            form: None,
            tag: None,
        }
    );
    let re = ContentToolCall::parse("world.spawn", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    let call = parse(
        "world.spawn",
        obj(r#"{"model": "kenney/nature-kit/tree_oak", "pos": [4, 0, 2], "form": "prop", "tag": "forest"}"#),
    )
    .unwrap();
    let re = ContentToolCall::parse("world.spawn", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    assert!(parse("world.spawn", obj(r#"{}"#)).is_err(), "model required");
    assert!(
        parse("world.spawn", obj(r#"{"model": "x", "form": "banana"}"#)).is_err(),
        "unknown form refuses"
    );
    assert!(
        parse("world.spawn", obj(r#"{"model": "x", "near": "player"}"#)).is_err(),
        "unknown keys refuse"
    );
}

#[test]
fn sandbox_definitions_are_consistent_and_disjoint_from_the_base() {
    let base = definitions();
    let extra = sandbox_definitions();
    assert_eq!(extra.len(), 11);
    for def in &extra {
        assert_eq!(
            canonical_from_api_name(def.api_name),
            Some(def.name),
            "{} must map back to {}",
            def.api_name,
            def.name
        );
        assert!(
            !base.iter().any(|b| b.name == def.name || b.api_name == def.api_name),
            "{} must not shadow a base tool",
            def.name
        );
        // The documented example must parse: a wrong args_doc would teach
        // the model a shape the parser refuses.
        for example in def.args_doc.split(" or ") {
            let args = obj(example.trim());
            ContentToolCall::parse(def.name, &args)
                .unwrap_or_else(|e| panic!("{} args_doc does not parse: {e}", def.name));
        }
    }
}

#[test]
fn base_args_docs_still_name_only_base_tools() {
    // The broker prompt must not grow sandbox tools by accident.
    let names: Vec<&str> = definitions().iter().map(|d| d.name).collect();
    assert!(!names.contains(&"assets.query"));
    assert!(!names.contains(&"world.place"));
}

// ------------------------------------------------- session advertisement

struct OneTurn {
    turns: Rc<RefCell<Vec<TurnInput>>>,
}

impl ChatProvider for OneTurn {
    fn kind(&self) -> ProviderKind {
        ProviderKind::FleetQwen
    }
    fn availability(&mut self) -> ProviderAvailability {
        ProviderAvailability::Available { model: "scripted".into(), detail: String::new() }
    }
    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        self.turns.borrow_mut().push(input.clone());
        Ok(())
    }
    fn poll(&mut self) -> Vec<ProviderEvent> {
        vec![ProviderEvent::Done { text: "ok".into() }]
    }
    fn cancel(&mut self) {}
}

struct SandboxExec;

impl ToolExecutor for SandboxExec {
    fn capability_doc(&mut self) -> String {
        "sandbox capabilities".into()
    }
    fn tool_definitions(&mut self) -> Vec<ToolDef> {
        let mut defs = definitions();
        defs.extend(sandbox_definitions());
        defs
    }
    fn execute(
        &mut self,
        _call: &ContentToolCall,
        _ctx: &ExecCtx,
        _progress: &mut dyn FnMut(u16, &str),
        _cancel: &CancelFlag,
    ) -> ToolOutcome {
        ToolOutcome::Ok { value: Value::Obj(Vec::new()) }
    }
}

struct DefaultExec;

impl ToolExecutor for DefaultExec {
    fn capability_doc(&mut self) -> String {
        "broker capabilities".into()
    }
    fn execute(
        &mut self,
        _call: &ContentToolCall,
        _ctx: &ExecCtx,
        _progress: &mut dyn FnMut(u16, &str),
        _cancel: &CancelFlag,
    ) -> ToolOutcome {
        ToolOutcome::Ok { value: Value::Obj(Vec::new()) }
    }
}

// ---------------------------------------------- native trained-format calls

use makepad_asset_chat::toolcall::{self, Extract};

#[test]
fn the_trained_qwen_tool_template_is_heard() {
    // Verbatim from a live village run: the model reverted to its trained
    // template and the turn silently died. Never again.
    let text = "Let me find cars.\n</think>\n\nSearching now.\n<tool_call>\n<function=asset_search>\n<parameter=query>\ncar vehicle driveable\n</parameter>\n<parameter=limit>\n15\n</parameter>\n</function>\n</tool_call>";
    let Extract::Call { clean, name, args } = toolcall::extract(text) else {
        panic!("native template not heard: {:?}", toolcall::extract(text));
    };
    assert_eq!(clean, "Searching now.");
    assert_eq!(name, "asset.search");
    assert_eq!(args.get("query").and_then(Value::as_str), Some("car vehicle driveable"));
    assert_eq!(args.get("limit").and_then(Value::as_i64), Some(15));
    // And the typed parser accepts the coerced arguments.
    ContentToolCall::parse(&name, &args).expect("typed parse of native-call args");
    // The UI strip hides the block.
    assert_eq!(toolcall::strip_marker(text), "Searching now.");
}

#[test]
fn native_template_carries_multiline_splash_source_intact() {
    let source = "game.sky({})\ngame.terrain({size: 120, cells: 65, smooth: true})\nlet hero = game.character({pos: vec3(0, 2, 8), player: true})";
    let text = format!(
        "</think>\n<tool_call>\n<function=world_set_source>\n<parameter=source>\n{source}\n</parameter>\n<parameter=note>\nvillage v1\n</parameter>\n</function>\n</tool_call>"
    );
    let Extract::Call { name, args, .. } = toolcall::extract(&text) else {
        panic!("not heard");
    };
    assert_eq!(name, "world.set_source");
    assert_eq!(args.get("source").and_then(Value::as_str), Some(source));
    let call = ContentToolCall::parse(&name, &args).unwrap();
    let ContentToolCall::WorldSetSource { source: parsed, note } = call else {
        panic!("wrong variant");
    };
    assert_eq!(parsed, source);
    assert_eq!(note.as_deref(), Some("village v1"));
}

#[test]
fn unknown_native_function_names_fail_closed_as_readable_refusals() {
    let text = "</think>\n<tool_call>\n<function=rm_rf>\n<parameter=path>\n/\n</parameter>\n</function>\n</tool_call>";
    let Extract::Call { name, args, .. } = toolcall::extract(text) else {
        panic!("expected a call shape");
    };
    assert_eq!(name, "rm_rf");
    assert!(ContentToolCall::parse(&name, &args).is_err(), "unknown tools refuse");
}

// -------------------------------------------- client-executed tool round trip

use makepad_asset_chat::tools::game_definitions;
use makepad_asset_chat::wire::ChatEventBody;

/// A provider that replays scripted turn texts (each `begin_turn` consumes
/// one) — enough to drive a tool round.
struct Replay {
    turns: Vec<String>,
    pending: Vec<ProviderEvent>,
    inputs: Rc<RefCell<Vec<TurnInput>>>,
}

impl ChatProvider for Replay {
    fn kind(&self) -> ProviderKind {
        ProviderKind::FleetQwen
    }
    fn availability(&mut self) -> ProviderAvailability {
        ProviderAvailability::Available { model: "scripted".into(), detail: String::new() }
    }
    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        self.inputs.borrow_mut().push(input.clone());
        if self.turns.is_empty() {
            return Err("script exhausted".into());
        }
        let text = self.turns.remove(0);
        self.pending = vec![ProviderEvent::Done { text }];
        Ok(())
    }
    fn poll(&mut self) -> Vec<ProviderEvent> {
        std::mem::take(&mut self.pending)
    }
    fn cancel(&mut self) {}
}

/// A game-session executor: world tools are client-executed; everything
/// else answers Ok. Records what it actually executed.
struct GameExec {
    executed: Rc<RefCell<Vec<String>>>,
}

impl ToolExecutor for GameExec {
    fn capability_doc(&mut self) -> String {
        "game capabilities".into()
    }
    fn tool_definitions(&mut self) -> Vec<ToolDef> {
        game_definitions()
    }
    fn client_executes(&mut self, call: &ContentToolCall) -> bool {
        call.name().starts_with("world.")
    }
    fn execute(
        &mut self,
        call: &ContentToolCall,
        _ctx: &ExecCtx,
        _progress: &mut dyn FnMut(u16, &str),
        _cancel: &CancelFlag,
    ) -> ToolOutcome {
        self.executed.borrow_mut().push(call.name().to_string());
        ToolOutcome::Ok { value: Value::Obj(Vec::new()) }
    }
}

#[test]
fn a_world_call_parks_the_turn_until_the_client_answers() {
    let inputs = Rc::new(RefCell::new(Vec::new()));
    let provider = Replay {
        turns: vec![
            "I'll rebuild it.\n<<tool>>{\"name\":\"world.set_source\",\"args\":{\"source\":\"game.sky({})\"}}".into(),
            "Done — the level is live.".into(),
        ],
        pending: Vec::new(),
        inputs: inputs.clone(),
    };
    let executed = Rc::new(RefCell::new(Vec::new()));
    let mut exec = GameExec { executed: executed.clone() };
    let mut session = Session::new("game", Box::new(provider));
    session.send("make a level", &[], &mut exec).unwrap();
    session.pump(&mut exec);

    // Parked: the ToolCall event is out, nothing executed server-side,
    // the session is busy but pumping does nothing.
    assert_eq!(session.awaiting_client_tool(), Some("tc_1_1"));
    assert!(!session.is_idle());
    assert!(executed.borrow().is_empty(), "world tools must not execute in the broker");
    let events = session.drain_events();
    assert!(events.iter().any(|e| matches!(
        &e.body,
        ChatEventBody::ToolCall { name, .. } if name == "world.set_source"
    )));
    session.pump(&mut exec);
    assert_eq!(session.awaiting_client_tool(), Some("tc_1_1"), "parked stays parked");

    // A wrong call id is refused; the right one resumes the turn.
    let ok = ToolOutcome::Ok {
        value: json::obj(vec![("eval", json::s("game evaluated successfully"))]),
    };
    assert!(session.provide_client_outcome("tc_9_9", ok.clone(), &mut exec).is_err());
    session.provide_client_outcome("tc_1_1", ok, &mut exec).unwrap();
    while !session.is_idle() {
        session.pump(&mut exec);
    }
    let events = session.drain_events();
    assert!(events.iter().any(|e| matches!(&e.body, ChatEventBody::ToolResult { .. })));
    assert!(events.iter().any(|e| matches!(&e.body, ChatEventBody::Done)));
    // The follow-up provider turn carried the tool outcome in history.
    let last_input = inputs.borrow().last().cloned().unwrap();
    assert!(last_input
        .messages
        .iter()
        .any(|m| m.text.contains("game evaluated successfully")));
    // The game prompt advertised the world tools and dropped generation.
    let system = inputs.borrow()[0].system.clone();
    assert!(system.contains("world.set_source"));
    assert!(!system.contains("- image.generate:"), "phase 1 hides generation tools");
}

#[test]
fn cancelling_a_parked_turn_frees_the_session() {
    let provider = Replay {
        turns: vec![
            "<<tool>>{\"name\":\"world.list\",\"args\":{}}".into(),
        ],
        pending: Vec::new(),
        inputs: Rc::new(RefCell::new(Vec::new())),
    };
    let executed = Rc::new(RefCell::new(Vec::new()));
    let mut exec = GameExec { executed };
    let mut session = Session::new("game", Box::new(provider));
    session.send("what is placed?", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    assert!(session.awaiting_client_tool().is_some());
    session.cancel();
    assert!(session.is_idle());
    assert!(session.awaiting_client_tool().is_none());
    let events = session.drain_events();
    assert!(events.iter().any(|e| matches!(&e.body, ChatEventBody::Cancelled)));
    // A late tool result for the cancelled call is refused, not applied.
    assert!(session
        .provide_client_outcome(
            "tc_1_1",
            ToolOutcome::Ok { value: Value::Obj(Vec::new()) },
            &mut exec
        )
        .is_err());
}

#[test]
fn the_system_prompt_lists_exactly_what_the_executor_advertises() {
    // Sandbox executor: the world tools are in the rendered system text.
    let turns = Rc::new(RefCell::new(Vec::new()));
    let mut session =
        Session::new("test", Box::new(OneTurn { turns: turns.clone() }));
    session.send("hello", &[], &mut SandboxExec).unwrap();
    let system = turns.borrow()[0].system.clone();
    assert!(system.contains("world.place"), "sandbox tools must be advertised");
    assert!(system.contains("assets.query"));
    assert!(system.contains("image.generate"), "base tools stay");

    // Default executor: the broker prompt is unchanged — no world tools.
    let turns2 = Rc::new(RefCell::new(Vec::new()));
    let mut session2 =
        Session::new("test", Box::new(OneTurn { turns: turns2.clone() }));
    session2.send("hello", &[], &mut DefaultExec).unwrap();
    let system2 = turns2.borrow()[0].system.clone();
    assert!(!system2.contains("world.place"), "broker prompt must not grow world tools");
    assert!(!system2.contains("assets.query"));
}
