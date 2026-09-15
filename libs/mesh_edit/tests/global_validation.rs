use makepad_mesh_edit::*;
use std::cell::Cell;

fn cube(size:f64,offset:[f64;3])->Mesh{
    let mut m=Mesh::cube([size;3],&mut Context::default()).unwrap();
    let ids=m.vertices().iter().map(|v|v.id).collect::<Vec<_>>();
    m.transform(&ids,[[1.,0.,0.,offset[0]],[0.,1.,0.,offset[1]],[0.,0.,1.,offset[2]],[0.,0.,0.,1.]],&mut Context::default()).unwrap();m
}
fn inward(m:&Mesh)->Mesh{
    let positions=m.vertices().iter().map(|v|v.position).collect::<Vec<_>>();
    let polygons=m.faces().iter().map(|f|{let cs=m.face_corners(f.id).unwrap();Polygon{vertices:cs.iter().rev().map(|c|m.vertices().iter().position(|v|v.id==c.vertex).unwrap()as u32).collect(),uvs:cs.iter().rev().map(|c|c.uv).collect(),material:f.material}}).collect::<Vec<_>>();
    Mesh::from_polygons(&positions,&polygons,&mut Context::default()).unwrap()
}
fn soup(points:&[[f64;3]],faces:&[[u32;3]])->Mesh{
    let polygons=faces.iter().map(|v|Polygon{vertices:v.to_vec(),uvs:vec![[0.;2];3],material:0}).collect::<Vec<_>>();Mesh::from_polygons(points,&polygons,&mut Context::default()).unwrap()
}

#[test]
fn closed_shells_require_global_contact_and_orientation_checks(){
    let mut m=cube(2.,[0.;3]);let cheap=m.validate(&mut Context::default()).unwrap();assert!(cheap.is_closed_manifold);assert!(!cheap.is_valid_solid);assert_eq!(cheap.self_intersections,SelfIntersectionStatus::NotChecked);
    let r=m.validate_global(&mut Context::default()).unwrap();assert!(r.is_valid_solid,"{r:?}");assert_eq!(r.components,1);assert_eq!(r.intersection_count,0);assert_eq!(r.local.self_intersections,SelfIntersectionStatus::Clear);
    m.append(&cube(2.,[4.,0.,0.]),&mut Context::default()).unwrap();let r=m.validate_global(&mut Context::default()).unwrap();assert!(r.is_valid_solid,"{r:?}");assert_eq!(r.components,2);
    let r=inward(&cube(2.,[0.;3])).validate_global(&mut Context::default()).unwrap();assert!(r.is_embedded_surface);assert!(!r.is_valid_solid);assert_eq!(r.orientation_errors,1);
}

#[test]
fn overlapping_and_distinct_id_touching_shells_are_not_certified(){
    for offset in [[0.5,0.5,0.5],[2.,0.,0.],[2.,2.,2.],[0.;3]]{
        let mut m=cube(2.,[0.;3]);m.append(&cube(2.,offset),&mut Context::default()).unwrap();
        let r=m.validate_global(&mut Context::default()).unwrap();assert!(r.local.is_closed_manifold);assert!(!r.is_valid_solid,"offset={offset:?}, {r:?}");assert!(!r.is_embedded_surface);assert!(r.intersection_count>0);assert_eq!(r.local.self_intersections,SelfIntersectionStatus::Found);
    }
}

#[test]
fn nested_cavity_requires_inward_inner_shell(){
    let mut m=cube(4.,[0.;3]);m.append(&cube(2.,[0.;3]),&mut Context::default()).unwrap();let r=m.validate_global(&mut Context::default()).unwrap();assert!(r.is_embedded_surface);assert!(!r.is_valid_solid);assert_eq!(r.orientation_errors,1);
    let mut m=cube(4.,[0.;3]);m.append(&inward(&cube(2.,[0.;3])),&mut Context::default()).unwrap();let r=m.validate_global(&mut Context::default()).unwrap();assert!(r.is_valid_solid,"{r:?}");assert_eq!(r.components,2);
}

#[test]
fn legal_shared_boundaries_and_illegal_crossings_are_distinguished(){
    let legal=soup(&[[0.,0.,0.],[1.,0.,0.],[0.,1.,0.],[1.,1.,0.]],&[[0,1,2],[1,3,2]]);let r=legal.validate_global(&mut Context::default()).unwrap();assert!(r.is_embedded_surface,"{r:?}");assert!(!r.is_valid_solid);
    // Shared edge IDs do not excuse two triangles overlapping on the same side.
    let folded=soup(&[[0.,0.,0.],[2.,0.,0.],[0.,2.,0.],[0.5,0.5,0.]],&[[0,1,2],[1,0,3]]);assert!(folded.validate_global(&mut Context::default()).unwrap().intersection_count>0);
    let cross=soup(&[[-2.,0.,0.],[2.,0.,0.],[0.,2.,0.],[0.,0.5,-1.],[0.,0.5,1.],[0.,-1.,0.]],&[[0,1,2],[3,4,5]]);assert!(cross.validate_global(&mut Context::default()).unwrap().intersection_count>0);
    // Interior edge contact with a different vertex identity is a T-junction.
    let touching=soup(&[[0.,0.,0.],[2.,0.,0.],[0.,2.,0.],[1.,0.,0.],[1.,-1.,0.],[2.,-1.,0.]],&[[0,1,2],[3,4,5]]);assert!(touching.validate_global(&mut Context::default()).unwrap().intersection_count>0);
}

#[test]
fn cancellation_and_work_or_memory_exhaustion_return_no_certificate(){
    let m=cube(2.,[0.;3]);let before=m.to_bytes(&mut Context::default()).unwrap();
    let calls=Cell::new(0);let count=||{calls.set(calls.get()+1);false};let mut ctx=Context::new(Limits::default(),Some(&count));m.validate_global(&mut ctx).unwrap();let complete=calls.get();calls.set(0);
    let cancel=||{calls.set(calls.get()+1);calls.get()>complete-3};assert_eq!(m.validate_global(&mut Context::new(Limits::default(),Some(&cancel))).unwrap_err(),MeshError::Cancelled);
    let limits=Limits{max_work:ctx.work_used()-1,..Limits::default()};assert!(matches!(m.validate_global(&mut Context::new(limits,None)),Err(MeshError::Budget{resource:"work",..})));
    let tri=m.triangulate(&mut Context::default()).unwrap();let limits=Limits{max_bytes:m.memory_bytes()+tri.vertices.len()*1024+tri.triangles.len()*1024-1,..Limits::default()};assert!(matches!(m.validate_global(&mut Context::new(limits,None)),Err(MeshError::Budget{resource:"bytes",..})));
    assert_eq!(before,m.to_bytes(&mut Context::default()).unwrap());
}

#[test]
fn diagnostic_page_is_bounded_but_total_pairs_are_exact(){
    let triangle=soup(&[[0.,0.,0.],[1.,0.,0.],[0.,1.,0.]],&[[0,1,2]]);let mut mesh=triangle.clone();
    for _ in 1..20{mesh.append(&triangle,&mut Context::default()).unwrap();}
    let r=mesh.validate_global(&mut Context::default()).unwrap();assert_eq!(r.intersection_count,190);assert_eq!(r.intersections.len(),128);assert_eq!(r.intersections_omitted,62);assert!(!r.is_embedded_surface);
    assert!(r.intersections.windows(2).all(|p|(p[0].a,p[0].b)<(p[1].a,p[1].b)));
}

#[test]
fn broadphase_selects_a_separating_axis_for_long_parallel_slats(){
    // All 1500 triangles overlap the long axis and the normal axis; only the
    // spacing axis separates them. Rotating the mesh must not change admission.
    let count=1500;
    for spacing_axis in 0..3{
        let long_axis=(spacing_axis+1)%3;let mut positions=Vec::new();let mut polygons=Vec::new();
        for i in 0..count{let base=positions.len()as u32;let mut p=[1.;3];p[spacing_axis]=2.*i as f64;
            p[long_axis]=0.;positions.push(p);p[long_axis]=100.;positions.push(p);p[spacing_axis]+=0.5;positions.push(p);
            polygons.push(Polygon::new(vec![base,base+1,base+2]));}
        let mesh=Mesh::from_polygons(&positions,&polygons,&mut Context::default()).unwrap();
        let mut ctx=Context::new(Limits{max_work:600_000,..Limits::default()},None);
        let report=mesh.validate_global(&mut ctx).unwrap();assert!(report.is_embedded_surface);assert_eq!(report.candidate_pairs,0);assert_eq!(report.components,count);
    }
}
