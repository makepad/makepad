//! Versioned, immutable authored soft-body metadata. No simulator state lives
//! in an asset; clients instantiate their own bounded worker-owned solver.
use makepad_strict_json::{self as json, Value};
use std::collections::BTreeSet;

pub const SOFT_BODY_EXTRAS_KEY: &str = "MAKEPAD_soft_body";
pub const SOFT_BODY_VERSION: u32 = 1;
pub const SOFT_BODY_PARTICLES: usize = 43;
pub const SOFT_BODY_TETRAHEDRA: usize = 80;
pub const SOFT_BODY_MAX_JOINTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBodyBinding {
    pub tetrahedron: u16,
    pub weights: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBodySettings {
    pub mass: f32,
    pub edge_compliance: f32,
    pub volume_compliance: f32,
    pub pose_compliance: f32,
    pub damping: f32,
    pub gravity: [f32; 3],
    pub contact_radius: f32,
    pub friction: f32,
    pub substeps: u8,
    pub iterations: u8,
    pub max_speed: f32,
    pub max_displacement: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoftBodyAttachment {
    pub name: String,
    /// Palette ordinal, never a glTF node index.
    pub joint: u16,
    pub binding: SoftBodyBinding,
    pub rest_pivot: [f32; 3],
    pub objects: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoftBodyMetadata {
    pub version: u32,
    pub object: String,
    /// Extra visible meshes (for example yarn fibers) deformed by the same
    /// cage but deliberately excluded from the body's contact samples.
    pub deform_objects: Vec<String>,
    pub root_joint: u16,
    pub rest_positions: Vec<[f32; 3]>,
    pub tetrahedra: Vec<[u16; 4]>,
    /// Embedded visible-body surface contacts. The larger enclosing cage is
    /// never implicitly treated as the collision surface.
    pub surface_samples: Vec<SoftBodyBinding>,
    pub anchors: Vec<u16>,
    pub settings: SoftBodySettings,
    pub tet_joints: Vec<u16>,
    pub attachments: Vec<SoftBodyAttachment>,
    /// Canonical authoring geometry/topology fingerprint, checked on reopen.
    /// Skin weights are validated against the cage and rigid-frame hierarchy.
    pub geometry_hash: String,
}

fn name(s: &str) -> bool { !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control) }
fn finite(v: f32) -> bool { v.is_finite() && v.abs() <= 1.0e6 }
fn range(v: f32, min: f32, max: f32) -> bool { v.is_finite() && (min..=max).contains(&v) }

impl SoftBodySettings {
    pub fn validate(&self) -> Result<(), String> {
        if !range(self.mass, f32::MIN_POSITIVE, 10_000.)
            || ![self.edge_compliance, self.volume_compliance, self.pose_compliance].into_iter().all(|v|range(v,0.,1000.))
            || !range(self.damping,0.,100.) || !self.gravity.into_iter().all(|v|range(v,-100.,100.))
            || !range(self.contact_radius,0.,10.) || !range(self.friction,0.,2.)
            || !(1..=8).contains(&self.substeps) || !(1..=16).contains(&self.iterations)
            || !range(self.max_speed,f32::MIN_POSITIVE,1000.) || !range(self.max_displacement,f32::MIN_POSITIVE,1000.) {
            return Err("invalid soft-body solver settings".into());
        }
        Ok(())
    }
    pub fn to_value(&self) -> Value {
        json::obj(vec![
            ("mass", n(self.mass)), ("edge_compliance",n(self.edge_compliance)),
            ("volume_compliance",n(self.volume_compliance)), ("pose_compliance",n(self.pose_compliance)),
            ("damping",n(self.damping)), ("gravity",vec_value(&self.gravity)),
            ("contact_radius",n(self.contact_radius)), ("friction",n(self.friction)),
            ("substeps",i(self.substeps)), ("iterations",i(self.iterations)),
            ("max_speed",n(self.max_speed)), ("max_displacement",n(self.max_displacement)),
        ])
    }
    pub fn from_value(v: &Value) -> Result<Self,String> {
        fields(v,&["mass","edge_compliance","volume_compliance","pose_compliance","damping","gravity","contact_radius","friction","substeps","iterations","max_speed","max_displacement"])?;
        let out=Self { mass:f(v,"mass")?,edge_compliance:f(v,"edge_compliance")?,volume_compliance:f(v,"volume_compliance")?,
            pose_compliance:f(v,"pose_compliance")?,damping:f(v,"damping")?,gravity:array(need(v,"gravity")?)?,
            contact_radius:f(v,"contact_radius")?,friction:f(v,"friction")?,substeps:integer(need(v,"substeps")?)?,
            iterations:integer(need(v,"iterations")?)?,max_speed:f(v,"max_speed")?,max_displacement:f(v,"max_displacement")? };
        out.validate()?;Ok(out)
    }
}

impl SoftBodyMetadata {
    pub fn validate(&self, joint_count: usize) -> Result<(),String> {
        if self.version != SOFT_BODY_VERSION {return Err("unsupported soft-body metadata version".into());}
        if !name(&self.object) || self.root_joint as usize >= joint_count || joint_count > SOFT_BODY_MAX_JOINTS {
            return Err("soft-body object/root/joint budget".into());
        }
        if self.rest_positions.len()!=SOFT_BODY_PARTICLES || self.tetrahedra.len()!=SOFT_BODY_TETRAHEDRA
            || self.tet_joints.len()!=self.tetrahedra.len() || self.surface_samples.is_empty() || self.surface_samples.len()>128
            || self.anchors.is_empty() || self.anchors.len()>self.rest_positions.len() || self.attachments.len()>32 {
            return Err("unsupported soft-body cage or binding budget".into());
        }
        if self.rest_positions.iter().flatten().any(|v|!finite(*v)) {return Err("non-finite soft-body rest position".into());}
        let mut cells=BTreeSet::new();
        for tet in &self.tetrahedra {
            let mut indices=*tet;indices.sort_unstable();
            if indices.windows(2).any(|p|p[0]==p[1]) || indices[3] as usize>=self.rest_positions.len() || !cells.insert(indices) {
                return Err("invalid or duplicate soft-body tetrahedron".into());
            }
            let [a,b,c,d]=tet.map(|i|self.rest_positions[i as usize].map(f64::from));
            let sub=|a:[f64;3],b:[f64;3]|std::array::from_fn::<_,3,_>(|i|a[i]-b[i]);
            let (b,c,d)=(sub(b,a),sub(c,a),sub(d,a));
            let volume=b[0]*(c[1]*d[2]-c[2]*d[1])-b[1]*(c[0]*d[2]-c[2]*d[0])+b[2]*(c[0]*d[1]-c[1]*d[0]);
            if !volume.is_finite() || volume<=1e-12 {return Err("soft-body tetrahedron must have positive rest volume".into());}
        }
        let mut joints=BTreeSet::from([self.root_joint]);
        for &joint in &self.tet_joints {
            if joint as usize>=joint_count || !joints.insert(joint) {return Err("duplicate/out-of-range soft-body palette joint".into());}
        }
        let mut anchors=BTreeSet::new();
        for &anchor in &self.anchors {
            if anchor as usize>=self.rest_positions.len() || !anchors.insert(anchor) {return Err("invalid soft-body anchor".into());}
        }
        for sample in &self.surface_samples {self.validate_binding(*sample)?;}
        let mut names=BTreeSet::new();let mut objects=BTreeSet::from([self.object.as_str()]);
        if self.deform_objects.len()>32 {return Err("soft-body deform object budget".into());}
        for object in &self.deform_objects {if !name(object)||!objects.insert(object.as_str()){return Err("duplicate/invalid soft-body deform object".into());}}
        for attachment in &self.attachments {
            if !name(&attachment.name) || !names.insert(attachment.name.as_str()) || attachment.joint as usize>=joint_count
                || !joints.insert(attachment.joint) || attachment.objects.is_empty() || attachment.objects.len()>64
                || attachment.rest_pivot.iter().any(|v|!finite(*v)) {
                return Err("invalid soft-body rigid attachment".into());
            }
            self.validate_binding(attachment.binding)?;
            let point=self.rest_point(attachment.binding)?;
            if point.iter().zip(attachment.rest_pivot).any(|(a,b)|(*a-b).abs()>1e-4*(1.+b.abs())) {
                return Err("soft-body attachment pivot does not match binding".into());
            }
            for object in &attachment.objects {if !name(object) || !objects.insert(object.as_str()) {return Err("duplicate/invalid soft-body attachment object".into());}}
        }
        if self.geometry_hash.len()!=64 || !self.geometry_hash.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)) {
            return Err("soft-body geometry fingerprint".into());
        }
        self.settings.validate()
    }
    pub fn validate_binding(&self, binding: SoftBodyBinding) -> Result<(),String> {
        if binding.tetrahedron as usize>=self.tetrahedra.len() || binding.weights.iter().any(|w|!range(*w,0.,1.))
            || (binding.weights.iter().sum::<f32>()-1.).abs()>1e-5 {return Err("invalid soft-body barycentric binding".into());} Ok(())
    }
    pub fn rest_point(&self,binding:SoftBodyBinding)->Result<[f32;3],String>{
        self.validate_binding(binding)?;let tet=self.tetrahedra[binding.tetrahedron as usize];let mut point=[0.;3];
        for (particle,weight) in tet.into_iter().zip(binding.weights) {let p=self.rest_positions.get(particle as usize).ok_or("soft-body particle index")?;for axis in 0..3{point[axis]+=p[axis]*weight;}}
        Ok(point)
    }
    pub fn to_value(&self)->Value {
        json::obj(vec![
            ("version",i(self.version)),("object",json::s(&self.object)),("root_joint",i(self.root_joint)),
            ("deform_objects",Value::Arr(self.deform_objects.iter().map(json::s).collect())),
            ("rest_positions",Value::Arr(self.rest_positions.iter().map(|p|vec_value(p)).collect())),
            ("tetrahedra",Value::Arr(self.tetrahedra.iter().map(|t|Value::Arr(t.iter().copied().map(i).collect())).collect())),
            ("surface_samples",Value::Arr(self.surface_samples.iter().map(binding_value).collect())),
            ("anchors",Value::Arr(self.anchors.iter().copied().map(i).collect())),
            ("settings",self.settings.to_value()),("tet_joints",Value::Arr(self.tet_joints.iter().copied().map(i).collect())),
            ("attachments",Value::Arr(self.attachments.iter().map(|a|json::obj(vec![
                ("name",json::s(&a.name)),("joint",i(a.joint)),("binding",binding_value(&a.binding)),
                ("rest_pivot",vec_value(&a.rest_pivot)),("objects",Value::Arr(a.objects.iter().map(json::s).collect())),
            ])).collect())),("geometry_hash",json::s(&self.geometry_hash)),
        ])
    }
    pub fn from_value(v:&Value,joint_count:usize)->Result<Self,String>{
        fields(v,&["version","object","deform_objects","root_joint","rest_positions","tetrahedra","surface_samples","anchors","settings","tet_joints","attachments","geometry_hash"])?;
        let out=Self {
            version:integer(need(v,"version")?)?,object:text(v,"object")?.into(),root_joint:integer(need(v,"root_joint")?)?,
            deform_objects:rows(need(v,"deform_objects")?,32)?.iter().map(|v|v.as_str().map(str::to_owned).ok_or_else(||"deform object must be a string".into())).collect::<Result<_,String>>()?,
            rest_positions:rows(need(v,"rest_positions")?,43)?.iter().map(array).collect::<Result<_,_>>()?,
            tetrahedra:rows(need(v,"tetrahedra")?,80)?.iter().map(|v|{let v=rows(v,4)?;if v.len()!=4{return Err("tetrahedron width".into());}Ok([integer(&v[0])?,integer(&v[1])?,integer(&v[2])?,integer(&v[3])?])}).collect::<Result<_,String>>()?,
            surface_samples:rows(need(v,"surface_samples")?,128)?.iter().map(binding).collect::<Result<_,_>>()?,
            anchors:rows(need(v,"anchors")?,43)?.iter().map(integer).collect::<Result<_,_>>()?,
            settings:SoftBodySettings::from_value(need(v,"settings")?)?,
            tet_joints:rows(need(v,"tet_joints")?,80)?.iter().map(integer).collect::<Result<_,_>>()?,
            attachments:rows(need(v,"attachments")?,32)?.iter().map(|v|{fields(v,&["name","joint","binding","rest_pivot","objects"])?;
                Ok(SoftBodyAttachment{name:text(v,"name")?.into(),joint:integer(need(v,"joint")?)?,binding:binding(need(v,"binding")?)?,rest_pivot:array(need(v,"rest_pivot")?)?,
                    objects:rows(need(v,"objects")?,64)?.iter().map(|v|v.as_str().map(str::to_owned).ok_or_else(||"attachment object must be a string".into())).collect::<Result<_,String>>()?})}).collect::<Result<_,String>>()?,
            geometry_hash:text(v,"geometry_hash")?.into(),
        };out.validate(joint_count)?;Ok(out)
    }
    /// glTF serializer representation; authoring source uses to_value's
    /// ordered JSON to retain canonical source hashes across processes.
    pub fn to_gltf_value(&self)->crate::JsonValue { gltf_value(self.to_value()) }
}

/// Parse only when the asset advertises the metadata. Strict parsing rejects
/// duplicate keys and malformed numbers instead of normalizing them away.
pub fn parse_soft_body_metadata_json(bytes:&[u8],joint_count:usize)->Result<Option<SoftBodyMetadata>,String>{
    let value=json::parse_depth(bytes,32).map_err(str::to_owned)?;
    let mut found=None;
    for node in value.get("nodes").and_then(Value::as_arr).unwrap_or(&[]) {
        if let Some(metadata)=node.get("extras").and_then(|extras|extras.get(SOFT_BODY_EXTRAS_KEY)) {
            if found.is_some(){return Err("multiple soft-body metadata owners".into());}
            found=Some(SoftBodyMetadata::from_value(metadata,joint_count)?);
        }
    }
    Ok(found)
}

/// Append metadata to the existing skin node without rebuilding the authored
/// vertex stream, materials, clips, or other extras.
pub fn augment_glb_soft_body(input:&[u8],metadata:&SoftBodyMetadata)->Result<Vec<u8>,crate::GltfError>{
    use crate::{augment::{GlbRewrite,validation},JsonValue as J};
    let mut rewrite=GlbRewrite::begin(input)?;
    let skins=rewrite.document.skins.as_deref().unwrap_or(&[]);
    if skins.len()!=1{return Err(validation("soft-body metadata requires one skin"));}
    let count=match &skins[0]{J::Object(fields)=>match fields.get("joints"){Some(J::Array(joints))=>joints.len(),_=>0},_=>0};
    metadata.validate(count).map_err(validation)?;
    let nodes=rewrite.array_mut("nodes")?;
    for node in nodes.iter(){if let J::Object(fields)=node{if let Some(J::Object(extras))=fields.get("extras"){
        if extras.contains_key(SOFT_BODY_EXTRAS_KEY){return Err(validation("soft-body metadata already exists"));}
    }}}
    let owner=nodes.iter_mut().find_map(|node|match node{J::Object(fields) if matches!(fields.get("skin"),Some(J::U64(0)|J::I64(0)))=>Some(fields),_=>None})
        .ok_or_else(||validation("soft-body skin node missing"))?;
    let extras=owner.entry("extras".into()).or_insert_with(||J::Object(Default::default()));
    let J::Object(extras)=extras else{return Err(validation("soft-body owner extras must be an object"));};
    extras.insert(SOFT_BODY_EXTRAS_KEY.into(),metadata.to_gltf_value());
    rewrite.finish()
}

fn n(v:f32)->Value{Value::F64(v as f64)}
fn i(v:impl Into<i64>)->Value{Value::Int(v.into())}
fn vec_value<const N:usize>(v:&[f32;N])->Value{Value::Arr(v.iter().copied().map(n).collect())}
fn binding_value(b:&SoftBodyBinding)->Value{json::obj(vec![("tetrahedron",i(b.tetrahedron)),("weights",vec_value(&b.weights))])}
fn binding(v:&Value)->Result<SoftBodyBinding,String>{fields(v,&["tetrahedron","weights"])?;Ok(SoftBodyBinding{tetrahedron:integer(need(v,"tetrahedron")?)?,weights:array(need(v,"weights")?)?})}
fn fields(v:&Value,allowed:&[&str])->Result<(),String>{let Value::Obj(fields)=v else{return Err("soft-body metadata must be an object".into());};let mut seen=BTreeSet::new();for(key,_)in fields{if !allowed.contains(&key.as_str())||!seen.insert(key){return Err("unknown/duplicate soft-body metadata field".into());}}Ok(())}
fn need<'a>(v:&'a Value,key:&str)->Result<&'a Value,String>{v.get(key).ok_or_else(||format!("missing soft-body field {key}"))}
fn text<'a>(v:&'a Value,key:&str)->Result<&'a str,String>{need(v,key)?.as_str().ok_or_else(||format!("soft-body {key} must be a string"))}
fn rows(v:&Value,max:usize)->Result<&[Value],String>{let a=v.as_arr().ok_or("soft-body metadata array")?;if a.len()>max{return Err("soft-body metadata array budget".into());}Ok(a)}
fn integer<T:TryFrom<u64>>(v:&Value)->Result<T,String>{v.as_u64().and_then(|v|v.try_into().ok()).ok_or_else(||"soft-body integer out of range".into())}
fn f(v:&Value,key:&str)->Result<f32,String>{scalar(need(v,key)?)}
fn scalar(v:&Value)->Result<f32,String>{let n=match v{Value::F64(v)=>*v,Value::Int(v)=>*v as f64,_=>return Err("soft-body numeric value".into())} as f32;if !n.is_finite(){return Err("soft-body non-finite value".into());}Ok(n)}
fn array<const N:usize>(v:&Value)->Result<[f32;N],String>{let a=rows(v,N)?;if a.len()!=N{return Err("soft-body vector width".into());}let mut out=[0.;N];for(o,v)in out.iter_mut().zip(a){*o=scalar(v)?;}Ok(out)}
fn gltf_value(v:Value)->crate::JsonValue{use crate::JsonValue as J;match v{Value::Null=>J::Null,Value::Bool(v)=>J::Bool(v),Value::Int(v)=>J::I64(v),Value::F64(v)=>J::F64(v),Value::Str(v)=>J::String(v),Value::Arr(v)=>J::Array(v.into_iter().map(gltf_value).collect()),Value::Obj(v)=>J::Object(v.into_iter().map(|(k,v)|(k,gltf_value(v))).collect())}}
