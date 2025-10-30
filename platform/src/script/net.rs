use crate::*;
use makepad_script::*;
use makepad_script::id;

pub fn define_net_module(vm:&mut ScriptVm){
    let net = vm.new_module(id_lut!(net));
    
    script_proto!(vm, net, HttpRequest);
    script_proto!(vm, net, HttpMethod);
    
    vm.add_fn(net, id!(http_request), script_args_def!(url=NIL, options=NIL), |vm, args|{
        let options =  script_value!(vm, args.options);
        let _req = HttpRequest::script_from_value(vm, options);
        NIL
    })
}
