use crate::*;
use crate::script::vm::*;
use makepad_script::*;
use makepad_script::id;
use crate::event::network::*;
use crate::web_socket::*;

pub struct CxScriptWebSocket{
    #[allow(unused)]
    id: LiveId,
    socket: WebSocket,
    events: WebSocketEvents,
}

pub struct CxScriptHttp{
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
        let mut i = 0;
        while i<self.script_data.web_sockets.len(){
            match self.script_data.web_sockets[i].socket.try_recv(){
                Ok(WebSocketMessage::String(s))=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_string.as_obj(){
                        self.with_vm(|vm|{
                            let str = vm.heap.new_string_from_str(&s);
                            vm.call(handler.into(), &[str.into()]);
                        })
                    }
                    i += 1;
                }
                Ok(WebSocketMessage::Binary(s))=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_string.as_obj(){
                        self.with_vm(|vm|{
                            let array = vm.heap.new_array_from_vec_u8(s);
                            vm.call(handler.into(), &[array.into()]);
                        })
                    }
                    i += 1;
                }
                Ok(WebSocketMessage::Opened)=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_opened.as_obj(){
                        self.with_vm(|vm|{
                            vm.call(handler.into(), &[]);
                        })
                    }
                    i += 1;
                }
                Ok(WebSocketMessage::Closed)=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_closed.as_obj(){
                        self.with_vm(|vm|{
                            vm.call(handler.into(), &[]);
                        })
                    }
                    self.script_data.web_sockets.remove(i);
                }
                Ok(WebSocketMessage::Error(s))=>{
                    if let Some(handler) = self.script_data.web_sockets[i].events.on_string.as_obj(){
                        self.with_vm(|vm|{
                            let str = vm.heap.new_string_from_str(&s);
                            vm.call(handler.into(), &[str.into()]);
                        })
                    }
                    i += 1;
                }
                Err(_)=>{i += 1;}
            }
            
        }
    }    
    
    pub(crate) fn handle_script_network_events(&mut self, items: &[NetworkResponseItem]){

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
                    if let Some(i) = self.script_data.http_requests.iter().position(|v| v.id == item.request_id){
                        if let Some(handler) = self.script_data.http_requests[i].events.on_complete.as_obj(){
                            self.with_vm(|vm|{
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                        self.script_data.http_requests.remove(i);
                    }
                }
                NetworkResponse::HttpResponse(res) => {
                    if let Some(i) = self.script_data.http_requests.iter().position(|v| v.id == item.request_id){
                        if let Some(handler) = self.script_data.http_requests[i].events.on_response.as_obj(){
                            self.with_vm(|vm|{
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                        self.script_data.http_requests.remove(i);
                    }
                }
                NetworkResponse::HttpRequestError(err) => {
                    if let Some(i) = self.script_data.http_requests.iter().position(|v| v.id == item.request_id){
                        if let Some(handler) = self.script_data.http_requests[i].events.on_error.as_obj(){
                            self.with_vm(|vm|{
                                let res = err.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                        self.script_data.http_requests.remove(i);
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
        cx.script_data.http_requests.push(CxScriptHttp{
            id,
            events
        });
        cx.http_request(id, request);
        id.escape()
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
        let id = LiveId::unique();
        cx.script_data.web_sockets.push(CxScriptWebSocket{
            socket: WebSocket::open(request),
            id,
            events
        });
        id.escape()
    });
}
