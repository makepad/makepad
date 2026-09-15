use makepad_mesh_edit::*;
use std::cell::Cell;
fn ctx()->Context<'static>{Context::default()}
fn ids(m:&Mesh)->Vec<VertexId>{m.vertices().iter().map(|v|v.id).collect()}
fn grid(n:usize,bump:f64)->Mesh{
    let positions=(0..n).flat_map(|z|(0..n).map(move |x|[2.*x as f64/(n-1)as f64-1.,if x==n/2&&z==n/2{bump}else{0.},2.*z as f64/(n-1)as f64-1.])).collect::<Vec<_>>();
    let mut polys=Vec::new();for z in 0..n-1{for x in 0..n-1{let corners=[z*n+x,(z+1)*n+x,(z+1)*n+x+1,z*n+x+1];polys.push(Polygon{vertices:corners.map(|x|x as u32).to_vec(),uvs:corners.map(|i|[(i%n)as f64/(n-1)as f64,(i/n)as f64/(n-1)as f64]).to_vec(),material:0});}}
    Mesh::from_polygons(&positions,&polys,&mut ctx()).unwrap()
}
#[test]
fn twist_taper_bend_and_lattice_change_positions_without_changing_attributes(){
    let original=Mesh::cube([2.;3],&mut ctx()).unwrap();let mut m=original.clone();let vertices=ids(&m);
    m.deform(&vertices,&Deformation::Twist{axis:1,angle:0.3,range:[-1.,1.]},&mut ctx()).unwrap();
    assert_eq!(m.vertices()[0].position,original.vertices()[0].position);assert_ne!(m.vertices()[2].position,original.vertices()[2].position);
    assert_eq!(m.corners().iter().map(|c|c.uv).collect::<Vec<_>>(),original.corners().iter().map(|c|c.uv).collect::<Vec<_>>());
    let mut m=original.clone();m.deform(&vertices,&Deformation::Taper{axis:1,scales:[1.,0.5],range:[-1.,1.]},&mut ctx()).unwrap();
    assert_eq!(m.vertices()[2].position,[0.5,1.,-0.5]);
    let mut m=original.clone();m.deform(&vertices,&Deformation::Bend{axis:1,angle:0.2,range:[-1.,1.]},&mut ctx()).unwrap();assert_ne!(m,original);
    let mut m=original.clone();m.lattice(&vertices,[[-1.;3],[1.;3]],[2;3],&[[1.,2.,3.];8],&mut ctx()).unwrap();
    for (a,b) in original.vertices().iter().zip(m.vertices()){for d in 0..3{assert!((b.position[d]-a.position[d]-(d+1)as f64).abs()<1e-12);}}
    assert_eq!(m.faces(),original.faces());assert_eq!(ids(&m),vertices);
}
#[test]
fn reference_projection_hits_triangle_interior_and_distance_failure_is_atomic(){
    let reference=Mesh::plane([4.,4.],&mut ctx()).unwrap();let mut m=Mesh::plane([2.,2.],&mut ctx()).unwrap();let vertices=ids(&m);
    m.transform(&vertices,[[1.,0.,0.,0.],[0.,1.,0.,1.],[0.,0.,1.,0.],[0.,0.,0.,1.]],&mut ctx()).unwrap();let before=m.clone();
    assert!(m.shrinkwrap(&vertices,&reference,0.5,0.,&mut ctx()).is_err());assert_eq!(m,before);
    m.shrinkwrap(&vertices,&reference,2.,0.25,&mut ctx()).unwrap();assert!(m.vertices().iter().all(|v|(v.position[1]-0.25).abs()<1e-12));
}
#[test]
fn sculpt_masks_and_symmetry_do_not_double_displace_the_center(){
    let mut m=grid(5,0.);let original=m.clone();let vertices=ids(&m);let center=m.vertices()[12].id;let protected=m.vertices()[13].id;
    let brush=SculptBrush{kind:SculptKind::Inflate,center:[0.;3],normal:[0.,1.,0.],radius:2.,strength:0.1,max_displacement:0.1,masks:vec![(protected,1.)],symmetry:5};
    m.sculpt(&vertices,&brush,&mut ctx()).unwrap();assert!((m.vertex(center).unwrap().position[1]-0.1).abs()<1e-12);assert_eq!(m.vertex(protected),original.vertex(protected));
    let mut bump=grid(5,0.2);let center=bump.vertices()[12].id;let brush=SculptBrush{kind:SculptKind::Flatten,strength:0.15,max_displacement:0.15,masks:Vec::new(),symmetry:0,..brush};
    bump.sculpt(&ids(&bump),&brush,&mut ctx()).unwrap();assert!(bump.vertex(center).unwrap().position[1]<0.2);
    let before=bump.clone();let calls=Cell::new(0);let stop=||{calls.set(calls.get()+1);calls.get()>10};assert!(bump.sculpt(&ids(&bump),&brush,&mut Context::new(Limits::default(),Some(&stop))).is_err());assert_eq!(bump,before);
}
#[test]
fn box_islands_pack_without_overlap_and_pins_survive_source_and_array(){
    let mut m=Mesh::cube([2.;3],&mut ctx()).unwrap();let faces=m.faces().iter().map(|f|f.id).collect::<Vec<_>>();m.project_uv_box(&faces,[1.;2],[0.;2],&mut ctx()).unwrap();
    assert_eq!(m.uv_islands(&faces,&mut ctx()).unwrap().len(),6);let pin=m.corners()[0].id;m.pin_uv(&[pin],true,&mut ctx()).unwrap();let before=m.clone();assert!(m.pack_uv(&faces,0.01,&mut ctx()).is_err());assert_eq!(m,before);
    let source=m.to_bytes(&mut ctx()).unwrap();assert_eq!(&source[..8],b"MPMESH02");assert_eq!(Mesh::from_bytes(&source,&mut ctx()).unwrap(),m);
    m.array(2,[3.,0.,0.],&mut ctx()).unwrap();assert_eq!(m.uv_pins().len(),2);
    m.pin_uv(&m.uv_pins().iter().copied().collect::<Vec<_>>(),false,&mut ctx()).unwrap();let faces=m.faces().iter().map(|f|f.id).collect::<Vec<_>>();m.pack_uv(&faces,0.01,&mut ctx()).unwrap();
    let islands=m.uv_islands(&faces,&mut ctx()).unwrap();for (i,a) in islands.iter().enumerate(){for d in 0..2{assert!(a.bounds[0][d]>=0.01-1e-12&&a.bounds[1][d]<=0.99+1e-12);}for b in &islands[i+1..]{assert!((0..2).any(|d|a.bounds[1][d]<=b.bounds[0][d]||b.bounds[1][d]<=a.bounds[0][d]));}}
    assert_eq!(&m.to_bytes(&mut ctx()).unwrap()[..8],b"MPMESH01");
}
#[test]
fn uv_relax_fixes_boundary_and_pins_while_moving_interior_toward_neighbors(){
    let mut m=grid(5,0.);let center=m.vertices()[12].id;let corners=m.corners().iter().filter(|c|c.vertex==center).map(|c|c.id).collect::<Vec<_>>();for &c in &corners{m.set_corner_uv(c,[0.8,0.8],&mut ctx()).unwrap();}
    let boundary=m.corners()[0].clone();let faces=m.faces().iter().map(|f|f.id).collect::<Vec<_>>();m.pin_uv(&[corners[0]],true,&mut ctx()).unwrap();m.relax_uv(&faces,16,1.,&mut ctx()).unwrap();assert_eq!(m.corner(corners[0]).unwrap().uv,[0.8,0.8]);
    m.pin_uv(&corners,false,&mut ctx()).unwrap();m.relax_uv(&faces,32,0.5,&mut ctx()).unwrap();let uv=m.corner(corners[0]).unwrap().uv;assert!((uv[0]-0.5).abs()<0.01);assert_eq!(m.corner(boundary.id),Some(&boundary));
}
#[test]
fn curved_disk_unwrap_produces_positive_area_and_closed_charts_are_complete(){
    let mut m=grid(5,0.25);let faces=m.faces().iter().map(|f|f.id).collect::<Vec<_>>();m.unwrap_uv(&faces,&mut ctx()).unwrap();let islands=m.uv_islands(&faces,&mut ctx()).unwrap();assert_eq!(islands.len(),1);assert!(islands[0].area>0.5);m.pack_uv(&faces,0.02,&mut ctx()).unwrap();
    let mut cube=Mesh::cube([2.;3],&mut ctx()).unwrap();let faces=cube.faces().iter().map(|f|f.id).collect::<Vec<_>>();cube.unwrap_uv(&faces,&mut ctx()).unwrap();assert_eq!(cube.uv_islands(&faces,&mut ctx()).unwrap().iter().map(|i|i.faces.len()).sum::<usize>(),6);
}
#[test]
fn decimation_reduces_a_continuous_triangle_grid_and_reports_constraints(){
    let mut m=grid(5,0.);let out=m.decimate(20,0.5,&mut ctx()).unwrap();assert!(out.collapses>0);assert!(out.achieved_faces<=20);assert_eq!(out.achieved_faces,m.faces().len());assert!(m.validate(&mut ctx()).unwrap().is_valid_surface);
    let mut cube=Mesh::cube([2.;3],&mut ctx()).unwrap();let out=cube.decimate(4,10.,&mut ctx()).unwrap();assert_eq!(out.collapses,0);assert_eq!(out.achieved_faces,12); // Explicit UV seams constrain every cube vertex.
}
#[test]
fn affine_normals_reflections_and_absolute_position_batches_preserve_identity(){
    let mut m=Mesh::cube([2.;3],&mut ctx()).unwrap();m.recalculate_normals(false,0.,&mut ctx()).unwrap();let vertices=ids(&m);let original=m.clone();
    m.transform(&vertices,[[-2.,0.,0.,1.],[0.,3.,0.,2.],[0.,0.,4.,3.],[0.,0.,0.,1.]],&mut ctx()).unwrap();
    assert!(m.validate(&mut ctx()).unwrap().is_closed_manifold);let t=m.triangulate(&mut ctx()).unwrap();let volume=t.triangles.iter().map(|t0|{let p=t0.indices.map(|i|t.vertices[i as usize].position);(p[0][0]*(p[1][1]*p[2][2]-p[1][2]*p[2][1])+p[0][1]*(p[1][2]*p[2][0]-p[1][0]*p[2][2])+p[0][2]*(p[1][0]*p[2][1]-p[1][1]*p[2][0]))/6.}).sum::<f64>();assert!((volume-192.).abs()<1e-9);
    assert!(m.corners().iter().all(|c|c.normal.is_some()));assert_eq!(ids(&m),vertices);
    let before=m.clone();let id=vertices[0];assert!(m.set_positions_bulk(&[(id,[0.;3]),(id,[1.;3])],&mut ctx()).is_err());assert_eq!(m,before);
    let positions=original.vertices().iter().map(|v|(v.id,v.position)).collect::<Vec<_>>();m.set_positions_bulk(&positions,&mut ctx()).unwrap();assert_eq!(m.vertices(),original.vertices());
}

#[test]
fn decimation_uses_local_candidates_and_preserves_atomic_budget_refusal(){
    let mut mesh=grid(40,0.);let before=mesh.clone();
    let mut ctx=Context::new(Limits{max_work:80_000_000,..Limits::default()},None);
    let result=mesh.decimate(1500,0.1,&mut ctx).unwrap();
    assert!(result.collapses>500,"{}",result.collapses);assert!(result.achieved_faces<=1500,"{}",result.achieved_faces);
    assert!(mesh.validate(&mut Context::default()).unwrap().is_valid_surface);
    let surviving=mesh.vertices().iter().map(|v|v.id).collect::<std::collections::BTreeSet<_>>();
    for (source,targets) in &result.changes.remapped{if let ElementId::Vertex(v)=source{assert!(before.vertex(*v).is_some());for target in targets{if let ElementId::Vertex(v)=target{assert!(surviving.contains(v));}}}}
    let mut refused=before.clone();assert!(matches!(refused.decimate(1500,0.1,&mut Context::new(Limits{max_work:ctx.work_used()-1,..Limits::default()},None)),Err(MeshError::Budget{resource:"work",..})));assert_eq!(refused,before);
    let calls=std::cell::Cell::new(0usize);let cancel=||{calls.set(calls.get()+1);calls.get()>20_000};
    assert!(matches!(refused.decimate(1500,0.1,&mut Context::new(Limits::default(),Some(&cancel))),Err(MeshError::Cancelled)));assert_eq!(refused,before);
}
