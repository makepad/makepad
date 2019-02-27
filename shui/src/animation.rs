use crate::math::*;
use crate::colors::*;
use crate::cxturtle::*;
use crate::cx::*;
use crate::events::*;

#[derive(Clone)]
pub struct AnimStates<T>{
    pub state:Option<T>,
    pub default:T,
    pub states:Vec<AnimState<T>>
}

impl<T> AnimStates<T>{

    pub fn new(default:T, states:Vec<AnimState<T>>)->AnimStates<T>{
        AnimStates{
            state:None,
            default:default,
            states:states
        }
    }

    pub fn set_state(&mut self, cx:&mut Cx, state:T, area:&Area){
        
/*
        let anim_state_opt = self.states.iter().find(|v| v.state_name == state_name);

        if anim_state_opt.is_none(){
            println!("Starting animation state {} does not exist in states", state_name);
            return;
        }
        let anim_state = anim_state_opt.unwrap();

        // alright first we find area
        if let Some(anim) = cx.animations.iter_mut().find(|v| v.area == *area){
            //do we queue or replace
            match anim_state.anim_start{
                AnimStart::Interrupt=>{
                    anim.start = std::f64::NAN;
                    anim.current_state = state_name.to_string();
                    anim.next_state = "".to_string();
                },
                AnimStart::Queue=>{
                    anim.next_state = state_name.to_string();
                    // lets add up the durations of both states
                    let prev_anim_state = self.states.iter().find(|v| v.state_name == anim.current_state).unwrap();
                    anim.duration = anim_state.duration + prev_anim_state.duration;
                }
            }
        }
        else{
            cx.animations.push(AnimArea{
                area:area.clone(),
                start:std::f64::NAN,
                duration:anim_state.duration,
                current_state:state_name.to_string(),
                next_state:"".to_string()
            })*/
    }

    pub fn animate(&mut self, cx: &mut Cx, _area_name:&str,  id_area:&Area, _tgt_area:&Area, ae:&AnimateEvent){
        // alright we need to compute an animation. 
        // ok so first we use the id area to fetch the animation
        // alright first we find area
        let anim_opt = cx.animations.iter_mut().find(|v| v.area == *id_area);
        if anim_opt.is_none(){
            return
        }
        let _anim = anim_opt.unwrap();
    
        // so if the start is NAN we are the first time called.

        // ok so, if we are interrupting 

    }    
}

#[derive(Clone,Debug)]
pub enum AnimData{
    NotSet,
    Vec4(Vec4),
    Float(f32)
}

#[derive(Clone,Debug)]
pub struct AnimValue{
    pub area_name:String,
    pub value_name:String,
    pub new_data:AnimData,
    pub old_data:AnimData
}

impl AnimValue{
    pub fn color(area_name:&str, value_name:&str, value:&str)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Vec4(color(value)),
            old_data:AnimData::NotSet
        }
    }
    pub fn vec4f(area_name:&str, value_name:&str, x:f32, y:f32, z:f32, w:f32)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Vec4(vec4(x,y,z,w)),
            old_data:AnimData::NotSet
        }
    }
    pub fn vec4(area_name:&str, value_name:&str, v:Vec4)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Vec4(v),
            old_data:AnimData::NotSet
        }
    }
   pub fn float(area_name:&str, value_name:&str, v:f32)->AnimValue{
        AnimValue{
            area_name:area_name.to_string(),
            value_name:value_name.to_string(),
            new_data:AnimData::Float(v),
            old_data:AnimData::NotSet
        }
    }

}

#[derive(Clone,Debug)]
pub struct AnimKey{
    pub time:f64,
    pub values:Vec<AnimValue>
}

impl AnimKey{
    pub fn new( time:f64, values:Vec<AnimValue>)->AnimKey{
        AnimKey{
            time:time,
            values:values
        }
    }
}

#[derive(Clone,Debug)]
pub enum AnimStart{
    Queue,
    Interrupt
}

impl Default for AnimStart{
    fn default()->AnimStart{
        AnimStart::Queue
    }
}

#[derive(Clone,Debug)]
pub enum AnimDuration{
    Seconds(f64),
    Forever
}

#[derive(Clone,Debug)]
pub struct AnimState<T>{
    pub state:T,
    pub duration:AnimDuration,
    pub anim_start:AnimStart,
    pub keys:Vec<AnimKey>
}

impl<T> AnimState<T>{
    pub fn new(state:T, duration:AnimDuration, anim_start:AnimStart, keys:Vec<AnimKey>)->AnimState<T>{
        AnimState{
            state:state,
            duration:duration,
            anim_start:anim_start,
            keys:keys
        }
    }
}

#[derive(Clone,Debug,PartialEq)]
pub struct AnimArea{
    pub area:Area,
    pub start:f64,
    pub duration:f64,
    pub current_state:String,
    pub next_state:String
}
