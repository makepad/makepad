//! Bounded mesh factories and explicit derived-geometry construction. Factories
//! return a new named object; existing sources are never destructively replaced.
use crate::{json::{self,Value},mesh::{Context,Mesh,Polygon,JointWeight},service::{array,fields,integer,need,text},Error,Limits,OperationResult,Result};
use std::collections::BTreeMap;
#[path="construction_fibers.rs"]
mod fibers;

#[derive(Clone,Debug,PartialEq)]
pub struct ConstructionOperation {object:String,kind:Construction}
#[derive(Clone,Debug,PartialEq)]
enum Construction {
    Fibers{source:String,count:u32,length:f64,width:f64,seed:u32,material:u32},
    Sweep{profile:Vec<[f64;2]>,path:Vec<[f64;3]>,caps:bool,material:u32},
    Lathe{profile:Vec<[f64;2]>,axis:usize,segments:u32,caps:bool,material:u32},
    Loft{profiles:Vec<Vec<[f64;3]>>,closed:bool,caps:bool,material:u32},
    QuadStrip{a:Vec<[f64;3]>,b:Vec<[f64;3]>,closed:bool,material:u32},
    Boolean{a:String,b:String,mode:BooleanMode},
    Voxel{source:String,resolution:u32},
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum BooleanMode {Union,Difference,Intersection}
impl ConstructionOperation {
    pub fn parse(v:&Value,limits:&Limits)->Result<Option<Self>>{
        let kind=match text(v,"op")?{
            "fiber_shell"=>{fields(v,&["op","object","source","count","length","width","seed","material"])?;
                let count=integer(need(v,"count")?)?;let length=crate::service::float(need(v,"length")?)?;let width=crate::service::float(need(v,"width")?)?;
                if !(1..=4096).contains(&count)||!(0.0001..=1.).contains(&length)||!(0.00001..=0.1).contains(&width)||width>length*0.5{return Err(Error::Invalid("fiber_shell count/length/width"));}
                Construction::Fibers{source:name(text(v,"source")?,limits)?,count,length,width,seed:integer(need(v,"seed")?)?,material:integer(need(v,"material")?)?}},
            "sweep"=>{fields(v,&["op","object","profile","path","caps","material"])?;Construction::Sweep{profile:vectors(need(v,"profile")?,3,128)?,path:vectors(need(v,"path")?,2,128)?,caps:flag(v,"caps")?,material:integer(need(v,"material")?)?}},
            "lathe"=>{fields(v,&["op","object","profile","axis","segments","caps","material"])?;let segments=integer(need(v,"segments")?)?;let axis=integer(need(v,"axis")?)? as usize;
                if !(3..=128).contains(&segments)||axis>2{return Err(Error::Invalid("lathe axis or segments"));}Construction::Lathe{profile:vectors(need(v,"profile")?,2,128)?,axis,segments,caps:flag(v,"caps")?,material:integer(need(v,"material")?)?}},
            "loft"=>{fields(v,&["op","object","profiles","closed","caps","material"])?;let p=need(v,"profiles")?.as_arr().filter(|a|(2..=128).contains(&a.len())).ok_or(Error::Invalid("loft profile count"))?;
                Construction::Loft{profiles:p.iter().map(|p|vectors(p,2,128)).collect::<Result<Vec<_>>>()?,closed:flag(v,"closed")?,caps:flag(v,"caps")?,material:integer(need(v,"material")?)?}},
            "quad_strip"=>{fields(v,&["op","object","a","b","closed","material"])?;Construction::QuadStrip{a:vectors(need(v,"a")?,2,4096)?,b:vectors(need(v,"b")?,2,4096)?,closed:flag(v,"closed")?,material:integer(need(v,"material")?)?}},
            "boolean"=>{fields(v,&["op","object","a","b","mode"])?;Construction::Boolean{a:name(text(v,"a")?,limits)?,b:name(text(v,"b")?,limits)?,mode:match text(v,"mode")?{"union"=>BooleanMode::Union,"difference"=>BooleanMode::Difference,"intersection"=>BooleanMode::Intersection,_=>return Err(Error::Invalid("boolean mode"))}}},
            "voxel_remesh"=>{fields(v,&["op","object","source","resolution"])?;let resolution=integer(need(v,"resolution")?)?;if !(8..=32).contains(&resolution)||!resolution.is_power_of_two(){return Err(Error::Invalid("voxel resolution must be 8,16,32"));}Construction::Voxel{source:name(text(v,"source")?,limits)?,resolution}},
            _=>return Ok(None),
        };Ok(Some(Self{object:name(text(v,"object")?,limits)?,kind}))
    }
    pub fn object_name(&self)->&str{&self.object}
    pub fn memory_bytes(&self)->usize{self.value().to_json().len().saturating_mul(4).saturating_add(256)}
    pub fn value(&self)->Value{
        let(op,mut args)=match &self.kind{
            Construction::Fibers{source,count,length,width,seed,material}=>("fiber_shell",vec![("source",json::s(source)),("count",Value::Int(*count as i64)),("length",Value::F64(*length)),("width",Value::F64(*width)),("seed",Value::Int(*seed as i64)),("material",Value::Int(*material as i64))]),
            Construction::Sweep{profile,path,caps,material}=>("sweep",vec![("profile",vector_values(profile)),("path",vector_values(path)),("caps",Value::Bool(*caps)),("material",Value::Int(*material as i64))]),
            Construction::Lathe{profile,axis,segments,caps,material}=>("lathe",vec![("profile",vector_values(profile)),("axis",Value::Int(*axis as i64)),("segments",Value::Int(*segments as i64)),("caps",Value::Bool(*caps)),("material",Value::Int(*material as i64))]),
            Construction::Loft{profiles,closed,caps,material}=>("loft",vec![("profiles",Value::Arr(profiles.iter().map(|p|vector_values(p)).collect())),("closed",Value::Bool(*closed)),("caps",Value::Bool(*caps)),("material",Value::Int(*material as i64))]),
            Construction::QuadStrip{a,b,closed,material}=>("quad_strip",vec![("a",vector_values(a)),("b",vector_values(b)),("closed",Value::Bool(*closed)),("material",Value::Int(*material as i64))]),
            Construction::Boolean{a,b,mode}=>("boolean",vec![("a",json::s(a)),("b",json::s(b)),("mode",json::s(match mode{BooleanMode::Union=>"union",BooleanMode::Difference=>"difference",BooleanMode::Intersection=>"intersection"}))]),
            Construction::Voxel{source,resolution}=>("voxel_remesh",vec![("source",json::s(source)),("resolution",Value::Int(*resolution as i64))]),
        };let mut values=vec![("op",json::s(op)),("object",json::s(&self.object))];values.append(&mut args);json::obj(values)
    }
    pub fn apply(&self,objects:&mut BTreeMap<String,Mesh>,limits:&Limits,ctx:&mut Context<'_>)->Result<OperationResult>{
        self.apply_in_scene(objects,&crate::SceneState::default(),limits,ctx)
    }
    pub(crate) fn apply_in_scene(&self,objects:&mut BTreeMap<String,Mesh>,scene:&crate::SceneState,limits:&Limits,ctx:&mut Context<'_>)->Result<OperationResult>{
        if objects.contains_key(&self.object){return Err(Error::DuplicateObject(self.object.clone()));}if objects.len()>=limits.max_objects{return Err(Error::Budget("objects"));}
        ctx.checkpoint(0)?;let mesh=match &self.kind{
            Construction::Fibers{source,count,length,width,seed,material}=>fibers::generate(objects.get(source).ok_or_else(||Error::MissingObject(source.clone()))?,*count,*length,*width,*seed,*material,ctx)?,
            Construction::Sweep{profile,path,caps,material}=>sweep(profile,path,*caps,*material,ctx)?,
            Construction::Lathe{profile,axis,segments,caps,material}=>lathe(profile,*axis,*segments,*caps,*material,ctx)?,
            Construction::Loft{profiles,closed,caps,material}=>loft(profiles,*closed,*caps,*material,ctx)?,
            Construction::QuadStrip{a,b,closed,material}=>quad_strip(a,b,*closed,*material,ctx)?,
            Construction::Boolean{a,b,mode}=>{
                let source_bytes=[a,b].iter().try_fold(0usize,|n,name|->Result<usize>{Ok(n.saturating_add(objects.get(*name).ok_or_else(||Error::MissingObject((*name).clone()))?.memory_bytes()))})?;
                if source_bytes.saturating_mul(3)>ctx.limits.max_bytes{return Err(Error::Budget("construction frame copies"));}
                let a=world_source(objects,scene,a,ctx)?;let b=world_source(objects,scene,b,ctx)?;
                boolean_mesh(&a,&b,*mode,ctx)?
            },
            Construction::Voxel{source,resolution}=>voxel_mesh(&world_source(objects,scene,source,ctx)?,*resolution,ctx)?,
        };
        ctx.checkpoint(0)?;let mut result=OperationResult{object:self.object.clone(),faces:mesh.faces().iter().map(|f|f.id).collect(),vertices:mesh.vertices().iter().map(|v|v.id).collect(),..Default::default()};
        result.metrics.insert("faces".into(),mesh.faces().len());result.metrics.insert("vertices".into(),mesh.vertices().len());objects.insert(self.object.clone(),mesh);Ok(result)
    }
}
// New Boolean/remesh objects have an identity frame, so their source meshes
// are baked to world space. Direct mesh operations retain source topology;
// linked instances and unapplied modifiers require explicit make_unique first.
fn world_source(objects:&BTreeMap<String,Mesh>,scene:&crate::SceneState,name:&str,ctx:&mut Context<'_>)->Result<Mesh>{
    let source=objects.get(name).ok_or_else(||Error::MissingObject(name.into()))?;
    if scene.nodes.get(name).is_some_and(|n|n.linked_to.is_some())||scene.modifiers.get(name).is_some_and(|m|m.iter().any(|m|m.enabled)){
        return Err(Error::Invalid("make construction sources unique and apply modifiers first"));
    }
    ctx.checkpoint(0)?;
    if source.memory_bytes().saturating_mul(3)>ctx.limits.max_bytes{return Err(Error::Budget("construction frame copy"));}
    let mut source=source.clone();let vertices=source.vertices().iter().map(|v|v.id).collect::<Vec<_>>();
    source.transform(&vertices,scene.world_matrix(name)?,ctx)?;Ok(source)
}
fn name(v:&str,l:&Limits)->Result<String>{if v.is_empty()||v.len()>l.max_name_bytes||v.chars().any(char::is_control){Err(Error::Invalid("construction object name"))}else{Ok(v.into())}}
fn flag(v:&Value,key:&str)->Result<bool>{need(v,key)?.as_bool().ok_or(Error::Invalid("expected boolean"))}
fn vectors<const N:usize>(v:&Value,min:usize,max:usize)->Result<Vec<[f64;N]>>{let a=v.as_arr().filter(|a|(min..=max).contains(&a.len())).ok_or(Error::Invalid("construction vector count"))?;a.iter().map(array).collect()}
fn vector_values<const N:usize>(v:&[[f64;N]])->Value{Value::Arr(v.iter().map(|p|Value::Arr(p.iter().copied().map(Value::F64).collect())).collect())}
fn add(a:[f64;3],b:[f64;3])->[f64;3]{std::array::from_fn(|d|a[d]+b[d])}
fn sub(a:[f64;3],b:[f64;3])->[f64;3]{std::array::from_fn(|d|a[d]-b[d])}
fn mul(a:[f64;3],s:f64)->[f64;3]{a.map(|v|v*s)}
fn dot(a:[f64;3],b:[f64;3])->f64{a.iter().zip(b).map(|(a,b)|a*b).sum()}
fn cross(a:[f64;3],b:[f64;3])->[f64;3]{[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]}
fn length(a:[f64;3])->f64{a[0].hypot(a[1]).hypot(a[2])}
fn unit(a:[f64;3])->Result<[f64;3]>{let n=length(a);if !n.is_finite()||n<=1e-12{Err(Error::Invalid("degenerate construction direction"))}else{Ok(mul(a,1./n))}}
fn admit(vertices:usize,faces:usize,corners:usize,ctx:&mut Context<'_>)->Result<()>{ctx.checkpoint((vertices+corners)as u64)?;if vertices>ctx.limits.max_vertices||faces>ctx.limits.max_faces||corners>ctx.limits.max_corners||vertices.saturating_mul(1024).saturating_add(corners.saturating_mul(1024))>ctx.limits.max_bytes{return Err(Error::Budget("construction geometry"));}Ok(())}
fn loft(profiles:&[Vec<[f64;3]>],closed:bool,caps:bool,material:u32,ctx:&mut Context<'_>)->Result<Mesh>{
    let n=profiles[0].len();if profiles.iter().any(|p|p.len()!=n)||(closed&&n<3)||(caps&&!closed){return Err(Error::Invalid("loft needs equal profile counts; caps need closed profiles"));}
    let strips=if closed{n}else{n-1};let faces=(profiles.len()-1)*strips+if caps{2}else{0};admit(profiles.len()*n,faces,4*faces+2*n,ctx)?;
    let positions=profiles.iter().flatten().copied().collect::<Vec<_>>();let mut polygons=Vec::new();
    for ring in 0..profiles.len()-1{for j in 0..strips{ctx.checkpoint(1)?;let k=(j+1)%n;let u=j as f64/strips as f64;let u1=(j+1)as f64/strips as f64;let v=ring as f64/(profiles.len()-1)as f64;let v1=(ring+1)as f64/(profiles.len()-1)as f64;
        polygons.push(Polygon{vertices:vec![(ring*n+j)as u32,(ring*n+k)as u32,((ring+1)*n+k)as u32,((ring+1)*n+j)as u32],uvs:vec![[u,v],[u1,v],[u1,v1],[u,v1]],material});}}
    if caps{polygons.push(cap_polygon(&positions,(0..n).rev().map(|i|i as u32).collect(),material)?);polygons.push(cap_polygon(&positions,((profiles.len()-1)*n..profiles.len()*n).map(|i|i as u32).collect(),material)?);}
    Ok(Mesh::from_polygons(&positions,&polygons,ctx)?)
}
fn cap_polygon(positions:&[[f64;3]],indices:Vec<u32>,material:u32)->Result<Polygon>{let mut normal=[0.;3];for i in 0..indices.len(){normal=add(normal,cross(positions[indices[i]as usize],positions[indices[(i+1)%indices.len()]as usize]));}let normal=unit(normal)?;let axis=(0..3).max_by(|&a,&b|normal[a].abs().total_cmp(&normal[b].abs())).unwrap();
    let uvs=indices.iter().map(|&i|[positions[i as usize][(axis+1)%3],positions[i as usize][(axis+2)%3]]).collect();Ok(Polygon{vertices:indices,uvs,material})}
fn quad_strip(a:&[[f64;3]],b:&[[f64;3]],closed:bool,material:u32,ctx:&mut Context<'_>)->Result<Mesh>{
    if a.len()!=b.len()||(closed&&a.len()<3){return Err(Error::Invalid("quad strip rails must have equal lengths"));}let profiles=vec![a.to_vec(),b.to_vec()];loft(&profiles,closed,false,material,ctx)
}
fn sweep(profile:&[[f64;2]],path:&[[f64;3]],caps:bool,material:u32,ctx:&mut Context<'_>)->Result<Mesh>{
    admit(profile.len()*path.len(),profile.len()*(path.len()-1)+2,4*profile.len()*path.len(),ctx)?;
    let mut rings=Vec::new();let mut previous_normal=None;let mut previous_tangent=None;
    for i in 0..path.len(){ctx.checkpoint(1)?;let tangent=unit(sub(path[(i+1).min(path.len()-1)],path[i.saturating_sub(1)]))?;
        if previous_tangent.is_some_and(|p|dot(p,tangent)< -0.999){return Err(Error::Invalid("sweep path reverses direction"));}
        let normal=if let Some(previous)=previous_normal{let projected=sub(previous,mul(tangent,dot(previous,tangent)));if length(projected)>1e-8{unit(projected)?}else{frame_normal(tangent)?}}else{frame_normal(tangent)?};
        let binormal=cross(tangent,normal);rings.push(profile.iter().map(|p|add(path[i],add(mul(normal,p[0]),mul(binormal,p[1])))).collect());previous_normal=Some(normal);previous_tangent=Some(tangent);
    }loft(&rings,true,caps,material,ctx)
}
fn frame_normal(tangent:[f64;3])->Result<[f64;3]>{let axis=(0..3).min_by(|&a,&b|tangent[a].abs().total_cmp(&tangent[b].abs())).unwrap();let mut up=[0.;3];up[axis]=1.;unit(cross(tangent,up))}
fn lathe(profile:&[[f64;2]],axis:usize,segments:u32,caps:bool,material:u32,ctx:&mut Context<'_>)->Result<Mesh>{
    let n=segments as usize;admit(n*profile.len(),n*(profile.len()-1)+2,4*n*profile.len(),ctx)?;let mut positions=Vec::new();let mut rings=Vec::new();
    for (i,&[radius,height]) in profile.iter().enumerate(){ctx.checkpoint(1)?;if radius<0.||(radius==0.&&i!=0&&i+1!=profile.len()){return Err(Error::Invalid("lathe radius must be positive except at endpoint poles"));}
        let count=if radius==0.{1}else{n};let mut ring=Vec::new();for j in 0..count{let theta=j as f64/n as f64*std::f64::consts::TAU;let mut p=[0.;3];p[axis]=height;p[(axis+1)%3]=radius*theta.cos();p[(axis+2)%3]=radius*theta.sin();ring.push(positions.len()as u32);positions.push(p);}rings.push(ring);
    }
    let mut polygons=Vec::new();for i in 0..rings.len()-1{let a=&rings[i];let b=&rings[i+1];if a.len()==1&&b.len()==1{return Err(Error::Invalid("lathe has no surface between adjacent poles"));}
        for j in 0..n{ctx.checkpoint(1)?;let k=(j+1)%n;let u=j as f64/n as f64;let u1=(j+1)as f64/n as f64;let v=i as f64/(rings.len()-1)as f64;let v1=(i+1)as f64/(rings.len()-1)as f64;
            let(indices,uvs)=if a.len()==1{(vec![a[0],b[k],b[j]],vec![[(u+u1)*0.5,v],[u1,v1],[u,v1]])}else if b.len()==1{(vec![a[j],a[k],b[0]],vec![[u,v],[u1,v],[(u+u1)*0.5,v1]])}else{(vec![a[j],a[k],b[k],b[j]],vec![[u,v],[u1,v],[u1,v1],[u,v1]])};polygons.push(Polygon{vertices:indices,uvs,material});
        }
    }
    if caps{if rings[0].len()>1{polygons.push(cap_polygon(&positions,rings[0].iter().rev().copied().collect(),material)?);}if rings.last().unwrap().len()>1{polygons.push(cap_polygon(&positions,rings.last().unwrap().clone(),material)?);}}
    Ok(Mesh::from_polygons(&positions,&polygons,ctx)?)
}

#[derive(Clone)]
struct SourceTriangle {points:[[f64;3];3],uv:[[f64;2];3],normals:[[f64;3];3],weights:[Vec<JointWeight>;3],material:u32}
fn source_triangles(mesh:&Mesh,ctx:&mut Context<'_>)->Result<Vec<SourceTriangle>>{
    let tri=mesh.triangulate(ctx)?;if tri.triangles.len()>2048{return Err(Error::Budget("derived construction input triangles"));}
    tri.triangles.iter().map(|t|{ctx.checkpoint(1)?;Ok(SourceTriangle{points:t.indices.map(|i|tri.vertices[i as usize].position),uv:t.indices.map(|i|tri.vertices[i as usize].uv),normals:t.indices.map(|i|tri.vertices[i as usize].normal),weights:t.indices.map(|i|tri.vertices[i as usize].weights.clone()),material:t.material})}).collect()
}
fn derived_source(mesh:&Mesh,ctx:&mut Context<'_>)->Result<()> {
    if !mesh.edge_attributes().is_empty()||!mesh.uv_pins().is_empty(){return Err(Error::Invalid("derived construction cannot preserve marked edges or UV pins"));}
    if !mesh.validate(ctx)?.is_closed_manifold{return Err(Error::Invalid("derived construction needs a closed oriented manifold"));}Ok(())
}
fn boolean_mesh(a:&Mesh,b:&Mesh,mode:BooleanMode,ctx:&mut Context<'_>)->Result<Mesh>{
    derived_source(a,ctx)?;derived_source(b,ctx)?;
    if a.vertices().iter().chain(b.vertices()).any(|v|!v.weights.is_empty()){return Err(Error::Invalid("boolean refuses ambiguous skin weights at new intersections"));}
    let mut sources=source_triangles(a,ctx)?;let other=source_triangles(b,ctx)?;let pairs=sources.len().saturating_mul(other.len());
    if pairs.saturating_mul(2048).saturating_add(a.memory_bytes()+b.memory_bytes())>ctx.limits.max_bytes{return Err(Error::Budget("boolean candidate pairs"));}
    ctx.checkpoint(pairs.saturating_mul(64)as u64)?;sources.extend(other);
    let convert=|m:&Mesh,ctx:&mut Context<'_>|->Result<makepad_csg_mesh::TriMesh>{let tri=m.triangulate(ctx)?;let indices=m.vertices().iter().enumerate().map(|(i,v)|(v.id,i as u32)).collect::<BTreeMap<_,_>>();
        Ok(makepad_csg_mesh::TriMesh{vertices:m.vertices().iter().map(|v|makepad_csg_math::dvec3(v.position[0],v.position[1],v.position[2])).collect(),triangles:tri.triangles.iter().map(|t|t.indices.map(|i|indices[&tri.vertices[i as usize].source_vertex])).collect()})};
    let ca=convert(a,ctx)?;let cb=convert(b,ctx)?;
    let output=makepad_csg_math::thread_pool::with_serial(||makepad_csg_boolean::mesh_boolean(&ca,&cb,match mode{BooleanMode::Union=>makepad_csg_boolean::BoolOp::Union,BooleanMode::Difference=>makepad_csg_boolean::BoolOp::Difference,BooleanMode::Intersection=>makepad_csg_boolean::BoolOp::Intersection}));
    ctx.checkpoint(0)?;if makepad_csg_math::thread_pool::cancelled(){return Err(crate::mesh::MeshError::Cancelled.into());}
    let positions=output.vertices.iter().map(|p|[p.x,p.y,p.z]).collect::<Vec<_>>();
    transfer_surface(&positions,&output.triangles,&sources,true,ctx)
}
fn voxel_mesh(source:&Mesh,resolution:u32,ctx:&mut Context<'_>)->Result<Mesh>{
    derived_source(source,ctx)?;let sources=source_triangles(source,ctx)?;let cells=(resolution as usize).pow(3);
    if cells.saturating_mul(2048).saturating_add(source.memory_bytes())>ctx.limits.max_bytes{return Err(Error::Budget("voxel working storage"));}ctx.checkpoint(cells.saturating_mul(128)as u64)?;
    let mut bounds=[[f64::INFINITY;3],[f64::NEG_INFINITY;3]];for v in source.vertices(){for d in 0..3{bounds[0][d]=bounds[0][d].min(v.position[d]);bounds[1][d]=bounds[1][d].max(v.position[d]);}}
    let center=std::array::from_fn::<_,3,_>(|d|(bounds[0][d]+bounds[1][d])*0.5);let span=(0..3).map(|d|bounds[1][d]-bounds[0][d]).fold(0f64,f64::max);
    if span<=0.||!span.is_finite(){return Err(Error::Invalid("voxel source bounds"));}let scale=1.9/span;
    let triangles=sources.iter().map(|t|t.points.map(|p|std::array::from_fn(|d|((p[d]-center[d])*scale)as f32))).collect::<Vec<_>>();
    let decoded=makepad_csg_math::thread_pool::with_serial(||{
        let tokens=makepad_remesh::encode(&triangles,resolution,&makepad_remesh::EncodeOptions{lambda_n:1.,lambda_d:1e-3,clamp_anchors:true,compute_flux:true,min_level:None});
        makepad_remesh::decode(resolution,&tokens.voxel_indices,&tokens.anchors,&tokens.flux,Some(&tokens.normals),makepad_remesh::TriangulationMode::Auto)
    });
    ctx.checkpoint(0)?;if makepad_csg_math::thread_pool::cancelled(){return Err(crate::mesh::MeshError::Cancelled.into());}
    if decoded.faces.is_empty(){return Err(Error::Invalid("voxel remesh produced an empty surface"));}
    let positions=decoded.vertices.iter().map(|p|std::array::from_fn(|d|p[d]as f64/scale+center[d])).collect::<Vec<_>>();
    transfer_surface(&positions,&decoded.faces,&sources,false,ctx)
}
fn transfer_surface(positions:&[[f64;3]],faces:&[[u32;3]],sources:&[SourceTriangle],exact:bool,ctx:&mut Context<'_>)->Result<Mesh>{
    admit(positions.len(),faces.len(),faces.len()*3,ctx)?;if faces.is_empty(){return Ok(Mesh::new());}
    let scale=positions.iter().map(|p|length(*p)).fold(1f64,f64::max);let epsilon=scale*1e-7;
    let mut weights=Vec::with_capacity(positions.len());for &p in positions{ctx.checkpoint(1)?;let(i,bary,_)=nearest_source(p,sources,ctx)?;weights.push(interpolate_weights(&sources[i],bary,ctx)?);}
    let mut polygons=Vec::new();let mut normals=Vec::new();
    for &face in faces {ctx.checkpoint(1)?;let p=face.map(|i|positions.get(i as usize).copied().ok_or(Error::Invalid("derived triangle index"))).into_iter().collect::<Result<Vec<_>>>()?;
        let points=[p[0],p[1],p[2]];let centroid=mul(add(add(p[0],p[1]),p[2]),1./3.);let mut mapped=None;
        if exact {for source in sources{ctx.checkpoint(1)?;let mapping=points.map(|p|barycentric(p,source.points));
            if mapping.iter().all(|(b,d)|*d<=epsilon&&b.iter().all(|v|*v>=-1e-7&&*v<=1.+1e-7)){mapped=Some((source,mapping.map(|(b,_)|b)));break;}
        }}else{let(i,_,_)=nearest_source(centroid,sources,ctx)?;mapped=Some((&sources[i],points.map(|p|closest_barycentric(p,sources[i].points).0)));}
        let (source,bary)=mapped.ok_or(Error::Invalid("boolean retessellation crosses an ambiguous authored UV/material triangle"))?;
        let uvs=bary.map(|b|std::array::from_fn(|d|(0..3).map(|i|source.uv[i][d]*b[i]).sum())).to_vec();
        let geometric=unit(cross(sub(p[1],p[0]),sub(p[2],p[0])))?;
        for b in bary{let n=unit(std::array::from_fn(|d|(0..3).map(|i|source.normals[i][d]*b[i]).sum())).unwrap_or(geometric);normals.push(if dot(n,geometric)<0.{mul(n,-1.)}else{n});}
        polygons.push(Polygon{vertices:face.to_vec(),uvs,material:source.material});
    }
    let mut mesh=Mesh::from_weighted_polygons(positions,&weights,&polygons,ctx)?;
    let split=mesh.corners().iter().zip(normals).map(|(c,n)|(c.id,Some(n))).collect::<Vec<_>>();mesh.set_corner_normals_bulk(&split,ctx)?;
    if !mesh.validate(ctx)?.is_valid_surface{return Err(Error::Invalid("derived operation produced a nonmanifold surface"));}Ok(mesh)
}
fn interpolate_weights(source:&SourceTriangle,b:[f64;3],ctx:&mut Context<'_>)->Result<Vec<JointWeight>>{
    let mut weights=BTreeMap::<u32,f64>::new();for i in 0..3{for w in &source.weights[i]{ctx.checkpoint(1)?;*weights.entry(w.joint).or_default()+=w.weight*b[i].max(0.);}}
    weights.retain(|_,v|*v>0.);if weights.len()>ctx.limits.max_weights_per_vertex{return Err(Error::Budget("reprojected influences"));}let sum=weights.values().sum::<f64>();Ok(weights.into_iter().map(|(joint,weight)|JointWeight{joint,weight:weight/sum}).collect())
}
fn barycentric(p:[f64;3],t:[[f64;3];3])->([f64;3],f64){
    let scale=length(sub(t[1],t[0])).max(length(sub(t[2],t[0])));if scale==0.{return ([1.,0.,0.],f64::INFINITY);}
    let a=mul(sub(t[1],t[0]),1./scale);let b=mul(sub(t[2],t[0]),1./scale);let v=mul(sub(p,t[0]),1./scale);let aa=dot(a,a);let ab=dot(a,b);let bb=dot(b,b);let av=dot(a,v);let bv=dot(b,v);let det=aa*bb-ab*ab;
    if det<=1e-20{return([1.,0.,0.],f64::INFINITY);}let y=(bb*av-ab*bv)/det;let z=(aa*bv-ab*av)/det;let b=[1.-y-z,y,z];let q=weighted_point(t,b);(b,length(sub(p,q)))
}
fn weighted_point(t:[[f64;3];3],b:[f64;3])->[f64;3]{std::array::from_fn(|d|(0..3).map(|i|t[i][d]*b[i]).sum())}
fn closest_barycentric(p:[f64;3],t:[[f64;3];3])->([f64;3],f64){let (b,distance)=barycentric(p,t);if b.iter().all(|v|*v>=0.)&&distance.is_finite(){return(b,distance);}
    let mut best=([1.,0.,0.],length(sub(p,t[0])));for i in 0..3{let j=(i+1)%3;let edge=sub(t[j],t[i]);let scale=length(edge);if scale==0.{continue;}let direction=mul(edge,1./scale);let f=(dot(sub(p,t[i]),direction)/scale).clamp(0.,1.);let mut bary=[0.;3];bary[i]=1.-f;bary[j]=f;let distance=length(sub(p,weighted_point(t,bary)));if distance<best.1{best=(bary,distance);}}best}
fn nearest_source(p:[f64;3],sources:&[SourceTriangle],ctx:&mut Context<'_>)->Result<(usize,[f64;3],f64)>{let mut best=None;for(i,source)in sources.iter().enumerate(){ctx.checkpoint(1)?;let(b,d)=closest_barycentric(p,source.points);if best.as_ref().is_none_or(|(_,_,distance)|d<*distance){best=Some((i,b,d));}}
    best.ok_or(Error::Invalid("empty source projection surface"))}

#[cfg(test)]
mod tests {
    use super::*;
    fn operation(s:&str)->ConstructionOperation{ConstructionOperation::parse(&json::parse(s.as_bytes()).unwrap(),&Limits::default()).unwrap().unwrap()}
    fn volume(mesh:&Mesh)->f64{let tri=mesh.triangulate(&mut Context::default()).unwrap();tri.triangles.iter().map(|t|{let p=t.indices.map(|i|tri.vertices[i as usize].position);dot(p[0],cross(p[1],p[2]))/6.}).sum()}
    #[test]
    fn sweep_lathe_loft_and_strip_are_real_geometry_with_canonical_commands(){
        let limits=Limits::default();let mut ctx=Context::default();let mut objects=BTreeMap::new();
        for command in [
            r#"{"op":"sweep","object":"tube","profile":[[-1,-1],[1,-1],[1,1],[-1,1]],"path":[[0,0,0],[0,0,1],[0,0,2]],"caps":true,"material":0}"#,
            r#"{"op":"lathe","object":"barrel","profile":[[1,-1],[1,1]],"axis":1,"segments":16,"caps":true,"material":0}"#,
            r#"{"op":"loft","object":"lofted","profiles":[[[-1,-1,0],[1,-1,0],[1,1,0],[-1,1,0]],[[-0.5,-0.5,2],[0.5,-0.5,2],[0.5,0.5,2],[-0.5,0.5,2]]],"closed":true,"caps":true,"material":0}"#,
            r#"{"op":"quad_strip","object":"ribbon","a":[[0,0,0],[0,1,0],[0,2,0]],"b":[[1,0,0],[1,1,0.1],[1,2,0]],"closed":false,"material":0}"#,
        ] {let op=operation(command);assert_eq!(ConstructionOperation::parse(&op.value(),&limits).unwrap().unwrap(),op);op.apply(&mut objects,&limits,&mut ctx).unwrap();}
        for name in ["tube","barrel","lofted"]{assert!(objects[name].validate(&mut ctx).unwrap().is_closed_manifold,"{name}");assert!(volume(&objects[name])>0.,"{name}");}
        assert!((volume(&objects["tube"])-8.).abs()<1e-10);assert_eq!(objects["ribbon"].faces().len(),2);
        let before=objects.clone();assert!(operation(r#"{"op":"lathe","object":"bad","profile":[[-1,0],[1,1]],"axis":1,"segments":16,"caps":true,"material":0}"#).apply(&mut objects,&limits,&mut ctx).is_err());assert_eq!(objects,before);
    }
    #[test]
    fn overlapping_cube_boolean_transfers_materials_and_preserves_sources(){
        let mut ctx=Context::default();let a=Mesh::cube([2.;3],&mut ctx).unwrap();let mut b=Mesh::cube([2.;3],&mut ctx).unwrap();
        let vertices=b.vertices().iter().map(|v|v.id).collect::<Vec<_>>();b.transform(&vertices,[[1.,0.,0.,0.5],[0.,1.,0.,0.5],[0.,0.,1.,0.5],[0.,0.,0.,1.]],&mut ctx).unwrap();
        b.set_face_materials(&b.faces().iter().map(|f|f.id).collect::<Vec<_>>(),7,&mut ctx).unwrap();let mut objects=BTreeMap::from([("a".into(),a.clone()),("b".into(),b.clone())]);
        operation(r#"{"op":"boolean","object":"union","a":"a","b":"b","mode":"union"}"#).apply(&mut objects,&Limits::default(),&mut ctx).unwrap();
        assert_eq!(objects["a"],a);assert_eq!(objects["b"],b);assert!(objects["union"].validate(&mut ctx).unwrap().is_closed_manifold);assert!((volume(&objects["union"])-12.625).abs()<1e-6);
        assert!(objects["union"].faces().iter().any(|f|f.material==7));assert!(objects["union"].faces().iter().any(|f|f.material==0));assert!(objects["union"].corners().iter().any(|c|c.uv!=[0.,0.]));
    }
    #[test]
    fn voxel_cube_is_bounded_reprojected_geometry_and_cancel_is_atomic(){
        let mut ctx=Context::default();let mut objects=BTreeMap::from([("source".into(),Mesh::cube([2.;3],&mut ctx).unwrap())]);let source=objects["source"].clone();
        let op=operation(r#"{"op":"voxel_remesh","object":"remeshed","source":"source","resolution":8}"#);
        assert!(op.apply(&mut objects,&Limits::default(),&mut Context::new(crate::mesh::Limits::default(),Some(&||true))).is_err());assert_eq!(objects.len(),1);
        op.apply(&mut objects,&Limits::default(),&mut ctx).unwrap();let mesh=&objects["remeshed"];assert!(mesh.validate(&mut ctx).unwrap().is_valid_surface);assert!(mesh.faces().len()>12);assert!(volume(mesh)>5.&&volume(mesh)<10.);assert_eq!(objects["source"],source);
    }
}
