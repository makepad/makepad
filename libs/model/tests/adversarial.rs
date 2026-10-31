//! Adversarial checks of document transactions, history, codec and the
//! typed operation validation. Foundation operations only.
use makepad_model::*;
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
fn stable_ids_and_generations_are_precise_decimal_strings() {
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, "make", vec![cube("body")]);
    let source = bytes(&doc);
    for id in [(1u64 << 53) + 1, u64::MAX] {
        let error = doc.apply(Transaction {
            request_id: format!("missing-{id}"), expected: doc.head(),
            operations: vec![Operation::Extrude { object:"body".into(), face:mesh::FaceId(id), offset:[0.,0.,1.] }],
        }, None).unwrap_err();
        assert!(matches!(error, Error::Mesh(mesh::MeshError::UnknownElement(mesh::ElementId::Face(face))) if face.0 == id), "{error:?}");
        assert_eq!(bytes(&doc), source);
    }
}

#[test]
fn empty_transaction_is_refused() {
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
}
