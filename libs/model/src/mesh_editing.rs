//! Typed mesh edits and their binary source codec for worker-owned documents.
use crate::{json::{self,Value},mesh::*,Error,Limits,OperationResult,Result};
use std::collections::BTreeMap;
use crate::canon::{Reader,Writer};
use codec::*;
#[path="mesh_editing_bevel.rs"]
mod rounded;

#[derive(Clone,Debug,PartialEq)]
pub struct MeshEditingOperation { pub object:String, pub action:Action }
#[derive(Clone,Debug,PartialEq)]
pub enum Action {
    Region{faces:Vec<FaceId>,offset:[f64;3]}, Split{edges:Vec<EdgeKey>,factor:f64}, Collapse{edge:EdgeKey,factor:f64}, Dissolve{edge:EdgeKey},
    Fill{vertices:Vec<VertexId>,material:u32}, Bridge{a:Vec<VertexId>,b:Vec<VertexId>,material:u32}, LoopCut{edge:EdgeKey,factor:f64},
    Slide{vertices:Vec<VertexId>,towards:Vec<VertexId>,factor:f64}, Bevel{edges:Vec<EdgeKey>,width:f64}, Solidify{thickness:f64},Array{count:u32,offset:[f64;3]},
    Deform{vertices:Vec<VertexId>,deformation:Deformation},Lattice{vertices:Vec<VertexId>,bounds:[[f64;3];2],divisions:[u32;3],displacements:Vec<[f64;3]>},
    Shrinkwrap{vertices:Vec<VertexId>,reference:String,max_distance:f64,offset:f64}, Decimate{target_faces:usize,max_error:f64},
    Sculpt{vertices:Vec<VertexId>,brush:SculptBrush}, BoxUv{faces:Vec<FaceId>,scale:[f64;2],offset:[f64;2]}, CylinderUv{faces:Vec<FaceId>,axis:usize,scale:[f64;2],offset:[f64;2]},
    Subdivide{levels:u32},Mirror{axis:usize,offset:f64},FlipFaces{faces:Vec<FaceId>},
    Unwrap{faces:Vec<FaceId>},Pack{faces:Vec<FaceId>,padding:f64},Relax{faces:Vec<FaceId>,iterations:u32,factor:f64},Pin{corners:Vec<CornerId>,pinned:bool},Normals{smooth:bool,angle:f64},
    RoundBevel{angle:f64,width:f64,segments:u32},Crease{angle:f64,weight:f64},
}
impl MeshEditingOperation {
    pub fn object_name(&self)->&str{&self.object}
    pub fn retarget(&self,object:&str)->Self{let mut result=self.clone();result.object=object.into();result}
    pub fn dependencies(&self)->Vec<&str>{match &self.action{Action::Shrinkwrap{reference,..}=>vec![reference.as_str()],_=>Vec::new()}}
    pub fn is_modifier_safe(&self)->bool{match &self.action{
        Action::Subdivide{..}|Action::Mirror{..}|Action::Array{..}|Action::Solidify{..}|Action::Decimate{..}|Action::Normals{..}=>true,
        Action::Deform{vertices,..}|Action::Lattice{vertices,..}|Action::Shrinkwrap{vertices,..}=>vertices.is_empty(),_=>false,
    }}
    /// Modifier entries with a vertex domain use an empty selection to mean
    /// "resolve all evaluated vertices" here, not in ordinary editable commands.
    pub fn for_modifier(&self,object:&str,mesh:&Mesh)->Result<Self>{
        if !self.is_modifier_safe(){return Err(Error::Invalid("modifier cannot depend on fixed element selections"));}let mut operation=self.retarget(object);
        match &mut operation.action{Action::Deform{vertices,..}|Action::Lattice{vertices,..}|Action::Shrinkwrap{vertices,..}=>*vertices=mesh.vertices().iter().map(|v|v.id).collect(),_=>{}}Ok(operation)
    }
    pub fn memory_bytes(&self)->usize{let mut w=Writer::new(usize::MAX);if self.write(&mut w).is_err(){return usize::MAX;}w.bytes.len().saturating_mul(4).saturating_add(256)}
    pub fn value(&self)->Value{
        let (op,mut args):(&str,Vec<(&str,Value)>)=match &self.action{
            Action::Subdivide{levels}=>("subdivide",vec![("levels",Value::Int(*levels as i64))]),
            Action::FlipFaces{faces}=>("flip_faces",vec![("faces",face_ids(faces))]),
            Action::Mirror{axis,offset}=>("mirror",vec![("axis",Value::Int(*axis as i64)),("offset",Value::F64(*offset))]),
            Action::Region{faces,offset}=>("extrude_region",vec![("faces",face_ids(faces)),("offset",vector(offset))]),
            Action::Split{edges,factor}=>("split_edges",vec![("edges",edge_values(edges)),("factor",Value::F64(*factor))]),
            Action::Collapse{edge,factor}=>("collapse_edge",vec![("edge",edge_value(*edge)),("factor",Value::F64(*factor))]),
            Action::Dissolve{edge}=>("dissolve_edge",vec![("edge",edge_value(*edge))]),
            Action::Fill{vertices,material}=>("fill_boundary",vec![("vertices",vertex_ids(vertices)),("material",Value::Int(*material as i64))]),
            Action::Bridge{a,b,material}=>("bridge_boundaries",vec![("a",vertex_ids(a)),("b",vertex_ids(b)),("material",Value::Int(*material as i64))]),
            Action::LoopCut{edge,factor}=>("loop_cut",vec![("edge",edge_value(*edge)),("factor",Value::F64(*factor))]),
            Action::Slide{vertices,towards,factor}=>("slide_vertices",vec![("vertices",vertex_ids(vertices)),("towards",vertex_ids(towards)),("factor",Value::F64(*factor))]),
            Action::Bevel{edges,width}=>("bevel_edges",vec![("edges",edge_values(edges)),("width",Value::F64(*width))]),
            Action::Solidify{thickness}=>("solidify",vec![("thickness",Value::F64(*thickness))]),
            Action::Array{count,offset}=>("array",vec![("count",Value::Int(*count as i64)),("offset",vector(offset))]),
            Action::Deform{vertices,deformation}=>{let(op,axis,range,param)=match deformation{Deformation::Bend{axis,range,angle}=>("bend",axis,range,("angle",Value::F64(*angle))),Deformation::Twist{axis,range,angle}=>("twist",axis,range,("angle",Value::F64(*angle))),Deformation::Taper{axis,range,scales}=>("taper",axis,range,("scales",vector(scales)))};
                (op,vec![("vertices",vertex_ids(vertices)),("axis",Value::Int(*axis as i64)),("range",vector(range)),param])},
            Action::Lattice{vertices,bounds,divisions,displacements}=>("lattice",vec![("vertices",vertex_ids(vertices)),("bounds",Value::Arr(bounds.iter().map(|p|vector(p)).collect())),("divisions",Value::Arr(divisions.iter().map(|&n|Value::Int(n as i64)).collect())),("displacements",Value::Arr(displacements.iter().map(|p|vector(p)).collect()))]),
            Action::Shrinkwrap{vertices,reference,max_distance,offset}=>("shrinkwrap",vec![("vertices",vertex_ids(vertices)),("reference",json::s(reference)),("max_distance",Value::F64(*max_distance)),("offset",Value::F64(*offset))]),
            Action::Decimate{target_faces,max_error}=>("decimate",vec![("target_faces",Value::Int(*target_faces as i64)),("max_error",Value::F64(*max_error))]),
            Action::Sculpt{vertices,brush}=>("sculpt",vec![("vertices",vertex_ids(vertices)),("kind",json::s(match brush.kind{SculptKind::Inflate=>"inflate",SculptKind::Flatten=>"flatten",SculptKind::Crease=>"crease"})),("center",vector(&brush.center)),("normal",vector(&brush.normal)),("radius",Value::F64(brush.radius)),("strength",Value::F64(brush.strength)),("max_displacement",Value::F64(brush.max_displacement)),("masks",Value::Arr(brush.masks.iter().map(|(v,x)|json::obj(vec![("vertex",json::s(v.0.to_string())),("value",Value::F64(*x))])).collect())),("symmetry",Value::Int(brush.symmetry as i64))]),
            Action::BoxUv{faces,scale,offset}=>("uv_box",vec![("faces",face_ids(faces)),("scale",vector(scale)),("offset",vector(offset))]),
            Action::CylinderUv{faces,axis,scale,offset}=>("uv_cylindrical",vec![("faces",face_ids(faces)),("axis",Value::Int(*axis as i64)),("scale",vector(scale)),("offset",vector(offset))]),
            Action::Unwrap{faces}=>("uv_unwrap",vec![("faces",face_ids(faces))]),
            Action::Pack{faces,padding}=>("uv_pack",vec![("faces",face_ids(faces)),("padding",Value::F64(*padding))]),
            Action::Relax{faces,iterations,factor}=>("uv_relax",vec![("faces",face_ids(faces)),("iterations",Value::Int(*iterations as i64)),("factor",Value::F64(*factor))]),
            Action::Pin{corners,pinned}=>("uv_pin",vec![("corners",Value::Arr(corners.iter().map(|c|json::s(c.0.to_string())).collect())),("pinned",Value::Bool(*pinned))]),
            Action::Normals{smooth,angle}=>("normals",vec![("smooth",Value::Bool(*smooth)),("angle",Value::F64(*angle))]),
            Action::RoundBevel{angle,width,segments}=>("bevel",vec![("width",Value::F64(*width)),("segments",Value::Int(*segments as i64)),("angle",Value::F64(*angle))]),
            Action::Crease{angle,weight}=>("crease",vec![("angle",Value::F64(*angle)),("weight",Value::F64(*weight))]),
        };let mut values=vec![("op",json::s(op)),("object",json::s(&self.object))];values.append(&mut args);json::obj(values)
    }

    pub fn apply(&self,objects:&mut BTreeMap<String,Mesh>,limits:&Limits,ctx:&mut Context<'_>)->Result<OperationResult>{
        self.apply_in_scene(objects,&crate::SceneState::default(),limits,ctx)
    }
    pub(crate) fn apply_in_scene(&self,objects:&mut BTreeMap<String,Mesh>,scene:&crate::SceneState,limits:&Limits,ctx:&mut Context<'_>)->Result<OperationResult>{
        self.check(limits)?;
        let reference=if let Action::Shrinkwrap{reference,..}=&self.action{let source=objects.get(reference).ok_or_else(||Error::MissingObject(reference.clone()))?;
            ctx.checkpoint(0)?;if source.memory_bytes().saturating_mul(2)>ctx.limits.max_bytes{return Err(Error::Budget("reference clone bytes"));}let mut source=source.clone();
            let matrix=crate::transform::matrix_mul(crate::transform::inverse(scene.world_matrix(&self.object)?)?,scene.world_matrix(reference)?);
            if matrix!=crate::transform::IDENTITY_MATRIX{let vertices=source.vertices().iter().map(|v|v.id).collect::<Vec<_>>();source.transform(&vertices,matrix,ctx)?;}Some(source)}else{None};
        let mesh=objects.get_mut(&self.object).ok_or_else(||Error::MissingObject(self.object.clone()))?;let mut result=OperationResult{object:self.object.clone(),..Default::default()};
        let changes=match &self.action {
            Action::Subdivide{levels}=>mesh.subdivide(*levels,ctx)?,Action::Mirror{axis,offset}=>mesh.mirror(*axis,*offset,ctx)?,
            Action::FlipFaces{faces}=>{result.faces=faces.clone();mesh.flip_faces(faces,ctx)?},
            Action::Region{faces,offset}=>{let out=mesh.extrude_region(faces,*offset,ctx)?;result.faces=out.caps;result.faces.extend(out.side_faces);out.changes},
            Action::Split{edges,factor}=>mesh.split_edges(edges,*factor,ctx)?,Action::Collapse{edge,factor}=>mesh.collapse_edge(*edge,*factor,ctx)?,Action::Dissolve{edge}=>mesh.dissolve_edge(*edge,ctx)?,
            Action::Fill{vertices,material}=>mesh.fill_boundary(vertices,*material,ctx)?,Action::Bridge{a,b,material}=>mesh.bridge_boundaries(a,b,*material,ctx)?,Action::LoopCut{edge,factor}=>mesh.loop_cut(*edge,*factor,ctx)?,
            Action::Slide{vertices,towards,factor}=>{result.vertices=vertices.clone();mesh.slide_vertices(vertices,towards,*factor,ctx)?},Action::Bevel{edges,width}=>mesh.bevel_edges(edges,*width,ctx)?,Action::Solidify{thickness}=>mesh.solidify(*thickness,ctx)?,Action::Array{count,offset}=>mesh.array(*count,*offset,ctx)?,
            Action::Deform{vertices,deformation}=>{result.vertices=vertices.clone();mesh.deform(vertices,deformation,ctx)?},Action::Lattice{vertices,bounds,divisions,displacements}=>{result.vertices=vertices.clone();mesh.lattice(vertices,*bounds,*divisions,displacements,ctx)?},
            Action::Shrinkwrap{vertices,max_distance,offset,..}=>{result.vertices=vertices.clone();mesh.shrinkwrap(vertices,reference.as_ref().unwrap(),*max_distance,*offset,ctx)?},
            Action::Decimate{target_faces,max_error}=>{let out=mesh.decimate(*target_faces,*max_error,ctx)?;
                result.metrics.insert("achieved_faces".into(),out.achieved_faces);result.metrics.insert("collapses".into(),out.collapses);
                result.faces=mesh.faces().iter().map(|f|f.id).collect();out.changes},
            Action::Sculpt{vertices,brush}=>{result.vertices=vertices.clone();mesh.sculpt(vertices,brush,ctx)?},
            Action::BoxUv{faces,scale,offset}=>{result.faces=faces.clone();mesh.project_uv_box(faces,*scale,*offset,ctx)?},Action::CylinderUv{faces,axis,scale,offset}=>{result.faces=faces.clone();mesh.project_uv_cylindrical(faces,*axis,*scale,*offset,ctx)?},
            Action::Unwrap{faces}=>{result.faces=faces.clone();mesh.unwrap_uv(faces,ctx)?},Action::Pack{faces,padding}=>{result.faces=faces.clone();mesh.pack_uv(faces,*padding,ctx)?},Action::Relax{faces,iterations,factor}=>{result.faces=faces.clone();mesh.relax_uv(faces,*iterations,*factor,ctx)?},
            Action::Pin{corners,pinned}=>mesh.pin_uv(corners,*pinned,ctx)?,Action::Normals{smooth,angle}=>mesh.recalculate_normals(*smooth,*angle,ctx)?,
            Action::RoundBevel{angle,width,segments}=>{*mesh=rounded::bevel(mesh,*angle,*width,*segments,ctx)?;result.faces=mesh.faces().iter().map(|f|f.id).collect();ChangeSet::default()},
            Action::Crease{angle,weight}=>rounded::crease(mesh,*angle,*weight,ctx)?,
        };
        if result.faces.is_empty(){result.faces=changes.created.iter().filter_map(|id|if let ElementId::Face(f)=id{Some(*f)}else{None}).collect();}
        if result.vertices.is_empty(){result.vertices=changes.created.iter().filter_map(|id|if let ElementId::Vertex(v)=id{Some(*v)}else{None}).collect();}
        Ok(result)
    }
}
fn checked_name(name:&str,limits:&Limits)->Result<()>{if name.is_empty()||name.len()>limits.max_name_bytes||name.chars().any(char::is_control){Err(Error::Invalid("mesh object name"))}else{Ok(())}}
fn vector(v:&[f64])->Value{Value::Arr(v.iter().copied().map(Value::F64).collect())}
fn vertex_ids(v:&[VertexId])->Value{Value::Arr(v.iter().map(|v|json::s(v.0.to_string())).collect())}
fn face_ids(v:&[FaceId])->Value{Value::Arr(v.iter().map(|v|json::s(v.0.to_string())).collect())}
fn edge_value(e:EdgeKey)->Value{vertex_ids(&[e.0,e.1])}
fn edge_values(e:&[EdgeKey])->Value{Value::Arr(e.iter().map(|&e|edge_value(e)).collect())}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_canonical_roundtrip_and_local_failure_atomicity(){
        let limits=Limits::default();let mut ctx=Context::default();let mesh=Mesh::cube([2.;3],&mut ctx).unwrap();let edge=*mesh.adjacency(&mut ctx).unwrap().edges.keys().next().unwrap();
        let op=MeshEditingOperation {object:"body".into(),action:Action::Bevel {edges:vec![edge],width:0.2}};
        let mut w=Writer::new(usize::MAX);op.write(&mut w).unwrap();
        let mut r=Reader::new(&w.bytes);assert_eq!(MeshEditingOperation::read(&mut r,&limits).unwrap(),op);r.end().unwrap();
        let mut objects=BTreeMap::from([("body".into(),mesh)]);op.apply(&mut objects,&limits,&mut ctx).unwrap();assert_eq!(objects["body"].faces().len(),7);
        let before=objects.clone();let bad=MeshEditingOperation { object: "body".into(), action: Action::Solidify { thickness: -1.0 } };assert!(bad.apply(&mut objects,&limits,&mut ctx).is_err());assert_eq!(objects,before);
        assert!(MeshEditingOperation {object:"body".into(),action:Action::Split {
            edges:vec![EdgeKey(VertexId(2),VertexId(1))],factor:0.5,
        }}.check(&limits).is_err());
    }
    #[test]
    fn decimation_metrics_are_explicit_when_seams_block_reduction(){
        let mut ctx=Context::default();let mut objects=BTreeMap::from([("body".into(),Mesh::cube([2.;3],&mut ctx).unwrap())]);let op=MeshEditingOperation { object: "body".into(), action: Action::Decimate { target_faces: 4, max_error: 10.0 } };
        let result=op.apply(&mut objects,&Limits::default(),&mut ctx).unwrap();assert_eq!(result.metrics["achieved_faces"],12);assert_eq!(result.metrics["collapses"],0);
    }
}

fn write_action(w:&mut Writer,value:&Action)->Result<()>{match value{
    Action::Region{faces,offset}=>{w.u8(0)?;w.count(faces.len())?;for item in faces{w.u64(item.0)?;}for item in offset{w.f64(*item)?;}},
    Action::Split{edges,factor}=>{w.u8(1)?;w.count(edges.len())?;for item in edges{w.u64(item.0.0)?;w.u64(item.1.0)?;}w.f64(*factor)?;},
    Action::Collapse{edge,factor}=>{w.u8(2)?;w.u64(edge.0.0)?;w.u64(edge.1.0)?;w.f64(*factor)?;},
    Action::Dissolve{edge}=>{w.u8(3)?;w.u64(edge.0.0)?;w.u64(edge.1.0)?;},
    Action::Fill{vertices,material}=>{w.u8(4)?;w.count(vertices.len())?;for item in vertices{w.u64(item.0)?;}w.u32(*material)?;},
    Action::Bridge{a,b,material}=>{w.u8(5)?;w.count(a.len())?;for item in a{w.u64(item.0)?;}w.count(b.len())?;for item in b{w.u64(item.0)?;}w.u32(*material)?;},
    Action::LoopCut{edge,factor}=>{w.u8(6)?;w.u64(edge.0.0)?;w.u64(edge.1.0)?;w.f64(*factor)?;},
    Action::Slide{vertices,towards,factor}=>{w.u8(7)?;w.count(vertices.len())?;for item in vertices{w.u64(item.0)?;}w.count(towards.len())?;for item in towards{w.u64(item.0)?;}w.f64(*factor)?;},
    Action::Bevel{edges,width}=>{w.u8(8)?;w.count(edges.len())?;for item in edges{w.u64(item.0.0)?;w.u64(item.1.0)?;}w.f64(*width)?;},
    Action::Solidify{thickness}=>{w.u8(9)?;w.f64(*thickness)?;},
    Action::Array{count,offset}=>{w.u8(10)?;w.u32(*count)?;for item in offset{w.f64(*item)?;}},
    Action::Deform{vertices,deformation}=>{w.u8(11)?;w.count(vertices.len())?;for item in vertices{w.u64(item.0)?;}write_deformation(w,deformation)?;},
    Action::Lattice{vertices,bounds,divisions,displacements}=>{w.u8(12)?;w.count(vertices.len())?;for item in vertices{w.u64(item.0)?;}for item in bounds{for item in item{w.f64(*item)?;}}for item in divisions{w.u32(*item)?;}w.count(displacements.len())?;for item in displacements{for item in item{w.f64(*item)?;}}},
    Action::Shrinkwrap{vertices,reference,max_distance,offset}=>{w.u8(13)?;w.count(vertices.len())?;for item in vertices{w.u64(item.0)?;}w.string(reference)?;w.f64(*max_distance)?;w.f64(*offset)?;},
    Action::Decimate{target_faces,max_error}=>{w.u8(14)?;w.u32((*target_faces).try_into().map_err(|_|Error::Budget("count"))?)?;w.f64(*max_error)?;},
    Action::Sculpt{vertices,brush}=>{w.u8(15)?;w.count(vertices.len())?;for item in vertices{w.u64(item.0)?;}write_sculptbrush(w,brush)?;},
    Action::BoxUv{faces,scale,offset}=>{w.u8(16)?;w.count(faces.len())?;for item in faces{w.u64(item.0)?;}for item in scale{w.f64(*item)?;}for item in offset{w.f64(*item)?;}},
    Action::CylinderUv{faces,axis,scale,offset}=>{w.u8(17)?;w.count(faces.len())?;for item in faces{w.u64(item.0)?;}w.u32((*axis).try_into().map_err(|_|Error::Budget("count"))?)?;for item in scale{w.f64(*item)?;}for item in offset{w.f64(*item)?;}},
    Action::Subdivide{levels}=>{w.u8(18)?;w.u32(*levels)?;},
    Action::Mirror{axis,offset}=>{w.u8(19)?;w.u32((*axis).try_into().map_err(|_|Error::Budget("count"))?)?;w.f64(*offset)?;},
    Action::FlipFaces{faces}=>{w.u8(20)?;w.count(faces.len())?;for item in faces{w.u64(item.0)?;}},
    Action::Unwrap{faces}=>{w.u8(21)?;w.count(faces.len())?;for item in faces{w.u64(item.0)?;}},
    Action::Pack{faces,padding}=>{w.u8(22)?;w.count(faces.len())?;for item in faces{w.u64(item.0)?;}w.f64(*padding)?;},
    Action::Relax{faces,iterations,factor}=>{w.u8(23)?;w.count(faces.len())?;for item in faces{w.u64(item.0)?;}w.u32(*iterations)?;w.f64(*factor)?;},
    Action::Pin{corners,pinned}=>{w.u8(24)?;w.count(corners.len())?;for item in corners{w.u64(item.0)?;}w.u8(*pinned as u8)?;},
    Action::Normals{smooth,angle}=>{w.u8(25)?;w.u8(*smooth as u8)?;w.f64(*angle)?;},
    Action::RoundBevel{angle,width,segments}=>{w.u8(26)?;w.f64(*angle)?;w.f64(*width)?;w.u32(*segments)?;},
    Action::Crease{angle,weight}=>{w.u8(27)?;w.f64(*angle)?;w.f64(*weight)?;},
}Ok(())}
fn read_action(r:&mut Reader,limits:&Limits)->Result<Action>{let _=limits;Ok(match r.u8()?{
    0=>Action::Region{faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values},offset:[r.f64()?,r.f64()?,r.f64()?]},
    1=>Action::Split{edges:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_edges)?{values.push(EdgeKey(VertexId(r.u64()?),VertexId(r.u64()?)));}values},factor:r.f64()?},
    2=>Action::Collapse{edge:EdgeKey(VertexId(r.u64()?),VertexId(r.u64()?)),factor:r.f64()?},
    3=>Action::Dissolve{edge:EdgeKey(VertexId(r.u64()?),VertexId(r.u64()?))},
    4=>Action::Fill{vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},material:r.u32()?},
    5=>Action::Bridge{a:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},b:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},material:r.u32()?},
    6=>Action::LoopCut{edge:EdgeKey(VertexId(r.u64()?),VertexId(r.u64()?)),factor:r.f64()?},
    7=>Action::Slide{vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},towards:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},factor:r.f64()?},
    8=>Action::Bevel{edges:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_edges)?{values.push(EdgeKey(VertexId(r.u64()?),VertexId(r.u64()?)));}values},width:r.f64()?},
    9=>Action::Solidify{thickness:r.f64()?},
    10=>Action::Array{count:r.u32()?,offset:[r.f64()?,r.f64()?,r.f64()?]},
    11=>Action::Deform{vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},deformation:read_deformation(r,limits)?},
    12=>Action::Lattice{vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},bounds:[[r.f64()?,r.f64()?,r.f64()?],[r.f64()?,r.f64()?,r.f64()?]],divisions:[r.u32()?,r.u32()?,r.u32()?],displacements:{let mut values=Vec::new();for _ in 0..r.count(512)?{values.push([r.f64()?,r.f64()?,r.f64()?]);}values}},
    13=>Action::Shrinkwrap{vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},reference:r.string(limits.max_name_bytes)?,max_distance:r.f64()?,offset:r.f64()?},
    14=>Action::Decimate{target_faces:r.u32()? as usize,max_error:r.f64()?},
    15=>Action::Sculpt{vertices:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push(VertexId(r.u64()?));}values},brush:read_sculptbrush(r,limits)?},
    16=>Action::BoxUv{faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values},scale:[r.f64()?,r.f64()?],offset:[r.f64()?,r.f64()?]},
    17=>Action::CylinderUv{faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values},axis:r.u32()? as usize,scale:[r.f64()?,r.f64()?],offset:[r.f64()?,r.f64()?]},
    18=>Action::Subdivide{levels:r.u32()?},
    19=>Action::Mirror{axis:r.u32()? as usize,offset:r.f64()?},
    20=>Action::FlipFaces{faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values}},
    21=>Action::Unwrap{faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values}},
    22=>Action::Pack{faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values},padding:r.f64()?},
    23=>Action::Relax{faces:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_faces)?{values.push(FaceId(r.u64()?));}values},iterations:r.u32()?,factor:r.f64()?},
    24=>Action::Pin{corners:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_corners)?{values.push(CornerId(r.u64()?));}values},pinned:read_bool(r)?},
    25=>Action::Normals{smooth:read_bool(r)?,angle:r.f64()?},
    26=>Action::RoundBevel{angle:r.f64()?,width:r.f64()?,segments:r.u32()?},
    27=>Action::Crease{angle:r.f64()?,weight:r.f64()?},
    _=>return Err(Error::Corrupt("Action tag")),
})}
fn write_deformation(w:&mut Writer,value:&Deformation)->Result<()>{match value{
    Deformation::Bend{axis,angle,range}=>{w.u8(0)?;w.u32((*axis).try_into().map_err(|_|Error::Budget("count"))?)?;w.f64(*angle)?;for item in range{w.f64(*item)?;}},
    Deformation::Twist{axis,angle,range}=>{w.u8(1)?;w.u32((*axis).try_into().map_err(|_|Error::Budget("count"))?)?;w.f64(*angle)?;for item in range{w.f64(*item)?;}},
    Deformation::Taper{axis,scales,range}=>{w.u8(2)?;w.u32((*axis).try_into().map_err(|_|Error::Budget("count"))?)?;for item in scales{w.f64(*item)?;}for item in range{w.f64(*item)?;}},
}Ok(())}
fn read_deformation(r:&mut Reader,limits:&Limits)->Result<Deformation>{let _=limits;Ok(match r.u8()?{
    0=>Deformation::Bend{axis:r.u32()? as usize,angle:r.f64()?,range:[r.f64()?,r.f64()?]},
    1=>Deformation::Twist{axis:r.u32()? as usize,angle:r.f64()?,range:[r.f64()?,r.f64()?]},
    2=>Deformation::Taper{axis:r.u32()? as usize,scales:[r.f64()?,r.f64()?],range:[r.f64()?,r.f64()?]},
    _=>return Err(Error::Corrupt("Deformation tag")),
})}
fn write_sculptkind(w:&mut Writer,value:&SculptKind)->Result<()>{match value{
    SculptKind::Inflate=>{w.u8(0)?;},
    SculptKind::Flatten=>{w.u8(1)?;},
    SculptKind::Crease=>{w.u8(2)?;},
}Ok(())}
fn read_sculptkind(r:&mut Reader,limits:&Limits)->Result<SculptKind>{let _=limits;Ok(match r.u8()?{
    0=>SculptKind::Inflate,
    1=>SculptKind::Flatten,
    2=>SculptKind::Crease,
    _=>return Err(Error::Corrupt("SculptKind tag")),
})}
fn write_sculptbrush(w:&mut Writer,value:&SculptBrush)->Result<()>{write_sculptkind(w,&value.kind)?;for item in &value.center{w.f64(*item)?;}for item in &value.normal{w.f64(*item)?;}w.f64(value.radius)?;w.f64(value.strength)?;w.f64(value.max_displacement)?;w.count(value.masks.len())?;for item in &value.masks{w.u64(item.0.0)?;w.f64(item.1)?;}w.u8(value.symmetry)?;Ok(())}
fn read_sculptbrush(r:&mut Reader,limits:&Limits)->Result<SculptBrush>{let _=limits;Ok(SculptBrush{kind:read_sculptkind(r,limits)?,center:[r.f64()?,r.f64()?,r.f64()?],normal:[r.f64()?,r.f64()?,r.f64()?],radius:r.f64()?,strength:r.f64()?,max_displacement:r.f64()?,masks:{let mut values=Vec::new();for _ in 0..r.count(limits.mesh.max_vertices)?{values.push((VertexId(r.u64()?),r.f64()?));}values},symmetry:r.u8()?})}

// Shared binary field primitives; callers supply typed collection bounds.
pub(crate) mod codec {
    use crate::{canon::Reader,Error,Result};
    pub fn read_bool(r:&mut Reader)->Result<bool>{match r.u8()?{0=>Ok(false),1=>Ok(true),_=>Err(Error::Corrupt("boolean tag"))}}
    pub fn read_f32(r:&mut Reader)->Result<f32>{let v=r.f64()?;let f=v as f32;if f as f64!=v{return Err(Error::Corrupt("noncanonical f32"));}Ok(f)}
    pub fn finite(v:impl IntoIterator<Item=f64>)->Result<()>{if v.into_iter().any(|v|!v.is_finite()){return Err(Error::Invalid("expected finite number"));}Ok(())}
    pub fn read_ids<T>(r:&mut Reader,max:usize,id:impl Fn(u64)->T)->Result<Vec<T>>{let mut out=Vec::new();for _ in 0..r.count(max)?{out.push(id(r.u64()?));}Ok(out)}
    pub fn check_count(n:usize,max:usize)->Result<()>{if n>max{return Err(Error::Budget("selection size"));}Ok(())}
}

impl MeshEditingOperation {
    pub fn check(&self,limits:&Limits)->Result<()>{
        checked_name(&self.object,limits)?;
        let vertices=|v:&[VertexId]|check_count(v.len(),limits.mesh.max_vertices);
        let faces=|v:&[FaceId]|check_count(v.len(),limits.mesh.max_faces);
        let edge=|e:EdgeKey|if e.0>=e.1{Err(Error::Invalid("edge endpoint IDs must be strictly increasing"))}else{Ok(())};
        match &self.action{
            Action::Region{faces:f,offset}=>{faces(f)?;finite(*offset)?;},
            Action::Split{edges,factor}=>{if edges.len()>limits.mesh.max_edges{return Err(Error::Budget("edge selection"));}for e in edges{edge(*e)?;}finite([*factor])?;},
            Action::Collapse{edge:e,factor}|Action::LoopCut{edge:e,factor}=>{edge(*e)?;finite([*factor])?;},
            Action::Dissolve{edge:e}=>edge(*e)?,
            Action::Fill{vertices:v,..}=>vertices(v)?,Action::Bridge{a,b,..}=>{vertices(a)?;vertices(b)?;},
            Action::Slide{vertices:v,towards,factor}=>{vertices(v)?;vertices(towards)?;finite([*factor])?;},
            Action::Bevel{edges,width}=>{if edges.len()>limits.mesh.max_edges{return Err(Error::Budget("edge selection"));}for e in edges{edge(*e)?;}finite([*width])?;},
            Action::Solidify{thickness}=>finite([*thickness])?,Action::Array{offset,..}=>finite(*offset)?,
            Action::Deform{vertices:v,deformation}=>{vertices(v)?;match deformation{
                Deformation::Bend{axis,range,angle}|Deformation::Twist{axis,range,angle}=>{u32::try_from(*axis).map_err(|_|Error::Invalid("expected u32 integer"))?;finite(*range)?;finite([*angle])?;},
                Deformation::Taper{axis,range,scales}=>{u32::try_from(*axis).map_err(|_|Error::Invalid("expected u32 integer"))?;finite(*range)?;finite(*scales)?;}
            }},
            Action::Lattice{vertices:v,bounds,divisions,displacements}=>{vertices(v)?;
                if divisions.iter().any(|d|!(2..=8).contains(d)){return Err(Error::Invalid("lattice divisions must be 2..8"));}
                if displacements.len()!=divisions.iter().product::<u32>()as usize{return Err(Error::Invalid("lattice control count mismatch"));}
                finite(bounds.iter().flatten().copied())?;finite(displacements.iter().flatten().copied())?;
            },
            Action::Shrinkwrap{vertices:v,reference,max_distance,offset}=>{vertices(v)?;checked_name(reference,limits)?;finite([*max_distance,*offset])?;},
            Action::Decimate{target_faces,max_error}=>{u32::try_from(*target_faces).map_err(|_|Error::Invalid("expected u32 integer"))?;finite([*max_error])?;},
            Action::Sculpt{vertices:v,brush}=>{vertices(v)?;if brush.masks.len()>limits.mesh.max_vertices{return Err(Error::Budget("sculpt masks"));}if brush.symmetry>7{return Err(Error::Invalid("symmetry bits must be 0..7"));}
                finite(brush.center)?;finite(brush.normal)?;finite([brush.radius,brush.strength,brush.max_displacement])?;finite(brush.masks.iter().map(|(_,v)|*v))?;},
            Action::BoxUv{faces:f,scale,offset}=>{faces(f)?;finite(*scale)?;finite(*offset)?;},
            Action::CylinderUv{faces:f,axis,scale,offset}=>{faces(f)?;u32::try_from(*axis).map_err(|_|Error::Invalid("expected u32 integer"))?;finite(*scale)?;finite(*offset)?;},
            Action::Mirror{axis,offset}=>{u32::try_from(*axis).map_err(|_|Error::Invalid("expected u32 integer"))?;finite([*offset])?;},
            Action::FlipFaces{faces:f}|Action::Unwrap{faces:f}=>faces(f)?,
            Action::Pack{faces:f,padding}=>{faces(f)?;finite([*padding])?;},Action::Relax{faces:f,factor,..}=>{faces(f)?;finite([*factor])?;},
            Action::Pin{corners,..}=>check_count(corners.len(),limits.mesh.max_corners)?,Action::Normals{angle,..}=>finite([*angle])?,
            Action::RoundBevel{angle,width,..}=>finite([*angle,*width])?,Action::Crease{angle,weight}=>finite([*angle,*weight])?,Action::Subdivide{..}=>{},
        }Ok(())
    }
    pub(crate) fn write(&self,w:&mut Writer)->Result<()>{w.string(&self.object)?;write_action(w,&self.action)}
    pub(crate) fn read(r:&mut Reader,limits:&Limits)->Result<Self>{let value=Self::read_template(r,limits)?;value.check(limits)?;Ok(value)}
    pub(crate) fn read_template(r:&mut Reader,limits:&Limits)->Result<Self>{Ok(Self{object:r.string(limits.max_name_bytes)?,action:read_action(r,limits)?})}
}

#[cfg(test)]
pub(crate) mod typed_tests {
    use super::*;
    pub(crate) fn actions()->Vec<Action>{
        let v=vec![VertexId(1),VertexId(u64::MAX)];let f=vec![FaceId(3),FaceId(u64::MAX)];let e=EdgeKey(v[0],v[1]);
        let mut out=vec![
            Action::Region{faces:f.clone(),offset:[1.,2.,3.]},Action::Split{edges:vec![e],factor:0.5},Action::Collapse{edge:e,factor:0.25},Action::Dissolve{edge:e},
            Action::Fill{vertices:v.clone(),material:7},Action::Bridge{a:v.clone(),b:vec![VertexId(4),VertexId(5)],material:8},Action::LoopCut{edge:e,factor:0.5},
            Action::Slide{vertices:v.clone(),towards:vec![VertexId(4),VertexId(5)],factor:0.2},Action::Bevel{edges:vec![e],width:0.1},Action::Solidify{thickness:0.1},Action::Array{count:3,offset:[1.,0.,0.]},
            Action::Lattice{vertices:v.clone(),bounds:[[0.;3],[1.;3]],divisions:[2;3],displacements:vec![[0.1,0.2,0.3];8]},
            Action::Shrinkwrap{vertices:v.clone(),reference:"reference".into(),max_distance:1.,offset:0.01},Action::Decimate{target_faces:4,max_error:0.1},
            Action::BoxUv{faces:f.clone(),scale:[2.;2],offset:[0.1;2]},Action::CylinderUv{faces:f.clone(),axis:1,scale:[1.;2],offset:[0.;2]},
            Action::Subdivide{levels:2},Action::Mirror{axis:2,offset:0.5},Action::FlipFaces{faces:f.clone()},Action::Unwrap{faces:f.clone()},
            Action::Pack{faces:f.clone(),padding:0.01},Action::Relax{faces:f,iterations:3,factor:0.5},Action::Pin{corners:vec![CornerId(u64::MAX)],pinned:true},
            Action::Normals{smooth:false,angle:0.5},Action::RoundBevel{angle:0.52,width:0.1,segments:3},Action::Crease{angle:0.52,weight:1.},
        ];
        for deformation in [Deformation::Bend{axis:0,angle:0.2,range:[0.,1.]},Deformation::Twist{axis:1,angle:0.4,range:[-1.,1.]},Deformation::Taper{axis:2,scales:[1.,0.5],range:[0.,2.]}]{out.push(Action::Deform{vertices:v.clone(),deformation});}
        for kind in [SculptKind::Inflate,SculptKind::Flatten,SculptKind::Crease]{out.push(Action::Sculpt{vertices:v.clone(),brush:SculptBrush{kind,center:[0.;3],normal:[0.,1.,0.],radius:1.,strength:0.2,max_displacement:0.5,masks:vec![(VertexId(1),0.25)],symmetry:7}});}
        out
    }
    #[test]
    fn literal_variants_have_canonical_binary_roundtrips(){
        let limits=Limits::default();
        for action in actions(){let op=MeshEditingOperation{object:"body".into(),action};op.check(&limits).unwrap();let mut w=Writer::new(usize::MAX);op.write(&mut w).unwrap();let mut r=Reader::new(&w.bytes);assert_eq!(MeshEditingOperation::read(&mut r,&limits).unwrap(),op);r.end().unwrap();assert_eq!(op.memory_bytes(),w.bytes.len()*4+256);}
    }
    #[test]
    fn validation_and_corrupt_input(){
        let limits=Limits::default();
        for action in [Action::Split{edges:vec![EdgeKey(VertexId(2),VertexId(1))],factor:0.5},Action::Solidify{thickness:f64::NAN},Action::Lattice{vertices:vec![],bounds:[[0.;3],[1.;3]],divisions:[u32::MAX;3],displacements:vec![]},Action::Lattice{vertices:vec![],bounds:[[0.;3],[1.;3]],divisions:[2;3],displacements:vec![]}]{assert!(MeshEditingOperation{object:"body".into(),action}.check(&limits).is_err());}
        let mut limits=limits;limits.mesh.max_vertices=1;assert!(MeshEditingOperation{object:"body".into(),action:Action::Fill{vertices:vec![VertexId(1),VertexId(2)],material:0}}.check(&limits).is_err());
        assert!(MeshEditingOperation{object:"".into(),action:Action::Subdivide{levels:1}}.check(&limits).is_err());
        let mut w=Writer::new(1024);w.string("body").unwrap();w.u8(255).unwrap();assert!(matches!(MeshEditingOperation::read(&mut Reader::new(&w.bytes),&limits),Err(Error::Corrupt("Action tag"))));
        let mut w=Writer::new(1024);w.string("body").unwrap();w.u8(25).unwrap();w.u8(2).unwrap();w.f64(0.5).unwrap();assert!(matches!(MeshEditingOperation::read(&mut Reader::new(&w.bytes),&limits),Err(Error::Corrupt("boolean tag"))));
        assert!(MeshEditingOperation::read(&mut Reader::new(&[]),&limits).is_err());
    }
}
