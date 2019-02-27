use crate::cxturtle::*;
use crate::cx::*;
use crate::events::*;

#[derive(Clone,Debug)]
pub struct AnimArea{
    pub area:Area,
    pub start_time:f64,
    pub duration:f64
}

#[derive(Clone,Debug)]
pub struct AnimState<T>{
    pub id:T,
    pub mode:AnimMode,
    pub tracks:Vec<AnimTrack>
}

#[derive(Clone)]
pub struct AnimStates<T>{
    pub current_id:T,
    pub next_id:Option<T>,
    pub states:Vec<AnimState<T>>
}

impl<T> AnimStates<T>
where T: std::cmp::PartialEq + std::clone::Clone
{

    pub fn new(current_id:T, states:Vec<AnimState<T>>)->AnimStates<T>{
        AnimStates{
            current_id:current_id,
            next_id:None,
            states:states
        }
    }

    pub fn change(&mut self, cx:&mut Cx, state_id:T, area:&Area){
        let state_index_op = self.states.iter().position(|v| v.id == state_id);

        if state_index_op.is_none(){
            println!("Starting animation state does not exist in states");
            return;
        }
        let state_index = state_index_op.unwrap();

        // alright first we find area, it already exists
        if let Some(anim) = cx.animations.iter_mut().find(|v| v.area == *area){
            //do we cut the animation in right now?
            if self.states[state_index].mode.cut(){
                anim.start_time = std::f64::NAN;
                self.current_id = self.states[state_index].id.clone();
                self.next_id = None;
                anim.duration = self.states[state_index].mode.duration();
            }
            else{ // queue it
                self.next_id = Some(self.states[state_index].id.clone());
                let prev_anim_state = self.find_state(self.current_id.clone()).unwrap();
                // lets ask an animation state how long it is
                anim.duration = prev_anim_state.mode.duration() + self.states[state_index].mode.duration()
            }
        }
        else{ // its new
            self.current_id = self.states[state_index].id.clone();
            self.next_id = None;
            cx.animations.push(AnimArea{
                area:area.clone(),
                start_time:std::f64::NAN,
                duration:self.states[state_index].mode.duration()
            })
        }
    }

    pub fn find_state(&mut self, id:T)->Option<&mut AnimState<T>>{
        self.states.iter_mut().find(|v| v.id == id)
    }

    pub fn animate(&mut self, cx: &mut Cx, area_name:&str,  id_area:&Area, tgt_area:&Area, ae:&AnimateEvent){

        // alright first we find area
        let anim_index_opt = cx.animations.iter().position(|v| v.area == *id_area);
        if anim_index_opt.is_none(){
            return
        }
        let anim_index = anim_index_opt.unwrap();
        
        // alright so now what.
        // well if thing is nan, set it to our
        if cx.animations[anim_index].start_time.is_nan(){
            cx.animations[anim_index].start_time = ae.time;
        }
        let start_time = cx.animations[anim_index].start_time;

        let current_state_opt = self.find_state(self.current_id.clone());
        if current_state_opt.is_none(){  // remove anim
            cx.animations.remove(anim_index);
            return
        }
        let current_state = current_state_opt.unwrap();

        // then we need to compute the track time
        let total_time = ae.time - start_time;

        let (stop, time) = current_state.mode.compute_time(total_time);
        if stop{
            cx.animations.remove(anim_index);
        }
        for track in &mut current_state.tracks{
            if track.area_name() == area_name{
                track.compute_and_write(time, tgt_area, cx);
            }
        }
        cx.paint_dirty = true;
    }    
}

#[derive(Clone,Debug)]
pub struct FloatTrack{
    pub area_name:String,
    pub prop_name:String,
    pub first:Option<f32>,
    pub track:Vec<(f64, f32)>
}

#[derive(Clone,Debug)]
pub struct Vec2Track{
    pub area_name:String,
    pub prop_name:String,
    pub first:Option<Vec2>,
    pub track:Vec<(f64, Vec2)>
}

#[derive(Clone,Debug)]
pub struct Vec3Track{
    pub area_name:String,
    pub prop_name:String,
    pub first:Option<Vec3>,
    pub track:Vec<(f64, Vec3)>
}

#[derive(Clone,Debug)]

pub struct Vec4Track{
    pub area_name:String,
    pub prop_name:String,
    pub first:Option<Vec4>,
    pub track:Vec<(f64, Vec4)>
}

#[derive(Clone,Debug)]
pub enum AnimTrack{
    Float(FloatTrack),
    Vec2(Vec2Track),
    Vec3(Vec3Track),
    Vec4(Vec4Track),
}

impl AnimTrack{
    pub fn float(area_name:&str, prop_name:&str, track:Vec<(f64,f32)>)->AnimTrack{
        AnimTrack::Float(FloatTrack{
            first:None,
            area_name:area_name.to_string(),
            prop_name:prop_name.to_string(),
            track:track
        })
    }

    pub fn vec2(area_name:&str, prop_name:&str, track:Vec<(f64,Vec2)>)->AnimTrack{
        AnimTrack::Vec2(Vec2Track{
            first:None,
            area_name:area_name.to_string(),
            prop_name:prop_name.to_string(),
            track:track
        })
    }

    pub fn vec3(area_name:&str, prop_name:&str, track:Vec<(f64,Vec3)>)->AnimTrack{
        AnimTrack::Vec3(Vec3Track{
            first:None,
            area_name:area_name.to_string(),
            prop_name:prop_name.to_string(),
            track:track
        })
    }

    pub fn vec4(area_name:&str, prop_name:&str, track:Vec<(f64,Vec4)>)->AnimTrack{
        AnimTrack::Vec4(Vec4Track{
            first:None,
            area_name:area_name.to_string(),
            prop_name:prop_name.to_string(),
            track:track
        })
    }

    fn compute_track_prop<T>(time:f64, prop_name:&str, area:&Area, cx:&mut Cx, track:&Vec<(f64,T)>, first:&mut Option<T>)
    where T:AccessInstanceProp<T> + Clone
    {
        if track.is_empty(){return }
        // find the 2 keys we want
        for i in 0..track.len(){
            if time>= track[i].0{ // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1{ // last key
                    val1.1.write_prop(cx, area, prop_name);
                    return
                }
                let val2 = &track[i+1];
                // lerp it
                let f = ((time - val1.0)/(val2.0-val1.0)) as f32;
                val1.1.lerp_prop(&val2.1, f).write_prop(cx, area, prop_name);
                return
            }
        }
        if first.is_none(){
            *first = Some(T::read_prop(cx, area, prop_name));
        }
        let val2 = &track[0];
        let val1 = first.as_mut().unwrap();
        let f = (time/val2.0) as f32;
        val1.lerp_prop(&val2.1, f).write_prop(cx, area, prop_name);
    }

    pub fn compute_and_write(&mut self, time:f64, area:&Area, cx:&mut Cx){
        match self{
            AnimTrack::Float(ft)=>{
                Self::compute_track_prop::<f32>(time, &ft.prop_name, area, cx, &ft.track, &mut ft.first);
            },
            AnimTrack::Vec2(ft)=>{
                Self::compute_track_prop::<Vec2>(time, &ft.prop_name, area, cx, &ft.track, &mut ft.first);
            }
            AnimTrack::Vec3(ft)=>{
                Self::compute_track_prop::<Vec3>(time, &ft.prop_name, area, cx, &ft.track, &mut ft.first);
            }
            AnimTrack::Vec4(ft)=>{
                Self::compute_track_prop::<Vec4>(time, &ft.prop_name, area, cx, &ft.track, &mut ft.first);
            }
        }
    }

    pub fn area_name(&mut self)->&String{
        match self{
            AnimTrack::Float(ft)=>{
                &ft.area_name
            },
            AnimTrack::Vec2(ft)=>{
                &ft.area_name
            }
            AnimTrack::Vec3(ft)=>{
                &ft.area_name
            }
            AnimTrack::Vec4(ft)=>{
                &ft.area_name
            }
        }
    }

    fn duration(&self)->f64{
        match self{
            AnimTrack::Float(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            },
            AnimTrack::Vec2(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            }
            AnimTrack::Vec3(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            }
            AnimTrack::Vec4(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            }
        }
    }
}

impl<T> AnimState<T>{
    pub fn new(id:T, mode:AnimMode, tracks:Vec<AnimTrack>)->AnimState<T>{
        AnimState{
            id:id,
            mode:mode,
            tracks:tracks
        }
    }
}

#[derive(Clone,Debug)]
pub enum AnimMode{
    Single{speed:f64, cut:bool, len:f64},
    Loop{speed:f64, cut:bool, repeats:f64,len:f64},
    Reverse{speed:f64, cut:bool, repeats:f64,len:f64},
    Bounce{speed:f64, cut:bool, repeats:f64,len:f64},
    Forever{speed:f64, cut:bool},
    LoopForever{speed:f64, cut:bool, len:f64},
    ReverseForever{speed:f64, cut:bool, len:f64},
    BounceForever{speed:f64, cut:bool, len:f64},
}

impl AnimMode{
    pub fn speed(&self)->f64{
        match self{
            AnimMode::Single{speed,..}=>*speed,
            AnimMode::Loop{speed,..}=>*speed,
            AnimMode::Reverse{speed,..}=>*speed,
            AnimMode::Bounce{speed,..}=>*speed,
            AnimMode::BounceForever{speed,..}=>*speed,
            AnimMode::Forever{speed,..}=>*speed,
            AnimMode::LoopForever{speed,..}=>*speed,
            AnimMode::ReverseForever{speed,..}=>*speed,
        }
    }
    pub fn duration(&self)->f64{
        match self{
            AnimMode::Single{len,speed,..}=>len*speed,
            AnimMode::Loop{len,speed,repeats,..}=>len*speed*repeats,
            AnimMode::Reverse{len,speed,repeats,..}=>len*speed*repeats,
            AnimMode::Bounce{len,speed,repeats,..}=>len*speed*repeats,
            AnimMode::BounceForever{..}=>std::f64::INFINITY,
            AnimMode::Forever{..}=>std::f64::INFINITY,
            AnimMode::LoopForever{..}=>std::f64::INFINITY,
            AnimMode::ReverseForever{..}=>std::f64::INFINITY,
        }
    }    
    pub fn cut(&self)->bool{
        match self{
            AnimMode::Single{cut,..}=>*cut,
            AnimMode::Loop{cut,..}=>*cut,
            AnimMode::Reverse{cut,..}=>*cut,
            AnimMode::Bounce{cut,..}=>*cut,
            AnimMode::BounceForever{cut,..}=>*cut,
            AnimMode::Forever{cut,..}=>*cut,
            AnimMode::LoopForever{cut,..}=>*cut,
            AnimMode::ReverseForever{cut,..}=>*cut,
        }
    }
    pub fn repeats(&self)->f64{
        match self{
            AnimMode::Single{..}=>1.0,
            AnimMode::Loop{repeats,..}=>*repeats,
            AnimMode::Reverse{repeats,..}=>*repeats,
            AnimMode::Bounce{repeats,..}=>*repeats,
            AnimMode::BounceForever{..}=>std::f64::INFINITY,
            AnimMode::Forever{..}=>std::f64::INFINITY,
            AnimMode::LoopForever{..}=>std::f64::INFINITY,
            AnimMode::ReverseForever{..}=>std::f64::INFINITY,
        }
    }
    
    pub fn compute_time(&self, time:f64)->(bool,f64){
        match self{
            AnimMode::Single{len,speed,..}=>{
                let local_time = time * speed;
                (local_time >= *len, local_time)
            },
            AnimMode::Loop{len,speed,repeats,..}=>{
                let loops = (time / speed) / len;
                let local_time = (time / speed)  % len;
                (loops > *repeats, local_time)
            },
            AnimMode::Reverse{len,speed,repeats,..}=>{
                let loops = (time / speed) / len;
                let local_time = len - (time / speed)  % len;
                (loops > *repeats, local_time)
            },
            AnimMode::Bounce{len,speed,repeats,..}=>{
                let loops = (time / speed) / len;
                let mut local_time = (time / speed)  % (len*2.0);
                if local_time > *len{
                    local_time = 2.0*len - local_time;
                };
                (loops > *repeats, local_time)
            },
            AnimMode::BounceForever{len,speed,..}=>{
                let mut local_time = (time / speed)  % (len*2.0);
                if local_time > *len{
                    local_time = 2.0*len - local_time;
                };
                (false, local_time)
            },
            AnimMode::Forever{speed,..}=>{
                let local_time = time / speed;
                (false, local_time)
            },
            AnimMode::LoopForever{len, speed, ..}=>{
                let local_time = (time / speed)  % len;
                (false, local_time)
            },
            AnimMode::ReverseForever{len, speed, ..}=>{
                let local_time = len - (time / speed)  % len;
                (false, local_time)
            },
        }
    }
}

trait AccessInstanceProp<T>{
    fn lerp_prop(&self, b:&T, f:f32)->T;
    fn write_prop(&self, cx:&mut Cx, area:&Area, prop_name:&str);
    fn read_prop(cx:&Cx, area:&Area, prop_name:&str)->T;
}

impl AccessInstanceProp<f32> for f32{
    fn lerp_prop(&self, b:&f32, f:f32)->f32{
        *self + (*b - *self) * f
    }

    fn write_prop(&self, cx:&mut Cx, area:&Area, prop_name:&str){
        area.write_prop_float(cx, prop_name, *self);
    }

    fn read_prop(cx:&Cx, area:&Area, prop_name:&str)->f32{
        area.read_prop_float(cx, prop_name)
    }
}

impl AccessInstanceProp<Vec2> for Vec2{
    fn lerp_prop(&self, b:&Vec2, f:f32)->Vec2{
        Vec2{
            x:self.x + (b.x - self.x) * f,
            y:self.y + (b.y - self.y) * f
        }
    }

    fn write_prop(&self, cx:&mut Cx, area:&Area, prop_name:&str){
        area.write_prop_vec2(cx, prop_name, self);
    }

    fn read_prop(cx:&Cx, area:&Area, prop_name:&str)->Vec2{
        area.read_prop_vec2(cx, prop_name)
    }
}

impl AccessInstanceProp<Vec3> for Vec3{
    fn lerp_prop(&self, b:&Vec3, f:f32)->Vec3{
        Vec3{
            x:self.x + (b.x - self.x) * f,
            y:self.y + (b.y - self.y) * f,
            z:self.z + (b.z - self.z) * f
        }
    }

    fn write_prop(&self, cx:&mut Cx, area:&Area, prop_name:&str){
        area.write_prop_vec3(cx, prop_name, self);
    }

    fn read_prop(cx:&Cx, area:&Area, prop_name:&str)->Vec3{
        area.read_prop_vec3(cx, prop_name)
    }

}

impl AccessInstanceProp<Vec4> for Vec4{
    fn lerp_prop(&self, b:&Vec4, f:f32)->Vec4{
        let of = 1.0-f;
        Vec4{
            x:self.x * of + b.x * f,
            y:self.y * of + b.y * f,
            z:self.z * of + b.z * f,
            w:self.w * of + b.w * f
        }
    }

    fn write_prop(&self, cx:&mut Cx, area:&Area, prop_name:&str){
        area.write_prop_vec4(cx, prop_name, self);
    }

    fn read_prop(cx:&Cx, area:&Area, prop_name:&str)->Vec4{
        area.read_prop_vec4(cx, prop_name)
    }
}