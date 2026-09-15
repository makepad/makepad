//! Derived LOD geometry and portable collision descriptors. Editable sources and
//! their stable IDs remain untouched; these meshes exist only in a render view.
use crate::{Document,Result,Error,mesh,SceneNode,CollisionProxy,transform::*};
use makepad_gltf::JsonValue;
fn object(values:Vec<(&str,JsonValue)>)->JsonValue{JsonValue::Object(values.into_iter().map(|(k,v)|(k.into(),v)).collect())}
fn vector(v:[f64;3])->JsonValue{JsonValue::Array(v.into_iter().map(JsonValue::F64).collect())}
pub(crate) fn lod_name(object:usize,level:usize)->String{format!("__derived_lod_{object}_{level}")}
pub(crate) fn expand(doc:&mut Document,ctx:&mut mesh::Context<'_>)->Result<()> {
    let limits=doc.limits().clone();
    let names=doc.objects().map(|(n,_)|n.to_owned()).collect::<Vec<_>>();
    let total=doc.scene().lods.values().map(Vec::len).sum::<usize>();if total+names.len()>256{return Err(Error::Budget("derived LOD objects"));}
    for (i,name) in names.iter().enumerate(){
        let levels=doc.scene().lods.get(name).cloned().unwrap_or_default();
        if levels.is_empty(){continue;}
        let original=crate::morph_export::source(doc,name)?;
        if doc.rig().morphs.values().any(|m|m.object==original){return Err(Error::Invalid("LOD generation requires shape keys baked or removed on that object"));}
        let base=doc.object(name).ok_or_else(||Error::MissingObject(name.clone()))?.clone();
        let mut previous=base.triangulate(ctx)?.triangles.len();
        for (level,options) in levels.iter().enumerate(){
            ctx.checkpoint(1)?;let derived=lod_name(i,level);if doc.object(&derived).is_some(){return Err(Error::Invalid("reserved derived LOD name collision"));}
            let mut mesh=base.clone();let result=mesh.decimate(options.target_faces,options.max_error,ctx)?;
            if result.achieved_faces>=previous{return Err(Error::Invalid("LOD target cannot reduce protected topology; relax seams/creases or choose a reachable target"));}previous=result.achieved_faces;
            let bytes=doc.objects().map(|(_,m)|m.memory_bytes()).sum::<usize>();if bytes.saturating_add(mesh.memory_bytes()*3)>ctx.limits.max_bytes{return Err(Error::Budget("derived LOD geometry"));}
            doc.state.surface.transfer_vertex_colors(name,&base,&derived,&mesh,&limits,ctx)?;
            let node=doc.scene().nodes.get(name).cloned().unwrap_or_default();doc.state.scene.nodes.insert(derived.clone(),SceneNode{linked_to:None,..node});
            doc.state.objects.insert(derived,mesh);
        }
    }Ok(())
}
pub(crate) fn collision(doc:&Document,name:&str,ctx:&mut mesh::Context<'_>)->Result<Option<JsonValue>>{
    let Some(proxy)=doc.scene().colliders.get(name)else{return Ok(None);};
    let source=doc.object(name).ok_or_else(||Error::MissingObject(name.into()))?;
    let mut bounds=[[f64::INFINITY;3],[f64::NEG_INFINITY;3]];
    // Skinned export meshes are in bind space. Collision descriptors belong to
    // the object node, so convert back before emitting its local proxy.
    let local=if doc.skeleton().is_some(){inverse(doc.scene().world_matrix(name)?)?}else{IDENTITY_MATRIX};
    for v in source.vertices(){let p=transform_point(local,v.position);for d in 0..3{bounds[0][d]=bounds[0][d].min(p[d]);bounds[1][d]=bounds[1][d].max(p[d]);}}
    if bounds.iter().flatten().any(|v|!v.is_finite()){return Err(Error::Invalid("collision proxy empty object"));}
    let value=match proxy {
        CollisionProxy::Box=>{if (0..3).any(|d|bounds[1][d]<=bounds[0][d]){return Err(Error::Invalid("box collider requires positive volume"));}object(vec![("kind",JsonValue::String("box".into())),("min",vector(bounds[0])),("max",vector(bounds[1]))])},
        CollisionProxy::Sphere=>{let center=mul(add(bounds[0],bounds[1]),0.5);let radius=source.vertices().iter().map(|v|length(sub(transform_point(local,v.position),center))).fold(0.,f64::max);object(vec![("kind",JsonValue::String("sphere".into())),("center",vector(center)),("radius",JsonValue::F64(radius))])},
        CollisionProxy::Mesh{object:name_source}=>{
            let reference=doc.object(name_source).ok_or_else(||Error::MissingObject(name_source.clone()))?;let tri=reference.triangulate(ctx)?;
            if tri.triangles.len()>8192{return Err(Error::Budget("collision mesh triangles"));}
            let to_local=if doc.skeleton().is_some(){local}else{matrix_mul(inverse(doc.scene().world_matrix(name)?)?,doc.scene().world_matrix(name_source)?)};
            let positions=tri.vertices.iter().map(|v|vector(transform_point(to_local,v.position))).collect();
            let indices=tri.triangles.iter().flat_map(|t|t.indices).map(|i|JsonValue::U64(i as u64)).collect();
            object(vec![("kind",JsonValue::String("mesh".into())),("positions",JsonValue::Array(positions)),("indices",JsonValue::Array(indices))])
        }
    };Ok(Some(value))
}
