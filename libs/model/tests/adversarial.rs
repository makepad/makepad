//! Adversarial checks of document transactions, history, codec and the
//! strict service protocol. Foundation operations only; rig/texture coverage
//! is limited to protocol rejection and rollback through existing apply.
use makepad_model::*;
use makepad_model::json::{self, Value};
use std::cell::Cell;

fn cube(object: &str) -> Operation {
    Operation::Cube {
        object: object.into(),
        size: [1., 1., 1.],
    }
}
fn plane(object: &str) -> Operation {
    Operation::Plane {
        object: object.into(),
        size: [2., 2.],
    }
}
fn apply(doc: &mut Document, id: &str, operations: Vec<Operation>) -> Applied {
    doc.apply(
        Transaction {
            request_id: id.into(),
            expected: doc.head(),
            operations,
        },
        None,
    )
    .unwrap()
}
fn bytes(doc: &Document) -> Vec<u8> {
    doc.to_bytes(None).unwrap()
}
fn arr<const N: usize>(v: [f64; N]) -> Value {
    Value::Arr(v.into_iter().map(Value::F64).collect())
}
fn sid(v: u64) -> Value {
    json::s(v.to_string())
}

#[test]
fn failed_second_operation_rolls_back_the_whole_transaction() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "body", vec![cube("body")]);
    let snapshot = bytes(&doc);
    let head = doc.head();
    let err = doc
        .apply(
            Transaction {
                request_id: "partial".into(),
                expected: head,
                operations: vec![
                    plane("ground"),
                    Operation::DeleteObject {
                        object: "absent".into(),
                    },
                ],
            },
            None,
        )
        .unwrap_err();
    assert!(matches!(err, Error::MissingObject(_)), "{err:?}");
    assert_eq!(bytes(&doc), snapshot);
    assert_eq!(doc.head(), head);
    assert!(doc.object("ground").is_none());
    assert!(doc.object("body").is_some());
}

#[test]
fn cancellation_midway_does_not_commit() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "body", vec![cube("body")]);
    let snapshot = bytes(&doc);
    let head = doc.head();
    let calls = Cell::new(0u32);
    let cancel = || {
        let n = calls.get() + 1;
        calls.set(n);
        n > 4
    };
    let err = doc
        .apply(
            Transaction {
                request_id: "cancelled".into(),
                expected: head,
                operations: vec![plane("ground"), cube("other")],
            },
            Some(&cancel),
        )
        .unwrap_err();
    assert!(
        matches!(err, Error::Mesh(mesh::MeshError::Cancelled)),
        "{err:?}"
    );
    assert_eq!(bytes(&doc), snapshot);
    assert_eq!(doc.head(), head);
}

#[test]
fn undo_restores_content_hash_but_generation_is_monotonic() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let empty = doc.head();
    assert_eq!(empty.generation, 0);
    let first = apply(&mut doc, "body", vec![cube("body")]);
    assert_eq!(first.committed.generation, 1);
    assert_ne!(first.committed.content, empty.content);
    let undone = doc.undo(doc.head(), None).unwrap();
    assert_eq!(undone.content, empty.content);
    assert!(undone.generation > first.committed.generation);
    assert_ne!(undone.generation, empty.generation);
    // ABA: a writer holding the original empty head must not land on the
    // restored content.
    let stale = doc
        .apply(
            Transaction {
                request_id: "stale-empty".into(),
                expected: empty,
                operations: vec![plane("ground")],
            },
            None,
        )
        .unwrap_err();
    assert!(
        matches!(stale, Error::StaleHead { expected, actual } if expected == empty && actual == undone),
        "{stale:?}"
    );
    let redone = doc.redo(undone, None).unwrap();
    assert_eq!(redone.content, first.committed.content);
    assert!(redone.generation > undone.generation);
}

#[test]
fn history_branch_discards_redo_and_keeps_old_receipt() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let tx = Transaction {
        request_id: "body".into(),
        expected: doc.head(),
        operations: vec![cube("body")],
    };
    let first = doc.apply(tx.clone(), None).unwrap();
    let after_cube = bytes(&doc);
    let undone = doc.undo(doc.head(), None).unwrap();
    apply(&mut doc, "branch", vec![plane("ground")]);
    assert!(doc.object("body").is_none());
    assert!(doc.object("ground").is_some());
    assert!(doc.redo(doc.head(), None).is_err());
    let replay = doc.apply(tx, None).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.committed, first.committed);
    assert_eq!(replay.current, doc.head());
    assert_ne!(replay.current, first.committed);
    assert_ne!(bytes(&doc), after_cube);
    assert_eq!(undone.content, Document::new(Limits::default()).unwrap().head().content);
}

#[test]
fn request_retry_is_idempotent_across_canonical_roundtrip() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let tx = Transaction {
        request_id: "retry-me".into(),
        expected: doc.head(),
        operations: vec![cube("body")],
    };
    let first = doc.apply(tx.clone(), None).unwrap();
    let encoded = bytes(&doc);
    let mut reopened = Document::from_bytes(&encoded, Limits::default(), None).unwrap();
    assert_eq!(bytes(&reopened), encoded);
    let replay = reopened.apply(tx.clone(), None).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.committed, first.committed);
    assert_eq!(bytes(&reopened), encoded);
    let mut different = tx.clone();
    different.operations = vec![plane("ground")];
    assert_eq!(
        reopened.apply(different, None).unwrap_err(),
        Error::RequestIdReused
    );
    assert_eq!(bytes(&reopened), encoded);
}

#[test]
fn same_request_id_with_moved_expected_is_not_a_retry() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let tx = Transaction {
        request_id: "same".into(),
        expected: doc.head(),
        operations: vec![cube("body")],
    };
    doc.apply(tx.clone(), None).unwrap();
    let mut moved = tx;
    moved.expected = doc.head();
    assert_eq!(doc.apply(moved, None).unwrap_err(), Error::RequestIdReused);
}

#[test]
fn request_id_is_not_recyclable_after_receipt_eviction() {
    let mut limits = Limits::default();
    limits.max_receipts = 1;
    let mut doc = Document::new(limits).unwrap();
    apply(&mut doc, "r1", vec![cube("body")]);
    apply(&mut doc, "r2", vec![plane("ground")]);
    let snapshot = bytes(&doc);
    let err = doc.apply(
        Transaction {
            request_id: "r1".into(),
            expected: doc.head(),
            operations: vec![cube("other")],
        },
        None,
    );
    // Idempotency receipts are a bounded window. Recycling an evicted request
    // ID would duplicate an edit under a previously acknowledged identity.
    assert!(
        matches!(err, Err(Error::RequestIdReused)),
        "evicted request id was recyclable: {err:?}"
    );
    assert_eq!(bytes(&doc), snapshot);
}

#[test]
fn checkpoint_preserves_receipts_and_clears_undo() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let tx = Transaction {
        request_id: "body".into(),
        expected: doc.head(),
        operations: vec![cube("body")],
    };
    let first = doc.apply(tx.clone(), None).unwrap();
    doc.checkpoint(doc.head()).unwrap();
    assert_eq!(doc.history_position(), (0, 0));
    assert!(doc.undo(doc.head(), None).is_err());
    let replay = doc.apply(tx, None).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.committed, first.committed);
}

#[test]
fn import_of_truncated_mesh_rolls_back() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "body", vec![cube("body")]);
    let good = doc.object("body").unwrap().to_bytes(&mut mesh::Context::default()).unwrap();
    let snapshot = bytes(&doc);
    let err = doc
        .apply(
            Transaction {
                request_id: "bad-import".into(),
                expected: doc.head(),
                operations: vec![Operation::ImportMesh {
                    object: "broken".into(),
                    source: good[..good.len() / 2].to_vec(),
                }],
            },
            None,
        )
        .unwrap_err();
    assert!(
        matches!(err, Error::Mesh(mesh::MeshError::CorruptData(_)) | Error::Mesh(mesh::MeshError::Budget { .. })),
        "{err:?}"
    );
    assert_eq!(bytes(&doc), snapshot);
    assert!(doc.object("broken").is_none());
}

#[test]
fn hostile_and_truncated_document_sources_are_refused() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "body", vec![cube("body")]);
    apply(&mut doc, "ground", vec![plane("ground")]);
    let encoded = bytes(&doc);
    let limits = Limits::default();
    for n in [0usize, 1, 8, 12, encoded.len() / 3, encoded.len() - 1] {
        assert!(
            Document::from_bytes(&encoded[..n], limits.clone(), None).is_err(),
            "accepted truncated document length {n}"
        );
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(Document::from_bytes(&trailing, limits.clone(), None).is_err());
    let mut bad_magic = encoded.clone();
    bad_magic[0] ^= 0x5a;
    assert!(Document::from_bytes(&bad_magic, limits.clone(), None).is_err());
    let mut tiny = limits.clone();
    tiny.max_source_bytes = 16;
    assert!(matches!(
        Document::from_bytes(&encoded, tiny, None),
        Err(Error::Budget("source bytes"))
    ));
    // Swap the two object records so the name order is no longer canonical.
    let mut shuffled = encoded.clone();
    if let Some(a) = find_ascii(&shuffled, b"body") {
        if let Some(b) = find_ascii(&shuffled, b"ground") {
            if a != b {
                shuffled[a] = b'z';
                assert!(Document::from_bytes(&shuffled, limits, None).is_err());
            }
        }
    }
}

fn find_ascii(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn engine_rejects_unknown_and_duplicate_fields() {
    let mut engine = Engine::new(Limits::default());
    engine.open("doc", None, None).unwrap();
    let expected = head_json(engine.document("doc").unwrap().head());
    let extra = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("r")),
        ("expected", expected.clone()),
        ("operations", Value::Arr(vec![json::obj(vec![
            ("op", json::s("cube")),
            ("object", json::s("body")),
            ("size", arr([1., 1., 1.])),
        ])])),
        ("surprise", Value::Bool(true)),
    ]);
    assert!(engine.execute("model.apply", &extra, None).is_err());

    let duplicate = Value::Obj(vec![
        ("document".into(), json::s("doc")),
        ("request_id".into(), json::s("r")),
        ("expected".into(), expected.clone()),
        ("operations".into(), Value::Arr(vec![json::obj(vec![
            ("op", json::s("cube")),
            ("object", json::s("body")),
            ("size", arr([1., 1., 1.])),
        ])])),
        ("document".into(), json::s("doc")),
    ]);
    assert!(engine.execute("model.apply", &duplicate, None).is_err());

    let parsed = json::parse(
        br#"{"document":"doc","request_id":"r","expected":{"generation":"0","content":"00"},"extra":1}"#,
    );
    assert!(parsed.is_err() || engine.execute("model.apply", &parsed.unwrap(), None).is_err());
}

#[test]
fn engine_rejects_unknown_operation_and_nested_unknown_fields() {
    let mut engine = Engine::new(Limits::default());
    engine.open("doc", None, None).unwrap();
    let expected = head_json(engine.document("doc").unwrap().head());
    let unknown_op = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("r")),
        ("expected", expected.clone()),
        ("operations", Value::Arr(vec![json::obj(vec![
            ("op", json::s("bevel")),
            ("object", json::s("body")),
        ])])),
    ]);
    assert!(engine.execute("model.apply", &unknown_op, None).is_err());
    let nested = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("r")),
        ("expected", expected),
        ("operations", Value::Arr(vec![json::obj(vec![
            ("op", json::s("cube")),
            ("object", json::s("body")),
            ("size", arr([1., 1., 1.])),
            ("color", arr([1., 0., 0.])),
        ])])),
    ]);
    assert!(engine.execute("model.apply", &nested, None).is_err());
}

#[test]
fn stable_ids_and_generations_are_precise_decimal_strings() {
    let mut engine = Engine::new(Limits::default());
    engine.open("doc", None, None).unwrap();
    let expected = head_json(engine.document("doc").unwrap().head());
    engine
        .execute(
            "model.apply",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("request_id", json::s("make")),
                ("expected", expected),
                (
                    "operations",
                    Value::Arr(vec![json::obj(vec![
                        ("op", json::s("cube")),
                        ("object", json::s("body")),
                        ("size", arr([1., 1., 1.])),
                    ])]),
                ),
            ]),
            None,
        )
        .unwrap();
    let head = engine.document("doc").unwrap().head();
    assert!(head_json(head).get("generation").unwrap().as_str().is_some());
    let face = engine.document("doc").unwrap().object("body").unwrap().faces()[0]
        .id
        .0;

    // JSON numbers are refused even when they fit in i64.
    let numeric_id = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("numeric")),
        ("expected", head_json(head)),
        (
            "operations",
            Value::Arr(vec![json::obj(vec![
                ("op", json::s("extrude")),
                ("object", json::s("body")),
                ("face", Value::Int(face as i64)),
                ("offset", arr([0., 0., 1.])),
            ])]),
        ),
    ]);
    assert!(engine.execute("model.apply", &numeric_id, None).is_err());

    let leading_zero = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("zero")),
        ("expected", head_json(head)),
        (
            "operations",
            Value::Arr(vec![json::obj(vec![
                ("op", json::s("extrude")),
                ("object", json::s("body")),
                ("face", json::s(format!("0{face}"))),
                ("offset", arr([0., 0., 1.])),
            ])]),
        ),
    ]);
    assert!(engine.execute("model.apply", &leading_zero, None).is_err());

    // Values above JSON number precision must round-trip as the exact u64.
    let past_mantissa = 9007199254740993u64; // 2^53 + 1
    let large = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("large")),
        ("expected", head_json(head)),
        (
            "operations",
            Value::Arr(vec![json::obj(vec![
                ("op", json::s("extrude")),
                ("object", json::s("body")),
                ("face", sid(past_mantissa)),
                ("offset", arr([0., 0., 1.])),
            ])]),
        ),
    ]);
    match engine.execute("model.apply", &large, None) {
        Err(Error::Mesh(mesh::MeshError::UnknownElement(mesh::ElementId::Face(id)))) => {
            assert_eq!(id.0, past_mantissa);
        }
        other => panic!("large face id lost precision or changed error: {other:?}"),
    }

    let overflow = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("overflow")),
        ("expected", head_json(head)),
        (
            "operations",
            Value::Arr(vec![json::obj(vec![
                ("op", json::s("extrude")),
                ("object", json::s("body")),
                ("face", json::s("18446744073709551616")),
                ("offset", arr([0., 0., 1.])),
            ])]),
        ),
    ]);
    assert!(engine.execute("model.apply", &overflow, None).is_err());

    let max = json::obj(vec![
        ("document", json::s("doc")),
        ("request_id", json::s("umax")),
        ("expected", head_json(head)),
        (
            "operations",
            Value::Arr(vec![json::obj(vec![
                ("op", json::s("extrude")),
                ("object", json::s("body")),
                ("face", sid(u64::MAX)),
                ("offset", arr([0., 0., 1.])),
            ])]),
        ),
    ]);
    match engine.execute("model.apply", &max, None) {
        Err(Error::Mesh(mesh::MeshError::UnknownElement(mesh::ElementId::Face(id)))) => {
            assert_eq!(id.0, u64::MAX);
        }
        other => panic!("u64::MAX face id was not preserved: {other:?}"),
    }

    let numeric_generation = json::obj(vec![
        ("generation", Value::Int(1)),
        (
            "content",
            json::s(head.content.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        ),
    ]);
    assert!(parse_head(&numeric_generation).is_err());
}

#[test]
fn inspect_paginates_and_caps_page_size() {
    let mut engine = Engine::new(Limits::default());
    engine.open("doc", None, None).unwrap();
    let expected = head_json(engine.document("doc").unwrap().head());
    engine
        .execute(
            "model.apply",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("request_id", json::s("make")),
                ("expected", expected),
                (
                    "operations",
                    Value::Arr(vec![json::obj(vec![
                        ("op", json::s("cube")),
                        ("object", json::s("body")),
                        ("size", arr([1., 1., 1.])),
                    ])]),
                ),
            ]),
            None,
        )
        .unwrap();

    let page = engine
        .execute(
            "model.inspect",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("object", json::s("body")),
                ("domain", json::s("vertices")),
                ("offset", Value::Int(0)),
                ("limit", Value::Int(3)),
            ]),
            None,
        )
        .unwrap();
    assert_eq!(page.get("total").and_then(Value::as_i64), Some(8));
    assert_eq!(page.get("offset").and_then(Value::as_i64), Some(0));
    assert_eq!(page.get("next_offset").and_then(Value::as_i64), Some(3));
    let items = page.get("items").and_then(Value::as_arr).unwrap();
    assert_eq!(items.len(), 3);
    assert!(items[0].get("id").and_then(Value::as_str).is_some());

    let last = engine
        .execute(
            "model.inspect",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("object", json::s("body")),
                ("domain", json::s("vertices")),
                ("offset", Value::Int(6)),
                ("limit", Value::Int(3)),
            ]),
            None,
        )
        .unwrap();
    let last_items = last.get("items").and_then(Value::as_arr).unwrap();
    assert_eq!(last_items.len(), 2);
    assert!(last.get("next_offset").unwrap().is_null());

    let past = engine
        .execute(
            "model.inspect",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("object", json::s("body")),
                ("domain", json::s("faces")),
                ("offset", Value::Int(50)),
                ("limit", Value::Int(4)),
            ]),
            None,
        )
        .unwrap();
    assert_eq!(past.get("items").and_then(Value::as_arr).unwrap().len(), 0);
    assert!(past.get("next_offset").unwrap().is_null());

    let too_big = json::obj(vec![
        ("document", json::s("doc")),
        ("object", json::s("body")),
        ("domain", json::s("vertices")),
        ("offset", Value::Int(0)),
        ("limit", Value::Int((MAX_INSPECTION_PAGE as i64) + 1)),
    ]);
    assert!(engine.execute("model.inspect", &too_big, None).is_err());
    let zero = json::obj(vec![
        ("document", json::s("doc")),
        ("object", json::s("body")),
        ("domain", json::s("vertices")),
        ("offset", Value::Int(0)),
        ("limit", Value::Int(0)),
    ]);
    assert!(engine.execute("model.inspect", &zero, None).is_err());

    let edges = engine
        .execute(
            "model.inspect",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("object", json::s("body")),
                ("domain", json::s("edges")),
                ("offset", Value::Int(0)),
                ("limit", Value::Int(5)),
            ]),
            None,
        )
        .unwrap();
    assert_eq!(edges.get("total").and_then(Value::as_i64), Some(12));
    assert_eq!(edges.get("items").and_then(Value::as_arr).unwrap().len(), 5);
    assert_eq!(edges.get("next_offset").and_then(Value::as_i64), Some(5));

    let overview = engine
        .execute(
            "model.inspect",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("object", json::s("body")),
                ("domain", json::s("overview")),
                ("offset", Value::Int(0)),
                ("limit", Value::Int(8)),
            ]),
            None,
        )
        .unwrap();
    let validation = overview.get("validation").unwrap();
    assert_eq!(
        validation
            .get("self_intersections")
            .and_then(Value::as_str),
        Some("not_checked")
    );
}

#[test]
fn engine_open_is_bounded_and_names_are_strict() {
    let mut engine = Engine::new(Limits::default());
    for i in 0..MAX_OPEN_DOCUMENTS {
        engine.open(&format!("d{i}"), None, None).unwrap();
    }
    assert!(matches!(
        engine.open("overflow", None, None),
        Err(Error::Budget("open documents"))
    ));
    let mut other = Engine::new(Limits::default());
    assert!(other.open("bad\nname", None, None).is_err());
    assert!(other.open("", None, None).is_err());
}

#[test]
fn inspect_ids_stay_strings_after_extrude() {
    let mut engine = Engine::new(Limits::default());
    engine.open("doc", None, None).unwrap();
    let expected = head_json(engine.document("doc").unwrap().head());
    let created = engine
        .execute(
            "model.apply",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("request_id", json::s("make")),
                ("expected", expected),
                (
                    "operations",
                    Value::Arr(vec![json::obj(vec![
                        ("op", json::s("cube")),
                        ("object", json::s("body")),
                        ("size", arr([1., 1., 1.])),
                    ])]),
                ),
            ]),
            None,
        )
        .unwrap();
    let face = created
        .get("results")
        .and_then(Value::as_arr)
        .unwrap()[0]
        .get("faces")
        .and_then(Value::as_arr)
        .unwrap()[0]
        .as_str()
        .unwrap()
        .to_string();
    let expected = created.get("head").unwrap().clone();
    let extruded = engine
        .execute(
            "model.apply",
            &json::obj(vec![
                ("document", json::s("doc")),
                ("request_id", json::s("ex")),
                ("expected", expected),
                (
                    "operations",
                    Value::Arr(vec![json::obj(vec![
                        ("op", json::s("extrude")),
                        ("object", json::s("body")),
                        ("face", json::s(face)),
                        ("offset", arr([0., 0., 1.])),
                    ])]),
                ),
            ]),
            None,
        )
        .unwrap();
    assert!(!extruded.get("replayed").unwrap().as_bool().unwrap());
    let faces = extruded
        .get("results")
        .and_then(Value::as_arr)
        .unwrap()[0]
        .get("faces")
        .and_then(Value::as_arr)
        .unwrap();
    assert!(faces.iter().all(|v| v.as_str().is_some()));
}

#[test]
fn empty_transaction_and_unknown_tool_are_refused() {
    let mut doc = Document::new(Limits::default()).unwrap();
    assert!(matches!(
        doc.apply(
            Transaction {
                request_id: "empty".into(),
                expected: doc.head(),
                operations: vec![],
            },
            None
        ),
        Err(Error::Invalid("empty transaction"))
    ));
    let mut engine = Engine::new(Limits::default());
    engine.open("doc", None, None).unwrap();
    assert!(engine
        .execute(
            "model.explode",
            &json::obj(vec![("document", json::s("doc"))]),
            None
        )
        .is_err());
}
