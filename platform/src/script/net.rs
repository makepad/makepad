use crate::*;
use crate::script::vm::*;
use makepad_script::*;
use makepad_script::id;
use crate::event::network::*;

pub struct CxScriptDataHttp{
    pub id: LiveId,
    pub events: HttpEvents,
}

#[derive(Script, ScriptHook)]
pub struct HttpEvents{
    #[live] pub on_stream: Option<ScriptFnRef>,
    #[live] pub on_error: Option<ScriptFnRef>,
}

impl Cx{
    pub(crate) fn handle_script_async_network_responses(&mut self, items: &[NetworkResponseItem]){
        for item in items{
            match &item.response {
                NetworkResponse::HttpStreamResponse(res)=>{
                    if let Some(s) = self.script_data.http_requests.iter().find(|v| v.id == item.request_id){
                        // alright lets call the script engine with our stream response
                        if let Some(on_stream) = s.events.on_stream.as_obj(){
                            self.with_vm(|vm|{
                                let res = res.script_to_value(vm);
                                vm.call(on_stream.into(), &[res]);
                            })
                        }
                    }
                }
                NetworkResponse::HttpStreamComplete(_res)=>{
                }
                NetworkResponse::HttpResponse(_res) => {
                }
                NetworkResponse::HttpRequestError(_err) => {
                }
                NetworkResponse::HttpProgress(_p)=>{
                }
            }
        } 
    }
}

pub fn define_net_module(vm:&mut ScriptVm){
    let net = vm.new_module(id_lut!(net));
    
    script_proto!(vm, net, HttpRequest);
    script_proto!(vm, net, HttpMethod);
    script_proto!(vm, net, HttpEvents);
        
    vm.add_fn(net, id!(http_request), script_args_def!(options=NIL), move |vm, args|{
        let options =  script_value!(vm, args.options);
        // we should check if options is actually of type HttpRequest
        if !script_has_proto!(vm, options, net.HttpRequest){
            return vm.thread.trap.err_invalid_arg_type()
        }
        let mut req = HttpRequest::script_from_value(vm, options);
        req.is_streaming = false;
        // alright now what
        let cx = vm.cx_mut();
        cx.http_request(LiveId::unique(), req);
        NIL
    });
    
    vm.add_fn(net, id!(http_request_stream), script_args_def!(request=NIL, events=NIL), move |vm, args|{
        let request = script_value!(vm, args.request);
        let events = script_value!(vm, args.events);
        // we should check if options is actually of type HttpRequest
        if !script_has_proto!(vm, request, net.HttpRequest) || 
            !script_has_proto!(vm, events, net.HttpEvents) {
            return vm.thread.trap.err_invalid_arg_type()
        }
        let mut request = HttpRequest::script_from_value(vm, request);
        let events = HttpEvents::script_from_value(vm, events);
        request.is_streaming = true;
        // alright now what
        let cx = vm.cx_mut();
        let id = LiveId::unique();
        cx.script_data.http_requests.push(CxScriptDataHttp{
            id,
            events
        });
        cx.http_request(id, request);
        NIL
    })
}
