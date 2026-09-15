use makepad_model::{json::{self, Value}, *};
fn args(v:&str)->Value {json::parse(v.as_bytes()).unwrap()}

#[test]
fn missing_subdivision_level_reports_the_field_and_allows_an_atomic_retry() {
    let mut engine=Engine::new(Limits::default());
    engine.open("d",None,None).unwrap();
    let head=engine.document("d").unwrap().head();
    let before=engine.document("d").unwrap().to_bytes(None).unwrap();
    let batch=|id,operations|json::obj(vec![("document",json::s("d")),
        ("request_id",json::s(id)),("expected",head_json(head)),("operations",operations)]);
    let error=engine.execute("model.apply",&batch("bad",args(r#"[
        {"op":"cube","object":"head","size":[0.88,0.83,0.52]},
        {"op":"subdivide","object":"head"}]"#)),None).unwrap_err().to_string();
    assert!(error.contains("levels") && error.contains("subdivide"),"{error}");
    assert_eq!(engine.document("d").unwrap().to_bytes(None).unwrap(),before);
    engine.execute("model.apply",&batch("corrected",args(r#"[
        {"op":"cube","object":"head","size":[0.88,0.83,0.52]},
        {"op":"subdivide","object":"head","levels":2}]"#)),None).unwrap();
    assert_ne!(engine.document("d").unwrap().head(),head);
    assert!(engine.document("d").unwrap().object("head").is_some());
}

#[test]
fn overview_reports_complete_transformed_dimensions_without_editing_source() {
    let mut engine=Engine::new(Limits::default());
    assert_eq!(engine.open("d",None,None).unwrap().get("bounds"),Some(&Value::Null));
    apply(&mut engine,"shape",args(r#"[{"op":"cube","object":"body","size":[2,1,4]},{"op":"object_node","object":"body","node":{"transform":{"translation":[3,2,-1]}}}]"#));
    let head=engine.document("d").unwrap().head();
    let overview=engine.open("d",None,None).unwrap();
    assert_eq!(overview.get("bounds").unwrap(),&args("[[2.0,1.5,-3.0],[4.0,2.5,1.0]]"));
    assert_eq!(overview.get("dimensions").unwrap(),&args("[2.0,1.0,4.0]"));
    assert_eq!(engine.document("d").unwrap().head(),head);
    assert!(overview.to_json().len()<12*1024);
}
fn apply(engine:&mut Engine,id:&str,ops:Value)->Value {
    engine.execute("model.apply",&json::obj(vec![("document",json::s("d")),("expected",head_json(engine.document("d").unwrap().head())),
        ("request_id",json::s(id)),("operations",ops)]),None).unwrap()
}
#[test]
fn primitives_are_closed_outward_and_have_seam_uvs() {
    let mut engine=Engine::new(Limits::default()); engine.open("d",None,None).unwrap();
    apply(&mut engine,"create",args(r#"[{"op":"sphere","object":"ball","radius":1,"segments":12,"rings":6},{"op":"cylinder","object":"barrel","radius":0.5,"height":2,"segments":12}]"#));
    for name in ["ball","barrel"] {
        let mesh=engine.document("d").unwrap().object(name).unwrap();
        assert!(mesh.validate(&mut mesh::Context::default()).unwrap().is_closed_manifold);
        let tri=mesh.triangulate(&mut mesh::Context::default()).unwrap();
        let mut volume=0.0;
        for t in &tri.triangles {
            let [a,b,c]=t.indices.map(|i|tri.vertices[i as usize].position);
            volume+=(a[0]*(b[1]*c[2]-b[2]*c[1])+a[1]*(b[2]*c[0]-b[0]*c[2])+a[2]*(b[0]*c[1]-b[1]*c[0]))/6.0;
        }
        assert!(volume>0.0,"{name} volume {volume}");
        assert!(mesh.corners().iter().any(|c|c.uv[0]==1.0));
    }
}
#[test]
fn paged_corner_lookup_and_exact_id_protocol() {
    let mut engine=Engine::new(Limits::default()); engine.open("d",None,None).unwrap();
    apply(&mut engine,"cube",args(r#"[{"op":"cube","object":"body","size":[1,1,1]}]"#));
    let page=engine.execute("model.inspect",&args(r#"{"document":"d","object":"body","domain":"corners","offset":2,"limit":3}"#),None).unwrap();
    assert_eq!(page.get("items").unwrap().as_arr().unwrap().len(),3); assert_eq!(page.get("next_offset").unwrap().as_u64(),Some(5));
    assert!(parse_operations(&args(r#"[{"op":"extrude","object":"body","face":1,"offset":[0,1,0]}]"#),&Limits::default()).is_err());
    assert!(parse_operations(&args(r#"[{"op":"extrude","object":"body","face":"01","offset":[0,1,0]}]"#),&Limits::default()).is_err());
    assert!(parse_operations(&args(r#"[{"op":"cube","object":"other","size":[1,1,1],"typo":true}]"#),&Limits::default()).is_err());
    let source=engine.document("d").unwrap().to_bytes(None).unwrap();
    assert!(engine.execute("model.apply",&json::obj(vec![("document",json::s("d")),("request_id",json::s("cancel")),
        ("expected",head_json(engine.document("d").unwrap().head())),("operations",args(r#"[{"op":"cube","object":"cancelled","size":[1,1,1]}]"#))]),Some(&||true)).is_err());
    assert_eq!(engine.document("d").unwrap().to_bytes(None).unwrap(),source);
}
