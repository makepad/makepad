use makepad_model::*;
fn apply(d:&mut Document,id:&str,ops:Vec<Operation>)->Result<Applied>{d.apply(Transaction{request_id:id.into(),expected:d.head(),operations:ops},None)}
fn parsed(s:&str,d:&Document)->Vec<Operation>{parse_operations(&json::parse_depth(s.as_bytes(),24).unwrap(),d.limits()).unwrap()}
#[test]
fn replacing_an_object_in_one_batch_discards_its_old_selection_identity() {
    for smaller in [false, true] {
        let mut d = Document::new(Limits::default()).unwrap();
        let seed = parsed(r#"[
            {"op":"cube","object":"head","size":[1,1,1]},
            {"op":"select","object":"head","name":"old","element":"faces","min":6,"max":6,"query":{"kind":"all"}}
        ]"#, &d);
        apply(&mut d,"seed",seed).unwrap();
        let before = d.to_bytes(None).unwrap();
        let replacement = if smaller {
            r#"{"op":"plane","object":"head","size":[1,1]}"#
        } else {
            r#"{"op":"cube","object":"head","size":[2,2,2]}"#
        };
        let ops = parsed(&format!(r#"[
            {{"op":"delete_object","object":"head"}}, {replacement},
            {{"op":"select","object":"head","name":"fresh","element":"faces","min":1,"max":6,"query":{{"kind":"all"}}}},
            {{"op":"use_selection","object":"head","name":"fresh","element":"faces","operation":{{"op":"assign_material","material":0}}}}
        ]"#), &d);
        apply(&mut d,"replace",ops).unwrap();
        assert_eq!(d.selections().groups.len(), 1);
        assert!(!d.selections().groups.contains_key(&("head".into(),"old".into())));
        let bytes=d.to_bytes(None).unwrap();
        assert_eq!(Document::from_bytes(&bytes,Limits::default(),None).unwrap().head(),d.head());
        d.undo(d.head(),None).unwrap();
        assert_eq!(d.selections(),Document::from_bytes(&before,Limits::default(),None).unwrap().selections());
    }
}

#[test]
fn reselect_after_topology_change_marks_other_groups_stale_in_same_batch() {
    let mut d=Document::new(Limits::default()).unwrap();
    let seed=parsed(r#"[
        {"op":"cube","object":"head","size":[1,1,1]},
        {"op":"select","object":"head","name":"old","element":"faces","min":6,"max":6,"query":{"kind":"all"}}
    ]"#,&d);
    apply(&mut d,"seed",seed).unwrap();
    let face=d.selections().groups[&("head".into(),"old".into())].faces[0].0;
    let ops=parsed(&format!(r#"[
        {{"op":"delete_faces","object":"head","faces":["{face}"]}},
        {{"op":"select","object":"head","name":"fresh","element":"faces","min":5,"max":5,"query":{{"kind":"all"}}}}
    ]"#),&d);
    apply(&mut d,"reselect",ops).unwrap();
    assert!(d.selections().groups[&("head".into(),"old".into())].stale);
    assert!(!d.selections().groups[&("head".into(),"fresh".into())].stale);
    let before=d.to_bytes(None).unwrap();
    let ops=parsed(r#"[{"op":"use_selection","object":"head","name":"old","element":"faces","operation":{"op":"delete_faces"}}]"#,&d);
    assert!(apply(&mut d,"stale-use",ops).is_err());
    assert_eq!(d.to_bytes(None).unwrap(),before);
}
#[test]fn world_query_and_named_group_edit_roundtrip_then_stale_failure(){
 let mut d=Document::new(Limits::default()).unwrap();apply(&mut d,"body",vec![Operation::Cube{object:"body".into(),size:[2.;3]},Operation::Scene(SceneOperation::Node{object:"body".into(),node:SceneNode{transform:Transform{translation:[10.,0.,0.],..Default::default()},..Default::default()}})]).unwrap();
 let ops=parsed(r#"[{"op":"select","object":"body","name":"right","element":"vertices","min":4,"max":4,"query":{"kind":"bounds","min":[10.5,-2,-2],"max":[12,2,2],"space":"world"}},{"op":"use_selection","object":"body","name":"right","element":"vertices","operation":{"op":"transform","matrix":[[1,0,0,1],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}}]"#,&d);apply(&mut d,"region",ops).unwrap();
 let group=&d.selections().groups[&("body".into(),"right".into())];assert_eq!(group.vertices.len(),4);assert!(group.vertices.iter().all(|id|d.object("body").unwrap().vertex(*id).unwrap().position[0]==2.));let face=group.faces[0];
 let bytes=d.to_bytes(None).unwrap();let restored=Document::from_bytes(&bytes,Limits::default(),None).unwrap();assert_eq!(restored.head(),d.head());assert_eq!(restored.selections(),d.selections());
 apply(&mut d,"delete",vec![Operation::DeleteFaces{object:"body".into(),faces:vec![face]}]).unwrap();assert!(d.selections().groups[&("body".into(),"right".into())].stale);
 let before=d.to_bytes(None).unwrap();let ops=parsed(r#"[{"op":"use_selection","object":"body","name":"right","element":"faces","operation":{"op":"delete_faces"}}]"#,&d);assert!(apply(&mut d,"stale",ops).is_err());assert_eq!(before,d.to_bytes(None).unwrap());
}
#[test]fn ray_cardinality_connected_and_cancelled_query_are_atomic(){
 let mut d=Document::new(Limits::default()).unwrap();apply(&mut d,"body",vec![Operation::Cube{object:"body".into(),size:[2.;3]}]).unwrap();let ops=parsed(r#"[{"op":"select","object":"body","name":"hit","element":"faces","min":1,"max":1,"query":{"kind":"ray","origin":[0,0,5],"direction":[0,0,-1],"space":"local"}}]"#,&d);apply(&mut d,"ray",ops).unwrap();let face=d.selections().groups[&("body".into(),"hit".into())].faces[0];
 apply(&mut d,"connected",vec![Operation::Selection(SelectionOperation::Query{object:"body".into(),name:"shell".into(),query:SelectionQuery::Connected{face,stop_seams:false},element:"faces".into(),min:6,max:6})]).unwrap();assert_eq!(d.selections().groups[&("body".into(),"shell".into())].vertices.len(),8);
 let before=d.to_bytes(None).unwrap();let op=Operation::Selection(SelectionOperation::Query{object:"body".into(),name:"bad".into(),query:SelectionQuery::All,element:"faces".into(),min:1,max:1});assert!(apply(&mut d,"ambiguous",vec![op]).is_err());assert_eq!(d.to_bytes(None).unwrap(),before);
 let ops=parsed(r#"[{"op":"select","object":"body","name":"all","element":"vertices","min":8,"max":8,"query":{"kind":"all"}}]"#,&d);assert!(d.apply(Transaction{request_id:"cancel".into(),expected:d.head(),operations:ops},Some(&||true)).is_err());assert_eq!(before,d.to_bytes(None).unwrap());
}
