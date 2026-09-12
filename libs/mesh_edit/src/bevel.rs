//! Constant-distance bevel for isolated edges with trivalent endpoints. The
//! deliberately narrow neighborhood prevents ambiguous multi-edge miters.
use crate::{context::invalid,geometry::*,topology::{surface,blended_vertex,replace_corners},*};
use std::collections::{BTreeMap,BTreeSet};

impl Mesh {
    /// Bevels disjoint closed-manifold edge neighborhoods. Each endpoint must
    /// have valence three; affected faces must be planar and convex. Width is
    /// perpendicular distance on each incident face and must consume less than
    /// half each flank edge. Shared-face selections and reflex edges are refused.
    pub fn bevel_edges(&mut self,edges:&[EdgeKey],width:f64,ctx:&mut Context<'_>)->Result<ChangeSet>{
        if !width.is_finite()||width<=0.{return Err(invalid("bevel width must be positive"));}
        ctx.limit("bevel edges",edges.len()as u64,ctx.limits.max_edges as u64)?;
        let selected=edges.iter().copied().collect::<BTreeSet<_>>();if selected.len()!=edges.len()||selected.is_empty(){return Err(invalid("bevel selection must be nonempty and unique"));}
        let adj=surface(self,ctx)?;let mut affected=BTreeSet::new();let mut removed=BTreeSet::new();let mut flanks=BTreeMap::<EdgeKey,(VertexId,f64)>::new();
        for &edge in &selected{ctx.checkpoint(1)?;let uses=adj.radial(edge);if uses.len()!=2{return Err(invalid("bevel requires two-sided edges"));}
            let mut local=BTreeSet::new();for v in [edge.0,edge.1]{if adj.vertex_edges[&v].len()!=3||adj.vertex_edges[&v].iter().any(|e|adj.radial(*e).len()!=2){return Err(invalid("bevel endpoints must be closed trivalent fans"));}
                if !removed.insert(v){return Err(invalid("bevel edges share endpoints"));}local.extend(adj.vertex_faces[&v].iter().copied());
                let p=self.vertex(v).unwrap().position;let other=if v==edge.0{edge.1}else{edge.0};let axis=sub(self.vertex(other).unwrap().position,p);let axis=mul(axis,1./length(axis));
                for &flank in &adj.vertex_edges[&v]{if flank==edge{continue;}let target=if flank.0==v{flank.1}else{flank.0};let delta=sub(self.vertex(target).unwrap().position,p);
                    let perpendicular=length(sub(delta,mul(axis,dot(axis,delta))));let t=width/perpendicular;
                    if !t.is_finite()||t<=0.||t>=0.5{return Err(invalid("bevel width exceeds half a flank edge or edge is collinear"));}
                    if flanks.insert(flank,(v,t)).is_some(){return Err(invalid("bevel neighborhoods share a flank"));}
                }
            }
            for face in local {if !affected.insert(face){return Err(invalid("bevel neighborhoods must not share faces"));}
                let g=face_geometry(self,face,ctx)?;if !g.planar{return Err(invalid("bevel needs planar faces"));}
                let mut sign=0.;for i in 0..g.points.len(){let a=orient(g.points[i],g.points[(i+1)%g.points.len()],g.points[(i+2)%g.points.len()]);if a!=0.{if sign!=0.&&sign*a<0.{return Err(invalid("bevel needs convex faces"));}sign=a;}}
            }
            let left=face_geometry(self,uses[0].face,ctx)?.normal;let right=face_geometry(self,uses[1].face,ctx)?.normal;
            let dir=sub(self.vertex(uses[0].to).unwrap().position,self.vertex(uses[0].from).unwrap().position);
            if dot(cross(left,right),dir)<=length(dir)*1e-10{return Err(invalid("bevel supports convex noncoplanar edges"));}
        }
        ctx.counts(self.vertices.len()+flanks.len(),self.faces.len()+edges.len(),self.corners.len()+8*edges.len(),self.corners.len()+8*edges.len())?;
        Ok(self.edit(ctx,flanks.len().saturating_mul(8192)+affected.len().saturating_mul(2048),|m,ctx|{
            let source=m.clone();let mut cuts=BTreeMap::new();let mut remaps=Vec::new();let mut vertex_targets=BTreeMap::<VertexId,Vec<ElementId>>::new();
            for (&flank,&(from,t)) in &flanks {ctx.checkpoint(1)?;let to=if flank.0==from{flank.1}else{flank.0};let(p,w)=blended_vertex(&source,from,to,t,ctx)?;let id=m.push_vertex(p,w)?;cuts.insert(flank,id);vertex_targets.entry(from).or_default().push(ElementId::Vertex(id));
                let new=EdgeKey::new(id,to);if let Some(data)=m.edge_data.remove(&flank){m.edge_data.insert(new,data);}remaps.push((ElementId::Edge(flank),vec![ElementId::Edge(new)]));
            }
            let mut replacements=BTreeMap::new();for &face in &affected{let cs=source.face_corners(face)?;let mut next=Vec::new();
                for i in 0..cs.len(){ctx.checkpoint(1)?;let c=&cs[i];if !removed.contains(&c.vertex){next.push(c.clone());continue;}
                    let prev=&cs[(i+cs.len()-1)%cs.len()];let after=&cs[(i+1)%cs.len()];let mut first=true;
                    for neighbor in [prev,after]{let flank=EdgeKey::new(c.vertex,neighbor.vertex);if let Some(&vertex)=cuts.get(&flank){let t=flanks[&flank].1;
                        let id=if first{c.id}else{CornerId(m.allocate_id()?)};first=false;next.push(Corner{id,vertex,uv:std::array::from_fn(|d|c.uv[d]*(1.-t)+neighbor.uv[d]*t),normal:None});
                    }}
                }for c in &mut next{c.normal=None;}replacements.insert(face,next);
            }
            for &edge in &selected{let uses=adj.radial(edge);let a=uses[0].from;let b=uses[0].to;
                let flank_for=|face:FaceId,v:VertexId,other:VertexId|->EdgeKey{let cs=source.face_corners(face).unwrap();let i=cs.iter().position(|c|c.vertex==v).unwrap();let neighbors=[cs[(i+cs.len()-1)%cs.len()].vertex,cs[(i+1)%cs.len()].vertex];EdgeKey::new(v,*neighbors.iter().find(|&&x|x!=other).unwrap())};
                let al=cuts[&flank_for(uses[0].face,a,b)];let bl=cuts[&flank_for(uses[0].face,b,a)];let ar=cuts[&flank_for(uses[1].face,a,b)];let br=cuts[&flank_for(uses[1].face,b,a)];
                let len=length(sub(source.vertex(a).unwrap().position,source.vertex(b).unwrap().position));
                m.push_face(&[(bl,[len,0.],None),(al,[0.,0.],None),(ar,[0.,width],None),(br,[len,width],None)],source.face(uses[0].face).unwrap().material)?;
                let rims=[EdgeKey::new(al,bl),EdgeKey::new(ar,br)];if let Some(data)=m.edge_data.remove(&edge){for rim in rims{m.edge_data.insert(rim,data);}}
                remaps.push((ElementId::Edge(edge),rims.into_iter().map(ElementId::Edge).collect()));
            }
            replace_corners(m,&replacements,ctx)?;m.vertices.retain(|v|!removed.contains(&v.id));m.prune_edge_attributes();
            remaps.extend(vertex_targets.into_iter().map(|(v,to)|(ElementId::Vertex(v),to)));surface(m,ctx)?;Ok(((),remaps))
        })?.1)
    }
}
