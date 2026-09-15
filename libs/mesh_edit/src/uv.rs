use crate::{context::invalid,geometry::*,mesh::check_uv,modifiers::select_faces,*};
use std::collections::{BTreeMap,BTreeSet,VecDeque};

impl Mesh {
    pub fn pin_uv(&mut self,corners:&[CornerId],pinned:bool,ctx:&mut Context<'_>)->Result<ChangeSet>{
        ctx.limit("UV pin selection",corners.len()as u64,ctx.limits.max_corners as u64)?;
        let set=corners.iter().copied().collect::<BTreeSet<_>>();if set.len()!=corners.len(){return Err(invalid("duplicate UV pin selection"));}
        let valid=self.corners.iter().map(|c|c.id).collect::<BTreeSet<_>>();for &id in &set{ctx.checkpoint(1)?;if !valid.contains(&id){return Err(MeshError::UnknownElement(ElementId::Corner(id)));}}
        Ok(self.edit(ctx,corners.len().saturating_mul(128),|m,ctx|{for &c in &set{ctx.checkpoint(1)?;if pinned{m.uv_pins.insert(c);}else{m.uv_pins.remove(&c);}}Ok(((),Vec::new()))})?.1)
    }

    /// Components join only across unmarked edges whose two endpoint UVs match
    /// exactly. Material boundaries may share an island when UVs are continuous.
    pub fn uv_islands(&self,faces:&[FaceId],ctx:&mut Context<'_>)->Result<Vec<UvIsland>>{
        let selected=select_faces(self,faces,ctx)?;let adj=self.adjacency(ctx)?;let mut neighbors=BTreeMap::<FaceId,Vec<FaceId>>::new();
        for (&edge,uses) in &adj.edges{ctx.checkpoint(1)?;if uses.len()!=2||self.edge_data.get(&edge).is_some_and(|d|d.attributes.seam)||!uses.iter().all(|u|selected.contains(&u.face)){continue;}
            let a=self.face_corners(uses[0].face)?;let b=self.face_corners(uses[1].face)?;
            if [edge.0,edge.1].iter().all(|v|a.iter().find(|c|c.vertex==*v).unwrap().uv==b.iter().find(|c|c.vertex==*v).unwrap().uv){neighbors.entry(uses[0].face).or_default().push(uses[1].face);neighbors.entry(uses[1].face).or_default().push(uses[0].face);}
        }
        let components=components(selected,&neighbors,ctx)?;components.into_iter().map(|faces|{
            let mut island=UvIsland{faces:faces.clone(),corners:Vec::new(),bounds:[[f64::INFINITY;2],[f64::NEG_INFINITY;2]],area:0.,pinned:0};
            for face in faces{let cs=self.face_corners(face)?;let mut signed=0.;for i in 0..cs.len(){ctx.checkpoint(1)?;let c=&cs[i];island.corners.push(c.id);island.pinned+=self.uv_pins.contains(&c.id)as usize;
                for d in 0..2{island.bounds[0][d]=island.bounds[0][d].min(c.uv[d]);island.bounds[1][d]=island.bounds[1][d].max(c.uv[d]);}
                let b=&cs[(i+1)%cs.len()];signed+=c.uv[0]*b.uv[1]-c.uv[1]*b.uv[0];}island.area+=signed.abs()*0.5;
            }Ok(island)
        }).collect()
    }

    pub fn project_uv_box(&mut self,faces:&[FaceId],scale:[f64;2],offset:[f64;2],ctx:&mut Context<'_>)->Result<ChangeSet>{
        check_uv(scale)?;check_uv(offset)?;let selected=select_faces(self,faces,ctx)?;
        let mut values=BTreeMap::new();for &face in &selected{let n=face_geometry(self,face,ctx)?.normal;let axis=(0..3).max_by(|&a,&b|n[a].abs().total_cmp(&n[b].abs())).unwrap();
            for c in self.face_corners(face)?{ctx.checkpoint(1)?;if self.uv_pins.contains(&c.id){continue;}let p=self.vertex(c.vertex).unwrap().position;let q=[p[(axis+1)%3]*n[axis].signum(),p[(axis+2)%3]];
                let uv=std::array::from_fn(|d|q[d]*scale[d]+offset[d]);check_uv(uv)?;values.insert(c.id,uv);}
        }install_uv(self,values,ctx)
    }

    /// Axis uses right-handed YZ/ZX/XY angular coordinates. Faces crossing the
    /// angular seam lift their low U values by one; no face interpolates a wrap.
    pub fn project_uv_cylindrical(&mut self,faces:&[FaceId],axis:usize,scale:[f64;2],offset:[f64;2],ctx:&mut Context<'_>)->Result<ChangeSet>{
        if axis>2{return Err(invalid("cylindrical axis must be 0..2"));}check_uv(scale)?;check_uv(offset)?;let selected=select_faces(self,faces,ctx)?;let mut values=BTreeMap::new();
        for &face in &selected{let cs=self.face_corners(face)?;let mut raw=Vec::new();for c in cs{ctx.checkpoint(1)?;let p=self.vertex(c.vertex).unwrap().position;
                if p[(axis+1)%3]==0.&&p[(axis+2)%3]==0.{return Err(invalid("cylindrical projection is undefined on the axis"));}
                raw.push([p[(axis+2)%3].atan2(p[(axis+1)%3])/std::f64::consts::TAU+0.5,p[axis]]);
            }
            let low=raw.iter().map(|p|p[0]).fold(f64::INFINITY,f64::min);let high=raw.iter().map(|p|p[0]).fold(f64::NEG_INFINITY,f64::max);
            for (c,mut uv) in cs.iter().zip(raw){if self.uv_pins.contains(&c.id){continue;}if high-low>0.5&&uv[0]<0.5{uv[0]+=1.;}
                for d in 0..2{uv[d]=uv[d]*scale[d]+offset[d];}check_uv(uv)?;values.insert(c.id,uv);}
        }install_uv(self,values,ctx)
    }

    /// Deterministic square-grid packing using one common scale for every
    /// island (preserving relative texel density). Padding is in output UV units.
    /// Pinned islands are refused because a pack necessarily changes placement.
    pub fn pack_uv(&mut self,faces:&[FaceId],padding:f64,ctx:&mut Context<'_>)->Result<ChangeSet>{
        if !padding.is_finite()||padding<0.||padding>=0.5{return Err(invalid("UV padding must be in [0,0.5)"));}
        let islands=self.uv_islands(faces,ctx)?;if islands.is_empty(){return Err(invalid("empty UV pack selection"));}
        if islands.iter().any(|i|i.pinned>0){return Err(invalid("unpin islands before packing"));}
        let columns=(islands.len()as f64).sqrt().ceil()as usize;let rows=islands.len().div_ceil(columns);let cell=[1./columns as f64,1./rows as f64];
        if cell.iter().any(|s|*s<=2.*padding){return Err(invalid("UV padding exceeds available island cell"));}
        let mut maximum=[0f64;2];for island in &islands{for d in 0..2{let extent=island.bounds[1][d]-island.bounds[0][d];if extent<=0.{return Err(invalid("cannot pack a collapsed UV island"));}maximum[d]=maximum[d].max(extent);}}
        let scale=((cell[0]-2.*padding)/maximum[0]).min((cell[1]-2.*padding)/maximum[1]);let mut transforms=BTreeMap::new();
        for (i,island) in islands.iter().enumerate(){let extent=std::array::from_fn::<_,2,_>(|d|island.bounds[1][d]-island.bounds[0][d]);let origin=[(i%columns)as f64*cell[0],(i/columns)as f64*cell[1]];
            let offset=std::array::from_fn(|d|origin[d]+(cell[d]-extent[d]*scale)*0.5-island.bounds[0][d]*scale);for &corner in &island.corners{transforms.insert(corner,offset);}
        }
        let values=self.corners.iter().filter_map(|c|transforms.get(&c.id).map(|offset:&[f64;2]|(c.id,std::array::from_fn(|d|c.uv[d]*scale+offset[d])))).collect();install_uv(self,values,ctx)
    }

    /// Uniform harmonic UV relaxation. Continuous corner groups move together;
    /// pins and island boundaries stay fixed. Seam sides never average together.
    pub fn relax_uv(&mut self,faces:&[FaceId],iterations:u32,factor:f64,ctx:&mut Context<'_>)->Result<ChangeSet>{
        if !(1..=128).contains(&iterations)||!factor.is_finite()||!(0.0..=1.0).contains(&factor){return Err(invalid("UV relax requires 1..128 iterations and factor in [0,1]"));}
        let islands=self.uv_islands(faces,ctx)?;let mut values=BTreeMap::new();
        for island in islands{let mut groups=BTreeMap::<(VertexId,u64,u64),usize>::new();let mut map=BTreeMap::new();let mut uv=Vec::new();let mut pinned=BTreeSet::new();let mut edges=BTreeMap::<(usize,usize),usize>::new();
            for face in &island.faces{let cs=self.face_corners(*face)?;for c in cs{ctx.checkpoint(1)?;let key=(c.vertex,canonical_bits(c.uv[0]),canonical_bits(c.uv[1]));let n=groups.len();let index=*groups.entry(key).or_insert_with(||{uv.push(c.uv);n});map.insert(c.id,index);if self.uv_pins.contains(&c.id){pinned.insert(index);}}
                for i in 0..cs.len(){let a=map[&cs[i].id];let b=map[&cs[(i+1)%cs.len()].id];*edges.entry((a.min(b),a.max(b))).or_default()+=1;}
            }
            let mut neighbors=vec![BTreeSet::new();uv.len()];for ((a,b),count) in edges{neighbors[a].insert(b);neighbors[b].insert(a);if count==1{pinned.insert(a);pinned.insert(b);}}
            let mut next=uv.clone();for _ in 0..iterations{for i in 0..uv.len(){ctx.checkpoint(1)?;if pinned.contains(&i)||neighbors[i].is_empty(){continue;}let mut avg=[0.;2];for &j in &neighbors[i]{ctx.checkpoint(1)?;for d in 0..2{avg[d]+=uv[j][d]/neighbors[i].len()as f64;}}
                    for d in 0..2{next[i][d]=uv[i][d]*(1.-factor)+avg[d]*factor;}}
                std::mem::swap(&mut uv,&mut next);
            }
            for (corner,index) in map{values.insert(corner,uv[index]);}
        }install_uv(self,values,ctx)
    }
}
fn canonical_bits(x:f64)->u64{if x==0.{0}else{x.to_bits()}}
pub(crate) fn install_uv(mesh:&mut Mesh,values:BTreeMap<CornerId,[f64;2]>,ctx:&mut Context<'_>)->Result<ChangeSet>{
    for &uv in values.values(){ctx.checkpoint(1)?;check_uv(uv)?;}
    Ok(mesh.edit(ctx,values.len().saturating_mul(128),|m,ctx|{for c in &mut m.corners{ctx.checkpoint(1)?;if let Some(&uv)=values.get(&c.id){c.uv=uv;}}Ok(((),Vec::new()))})?.1)
}
pub(crate) fn components(mut selected:BTreeSet<FaceId>,neighbors:&BTreeMap<FaceId,Vec<FaceId>>,ctx:&mut Context<'_>)->Result<Vec<Vec<FaceId>>>{
    let mut result=Vec::new();while let Some(&seed)=selected.first(){selected.remove(&seed);let mut queue=VecDeque::from([seed]);let mut component=Vec::new();
        while let Some(face)=queue.pop_front(){ctx.checkpoint(1)?;component.push(face);for &other in neighbors.get(&face).into_iter().flatten(){if selected.remove(&other){queue.push_back(other);}}}
        component.sort();result.push(component);
    }Ok(result)
}
