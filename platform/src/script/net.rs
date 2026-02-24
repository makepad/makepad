use crate::makepad_network::{
    http_server::{HttpServer, HttpServerRequest, HttpServerResponse},
    utils::HttpServerHeaders,
    HttpMethod, HttpRequest, NetworkResponse, WsMessage,
};
use crate::script::vm::*;
use crate::thread::*;
use crate::*;
use makepad_script::id;
use makepad_script::*;
use std::sync::mpsc::channel;

pub struct CxScriptWebSocket {
    #[allow(unused)]
    id: LiveId,
    socket_id: LiveId,
    events: WebSocketEvents,
}

pub struct CxScriptServerWebSocket {
    pub web_socket_id: u64,
    #[allow(unused)]
    pub response_sender: std::sync::mpsc::Sender<Vec<u8>>,
    pub events: WebSocketEvents,
}

pub struct CxScriptHttp {
    pub id: LiveId,
    pub events: HttpEvents,
}

pub struct CxScriptHttpServer {
    pub id: LiveId,
    pub receiver: ToUIReceiver<HttpServerRequest>,
    pub events: HttpServerEvents,
    pub web_sockets: Vec<CxScriptServerWebSocket>,
}

#[derive(Script, ScriptHook)]
pub struct HttpServerOptions {
    #[live]
    pub listen: String,
}

#[derive(Script, ScriptHook)]
pub struct HttpServerEvents {
    #[live]
    pub on_get: Option<ScriptFnRef>,
    #[live]
    pub on_post: Option<ScriptFnRef>,
    #[live]
    pub on_connect_websocket: Option<ScriptFnRef>,
}

#[derive(Script, ScriptHook)]
pub struct HttpEvents {
    #[live]
    pub on_stream: Option<ScriptFnRef>,
    #[live]
    pub on_response: Option<ScriptFnRef>,
    #[live]
    pub on_complete: Option<ScriptFnRef>,
    #[live]
    pub on_error: Option<ScriptFnRef>,
    #[live]
    pub on_progress: Option<ScriptFnRef>,
}

#[derive(Script, ScriptHook)]
pub struct WebSocketEvents {
    #[live]
    pub on_opened: Option<ScriptFnRef>,
    #[live]
    pub on_closed: Option<ScriptFnRef>,
    #[live]
    pub on_binary: Option<ScriptFnRef>,
    #[live]
    pub on_string: Option<ScriptFnRef>,
    #[live]
    pub on_error: Option<ScriptFnRef>,
}

impl Cx {
    pub(crate) fn handle_script_signals(&mut self) {
        self.handle_script_child_processes();
        self.handle_script_http_servers();
    }

    pub(crate) fn handle_script_http_servers(&mut self) {
        let mut i = 0;
        while i < self.script_data.http_servers.len() {
            while let Ok(msg) = self.script_data.http_servers[i].receiver.try_recv() {
                let server = &mut self.script_data.http_servers[i];
                match msg {
                    HttpServerRequest::ConnectWebSocket {
                        web_socket_id,
                        headers,
                        response_sender,
                    } => {
                        let handler = server.events.on_connect_websocket.clone();
                        if let Some(handler) = handler.as_object() {
                            self.with_vm_and_async(|vm| {
                                let net = vm.module(id_lut!(net));
                                let headers_val = headers.script_to_value(vm);
                                let ret = vm.call(handler.into(), &[headers_val]);
                                if script_has_proto!(vm, ret, net.WebSocketEvents) {
                                    let events = WebSocketEvents::script_from_value(vm, ret);
                                    // lets open it
                                    if let Some(handler) = events.on_opened.as_object() {
                                        vm.call(handler.into(), &[]);
                                    }
                                    // Re-borrow server here inside the loop context,
                                    // but we need to access it via self.script_data
                                    // This is tricky because self is borrowed by with_vm_and_async
                                    // We need to return the event to be pushed outside
                                    return Some(CxScriptServerWebSocket {
                                        web_socket_id,
                                        response_sender,
                                        events,
                                    });
                                }
                                None
                            })
                            .map(|ws| {
                                self.script_data.http_servers[i].web_sockets.push(ws);
                            });
                        }
                    }
                    HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                        let mut handler = None;
                        let mut remove_index = None;
                        if let Some(index) = server
                            .web_sockets
                            .iter()
                            .position(|v| v.web_socket_id == web_socket_id)
                        {
                            handler = server.web_sockets[index].events.on_closed.clone();
                            remove_index = Some(index);
                        }

                        if let Some(handler) = handler.as_object() {
                            self.with_vm_and_async(|vm| {
                                vm.call(handler.into(), &[]);
                            });
                        }
                        if let Some(index) = remove_index {
                            self.script_data.http_servers[i].web_sockets.remove(index);
                        }
                    }
                    HttpServerRequest::BinaryMessage {
                        web_socket_id,
                        response_sender: _,
                        data,
                    } => {
                        let mut handler = None;
                        if let Some(index) = server
                            .web_sockets
                            .iter()
                            .position(|v| v.web_socket_id == web_socket_id)
                        {
                            handler = server.web_sockets[index].events.on_binary.clone();
                        }
                        if let Some(handler) = handler.as_object() {
                            self.with_vm_and_async(|vm| {
                                let array = vm.bx.heap.new_array_from_vec_u8(data);
                                vm.call(handler.into(), &[array.into()]);
                            });
                        }
                    }
                    HttpServerRequest::TextMessage {
                        web_socket_id,
                        response_sender: _,
                        string,
                    } => {
                        let mut handler = None;
                        if let Some(index) = server
                            .web_sockets
                            .iter()
                            .position(|v| v.web_socket_id == web_socket_id)
                        {
                            handler = server.web_sockets[index].events.on_string.clone();
                        }
                        if let Some(handler) = handler.as_object() {
                            self.with_vm_and_async(|vm| {
                                let string = vm.bx.heap.new_string_from_str(&string);
                                vm.call(handler.into(), &[string.into()]);
                            });
                        }
                    }
                    HttpServerRequest::Get {
                        headers,
                        response_sender,
                    } => {
                        let handler = server.events.on_get.clone();
                        if let Some(handler) = handler.as_object() {
                            self.with_vm_and_async(|vm| {
                                let net = vm.module(id_lut!(net));
                                let headers_val = headers.script_to_value(vm);
                                let ret = vm.call(handler.into(), &[headers_val]);
                                if script_has_proto!(vm, ret, net.HttpServerResponse) {
                                    let response = HttpServerResponse::script_from_value(vm, ret);
                                    let _ = response_sender.send(response);
                                } else {
                                    let _ = response_sender.send(HttpServerResponse {
                                        header:
                                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n"
                                                .to_string(),
                                        body: "No body".to_string().into_bytes(),
                                    });
                                }
                            });
                        }
                    }
                    HttpServerRequest::Post {
                        headers,
                        body,
                        response,
                    } => {
                        let handler = server.events.on_post.clone();
                        if let Some(handler) = handler.as_object() {
                            self.with_vm_and_async(|vm| {
                                let net = vm.module(id_lut!(net));
                                let headers_val = headers.script_to_value(vm);
                                let body_array = vm.bx.heap.new_array_from_vec_u8(body);
                                let ret =
                                    vm.call(handler.into(), &[headers_val, body_array.into()]);

                                if script_has_proto!(vm, ret, net.HttpServerResponse) {
                                    let response_obj =
                                        HttpServerResponse::script_from_value(vm, ret);
                                    let _ = response.send(response_obj);
                                } else {
                                    let _ = response.send(HttpServerResponse {
                                        header:
                                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n"
                                                .to_string(),
                                        body: "No body".to_string().into_bytes(),
                                    });
                                }
                            });
                        }
                    }
                }
            }
            i += 1;
        }
    }

    pub(crate) fn handle_script_web_socket_event(&mut self, event: NetworkResponse) {
        match event {
            NetworkResponse::WsOpened { socket_id } => {
                if let Some(item) = self
                    .script_data
                    .web_sockets
                    .iter()
                    .find(|item| item.socket_id == socket_id)
                {
                    if let Some(handler) = item.events.on_opened.as_object() {
                        self.with_vm_and_async(|vm| {
                            vm.call(handler.into(), &[]);
                        });
                    }
                }
            }
            NetworkResponse::WsMessage { socket_id, message } => {
                let Some(item) = self
                    .script_data
                    .web_sockets
                    .iter()
                    .find(|item| item.socket_id == socket_id)
                else {
                    return;
                };
                match message {
                    WsMessage::Text(string) => {
                        if let Some(handler) = item.events.on_string.as_object() {
                            self.with_vm_and_async(|vm| {
                                let string = vm.bx.heap.new_string_from_str(&string);
                                vm.call(handler.into(), &[string.into()]);
                            });
                        }
                    }
                    WsMessage::Binary(data) => {
                        if let Some(handler) = item.events.on_binary.as_object() {
                            self.with_vm_and_async(|vm| {
                                let data = vm.bx.heap.new_array_from_vec_u8(data);
                                vm.call(handler.into(), &[data.into()]);
                            });
                        }
                    }
                }
            }
            NetworkResponse::WsClosed { socket_id } => {
                if let Some(index) = self
                    .script_data
                    .web_sockets
                    .iter()
                    .position(|item| item.socket_id == socket_id)
                {
                    if let Some(handler) = self.script_data.web_sockets[index]
                        .events
                        .on_closed
                        .as_object()
                    {
                        self.with_vm_and_async(|vm| {
                            vm.call(handler.into(), &[]);
                        });
                    }
                    self.script_data.web_sockets.remove(index);
                }
            }
            NetworkResponse::WsError { socket_id, message } => {
                if let Some(index) = self
                    .script_data
                    .web_sockets
                    .iter()
                    .position(|item| item.socket_id == socket_id)
                {
                    if let Some(handler) = self.script_data.web_sockets[index]
                        .events
                        .on_error
                        .as_object()
                    {
                        self.with_vm_and_async(|vm| {
                            let message = vm.bx.heap.new_string_from_str(&message);
                            vm.call(handler.into(), &[message.into()]);
                        });
                    }
                    self.script_data.web_sockets.remove(index);
                }
            }
            NetworkResponse::HttpResponse { .. }
            | NetworkResponse::HttpStreamChunk { .. }
            | NetworkResponse::HttpStreamComplete { .. }
            | NetworkResponse::HttpError { .. }
            | NetworkResponse::HttpProgress { .. } => {}
        }
    }

    #[allow(unused)]
    pub(crate) fn handle_script_network_events(&mut self, responses: &[NetworkResponse]) {
        for response in responses {
            let request_id = match response {
                NetworkResponse::HttpResponse { request_id, .. }
                | NetworkResponse::HttpStreamChunk { request_id, .. }
                | NetworkResponse::HttpStreamComplete { request_id, .. }
                | NetworkResponse::HttpError { request_id, .. }
                | NetworkResponse::HttpProgress { request_id, .. } => *request_id,
                NetworkResponse::WsOpened { .. }
                | NetworkResponse::WsMessage { .. }
                | NetworkResponse::WsClosed { .. }
                | NetworkResponse::WsError { .. } => continue,
            };

            // Handle http_resource responses (resource loading via HTTP)
            if self.script_data.resources.is_http_resource(request_id) {
                let resource_info = {
                    let handle = self
                        .script_data
                        .resources
                        .http_resources
                        .iter()
                        .find(|r| r.request_id == request_id)
                        .map(|r| r.handle);
                    if let Some(handle) = handle {
                        let resources = self.script_data.resources.resources.borrow();
                        if let Some(res) = resources.iter().find(|r| r.handle == handle) {
                            format!(
                                "abs_path={} web_url={:?} dependency_path={:?}",
                                res.abs_path, res.web_url, res.dependency_path
                            )
                        } else {
                            format!("handle={:?} (resource entry not found)", handle)
                        }
                    } else {
                        "unknown resource".to_string()
                    }
                };
                match response {
                    NetworkResponse::HttpResponse { response: res, .. } => {
                        if let Some(body) = res.get_body() {
                            if res.status_code >= 200 && res.status_code < 300 {
                                self.script_data
                                    .resources
                                    .handle_http_response(request_id, body.clone());
                            } else {
                                crate::log!(
                                    "Script resource HTTP load failed: status={} {}",
                                    res.status_code,
                                    resource_info
                                );
                                self.script_data.resources.handle_http_error(
                                    request_id,
                                    format!("HTTP error: status {}", res.status_code),
                                );
                            }
                        } else {
                            crate::log!(
                                "Script resource HTTP load failed: empty response body {}",
                                resource_info
                            );
                            self.script_data.resources.handle_http_error(
                                request_id,
                                "HTTP error: empty response body".to_string(),
                            );
                        }
                        self.redraw_all();
                    }
                    NetworkResponse::HttpError { error: err, .. } => {
                        crate::log!(
                            "Script resource HTTP request error: message={} {}",
                            err.message,
                            resource_info
                        );
                        self.script_data.resources.handle_http_error(
                            request_id,
                            format!("HTTP request error: {}", err.message),
                        );
                    }
                    _ => {}
                }
                continue;
            }

            match response {
                NetworkResponse::HttpStreamChunk { response: res, .. } => {
                    if let Some(s) = self
                        .script_data
                        .http_requests
                        .iter()
                        .find(|v| v.id == request_id)
                    {
                        if let Some(handler) = s.events.on_stream.as_object() {
                            self.with_vm_and_async(|vm| {
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                    }
                }
                NetworkResponse::HttpStreamComplete { response: res, .. } => {
                    if let Some(i) = self
                        .script_data
                        .http_requests
                        .iter()
                        .position(|v| v.id == request_id)
                    {
                        if let Some(handler) = self.script_data.http_requests[i]
                            .events
                            .on_complete
                            .as_object()
                        {
                            self.with_vm_and_async(|vm| {
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                        self.script_data.http_requests.remove(i);
                    }
                }
                NetworkResponse::HttpResponse { response: res, .. } => {
                    if let Some(i) = self
                        .script_data
                        .http_requests
                        .iter()
                        .position(|v| v.id == request_id)
                    {
                        if let Some(handler) = self.script_data.http_requests[i]
                            .events
                            .on_response
                            .as_object()
                        {
                            self.with_vm_and_async(|vm| {
                                let res = res.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                        self.script_data.http_requests.remove(i);
                    }
                }
                NetworkResponse::HttpError { error: err, .. } => {
                    if let Some(i) = self
                        .script_data
                        .http_requests
                        .iter()
                        .position(|v| v.id == request_id)
                    {
                        if let Some(handler) = self.script_data.http_requests[i]
                            .events
                            .on_error
                            .as_object()
                        {
                            self.with_vm_and_async(|vm| {
                                let res = err.script_to_value(vm);
                                vm.call(handler.into(), &[res]);
                            })
                        }
                        self.script_data.http_requests.remove(i);
                    }
                }
                NetworkResponse::HttpProgress { .. }
                | NetworkResponse::WsOpened { .. }
                | NetworkResponse::WsMessage { .. }
                | NetworkResponse::WsClosed { .. }
                | NetworkResponse::WsError { .. } => {}
            }
        }
    }
}

pub fn script_mod(vm: &mut ScriptVm) {
    let net = vm.new_module(id_lut!(net));

    set_script_value_to_api!(vm, net.HttpRequest);
    set_script_value_to_api!(vm, net.HttpMethod);
    set_script_value_to_api!(vm, net.HttpEvents);
    set_script_value_to_api!(vm, net.HttpServerEvents);
    set_script_value_to_api!(vm, net.HttpServerOptions);
    set_script_value_to_api!(vm, net.HttpServerResponse);
    set_script_value_to_api!(vm, net.HttpServerHeaders);

    vm.add_method(
        net,
        id_lut!(http_server),
        script_args_def!(options = NIL, events = NIL),
        move |vm, args| {
            let options = script_value!(vm, args.options);
            let events = script_value!(vm, args.events);
            if !script_has_proto!(vm, options, net.HttpServerOptions)
                || !script_has_proto!(vm, events, net.HttpServerEvents)
            {
                return script_err_type_mismatch!(vm.trap(), "invalid net arg type");
            }

            let options = HttpServerOptions::script_from_value(vm, options);
            let events = HttpServerEvents::script_from_value(vm, events);

            let (server_tx, server_rx) = channel();
            let ui_receiver = ToUIReceiver::default();
            let ui_sender = ui_receiver.sender();

            std::thread::spawn(move || {
                while let Ok(msg) = server_rx.recv() {
                    let _ = ui_sender.send(msg);
                }
            });

            let server = HttpServer {
                listen_address: options.listen.parse().unwrap(),
                post_max_size: 1024 * 1024 * 10,
                request: server_tx,
            };

            let cx = vm.cx_mut();
            cx.net.start_http_server(server);
            let id = LiveId::unique();
            cx.script_data.http_servers.push(CxScriptHttpServer {
                id,
                receiver: ui_receiver,
                events,
                web_sockets: Vec::new(),
            });
            id.escape()
        },
    );

    vm.add_method(
        net,
        id_lut!(http_request),
        script_args_def!(request = NIL, events = NIL),
        move |vm, args| {
            let request = script_value!(vm, args.request);
            let events = script_value!(vm, args.events);
            // we should check if options is actually of type HttpRequest
            if !script_has_proto!(vm, request, net.HttpRequest)
                || !script_has_proto!(vm, events, net.HttpEvents)
            {
                return script_err_type_mismatch!(vm.trap(), "invalid net arg type");
            }
            let request = HttpRequest::script_from_value(vm, request);
            let events = HttpEvents::script_from_value(vm, events);
            // alright now what
            let cx = vm.cx_mut();
            let id = LiveId::unique();
            cx.script_data
                .http_requests
                .push(CxScriptHttp { id, events });
            cx.http_request(id, request);
            id.escape()
        },
    );

    set_script_value_to_api!(vm, net.WebSocketEvents);

    vm.add_method(
        net,
        id_lut!(web_socket),
        script_args_def!(request = NIL, events = NIL),
        move |vm, args| {
            let request = script_value!(vm, args.request);
            let events = script_value!(vm, args.events);
            // we should check if options is actually of type HttpRequest

            let request = if request.is_string_like() {
                vm.string_with(request, |_vm, s| HttpRequest {
                    url: s.to_string(),
                    ..Default::default()
                })
                .unwrap()
            } else {
                if !script_has_proto!(vm, request, net.HttpRequest) {
                    return script_err_type_mismatch!(vm.trap(), "invalid net arg type");
                }
                HttpRequest::script_from_value(vm, request)
            };

            if !script_has_proto!(vm, events, net.WebSocketEvents) {
                return script_err_type_mismatch!(vm.trap(), "invalid net arg type");
            }
            let events = WebSocketEvents::script_from_value(vm, events);

            // alright now what
            let cx = vm.cx_mut();
            let id = LiveId::unique();
            if let Err(err) = cx.net.ws_open(id, request) {
                crate::error!("script net.web_socket open failed: {err}");
                return NIL;
            }
            cx.script_data.web_sockets.push(CxScriptWebSocket {
                id,
                socket_id: id,
                events,
            });
            id.escape()
        },
    );
}
