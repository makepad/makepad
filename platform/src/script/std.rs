use crate::*;
use makepad_script::*;
use makepad_script::id;
use crate::script::vm::*;
use std::rc::Rc;
use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};
use std::fmt::Debug;
use std::collections::VecDeque;

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

#[derive(Clone, Debug)]
pub struct CxScriptChannel{
    pub handle: ScriptHandle,
    pub array_ref: ScriptArrayRef,
    pub max_depth: usize,
    pub closed: bool,
    pub send_pause: VecDeque<ScriptThreadId>,
    pub recv_pause: VecDeque<ScriptThreadId>,
}

#[derive(Default)]
pub struct CxScriptChannels{
    pub channels: Rc<RefCell<Vec<CxScriptChannel>>>,
}

// this is a UI-thread pipe
#[derive(Debug)]
pub struct CxScriptChannelGc{
    pub channels: Rc<RefCell<Vec<CxScriptChannel>>>,
    pub handle: ScriptHandle,
}

impl ScriptHandleGc for CxScriptChannelGc{
    fn gc(&mut self){
        self.channels.borrow_mut().retain(|v| v.handle != self.handle)
    }
    fn set_handle(&mut self, handle:ScriptHandle){
        self.handle = handle
    }
    fn debug_fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result{
        self.fmt(f)
    }
}

impl Cx{
    pub(crate) fn handle_script_channels(&mut self){
        loop{
            let mut next_thread = None;
            let mut channels = self.script_data.channels.channels.borrow_mut();
            for channel in channels.iter_mut(){
                // alright lets check each channels array len and if they are waiting
                // ifso we call that thread
                let array = channel.array_ref.as_array();
                
                let array_len = self.script_vm.as_ref().unwrap().heap.array_len(array);
                if channel.recv_pause.len()>0 && array_len > 0{
                    next_thread = channel.recv_pause.pop_back();
                    break;
                }
                if channel.send_pause.len()>0 && array_len<channel.max_depth{
                    next_thread = channel.send_pause.pop_back();
                    break;
                }                
            }
            drop(channels);
            // alright execute this thread
            if let Some(next_thread) = next_thread.take(){
                self.with_vm_thread(next_thread, |vm|{
                    vm.resume();
                });
            }
            else{
                break
            }
        }
    }
    
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
            self.with_vm_and_async(|vm|{
                vm.call(callback.into(), &[time]);
            })
        }
    }
}

pub fn extend_std_module(vm:&mut ScriptVm){
    let std = vm.module(id!(std));
    
    let channel_type = vm.new_handle_type(id_lut!(channel));
    
    for fn_id in [id_lut!(send), id_lut!(close)]{
        vm.add_handle_method(channel_type, fn_id, script_args_def!(), move |vm, args|{
            if let Some(handle) = script_value!(vm, args.this).as_handle(){
                let cx = vm.host.cx_mut();
                if let Some(chan) = cx.script_data.channels.channels.borrow_mut().iter_mut().find(|v| v.handle == handle){
                    let array_len = vm.heap.array_len(chan.array_ref.as_array());
                    
                    if chan.max_depth == 0 || array_len < chan.max_depth{
                        if vm.heap.vec_len(args.into()) == 1{
                            let value = vm.heap.vec_value(args, 0, &vm.thread.trap);
                            vm.heap.array_push(chan.array_ref.as_array(), value, &vm.thread.trap);
                        }
                        else{
                            vm.heap.array_push(chan.array_ref.as_array(), args.into(), &vm.thread.trap);
                        }
                        if fn_id == id!(close){
                            chan.closed = true;
                        }
                        return ((array_len + 1) as f64).into()
                    }
                    else {
                        if chan.send_pause.len() > 100{
                            return vm.thread.trap.err_too_many_paused_calls()
                        }
                        chan.send_pause.push_front(vm.thread.pause());
                        return NIL
                    }
                }
            }
            NIL
        });
    }
    vm.add_handle_method(channel_type, id_lut!(recv), script_args_def!(), |vm, args|{
        // lets find the channel
        if let Some(handle) = script_value!(vm, args.this).as_handle(){
            let cx = vm.host.cx_mut();
            if let Some(chan) = cx.script_data.channels.channels.borrow_mut().iter_mut().find(|v| v.handle == handle){
                if let Some(value) = vm.heap.array_pop_front_option(chan.array_ref.as_array()){
                    return value
                }
                else if chan.closed{
                    return NIL
                }
                else{
                    if chan.recv_pause.len() > 100{
                        return vm.thread.trap.err_too_many_paused_calls()
                    }
                    chan.recv_pause.push_front(vm.thread.pause());
                    return NIL
                }
            }
        }
        vm.thread.trap.err_unexpected()
    });
    vm.add_handle_method(channel_type, id_lut!(wait), script_args_def!(), |vm, args|{
        // lets find the channel
        if let Some(handle) = script_value!(vm, args.this).as_handle(){
            let cx = vm.host.cx_mut();
            if let Some(chan) = cx.script_data.channels.channels.borrow_mut().iter_mut().find(|v| v.handle == handle){
                vm.heap.array_clear(chan.array_ref.as_array(), &vm.thread.trap);
                if chan.closed{
                    return NIL
                }
                else{
                    if chan.recv_pause.len() > 100{
                        return vm.thread.trap.err_too_many_paused_calls()
                    }
                    chan.recv_pause.push_front(vm.thread.pause());
                    return NIL
                }
            }
        }
        vm.thread.trap.err_unexpected()
    });
    
    vm.set_handle_getter(channel_type, |vm, this, prop|{
        // lets find the channel
        if prop == id!(array){
            if let Some(handle) = this.as_handle(){
                let cx = vm.host.cx_mut();
                if let Some(chan) = cx.script_data.channels.channels.borrow_mut().iter_mut().find(|v| v.handle == handle){
                    return chan.array_ref.as_array().into()
                }
            }
        }
        vm.thread.trap.err_invalid_prop_name()
    });
    
    vm.add_method(std, id_lut!(channel), script_args_def!(max_depth = NIL), move |vm, args|{
        // lets make a new channel
        let max_depth = script_value_f64!(vm, args.max_depth);
        let cx = vm.host.cx_mut();
        let handle_gc = CxScriptChannelGc{
            channels: cx.script_data.channels.channels.clone(),
            handle: ScriptHandle::ZERO
        };
        let handle = vm.heap.new_handle(channel_type, Box::new(handle_gc));
        let array = vm.heap.new_array();
        let array_ref = vm.heap.new_array_ref(array);
        cx.script_data.channels.channels.borrow_mut().push(
            CxScriptChannel{
                max_depth: max_depth as usize,
                handle,
                closed: false,
                recv_pause: Default::default(),
                send_pause: Default::default(),
                array_ref,
            }
        );
        
        handle.into()
    });
        
    vm.new_handle_type(id_lut!(timer));
    
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
    
    vm.add_method(std, id_lut!(random_seed), script_args_def!(), |vm, _args|{
        let start = SystemTime::now();
        let since_the_epoch = start.duration_since(UNIX_EPOCH).unwrap();
        let nanos = since_the_epoch.as_nanos();
        let cx = vm.cx_mut();
        cx.script_data.random_seed = (nanos >>64)as u64 ^ (nanos as u64);
        NIL
    });
    
    vm.add_method(std, id_lut!(random), script_args_def!(), |vm, _args|{
        let cx = vm.cx_mut();
        let seed = cx.script_data.random_seed;
        let seed = next_hash(&seed.to_ne_bytes());
        cx.script_data.random_seed = seed;
        ((seed as f64) / u64::MAX as f64).into()
    });
    
    vm.add_method(std, id_lut!(random_u32), script_args_def!(), |vm, _args|{
        let cx = vm.cx_mut();
        let seed = cx.script_data.random_seed;
        let seed = next_hash(&seed.to_ne_bytes());
        cx.script_data.random_seed = seed;
        (seed as u32 as f64).into()
    });
    
    vm.add_method(std, id_lut!(start_timeout), script_args_def!(delay=NIL, callback=NIL), |vm, args|{
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
    
    vm.add_method(std, id_lut!(start_interval), script_args_def!(delay=NIL, callback=NIL), |vm, args|{
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
    
    vm.add_method(std, id_lut!(stop_timer), script_args_def!(timer=NIL), |vm, args|{
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