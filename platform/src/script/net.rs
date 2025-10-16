use crate::*;
use crate::script::vm::*;
use makepad_script::*;
use makepad_script::id;

pub fn define_net_module(vm:&mut ScriptVm){
    let net = vm.new_module(id!(net));
    vm.add_fn(net, id!(fetch), args!(url:NIL, options:NIL), |vm, _args|{
        if let Some(_cx) = vm.cx(){
                        
        }
        NIL
    });
}
