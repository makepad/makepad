use makepad_model::*;

fn escaped_name(index: usize) -> String {
    let name = format!("{}{:04}", "\"\\".repeat(46), index);
    assert_eq!(name.len(), Limits::default().max_name_bytes);
    name
}
fn apply(doc: &mut Document, operations: Vec<Operation>) {
    doc.apply(Transaction { request_id: format!("seed_{}", doc.head().generation), expected: doc.head(), operations }, None).unwrap();
}

#[test]
fn a_256_operation_commit_and_retry_keep_both_heads_when_result_rows_are_omitted() {
    let name = escaped_name(7);
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, vec![Operation::Cube { object: name.clone(), size: [1.; 3] }]);
    let ids: Vec<_> = doc.object(&name).unwrap().vertices().iter().map(|v| v.id).collect();
    let expected = doc.head();
    let operations = (0..256).map(|_| Operation::Transform {
        object: name.clone(), vertices: ids.clone(), matrix: transform::IDENTITY_MATRIX,
    }).collect();
    let transaction = Transaction { request_id: "large-result".into(), expected, operations };
    let reply = doc.apply(transaction.clone(), None).unwrap();
    let committed = doc.head();
    assert_eq!(committed.generation, expected.generation + 1);
    assert_eq!(reply.current, committed);
    assert_eq!(reply.committed, committed);
    assert!(!reply.replayed);
    assert_eq!(reply.results.len(), 256);
    for row in &reply.results {
        assert_eq!(row.object, name);
        assert_eq!(row.vertices, ids);
    }
    doc.undo(committed, None).unwrap();
    let current = doc.head();
    let replay = doc.apply(transaction, None).unwrap();
    assert_eq!(replay.current, current);
    assert_eq!(replay.committed, committed);
    assert!(replay.replayed);
    assert_eq!(doc.head(), current);
    assert_eq!(doc.object(&name).unwrap().vertices().iter().map(|v|v.id).collect::<Vec<_>>(), ids);
}
