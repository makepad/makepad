use crate::*;
use makepad_script::*;
use makepad_script::id;



pub fn define_net_module(vm:&mut ScriptVm){
    let net = vm.new_module(id!(net));
    vm.add_fn(net, id!(fetch), args!(url=NIL, options=NIL), |_vm, _args|{
        // we have an options object
        /*
        FetchOptions::from_object(vm, value!(vm, args.options));
                
        let mut request = HttpRequest::new(completion_url, HttpMethod::POST);
        
        
        vm.cx();
        */
        NIL
    });
}
