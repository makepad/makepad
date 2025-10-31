use crate::*;
use crate::script::vm::*;
use makepad_script::*;
use makepad_script::id;
use crate::event::network::*;
use crate::web_socket::*;

pub struct CxScriptDataWebSocket{
    socket: WebSocket,
    events: WebSocketEvents,
}

pub struct CxScriptDataHttp{
    pub id: LiveId,
    pub events: HttpEvents,
}

#[derive(Script, ScriptHook)]
pub struct HttpEvents{
    #[live] pub on_stream: Option<ScriptFnRef>,
    #[live] pub on_response: Option<ScriptFnRef>,
    #[live] pub on_complete: Option<ScriptFnRef>,
    #[live] pub on_error: Option<ScriptFnRef>,
    #[live] pub on_progress: Option<ScriptFnRef>,
}

#[derive(Script, ScriptHook)]
pub struct WebSocketEvents{
    #[live] pub on_opened: Option<ScriptFnRef>,
    #[live] pub on_closed: Option<ScriptFnRef>,
    #[live] pub on_binary: Option<ScriptFnRef>,
    #[live] pub on_string: Option<ScriptFnRef>,
    #[live] pub on_error: Option<ScriptFnRef>,
}

impl Cx{
    pub(crate) fn handle_script_signals(&mut self){
        for i in 0..self.script_data.web_sockets.len(){
            match self.script_data.web_sockets[i].socket.try_recv(){
                Ok(WebSocketMessage::String(s))=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_string.as_obj(){
                        self.with_vm(|vm|{
                            let str = vm.heap.new_string_from_str(&s);
                            vm.call(handler.into(), &[str.into()]);
                        })
                    }
                }
                Ok(WebSocketMessage::Binary(s))=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_string.as_obj(){
                        self.with_vm(|vm|{
                            let array = vm.heap.new_array_from_vec_u8(s);
                            vm.call(handler.into(), &[array.into()]);
                        })
                    }
                }
                Ok(WebSocketMessage::Opened)=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_opened.as_obj(){
                        self.with_vm(|vm|{
                            vm.call(handler.into(), &[]);
                        })
                    }
                }
                Ok(WebSocketMessage::Closed)=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_closed.as_obj(){
                        self.with_vm(|vm|{
                            vm.call(handler.into(), &[]);
                        })
                    }
                }
                Ok(WebSocketMessage::Error(s))=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_string.as_obj(){
                        self.with_vm(|vm|{
                            let str = vm.heap.new_string_from_str(&s);
                            vm.call(handler.into(), &[str.into()]);
                        })
                    }
                }
                Err(_)=>{}
            }
            
        }
    }    
    
    pub(crate) fn handle_script_async_network_responses(&mut self, items: &[NetworkResponseItem]){
        for item in items{
            match &item.response {
                NetworkResponse::HttpStreamResponse(res)=>{
                    if let Some(s) = self.script_data.http_requests.iter().find(|v| v.id == item.request_id){
                        if let Some(handler) = s.events.on_stream.as_obj(){
                            self.with_vm(|vm|{
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                    }
                }
                NetworkResponse::HttpStreamComplete(res)=>{
                    if let Some(s) = self.script_data.http_requests.iter().find(|v| v.id == item.request_id){
                        if let Some(handler) = s.events.on_complete.as_obj(){
                            self.with_vm(|vm|{
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                    }
                }
                NetworkResponse::HttpResponse(res) => {
                    if let Some(s) = self.script_data.http_requests.iter().find(|v| v.id == item.request_id){
                        if let Some(handler) = s.events.on_response.as_obj(){
                            self.with_vm(|vm|{
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                    }
                }
                NetworkResponse::HttpRequestError(err) => {
                    if let Some(s) = self.script_data.http_requests.iter().find(|v| v.id == item.request_id){
                        if let Some(handler) = s.events.on_error.as_obj(){
                            self.with_vm(|vm|{
                                let res = err.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                    }
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

    vm.add_fn(net, id!(http_request), script_args_def!(request=NIL, events=NIL), move |vm, args|{
        let request = script_value!(vm, args.request);
        let events = script_value!(vm, args.events);
        // we should check if options is actually of type HttpRequest
        if !script_has_proto!(vm, request, net.HttpRequest) || 
            !script_has_proto!(vm, events, net.HttpEvents) {
            return vm.thread.trap.err_invalid_arg_type()
        }
        let request = HttpRequest::script_from_value(vm, request);
        let events = HttpEvents::script_from_value(vm, events);
        // alright now what
        let cx = vm.cx_mut();
        let id = LiveId::unique();
        cx.script_data.http_requests.push(CxScriptDataHttp{
            id,
            events
        });
        cx.http_request(id, request);
        NIL
    });
    
    script_proto!(vm, net, WebSocketEvents);
        
    vm.add_fn(net, id!(web_socket), script_args_def!(request=NIL, events=NIL), move |vm, args|{
        let request = script_value!(vm, args.request);
        let events = script_value!(vm, args.events);
        // we should check if options is actually of type HttpRequest
        
        let request = if request.is_string_like(){
            vm.heap.string_with(request, |_heap, s|{
                HttpRequest{
                    url: s.to_string(),
                    ..Default::default()
                }
            }).unwrap()
        }
        else{
            if !script_has_proto!(vm, request, net.HttpRequest){
                return vm.thread.trap.err_invalid_arg_type()
            }
            HttpRequest::script_from_value(vm, request)
        };
        
        if !script_has_proto!(vm, events, net.WebSocketEvents) {
            return vm.thread.trap.err_invalid_arg_type()
        }
        let events = WebSocketEvents::script_from_value(vm, events);
        
        // alright now what
        let cx = vm.cx_mut();
        cx.script_data.web_sockets.push(CxScriptDataWebSocket{
            socket: WebSocket::open(request),
            events
        });
        NIL
    });
}
