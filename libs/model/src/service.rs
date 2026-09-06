//! Strict AI-facing commands. The host owns networking and worker scheduling;
//! this registry has no locks, threads, UI handles or ambient global state.
use crate::{json::{self, Value}, mesh, Document, Error, Head, Limits, Material, Operation, Result, Transaction};
use std::collections::BTreeMap;

pub const MAX_OPEN_DOCUMENTS: usize = 4;
pub const MAX_INSPECTION_PAGE: usize = 128;
/// Leaves room for the chat ToolOutcome wrapper inside its 16 KiB envelope.
pub const MAX_REPLY_BYTES:usize = 12 * 1024;

pub struct Engine { limits: Limits, documents: BTreeMap<String, Document> }
impl Engine {
    pub fn new(limits: Limits) -> Self { Self { limits, documents: BTreeMap::new() } }
    pub fn document(&self, name: &str) -> Result<&Document> {
        self.documents.get(name).ok_or_else(|| Error::MissingObject(name.into()))
    }
    /// Worker-owned binary resources enter through typed transactions, never
    /// through base64 or pixel arrays in the language model's tool context.
    pub fn apply_transaction(&mut self, document: &str, transaction: Transaction, cancelled: Option<&dyn Fn()->bool>) -> Result<Value> {
        let doc = self.documents.get_mut(document).ok_or_else(||Error::MissingObject(document.into()))?;
        let result = doc.apply(transaction, cancelled)?;
        Ok(apply_reply(document, &result))
    }
    pub fn open(&mut self, name: &str, source: Option<&[u8]>, cancelled: Option<&dyn Fn()->bool>) -> Result<Value> {
        self.open_with_joint_budget(name,source,None,cancelled)
    }
    /// Explicit per-document opt-in for larger bounded affine skin palettes.
    /// Reopening an already-open name never changes that document's limits.
    pub fn open_with_joint_budget(&mut self, name:&str, source:Option<&[u8]>, budget:Option<usize>, cancelled:Option<&dyn Fn()->bool>) -> Result<Value> {
        if budget.is_some_and(|budget|!(1..=128).contains(&budget)){return Err(Error::Invalid("max_joints must be within 1..128"));}
        if name.is_empty() || name.len()>self.limits.max_name_bytes || name.chars().any(char::is_control) { return Err(Error::Invalid("document name")); }
        // Every successful reply must retain this exact document identifier.
        if json::s(name).to_json().len()>MAX_REPLY_BYTES/8 {return Err(Error::Budget("document name in reply"));}
        if cancelled.is_some_and(|f| f()) { return Err(mesh::MeshError::Cancelled.into()); }
        let inserted = !self.documents.contains_key(name);
        if !inserted {
            if source.is_some() { return Err(Error::DuplicateObject(name.into())); }
            if budget.is_some_and(|budget|budget!=self.documents[name].limits().max_joints){return Err(Error::Invalid("max_joints cannot change for an open document; close and reopen its source"));}
        } else {
            if self.documents.len()>=MAX_OPEN_DOCUMENTS { return Err(Error::Budget("open documents")); }
            let mut limits=self.limits.clone();if let Some(budget)=budget{limits.max_joints=budget;}
            let document = match source { Some(bytes)=>Document::from_bytes(bytes,limits,cancelled)?, None=>Document::new(limits)? };
            self.documents.insert(name.into(),document);
        }
        let result=self.inspect(name,None,"overview",0,32,cancelled);
        if result.is_err() && inserted { self.documents.remove(name); }
        result
    }
    pub fn execute(&mut self, name: &str, args: &Value, cancelled: Option<&dyn Fn()->bool>) -> Result<Value> {
        let document = text(args,"document")?;
        if cancelled.is_some_and(|f| f()) { return Err(mesh::MeshError::Cancelled.into()); }
        match name {
            "model.apply" => {
                fields(args,&["document","request_id","expected","operations","result_mode"])?;
                let summary = match args.get("result_mode").and_then(Value::as_str) {
                    None if args.get("result_mode").is_none() => false,
                    Some("selections") => false, Some("summary") => true,
                    _ => return Err(Error::Invalid("result_mode must be summary or selections")),
                };
                let expected = parse_head(need(args,"expected")?)?;
                let operations = parse_operations_inner(need(args,"operations")?,self.document(document)?.limits(),cancelled)?;
                let request_id = text(args,"request_id")?.to_owned();
                let doc = self.documents.get_mut(document).ok_or_else(||Error::MissingObject(document.into()))?;
                let result = doc.apply(Transaction {request_id,expected,operations},cancelled)?;
                // Once committed, response construction cannot turn success into
                // an error. The bounded base always retains both heads and replay.
                Ok(if summary { json::obj(vec![("document",json::s(document)),("head",head_json(result.current)),
                    ("committed",head_json(result.committed)),("replayed",Value::Bool(result.replayed)),("operations",num(result.results.len()))])
                } else {apply_reply(document,&result)})
            }
            "model.inspect" => {
                fields(args,&["document","object","domain","offset","limit"])?;
                let object = args.get("object").map(|v|v.as_str().ok_or(Error::Invalid("object must be a string"))).transpose()?;
                let domain = args.get("domain").map(|v|v.as_str().ok_or(Error::Invalid("inspection domain"))).transpose()?.unwrap_or("overview");
                let offset = args.get("offset").map(integer).transpose()?.unwrap_or(0) as usize;
                let limit = args.get("limit").map(integer).transpose()?.unwrap_or(32) as usize;
                if !(1..=MAX_INSPECTION_PAGE).contains(&limit) { return Err(Error::Invalid("inspection limit must be 1..128")); }
                self.inspect(document,object,domain,offset,limit,cancelled)
            }
            "model.history" => {
                fields(args,&["document","expected","action"])?;
                let expected=parse_head(need(args,"expected")?)?;
                let doc=self.documents.get_mut(document).ok_or_else(||Error::MissingObject(document.into()))?;
                let head=match text(args,"action")? {
                    "undo"=>doc.undo(expected,cancelled)?, "redo"=>doc.redo(expected,cancelled)?,
                    "checkpoint"=> { doc.checkpoint(expected)?; doc.head() },
                    _=>return Err(Error::Invalid("history action must be undo, redo or checkpoint")),
                };
                let (cursor,total)=doc.history_position();
                Ok(json::obj(vec![("head",head_json(head)),("cursor",num(cursor)),("total",num(total))]))
            }
            "model.close" => {
                fields(args,&["document","expected"])?;
                let expected=parse_head(need(args,"expected")?)?; let actual=self.document(document)?.head();
                if expected!=actual { return Err(Error::StaleHead {expected,actual}); }
                self.documents.remove(document);
                Ok(json::obj(vec![("closed",json::s(document))]))
            }
            _=>Err(Error::Invalid("unknown editable model tool")),
        }
    }
    fn inspect(&self, document:&str, object:Option<&str>, domain:&str, offset:usize, limit:usize,
        cancelled:Option<&dyn Fn()->bool>)->Result<Value> {
        let doc=self.document(document)?;
        let mut ctx=mesh::Context::new(self.limits.mesh.clone(),cancelled);
        let mut response=json::obj(vec![("document",json::s(document)),("head",head_json(doc.head())),("domain",json::s(domain))]);
        if let Some((total,rows))=extended::inspect(doc,object,domain,offset,limit,&mut ctx)? {
            if let Some(object)=object { put(&mut response,"object",json::s(object)); }
            return page(response,total,offset,limit,rows,&mut ctx);
        }
        let Some(object)=object else {
            let totals=inventory_totals(doc);
            let rows=match domain {
                "overview"=>return document_overview(response,doc,totals,&mut ctx),
                "objects"=>doc.objects().skip(offset).take(limit).map(|(name,m)|object_row(name,m)).collect(),
                "materials"=>doc.materials().iter().skip(offset).take(limit).map(|(&id,m)|material_row(id,m)).collect(),
                "joints"=>doc.skeleton().map(|s|s.joints.iter().enumerate().skip(offset).take(limit).map(|(i,j)|Ok(joint_row(i,j,&doc.rig().local_rest(s,i)?))).collect::<Result<Vec<_>>>()).transpose()?.unwrap_or_default(),
                "clips"=> {
                    let mut key_offset=0usize;let mut rows=Vec::new();
                    for (i,clip) in doc.clips().values().enumerate() {
                        ctx.checkpoint(1)?;
                        let count=clip_key_count(clip);
                        if i>=offset && rows.len()<limit {rows.push(clip_row(clip,key_offset));}
                        key_offset=key_offset.saturating_add(count);
                    }
                    rows
                },
                "clip_keys"=>clip_key_rows(doc,offset,limit,&mut ctx)?,
                _=>return Err(Error::Invalid("mesh inspection requires object")),
            };
            let total=totals.iter().find(|(name,_)|*name==domain).map(|(_,n)|*n).unwrap();
            return page(response,total,offset,limit,rows,&mut ctx);
        };
        let mesh=doc.object(object).ok_or_else(||Error::MissingObject(object.into()))?;
        put(&mut response,"object",json::s(object));
        let (total,items): (usize,Vec<Value>)=match domain {
            "vertices"=> {
                let mut weight_offset=0usize;let mut rows=Vec::new();
                for (i,v) in mesh.vertices().iter().enumerate() {
                    ctx.checkpoint(1)?;
                    if i>=offset && rows.len()<limit {rows.push(vertex_row(v,weight_offset));}
                    weight_offset=weight_offset.saturating_add(v.weights.len());
                    if rows.len()==limit {break;}
                }
                (mesh.vertices().len(),rows)
            },
            "weights"=>weight_rows(mesh,offset,limit,&mut ctx)?,
            "faces"=>(mesh.faces().len(),mesh.faces().iter().skip(offset).take(limit).map(|f|json::obj(vec![
                ("id",id(f.id.0)),("material",Value::Int(f.material as i64)),
                ("corner_offset",Value::Int(f.first_corner as i64)),("corner_count",Value::Int(f.corner_count as i64)),
            ])).collect()),
            "corners"=>(mesh.corners().len(),mesh.corners().iter().skip(offset).take(limit).map(|c|json::obj(vec![
                ("id",id(c.id.0)),("vertex",id(c.vertex.0)),("uv",vector(&c.uv)),
                ("normal",c.normal.as_ref().map(|v|vector(v)).unwrap_or(Value::Null)),
            ])).collect()),
            "edges"=> {
                let adj=mesh.adjacency(&mut ctx)?;
                (adj.edges.len(),adj.edges.iter().skip(offset).take(limit).map(|(e,uses)|json::obj(vec![
                    ("vertices",ids([e.0.0,e.1.0])),("uses",num(uses.len())),
                    ("seam",Value::Bool(mesh.edge_attributes().get(e).is_some_and(|v|v.attributes.seam))),
                    ("crease",Value::F64(mesh.edge_attributes().get(e).map_or(0.0,|v|v.attributes.crease))),
                ])).collect())
            }
            "overview"=> {
                let validation=mesh.validate(&mut ctx)?;
                let mut bounds=[[f64::INFINITY;3],[f64::NEG_INFINITY;3]];
                for vertex in mesh.vertices() {ctx.checkpoint(1)?;for d in 0..3 {
                    bounds[0][d]=bounds[0][d].min(vertex.position[d]);bounds[1][d]=bounds[1][d].max(vertex.position[d]);
                }}
                put(&mut response,"vertices",num(mesh.vertices().len()));put(&mut response,"faces",num(mesh.faces().len()));
                put(&mut response,"corners",num(mesh.corners().len()));
                put(&mut response,"bounds",if mesh.vertices().is_empty(){Value::Null}else{Value::Arr(bounds.iter().map(|v|vector(v)).collect())});
                put(&mut response,"validation",json::obj(vec![
                    ("valid_surface",Value::Bool(validation.is_valid_surface)),("closed",Value::Bool(validation.is_closed)),
                    ("closed_manifold",Value::Bool(validation.is_closed_manifold)),("valid_solid",Value::Bool(validation.is_valid_solid)),
                    ("manifold",Value::Bool(validation.is_surface_manifold)),("oriented",Value::Bool(validation.is_consistently_oriented)),
                    ("self_intersections",json::s("not_checked")),("boundary_edges",num(validation.boundary_edges)),
                    ("non_manifold_edges",num(validation.non_manifold_edges)),("non_manifold_vertices",num(validation.non_manifold_vertices)),
                    ("issue_count",num(validation.issues.len())),("issue_offset",num(offset)),
                    ("issues",Value::Arr(Vec::new())),("issues_returned",num(0)),
                    ("issues_omitted",num(validation.issues.len().saturating_sub(offset))),
                    ("next_issue_offset",next_offset(offset,0,validation.issues.len())),
                ]));
                let mut emitted=0;
                for issue in validation.issues.iter().skip(offset).take(limit) {
                    ctx.checkpoint(1)?;
                    let mut candidate=response.clone();
                    let nested=get_mut(&mut candidate,"validation");
                    array_push(nested,"issues",json::obj(vec![("kind",json::s(format!("{:?}",issue.kind))),("element",element_json(issue.element))]));
                    put(nested,"issues_returned",num(emitted+1));
                    put(nested,"issues_omitted",num(validation.issues.len().saturating_sub(offset).saturating_sub(emitted+1)));
                    put(nested,"next_issue_offset",next_offset(offset,emitted+1,validation.issues.len()));
                    if candidate.to_json().len()>MAX_REPLY_BYTES {break;}
                    response=candidate;emitted+=1;
                }
                if emitted==0 && offset<validation.issues.len() {return Err(Error::Budget("inspection issue exceeds reply bytes"));}
                return checked_reply(response);
            }
            _=>return Err(Error::Invalid("unknown mesh inspection domain")),
        };
        page(response,total,offset,limit,items,&mut ctx)
    }
}

fn put(object:&mut Value,key:&str,value:Value) {
    let Value::Obj(fields)=object else {unreachable!("response object")};
    if let Some((_,old))=fields.iter_mut().find(|(name,_)|name==key) {*old=value;} else {fields.push((key.into(),value));}
}
fn get_mut<'a>(object:&'a mut Value,key:&str)->&'a mut Value {
    let Value::Obj(fields)=object else {unreachable!("response object")};
    &mut fields.iter_mut().find(|(name,_)|name==key).expect("response field").1
}
pub(crate) fn array_push(object:&mut Value,key:&str,value:Value) {
    let Value::Arr(items)=get_mut(object,key) else {unreachable!("response array")};items.push(value);
}
fn checked_reply(response:Value)->Result<Value> {
    if response.to_json().len()>MAX_REPLY_BYTES {Err(Error::Budget("inspection reply bytes"))} else {Ok(response)}
}
fn next_offset(offset:usize,count:usize,total:usize)->Value {
    if offset.saturating_add(count)<total {num(offset+count)} else {Value::Null}
}
fn page(mut base:Value,total:usize,offset:usize,limit:usize,rows:Vec<Value>,ctx:&mut mesh::Context<'_>)->Result<Value> {
    put(&mut base,"total",num(total));put(&mut base,"offset",num(offset));put(&mut base,"limit",num(limit));
    put(&mut base,"count",num(0));put(&mut base,"omitted",num(total.saturating_sub(offset)));
    put(&mut base,"next_offset",next_offset(offset,0,total));put(&mut base,"byte_limited",Value::Bool(false));
    put(&mut base,"items",Value::Arr(Vec::new()));
    let mut emitted=0;
    for row in rows {
        ctx.checkpoint(1)?;
        let mut candidate=base.clone();array_push(&mut candidate,"items",row);
        put(&mut candidate,"count",num(emitted+1));
        put(&mut candidate,"omitted",num(total.saturating_sub(offset).saturating_sub(emitted+1)));
        put(&mut candidate,"next_offset",next_offset(offset,emitted+1,total));
        if candidate.to_json().len()>MAX_REPLY_BYTES {
            // false -> true is one byte shorter; no cap can be crossed here.
            put(&mut base,"byte_limited",Value::Bool(true));break;
        }
        base=candidate;emitted+=1;
    }
    if emitted==0 && offset<total {return Err(Error::Budget("one inspection row exceeds reply bytes"));}
    checked_reply(base)
}

fn object_row(name:&str,mesh:&mesh::Mesh)->Value {
    json::obj(vec![("object",json::s(name)),("vertices",num(mesh.vertices().len())),("faces",num(mesh.faces().len())),("corners",num(mesh.corners().len()))])
}
fn material_row(id:u32,material:&Material)->Value {
    json::obj(vec![("id",Value::Int(id as i64)),("color",vector(&material.color)),("texture_bytes",num(material.base_color_png.len()))])
}
fn joint_row(index:usize,joint:&crate::Joint,rest:&crate::transform::Transform)->Value {
    json::obj(vec![("joint",num(index)),("name",json::s(&joint.name)),("parent",joint.parent.map(|p|Value::Int(p as i64)).unwrap_or(Value::Null)),("translation",vector(&rest.translation)),("rotation",vector(&rest.rotation)),("scale",vector(&rest.scale))])
}
fn clip_key_count(clip:&crate::AnimationClip)->usize {clip.channels.iter().map(|c|c.keys.len()).sum()}
fn clip_row(clip:&crate::AnimationClip,key_offset:usize)->Value {
    json::obj(vec![("name",json::s(&clip.name)),("channels",num(clip.channels.len())),("keyframes",num(clip_key_count(clip))),
        ("key_offset",num(key_offset)),("duration",Value::F64(clip.duration()))])
}
fn inventory_totals(doc:&Document)->[(&'static str,usize);5] {
    [("objects",doc.objects().count()),("materials",doc.materials().len()),("joints",doc.skeleton().map_or(0,|s|s.joints.len())),
        ("clips",doc.clips().len()),("clip_keys",doc.clips().values().map(clip_key_count).sum())]
}
fn document_overview(mut response:Value,doc:&Document,totals:[(&'static str,usize);5],ctx:&mut mesh::Context<'_>)->Result<Value> {
    put(&mut response,"max_joints",num(doc.limits().max_joints));
    // Give the author the complete measured size in the first response. Local
    // object bounds alone otherwise invite repeated inventory/SQL lookups.
    let mut bounds=[[f64::INFINITY;3],[f64::NEG_INFINITY;3]];
    for (name,mesh) in doc.scene().evaluated_meshes(&doc.state,ctx)? {
        let source=crate::morph_export::source(doc,&name)?;
        let frame=doc.scene().world_matrix(&name)?;
        for vertex in mesh.vertices() {
            ctx.checkpoint(1)?;
            let mut p=vertex.position;
            for morph in doc.rig().morphs.values().filter(|m|m.object==source) {
                if let Some(delta)=morph.deltas.get(&vertex.id) {p=crate::transform::add(p,crate::transform::mul(*delta,morph.weight));}
            }
            let p=crate::transform::transform_point(frame,p);
            for d in 0..3 {bounds[0][d]=bounds[0][d].min(p[d]);bounds[1][d]=bounds[1][d].max(p[d]);}
        }
    }
    let has_bounds=bounds[0][0].is_finite();
    put(&mut response,"bounds",if has_bounds{Value::Arr(bounds.iter().map(|v|vector(v)).collect())}else{Value::Null});
    put(&mut response,"dimensions",if has_bounds{vector(&crate::transform::sub(bounds[1],bounds[0]))}else{Value::Null});
    put(&mut response,"bounds_space",json::s("model metres, evaluated rest geometry including object transforms"));
    put(&mut response,"inspection_domains",Value::Arr(["overview","objects","materials","joints","clips","clip_keys","vertices","weights","faces","corners","edges"].into_iter().chain(EXTENDED_INSPECTION_DOMAINS.iter().copied()).map(json::s).collect()));
    put(&mut response,"counts",json::obj(totals.iter().map(|(name,n)|(*name,num(*n))).collect()));
    let (cursor,total)=doc.history_position();put(&mut response,"history",json::obj(vec![("cursor",num(cursor)),("total",num(total))]));
    put(&mut response,"inventories",json::obj(totals.iter().map(|(name,n)|(*name,json::obj(vec![
        ("total",num(*n)),("count",num(0)),("omitted",num(*n)),("next_offset",next_offset(0,0,*n)),
    ]))).collect()));
    for field in ["objects","materials","skeleton","clips"] {put(&mut response,field,Value::Arr(Vec::new()));}
    if doc.skeleton().is_none() {put(&mut response,"skeleton",Value::Null);}
    let object_rows=doc.objects().take(4).map(|(name,m)|object_row(name,m)).collect();
    preview(&mut response,"objects","objects",totals[0].1,object_rows,ctx)?;
    preview(&mut response,"materials","materials",totals[1].1,doc.materials().iter().take(4).map(|(&id,m)|material_row(id,m)).collect(),ctx)?;
    if let Some(skeleton)=doc.skeleton() {
        preview(&mut response,"skeleton","joints",totals[2].1,skeleton.joints.iter().enumerate().take(4).map(|(i,j)|Ok(joint_row(i,j,&doc.rig().local_rest(skeleton,i)?))).collect::<Result<Vec<_>>>()?,ctx)?;
    }
    let mut key_offset=0;let clips=doc.clips().values().take(4).map(|c|{let row=clip_row(c,key_offset);key_offset+=clip_key_count(c);row}).collect();
    preview(&mut response,"clips","clips",totals[3].1,clips,ctx)?;
    checked_reply(response)
}
fn preview(response:&mut Value,field:&str,domain:&str,total:usize,rows:Vec<Value>,ctx:&mut mesh::Context<'_>)->Result<()> {
    let mut count=0;
    for row in rows {
        ctx.checkpoint(1)?;
        let mut candidate=response.clone();array_push(&mut candidate,field,row);
        let inventory=get_mut(get_mut(&mut candidate,"inventories"),domain);
        put(inventory,"count",num(count+1));put(inventory,"omitted",num(total-count-1));put(inventory,"next_offset",next_offset(0,count+1,total));
        if candidate.to_json().len()>MAX_REPLY_BYTES {break;}
        *response=candidate;count+=1;
    }
    Ok(())
}
fn vertex_row(vertex:&mesh::Vertex,weight_offset:usize)->Value {
    let mut row=json::obj(vec![("id",id(vertex.id.0)),("position",vector(&vertex.position)),("weights",Value::Arr(Vec::new())),
        ("weight_count",num(vertex.weights.len())),("weights_omitted",num(vertex.weights.len())),("weight_offset",num(weight_offset))]);
    for (i,weight) in vertex.weights.iter().enumerate() {
        let mut candidate=row.clone();array_push(&mut candidate,"weights",json::obj(vec![("joint",Value::Int(weight.joint as i64)),("weight",Value::F64(weight.weight))]));
        put(&mut candidate,"weights_omitted",num(vertex.weights.len()-i-1));
        if candidate.to_json().len()>MAX_REPLY_BYTES/2 {break;}
        row=candidate;
    }
    row
}
fn weight_rows(mesh:&mesh::Mesh,offset:usize,limit:usize,ctx:&mut mesh::Context<'_>)->Result<(usize,Vec<Value>)> {
    let mut total=0usize;let mut rows=Vec::new();
    for vertex in mesh.vertices() {
        ctx.checkpoint(1)?;
        for (index,weight) in vertex.weights.iter().enumerate().skip(offset.saturating_sub(total)) {
            if rows.len()==limit {break;}
            ctx.checkpoint(1)?;
            rows.push(json::obj(vec![("vertex",id(vertex.id.0)),("weight_index",num(index)),("joint",Value::Int(weight.joint as i64)),("weight",Value::F64(weight.weight))]));
        }
        total=total.saturating_add(vertex.weights.len());
    }
    Ok((total,rows))
}
fn clip_key_rows(doc:&Document,offset:usize,limit:usize,ctx:&mut mesh::Context<'_>)->Result<Vec<Value>> {
    let mut base=0usize;let mut rows=Vec::new();
    for clip in doc.clips().values() {
        for (channel_index,channel) in clip.channels.iter().enumerate() {
            ctx.checkpoint(1)?;
            for (key_index,key) in channel.keys.iter().enumerate().skip(offset.saturating_sub(base)) {
                if rows.len()==limit {break;}
                ctx.checkpoint(1)?;
                rows.push(json::obj(vec![("clip",json::s(&clip.name)),("channel",num(channel_index)),("joint",num(channel.joint as usize)),
                    ("path",json::s(match channel.path {crate::AnimationPath::Translation=>"translation",crate::AnimationPath::Rotation=>"rotation",crate::AnimationPath::Scale=>"scale"})),
                    ("key",num(key_index)),("time",Value::F64(key.time)),("value",vector(&key.value))]));
            }
            base=base.saturating_add(channel.keys.len());
        }
    }
    Ok(rows)
}
fn apply_reply(document:&str,result:&crate::Applied)->Value {
    let face_total=result.results.iter().fold(0usize,|n,r|n.saturating_add(r.faces.len()));
    let vertex_total=result.results.iter().fold(0usize,|n,r|n.saturating_add(r.vertices.len()));
    let mut reply=json::obj(vec![("document",json::s(document)),("head",head_json(result.current)),
        ("committed",head_json(result.committed)),("replayed",Value::Bool(result.replayed)),
        ("results_total",num(result.results.len())),("results_returned",num(0)),("results_omitted",num(result.results.len())),
        ("results_truncated",Value::Bool(!result.results.is_empty())),("selection_face_total",num(face_total)),
        ("selection_vertex_total",num(vertex_total)),("selection_ids_returned",num(0)),
        ("selection_ids_omitted",num(face_total.saturating_add(vertex_total))),
        ("selection_lookup",json::s("inspect faces/vertices for surviving elements; results refer to committed head")),("results",Value::Arr(Vec::new()))]);
    let mut selection_budget=256usize;let mut emitted=0usize;let mut ids_returned=0usize;
    for (index,r) in result.results.iter().enumerate() {
        let nf=r.faces.len().min(selection_budget);let nv=r.vertices.len().min(selection_budget-nf);
        let row=json::obj(vec![("result_index",num(index)),("object",json::s(&r.object)),
            ("metrics",Value::Obj(r.metrics.iter().map(|(name,value)|(name.clone(),num(*value))).collect())),
            ("faces",ids(r.faces.iter().take(nf).map(|id|id.0))),("vertices",ids(r.vertices.iter().take(nv).map(|id|id.0))),
            ("face_count",num(r.faces.len())),("vertex_count",num(r.vertices.len())),
            ("selections_truncated",Value::Bool(nf<r.faces.len()||nv<r.vertices.len()))]);
        let mut candidate=reply.clone();array_push(&mut candidate,"results",row);
        put(&mut candidate,"results_returned",num(emitted+1));put(&mut candidate,"results_omitted",num(result.results.len()-emitted-1));
        put(&mut candidate,"results_truncated",Value::Bool(emitted+1<result.results.len()));
        put(&mut candidate,"selection_ids_returned",num(ids_returned+nf+nv));
        put(&mut candidate,"selection_ids_omitted",num(face_total.saturating_add(vertex_total).saturating_sub(ids_returned+nf+nv)));
        if candidate.to_json().len()>MAX_REPLY_BYTES {break;}
        reply=candidate;emitted+=1;ids_returned+=nf+nv;selection_budget-=nf+nv;
    }
    debug_assert!(reply.to_json().len()<=MAX_REPLY_BYTES);
    reply
}
pub fn head_json(head:Head)->Value {
    json::obj(vec![("generation",id(head.generation)),("content",json::s(head.content.iter().map(|b|format!("{b:02x}")).collect::<String>()))])
}
pub fn parse_head(value:&Value)->Result<Head> {
    fields(value,&["generation","content"])?;
    let generation=stable_id(need(value,"generation")?)?;
    let hex=text(value,"content")?;
    if hex.len()!=64 || !hex.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)) { return Err(Error::Invalid("head content must be 64 lowercase hex digits")); }
    let mut content=[0;32];
    for (i,byte) in content.iter_mut().enumerate() { *byte=u8::from_str_radix(&hex[i*2..i*2+2],16).map_err(|_|Error::Invalid("content hash"))?; }
    Ok(Head {generation,content})
}
fn id(v:u64)->Value { json::s(v.to_string()) }
fn element_json(element:mesh::ElementId)->Value {
    let (kind,value)=match element {
        mesh::ElementId::Vertex(v)=>("vertex",id(v.0)),mesh::ElementId::Face(f)=>("face",id(f.0)),
        mesh::ElementId::Corner(c)=>("corner",id(c.0)),mesh::ElementId::Edge(e)=>("edge",ids([e.0.0,e.1.0])),
    };
    json::obj(vec![("kind",json::s(kind)),("id",value)])
}
fn ids(v:impl IntoIterator<Item=u64>)->Value { Value::Arr(v.into_iter().map(id).collect()) }
fn num(v:usize)->Value { Value::Int(v.min(i64::MAX as usize) as i64) }
fn vector(v:&[f64])->Value { Value::Arr(v.iter().copied().map(Value::F64).collect()) }
pub(crate) fn fields(value:&Value,allowed:&[&str])->Result<()> {
    let Value::Obj(pairs)=value else { return Err(Error::Invalid("expected object")); };
    for (i,(key,_)) in pairs.iter().enumerate() {
        if !allowed.contains(&key.as_str()) || pairs[..i].iter().any(|(k,_)|k==key) { return Err(Error::Invalid("unknown or duplicate field")); }
    }
    Ok(())
}
pub(crate) fn need<'a>(v:&'a Value,key:&str)->Result<&'a Value> { v.get(key).ok_or(Error::Invalid("required field missing")) }
pub(crate) fn text<'a>(v:&'a Value,key:&str)->Result<&'a str> { need(v,key)?.as_str().ok_or(Error::Invalid("expected string")) }
pub(crate) fn integer(v:&Value)->Result<u32> { v.as_u64().and_then(|v|v.try_into().ok()).ok_or(Error::Invalid("expected u32 integer")) }
pub(crate) fn stable_id(v:&Value)->Result<u64> {
    let s=v.as_str().ok_or(Error::Invalid("stable IDs and generations must be decimal strings"))?;
    if s.is_empty() || (s.len()>1&&s.starts_with('0')) || !s.bytes().all(|v|v.is_ascii_digit()) { return Err(Error::Invalid("noncanonical stable ID")); }
    s.parse().map_err(|_|Error::Invalid("stable ID overflow"))
}
pub(crate) fn float(v:&Value)->Result<f64> {
    match v { Value::F64(v) if v.is_finite()=>Ok(*v),Value::Int(v)=>Ok(*v as f64),_=>Err(Error::Invalid("expected finite number")) }
}
pub(crate) fn array<const N:usize>(v:&Value)->Result<[f64;N]> {
    let values=v.as_arr().ok_or(Error::Invalid("expected numeric vector"))?;
    if values.len()!=N { return Err(Error::Invalid("vector length")); }
    let mut out=[0.0;N]; for (o,v) in out.iter_mut().zip(values) { *o=float(v)?; } Ok(out)
}
pub(crate) fn selections(v:&Value,max:usize)->Result<Vec<u64>> {
    let values=v.as_arr().ok_or(Error::Invalid("expected ID array"))?;
    if values.len()>max { return Err(Error::Budget("selection size")); }
    values.iter().map(stable_id).collect()
}

pub fn parse_operations(value:&Value,limits:&Limits)->Result<Vec<Operation>> {
    parse_operations_inner(value,limits,None)
}
fn parse_operations_inner(value:&Value,limits:&Limits,cancelled:Option<&dyn Fn()->bool>)->Result<Vec<Operation>> {
    let values=value.as_arr().ok_or(Error::Invalid("operations must be an array"))?;
    if values.is_empty() || values.len()>limits.max_operations { return Err(Error::Budget("operation count")); }
    let mut budget=limits.max_transaction_bytes;
    value_cost(value,&mut budget,0)?;
    let mut generated=0usize; let mut out=Vec::new();
    for value in values {
        if cancelled.is_some_and(|f|f()) { return Err(mesh::MeshError::Cancelled.into()); }
        let op=parse_operation(value,limits)?;
        if let Operation::ImportMesh {source,..}=&op {
            generated=generated.saturating_add(source.len());
            if generated>limits.max_transaction_bytes { return Err(Error::Budget("generated transaction bytes")); }
        }
        out.push(op);
    }
    Ok(out)
}
fn value_cost(value:&Value,remaining:&mut usize,depth:usize)->Result<()> {
    if depth>16 { return Err(Error::Invalid("operation nesting too deep")); }
    let size=match value {Value::Str(s)=>s.len(),Value::Obj(v)=>v.iter().map(|(k,_)|k.len()).sum(),_=>16};
    if size>*remaining { return Err(Error::Budget("transaction input bytes")); } *remaining-=size;
    match value {
        Value::Arr(values)=>for v in values {value_cost(v,remaining,depth+1)?;},
        Value::Obj(values)=>for (_,v) in values {value_cost(v,remaining,depth+1)?;},_=>{}
    }
    Ok(())
}
fn parse_operation(v:&Value,limits:&Limits)->Result<Operation> {
    if text(v,"op")? == "soft_body_unbind" {
        fields(v,&["op"])?;
        return Ok(Operation::SoftBodyUnbind);
    }
    if let Some(op)=crate::SoftBodyBind::parse(v,limits)? {return Ok(Operation::SoftBody(op));}
    if let Some(op)=crate::SelectionOperation::parse(v,limits)? {return Ok(Operation::Selection(op));}
    if let Some(op)=crate::ConstructionOperation::parse(v,limits)? {return Ok(Operation::Construction(op));}
    if let Some(op)=crate::RigOperation::parse(v,limits)? {return Ok(Operation::Rig(op));}
    if let Some(op)=crate::MeshEditingOperation::parse(v,limits)? {return Ok(Operation::MeshEditing(op));}
    if let Some(op)=crate::SurfaceOperation::parse(v,limits)? {return Ok(Operation::Surface(op));}
    if let Some(op)=crate::SceneOperation::parse(v,limits)? {return Ok(Operation::Scene(op));}
    let op=text(v,"op")?;
    match op {
        "texture_solid"=> {
            fields(v,&["op","material","width","height","color"])?;
            return Ok(Operation::TextureSolid {material:integer(need(v,"material")?)?,width:integer(need(v,"width")?)?,height:integer(need(v,"height")?)?,color:rgb(need(v,"color")?)?});
        }
        "paint_texture"=> {
            fields(v,&["op","material","center","radius","color"])?;
            return Ok(Operation::PaintTexture {material:integer(need(v,"material")?)?,center:array(need(v,"center")?)?,radius:float(need(v,"radius")?)?,color:rgb(need(v,"color")?)?});
        }
        "skeleton"=> {
            fields(v,&["op","joints"])?;
            let joints=need(v,"joints")?.as_arr().ok_or(Error::Invalid("joints must be an array"))?;
            if joints.len()>limits.max_joints { return Err(Error::Budget("joints")); }
            let joints=joints.iter().map(|j| {
                fields(j,&["name","parent","translation"])?;
                let name=text(j,"name")?.to_owned();
                if name.len()>limits.max_name_bytes { return Err(Error::Budget("joint name")); }
                let parent=match need(j,"parent")? { Value::Null=>None,v=>Some(integer(v)?) };
                Ok(crate::Joint {name,parent,translation:array(need(j,"translation")?)?})
            }).collect::<Result<Vec<_>>>()?;
            return Ok(Operation::SetSkeleton {skeleton:crate::Skeleton {joints}});
        }
        "clip"=> {
            fields(v,&["op","name","channels"])?;
            let name=text(v,"name")?.to_owned();
            if name.len()>limits.max_name_bytes { return Err(Error::Budget("clip name")); }
            let channels=need(v,"channels")?.as_arr().ok_or(Error::Invalid("clip channels"))?;
            if channels.len()>limits.max_joints.saturating_mul(3) { return Err(Error::Budget("clip channels")); }
            let mut key_count=0usize;
            let channels=channels.iter().map(|c| {
                fields(c,&["joint","path","keys"])?;
                let path=match text(c,"path")? {
                    "translation"=>crate::AnimationPath::Translation,"rotation"=>crate::AnimationPath::Rotation,
                    "scale"=>crate::AnimationPath::Scale,_=>return Err(Error::Invalid("animation path")),
                };
                let keys=need(c,"keys")?.as_arr().ok_or(Error::Invalid("animation keys"))?;
                key_count=key_count.saturating_add(keys.len());
                if key_count>limits.max_keyframes { return Err(Error::Budget("keyframes")); }
                let keys=keys.iter().map(|k| {
                    fields(k,&["time","value"])?;
                    Ok(crate::Keyframe {time:float(need(k,"time")?)?,value:array(need(k,"value")?)?})
                }).collect::<Result<Vec<_>>>()?;
                Ok(crate::AnimationChannel {joint:integer(need(c,"joint")?)?,path,keys})
            }).collect::<Result<Vec<_>>>()?;
            return Ok(Operation::SetClip {clip:crate::AnimationClip {name,channels}});
        }
        "delete_clip"=> { fields(v,&["op","name"])?; return Ok(Operation::DeleteClip {name:text(v,"name")?.to_owned()}); }
        _=>{}
    }
    if op=="material" {
        fields(v,&["op","material","color"])?;
        return Ok(Operation::SetMaterial { material:integer(need(v,"material")?)?,value:Material {color:array(need(v,"color")?)?,base_color_png:Vec::new()} });
    }
    let object=text(v,"object")?.to_owned();
    if object.len()>limits.max_name_bytes { return Err(Error::Budget("object name")); }
    Ok(match op {
        "cube"=> { fields(v,&["op","object","size"])?; Operation::Cube {object,size:array(need(v,"size")?)?} }
        "plane"=> { fields(v,&["op","object","size"])?; Operation::Plane {object,size:array(need(v,"size")?)?} }
        "sphere"=> {
            fields(v,&["op","object","radius","segments","rings","smooth"])?;
            let smooth=v.get("smooth").map(|v|v.as_bool().ok_or(Error::Invalid("smooth must be boolean"))).transpose()?.unwrap_or(true);
            Operation::ImportMesh {object,source:crate::primitives::sphere(float(need(v,"radius")?)?,integer(need(v,"segments")?)?,integer(need(v,"rings")?)?,smooth,limits)?}
        }
        "cylinder"=> {
            fields(v,&["op","object","radius","height","segments","smooth"])?;
            let smooth=v.get("smooth").map(|v|v.as_bool().ok_or(Error::Invalid("smooth must be boolean"))).transpose()?.unwrap_or(true);
            Operation::ImportMesh {object,source:crate::primitives::cylinder(float(need(v,"radius")?)?,float(need(v,"height")?)?,integer(need(v,"segments")?)?,smooth,limits)?}
        }
        "polygon_mesh"=> {
            fields(v,&["op","object","positions","polygons"])?;
            let positions=need(v,"positions")?.as_arr().ok_or(Error::Invalid("positions must be an array"))?;
            let polygons=need(v,"polygons")?.as_arr().ok_or(Error::Invalid("polygons must be an array"))?;
            if positions.len()>limits.mesh.max_vertices || polygons.len()>limits.mesh.max_faces { return Err(Error::Budget("polygon mesh size")); }
            let positions=positions.iter().map(array).collect::<Result<Vec<[f64;3]>>>()?;
            let mut corners=0usize;
            let polygons=polygons.iter().map(|p| {
                fields(p,&["vertices","uvs","material"])?;
                let vertices=need(p,"vertices")?.as_arr().ok_or(Error::Invalid("polygon vertex indices"))?;
                corners=corners.saturating_add(vertices.len());
                if vertices.len()>limits.mesh.max_face_corners || corners>limits.mesh.max_corners { return Err(Error::Budget("polygon corners")); }
                let vertices=vertices.iter().map(integer).collect::<Result<Vec<_>>>()?;
                let uvs=match p.get("uvs") {
                    None=>Vec::new(),Some(v)=> {
                        let uvs=v.as_arr().ok_or(Error::Invalid("polygon UV array"))?;
                        if uvs.len()!=vertices.len() { return Err(Error::Invalid("polygon UV count")); }
                        uvs.iter().map(array).collect::<Result<Vec<[f64;2]>>>()?
                    }
                };
                let material=p.get("material").map(integer).transpose()?.unwrap_or(0);
                Ok(mesh::Polygon {vertices,uvs,material})
            }).collect::<Result<Vec<_>>>()?;
            let mut ctx=mesh::Context::new(limits.mesh.clone(),None);
            Operation::ImportMesh {object,source:mesh::Mesh::from_polygons(&positions,&polygons,&mut ctx)?.to_bytes(&mut ctx)?}
        }
        "delete_object"=> { fields(v,&["op","object"])?; Operation::DeleteObject {object} }
        "transform"=> {
            fields(v,&["op","object","vertices","matrix"])?;
            let vertices=selections(need(v,"vertices")?,limits.mesh.max_vertices)?.into_iter().map(mesh::VertexId).collect();
            let rows=need(v,"matrix")?.as_arr().ok_or(Error::Invalid("matrix must contain four rows"))?;
            if rows.len()!=4 { return Err(Error::Invalid("matrix must contain four rows")); }
            Operation::Transform {object,vertices,matrix:[array(&rows[0])?,array(&rows[1])?,array(&rows[2])?,array(&rows[3])?]}
        }
        "extrude"=> { fields(v,&["op","object","face","offset"])?; Operation::Extrude {object,face:mesh::FaceId(stable_id(need(v,"face")?)?),offset:array(need(v,"offset")?)?} }
        "delete_faces"=> { fields(v,&["op","object","faces"])?; Operation::DeleteFaces {object,faces:selections(need(v,"faces")?,limits.mesh.max_faces)?.into_iter().map(mesh::FaceId).collect()} }
        "mirror"=> {
            fields(v,&["op","object","axis","offset"])?; let axis=integer(need(v,"axis")?)?;
            if axis>2 { return Err(Error::Invalid("mirror axis must be 0, 1 or 2")); }
            Operation::Mirror {object,axis:axis as u8,offset:float(need(v,"offset")?)?}
        }
        "uv"=> { fields(v,&["op","object","corner","uv"])?; Operation::SetUv {object,corner:mesh::CornerId(stable_id(need(v,"corner")?)?),uv:array(need(v,"uv")?)?} }
        "weights"=> {
            fields(v,&["op","object","vertex","weights"])?;
            let weights=need(v,"weights")?.as_arr().ok_or(Error::Invalid("weights must be an array"))?;
            if weights.len()>limits.mesh.max_weights_per_vertex { return Err(Error::Budget("joint influences")); }
            let weights=weights.iter().map(|w| { fields(w,&["joint","weight"])?; Ok(mesh::JointWeight {joint:integer(need(w,"joint")?)?,weight:float(need(w,"weight")?)?}) }).collect::<Result<Vec<_>>>()?;
            Operation::SetWeights {object,vertex:mesh::VertexId(stable_id(need(v,"vertex")?)?),weights}
        }
        "assign_material"=> { fields(v,&["op","object","faces","material"])?; Operation::AssignMaterial {object,faces:selections(need(v,"faces")?,limits.mesh.max_faces)?.into_iter().map(mesh::FaceId).collect(),material:integer(need(v,"material")?)?} }
        "inset"=> { fields(v,&["op","object","face","distance"])?; Operation::Inset {object,face:mesh::FaceId(stable_id(need(v,"face")?)?),distance:float(need(v,"distance")?)?} }
        "weld"=> { fields(v,&["op","object","vertices","distance"])?; Operation::Weld {object,vertices:selections(need(v,"vertices")?,limits.mesh.max_vertices)?.into_iter().map(mesh::VertexId).collect(),distance:float(need(v,"distance")?)?} }
        "edge_attributes"=> {
            fields(v,&["op","object","vertices","seam","crease"])?;
            let vertices=selections(need(v,"vertices")?,2)?;
            if vertices.len()!=2 { return Err(Error::Invalid("edge needs two vertex IDs")); }
            Operation::EdgeAttributes {object,edge:mesh::EdgeKey::new(mesh::VertexId(vertices[0]),mesh::VertexId(vertices[1])),attributes:mesh::EdgeAttributes {
                seam:need(v,"seam")?.as_bool().ok_or(Error::Invalid("seam must be boolean"))?,crease:float(need(v,"crease")?)?,
            }}
        }
        "corner_normal"=> {
            fields(v,&["op","object","corner","normal"])?;
            let normal=match need(v,"normal")? {Value::Null=>None,v=>Some(array(v)?)};
            Operation::CornerNormal {object,corner:mesh::CornerId(stable_id(need(v,"corner")?)?),normal}
        }
        "auto_weights"=> {fields(v,&["op","object"])?; Operation::AutoWeights {object} }
        "smooth"=> {
            fields(v,&["op","object","vertices","iterations","factor","preserve_boundary"])?;
            Operation::Smooth {object,vertices:selections(need(v,"vertices")?,limits.mesh.max_vertices)?.into_iter().map(mesh::VertexId).collect(),
                iterations:integer(need(v,"iterations")?)?,factor:float(need(v,"factor")?)?,
                preserve_boundary:need(v,"preserve_boundary")?.as_bool().ok_or(Error::Invalid("preserve_boundary must be boolean"))?}
        }
        "subdivide"=> {
            fields(v,&["op","object","levels"])?;
            Operation::Subdivide {object,levels:integer(need(v,"levels")?)?}
        }
        "project_uv"=> {
            fields(v,&["op","object","faces","axis","scale","offset"])?;
            let axis=integer(need(v,"axis")?)?;
            if axis>2 { return Err(Error::Invalid("projection axis must be 0, 1 or 2")); }
            Operation::ProjectUv {object,faces:selections(need(v,"faces")?,limits.mesh.max_faces)?.into_iter().map(mesh::FaceId).collect(),
                axis:axis as u8,scale:array(need(v,"scale")?)?,offset:array(need(v,"offset")?)?}
        }
        "brush"=> {
            fields(v,&["op","object","vertices","center","radius","delta","max_displacement"])?;
            Operation::Brush {object,vertices:selections(need(v,"vertices")?,limits.mesh.max_vertices)?.into_iter().map(mesh::VertexId).collect(),
                center:array(need(v,"center")?)?,radius:float(need(v,"radius")?)?,delta:array(need(v,"delta")?)?,
                max_displacement:float(need(v,"max_displacement")?)?}
        }
        _=>return Err(Error::Invalid("unknown modeling operation")),
    })
}
pub(crate) fn rgb(v:&Value)->Result<[u8;3]> {
    let values=v.as_arr().ok_or(Error::Invalid("RGB color must be three bytes"))?;
    if values.len()!=3 { return Err(Error::Invalid("RGB color must be three bytes")); }
    let mut out=[0;3];
    for (o,v) in out.iter_mut().zip(values) { *o=integer(v)?.try_into().map_err(|_|Error::Invalid("RGB byte exceeds 255"))?; }
    Ok(out)
}

#[path="extended_inspect.rs"] mod extended;
pub use extended::EXTENDED_INSPECTION_DOMAINS;
