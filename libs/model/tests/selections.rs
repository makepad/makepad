use makepad_model::*;
fn apply(d:&mut Document,id:&str,ops:Vec<Operation>)->Result<Applied>{d.apply(Transaction{request_id:id.into(),expected:d.head(),operations:ops},None)}
#[test]
fn replacing_an_object_in_one_batch_discards_its_old_selection_identity() {
    for smaller in [false, true] {
        let mut d = Document::new(Limits::default()).unwrap();
        let seed = vec![
        Operation::Cube { object: "head".into(), size: [1.0, 1.0, 1.0] },
        Operation::Selection(SelectionOperation::Query {
            object: "head".into(),
            name: "old".into(),
            query: SelectionQuery::All,
            element: "faces".into(),
            min: 6,
            max: 6
        }),
    ];
        apply(&mut d,"seed",seed).unwrap();
        let before = d.to_bytes(None).unwrap();
        let replacement = if smaller {
            Operation::Plane { object: "head".into(), size: [1.; 2] }
        } else {
            Operation::Cube { object: "head".into(), size: [2.; 3] }
        };
        let ops = vec![
            Operation::DeleteObject { object: "head".into() },
            replacement,
            Operation::Selection(SelectionOperation::Query {
                object: "head".into(),
                name: "fresh".into(),
                query: SelectionQuery::All,
                element: "faces".into(),
                min: 1,
                max: 6
            }),
            Operation::Selection(SelectionOperation::Use {
                object: "head".into(),
                name: "fresh".into(),
                element: "faces".into(),
                operation: Box::new(Operation::AssignMaterial { material: 0, object: "head".into(), faces: vec![] })
            }),
        ];
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
    let seed=vec![
        Operation::Cube { object: "head".into(), size: [1.0, 1.0, 1.0] },
        Operation::Selection(SelectionOperation::Query {
            object: "head".into(),
            name: "old".into(),
            query: SelectionQuery::All,
            element: "faces".into(),
            min: 6,
            max: 6
        }),
    ];
    apply(&mut d,"seed",seed).unwrap();
    let face=d.selections().groups[&("head".into(),"old".into())].faces[0];
    let ops=vec![
        Operation::DeleteFaces { object:"head".into(), faces:vec![face] },
        Operation::Selection(SelectionOperation::Query {object:"head".into(),name:"fresh".into(),element:"faces".into(),min:5,max:5,query:SelectionQuery::All}),
    ];
    apply(&mut d,"reselect",ops).unwrap();
    assert!(d.selections().groups[&("head".into(),"old".into())].stale);
    assert!(!d.selections().groups[&("head".into(),"fresh".into())].stale);
    let before=d.to_bytes(None).unwrap();
    let ops=vec![
        Operation::Selection(SelectionOperation::Use {
            object: "head".into(),
            name: "old".into(),
            element: "faces".into(),
            operation: Box::new(Operation::DeleteFaces { object: "head".into(), faces: vec![] })
        }),
    ];
    assert!(apply(&mut d,"stale-use",ops).is_err());
    assert_eq!(d.to_bytes(None).unwrap(),before);
}
#[test]fn world_query_and_named_group_edit_roundtrip_then_stale_failure(){
 let mut d=Document::new(Limits::default()).unwrap();apply(&mut d,"body",vec![Operation::Cube{object:"body".into(),size:[2.;3]},Operation::Scene(SceneOperation::Node{object:"body".into(),node:SceneNode{transform:Transform{translation:[10.,0.,0.],..Default::default()},..Default::default()}})]).unwrap();
 let ops=vec![
        Operation::Selection(SelectionOperation::Query {
            object: "body".into(),
            name: "right".into(),
            query: SelectionQuery::Bounds { min: [10.5, -2.0, -2.0], max: [12.0, 2.0, 2.0], world: true },
            element: "vertices".into(),
            min: 4,
            max: 4
        }),
        Operation::Selection(SelectionOperation::Use {
            object: "body".into(),
            name: "right".into(),
            element: "vertices".into(),
            operation: Box::new(Operation::Transform {
                matrix: [[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
                object: "body".into(),
                vertices: vec![]
            })
        }),
    ];apply(&mut d,"region",ops).unwrap();
 let group=&d.selections().groups[&("body".into(),"right".into())];assert_eq!(group.vertices.len(),4);assert!(group.vertices.iter().all(|id|d.object("body").unwrap().vertex(*id).unwrap().position[0]==2.));let face=group.faces[0];
 let bytes=d.to_bytes(None).unwrap();let restored=Document::from_bytes(&bytes,Limits::default(),None).unwrap();assert_eq!(restored.head(),d.head());assert_eq!(restored.selections(),d.selections());
 apply(&mut d,"delete",vec![Operation::DeleteFaces{object:"body".into(),faces:vec![face]}]).unwrap();assert!(d.selections().groups[&("body".into(),"right".into())].stale);
 let before=d.to_bytes(None).unwrap();let ops=vec![
        Operation::Selection(SelectionOperation::Use {
            object: "body".into(),
            name: "right".into(),
            element: "faces".into(),
            operation: Box::new(Operation::DeleteFaces { object: "body".into(), faces: vec![] })
        }),
    ];assert!(apply(&mut d,"stale",ops).is_err());assert_eq!(before,d.to_bytes(None).unwrap());
}
#[test]fn ray_cardinality_connected_and_cancelled_query_are_atomic(){
 let mut d=Document::new(Limits::default()).unwrap();apply(&mut d,"body",vec![Operation::Cube{object:"body".into(),size:[2.;3]}]).unwrap();let ops=vec![
        Operation::Selection(SelectionOperation::Query {
            object: "body".into(),
            name: "hit".into(),
            query: SelectionQuery::Ray { origin: [0.0, 0.0, 5.0], direction: [0.0, 0.0, -1.0], world: false },
            element: "faces".into(),
            min: 1,
            max: 1
        }),
    ];apply(&mut d,"ray",ops).unwrap();let face=d.selections().groups[&("body".into(),"hit".into())].faces[0];
 apply(&mut d,"connected",vec![Operation::Selection(SelectionOperation::Query{object:"body".into(),name:"shell".into(),query:SelectionQuery::Connected{face,stop_seams:false},element:"faces".into(),min:6,max:6})]).unwrap();assert_eq!(d.selections().groups[&("body".into(),"shell".into())].vertices.len(),8);
 let before=d.to_bytes(None).unwrap();let op=Operation::Selection(SelectionOperation::Query{object:"body".into(),name:"bad".into(),query:SelectionQuery::All,element:"faces".into(),min:1,max:1});assert!(apply(&mut d,"ambiguous",vec![op]).is_err());assert_eq!(d.to_bytes(None).unwrap(),before);
 let ops=vec![
        Operation::Selection(SelectionOperation::Query {
            object: "body".into(),
            name: "all".into(),
            query: SelectionQuery::All,
            element: "vertices".into(),
            min: 8,
            max: 8
        }),
    ];assert!(d.apply(Transaction{request_id:"cancel".into(),expected:d.head(),operations:ops},Some(&||true)).is_err());assert_eq!(before,d.to_bytes(None).unwrap());
}
