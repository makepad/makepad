//! Render snapshots; static architecture uses triangles, not collider boxes.
use super::*;
use crate::fast_gi::{Instance,Mesh,Mover,MAX_TRIANGLES,MAX_MOVERS};
use std::{collections::HashMap,sync::Arc};
fn rgb(c:Vec4f)->Vec3f{vec3f(c.x,c.y,c.z)}
fn scaled(mut t:Mat4f,size:Vec3f)->Mat4f{for j in 0..3{t.v[j]*=size.x;t.v[4+j]*=size.y;t.v[8+j]*=size.z;}t}
fn material(m:&LayerMaterial)->(Vec3f,f32){let e=m.surface.as_ref().map(|s|s.definition.emissive).unwrap_or([0.0;3]);(vec3f(e[0],e[1],e[2]),if m.surface.is_some(){1.0-m.metallic.clamp(0.0,1.0)}else{1.0})}
impl Renderer {
    pub(super) fn gi_snapshot(&mut self,cx:&mut Cx,world:&GameWorld)->Result<Vec<Instance>,String>{
        let mut out=Vec::new();let mut cache=HashMap::<GeometryId,Vec<(Option<TextureId>,Arc<Mesh>)>>::new();let mut total=0usize;let mut source_bytes=0usize;
        let mut add=|cx:&mut Cx,geometry:&Geometry,stride:usize,color_lane:i32,texture:Option<&Texture>,transform:Mat4f,tint:Vec3f,emission:Vec3f,diffuse:f32,exact_box:bool|->Result<(),String>{
            let image_bytes=texture.map(|t|match t.get_format(cx){TextureFormat::VecBGRAu8_32{width,height,..}|TextureFormat::VecMipBGRAu8_32{width,height,..}=>width.saturating_mul(*height).saturating_mul(4),_=>0}).unwrap_or(0);
            let (indices,vertices)=geometry.cpu_buffers(cx);
            let indices=indices.as_u32().ok_or("GI requires u32 render indices")?;
            total=total.saturating_add(indices.len()/3);
            if total>MAX_TRIANGLES{return Err(format!("GI triangle cap exceeded ({total}>{MAX_TRIANGLES}); keeping ordinary lighting"));}
            if out.len()>=4096{return Err("GI instance cap exceeded (4096)".into());}
            let key=geometry.geometry_id();let texture_id=texture.map(|t|t.texture_id());
            let mesh=if let Some((_,mesh))=cache.get(&key).and_then(|v|v.iter().find(|(id,_)|*id==texture_id)){mesh.clone()}else{
                let vertices=vertices.as_f32().ok_or("GI requires float render positions")?;
                source_bytes=source_bytes.saturating_add(vertices.len().saturating_add(indices.len()).saturating_mul(4)).saturating_add(image_bytes);
                if source_bytes>64*1024*1024{return Err("GI source snapshot exceeds 64 MiB budget".into());}
                let vertices=vertices.to_vec();let indices=indices.to_vec();
                let image=texture.and_then(|t|match t.get_format(cx){
                    TextureFormat::VecBGRAu8_32{width,height,data:Some(data),..}|TextureFormat::VecMipBGRAu8_32{width,height,data:Some(data),..}
                        if *width>0&&*height>0&&width.saturating_mul(*height)<=4_194_304=>Some((*width,*height,data[..width.saturating_mul(*height).min(data.len())].to_vec())),_=>None
                });
                let mesh=Arc::new(Mesh{vertices,indices,stride,color_lane,image});cache.entry(key).or_default().push((texture_id,mesh.clone()));mesh
            };
            out.push(Instance{mesh,transform,tint,emission,diffuse,exact_box});Ok(())
        };
        let mut primitive=HashMap::<usize,Geometry>::new();
        for e in &world.entities {
            if e.kind!=BodyKind::Static || e.hidden || primitive_bucket(e)!=Some(PrimitiveBucket::Opaque){continue;}
            let geometry=primitive.entry(e.shape.index()).or_insert_with(||{let (v,i)=shape_geometry_data(e.shape);let g=Geometry::new(cx);g.update(cx,i,v);g});
            let size=vec3f(e.half.x*e.scale.x,e.half.y*e.scale.y,e.half.z*e.scale.z)*2.0;
            add(cx,geometry,12,-1,None,scaled(Self::rigid_transform(e),size),rgb(e.color),rgb(e.color)*(e.glow*0.6),1.0,e.shape==Shape::Box)?;
        }
        for p in &world.parts {
            let Some(e)=entity_index_sorted(&world.entities,p.owner).map(|i|&world.entities[i])else{continue;};
            if e.kind!=BodyKind::Static||e.hidden||p.color.w<0.999{continue;}
            let geometry=primitive.entry(p.shape.index()).or_insert_with(||{let(v,i)=shape_geometry_data(p.shape);let g=Geometry::new(cx);g.update(cx,i,v);g});
            let size=vec3f(p.half.x*e.scale.x,p.half.y*e.scale.y,p.half.z*e.scale.z)*2.0;
            add(cx,geometry,12,-1,None,scaled(Self::part_transform(e,p),size),rgb(p.color),rgb(p.color)*(p.glow*0.6),1.0,p.shape==Shape::Box)?;
        }
        for inst in self.placed_models.iter().filter(|i|!i.dynamic) {
            if self.model_casts_shadow.get(&inst.model)==Some(&false){continue;}
            let Some((_,m))=self.static_models.iter().find(|(id,_)|id==&inst.model)else{continue;};
            if m.morph.is_some(){continue;}
            for (g,t,mat) in std::iter::once((&m.geometry,&m.texture,&m.material)).chain(m.extra_draws.iter().map(|(g,t,_,_,mat)|(g,t,mat))){
                if mat.surface.as_ref().map_or(false,|s|s.definition.alpha_mode==2){continue;}
                let(emission,diffuse)=material(mat);add(cx,g,7,5,Some(t),inst.transform,rgb(inst.tint),emission,diffuse,false)?;
            }
        }
        if self.stage.shows_environment(){
            if let Some(terrain)=world.terrain.as_ref(){self.ensure_terrain_tiles(cx,terrain,world.terrain_materials.as_ref());}
            self.ensure_voxel_tiles(cx,world.voxel.as_deref());
            for tile in &self.terrain_tiles{add(cx,&tile.geometry,16,8,None,Mat4f::identity(),vec3f(1.0,1.0,1.0),Vec3f::default(),1.0,false)?;}
            for tile in &self.voxel_tiles{add(cx,&tile.geometry,16,8,None,Mat4f::identity(),vec3f(1.0,1.0,1.0),Vec3f::default(),1.0,false)?;}
        }
        Ok(out)
    }
    pub(super) fn gi_movers(&self,world:&GameWorld,center:Vec3f,skins:Option<&[SkinnedDraw]>)->Vec<Mover>{
        let mut out=Vec::new();let radius=self.gi.config().ray_distance+self.gi.config().spacing*16.0;
        let mut add=|transform:Mat4f,min:Vec3f,max:Vec3f,color:Vec3f,emission:Vec3f,exact_box:bool|{
            // One sentinel is enough to signal overflow; never allocate a
            // proxy vector proportional to an unbounded actor population.
            if out.len()>MAX_MOVERS{return;}
            let(lo,hi)=crate::lightmap::world_bounds(&transform,(min,max));
            if ((lo+hi)*0.5-center).length()<=radius+(hi-lo).length()*0.5 {out.push(Mover{transform,min,max,color,emission,exact_box});}
        };
        for e in &world.entities {
            if e.kind==BodyKind::Static||e.hidden||e.sensor||primitive_bucket(e)!=Some(PrimitiveBucket::Opaque){continue;}
            let size=vec3f(e.half.x*e.scale.x,e.half.y*e.scale.y,e.half.z*e.scale.z)*2.0;
            add(scaled(Self::rigid_transform(e),size),vec3f(-0.5,-0.5,-0.5),vec3f(0.5,0.5,0.5),rgb(e.color),rgb(e.color)*(e.glow*0.6),e.shape==Shape::Box);
        }
        for p in &world.parts {
            let Some(e)=entity_index_sorted(&world.entities,p.owner).map(|i|&world.entities[i])else{continue;};
            if e.kind==BodyKind::Static||e.hidden||p.color.w<0.999{continue;}
            let size=vec3f(p.half.x*e.scale.x,p.half.y*e.scale.y,p.half.z*e.scale.z)*2.0;
            add(scaled(Self::part_transform(e,p),size),vec3f(-0.5,-0.5,-0.5),vec3f(0.5,0.5,0.5),rgb(p.color),rgb(p.color)*(p.glow*0.6),p.shape==Shape::Box);
        }
        for (i,inst) in self.placed_models.iter().enumerate() {
            let Some((_,m))=self.static_models.iter().find(|(id,_)|id==&inst.model)else{continue;};
            if self.model_casts_shadow.get(&inst.model)==Some(&false){continue;}
            if inst.dynamic||m.morph.is_some(){add(inst.transform,m.min,m.max,rgb(inst.tint)*0.6,Vec3f::default(),false);}
            for part in &m.anim_parts {
                let t=Mat4f::mul(&inst.transform,&self.model_anim_state.transform(&ModelTarget::Instance(i),&inst.model,&part.def));
                add(t,part.def.min,part.def.max,rgb(inst.tint)*0.6,Vec3f::default(),false);
            }
            for part in &m.driven_parts {
                let local=inst.part_poses.iter().find(|p|p.connection==part.def.connection).map(|p|p.transform).unwrap_or_else(||part.def.rest_transform());
                add(Mat4f::mul(&inst.transform,&local),part.def.min,part.def.max,rgb(inst.tint)*0.6,Vec3f::default(),false);
            }
        }
        if let Some(skins)=skins{for skin in skins{let(min,max)=skin.bounds.unwrap_or((vec3f(-0.35,0.0,-0.35),vec3f(0.35,1.8,0.35)));add(skin.transform,min,max,vec3f(0.5,0.5,0.5),Vec3f::default(),false);}}
        out
    }
}
