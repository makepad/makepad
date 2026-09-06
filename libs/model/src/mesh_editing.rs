//! Typed bridge from strict JSON/source logs to worker-owned mesh operations.
use crate::{json::{self,Value},mesh::*,service::{array,fields,float,integer,need,selections,stable_id,text},Error,Limits,OperationResult,Result};
use std::collections::BTreeMap;

#[derive(Clone,Debug,PartialEq)]
pub struct MeshEditingOperation { object:String, action:Action }
#[derive(Clone,Debug,PartialEq)]
enum Action {
    Region{faces:Vec<FaceId>,offset:[f64;3]}, Split{edges:Vec<EdgeKey>,factor:f64}, Collapse{edge:EdgeKey,factor:f64}, Dissolve{edge:EdgeKey},
    Fill{vertices:Vec<VertexId>,material:u32}, Bridge{a:Vec<VertexId>,b:Vec<VertexId>,material:u32}, LoopCut{edge:EdgeKey,factor:f64},
    Slide{vertices:Vec<VertexId>,towards:Vec<VertexId>,factor:f64}, Bevel{edges:Vec<EdgeKey>,width:f64}, Solidify{thickness:f64},Array{count:u32,offset:[f64;3]},
    Deform{vertices:Vec<VertexId>,deformation:Deformation},Lattice{vertices:Vec<VertexId>,bounds:[[f64;3];2],divisions:[u32;3],displacements:Vec<[f64;3]>},
    Shrinkwrap{vertices:Vec<VertexId>,reference:String,max_distance:f64,offset:f64}, Decimate{target_faces:usize,max_error:f64},
    Sculpt{vertices:Vec<VertexId>,brush:SculptBrush}, BoxUv{faces:Vec<FaceId>,scale:[f64;2],offset:[f64;2]}, CylinderUv{faces:Vec<FaceId>,axis:usize,scale:[f64;2],offset:[f64;2]},
    Subdivide{levels:u32},Mirror{axis:usize,offset:f64},FlipFaces{faces:Vec<FaceId>},
    Unwrap{faces:Vec<FaceId>},Pack{faces:Vec<FaceId>,padding:f64},Relax{faces:Vec<FaceId>,iterations:u32,factor:f64},Pin{corners:Vec<CornerId>,pinned:bool},Normals{smooth:bool,angle:f64},
}
impl MeshEditingOperation {
    pub fn parse(value:&Value,limits:&Limits)->Result<Option<Self>>{
        let op=text(value,"op")?;let vf=|name:&str|->Result<Vec<VertexId>>{Ok(selections(need(value,name)?,limits.mesh.max_vertices)?.into_iter().map(VertexId).collect())};
        let ff=||->Result<Vec<FaceId>>{Ok(selections(need(value,"faces")?,limits.mesh.max_faces)?.into_iter().map(FaceId).collect())};
        let edges=||edge_list(need(value,"edges")?,limits.mesh.max_edges);
        let action=match op {
            "subdivide"=>{fields(value,&["op","object","levels"])?;Action::Subdivide{levels:integer(need(value,"levels")?)?}},
            "flip_faces"=>{fields(value,&["op","object","faces"])?;Action::FlipFaces{faces:ff()?}},
            "mirror"=>{fields(value,&["op","object","axis","offset"])?;Action::Mirror{axis:integer(need(value,"axis")?)? as usize,offset:float(need(value,"offset")?)?}},
            "extrude_region"=>{fields(value,&["op","object","faces","offset"])?;Action::Region{faces:ff()?,offset:array(need(value,"offset")?)?}},
            "split_edges"=>{fields(value,&["op","object","edges","factor"])?;Action::Split{edges:edges()?,factor:float(need(value,"factor")?)?}},
            "collapse_edge"=>{fields(value,&["op","object","edge","factor"])?;Action::Collapse{edge:edge(need(value,"edge")?)?,factor:float(need(value,"factor")?)?}},
            "dissolve_edge"=>{fields(value,&["op","object","edge"])?;Action::Dissolve{edge:edge(need(value,"edge")?)?}},
            "fill_boundary"=>{fields(value,&["op","object","vertices","material"])?;Action::Fill{vertices:vf("vertices")?,material:integer(need(value,"material")?)?}},
            "bridge_boundaries"=>{fields(value,&["op","object","a","b","material"])?;Action::Bridge{a:vf("a")?,b:vf("b")?,material:integer(need(value,"material")?)?}},
            "loop_cut"=>{fields(value,&["op","object","edge","factor"])?;Action::LoopCut{edge:edge(need(value,"edge")?)?,factor:float(need(value,"factor")?)?}},
            "slide_vertices"=>{fields(value,&["op","object","vertices","towards","factor"])?;Action::Slide{vertices:vf("vertices")?,towards:vf("towards")?,factor:float(need(value,"factor")?)?}},
            "bevel_edges"=>{fields(value,&["op","object","edges","width"])?;Action::Bevel{edges:edges()?,width:float(need(value,"width")?)?}},
            "solidify"=>{fields(value,&["op","object","thickness"])?;Action::Solidify{thickness:float(need(value,"thickness")?)?}},
            "array"=>{fields(value,&["op","object","count","offset"])?;Action::Array{count:integer(need(value,"count")?)?,offset:array(need(value,"offset")?)?}},
            "bend"|"twist"|"taper"=>{
                fields(value,if op=="taper"{&["op","object","vertices","axis","range","scales"]}else{&["op","object","vertices","axis","range","angle"]})?;
                let axis=integer(need(value,"axis")?)? as usize;let range=array(need(value,"range")?)?;let deformation=match op {
                    "bend"=>Deformation::Bend{axis,range,angle:float(need(value,"angle")?)?},"twist"=>Deformation::Twist{axis,range,angle:float(need(value,"angle")?)?},
                    _=>Deformation::Taper{axis,range,scales:array(need(value,"scales")?)?},};Action::Deform{vertices:vf("vertices")?,deformation}
            },
            "lattice"=>{fields(value,&["op","object","vertices","bounds","divisions","displacements"])?;
                let bounds=need(value,"bounds")?.as_arr().filter(|a|a.len()==2).ok_or(Error::Invalid("lattice bounds require two XYZ values"))?;
                let d=need(value,"divisions")?.as_arr().filter(|a|a.len()==3).ok_or(Error::Invalid("lattice divisions require three integers"))?;
                let divisions=[integer(&d[0])?,integer(&d[1])?,integer(&d[2])?];if divisions.iter().any(|d|!(2..=8).contains(d)){return Err(Error::Invalid("lattice divisions must be 2..8"));}
                let displacements=need(value,"displacements")?.as_arr().filter(|a|a.len()==divisions.iter().product::<u32>()as usize).ok_or(Error::Invalid("lattice control count mismatch"))?.iter().map(array).collect::<Result<Vec<_>>>()?;
                Action::Lattice{vertices:vf("vertices")?,bounds:[array(&bounds[0])?,array(&bounds[1])?],divisions,displacements}},
            "shrinkwrap"=>{fields(value,&["op","object","vertices","reference","max_distance","offset"])?;Action::Shrinkwrap{vertices:vf("vertices")?,reference:checked_name(text(value,"reference")?,limits)?,max_distance:float(need(value,"max_distance")?)?,offset:float(need(value,"offset")?)?}},
            "decimate"=>{fields(value,&["op","object","target_faces","max_error"])?;Action::Decimate{target_faces:integer(need(value,"target_faces")?)? as usize,max_error:float(need(value,"max_error")?)?}},
            "sculpt"=>{fields(value,&["op","object","vertices","kind","center","normal","radius","strength","max_displacement","masks","symmetry"])?;
                let kind=match text(value,"kind")?{"inflate"=>SculptKind::Inflate,"flatten"=>SculptKind::Flatten,"crease"=>SculptKind::Crease,_=>return Err(Error::Invalid("sculpt kind"))};
                let masks=need(value,"masks")?.as_arr().ok_or(Error::Invalid("sculpt masks must be an array"))?;if masks.len()>limits.mesh.max_vertices{return Err(Error::Budget("sculpt masks"));}
                let masks=masks.iter().map(|m|{fields(m,&["vertex","value"])?;Ok((VertexId(stable_id(need(m,"vertex")?)?),float(need(m,"value")?)?))}).collect::<Result<Vec<_>>>()?;
                let symmetry=integer(need(value,"symmetry")?)?;if symmetry>7{return Err(Error::Invalid("symmetry bits must be 0..7"));}
                Action::Sculpt{vertices:vf("vertices")?,brush:SculptBrush{kind,center:array(need(value,"center")?)?,normal:array(need(value,"normal")?)?,radius:float(need(value,"radius")?)?,strength:float(need(value,"strength")?)?,max_displacement:float(need(value,"max_displacement")?)?,masks,symmetry:symmetry as u8}}},
            "uv_box"=>{fields(value,&["op","object","faces","scale","offset"])?;Action::BoxUv{faces:ff()?,scale:array(need(value,"scale")?)?,offset:array(need(value,"offset")?)?}},
            "uv_cylindrical"=>{fields(value,&["op","object","faces","axis","scale","offset"])?;Action::CylinderUv{faces:ff()?,axis:integer(need(value,"axis")?)? as usize,scale:array(need(value,"scale")?)?,offset:array(need(value,"offset")?)?}},
            "uv_unwrap"=>{fields(value,&["op","object","faces"])?;Action::Unwrap{faces:ff()?}},
            "uv_pack"=>{fields(value,&["op","object","faces","padding"])?;Action::Pack{faces:ff()?,padding:float(need(value,"padding")?)?}},
            "uv_relax"=>{fields(value,&["op","object","faces","iterations","factor"])?;Action::Relax{faces:ff()?,iterations:integer(need(value,"iterations")?)?,factor:float(need(value,"factor")?)?}},
            "uv_pin"=>{fields(value,&["op","object","corners","pinned"])?;Action::Pin{corners:selections(need(value,"corners")?,limits.mesh.max_corners)?.into_iter().map(CornerId).collect(),pinned:boolean(need(value,"pinned")?)?}},
            "normals"=>{fields(value,&["op","object","smooth","angle"])?;Action::Normals{smooth:boolean(need(value,"smooth")?)?,angle:float(need(value,"angle")?)?}},
            _=>return Ok(None),
        };
        Ok(Some(Self{object:checked_name(text(value,"object")?,limits)?,action}))
    }
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
    pub fn memory_bytes(&self)->usize{self.value().to_json().len().saturating_mul(4).saturating_add(256)}
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
        };let mut values=vec![("op",json::s(op)),("object",json::s(&self.object))];values.append(&mut args);json::obj(values)
    }

    pub fn apply(&self,objects:&mut BTreeMap<String,Mesh>,limits:&Limits,ctx:&mut Context<'_>)->Result<OperationResult>{
        self.apply_in_scene(objects,&crate::SceneState::default(),limits,ctx)
    }
    pub(crate) fn apply_in_scene(&self,objects:&mut BTreeMap<String,Mesh>,scene:&crate::SceneState,_limits:&Limits,ctx:&mut Context<'_>)->Result<OperationResult>{
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
        };
        if result.faces.is_empty(){result.faces=changes.created.iter().filter_map(|id|if let ElementId::Face(f)=id{Some(*f)}else{None}).collect();}
        if result.vertices.is_empty(){result.vertices=changes.created.iter().filter_map(|id|if let ElementId::Vertex(v)=id{Some(*v)}else{None}).collect();}
        Ok(result)
    }
}
fn checked_name(name:&str,limits:&Limits)->Result<String>{if name.is_empty()||name.len()>limits.max_name_bytes||name.chars().any(char::is_control){Err(Error::Invalid("mesh object name"))}else{Ok(name.into())}}
fn boolean(v:&Value)->Result<bool>{v.as_bool().ok_or(Error::Invalid("expected boolean"))}
fn edge(v:&Value)->Result<EdgeKey>{let a=v.as_arr().filter(|a|a.len()==2).ok_or(Error::Invalid("edge needs two ID strings"))?;let a=VertexId(stable_id(&a[0])?);let b=VertexId(stable_id(&v.as_arr().unwrap()[1])?);if a>=b{return Err(Error::Invalid("edge endpoint IDs must be strictly increasing"));}Ok(EdgeKey(a,b))}
fn edge_list(v:&Value,max:usize)->Result<Vec<EdgeKey>>{let values=v.as_arr().ok_or(Error::Invalid("expected edge array"))?;if values.len()>max{return Err(Error::Budget("edge selection"));}values.iter().map(edge).collect()}
fn vector(v:&[f64])->Value{Value::Arr(v.iter().copied().map(Value::F64).collect())}
fn vertex_ids(v:&[VertexId])->Value{Value::Arr(v.iter().map(|v|json::s(v.0.to_string())).collect())}
fn face_ids(v:&[FaceId])->Value{Value::Arr(v.iter().map(|v|json::s(v.0.to_string())).collect())}
fn edge_value(e:EdgeKey)->Value{vertex_ids(&[e.0,e.1])}
fn edge_values(e:&[EdgeKey])->Value{Value::Arr(e.iter().map(|&e|edge_value(e)).collect())}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(s:&str)->MeshEditingOperation{MeshEditingOperation::parse(&json::parse(s.as_bytes()).unwrap(),&Limits::default()).unwrap().unwrap()}
    #[test]
    fn strict_canonical_roundtrip_and_local_failure_atomicity(){
        let limits=Limits::default();let mut ctx=Context::default();let mesh=Mesh::cube([2.;3],&mut ctx).unwrap();let edge=*mesh.adjacency(&mut ctx).unwrap().edges.keys().next().unwrap();
        let value=json::obj(vec![("op",json::s("bevel_edges")),("object",json::s("body")),("edges",edge_values(&[edge])),("width",Value::F64(0.2))]);
        let op=MeshEditingOperation::parse(&value,&limits).unwrap().unwrap();assert_eq!(MeshEditingOperation::parse(&op.value(),&limits).unwrap().unwrap(),op);
        let mut objects=BTreeMap::from([("body".into(),mesh)]);op.apply(&mut objects,&limits,&mut ctx).unwrap();assert_eq!(objects["body"].faces().len(),7);
        let before=objects.clone();let bad=parse(r#"{"op":"solidify","object":"body","thickness":-1}"#);assert!(bad.apply(&mut objects,&limits,&mut ctx).is_err());assert_eq!(objects,before);
        assert!(MeshEditingOperation::parse(&json::parse(br#"{"op":"uv_pin","object":"body","corners":[],"pinned":true,"extra":0}"#).unwrap(),&limits).is_err());
        assert!(MeshEditingOperation::parse(&json::parse(br#"{"op":"split_edges","object":"body","edges":[["2","1"]],"factor":0.5}"#).unwrap(),&limits).is_err());
    }
    #[test]
    fn decimation_metrics_are_explicit_when_seams_block_reduction(){
        let mut ctx=Context::default();let mut objects=BTreeMap::from([("body".into(),Mesh::cube([2.;3],&mut ctx).unwrap())]);let op=parse(r#"{"op":"decimate","object":"body","target_faces":4,"max_error":10}"#);
        let result=op.apply(&mut objects,&Limits::default(),&mut ctx).unwrap();assert_eq!(result.metrics["achieved_faces"],12);assert_eq!(result.metrics["collapses"],0);
    }
}
