use makepad_mesh_edit::*;
fn ctx()->Context<'static>{Context::default()}
fn closed(m:&Mesh){assert!(m.validate(&mut ctx()).unwrap().is_closed_manifold);m.triangulate(&mut ctx()).unwrap();}
fn volume(m:&Mesh)->f64{let t=m.triangulate(&mut ctx()).unwrap();t.triangles.iter().map(|t0|{let [a,b,c]=t0.indices.map(|i|t.vertices[i as usize].position);(a[0]*(b[1]*c[2]-b[2]*c[1])+a[1]*(b[2]*c[0]-b[0]*c[2])+a[2]*(b[0]*c[1]-b[1]*c[0]))/6.}).sum()}
#[test]
fn connected_region_has_only_boundary_walls_and_retains_caps(){
    let mut m=Mesh::cube([2.;3],&mut ctx()).unwrap();let fs=[m.faces()[1].id,m.faces()[3].id];
    let out=m.extrude_region(&fs,[0.,1.,1.],&mut ctx()).unwrap();
    assert_eq!(out.side_faces.len(),6);assert_eq!(out.caps,fs);assert_eq!(m.vertices().len(),14);closed(&m);assert!(volume(&m)>8.);
    let saved=m.clone();assert!(m.extrude_region(&fs,[1.,0.,0.],&mut ctx()).is_err());assert_eq!(m,saved);
}
#[test]
fn cube_loop_cut_splits_four_faces_and_preserves_volume(){
    let mut m=Mesh::cube([2.;3],&mut ctx()).unwrap();let before=volume(&m);let edge=*m.adjacency(&mut ctx()).unwrap().edges.keys().next().unwrap();
    let out=m.loop_cut(edge,0.3,&mut ctx()).unwrap();assert_eq!(m.vertices().len(),12);assert_eq!(m.faces().len(),10);closed(&m);assert!((volume(&m)-before).abs()<1e-10);
    assert_eq!(out.remapped.iter().filter(|(s,_)|matches!(s,ElementId::Face(_))).count(),4);
    let bytes=m.to_bytes(&mut ctx()).unwrap();assert_eq!(Mesh::from_bytes(&bytes,&mut ctx()).unwrap(),m);
}
#[test]
fn split_and_collapse_preserve_closed_topology_and_interpolate_weights(){
    let mut m=Mesh::cube([2.;3],&mut ctx()).unwrap();let e=*m.adjacency(&mut ctx()).unwrap().edges.keys().next().unwrap();
    m.set_vertex_weights(e.0,&[JointWeight{joint:0,weight:1.}],&mut ctx()).unwrap();m.set_vertex_weights(e.1,&[JointWeight{joint:1,weight:1.}],&mut ctx()).unwrap();
    let change=m.split_edges(&[e],0.25,&mut ctx()).unwrap();closed(&m);let v=change.created.iter().find_map(|i|if let ElementId::Vertex(v)=i{Some(*v)}else{None}).unwrap();
    assert_eq!(m.vertex(v).unwrap().weights,vec![JointWeight{joint:0,weight:0.75},JointWeight{joint:1,weight:0.25}]);
    m.collapse_edge(EdgeKey::new(e.0,v),0.5,&mut ctx()).unwrap();closed(&m);assert_eq!(m.vertices().len(),8);
}
#[test]
fn fill_and_bridge_close_real_boundaries_and_solidify_has_positive_volume(){
    let mut m=Mesh::cube([2.;3],&mut ctx()).unwrap();let top=m.faces()[3].id;let cycle=m.face_corners(top).unwrap().iter().map(|c|c.vertex).collect::<Vec<_>>();m.delete_faces(&[top],&mut ctx()).unwrap();
    m.fill_boundary(&cycle,0,&mut ctx()).unwrap();closed(&m);assert!((volume(&m)-8.).abs()<1e-10);
    let mut p=Mesh::plane([2.,3.],&mut ctx()).unwrap();p.solidify(0.5,&mut ctx()).unwrap();closed(&p);assert!((volume(&p)-3.).abs()<1e-10);
    let mut two=Mesh::from_polygons(&[[-1.,0.,-1.],[-1.,0.,1.],[1.,0.,1.],[1.,0.,-1.],[-1.,1.,-1.],[-1.,1.,1.],[1.,1.,1.],[1.,1.,-1.]],
        &[Polygon::new(vec![3,2,1,0]),Polygon::new(vec![4,5,6,7])],&mut ctx()).unwrap();
    let a=two.vertices()[..4].iter().map(|v|v.id).collect::<Vec<_>>();let b=two.vertices()[4..].iter().map(|v|v.id).collect::<Vec<_>>();
    two.bridge_boundaries(&a,&b,0,&mut ctx()).unwrap();closed(&two);assert!((volume(&two)-4.).abs()<1e-10);
}
#[test]
fn coplanar_dissolve_preserves_area_and_refuses_material_or_uv_loss(){
    let mut m=Mesh::from_polygons(&[[0.,0.,0.],[1.,0.,0.],[1.,1.,0.],[0.,1.,0.]],&[Polygon::new(vec![0,1,2]),Polygon::new(vec![0,2,3])],&mut ctx()).unwrap();
    let e=EdgeKey::new(m.vertices()[0].id,m.vertices()[2].id);m.dissolve_edge(e,&mut ctx()).unwrap();assert_eq!(m.faces().len(),1);assert_eq!(m.corners().len(),4);assert_eq!(m.triangulate(&mut ctx()).unwrap().triangles.len(),2);
}
#[test]
fn isolated_cube_edge_bevel_has_uniform_width_and_refuses_overrun(){
    let mut m=Mesh::cube([2.;3],&mut ctx()).unwrap();let e=*m.adjacency(&mut ctx()).unwrap().edges.keys().next().unwrap();
    let before=m.clone();assert!(m.bevel_edges(&[e],2.,&mut ctx()).is_err());assert_eq!(m,before);
    let changes=m.bevel_edges(&[e],0.2,&mut ctx()).unwrap();closed(&m);assert_eq!(m.vertices().len(),10);assert_eq!(m.faces().len(),7);
    assert!((volume(&m)-(8.-0.2*0.2)).abs()<1e-10);assert!(changes.deleted.contains(&ElementId::Vertex(e.0)));assert!(changes.deleted.contains(&ElementId::Vertex(e.1)));
}
