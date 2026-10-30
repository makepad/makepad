//! Bounded inspection pages for worker-owned model documents.
use crate::{json::{self, Value}, mesh, Document, Error, Head, Material, Result};

/// Leaves room for the chat ToolOutcome wrapper inside its 16 KiB envelope.
pub const MAX_REPLY_BYTES:usize = 12 * 1024;

/// One page of `domain` of a document (the chat's `model.inspect`).
pub fn inspect_document(doc:&Document, document:&str, object:Option<&str>, domain:&str, offset:usize, limit:usize,
    cancelled:Option<&dyn Fn()->bool>)->Result<Value> {
    {
        let mut ctx=mesh::Context::new(doc.limits().mesh.clone(),cancelled);
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
pub(crate) fn head_json(head:Head)->Value {
    json::obj(vec![("generation",id(head.generation)),("content",json::s(head.content.iter().map(|b|format!("{b:02x}")).collect::<String>()))])
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
#[path="extended_inspect.rs"] mod extended;
pub use extended::EXTENDED_INSPECTION_DOMAINS;

#[cfg(test)]
mod extended_inspect_tests {
    use crate::{json::Value,*};
    use std::collections::BTreeMap;
    fn fixture()->Document{
        let mut d=Document::new(Limits::default()).unwrap();let mut image=RgbaImage::new(32,32,[0;4],d.limits()).unwrap();for(i,p)in image.pixels.chunks_exact_mut(4).enumerate(){p.copy_from_slice(&[(i%255) as u8,(i/255) as u8,128,255]);}
        d.apply(Transaction{request_id:"seed".into(),expected:d.head(),operations:vec![Operation::Cube{object:"body".into(),size:[1.;3]},Operation::Surface(SurfaceOperation::Layer{material:0,channel:SurfaceChannel::BaseColor,layer:SurfaceLayer::new("paint",image)}),Operation::SetSkeleton{skeleton:Skeleton{joints:vec![Joint{name:"root".into(),parent:None,translation:[0.;3]}]}},Operation::AutoWeights{object:"body".into()},Operation::SetClip{clip:AnimationClip{name:"clip".into(),channels:vec![AnimationChannel{joint:0,path:AnimationPath::Translation,keys:vec![Keyframe{time:0.,value:[0.;4]},Keyframe{time:1.,value:[0.,1.,0.,0.]}]}]}},Operation::Rig(RigOperation::Pose(Pose{name:"pose".into(),joints:BTreeMap::from([(0,Transform::default())])})),Operation::Rig(RigOperation::ClipOptions{name:"clip".into(),options:ClipOptions{events:vec![ClipEvent{time:0.5,name:"event".into(),payload:"\0".repeat(4096)}],..Default::default()}})]},None).unwrap();
        let vertex=d.object("body").unwrap().vertices()[0].id;let face=d.object("body").unwrap().faces()[0].id;let ops=vec![Operation::Selection(SelectionOperation::Set{object:"body".into(),name:"chosen".into(),vertices:vec![vertex],faces:vec![face]})];d.apply(Transaction{request_id:"selection".into(),expected:d.head(),operations:ops},None).unwrap();d
    }
    fn reopen(d:&Document)->Document{Document::from_bytes(&d.to_bytes(None).unwrap(),d.limits().clone(),None).unwrap()}
    fn inspect(d:&Document,domain:&str,offset:u32)->Value {
        let object=if ["uv_islands","uv_island_faces","uv_pins","solid"].contains(&domain){Some("body")}else{None};
        inspect_document(d,"doc",object,domain,offset as usize,128,None).unwrap()
    }
    #[test]fn every_extended_domain_obeys_shared_page_envelope_and_source_is_unchanged(){let d=reopen(&fixture());let before=d.to_bytes(None).unwrap();for domain in EXTENDED_INSPECTION_DOMAINS{let result=inspect(&d,domain,0);assert!(result.to_json().len()<=MAX_REPLY_BYTES,"{domain}");assert!(result.get("total").is_some());assert!(result.get("count").is_some());assert!(result.get("next_offset").is_some());}assert_eq!(d.to_bytes(None).unwrap(),before);let solid=inspect(&d,"solid",0);assert_eq!(solid.get("items").unwrap().as_arr().unwrap()[0].get("is_valid_solid").and_then(Value::as_bool),Some(true),"{}",solid.to_json());}
    #[test]fn binary_pixels_and_escaped_event_payload_have_complete_advancing_pages(){let d=reopen(&fixture());let mut offset=0;let mut pixels=Vec::new();loop{let result=inspect(&d,"surface_pixels",offset);assert!(result.to_json().len()<=MAX_REPLY_BYTES);for row in result.get("items").unwrap().as_arr().unwrap(){pixels.extend(row.get("rgba").unwrap().as_arr().unwrap().iter().map(|v|v.as_u64().unwrap()as u8));}match result.get("next_offset").and_then(Value::as_u64){Some(next)=>{assert!(next>offset as u64);offset=next as u32;},None=>break}}assert_eq!(pixels,d.surface().materials[&0].channels[&SurfaceChannel::BaseColor][0].image.pixels);
        let preview=inspect(&d,"clip_events",0);let row=&preview.get("items").unwrap().as_arr().unwrap()[0];assert_eq!(row.get("payload_omitted").and_then(Value::as_u64),Some(3584));let mut text=String::new();let mut offset=0;loop{let page=inspect(&d,"clip_event_payload",offset);assert!(page.to_json().len()<=MAX_REPLY_BYTES);for row in page.get("items").unwrap().as_arr().unwrap(){assert_eq!(row.get("byte_offset").and_then(Value::as_u64),Some(text.len()as u64));text.push_str(row.get("text").unwrap().as_str().unwrap());}match page.get("next_offset").and_then(Value::as_u64){Some(next)=>{assert!(next>offset as u64);offset=next as u32;},None=>break}}assert_eq!(text,"\0".repeat(4096));
    }
    #[test]fn selections_have_stable_ids_and_cancelled_global_checks_return_no_certificate(){let d=reopen(&fixture());let groups=inspect(&d,"selections",0);assert_eq!(groups.get("total").and_then(Value::as_u64),Some(1));let vertices=inspect(&d,"selection_vertices",0);assert!(vertices.get("items").unwrap().as_arr().unwrap()[0].get("vertex").unwrap().as_str().is_some());let before=d.to_bytes(None).unwrap();assert!(inspect_document(&d,"doc",Some("body"),"solid",0,32,Some(&||true)).is_err());assert_eq!(d.to_bytes(None).unwrap(),before);}

    #[test]fn joint_inventory_reports_effective_rest_override(){
        let mut d=fixture();
        d.apply(Transaction{request_id:"rest".into(),expected:d.head(),operations:vec![
            Operation::Rig(RigOperation::Rest{joint:0,transform:Transform{translation:[3.,4.,5.],rotation:[0.,0.,0.,1.],scale:[2.;3]}}),
        ]},None).unwrap();

        let d=reopen(&d);
        let joints=inspect(&d,"joints",0);let row=&joints.get("items").unwrap().as_arr().unwrap()[0];
        assert_eq!(row.get("translation").unwrap().to_json(),"[3.0,4.0,5.0]");
        assert_eq!(row.get("scale").unwrap().to_json(),"[2.0,2.0,2.0]");
    }

    #[test]
    fn vehicle_wheel_inventory_returns_bound_local_geometry_and_advances() {
        let mut d=Document::new(Limits::default()).unwrap();
        d.apply(Transaction{request_id:"wheel".into(),expected:d.head(),operations:vec![
            Operation::Cube { object: "tire".into(), size: [0.2, 0.7, 0.7] },
            Operation::Scene(SceneOperation::VehicleWheel {
                object: "tire".into(),
                wheel: VehicleWheel {
                    connection: "wheel_front_left".into(),
                    pivot: [0.0, 0.0, 0.0],
                    radius: 0.35,
                    width: 0.2,
                    visual: None
                }
            }),
        ]},None).unwrap();

        let d=reopen(&d);
        let page = inspect(&d, "vehicle_wheels", 0);
        assert_eq!(page.get("count").and_then(Value::as_u64), Some(1));
        let row = &page.get("items").unwrap().as_arr().unwrap()[0];
        assert_eq!(row.get("connection").and_then(Value::as_str), Some("wheel_front_left"));
        assert_eq!(row.get("object").and_then(Value::as_str), Some("tire"));
        assert_eq!(row.get("radius"), Some(&Value::F64(0.35)));
        assert_eq!(inspect(&d, "vehicle_wheels", 1).get("count").and_then(Value::as_u64), Some(0));
        assert_eq!(inspect_document(&d,"doc",Some("tire"),"vehicle_wheels",0,32,None).unwrap().get("total").and_then(Value::as_u64),Some(1));
        assert_eq!(inspect_document(&d,"doc",Some("missing"),"vehicle_wheels",0,32,None).unwrap().get("total").and_then(Value::as_u64),Some(0));
    }
}
