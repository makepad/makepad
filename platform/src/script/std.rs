
use crate::*;
use makepad_script::*;
use makepad_script::id;
use crate::script::vm::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct CxScriptTimer{
    pub id: LiveId,
    pub repeat: bool,
    pub timer: Timer,
    pub callback: ScriptFnRef
}

#[derive(Clone, Default)]
pub struct CxScriptTimers{
    pub timers: Vec<CxScriptTimer>,
}

impl Cx{
    pub(crate) fn handle_script_timer(&mut self, event:&TimerEvent){
        if let Some(i) = self.script_data.timers.timers.iter().position(|v| v.timer.is_timer(event).is_some()){
            let timer = &self.script_data.timers.timers[i];
            let callback = timer.callback.as_obj();
            if !timer.repeat{
                self.script_data.timers.timers.remove(i);
            }
            let time = if let Some(time) = event.time{
                time.into()
            }
            else{
                NIL
            };
            self.with_vm(|vm|{
                vm.call(callback.into(), &[time]);
            })
        }
    }
}

pub fn extend_std_module(vm:&mut ScriptVm){
    let std = vm.module(id!(std));
    
    pub fn next_hash(bytes: &[u8;8]) -> u64 {
        let mut x:u64 = 0xd6e8_feb8_6659_fd93;
        let mut i = 0;
        while i < 8 {
            x = x.overflowing_add(bytes[i] as u64).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            i += 1;
        }
        x
    }
    
    vm.add_fn(std, id!(random_seed), script_args_def!(), |vm, _args|{
        let start = SystemTime::now();
        let since_the_epoch = start.duration_since(UNIX_EPOCH).unwrap();
        let nanos = since_the_epoch.as_nanos();
        let cx = vm.cx_mut();
        cx.script_data.random_seed = (nanos >>64)as u64 ^ (nanos as u64);
        NIL
    });
    
    vm.add_fn(std, id!(random), script_args_def!(), |vm, _args|{
        let cx = vm.cx_mut();
        let seed = cx.script_data.random_seed;
        let seed = next_hash(&seed.to_ne_bytes());
        cx.script_data.random_seed = seed;
        ((seed as f64) / u64::MAX as f64).into()
    });
    
    vm.add_fn(std, id!(random_u32), script_args_def!(), |vm, _args|{
        let cx = vm.cx_mut();
        let seed = cx.script_data.random_seed;
        let seed = next_hash(&seed.to_ne_bytes());
        cx.script_data.random_seed = seed;
        (seed as u32 as f64).into()
    });
    
    vm.add_fn(std, id!(start_timeout), script_args_def!(delay=NIL, callback=NIL), |vm, args|{
        let delay = script_value!(vm, args.delay);
        let callback = script_value!(vm, args.callback);
        
        if !delay.is_number() || !vm.heap.is_fn(callback.into()){
            return vm.thread.trap.err_invalid_arg_type()
        }
        let callback = ScriptFnRef::script_from_value(vm, callback);
        
        let cx = vm.cx_mut();
        let timer = cx.start_timeout(delay.as_f64().unwrap_or(1.0));
        
        let id = LiveId::unique();
        cx.script_data.timers.timers.push(CxScriptTimer{
            repeat: false,
            timer,
            id,
            callback
        });
        id.escape()
    });
    
    vm.add_fn(std, id!(start_interval), script_args_def!(delay=NIL, callback=NIL), |vm, args|{
        let delay = script_value!(vm, args.delay);
        let callback = script_value!(vm, args.callback);
                
        if !delay.is_number() || !ScriptFnRef::script_type_check(vm.heap, callback){
            return vm.thread.trap.err_invalid_arg_type()
        }
        let callback = ScriptFnRef::script_from_value(vm, callback);
                
        let cx = vm.cx_mut();
                
        let timer = cx.start_interval(delay.as_f64().unwrap_or(1.0));
                        
        let id = LiveId::unique();
        cx.script_data.timers.timers.push(CxScriptTimer{
            repeat: true,
            timer,
            id,
            callback
        });
        id.escape()
    });
    
    vm.add_fn(std, id!(stop_timer), script_args_def!(timer=NIL), |vm, args|{
        let timer = script_value!(vm, args.timer);
        if !timer.is_id(){ 
            return vm.thread.trap.err_invalid_arg_type()
        }
        let timer = timer.as_id().unwrap_or(id!());
        let cx = vm.cx_mut();
        cx.script_data.timers.timers.retain(|v| v.id != timer);
        NIL
    });
    
}