//! Derived LOD geometry and portable collision descriptors. Editable sources and
//! their stable IDs remain untouched; these meshes exist only in a render view.
use crate::{Document,Result,Error,mesh,SceneNode,CollisionProxy,transform::*};
use crate::authoring_export::f32_value;
use makepad_gltf::{GlbCollisionMesh,JsonValue};
use makepad_remesh::simplify::{simplify_levels, SimplifyInput, SimplifyLevel};
use std::collections::HashMap;
#[path="delivery_hull.rs"]
mod hull;
pub(crate) enum Collision{Shape(JsonValue),Mesh(GlbCollisionMesh)}
fn mesh_proxy(positions:&[[f64;3]],triangles:&[[u32;3]])->Result<GlbCollisionMesh>{
    Ok(GlbCollisionMesh{positions:positions.iter().map(|p|Ok([f32_value(p[0])?,f32_value(p[1])?,f32_value(p[2])?])).collect::<Result<_>>()?,indices:triangles.iter().flatten().copied().collect()})
}
fn object(values:Vec<(&str,JsonValue)>)->JsonValue{JsonValue::Object(values.into_iter().map(|(k,v)|(k.into(),v)).collect())}
fn vector(v:[f64;3])->JsonValue{JsonValue::Array(v.into_iter().map(JsonValue::F64).collect())}
pub(crate) fn lod_name(object:usize,level:usize)->String{format!("__derived_lod_{object}_{level}")}
/// Render-only outcomes; repeats name the base or an earlier derived mesh.
#[derive(Clone,Debug)]
pub(crate) enum LodGeometry {Derived(String),Previous(String),Empty}
#[derive(Default)]
pub(crate) struct LodDelivery {pub levels:std::collections::BTreeMap<String,Vec<LodGeometry>>,pub warnings:Vec<String>}
pub(crate) fn expand(doc: &mut Document, ctx: &mut mesh::Context<'_>, cancelled: Option<&dyn Fn() -> bool>) -> Result<LodDelivery> {
    let limits = doc.limits().clone();
    let mut delivery = LodDelivery::default();
    let names = doc.objects().map(|(n, _)| n.to_owned()).collect::<Vec<_>>();
    let mut bytes = doc.objects().map(|(_, m)| m.memory_bytes()).sum::<usize>();
    for (i, name) in names.iter().enumerate() {
        let levels = doc.scene().lods.get(name).cloned().unwrap_or_default();
        if levels.is_empty() { continue; }
        let base = doc.object(name).ok_or_else(|| Error::MissingObject(name.clone()))?.clone();
        let mut previous = name.clone();
        let mut outcomes = Vec::with_capacity(levels.len());
        // A part's work/geometry budget cannot poison the next part's context.
        let mut part_ctx = mesh::Context::new(ctx.limits.clone(), cancelled);
        let requested: Vec<_> = levels.iter().filter(|level| level.target_faces != 0)
            .map(|level| SimplifyLevel { max_error: level.max_error, target_faces: level.target_faces }).collect();
        let prepared = (|| -> Result<_> {
            if requested.is_empty() { return Ok(None); }
            for (level, options) in levels.iter().enumerate() {
                if options.target_faces != 0 && doc.object(&lod_name(i, level)).is_some() {
                    return Err(Error::Invalid("reserved derived LOD name collision"));
                }
            }
            let original = crate::morph_export::source(doc, name)?;
            if doc.rig().morphs.values().any(|m| m.object == original) {
                return Err(Error::Invalid("LOD generation requires shape keys baked or removed on that object"));
            }
            let mut base_world=base.clone();
            // Error is measured in world-space metres. Skinned meshes were
            // already baked into bind space by the evaluation pass.
            let world=if doc.skeleton().is_some(){IDENTITY_MATRIX}else{doc.scene().world_matrix(name)?};
            let local=inverse(world)?;
            if world!=IDENTITY_MATRIX{let ids=base_world.vertices().iter().map(|v|v.id).collect::<Vec<_>>();base_world.transform(&ids,world,&mut part_ctx)?;}
            let source = base_world.triangulate(&mut part_ctx)?;
            let source_faces = source.triangles.len();
            let group_count = source.triangles.iter().map(|t| t.material).max()
                .map_or(Ok(0), |material| (material as usize).checked_add(1).ok_or(Error::Budget("LOD groups")))?;
            let mut uv_bound = Vec::with_capacity(group_count);
            for material in 0..group_count {
                part_ctx.checkpoint(1)?;
                uv_bound.push(crate::export::material_samples_texture(doc, material as u32));
            }
            let mut positions = Vec::new();
            let mut source_vertices = Vec::new();
            let mut position_ids = HashMap::new();
            let mut triangles = Vec::with_capacity(source.triangles.len());
            let mut groups = Vec::with_capacity(source.triangles.len());
            let mut wedges = Vec::with_capacity(source.triangles.len());
            let mut wedge_ids = HashMap::new();
            let mut wedge_uv = Vec::new();
            // Normals do not split wedges, so one wedge can cover corners
            // of a hard edge; each keeps every source normal it covers.
            let mut wedge_normals = Vec::with_capacity(source.triangles.len() * 3);
            for triangle in &source.triangles {
                part_ctx.checkpoint(1)?;
                let mut indices = [0; 3];
                let mut corner_wedges = [0; 3];
                for (corner, source_index) in triangle.indices.into_iter().enumerate() {
                    let vertex = &source.vertices[source_index as usize];
                    let next = u32::try_from(positions.len()).map_err(|_| Error::Budget("LOD positions"))?;
                    let position = *position_ids.entry(vertex.source_vertex).or_insert_with(|| {
                        positions.push(vertex.position);
                        source_vertices.push(vertex.source_vertex);
                        next
                    });
                    let uv_key = if uv_bound[triangle.material as usize] { vertex.uv.map(f64::to_bits) } else { [0; 2] };
                    let next = u32::try_from(wedge_uv.len()).map_err(|_| Error::Budget("LOD wedges"))?;
                    let wedge = *wedge_ids.entry((position, triangle.material, uv_key)).or_insert_with(|| {
                        wedge_uv.push(vertex.uv);
                        next
                    });
                    wedge_normals.push((wedge, vertex.normal.map(f64::to_bits)));
                    indices[corner] = position;
                    corner_wedges[corner] = wedge;
                }
                triangles.push(indices);
                wedges.push(corner_wedges);
                groups.push(triangle.material);
            }
            wedge_normals.sort_unstable();
            wedge_normals.dedup();
            let mut normal_starts = vec![0usize; wedge_uv.len() + 1];
            for &(wedge, _) in &wedge_normals { normal_starts[wedge as usize + 1] += 1; }
            for i in 1..normal_starts.len() { normal_starts[i] += normal_starts[i - 1]; }
            let input = SimplifyInput {
                positions: &positions, triangles: &triangles, wedges: &wedges,
                wedge_uv: &wedge_uv, groups: &groups, uv_bound: &uv_bound,
            };
            let mut control_error = None;
            let result = simplify_levels(
                &input, &requested, &mut |work| {
                    match part_ctx.checkpoint(work as u64) {
                        Ok(()) => true,
                        Err(error) => { control_error = Some(error); false }
                    }
                });
            // simplify_levels uses None for a stopped control callback. Preserve
            // the context's cancellation/budget error before keep-previous.
            if let Some(error) = control_error { return Err(error.into()); }
            let Some(result) = result.map_err(mesh::MeshError::InvalidInput)? else { return Ok(None); };
            Ok(Some((base_world, world, local, positions, source_vertices, wedge_uv,
                wedge_normals, normal_starts, source_faces, result)))
        })();
        let prepared = match prepared {
            Err(error @ Error::Mesh(mesh::MeshError::Cancelled)) => return Err(error),
            Err(error) => {
                for (level, options) in levels.iter().enumerate() {
                    ctx.checkpoint(1)?;
                    if options.target_faces == 0 {
                        outcomes.push(LodGeometry::Empty);
                    } else {
                        delivery.warnings.push(format!("LOD part '{name}' level {} keeps previous geometry: {error}", level + 1));
                        outcomes.push(LodGeometry::Previous(previous.clone()));
                    }
                }
                delivery.levels.insert(name.clone(), outcomes);
                continue;
            }
            Ok(prepared) => prepared,
        };
        let Some((base_world, world, local, positions, source_vertices, wedge_uv,
            wedge_normals, normal_starts, mut previous_faces, results)) = prepared else {
            for options in &levels {
                ctx.checkpoint(1)?;
                outcomes.push(if options.target_faces == 0 { LodGeometry::Empty }
                    else { LodGeometry::Previous(previous.clone()) });
            }
            delivery.levels.insert(name.clone(), outcomes);
            continue;
        };
        let mut results = results.into_iter();
        for (level, options) in levels.iter().enumerate() {
            ctx.checkpoint(1)?;
            if options.target_faces == 0 { outcomes.push(LodGeometry::Empty); continue; }
            let derived = lod_name(i, level);
            let result = results.next().expect("one snapshot per non-empty LOD level");
            let achieved = result.report.faces;
            // A part that cannot reduce within the bound keeps its previous mesh:
            // this is data for the LOD table, not a warning.
            if achieved >= previous_faces {
                outcomes.push(LodGeometry::Previous(previous.clone()));
                continue;
            }
            let attempt = (|| -> Result<mesh::Mesh> {
                let mut derived_triangles = Vec::with_capacity(result.triangles.len());
                for (corners, material) in result.triangles.into_iter().zip(result.groups) {
                    part_ctx.checkpoint(1)?;
                    // Each corner takes the source normal of its wedge that is
                    // closest to this face's normal: an unchanged face at a
                    // hard edge keeps its own side, a smooth vertex its one normal.
                    let [a, b, c] = corners.map(|(position, _)| positions[position as usize]);
                    let face = cross(sub(b, a), sub(c, a));
                    derived_triangles.push(mesh::DerivedTriangle {
                        corners: corners.map(|(position, wedge)| mesh::DerivedCorner {
                            vertex: source_vertices[position as usize],
                            uv: wedge_uv[wedge as usize],
                            normal: wedge_normals[normal_starts[wedge as usize]..normal_starts[wedge as usize + 1]].iter()
                                .map(|(_, bits)| bits.map(f64::from_bits))
                                .max_by(|x, y| dot(*x, face).total_cmp(&dot(*y, face)))
                                .expect("every wedge has a source corner"),
                        }),
                        material,
                    });
                }
                let mut mesh = base_world.derive_triangles(&derived_triangles, &mut part_ctx)?;
                if world!=IDENTITY_MATRIX{let ids=mesh.vertices().iter().map(|v|v.id).collect::<Vec<_>>();mesh.transform(&ids,local,&mut part_ctx)?;}
                // Export triangulates every derived mesh. One it would refuse is this
                // part's failure at this level (it keeps its previous geometry), never
                // the whole compile's.
                mesh.triangulate(&mut part_ctx)?;
                if bytes.saturating_add(mesh.memory_bytes().saturating_mul(3))>ctx.limits.max_bytes{return Err(Error::Budget("derived LOD geometry"));}
                doc.state.surface.transfer_vertex_colors(name,&base,&derived,&mesh,&limits,&mut part_ctx)?;
                Ok(mesh)
            })();
            match attempt {
                Ok(mesh) => {
                    bytes = bytes.saturating_add(mesh.memory_bytes());
                    previous_faces = achieved;
                    previous = derived.clone();
                    let node = doc.scene().nodes.get(name).cloned().unwrap_or_default();
                    doc.state.scene.nodes.insert(derived.clone(), SceneNode { linked_to: None, ..node });
                    doc.state.objects.insert(derived.clone(), mesh);
                    outcomes.push(LodGeometry::Derived(derived));
                }
                Err(error @ Error::Mesh(mesh::MeshError::Cancelled)) => return Err(error),
                Err(error) => {
                    delivery.warnings.push(format!("LOD part '{name}' level {} keeps previous geometry: {error}", level + 1));
                    outcomes.push(LodGeometry::Previous(previous.clone()));
                }
            }
        }
        delivery.levels.insert(name.clone(), outcomes);
    }
    Ok(delivery)
}
pub(crate) fn collision(doc:&Document,name:&str,ctx:&mut mesh::Context<'_>)->Result<Option<Collision>>{
    let Some(proxy)=doc.scene().colliders.get(name)else{return Ok(None);};
    let source=doc.object(name).ok_or_else(||Error::MissingObject(name.into()))?;
    let mut bounds=[[f64::INFINITY;3],[f64::NEG_INFINITY;3]];
    // Skinned export meshes are in bind space. Collision descriptors belong to
    // the object node, so convert back before emitting its local proxy.
    let local=if doc.skeleton().is_some(){inverse(doc.scene().world_matrix(name)?)?}else{IDENTITY_MATRIX};
    for v in source.vertices(){let p=transform_point(local,v.position);for d in 0..3{bounds[0][d]=bounds[0][d].min(p[d]);bounds[1][d]=bounds[1][d].max(p[d]);}}
    if bounds.iter().flatten().any(|v|!v.is_finite()){return Err(Error::Invalid("collision proxy empty object"));}
    let value=match proxy {
        CollisionProxy::Box=>{if (0..3).any(|d|bounds[1][d]<=bounds[0][d]){return Err(Error::Invalid("box collider requires positive volume"));}Collision::Shape(object(vec![("kind",JsonValue::String("box".into())),("min",vector(bounds[0])),("max",vector(bounds[1]))]))},
        CollisionProxy::Sphere=>{let center=mul(add(bounds[0],bounds[1]),0.5);let radius=source.vertices().iter().map(|v|length(sub(transform_point(local,v.position),center))).fold(0.,f64::max);Collision::Shape(object(vec![("kind",JsonValue::String("sphere".into())),("center",vector(center)),("radius",JsonValue::F64(radius))]))},
        CollisionProxy::Hull=>{
            let points=source.vertices().iter().map(|v|transform_point(local,v.position)).collect::<Vec<_>>();
            let (positions,triangles)=hull::convex_hull(&points).ok_or(Error::Invalid("hull collider needs a solid, non-flat object"))?;
            Collision::Mesh(mesh_proxy(&positions,&triangles)?)
        }
        CollisionProxy::Capsule=>{let (positions,triangles)=hull::capsule(bounds);Collision::Mesh(mesh_proxy(&positions,&triangles)?)}
        CollisionProxy::Mesh{object:name_source}=>{
            let reference=doc.object(name_source).ok_or_else(||Error::MissingObject(name_source.clone()))?;let tri=reference.triangulate(ctx)?;
            // Any size: libs/render cooks a mesh over its per-mesh bound as several pieces.
            let to_local=if doc.skeleton().is_some(){local}else{matrix_mul(inverse(doc.scene().world_matrix(name)?)?,doc.scene().world_matrix(name_source)?)};
            // One position per source vertex: the triangulation repeats it for every face corner.
            let mut welded=HashMap::new();let mut positions=Vec::new();
            let indices=tri.triangles.iter().flat_map(|t|t.indices).map(|i|{let v=&tri.vertices[i as usize];
                *welded.entry(v.source_vertex).or_insert_with(||{positions.push(transform_point(to_local,v.position));(positions.len()-1) as u32})}).collect();
            Collision::Mesh(GlbCollisionMesh{positions:positions.iter().map(|p|Ok([f32_value(p[0])?,f32_value(p[1])?,f32_value(p[2])?])).collect::<Result<_>>()?,indices})
        }
    };Ok(Some(value))
}
