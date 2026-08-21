//! Wire/schema contract tests. The sandbox will speak this exact schema
//! later, so representative encodings are pinned byte-for-byte: changing
//! them is a WIRE_VERSION event, not a refactor.

use makepad_asset_chat::toolcall::{self, Extract};
use makepad_asset_chat::tools::{self, ContentToolCall, GenerateThen};
use makepad_asset_chat::wire::*;
use makepad_asset_client::json::{self, Value};
use makepad_asset_data::AssetRevisionId;

fn rev(byte: u8) -> AssetRevisionId {
    AssetRevisionId::from_bytes([byte; 32])
}

fn schema_required(schema: &Value) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_arr)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn schema_additional(schema: &Value) -> Option<bool> {
    schema.get("additionalProperties").and_then(Value::as_bool)
}

fn schema_minmax(schema: &Value) -> (i64, i64) {
    (
        schema.get("minimum").and_then(Value::as_i64).unwrap(),
        schema.get("maximum").and_then(Value::as_i64).unwrap(),
    )
}

#[test]
fn delta_chunks_are_utf8_safe_and_bounded() {
    let text = format!("{}{}", "x".repeat(MAX_DELTA_BYTES - 1), "éy");
    let chunks = split_delta_text(&text);
    assert!(chunks.len() >= 2);
    assert!(chunks.iter().all(|c| c.len() <= MAX_DELTA_BYTES));
    assert!(chunks.iter().all(|c| std::str::from_utf8(c.as_bytes()).is_ok()));
    assert_eq!(chunks.concat(), text);
}

#[test]
fn provider_kinds_are_explicit_and_closed() {
    // Exactly three providers; slugs are stable wire identifiers.
    assert_eq!(ProviderKind::FleetQwen.slug(), "fleet-qwen");
    assert_eq!(ProviderKind::OpenAi.slug(), "openai");
    assert_eq!(ProviderKind::Grok.slug(), "grok");
    assert_eq!(ProviderKind::from_slug("fleet-qwen"), Some(ProviderKind::FleetQwen));
    assert_eq!(ProviderKind::from_slug("openai"), Some(ProviderKind::OpenAi));
    assert_eq!(ProviderKind::from_slug("grok"), Some(ProviderKind::Grok));
    // No auto/fallback/Claude provider is representable on this wire.
    assert_eq!(ProviderKind::from_slug("auto"), None);
    assert_eq!(ProviderKind::from_slug("fallback"), None);
    assert_eq!(ProviderKind::from_slug("server-claude"), None);
    assert_eq!(ProviderKind::from_slug("claude"), None);
    assert_eq!(ProviderKind::from_slug(""), None);
    assert!(ProviderKind::OpenAi.uses_native_tools());
    assert!(ProviderKind::Grok.uses_native_tools());
    assert!(!ProviderKind::FleetQwen.uses_native_tools());
}

#[test]
fn availability_roundtrip_and_stable_encoding() {
    let a = ProviderAvailability::Available {
        model: "qwen3.8-27b".to_string(),
        detail: "http://10.0.0.217:8765".to_string(),
    };
    assert_eq!(
        a.encode().to_json(),
        r#"{"state":"available","model":"qwen3.8-27b","detail":"http://10.0.0.217:8765"}"#
    );
    assert_eq!(ProviderAvailability::decode(&a.encode()).unwrap(), a);

    let u = ProviderAvailability::Unavailable { reason: "no chat capability".to_string() };
    assert_eq!(u.encode().to_json(), r#"{"state":"unavailable","reason":"no chat capability"}"#);
    assert_eq!(ProviderAvailability::decode(&u.encode()).unwrap(), u);

    assert!(ProviderAvailability::decode(&json::obj(vec![("state", json::s("maybe"))])).is_err());
}

#[test]
fn message_bounds_are_enforced() {
    let ok = ChatMessage::new(ChatRole::User, "hello");
    assert!(ok.validate().is_ok());
    assert_eq!(ChatMessage::decode(&ok.encode()).unwrap(), ok);

    let empty = ChatMessage::new(ChatRole::User, "");
    assert!(empty.validate().is_err());

    let big = ChatMessage::new(ChatRole::User, "x".repeat(MAX_MESSAGE_BYTES + 1));
    assert!(big.validate().is_err());
    assert!(ChatMessage::decode(&big.encode()).is_err());
}

#[test]
fn attachment_binding_is_exact_revision_plus_role() {
    let a = AttachmentBinding { revision: rev(0x11), role: "source".to_string() };
    assert!(a.validate().is_ok());
    let decoded = AttachmentBinding::decode(&a.encode()).unwrap();
    assert_eq!(decoded, a);

    // Roles are bounded identifiers, not free text or paths.
    let bad = AttachmentBinding { revision: rev(0x11), role: "../etc".to_string() };
    assert!(bad.validate().is_err());
    let spacey = AttachmentBinding { revision: rev(0x11), role: "my role".to_string() };
    assert!(spacey.validate().is_err());

    // A malformed revision id is refused at decode.
    let forged = json::obj(vec![
        ("revision", json::s("arev_zz")),
        ("role", json::s("source")),
    ]);
    assert!(AttachmentBinding::decode(&forged).is_err());
}

/// The serving block is ADDITIVE on `delta`: a delta that knows nothing
/// encodes exactly the old two keys (so an old client sees the old event),
/// and a client that predates the block ignores it — pinned here by
/// decoding one WITH the block and checking the text survives untouched.
#[test]
fn serving_facts_are_additive_on_delta() {
    let bare = ChatEvent { seq: 7, body: ChatEventBody::Delta { text: "hi".into(), serving: None } };
    assert_eq!(bare.encode().to_json(), r#"{"seq":7,"type":"delta","text":"hi"}"#);

    let full = ChatEvent {
        seq: 8,
        body: ChatEventBody::Delta {
            text: "hi".into(),
            serving: Some(ServingFacts {
                gen_tokens: 128,
                lanes_active: Some(2),
                slots_total: Some(4), ..Default::default() }),
        },
    };
    let encoded = full.encode();
    assert_eq!(ChatEvent::decode(&encoded).unwrap(), full);
    // The old fields are byte-identical; only a new key was added.
    let json = encoded.to_json();
    assert!(json.contains(r#""text":"hi""#), "{json}");
    assert!(json.contains(r#""gen_tokens":128"#), "{json}");

    // Lanes are optional inside the block (a single-lane box says nothing).
    let no_lanes = ChatEvent {
        seq: 9,
        body: ChatEventBody::Delta {
            text: "x".into(),
            serving: Some(ServingFacts { gen_tokens: 1, lanes_active: None, slots_total: None, ..Default::default() }),
        },
    };
    assert_eq!(ChatEvent::decode(&no_lanes.encode()).unwrap(), no_lanes);
}

/// A cosmetic counter must never kill a live turn: garbage in the block
/// decodes as "no facts", and the delta still arrives.
#[test]
fn a_malformed_serving_block_never_fails_the_delta() {
    for junk in [
        json::s("nonsense"),
        Value::Obj(vec![]),
        json::obj(vec![("gen_tokens", json::s("many"))]),
    ] {
        let v = json::obj(vec![
            ("seq", Value::Int(1)),
            ("type", json::s("delta")),
            ("text", json::s("still here")),
            ("serving", junk),
        ]);
        let decoded = ChatEvent::decode(&v).expect("delta survives a bad serving block");
        match decoded.body {
            ChatEventBody::Delta { text, serving } => {
                assert_eq!(text, "still here");
                assert!(serving.is_none());
            }
            other => panic!("{other:?}"),
        }
    }
    // Implausible counters are clamped, not refused.
    let v = json::obj(vec![
        ("seq", Value::Int(1)),
        ("type", json::s("delta")),
        ("text", json::s("t")),
        (
            "serving",
            json::obj(vec![
                ("gen_tokens", Value::Int(i64::MAX)),
                ("lanes_active", Value::Int(9_000_000)),
                ("slots_total", Value::Int(9_000_000)),
            ]),
        ),
    ]);
    match ChatEvent::decode(&v).unwrap().body {
        ChatEventBody::Delta { serving: Some(s), .. } => {
            assert_eq!(s.gen_tokens, 10_000_000);
            assert_eq!(s.lanes_active, Some(1024));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn event_roundtrip_all_variants() {
    let events = vec![
        ChatEvent { seq: 0, body: ChatEventBody::Delta { text: "hi".to_string(), serving: None } },
        ChatEvent {
            seq: 1,
            body: ChatEventBody::ToolCall {
                id: "tc_1_1".to_string(),
                name: "asset.search".to_string(),
                args: json::obj(vec![("query", json::s("neon"))]),
            },
        },
        ChatEvent {
            seq: 2,
            body: ChatEventBody::ToolProgress {
                id: "tc_1_1".to_string(),
                permille: 500,
                note: "halfway".to_string(),
            },
        },
        ChatEvent {
            seq: 3,
            body: ChatEventBody::ToolResult {
                id: "tc_1_1".to_string(),
                outcome: ToolOutcome::Ok { value: json::obj(vec![("hits", Value::Arr(vec![]))]) },
            },
        },
        ChatEvent { seq: 4, body: ChatEventBody::Done },
        ChatEvent { seq: 5, body: ChatEventBody::Cancelled },
        ChatEvent {
            seq: 6,
            body: ChatEventBody::Error { code: "provider".to_string(), message: "x".to_string() },
        },
    ];
    for e in events {
        let round = ChatEvent::decode(&e.encode()).unwrap();
        assert_eq!(round, e);
    }
}

#[test]
fn tool_outcomes_are_typed_including_unavailable() {
    let outcomes = vec![
        ToolOutcome::Ok { value: json::obj(vec![("job", json::s("job_00000000000000000000000000000000"))]) },
        ToolOutcome::Unavailable { reason: "profile 'x' is not advertised".to_string() },
        ToolOutcome::Denied { what: "the asset server denied this operation".to_string() },
        ToolOutcome::Refused { what: "input not bound".to_string() },
        ToolOutcome::Failed { message: "timeout".to_string() },
    ];
    for o in outcomes {
        assert_eq!(ToolOutcome::decode(&o.encode()).unwrap(), o);
    }
    // Unknown tags fail closed.
    assert!(ToolOutcome::decode(&json::obj(vec![("outcome", json::s("shrug"))])).is_err());
}

#[test]
fn clamped_outcomes_always_decode() {
    // The entry-22 regression: a long honest refusal must arrive truncated,
    // never be refused wholesale as malformed by the receiving parser.
    let long = "refused: buildings are CRAMMED — ".repeat(40);
    assert!(long.len() > 512);
    for o in [
        ToolOutcome::Failed { message: long.clone() },
        ToolOutcome::Refused { what: long.clone() },
        ToolOutcome::Denied { what: long.clone() },
        ToolOutcome::Unavailable { reason: long.clone() },
    ] {
        // Unclamped, the receiver refuses it.
        assert!(ToolOutcome::decode(&o.encode()).is_err());
        let clamped = o.clamped();
        let back = ToolOutcome::decode(&clamped.encode()).expect("clamped must decode");
        assert_eq!(back, clamped);
        // The guidance survives (truncated, ellipsis-terminated).
        let msg = match &back {
            ToolOutcome::Failed { message } => message,
            ToolOutcome::Refused { what } | ToolOutcome::Denied { what } => what,
            ToolOutcome::Unavailable { reason } => reason,
            ToolOutcome::Ok { .. } => unreachable!(),
        };
        assert!(msg.starts_with("refused: buildings"));
        assert!(msg.ends_with('…'));
    }
    // Multi-byte boundary: a wall of em-dashes truncates on a char boundary.
    let dashes = "—".repeat(400);
    let c = ToolOutcome::Failed { message: dashes }.clamped();
    assert!(ToolOutcome::decode(&c.encode()).is_ok());
    // Short outcomes pass through untouched.
    let short = ToolOutcome::Failed { message: "timeout".into() };
    assert_eq!(short.clone().clamped(), short);
    // An oversized Ok value downgrades to the broker's own honest bound.
    let big = ToolOutcome::Ok {
        value: json::obj(vec![("text", json::s("x".repeat(17 * 1024)))]),
    };
    assert_eq!(
        big.clamped(),
        ToolOutcome::Failed { message: "tool result too large".to_string() }
    );
}

#[test]
fn permille_range_is_enforced_on_decode() {
    let mut v = ChatEvent {
        seq: 9,
        body: ChatEventBody::ToolProgress { id: "t".into(), permille: 1000, note: "n".into() },
    }
    .encode();
    // Force an out-of-range permille into the encoded form.
    if let Value::Obj(pairs) = &mut v {
        for (k, val) in pairs.iter_mut() {
            if k == "permille" {
                *val = Value::Int(1001);
            }
        }
    }
    assert!(ChatEvent::decode(&v).is_err());
}

/// No wire type can carry a credential: the schema has no such field, and
/// the encoded key sets are pinned here so one cannot appear unnoticed.
#[test]
fn wire_schema_has_no_credential_fields() {
    let sample_events = vec![
        ChatEvent { seq: 0, body: ChatEventBody::Delta { text: "t".into(), serving: None } },
        ChatEvent {
            seq: 1,
            body: ChatEventBody::ToolCall {
                id: "i".into(),
                name: "operation.create".into(),
                args: json::obj(vec![("prompt", json::s("p"))]),
            },
        },
        ChatEvent {
            seq: 2,
            body: ChatEventBody::ToolResult {
                id: "i".into(),
                outcome: ToolOutcome::Ok { value: Value::Obj(vec![]) },
            },
        },
        ChatEvent { seq: 3, body: ChatEventBody::Done },
    ];
    let mut all = String::new();
    for e in &sample_events {
        all.push_str(&e.encode().to_json());
    }
    all.push_str(
        &ProviderAvailability::Available { model: "m".into(), detail: "d".into() }
            .encode()
            .to_json(),
    );
    all.push_str(
        &AttachmentBinding { revision: rev(1), role: "source".into() }.encode().to_json(),
    );
    for forbidden in ["token", "secret", "api_key", "bearer", "mpat_", "authorization"] {
        assert!(
            !all.to_lowercase().contains(forbidden),
            "wire encoding contains credential-shaped key '{forbidden}': {all}"
        );
    }
}

#[test]
fn origin_audit_fields_are_not_on_tool_or_chat_wire() {
    // Origin.principal is local session metadata. It is not a tool argument
    // and is not recorded by the Asset Server in this foundation.
    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let call = ContentToolCall::parse(
        "operation.create",
        &json::obj(vec![
            ("kind", json::s("mesh.from_image.v1")),
            (
                "inputs",
                Value::Arr(vec![json::obj(vec![
                    ("slot", json::s("image")),
                    ("asset", json::s(asset.to_string())),
                    ("revision", json::s(rev(7).to_string())),
                    ("role", json::s("texture")),
                ])]),
            ),
        ]),
    )
    .unwrap();
    let encoded = tools::encode_args(&call).to_json();
    for forbidden in ["principal", "audit", "enqueued_by"] {
        assert!(
            !encoded.contains(forbidden),
            "create args must not carry {forbidden}: {encoded}"
        );
    }
}

// ------------------------------------------------------------- tool calls

#[test]
fn typed_tool_parse_accepts_the_allowlist() {
    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let call = ContentToolCall::parse(
        "operation.create",
        &json::obj(vec![
            ("kind", json::s("mesh.from_image.v1")),
            (
                "inputs",
                Value::Arr(vec![json::obj(vec![
                    ("slot", json::s("image")),
                    ("asset", json::s(asset.to_string())),
                    ("revision", json::s(rev(7).to_string())),
                    ("role", json::s("texture")),
                ])]),
            ),
            ("params", json::obj(vec![("seed", Value::Int(3))])),
        ]),
    )
    .unwrap();
    match &call {
        ContentToolCall::OperationCreate { kind, inputs, params, .. } => {
            assert_eq!(kind, "mesh.from_image.v1");
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].asset, asset);
            assert_eq!(inputs[0].revision, rev(7));
            assert_eq!(inputs[0].role, "texture");
            assert_eq!(params.get("seed").and_then(Value::as_i64), Some(3));
        }
        other => panic!("unexpected parse: {other:?}"),
    }
    // encode_args -> parse roundtrip holds for the typed form.
    let re = ContentToolCall::parse("operation.create", &tools::encode_args(&call)).unwrap();
    assert_eq!(re, call);
}

#[test]
fn canonicalize_json_sorts_nested_objects() {
    let a = json::obj(vec![
        ("z", json::s("1")),
        ("nested", json::obj(vec![("b", Value::Int(2)), ("a", Value::Int(1))])),
    ]);
    let b = json::obj(vec![
        ("nested", json::obj(vec![("a", Value::Int(1)), ("b", Value::Int(2))])),
        ("z", json::s("1")),
    ]);
    assert_eq!(
        tools::canonicalize_json(&a).to_json(),
        tools::canonicalize_json(&b).to_json()
    );
}

/// The alias-publication default is the SAFE expectation: omitting `expect`
/// means "the alias must not exist yet". A model can only overwrite an
/// existing head by spelling out `any` (or a compare-and-set `head`).
#[test]
fn alias_publication_defaults_to_absent_expectation() {
    use makepad_asset_chat::tools::{AliasExpectArg, PublicationArg};
    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let create = |publication: Value| {
        ContentToolCall::parse(
            "operation.create",
            &json::obj(vec![
                ("kind", json::s("mesh.from_image.v1")),
                (
                    "inputs",
                    Value::Arr(vec![json::obj(vec![
                        ("slot", json::s("image")),
                        ("asset", json::s(asset.to_string())),
                        ("revision", json::s(rev(7).to_string())),
                        ("role", json::s("texture")),
                    ])]),
                ),
                ("publication", publication),
            ]),
        )
    };
    let implicit = create(json::obj(vec![
        ("mode", json::s("publish_and_alias")),
        ("alias", json::s("gen/hero")),
    ]))
    .unwrap();
    match implicit {
        ContentToolCall::OperationCreate {
            publication: PublicationArg::PublishAndAlias { expect, .. },
            ..
        } => assert_eq!(expect, AliasExpectArg::Absent),
        other => panic!("unexpected parse: {other:?}"),
    }
    // Unconditional overwrite stays expressible, but only explicitly.
    let explicit = create(json::obj(vec![
        ("mode", json::s("publish_and_alias")),
        ("alias", json::s("gen/hero")),
        ("expect", json::s("any")),
    ]))
    .unwrap();
    match explicit {
        ContentToolCall::OperationCreate {
            publication: PublicationArg::PublishAndAlias { expect, .. },
            ..
        } => assert_eq!(expect, AliasExpectArg::Any),
        other => panic!("unexpected parse: {other:?}"),
    }
}

#[test]
fn tool_parse_fails_closed() {
    // Unknown tools are refusals — the allowlist is closed. Every removed
    // privileged surface stays unknown: no raw jobs, publish, or aliases.
    for gone in [
        "run_command",
        "write_file",
        "generate",
        "transform",
        "job_status",
        "await_job",
        "cancel_job",
        "choose_candidate",
        "publish_alias",
        "get_revision",
        "catalog_search",
        "asset_inspect",
    ] {
        assert!(
            ContentToolCall::parse(gone, &Value::Obj(vec![])).is_err(),
            "'{gone}' must not parse"
        );
    }

    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let input = |i: u8| {
        json::obj(vec![
            ("slot", json::s("image")),
            ("asset", json::s(asset.to_string())),
            ("revision", json::s(rev(i).to_string())),
            ("role", json::s("texture")),
        ])
    };

    // Operation input bounds.
    let too_many: Vec<Value> = (0..5).map(|i| input(i as u8)).collect();
    assert!(ContentToolCall::parse(
        "operation.create",
        &json::obj(vec![
            ("kind", json::s("mesh.from_image.v1")),
            ("inputs", Value::Arr(too_many)),
        ]),
    )
    .is_err());

    // UNKNOWN FIELDS refuse on the mutating surface — a model mistake can
    // never become a silently different operation.
    assert!(ContentToolCall::parse(
        "operation.create",
        &json::obj(vec![
            ("kind", json::s("mesh.from_image.v1")),
            ("inputs", Value::Arr(vec![input(1)])),
            ("owner", json::s("prin_someone")),
        ]),
    )
    .is_err());
    let mut sneaky = input(1);
    if let Value::Obj(pairs) = &mut sneaky {
        pairs.push(("path".to_string(), json::s("/etc/passwd")));
    }
    assert!(ContentToolCall::parse(
        "operation.create",
        &json::obj(vec![
            ("kind", json::s("mesh.from_image.v1")),
            ("inputs", Value::Arr(vec![sneaky])),
        ]),
    )
    .is_err());

    // Malformed ids never survive parsing.
    assert!(ContentToolCall::parse(
        "operation.get",
        &json::obj(vec![("operation", json::s("op_not_hex"))]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "operation.cancel",
        &json::obj(vec![("operation", json::s("rm -rf /"))]),
    )
    .is_err());

    // Limits are bounded.
    assert!(ContentToolCall::parse(
        "asset.search",
        &json::obj(vec![("query", json::s("x")), ("limit", Value::Int(9999))]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "operation.wait",
        &json::obj(vec![
            ("operation", json::s("op_00000000000000000000000000000000")),
            ("timeout_ms", Value::Int(600000)),
        ]),
    )
    .is_err());
}

#[test]
fn tool_parse_rejects_unknown_fields_and_wrong_types() {
    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let input = json::obj(vec![
        ("slot", json::s("image")),
        ("asset", json::s(asset.to_string())),
        ("revision", json::s(rev(1).to_string())),
        ("role", json::s("texture")),
    ]);
    let create = |extra: Vec<(&str, Value)>| {
        let mut pairs = vec![
            ("kind", json::s("mesh.from_image.v1")),
            ("inputs", Value::Arr(vec![input.clone()])),
        ];
        pairs.extend(extra);
        ContentToolCall::parse("operation.create", &json::obj(pairs))
    };

    assert!(ContentToolCall::parse("asset.search", &Value::Arr(vec![])).is_err());
    assert!(ContentToolCall::parse(
        "asset.search",
        &json::obj(vec![("query", json::s("x")), ("limit", json::s("10"))]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "asset.search",
        &json::obj(vec![("query", Value::Int(1))]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "asset.search",
        &json::obj(vec![("query", json::s("x")), ("extra", json::s("no"))]),
    )
    .is_err());

    assert!(ContentToolCall::parse("asset.inspect", &Value::Obj(vec![])).is_err());
    assert!(ContentToolCall::parse(
        "asset.inspect",
        &json::obj(vec![("asset", json::s(asset.to_string())), ("alias", json::s("gen/hero"))]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "asset.inspect",
        &json::obj(vec![("asset", Value::Int(1))]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "asset.inspect",
        &json::obj(vec![("asset", json::s(asset.to_string())), ("note", json::s("x"))]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "asset.inspect",
        &json::obj(vec![("asset", json::s(asset.to_string()))]),
    )
    .is_ok());

    assert!(ContentToolCall::parse(
        "operation.capabilities",
        &json::obj(vec![("extra", json::s("x"))]),
    )
    .is_err());

    assert!(create(vec![("publication", json::s("publish"))]).is_err());
    assert!(create(vec![("publication", json::obj(vec![("mode", json::s("nope"))]))]).is_err());
    assert!(create(vec![("publication", json::obj(vec![("alias", json::s("gen/hero"))]))]).is_err());
    assert!(create(vec![("idempotency_key", Value::Int(1))]).is_err());
    assert!(create(vec![("params", json::s("nope"))]).is_err());

    let mut bad_slot = input.clone();
    if let Value::Obj(pairs) = &mut bad_slot {
        for (k, v) in pairs.iter_mut() {
            if k == "slot" {
                *v = Value::Int(1);
            }
        }
    }
    assert!(ContentToolCall::parse(
        "operation.create",
        &json::obj(vec![
            ("kind", json::s("mesh.from_image.v1")),
            ("inputs", Value::Arr(vec![bad_slot])),
        ]),
    )
    .is_err());

    for (field, val) in [
        ("tier", Value::Int(1)),
        ("lod", json::s("1")),
        ("media", Value::Bool(true)),
    ] {
        let mut item = input.clone();
        if let Value::Obj(pairs) = &mut item {
            pairs.push((field.to_string(), val));
        }
        assert!(
            ContentToolCall::parse(
                "operation.create",
                &json::obj(vec![
                    ("kind", json::s("mesh.from_image.v1")),
                    ("inputs", Value::Arr(vec![item])),
                ]),
            )
            .is_err(),
            "{field} wrong type must refuse"
        );
    }

    assert!(ContentToolCall::parse(
        "operation.wait",
        &json::obj(vec![
            ("operation", json::s("op_00000000000000000000000000000000")),
            ("timeout_ms", json::s("60000")),
        ]),
    )
    .is_err());
    assert!(ContentToolCall::parse(
        "operation.wait",
        &json::obj(vec![
            ("operation", json::s("op_00000000000000000000000000000000")),
            ("after", json::s("0")),
        ]),
    )
    .is_err());
}

#[test]
fn publication_rejects_incompatible_field_combinations() {
    let asset = makepad_asset_data::AssetId::from_bytes([5; 16]);
    let input = json::obj(vec![
        ("slot", json::s("image")),
        ("asset", json::s(asset.to_string())),
        ("revision", json::s(rev(1).to_string())),
        ("role", json::s("texture")),
    ]);
    let create = |publication: Value| {
        ContentToolCall::parse(
            "operation.create",
            &json::obj(vec![
                ("kind", json::s("mesh.from_image.v1")),
                ("inputs", Value::Arr(vec![input.clone()])),
                ("publication", publication),
            ]),
        )
    };
    assert!(create(json::obj(vec![
        ("mode", json::s("publish")),
        ("alias", json::s("gen/hero")),
    ]))
    .is_err());
    assert!(create(json::obj(vec![
        ("mode", json::s("publish")),
        ("expect", json::s("any")),
    ]))
    .is_err());
    assert!(create(json::obj(vec![
        ("mode", json::s("publish_and_alias")),
        ("alias", json::s("gen/hero")),
        ("expect", json::s("any")),
        ("expect_head", json::s(rev(2).to_string())),
    ]))
    .is_err());
    assert!(create(json::obj(vec![
        ("mode", json::s("publish_and_alias")),
        ("alias", json::s("gen/hero")),
        ("expect", json::s("absent")),
        ("expect_head", json::s(rev(2).to_string())),
    ]))
    .is_err());
}

#[test]
fn public_error_sanitize_redacts_keys_and_caps() {
    assert_eq!(
        sanitize_public_error("invalid api key sk-secret-ABC"),
        "provider error"
    );
    assert!(!sanitize_public_error("Bearer leaked").contains("Bearer"));
    let long = "e".repeat(MAX_PUBLIC_ERROR_BYTES + 40);
    assert!(sanitize_public_error(&long).len() <= MAX_PUBLIC_ERROR_BYTES);
    assert!(!sanitize_public_error("line\nwith\rCR").contains('\n'));
    let almost = format!("{}é", "x".repeat(MAX_PUBLIC_ERROR_BYTES - 1));
    let capped = sanitize_public_error(&almost);
    assert!(capped.len() <= MAX_PUBLIC_ERROR_BYTES);
    assert!(!capped.ends_with('é'));
    let exact = format!("{}é", "x".repeat(MAX_PUBLIC_ERROR_BYTES - 2));
    let kept = sanitize_public_error(&exact);
    assert_eq!(kept.len(), MAX_PUBLIC_ERROR_BYTES);
    assert!(kept.ends_with('é'));
}

#[test]
fn chat_event_error_decode_uses_public_error_cap() {
    let ok = ChatEvent {
        seq: 1,
        body: ChatEventBody::Error {
            code: "provider".into(),
            message: "e".repeat(MAX_PUBLIC_ERROR_BYTES),
        },
    };
    assert_eq!(ChatEvent::decode(&ok.encode()).unwrap(), ok);
    let over = ChatEvent {
        seq: 1,
        body: ChatEventBody::Error {
            code: "provider".into(),
            message: "e".repeat(MAX_PUBLIC_ERROR_BYTES + 1),
        },
    };
    assert!(ChatEvent::decode(&over.encode()).is_err());
}

#[test]
fn tool_outcome_validate_matches_decode_field_caps() {
    let ok512 = ToolOutcome::Failed { message: "m".repeat(512) };
    assert!(ok512.validate().is_ok());
    assert!(ToolOutcome::decode(&ok512.encode()).is_ok());
    let over = ToolOutcome::Failed { message: "m".repeat(513) };
    assert!(over.validate().is_err());
    assert!(ToolOutcome::decode(&over.encode()).is_err());
    let reason = ToolOutcome::Unavailable { reason: "r".repeat(513) };
    assert!(reason.validate().is_err());
    let what = ToolOutcome::Denied { what: "w".repeat(513) };
    assert!(what.validate().is_err());
    let refused = ToolOutcome::Refused { what: "w".repeat(512) };
    assert!(refused.validate().is_ok());
}

#[test]
fn tool_outcome_and_event_enforce_object_shape_and_size() {
    assert!(ToolOutcome::Ok { value: Value::Arr(vec![]) }.validate().is_err());
    let ok = ToolOutcome::Ok { value: json::obj(vec![("hits", Value::Arr(vec![]))]) };
    assert!(ok.validate().is_ok());
    assert!(ToolOutcome::decode(&ok.encode()).is_ok());

    let huge = ToolOutcome::Ok {
        value: json::obj(vec![("blob", json::s("x".repeat(MAX_TOOL_JSON_BYTES)))]),
    };
    assert!(huge.validate().is_err());
    assert!(ToolOutcome::decode(&huge.encode()).is_err());

    let ev = ChatEvent {
        seq: 1,
        body: ChatEventBody::ToolCall {
            id: "tc".into(),
            name: "asset.search".into(),
            args: Value::Arr(vec![]),
        },
    };
    assert!(ChatEvent::decode(&ev.encode()).is_err());

    let ev = ChatEvent {
        seq: 1,
        body: ChatEventBody::ToolCall {
            id: "tc".into(),
            name: "asset.search".into(),
            args: json::obj(vec![("query", json::s("x".repeat(MAX_TOOL_JSON_BYTES)))]),
        },
    };
    assert!(ChatEvent::decode(&ev.encode()).is_err());
}

// ---------------------------------------------------------- text contract

#[test]
fn toolcall_extraction_contract() {
    // Plain text: no call.
    assert_eq!(toolcall::extract("just chatting"), Extract::None);

    // A call on its own line after visible text.
    let text = "Searching now.\n<<tool>>{\"name\":\"catalog_search\",\"args\":{\"query\":\"neon\"}}\ntrailing chatter";
    match toolcall::extract(text) {
        Extract::Call { clean, name, args } => {
            assert_eq!(clean, "Searching now.");
            assert_eq!(name, "catalog_search");
            assert_eq!(args.get("query").and_then(Value::as_str), Some("neon"));
        }
        other => panic!("expected call, got {other:?}"),
    }

    // Marker mid-line is prose, not a call.
    assert_eq!(toolcall::extract("the marker <<tool>> is literal here"), Extract::None);

    // A mid-line draft must not hide a later real line-start call.
    match toolcall::extract(
        "draft `<<tool>>{\"name\":\"image.generate\",\"args\":{\"prompt\":\"no\"}}`\n\
         <<tool>>{\"name\":\"video.generate\",\"args\":{\"prompt\":\"yes\"}}",
    ) {
        Extract::Call { clean, name, args } => {
            assert!(clean.contains("draft"));
            assert_eq!(name, "video.generate");
            assert_eq!(args.get("prompt").and_then(Value::as_str), Some("yes"));
        }
        other => panic!("expected later line-start call, got {other:?}"),
    }

    // Thinking dump: execute the call AFTER </think>, not a draft inside.
    let think_dump = "\
The user wants a video.\n\
` <<tool>>{\"name\":\"video.generate\",\"args\":{\"prompt\":\"draft\",\"frames\":58}} `\n\
</think>\n\
\n\
<<tool>>{\"name\":\"video.generate\",\"args\":{\"prompt\":\"a dancing unicorn\",\"frames\":58}}";
    match toolcall::extract(think_dump) {
        Extract::Call { clean, name, args } => {
            assert!(!clean.contains("The user wants"), "thinking leaked into clean: {clean:?}");
            assert_eq!(name, "video.generate");
            assert_eq!(args.get("prompt").and_then(Value::as_str), Some("a dancing unicorn"));
            assert_eq!(args.get("frames").and_then(Value::as_i64), Some(58));
        }
        other => panic!("expected post-think call, got {other:?}"),
    }
    let split = toolcall::split_thinking(think_dump);
    assert!(split.think_closed);
    assert!(split.thinking.contains("The user wants a video"));
    assert!(split.visible.starts_with("<<tool>>"));
    assert_eq!(toolcall::strip_marker(think_dump), "");

    // Malformed JSON is a typed refusal the model can react to.
    match toolcall::extract("<<tool>>{not json}") {
        Extract::Malformed { reason, .. } => assert!(reason.contains("JSON")),
        other => panic!("expected malformed, got {other:?}"),
    }
    match toolcall::extract("<<tool>>{\"args\":{}}") {
        Extract::Malformed { reason, .. } => assert!(reason.contains("name")),
        other => panic!("expected malformed, got {other:?}"),
    }

    // Model forgot the newline and kept talking after the JSON object.
    match toolcall::extract(
        "<<tool>>{\"name\":\"fleet.introspect\",\"args\":{\"domain\":\"image\"}}We have three",
    ) {
        Extract::Call { clean, name, args } => {
            assert!(clean.is_empty());
            assert_eq!(name, "fleet.introspect");
            assert_eq!(args.get("domain").and_then(Value::as_str), Some("image"));
        }
        other => panic!("expected call despite trailing prose, got {other:?}"),
    }
    assert_eq!(
        toolcall::strip_marker("hello\n<<tool>>{\"name\":\"x\",\"args\":{}}\nmore"),
        "hello"
    );
}

#[test]
fn system_rendering_lists_allowlist_and_capabilities() {
    let rendered =
        toolcall::render_system(&tools::definitions(), "Registered operations: none");
    for name in [
        "image.generate",
        "video.generate",
        "audio.generate",
        "speech.generate",
        "music.generate",
        "mesh.generate",
        "world.generate",
        "character.generate",
        "defaults.get",
        "defaults.set",
        "fleet.introspect",
        "asset.search",
        "asset.inspect",
        "operation.capabilities",
        "operation.create",
        "operation.get",
        "operation.wait",
        "operation.cancel",
        "operation.retry",
        "llm.consult",
    ] {
        assert!(rendered.contains(name), "system prompt missing tool {name}");
    }
    // The privileged verbs never appear as tools in the prompt.
    for gone in ["publish_alias", "choose_candidate", "job_status", "\n- generate:"] {
        assert!(!rendered.contains(gone), "system prompt leaks removed tool {gone}");
    }
    assert!(rendered.contains("Registered operations: none"));
    assert!(rendered.contains("Never invent asset or revision ids"));

    let native = tools::render_native_system(&tools::definitions(), "Registered operations: none");
    assert!(!native.contains("<<tool>>"), "native prompt must not teach the marker");
    for api in [
        "image_generate",
        "video_generate",
        "audio_generate",
        "speech_generate",
        "music_generate",
        "mesh_generate",
        "world_generate",
        "character_generate",
        "defaults_get",
        "defaults_set",
        "fleet_introspect",
        "asset_search",
        "asset_inspect",
        "operation_capabilities",
        "operation_create",
        "operation_get",
        "operation_wait",
        "operation_cancel",
        "operation_retry",
        "llm_consult",
    ] {
        assert!(native.contains(api), "native prompt missing {api}");
    }

    let atts = toolcall::render_attachments(&[AttachmentBinding {
        revision: rev(0x22),
        role: "source".into(),
    }]);
    assert!(atts.contains(&rev(0x22).to_string()));
    assert!(atts.contains("source"));
}

#[test]
fn native_api_names_map_fail_closed_and_schemas_cover_allowlist() {
    let defs = tools::definitions();
    assert_eq!(defs.len(), 20);
    for d in &defs {
        assert_eq!(tools::canonical_from_api_name(d.api_name), Some(d.name));
        assert!(d.api_name.bytes().all(|b| b != b'.'), "api name must be underscore form");
        match &d.parameters {
            Value::Obj(pairs) => {
                assert!(pairs.iter().any(|(k, v)| k == "type" && v.as_str() == Some("object")));
                assert!(pairs.iter().any(|(k, _)| k == "properties"));
            }
            other => panic!("parameters must be a schema object, got {other:?}"),
        }
        match d.name {
            "asset.search" => {
                assert_eq!(schema_required(&d.parameters), vec!["query"]);
                assert_eq!(schema_additional(&d.parameters), Some(false));
                assert_eq!(
                    schema_minmax(d.parameters.get("properties").unwrap().get("limit").unwrap()),
                    (1, 25)
                );
            }
            "asset.inspect" => {
                assert_eq!(schema_additional(&d.parameters), Some(false));
                assert_eq!(d.parameters.get("minProperties").and_then(Value::as_i64), Some(1));
                assert_eq!(d.parameters.get("maxProperties").and_then(Value::as_i64), Some(1));
            }
            "operation.create" => {
                assert_eq!(schema_required(&d.parameters), vec!["kind", "inputs"]);
                assert_eq!(schema_additional(&d.parameters), Some(false));
                let inputs = d.parameters.get("properties").unwrap().get("inputs").unwrap();
                assert_eq!(inputs.get("minItems").and_then(Value::as_i64), Some(1));
                assert_eq!(
                    inputs.get("maxItems").and_then(Value::as_i64),
                    Some(MAX_TRANSFORM_INPUTS as i64)
                );
                let item = inputs.get("items").unwrap();
                assert_eq!(schema_additional(item), Some(false));
                assert_eq!(schema_required(item), vec!["asset", "revision", "role"]);
                let item_props = item.get("properties").expect("input item properties");
                for key in ["slot", "asset", "revision", "role", "tier", "lod", "media"] {
                    assert!(item_props.get(key).is_some(), "missing input property {key}");
                }
                for ident in ["slot", "role", "tier", "media"] {
                    assert_eq!(
                        item_props.get(ident).unwrap().get("pattern").and_then(Value::as_str),
                        Some(r"^[a-z0-9_-]{1,32}$"),
                        "{ident} schema must match ident_ok"
                    );
                }
                assert_eq!(
                    item_props.get("asset").unwrap().get("maxLength").and_then(Value::as_i64),
                    Some(64)
                );
                assert_eq!(
                    item_props.get("revision").unwrap().get("maxLength").and_then(Value::as_i64),
                    Some(80)
                );
                let pub_schema = d.parameters.get("properties").unwrap().get("publication").unwrap();
                assert_eq!(schema_required(pub_schema), vec!["mode"]);
                assert_eq!(schema_additional(pub_schema), Some(false));
                let pub_mode = pub_schema.get("properties").unwrap().get("mode").unwrap();
                let modes: Vec<&str> = pub_mode
                    .get("enum")
                    .and_then(Value::as_arr)
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                assert_eq!(modes, vec!["publish", "publish_and_alias"]);
                assert!(d.parameters.get("properties").unwrap().get("params").is_some());
            }
            "operation.wait" => {
                assert_eq!(schema_required(&d.parameters), vec!["operation"]);
                assert_eq!(
                    schema_minmax(
                        d.parameters.get("properties").unwrap().get("timeout_ms").unwrap()
                    ),
                    (1, 120_000)
                );
            }
            "fleet.introspect" => {
                assert_eq!(schema_additional(&d.parameters), Some(false));
                assert!(d.parameters.get("properties").unwrap().get("domain").is_some());
            }
            "image.generate" => {
                assert_eq!(schema_required(&d.parameters), vec!["prompt"]);
                let then = d.parameters.get("properties").unwrap().get("then").unwrap();
                let slugs: Vec<&str> = then
                    .get("enum")
                    .and_then(Value::as_arr)
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                assert_eq!(slugs, GenerateThen::SLUGS);
            }
            "video.generate" | "audio.generate" | "speech.generate" | "music.generate"
            | "mesh.generate" | "world.generate" | "character.generate" => {
                assert_eq!(schema_required(&d.parameters), vec!["prompt"]);
            }
            _ => {}
        }
    }
    assert_eq!(
        ContentToolCall::parse("fleet.introspect", &json::obj(vec![("domain", json::s("image"))]))
            .unwrap(),
        ContentToolCall::FleetIntrospect { domain: Some("image".into()) }
    );
    assert_eq!(
        ContentToolCall::parse("defaults.get", &Value::Obj(vec![])).unwrap(),
        ContentToolCall::DefaultsGet
    );
    assert_eq!(
        ContentToolCall::parse(
            "defaults.set",
            &json::obj(vec![
                ("image_model", json::s("flux1-dev")),
                ("width", Value::Int(1024)),
                ("height", Value::Int(1024)),
                ("steps", Value::Int(20)),
                ("then", json::s("mesh")),
            ]),
        )
        .unwrap(),
        ContentToolCall::DefaultsSet {
            image_model: Some("flux1-dev".into()),
            width: Some(1024),
            height: Some(1024),
            steps: Some(20),
            then: Some(GenerateThen::Mesh),
        }
    );
    assert_eq!(
        ContentToolCall::parse(
            "video.generate",
            &json::obj(vec![
                ("prompt", json::s("trawler cutting through fog")),
                ("frames", Value::Int(39)),
            ]),
        )
        .unwrap(),
        ContentToolCall::VideoGenerate {
            prompt: "trawler cutting through fog".into(),
            model: None,
            width: None,
            height: None,
            frames: Some(39),
            steps: None,
        }
    );
    assert_eq!(
        ContentToolCall::parse(
            "image.generate",
            &json::obj(vec![
                ("prompt", json::s("rusty trawler")),
                ("then", json::s("video")),
            ]),
        )
        .unwrap(),
        ContentToolCall::ImageGenerate {
            prompt: "rusty trawler".into(),
            then: Some(GenerateThen::Video),
            model: None,
            width: None,
            height: None,
            steps: None,
        }
    );
    for (name, kind) in [
        ("audio.generate", "AudioGenerate"),
        ("speech.generate", "SpeechGenerate"),
        ("music.generate", "MusicGenerate"),
        ("mesh.generate", "MeshGenerate"),
        ("world.generate", "WorldGenerate"),
        ("character.generate", "CharacterGenerate"),
    ] {
        let call = ContentToolCall::parse(name, &json::obj(vec![("prompt", json::s("x"))])).unwrap();
        assert_eq!(call.name(), name, "{kind}");
        assert_eq!(
            ContentToolCall::parse(name, &tools::encode_args(&call)).unwrap(),
            call,
            "{kind} encode/parse"
        );
    }
    assert!(ContentToolCall::parse(
        "image.generate",
        &json::obj(vec![("prompt", json::s("x")), ("then", json::s("song"))]),
    )
    .is_err());
    assert_eq!(tools::canonical_from_api_name("video_generate"), Some("video.generate"));
    assert_eq!(tools::canonical_from_api_name("character_generate"), Some("character.generate"));
    assert_eq!(tools::canonical_from_api_name("fleet_introspect"), Some("fleet.introspect"));
    assert_eq!(tools::canonical_from_api_name("llm_consult"), Some("llm.consult"));
    assert_eq!(
        ContentToolCall::parse(
            "llm.consult",
            &json::obj(vec![
                ("task", json::s("level")),
                ("prompt", json::s("draft a quarry arena")),
                ("provider", json::s("grok")),
            ]),
        )
        .unwrap(),
        ContentToolCall::LlmConsult {
            task: makepad_asset_chat::ConsultTask::Level,
            prompt: "draft a quarry arena".into(),
            provider: Some(makepad_asset_chat::ProviderKind::Grok),
        }
    );
    assert!(ContentToolCall::parse(
        "llm.consult",
        &json::obj(vec![
            ("task", json::s("level")),
            ("prompt", json::s("x")),
            ("provider", json::s("fleet-qwen")),
        ]),
    )
    .is_err());
    assert_eq!(tools::canonical_from_api_name("asset.search"), None);
    assert_eq!(tools::canonical_from_api_name("run_command"), None);
    let payload = tools::native_tools_payload();
    let tools_arr = payload.as_arr().expect("tools array");
    assert_eq!(tools_arr.len(), 20);
    for t in tools_arr {
        assert_eq!(t.get("type").and_then(Value::as_str), Some("function"));
        assert_eq!(t.get("strict").and_then(Value::as_bool), Some(false));
        assert!(t.get("name").and_then(Value::as_str).is_some());
        assert!(t.get("parameters").is_some());
    }
}
