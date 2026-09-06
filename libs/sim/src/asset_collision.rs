//! Immutable collision geometry cooked on an asset worker. Installation only
//! shares the finished Box3D acceleration structure with an entity.
use makepad_math::*;
use std::sync::Arc;
#[derive(Clone,Debug)]
pub struct PreparedAssetCollider {pub data:Arc<makepad_box3d::types::MeshData>,pub center:Vec3f,pub half:Vec3f}
impl PreparedAssetCollider {
    pub fn cook(positions:&[[f32;3]],indices:&[u32])->Result<Arc<Self>,String>{
        if positions.len()<3||positions.len()>100_000||indices.len()<3||indices.len()%3!=0||indices.len()>24_576{return Err("authored collision geometry budget".into());}
        let mut min=[f32::INFINITY;3];let mut max=[f32::NEG_INFINITY;3];
        for p in positions{for i in 0..3{if !p[i].is_finite()||p[i].abs()>1e6{return Err("authored collision coordinate".into());}min[i]=min[i].min(p[i]);max[i]=max[i].max(p[i]);}}
        if indices.iter().any(|i|*i as usize>=positions.len()){return Err("authored collision index".into());}
        let center=vec3f((min[0]+max[0])*0.5,(min[1]+max[1])*0.5,(min[2]+max[2])*0.5);
        let half=vec3f((max[0]-min[0])*0.5,(max[1]-min[1])*0.5,(max[2]-min[2])*0.5);
        let vertices=positions.iter().map(|p|makepad_box3d::math_functions::Vec3{x:p[0]-center.x,y:p[1]-center.y,z:p[2]-center.z}).collect::<Vec<_>>();
        let indices=indices.iter().map(|i|*i as i32).collect::<Vec<_>>();
        let definition=makepad_box3d::types::MeshDef{vertices:&vertices,indices:&indices,material_indices:&[],weld_tolerance:1e-6,weld_vertices:true,use_median_split:true,identify_edges:true};
        let mut rejected=Vec::new();let data=makepad_box3d::mesh::create_mesh(&definition,Some(&mut rejected)).ok_or("authored collision cooking failed")?;
        if !rejected.is_empty(){return Err("degenerate authored collision triangles".into());}
        Ok(Arc::new(Self{data,center,half}))
    }
    /// Placement admits ordinary positive TRS. Sheared or reflected instance
    /// transforms require baking into the worker geometry first.
    pub fn placement(&self,m:&Mat4f)->Result<(Vec3f,Vec3f,Quat,Vec3f),String>{
        let a=&m.v;if a.iter().any(|v|!v.is_finite())||a[3].abs()>1e-6||a[7].abs()>1e-6||a[11].abs()>1e-6||(a[15]-1.).abs()>1e-6{return Err("authored collision placement matrix".into());}
        let mut r=[[0.;3];3];let mut scale=[0.;3];for c in 0..3{scale[c]=(a[c*4]*a[c*4]+a[c*4+1]*a[c*4+1]+a[c*4+2]*a[c*4+2]).sqrt();if scale[c]<1e-6{return Err("singular authored collision scale".into());}for row in 0..3{r[row][c]=a[c*4+row]/scale[c];}}
        for i in 0..3{for j in i+1..3{if (0..3).map(|k|r[k][i]*r[k][j]).sum::<f32>().abs()>1e-4{return Err("sheared collision instance must be baked".into());}}}
        let det=r[0][0]*(r[1][1]*r[2][2]-r[1][2]*r[2][1])-r[0][1]*(r[1][0]*r[2][2]-r[1][2]*r[2][0])+r[0][2]*(r[1][0]*r[2][1]-r[1][1]*r[2][0]);if det<0.99{return Err("reflected collision instance must be baked".into());}
        let q=quat(r);let p=self.center;let position=vec3f(a[0]*p.x+a[4]*p.y+a[8]*p.z+a[12],a[1]*p.x+a[5]*p.y+a[9]*p.z+a[13],a[2]*p.x+a[6]*p.y+a[10]*p.z+a[14]);
        let half=vec3f(self.half.x*scale[0],self.half.y*scale[1],self.half.z*scale[2]);let scale=vec3f(scale[0],scale[1],scale[2]);Ok((position,half,q,scale))
    }
}
fn quat(r:[[f32;3];3])->Quat{
    let trace=r[0][0]+r[1][1]+r[2][2];let q=if trace>0.{let s=(trace+1.).sqrt()*2.;Quat{x:(r[2][1]-r[1][2])/s,y:(r[0][2]-r[2][0])/s,z:(r[1][0]-r[0][1])/s,w:s*0.25}}
    else{let i=if r[0][0]>r[1][1]&&r[0][0]>r[2][2]{0}else if r[1][1]>r[2][2]{1}else{2};let j=(i+1)%3;let k=(i+2)%3;let s=(1.+r[i][i]-r[j][j]-r[k][k]).sqrt()*2.;let mut v=[0.;4];v[i]=s*0.25;v[j]=(r[j][i]+r[i][j])/s;v[k]=(r[k][i]+r[i][k])/s;v[3]=(r[k][j]-r[j][k])/s;Quat{x:v[0],y:v[1],z:v[2],w:v[3]}};
    let length=(q.x*q.x+q.y*q.y+q.z*q.z+q.w*q.w).sqrt();Quat{x:q.x/length,y:q.y/length,z:q.z/length,w:q.w/length}
}

/// Share cooked collision into a world after an atomic placement preflight.
pub fn spawn_authored_asset_colliders(world:&mut crate::GameWorld,parts:&[Arc<PreparedAssetCollider>],matrix:&Mat4f,tag:&str)->Result<usize,String>{
    if parts.len()>64||tag.len()>1024{return Err("authored collider placement budget".into());}
    let placements=parts.iter().map(|p|p.placement(matrix)).collect::<Result<Vec<_>,_>>()?;
    if world.next_id.checked_add(parts.len()as u64).is_none(){return Err("collider entity IDs exhausted".into());}
    for (prepared,(position,half,rotation,scale)) in parts.iter().zip(placements){
        world.next_id+=1;let id=world.next_id;
        world.push_entity(crate::Entity{id,kind:crate::BodyKind::Static,pos:position,half,orient:rotation,collide:true,hidden:true,bake_skip:true,tag:tag.into(),
            prepared_collider:Some(prepared.clone()),collider_scale:scale,..Default::default()});
    }
    if !parts.is_empty(){world.mark_render_dirty();}Ok(parts.len())
}
