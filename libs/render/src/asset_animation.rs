//! Bounded authored animation metadata and deterministic playback consumers.
use makepad_gltf::JsonValue;
#[derive(Clone,Debug,PartialEq)]pub struct AssetClipEvent{pub time:f64,pub name:String,pub payload:String}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub enum AssetInterpolation{Linear,Step,Cubic}
#[derive(Clone,Debug,PartialEq)]pub struct AssetAnimationMetadata{pub duration:f64,pub root_motion:bool,pub events:Vec<AssetClipEvent>,pub interpolation:AssetInterpolation}
fn num(v:&JsonValue)->Option<f64>{match v{JsonValue::F64(v)=>Some(*v),JsonValue::U64(v)=>Some(*v as f64),JsonValue::I64(v)=>Some(*v as f64),_=>None}.filter(|v|v.is_finite())}
fn list(v:&JsonValue)->Option<&[JsonValue]>{if let JsonValue::Array(v)=v{Some(v)}else{None}}
fn flag(v:Option<&JsonValue>,default:bool)->Result<bool,String>{match v{None=>Ok(default),Some(JsonValue::Bool(v))=>Ok(*v),_=>Err("animation boolean".into())}}
impl AssetAnimationMetadata{
    pub fn parse(animation:&JsonValue,duration:f64)->Result<Option<Self>,String>{
        let Some(value)=animation.key("extras").and_then(|v|v.key("MAKEPAD_animation"))else{return Ok(None)};
        if !duration.is_finite()||duration<=0.||duration>3600.{return Err("animation duration outside0..3600".into());}
        if !matches!(value,JsonValue::Object(_)){return Err("animation metadata object".into());}
        let interpolation=match value.key("interpolation"){None=>AssetInterpolation::Linear,Some(JsonValue::String(v))=>match v.as_str(){"linear"=>AssetInterpolation::Linear,"step"=>AssetInterpolation::Step,"cubic"=>AssetInterpolation::Cubic,_=>return Err("animation interpolation".into())},_=>return Err("animation interpolation".into())};
        let mut events=Vec::new();let mut bytes=0usize;let mut previous=0.;if let Some(values)=value.key("events"){let values=list(values).ok_or("animation events array")?;if values.len()>1024{return Err("animation event count exceeds1024".into());}for e in values{
            let time=e.key("time").and_then(num).ok_or("animation event time")?;let name=e.key("name").and_then(JsonValue::string).ok_or("animation event name")?;let payload=match e.key("payload"){None=>"",Some(JsonValue::String(v))=>v,_=>return Err("animation event payload".into())};
            if time<previous||time>duration||name.is_empty()||name.len()>96||name.chars().any(char::is_control)||payload.len()>4096{return Err("invalid animation event order/name/payload".into());}bytes+=name.len()+payload.len()+64;if bytes>1024*1024{return Err("animation event bytes exceed1MiB".into());}events.push(AssetClipEvent{time,name:name.clone(),payload:payload.into()});previous=time;
        }}Ok(Some(Self{duration,root_motion:flag(value.key("root_motion"),false)?,events,interpolation}))
    }
}
#[derive(Clone,Copy,Debug,PartialEq)]pub struct EventOccurrence{pub event:usize,pub cycle:u64,pub time:f64}
#[derive(Default,Clone,Debug)]pub struct AnimationCursor{last:Option<f64>}
impl AnimationCursor{
    /// Seek without firing events. A fresh cursor fires time-zero events on its
    /// first advance. Time is monotonic and unwrapped even for looping clips.
    pub fn seek(&mut self,time:f64)->Result<(),String>{valid_time(time)?;self.last=Some(time);Ok(())}
    pub fn reset(&mut self){self.last=None;}
    pub fn advance(&mut self,metadata:&AssetAnimationMetadata,time:f64,looping:bool,event_limit:usize)->Result<Vec<EventOccurrence>,String>{
        valid_time(time)?;if event_limit>4096{return Err("animation event delivery limit exceeds4096".into());}if self.last.is_some_and(|old|time<old){return Err("animation time went backward; seek explicitly".into());}
        let from=self.last.unwrap_or(-f64::EPSILON);if time==from{return Ok(Vec::new());}
        let duration=metadata.duration;if !duration.is_finite()||duration<=0.{return Err("invalid animation duration".into());}
        let first=if looping{(from.max(0.)/duration).floor() as u64}else{0};let last=if looping{(time/duration).floor() as u64}else{0};if last>1_000_000_000||last.saturating_sub(first)>4096{return Err("animation event cycle budget".into());}
        let mut out=Vec::new();for cycle in first..=last{for(event,e)in metadata.events.iter().enumerate(){let t=cycle as f64*duration+e.time;if t>from&&t<=time&&(looping||t<=duration){if out.len()==event_limit{return Err("animation event output full; cursor unchanged".into());}out.push(EventOccurrence{event,cycle,time:t});}}}
        // At a wrap, terminal events of the old cycle precede initial events
        // of the next cycle; source order breaks equal-time ties within a cycle.
        self.last=Some(time);Ok(out)
    }
}
fn valid_time(t:f64)->Result<(),String>{if t.is_finite()&&t>=0.&&t<=3.6e12{Ok(())}else{Err("invalid unwrapped animation time".into())}}
#[derive(Clone,Copy,Debug,PartialEq)]pub struct RigidTransform{pub translation:[f64;3],pub rotation:[f64;4]}
impl Default for RigidTransform{fn default()->Self{Self{translation:[0.;3],rotation:[0.,0.,0.,1.]}}}
impl RigidTransform{
    pub fn validate(&self)->Result<(),String>{if self.translation.iter().any(|v|!v.is_finite()||v.abs()>1e12)||self.rotation.iter().any(|v|!v.is_finite())||(self.rotation.iter().map(|v|v*v).sum::<f64>()-1.).abs()>1e-6{Err("invalid rigid root transform".into())}else{Ok(())}}
    pub fn compose(self,other:Self)->Result<Self,String>{self.validate()?;other.validate()?;let r=rotate(self.rotation,other.translation);let out=Self{translation:std::array::from_fn(|i|self.translation[i]+r[i]),rotation:qnorm(qmul(self.rotation,other.rotation))?};out.validate()?;Ok(out)}
    pub fn inverse(self)->Result<Self,String>{self.validate()?;let r=[-self.rotation[0],-self.rotation[1],-self.rotation[2],self.rotation[3]];Ok(Self{translation:rotate(r,self.translation.map(|v|-v)),rotation:r})}
}
fn qmul(a:[f64;4],b:[f64;4])->[f64;4]{[a[3]*b[0]+a[0]*b[3]+a[1]*b[2]-a[2]*b[1],a[3]*b[1]-a[0]*b[2]+a[1]*b[3]+a[2]*b[0],a[3]*b[2]+a[0]*b[1]-a[1]*b[0]+a[2]*b[3],a[3]*b[3]-a[0]*b[0]-a[1]*b[1]-a[2]*b[2]]}
fn qnorm(q:[f64;4])->Result<[f64;4],String>{let n=q.iter().map(|v|v*v).sum::<f64>().sqrt();if !n.is_finite()||n<1e-12{return Err("invalid root quaternion".into())}Ok(q.map(|v|v/n))}
fn rotate(q:[f64;4],p:[f64;3])->[f64;3]{let v=qmul(qmul(q,[p[0],p[1],p[2],0.]),[-q[0],-q[1],-q[2],q[3]]);[v[0],v[1],v[2]]}
fn power(mut transform:RigidTransform,mut n:u64)->Result<RigidTransform,String>{let mut result=RigidTransform::default();while n>0{if n&1==1{result=result.compose(transform)?;}n>>=1;if n>0{transform=transform.compose(transform)?;}}Ok(result)}
fn absolute(time:f64,duration:f64,looping:bool,sample:&mut impl FnMut(f64)->Result<RigidTransform,String>)->Result<RigidTransform,String>{valid_time(time)?;if !duration.is_finite()||duration<=0.||duration>3600.{return Err("invalid root clip duration".into());}let start=sample(0.)?.inverse()?;if !looping{return start.compose(sample(time.min(duration))?)}let cycles=(time/duration).floor();if cycles>1_000_000_000.{return Err("root motion cycle budget".into());}let cycle=start.compose(sample(duration)?)?;let phase=start.compose(sample(time.rem_euclid(duration))?)?;power(cycle,cycles as u64)?.compose(phase)}
#[derive(Clone,Debug,Default)]pub struct RootMotionCursor{last_time:Option<f64>,last:RigidTransform,pending:RigidTransform}
impl RootMotionCursor{
    pub fn seek(&mut self,time:f64,duration:f64,looping:bool,mut sample:impl FnMut(f64)->Result<RigidTransform,String>)->Result<(),String>{let value=absolute(time,duration,looping,&mut sample)?;self.last=value;self.last_time=Some(time);self.pending=RigidTransform::default();Ok(())}
    /// Compose the root's translation and rotation across wrapped cycles. The
    /// caller removes the consumed root channel from the visual pose and applies
    /// consume() to the actor exactly once, avoiding double motion.
    pub fn advance(&mut self,time:f64,duration:f64,looping:bool,mut sample:impl FnMut(f64)->Result<RigidTransform,String>)->Result<RigidTransform,String>{if self.last_time.is_some_and(|old|time<old){return Err("root time went backward; seek explicitly".into());}let value=absolute(time,duration,looping,&mut sample)?;let delta=self.last.inverse()?.compose(value)?;let pending=self.pending.compose(delta)?;self.last=value;self.last_time=Some(time);self.pending=pending;Ok(delta)}
    pub fn consume(&mut self)->RigidTransform{std::mem::take(&mut self.pending)}
}
#[cfg(test)]mod tests{
    use super::*;
    fn metadata()->AssetAnimationMetadata{AssetAnimationMetadata{duration:1.,root_motion:true,interpolation:AssetInterpolation::Linear,events:vec![AssetClipEvent{time:0.,name:"start".into(),payload:String::new()},AssetClipEvent{time:0.5,name:"step".into(),payload:String::new()},AssetClipEvent{time:1.,name:"end".into(),payload:String::new()}]}}
    #[test]fn crossings_include_wraps_once_and_overflow_does_not_lose_events(){let mut c=AnimationCursor::default();assert_eq!(c.advance(&metadata(),0.,true,8).unwrap().len(),1);assert_eq!(c.advance(&metadata(),0.75,true,8).unwrap()[0].event,1);let events=c.advance(&metadata(),1.,true,8).unwrap();assert_eq!(events.iter().map(|e|(e.event,e.cycle)).collect::<Vec<_>>(),vec![(2,0),(0,1)]);assert!(c.advance(&metadata(),1.,true,8).unwrap().is_empty());assert!(c.advance(&metadata(),2.,true,1).is_err());assert_eq!(c.advance(&metadata(),2.,true,8).unwrap().len(),3);c.seek(0.4).unwrap();assert_eq!(c.advance(&metadata(),0.6,false,8).unwrap()[0].event,1);}
    #[test]fn root_motion_composes_rotated_cycles_and_consume_is_once(){let sample=|t:f64|Ok(RigidTransform{translation:[t,0.,0.],rotation:[0.,0.,(t*std::f64::consts::FRAC_PI_4).sin(),(t*std::f64::consts::FRAC_PI_4).cos()]});let mut c=RootMotionCursor::default();c.advance(1.,1.,true,sample).unwrap();let first=c.consume();assert!((first.translation[0]-1.).abs()<1e-10);assert_eq!(c.consume(),RigidTransform::default());c.advance(2.,1.,true,sample).unwrap();let second=c.consume();let world=first.compose(second).unwrap();assert!((world.translation[0]-1.).abs()<1e-10&&(world.translation[1]-1.).abs()<1e-10);c.seek(0.5,1.,true,sample).unwrap();assert_eq!(c.consume(),RigidTransform::default());assert!(c.advance(0.4,1.,true,sample).is_err());}
    #[test]fn malformed_metadata_is_rejected(){for extra in [r#"{"root_motion":"yes"}"#,r#"{"events":[{"time":2,"name":"x"}]}"#,r#"{"events":[{"time":0.8,"name":"x"},{"time":0.4,"name":"y"}]}"#]{let doc=makepad_gltf::parse_gltf_json(&format!(r#"{{"asset":{{"version":"2.0"}},"animations":[{{"extras":{{"MAKEPAD_animation":{extra}}}}}]}}"#)).unwrap();assert!(AssetAnimationMetadata::parse(&doc.animations.as_ref().unwrap()[0],1.).is_err());}}
}
