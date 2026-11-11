//! GI scene snapshots: the static world as instances over SHARED meshes.
//!
//! A snapshot copies transforms and materials, not geometry: each render
//! mesh is copied into the GI mesh cache once (per geometry, texture and
//! terrain/voxel revision) and reused by every later snapshot, so a script
//! edit or hot reload costs O(instances) here and the voxel worker does the
//! rest. Only instances overlapping the region around the camera are sent;
//! the others are counted, never a reason to switch GI off.
use super::*;
use crate::fast_gi::{Instance,Mesh,Snapshot,voxels::gi_image};
use std::sync::Arc;
fn rgb(c:Vec4f)->Vec3f{vec3f(c.x,c.y,c.z)}
fn scaled(mut t:Mat4f,size:Vec3f)->Mat4f{for j in 0..3{t.v[j]*=size.x;t.v[4+j]*=size.y;t.v[8+j]*=size.z;}t}
// The loader has already multiplied emissiveFactor by emissiveStrength.
// Read only this draw layer's material; coverage is applied by the worker.
fn material(m:&LayerMaterial)->(Vec3f,f32){let e=m.surface.as_ref().map(|s|s.definition.emissive).unwrap_or([0.0;3]);(vec3f(e[0],e[1],e[2]),if m.surface.is_some(){1.0-m.metallic.clamp(0.0,1.0)}else{1.0})}
fn overlaps(i:&Instance,(lo,hi):(Vec3f,Vec3f))->bool{!(i.hi.x<lo.x||i.lo.x>hi.x||i.hi.y<lo.y||i.lo.y>hi.y||i.hi.z<lo.z||i.lo.z>hi.z)}
/// The cached GI copy of a render geometry (+ its albedo texture).
fn gi_mesh(cache:&mut crate::fast_gi::MeshCache,cx:&mut Cx,geometry:&Geometry,rev:u64,stride:usize,color_lane:i32,texture:Option<&Texture>)->Option<Arc<Mesh>>{
    let generation=cache.generation;
    let key=(geometry.geometry_id(),rev);let texture_id=texture.map(|t|t.texture_id());
    if let Some(e)=cache.geometry.get_mut(&key).and_then(|v|v.iter_mut().find(|e|e.0==texture_id)){e.2=generation;return Some(e.1.clone());}
    let image=texture.and_then(|t|match t.get_format(cx){
        TextureFormat::VecBGRAu8_32{width,height,data:Some(data),..}|TextureFormat::VecMipBGRAu8_32{width,height,data:Some(data),..}=>gi_image(*width,*height,data),_=>None
    });
    let (indices,vertices)=geometry.cpu_buffers(cx);
    let mesh=Arc::new(Mesh::new(vertices.as_f32()?.to_vec(),indices.as_u32()?.to_vec(),stride,color_lane,image)?);
    cache.geometry.entry(key).or_default().push((texture_id,mesh.clone(),generation));
    Some(mesh)
}

impl Renderer {
    fn gi_shape(&mut self,shape:Shape)->Option<Arc<Mesh>>{
        if let Some(m)=self.gi.meshes.shapes.get(&shape.index()){return Some(m.clone());}
        let (v,i)=shape_geometry_data(shape);
        let mesh=Arc::new(Mesh::new(v,i,12,-1,None)?);
        self.gi.meshes.shapes.insert(shape.index(),mesh.clone());
        Some(mesh)
    }
    /// The static scene around `center` for the GI voxel worker.
    pub(super) fn gi_snapshot(&mut self,cx:&mut Cx,world:&World,region:(Vec3f,Vec3f))->Snapshot{
        self.gi.meshes.generation+=1;
        let mut out=Vec::new();let mut omitted=0usize;
        let mut push=|i:Option<Instance>,out:&mut Vec<Instance>|if let Some(i)=i{if overlaps(&i,region){out.push(i)}else{omitted+=1}};
        if self.stage.shows_environment(){
            if let Some(terrain)=world.terrain.as_deref(){self.ensure_terrain_tiles(cx,terrain,world.terrain_materials.as_deref());}
            self.ensure_voxel_tiles(cx,world.voxel.as_deref());
            // Revisions restart per realm: the epoch keeps them apart.
            let epoch=self.gi.meshes.epoch.rotate_left(40);
            let terrain_rev=world.terrain.as_deref().map_or(0,|t|t.revision)^epoch;
            let tiles=self.terrain_tiles.iter().map(|t|(&t.geometry,terrain_rev)).chain(self.voxel_tiles.iter().map(|t|(&t.geometry,t.rev^epoch)));
            for (g,rev) in tiles {
                let m=gi_mesh(&mut self.gi.meshes,cx,g,rev,16,8,None);
                push(m.and_then(|m|Instance::new(m,Mat4f::identity(),vec3f(1.0,1.0,1.0),Vec3f::default(),1.0)),&mut out);
            }
        }
        for e in &world.entities {
            if e.kind!=BodyKind::Static || e.hidden || primitive_bucket(e)!=Some(PrimitiveBucket::Opaque){continue;}
            let size=vec3f(e.half.x*e.scale.x,e.half.y*e.scale.y,e.half.z*e.scale.z)*2.0;
            let m=self.gi_shape(e.shape);
            push(m.and_then(|m|Instance::new(m,scaled(Self::rigid_transform(e),size),rgb(e.color),rgb(e.color)*(e.glow*0.6),1.0)),&mut out);
        }
        for p in &world.parts {
            let Some(e)=entity_index_sorted(&world.entities,p.owner).map(|i|&world.entities[i])else{continue;};
            if e.kind!=BodyKind::Static||e.hidden||p.color.w<0.999{continue;}
            let size=vec3f(p.half.x*e.scale.x,p.half.y*e.scale.y,p.half.z*e.scale.z)*2.0;
            let m=self.gi_shape(p.shape);
            push(m.and_then(|m|Instance::new(m,scaled(Self::part_transform(e,p),size),rgb(p.color),rgb(p.color)*(p.glow*0.6),1.0)),&mut out);
        }
        // Movers, animated parts and skinned actors are not GI occluders:
        // as solid boxes they left ghosts; they still RECEIVE GI.
        let models:std::collections::HashMap<&str,usize>=self.static_models.iter().enumerate().map(|(i,(id,_))|(id.as_str(),i)).collect();
        for inst in &self.placed_models {
            if inst.dynamic||self.model_casts_shadow.get(&inst.model)==Some(&false){continue;}
            let Some(&mi)=models.get(inst.model.as_str())else{continue;};
            let m=&self.static_models[mi].1;
            if m.morph.is_some(){continue;}
            for (g,t,mat) in std::iter::once((&m.geometry,&m.texture,&m.material)).chain(m.extra_draws.iter().map(|(g,t,_,_,mat)|(g,t,mat))){
                if mat.surface.as_ref().is_some_and(|s|s.definition.alpha_mode==2){continue;}
                let (emission,diffuse)=material(mat);
                let m=gi_mesh(&mut self.gi.meshes,cx,g,0,7,5,Some(t));
                push(m.and_then(|m|Instance::new(m,inst.transform,rgb(inst.tint),emission,diffuse)),&mut out);
            }
        }
        self.gi.meshes.evict();
        if omitted>0&&omitted!=self.gi_snapshot_omitted&&std::env::var_os("MAKEPAD_GI_STATS").is_some(){log!("fast GI: {omitted} instance(s) outside the GI region around the camera");}
        self.gi_snapshot_omitted=omitted;self.gi.stats.omitted_instances=omitted;
        let triangles=out.iter().map(|i|i.triangles()).sum();
        Snapshot{instances:out,triangles}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_layers_keep_their_own_emission_through_snapshot_and_voxelization() {
        use crate::fast_gi::voxels::{VoxelLayout, VoxelWorld, BRICK};
        use crate::material_surface::MaterialSurface;
        use crate::model::{PbrMaterial, StaticDrawLayer};

        let layer = |x: f32, emissive| {
            let mut vertices = Vec::new();
            for (dx, dy) in [(0.0, 0.0), (0.2, 0.0), (0.2, 0.2), (0.0, 0.2)] {
                vertices.extend_from_slice(&[x + dx, 0.4 + dy, 0.5, 0.0, 0.0, f32::from_bits(0xffff_ffff), 0.0]);
            }
            StaticDrawLayer {
                vertices, indices: vec![0, 1, 2, 0, 2, 3], uvs: vec![[0.0; 2]; 4],
                texture_png: None, detail_png: None, detail_scale: [1.0; 2],
                pbr: PbrMaterial { metallic: 0.0, surface: Some(Arc::new(MaterialSurface { emissive, ..Default::default() })), ..Default::default() },
            }
        };
        let layers = vec![layer(0.4, [9.0, 4.5, 0.0]), layer(2.4, [0.0; 3])];
        let model = StaticModel {
            vertices: layers.iter().flat_map(|l| l.vertices.iter().copied()).collect(),
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            min: vec3f(0.4, 0.4, 0.5), max: vec3f(2.6, 0.6, 0.5),
            texture_uri: None, texture_png: None, parts: Vec::new(), ground_ao: None,
            draw_layers: layers, detail_png: None, detail_scale: [1.0; 2], prelit: false,
            anim_parts: Vec::new(), driven_parts: Vec::new(), sky: None,
            liquids: Vec::new(), liquid_ranges: Vec::new(), pbr: Default::default(),
        };
        // Only CPU geometry/texture handles are allocated; no GPU or window.
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let prepared = PreparedStaticPreview::prepare(model).unwrap();
        let uploaded = renderer.upload_static_preview(&mut cx, prepared);
        renderer.install_uploaded_static("layered-emitter", uploaded);
        renderer.set_models(vec![ModelInstance {
            model: "layered-emitter".into(), transform: Mat4f::identity(),
            tint: vec4(1.0, 1.0, 1.0, 1.0), color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
            dynamic: false, depth_order: 0.0, part_poses: Vec::new(), custom_material: None, lamps: Default::default(),
        }]);
        let snapshot = renderer.gi_snapshot(&mut cx, &World::default(), (Vec3f::default(), vec3f(8.0, 8.0, 8.0)));
        assert_eq!(snapshot.instances.len(), 2, "one GI instance per model draw layer");
        assert_eq!(snapshot.instances[0].emission, vec3f(9.0, 4.5, 0.0), "material emission is already scaled");
        assert_eq!(snapshot.instances[1].emission, Vec3f::default(), "the other layer has no emission");
        let layout = VoxelLayout { dims: [BRICK; 3], levels: 1, voxel: 1.0, home: [0.0; 3] };
        let mut voxels = VoxelWorld::new(layout);
        voxels.update(Some(Arc::new(snapshot)), &[[0, 0, 0]]);
        assert_ne!(voxels.emission[layout.texel(0, [0, 0, 0])], 0, "the emitting layer contributes light");
        assert_eq!(voxels.solid(0, [2, 0, 0]), Some(true), "the non-emitting layer is present");
        assert_eq!(voxels.emission[layout.texel(0, [2, 0, 0])], 0, "emission never spreads to the other layer");
    }
}
