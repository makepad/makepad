use crate::*;
use makepad_script::*;
use makepad_script::id;

pub fn define_net_module(vm:&mut ScriptVm){
    let net = vm.new_module(id_lut!(net));
    
    script_proto!(vm, net, HttpRequest);
    script_proto!(vm, net, HttpMethod);
    
    vm.add_fn(net, id!(http_request), script_args_def!(options=NIL), move |vm, args|{
        let options =  script_value!(vm, args.options);
        // we should check if options is actually of type HttpRequest
        if !script_has_proto!(vm, options, net.HttpRequest){
            return vm.thread.trap.err_invalid_arg_type()
        }
        
        let _req = HttpRequest::script_from_value(vm, options);
        // alright! now what.
        
        
        
        NIL
    })
}
