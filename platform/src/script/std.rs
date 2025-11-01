
use crate::*;
use makepad_script::*;
use makepad_script::id;
use crate::script::vm::*;

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