use crate::*;
use makepad_script::*;
use makepad_script::id;



pub fn define_net_module(vm:&mut ScriptVm){
    let net = vm.new_module(id!(net));
    
    let req = HttpRequest::script_proto(vm);
    vm.heap.set_value_def(net, id!(HttpRequest).into(), req);
    
    vm.add_fn(net, id!(fetch), script_args!(url=NIL, options=NIL), |vm, args|{
        let options =  script_value!(vm, args.options);
        let _req = HttpRequest::script_from_value(vm, options);
        
        //FetchOptions::new(vm,options);
        // we have an options object
        /*
        FetchOptions::from_object(vm, value!(vm, args.options));
        let mut request = HttpRequest::new(completion_url, HttpMethod::POST);
        vm.cx();
        */
        NIL
    })
}
