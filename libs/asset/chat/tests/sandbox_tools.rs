//! The sandbox tool extension: queued generation, `assets.query`/`assets.schema`
//! and the `world.*` placement verbs. Parsing is the security boundary —
//! every wrong shape refuses — and the extension is only ADVERTISED when an
//! executor opts in via `ToolExecutor::tool_definitions`.

use makepad_asset_chat::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_asset_chat::session::{CancelFlag, ExecCtx, Session, ToolExecutor};
use makepad_asset_chat::tools::{
    canonical_from_api_name, definitions, encode_args, sandbox_definitions, ContentGenerateKind,
    ContentToolCall, SpawnForm, SpawnScale, ToolDef, CSG_MODEL_TOOL_DOC,
};
use makepad_asset_chat::wire::{ProviderAvailability, ProviderKind, ToolOutcome};
use makepad_asset_client::json::{self, Value};
use makepad_asset_data::ScalePreset;
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
fn world_api_is_a_bounded_read_only_game_tool_with_native_name_roundtrip() {
    use makepad_asset_chat::context::ClientProfile;
    use makepad_asset_chat::sandbox_effect::SandboxEffect;
    for args in ["{}", r#"{"query":"game.ui","limit":20,"cursor":12}"#] {
        let call = parse("world.api", obj(args)).unwrap();
        assert_eq!(parse(call.name(), encode_args(&call)).unwrap(), call);
        assert_eq!(call.sandbox_effect(), Some(SandboxEffect::Read));
        assert!(ClientProfile::Game.client_executes(&call));
        assert!(!ClientProfile::General.client_executes(&call));
    }
    assert_eq!(canonical_from_api_name("world_api"), Some("world.api"));
    for bad in [r#"{"query":12}"#, r#"{"limit":0}"#, r#"{"limit":21}"#,
        r#"{"cursor":-1}"#, r#"{"cursor":1000001}"#, r#"{"path":"/etc/passwd"}"#] {
        assert!(parse("world.api", obj(bad)).is_err(), "{bad}");
    }
    assert!(parse("world.api", json::obj(vec![("query", json::s("x".repeat(161)))])).is_err());
}

#[test]
fn content_generate_roundtrips_and_refuses_unbounded_jobs() {
    let call = parse(
        "content.generate",
        obj(r#"{"kind":"character","prompt":"a hopping clockwork bunny","dim_height":1.25}"#),
    )
    .unwrap();
    assert_eq!(
        call,
        ContentToolCall::ContentGenerate {
            kind: ContentGenerateKind::Character,
            prompt: "a hopping clockwork bunny".into(),
            dim_height: Some(1.25),
        }
    );
    assert_eq!(ContentToolCall::parse(call.name(), &encode_args(&call)).unwrap(), call);
    for bad in [
        r#"{"kind":"video","prompt":"x"}"#,
        r#"{"kind":"prop","prompt":""}"#,
        r#"{"kind":"prop","prompt":"x","dim_height":0}"#,
        r#"{"kind":"sound","prompt":"x","dim_height":101}"#,
        r#"{"kind":"sound","prompt":"x","model":"expensive"}"#,
    ] {
        assert!(parse("content.generate", obj(bad)).is_err(), "{bad}");
    }
}

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

#[test]
fn reviewed_model_tools_roundtrip_and_teaching_stays_small() {
    let source = "let s=csg.sphere({r:0.1})\ncsg.part(\"ball\",s,{color:#4477aa})";
    let call = parse(
        "model.build",
        json::obj(vec![("title", json::s("Blue Ball")), ("source", json::s(source))]),
    )
    .unwrap();
    assert!(matches!(&call, ContentToolCall::ModelBuild { title, source: got }
        if title == "Blue Ball" && got == source));
    assert_eq!(ContentToolCall::parse(call.name(), &encode_args(&call)).unwrap(), call);

    let fetch = parse("model.fetch", obj(r#"{"alias":"gen/csg/blue-ball"}"#)).unwrap();
    assert!(matches!(&fetch, ContentToolCall::ModelFetch { alias }
        if alias.as_str() == "gen/csg/blue-ball"));
    assert_eq!(ContentToolCall::parse(fetch.name(), &encode_args(&fetch)).unwrap(), fetch);
    assert!(parse("model.build", obj(r#"{"title":"x"}"#)).is_err());
    assert!(parse("model.fetch", obj(r#"{"alias":"public/not-csg"}"#)).is_err());

    assert!(CSG_MODEL_TOOL_DOC.len() < 4_608, "{} bytes", CSG_MODEL_TOOL_DOC.len());
    for verb in ["box", "sphere", "cylinder", "torus", "extrude", "lathe",
        "union", "difference", "intersect", "move", "rotate", "scale", "mirror",
        "implicit", "part", "anim"] {
        assert!(CSG_MODEL_TOOL_DOC.contains(&format!("csg.{verb}")), "missing {verb}");
    }
    for forbidden in ["csg.shape", "csg.op", "csg.transform", "csg.gear", "csg.xor"] {
        assert!(!CSG_MODEL_TOOL_DOC.contains(forbidden), "obsolete surface: {forbidden}");
    }
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
            scale: None,
            color: None,
            hue: None,
            tag: None,
        }
    );
    let re = ContentToolCall::parse("world.spawn", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    let exact = parse(
        "world.spawn",
        obj(r#"{"model": "kenney/car-kit/ambulance", "scale": 0.5}"#),
    )
    .unwrap();
    assert!(matches!(
        &exact,
        ContentToolCall::WorldSpawn { scale: Some(SpawnScale::Exact(0.5)), .. }
    ));
    assert_eq!(ContentToolCall::parse("world.spawn", &encode_args(&exact)).unwrap(), exact);
    let preset = parse(
        "world.spawn",
        obj(r#"{"model": "kenney/car-kit/ambulance", "scale": "small"}"#),
    )
    .unwrap();
    assert!(matches!(
        &preset,
        ContentToolCall::WorldSpawn {
            scale: Some(SpawnScale::Preset(ScalePreset::Small)),
            ..
        }
    ));
    assert_eq!(ContentToolCall::parse("world.spawn", &encode_args(&preset)).unwrap(), preset);
    let recolored = parse(
        "world.spawn",
        obj(r##"{"model": "gen/csg/corgi-dog", "color": "#44aaff", "hue": -75}"##),
    )
    .unwrap();
    assert!(matches!(
        &recolored,
        ContentToolCall::WorldSpawn {
            color: Some(color),
            hue: Some(-75.0),
            ..
        } if color == "#44aaff"
    ));
    assert_eq!(
        ContentToolCall::parse("world.spawn", &encode_args(&recolored)).unwrap(),
        recolored
    );
    assert!(parse(
        "world.spawn",
        obj(r##"{"model": "gen/csg/corgi-dog", "color": "blue"}"##),
    )
    .is_err());
    assert!(parse(
        "world.spawn",
        obj(r#"{"model": "kenney/car-kit/ambulance", "scale": 0.1}"#),
    )
    .is_err());
    assert!(parse(
        "world.spawn",
        obj(r#"{"model": "kenney/car-kit/ambulance", "scale": "tiny"}"#),
    )
    .is_err());
    let call = parse(
        "world.spawn",
        obj(r#"{"model": "kenney/nature-kit/tree_oak", "pos": [4, 0, 2], "form": "prop", "tag": "forest"}"#),
    )
    .unwrap();
    let re = ContentToolCall::parse("world.spawn", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    let follower = parse(
        "world.spawn",
        obj(r#"{"model": "gen/csg/corgi-dog", "form": "follower", "tag": "corgi"}"#),
    )
    .unwrap();
    assert!(matches!(
        &follower,
        ContentToolCall::WorldSpawn {
            form: Some(SpawnForm::Follower),
            ..
        }
    ));
    assert_eq!(
        ContentToolCall::parse("world.spawn", &encode_args(&follower)).unwrap(),
        follower
    );
    // world.tune: the world-knob verb (§4.5 tune slice). Both knobs are
    // optional, at least one is required, and each has its own band.
    let call = parse("world.tune", obj(r#"{"car_speed": 0.6}"#)).unwrap();
    assert_eq!(
        call,
        ContentToolCall::WorldTune { time: None, car_speed: Some(0.6) }
    );
    let re = ContentToolCall::parse("world.tune", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    let call = parse("world.tune", obj(r#"{"time": 22, "car_speed": 2}"#)).unwrap();
    assert_eq!(
        call,
        ContentToolCall::WorldTune { time: Some(22.0), car_speed: Some(2.0) }
    );
    let re = ContentToolCall::parse("world.tune", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    assert!(parse("world.tune", obj(r#"{}"#)).is_err(), "one knob required");
    assert!(
        parse("world.tune", obj(r#"{"car_speed": 0}"#)).is_err(),
        "a frozen fleet is out of band"
    );
    assert!(
        parse("world.tune", obj(r#"{"car_speed": 12}"#)).is_err(),
        "an uncatchable fleet is out of band"
    );
    assert!(
        parse("world.tune", obj(r#"{"speed": 2}"#)).is_err(),
        "unknown knobs refuse rather than silently doing nothing"
    );

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
    assert_eq!(extra.len(), 19);
    assert!(extra.iter().any(|d| d.name == "world.get_plan"));
    assert!(extra.iter().any(|d| d.name == "world.set_plan"));
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
    assert!(!names.contains(&"content.generate"));
}


#[test]
fn plan_schema_guides_required_routes_and_rejects_unsupported_nested_input() {
    let args = obj(r#"{"revision":0,"plan":{"v":1,"terrain":{"size":220,"relief":"flat"},"corridors":[{"id":"loop","kind":"rail","closed":true,"size":80}]}}"#);
    let call = parse("world.set_plan", args).unwrap();
    let ContentToolCall::WorldSetPlan { plan, .. } = &call else { panic!("plan variant") };
    assert_eq!(plan.get("corridors").unwrap().as_arr().unwrap()[0].get("required").and_then(Value::as_bool), Some(true));
    assert_eq!(parse(call.name(), encode_args(&call)).unwrap(), call);
    let optional = parse("world.set_plan", obj(r#"{"revision":0,"plan":{"corridors":[{"id":"loop","kind":"rail","closed":true,"required":false}]}}"#)).unwrap();
    let ContentToolCall::WorldSetPlan { plan, .. } = optional else { panic!("plan variant") };
    assert_eq!(plan.get("corridors").unwrap().as_arr().unwrap()[0].get("required").and_then(Value::as_bool), Some(false));
    for plan in [
        r#"{"terrain":{"size":601}}"#, r#"{"terrain":{"amp":"flat"}}"#,
        r#"{"corridors":[{"id":"x","kind":"rail","required":"yes"}]}"#,
        r#"{"corridors":[{"id":"x","kind":"tunnel","closed":true}]}"#,
        r#"{"corridors":[{"id":"x","kind":"road","path":"west"}]}"#,
        r#"{"corridors":[{"id":"x","kind":"road","path":[[1,2]]}]}"#,
        r#"{"corridors":[{"id":"x","kind":"road","from":"up","to":"east"}]}"#,
        r#"{"corridors":[{"id":"x","kind":"road","through":["loop@1.2"]}]}"#,
        r#"{"corridors":[{"id":"x","kind":"road","widht":9}]}"#,
        r#"{"corridors":[{"id":"x","kind":"rail"},{"id":"x","kind":"road"}]}"#,
        r#"{"corridors":[{"id":"bad:id","kind":"road"}]}"#,
        r#"{"water":[{"id":"brook","kind":"river","depth":0}]}"#,
        r#"{"dressing":{"forest":1.1}}"#, r#"{"biome":"volcanic"}"#,
    ] {
        let args = json::obj(vec![("revision", Value::Int(0)), ("plan", obj(plan))]);
        assert!(parse("world.set_plan", args).is_err(), "{plan}");
    }
}

#[test]
fn plan_schema_publishes_the_nested_contract() {
    fn non_null(v: &Value) -> &Value {
        v.get("anyOf").and_then(Value::as_arr).map_or(v, |v| &v[0])
    }
    let defs = sandbox_definitions();
    let def = defs.iter().find(|d| d.name == "world.set_plan").unwrap();
    let schema = def.parameters.get("properties").unwrap().get("plan").unwrap();
    assert_eq!(schema.get("additionalProperties").and_then(Value::as_bool), Some(false));
    let props = schema.get("properties").unwrap();
    let corridors = non_null(props.get("corridors").unwrap()).get("items").unwrap();
    assert_eq!(corridors.get("required").unwrap().as_arr().unwrap(), &[json::s("id")]);
    let fields = corridors.get("properties").unwrap();
    let requirement = non_null(fields.get("required").unwrap());
    assert_eq!(requirement.get("type").and_then(Value::as_str), Some("boolean"));
    assert_eq!(requirement.get("default").and_then(Value::as_bool), Some(true));
    let kinds = non_null(fields.get("kind").unwrap()).get("enum").unwrap().as_arr().unwrap();
    assert_eq!(kinds, &["road", "highway", "rail", "monorail", "path", "coaster"].map(json::s));
    let radius = non_null(fields.get("radius").unwrap());
    assert_eq!(radius.get("minimum"), Some(&Value::F64(4.0)));
    assert_eq!(radius.get("maximum"), Some(&Value::F64(60.0)));
    for key in ["biomes", "landforms", "water", "corridors", "places"] {
        let item = non_null(props.get(key).unwrap()).get("items").unwrap();
        assert_eq!(item.get("additionalProperties").and_then(Value::as_bool), Some(false));
    }
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

/// `world.new_level` parses like `world.set_source` plus a title, is
/// advertised next to it on the game surface, and is client-executed.
#[test]
fn world_new_level_roundtrips_bounds_and_is_a_game_client_tool() {
    let call = parse(
        "world.new_level",
        obj(r#"{"title": "Quarry Arena", "source": "game.sky({})\ngame.terrain({size: 120, cells: 65, smooth: true})", "note": "first cut"}"#),
    )
    .unwrap();
    let ContentToolCall::WorldNewLevel { title, source, note } = &call else {
        panic!("wrong variant");
    };
    assert_eq!(title, "Quarry Arena");
    assert!(source.starts_with("game.sky"));
    assert_eq!(note.as_deref(), Some("first cut"));
    assert_eq!(call.name(), "world.new_level");
    let re = ContentToolCall::parse("world.new_level", &encode_args(&call)).unwrap();
    assert_eq!(re, call);
    assert_eq!(canonical_from_api_name("world_new_level"), Some("world.new_level"));

    // Same caps as set_source on the source; a title is display text.
    let big = format!(r#"{{"title": "t", "source": "{}"}}"#, "g".repeat(13_000));
    assert!(parse("world.new_level", obj(&big)).is_err(), "oversized source refuses");
    assert!(parse("world.new_level", obj(r#"{"source": "x"}"#)).is_err(), "title required");
    assert!(parse("world.new_level", obj(r#"{"title": "t"}"#)).is_err(), "source required");
    assert!(parse("world.new_level", obj(r#"{"title": "   ", "source": "x"}"#)).is_err());
    assert!(parse("world.new_level", obj(r#"{"title": "ab", "source": "x"}"#)).is_ok());
    assert!(parse("world.new_level", obj(r#"{"title": "a\u0007b", "source": "x"}"#)).is_err(), "control chars refuse");
    let long_title = format!(r#"{{"title": "{}", "source": "x"}}"#, "t".repeat(81));
    assert!(parse("world.new_level", obj(&long_title)).is_err());
    assert!(parse("world.new_level", obj(r#"{"title": "t", "source": "x", "switch": true}"#)).is_err());

    // Advertised on the game surface, right after set_source; never on the
    // base list.
    let names: Vec<&str> = game_definitions().iter().map(|d| d.name).collect();
    let set = names.iter().position(|n| *n == "world.set_source").unwrap();
    assert_eq!(names[set + 1], "world.new_level", "{names:?}");
    assert!(!definitions().iter().any(|d| d.name == "world.new_level"));
    assert!(makepad_asset_chat::context::ClientProfile::Game.client_executes(&call));
    assert!(!makepad_asset_chat::context::ClientProfile::General.client_executes(&call));
}

#[test]
fn splash_source_tools_teach_if_else_and_range_loop_syntax() {
    let defs = sandbox_definitions();
    for name in ["world.add_addon", "world.set_source"] {
        let description = defs
            .iter()
            .find(|def| def.name == name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .description;
        assert!(description.contains("NO ternary `?:` — use if/else"), "{name}: {description}");
        assert!(description.contains("`for i in 0..n {}`"), "{name}: {description}");
    }
}

/// The client's answer to `world.new_level` ENDS the turn: the round is in
/// the history, `done` is emitted, and NO follow-up model round runs — the
/// player is in another game, with its own chat, by then.
#[test]
fn a_new_level_answer_ends_the_turn_without_another_model_round() {
    let inputs = Rc::new(RefCell::new(Vec::new()));
    let provider = Replay {
        turns: vec![
            "New level coming.\n<<tool>>{\"name\":\"world.new_level\",\"args\":{\"title\":\"Quarry\",\"source\":\"game.sky({})\"}}".into(),
            "THIS ROUND MUST NEVER RUN".into(),
        ],
        pending: Vec::new(),
        inputs: inputs.clone(),
    };
    let executed = Rc::new(RefCell::new(Vec::new()));
    let mut exec = GameExec { executed: executed.clone() };
    let mut session = Session::new("game", Box::new(provider));
    session.send("make me a new level", &[], &mut exec).unwrap();
    session.pump(&mut exec);
    assert_eq!(session.awaiting_client_tool(), Some("tc_1_1"));
    assert!(executed.borrow().is_empty());

    let ok = ToolOutcome::Ok {
        value: json::obj(vec![
            ("asset_id", json::s("ast_0123456789abcdef0123456789abcdef")),
            ("alias", json::s("games/quarry")),
            ("title", json::s("Quarry")),
        ]),
    };
    session.provide_client_outcome("tc_1_1", ok, &mut exec).unwrap();
    assert!(session.is_idle(), "the turn ends on the answer");
    let events = session.drain_events();
    assert!(events.iter().any(|e| matches!(&e.body, ChatEventBody::ToolResult { .. })));
    assert!(events.iter().any(|e| matches!(&e.body, ChatEventBody::Done)));
    assert!(!events.iter().any(|e| matches!(&e.body, ChatEventBody::Error { .. })), "{events:?}");
    assert_eq!(inputs.borrow().len(), 1, "no follow-up provider turn");
    // The round is recorded for the transcript: user, assistant(+call), tool.
    let roles: Vec<_> = session.history().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            makepad_asset_chat::wire::ChatRole::User,
            makepad_asset_chat::wire::ChatRole::Assistant,
            makepad_asset_chat::wire::ChatRole::Tool
        ]
    );
    let rows = makepad_asset_chat::transcript::render(session.history());
    assert_eq!(rows.last().unwrap().text, "world.new_level · ok");
    // The session is reusable afterwards (it is still this game's chat).
    assert!(session.send("still here?", &[], &mut exec).is_ok());
}

/// A session rebuilt from its persisted history keeps its id, turn and the
/// conversation, and the next send replays that history to a fresh
/// provider. Over-long histories keep the newest half.
#[test]
fn a_resumed_session_replays_its_history_on_the_next_send() {
    use makepad_asset_chat::session::SessionId;
    use makepad_asset_chat::wire::{ChatMessage, ChatRole};
    let inputs = Rc::new(RefCell::new(Vec::new()));
    let provider = Replay {
        turns: vec!["Welcome back.".into()],
        pending: Vec::new(),
        inputs: inputs.clone(),
    };
    let id = SessionId::parse("chat_00000000deadbeef").unwrap();
    let history = vec![
        ChatMessage::new(ChatRole::User, "make a level"),
        ChatMessage::new(ChatRole::Assistant, "Built it."),
        // An invalid (empty) row from a torn file is dropped, not fatal.
        ChatMessage::new(ChatRole::Tool, ""),
    ];
    let mut exec = GameExec { executed: Rc::new(RefCell::new(Vec::new())) };
    let mut session = Session::resume(id.clone(), "game", Box::new(provider), history, 7, None);
    assert_eq!(session.id(), &id);
    assert_eq!(session.turn(), 7);
    assert_eq!(session.history().len(), 2);
    assert_eq!(session.send("and now?", &[], &mut exec).unwrap(), 8);
    while !session.is_idle() {
        session.pump(&mut exec);
    }
    let input = inputs.borrow()[0].clone();
    assert_eq!(input.messages.len(), 3);
    assert_eq!(input.messages[0].text, "make a level");
    assert!(input.messages[2].text.starts_with("and now?"));

    let many: Vec<ChatMessage> = (0..200)
        .map(|i| ChatMessage::new(ChatRole::User, format!("m{i}")))
        .collect();
    let provider = Replay { turns: Vec::new(), pending: Vec::new(), inputs: inputs.clone() };
    let session = Session::resume(id, "game", Box::new(provider), many, 1, None);
    assert_eq!(session.history().len(), makepad_asset_chat::wire::MAX_MESSAGES / 2);
    assert_eq!(session.history().last().unwrap().text, "m199");
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
    assert!(!system2.contains("- world.place:"), "broker prompt must not advertise world tools");
    assert!(!system2.contains("- assets.query:"), "a guidance example is not an advertised tool");
}

#[test]
fn world_tools_accept_and_roundtrip_an_explicit_sub_world() {
    let call = parse("world.get_source", obj(r#"{"sub":"dogshop"}"#)).unwrap();
    assert!(matches!(
        call,
        ContentToolCall::WorldInSub { ref sub, ref call }
            if sub == "dogshop" && matches!(**call, ContentToolCall::WorldGetSource)
    ));
    assert_eq!(encode_args(&call).get("sub").and_then(Value::as_str), Some("dogshop"));
    assert!(parse("world.list", obj(r#"{"sub":"../escape"}"#)).is_err());
    let def = sandbox_definitions().into_iter().find(|def| def.name == "world.list").unwrap();
    assert!(def.parameters.to_json().contains("\"sub\""));
}

#[test]
fn per_turn_world_manifest_is_volatile_context_not_transcript_text() {
    let turns = Rc::new(RefCell::new(Vec::new()));
    let mut session = Session::new("test", Box::new(OneTurn { turns: turns.clone() }));
    let context = "WORLD MANIFEST: asset ast_x; worlds: `main`, `dogshop`\nthe player is currently in: `dogshop`";
    session
        .send_with_context("add a chair", &[], context, &mut SandboxExec)
        .unwrap();
    assert_eq!(turns.borrow()[0].dynamic_context, context);
    assert!(!turns.borrow()[0].system.contains(context), "the stable KV prefix must not churn each turn");
    assert_eq!(session.history()[0].text, "add a chair");
    assert!(!session.history()[0].text.contains("WORLD MANIFEST"));
}
