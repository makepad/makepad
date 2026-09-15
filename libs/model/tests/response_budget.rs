use makepad_model::{json::{self, Value}, *};
use std::collections::BTreeSet;

fn escaped_name(index: usize) -> String {
    let name = format!("{}{:04}", "\"\\".repeat(46), index);
    assert_eq!(name.len(), Limits::default().max_name_bytes);
    name
}
fn bounded(value: &Value) {
    let text = value.to_json();
    assert!(text.len() <= MAX_REPLY_BYTES, "reply is {} bytes", text.len());
    assert!(json::parse(text.as_bytes()).is_ok());
    let wrapped = json::obj(vec![("outcome", json::s("ok")), ("value", value.clone())]).to_json();
    assert!(wrapped.len() <= 16 * 1024);
    assert!(json::parse(wrapped.as_bytes()).is_ok());
}
fn number(value: &Value, key: &str) -> usize {
    value.get(key).unwrap().as_u64().unwrap() as usize
}
fn apply(doc: &mut Document, operations: Vec<Operation>) {
    doc.apply(Transaction { request_id: format!("seed_{}", doc.head().generation), expected: doc.head(), operations }, None).unwrap();
}
fn engine(doc: &Document, name: &str) -> Engine {
    let mut engine = Engine::new(doc.limits().clone());
    bounded(&engine.open(name, Some(&doc.to_bytes(None).unwrap()), None).unwrap());
    engine
}
fn inspect(engine: &mut Engine, document: &str, object: Option<&str>, domain: &str, offset: usize) -> Value {
    let mut args = vec![("document", json::s(document)), ("domain", json::s(domain)),
        ("offset", Value::Int(offset as i64)), ("limit", Value::Int(128))];
    if let Some(object) = object {args.push(("object", json::s(object)));}
    let value = engine.execute("model.inspect", &json::obj(args), None).unwrap();
    bounded(&value);
    value
}
fn all_rows(engine: &mut Engine, document: &str, object: Option<&str>, domain: &str, total: usize) -> Vec<Value> {
    let mut offset = 0;
    let mut rows = Vec::new();
    let head = engine.document(document).unwrap().head();
    loop {
        let page = inspect(engine, document, object, domain, offset);
        assert_eq!(parse_head(page.get("head").unwrap()).unwrap(), head);
        assert_eq!(number(&page, "total"), total);
        assert_eq!(number(&page, "offset"), offset);
        let items = page.get("items").unwrap().as_arr().unwrap();
        assert_eq!(number(&page, "count"), items.len());
        assert_eq!(number(&page, "omitted"), total - offset - items.len());
        rows.extend_from_slice(items);
        match page.get("next_offset").unwrap().as_u64() {
            Some(next) => {
                assert!(!items.is_empty());
                assert_eq!(next as usize, offset + items.len());
                offset = next as usize;
            }
            None => break,
        }
    }
    assert_eq!(rows.len(), total);
    rows
}

#[test]
fn extreme_inventories_and_keys_page_by_serialized_bytes_without_losing_names() {
    let mut doc = Document::new(Limits::default()).unwrap();
    let mut operations = Vec::new();
    for i in 0..128 {
        operations.push(Operation::Cube {object: escaped_name(i), size: [1.; 3]});
    }
    apply(&mut doc, std::mem::take(&mut operations));
    // Tiny but finite values have long JSON decimal encodings in this writer.
    for i in 0..64 {
        operations.push(Operation::SetMaterial {material: i, value: Material {color: [1e-300; 3], ..Default::default()}});
    }
    operations.push(Operation::SetSkeleton {skeleton: Skeleton {joints: (0..64).map(|i| Joint {
        name: escaped_name(i), parent: if i == 0 {None} else {Some(0)}, translation: [1e-300; 3],
    }).collect()}});
    for i in 0..64 {
        operations.push(Operation::SetClip {clip: AnimationClip {name: escaped_name(i), channels: vec![AnimationChannel {
            joint: i as u32, path: AnimationPath::Translation, keys: vec![
                Keyframe {time: 0., value: [1e-300, 1e-300, 1e-300, 0.]},
                Keyframe {time: 1., value: [1., 2., 3., 0.]},
            ],
        }]}});
    }
    apply(&mut doc, operations);
    let document = escaped_name(9999);
    let mut engine = engine(&doc, &document);
    let overview = inspect(&mut engine, &document, None, "overview", 0);
    for (domain, total) in [("objects", 128), ("materials", 64), ("joints", 64), ("clips", 64), ("clip_keys", 128)] {
        assert_eq!(number(overview.get("counts").unwrap(), domain), total);
        let inventory = overview.get("inventories").unwrap().get(domain).unwrap();
        assert_eq!(number(inventory, "total"), total);
        assert_eq!(number(inventory, "count") + number(inventory, "omitted"), total);
        let rows = all_rows(&mut engine, &document, None, domain, total);
        match domain {
            "objects" | "clips" => {
                let field = if domain == "objects" {"object"} else {"name"};
                assert_eq!(rows.iter().map(|r| r.get(field).unwrap().as_str().unwrap().to_owned()).collect::<Vec<_>>(),
                    (0..total).map(escaped_name).collect::<Vec<_>>());
                if domain == "clips" {
                    for (i, row) in rows.iter().enumerate() {assert_eq!(number(row, "key_offset"), i * 2);}
                }
            }
            "materials" | "joints" => {
                let field = if domain == "materials" {"id"} else {"joint"};
                assert_eq!(rows.iter().map(|r| number(r, field)).collect::<Vec<_>>(), (0..total).collect::<Vec<_>>());
                let first = inspect(&mut engine, &document, None, domain, 0);
                if domain == "joints" {
                    assert_eq!(first.get("byte_limited").unwrap().as_bool(), Some(true));
                    assert!(number(&first, "count") < total);
                }
            }
            "clip_keys" => {
                let unique: BTreeSet<_> = rows.iter().map(|r| (r.get("clip").unwrap().as_str().unwrap(), number(r, "channel"), number(r, "key"))).collect();
                assert_eq!(unique.len(), 128);
                for (i, row) in rows.iter().enumerate() {
                    assert_eq!(row.get("clip").unwrap().as_str().unwrap(), escaped_name(i / 2));
                    assert_eq!(number(row, "key"), i % 2);
                }
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn every_vertex_and_weight_remains_addressable_in_128_by_64_weight_pages() {
    let positions = (0..128).map(|i| [i as f64, 0., 0.]).collect::<Vec<_>>();
    let weights = (0..128).map(|i| (0..64).map(|joint| mesh::JointWeight {
        joint, weight: if i == 0 {if joint == 0 {1.} else {1e-300}} else {1. / 64.},
    }).collect()).collect::<Vec<Vec<_>>>();
    let mesh = mesh::Mesh::from_weighted_polygons(&positions, &weights, &[], &mut mesh::Context::default()).unwrap();
    let ids: Vec<_> = mesh.vertices().iter().map(|v| v.id.0.to_string()).collect();
    let name = escaped_name(42);
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, vec![Operation::ImportMesh {object: name.clone(), source: mesh.to_bytes(&mut mesh::Context::default()).unwrap()}]);
    let mut engine = engine(&doc, "d");
    let first = inspect(&mut engine, "d", Some(&name), "vertices", 0);
    assert_eq!(first.get("byte_limited").unwrap().as_bool(), Some(true));
    let vertices = all_rows(&mut engine, "d", Some(&name), "vertices", 128);
    assert_eq!(vertices.iter().map(|r| r.get("id").unwrap().as_str().unwrap().to_owned()).collect::<Vec<_>>(), ids);
    for (i, row) in vertices.iter().enumerate() {
        assert_eq!(number(row, "weight_count"), 64);
        assert_eq!(number(row, "weight_offset"), i * 64);
        assert_eq!(row.get("weights").unwrap().as_arr().unwrap().len() + number(row, "weights_omitted"), 64);
    }
    let influences = all_rows(&mut engine, "d", Some(&name), "weights", 128 * 64);
    for (i, row) in influences.iter().enumerate() {
        assert_eq!(row.get("vertex").unwrap().as_str().unwrap(), ids[i / 64]);
        assert_eq!(number(row, "weight_index"), i % 64);
        assert_eq!(number(row, "joint"), i % 64);
        assert_eq!(row.get("weight").unwrap(), &Value::F64(weights[i / 64][i % 64].weight));
    }
    let end = inspect(&mut engine, "d", Some(&name), "weights", 128 * 64 + 100);
    assert_eq!(number(&end, "count"), 0);
    assert_eq!(end.get("next_offset"), Some(&Value::Null));
}

#[test]
fn a_vertex_above_the_row_budget_has_an_explicit_recoverable_weight_preview() {
    let mut limits = Limits::default();
    limits.mesh.max_weights_per_vertex = 512;
    let weights = vec![(0..512).map(|joint| mesh::JointWeight {joint, weight: 1. / 512.}).collect::<Vec<_>>()];
    let mesh = mesh::Mesh::from_weighted_polygons(&[[0.; 3]], &weights, &[],
        &mut mesh::Context::new(limits.mesh.clone(), None)).unwrap();
    let source = mesh.to_bytes(&mut mesh::Context::new(limits.mesh.clone(), None)).unwrap();
    let mut doc = Document::new(limits).unwrap();
    apply(&mut doc, vec![Operation::ImportMesh {object: "weighted".into(), source}]);
    let mut engine = engine(&doc, "d");
    let vertices = all_rows(&mut engine, "d", Some("weighted"), "vertices", 1);
    let row = &vertices[0];
    assert!(number(row, "weights_omitted") > 0);
    assert_eq!(row.get("weights").unwrap().as_arr().unwrap().len() + number(row, "weights_omitted"), 512);
    let weights = all_rows(&mut engine, "d", Some("weighted"), "weights", 512);
    assert_eq!(weights.iter().map(|w| number(w, "joint")).collect::<Vec<_>>(), (0..512).collect::<Vec<_>>());
}

#[test]
fn a_256_operation_commit_and_retry_keep_both_heads_when_result_rows_are_omitted() {
    let name = escaped_name(7);
    let document = escaped_name(8);
    let mut doc = Document::new(Limits::default()).unwrap();
    apply(&mut doc, vec![Operation::Cube {object: name.clone(), size: [1.; 3]}]);
    let ids: Vec<_> = doc.object(&name).unwrap().vertices().iter().map(|v| v.id.0.to_string()).collect();
    let mut engine = engine(&doc, &document);
    let expected = engine.document(&document).unwrap().head();
    let matrix = json::parse(b"[[1,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]").unwrap();
    let operations = (0..256).map(|_| json::obj(vec![
        ("op", json::s("transform")), ("object", json::s(&name)),
        ("vertices", Value::Arr(ids.iter().map(json::s).collect())), ("matrix", matrix.clone()),
    ])).collect();
    let args = json::obj(vec![("document", json::s(&document)), ("request_id", json::s("large-result")),
        ("expected", head_json(expected)), ("operations", Value::Arr(operations))]);
    let reply = engine.execute("model.apply", &args, None).unwrap();
    bounded(&reply);
    let committed = engine.document(&document).unwrap().head();
    assert_eq!(committed.generation, expected.generation + 1);
    assert_eq!(parse_head(reply.get("head").unwrap()).unwrap(), committed);
    assert_eq!(parse_head(reply.get("committed").unwrap()).unwrap(), committed);
    assert_eq!(reply.get("replayed").unwrap().as_bool(), Some(false));
    assert_eq!(number(&reply, "results_total"), 256);
    assert!(number(&reply, "results_omitted") > 0);
    assert_eq!(number(&reply, "results_returned") + number(&reply, "results_omitted"), 256);
    assert_eq!(reply.get("results_truncated").unwrap().as_bool(), Some(true));
    let rows = reply.get("results").unwrap().as_arr().unwrap();
    assert_eq!(rows.len(), number(&reply, "results_returned"));
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(number(row, "result_index"), i);
        assert_eq!(row.get("object").unwrap().as_str().unwrap(), name);
    }
    assert_eq!(number(&reply, "selection_vertex_total"), 256 * ids.len());
    assert_eq!(number(&reply, "selection_ids_returned") + number(&reply, "selection_ids_omitted"), 256 * ids.len());
    let undo = json::obj(vec![("document", json::s(&document)), ("expected", head_json(committed)), ("action", json::s("undo"))]);
    bounded(&engine.execute("model.history", &undo, None).unwrap());
    let current = engine.document(&document).unwrap().head();
    let replay = engine.execute("model.apply", &args, None).unwrap();
    bounded(&replay);
    assert_eq!(parse_head(replay.get("head").unwrap()).unwrap(), current);
    assert_eq!(parse_head(replay.get("committed").unwrap()).unwrap(), committed);
    assert_eq!(replay.get("replayed").unwrap().as_bool(), Some(true));
    assert_eq!(engine.document(&document).unwrap().head(), current);
    assert_eq!(all_rows(&mut engine, &document, Some(&name), "vertices", ids.len()).iter()
        .map(|r| r.get("id").unwrap().as_str().unwrap().to_owned()).collect::<Vec<_>>(), ids);
}
