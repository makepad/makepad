use crate::makepad_network::{
    http_server::{HttpServer, HttpServerRequest, HttpServerResponse},
    utils::HttpServerHeaders,
    FromUIReceiver, FromUISender, HttpMethod, HttpRequest, NetworkResponse, SocketStream,
    ToUIReceiver, ToUISender, WsMessage,
};
use crate::{task, vm, ScriptStd, ScriptVmStdExt};
use makepad_script::id;
use makepad_script::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::mpsc::channel;
use std::time::Duration;

pub struct ScriptWebSocket {
    #[allow(unused)]
    pub id: LiveId,
    pub socket_id: LiveId,
    pub events: WebSocketEvents,
}

pub struct ScriptServerWebSocket {
    pub web_socket_id: u64,
    #[allow(unused)]
    pub response_sender: std::sync::mpsc::Sender<Vec<u8>>,
    pub events: WebSocketEvents,
}

pub struct ScriptHttp {
    pub id: LiveId,
    pub events: HttpEvents,
}

pub struct ScriptHttpServer {
    pub id: LiveId,
    pub receiver: ToUIReceiver<HttpServerRequest>,
    pub events: HttpServerEvents,
    pub web_sockets: Vec<ScriptServerWebSocket>,
}

enum SocketStreamIn {
    Write(Vec<u8>),
    StartTls { host: String, ignore_ssl_cert: bool },
    Close,
}

enum SocketStreamOut {
    Data(Vec<u8>),
    Error(String),
    Closed,
}

pub struct ScriptSocketStream {
    pub handle: ScriptHandle,
    pub host: String,
    in_send: FromUISender<SocketStreamIn>,
    out_recv: ToUIReceiver<SocketStreamOut>,
    recv_pause: VecDeque<ScriptThreadId>,
    pending_chunks: VecDeque<Vec<u8>>,
    is_closed: bool,
    last_error: Option<String>,
}

pub struct ScriptSocketStreamGc {
    pub streams: Rc<RefCell<Vec<ScriptSocketStream>>>,
    pub handle: ScriptHandle,
}

impl ScriptHandleGc for ScriptSocketStreamGc {
    fn gc(&mut self) {
        let mut streams = self.streams.borrow_mut();
        if let Some(index) = streams.iter().position(|v| v.handle == self.handle) {
            let mut stream = streams.remove(index);
            let _ = stream.in_send.send(SocketStreamIn::Close);
            stream.is_closed = true;
        }
    }

    fn set_handle(&mut self, handle: ScriptHandle) {
        self.handle = handle;
    }
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

#[derive(Script, ScriptHook)]
pub struct SocketStreamOptions {
    #[live]
    pub host: String,
    #[live]
    pub port: String,
    #[live]
    pub use_tls: bool,
    #[live]
    pub ignore_ssl_cert: bool,
    #[live]
    pub read_timeout_ms: f64,
    #[live]
    pub write_timeout_ms: f64,
}

fn is_would_block(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

fn socket_stream_thread(
    mut socket: SocketStream,
    default_host: String,
    in_recv: FromUIReceiver<SocketStreamIn>,
    out_send: ToUISender<SocketStreamOut>,
) {
    let mut read_buf = vec![0u8; 16 * 1024];
    let mut current_host = default_host;

    loop {
        while let Ok(msg) = in_recv.try_recv() {
            match msg {
                SocketStreamIn::Write(data) => {
                    if let Err(err) = socket.write_all(&data) {
                        let _ = out_send.send(SocketStreamOut::Error(err.to_string()));
                        let _ = out_send.send(SocketStreamOut::Closed);
                        return;
                    }
                    if let Err(err) = socket.flush() {
                        let _ = out_send.send(SocketStreamOut::Error(err.to_string()));
                        let _ = out_send.send(SocketStreamOut::Closed);
                        return;
                    }
                }
                SocketStreamIn::StartTls {
                    host,
                    ignore_ssl_cert,
                } => {
                    if !host.is_empty() {
                        current_host = host;
                    }
                    match socket.into_tls(&current_host, ignore_ssl_cert) {
                        Ok(new_socket) => socket = new_socket,
                        Err(err) => {
                            let _ = out_send.send(SocketStreamOut::Error(err.to_string()));
                            return;
                        }
                    }
                }
                SocketStreamIn::Close => {
                    socket.shutdown();
                    let _ = out_send.send(SocketStreamOut::Closed);
                    return;
                }
            }
        }

        match socket.read(&mut read_buf) {
            Ok(0) => {
                let _ = out_send.send(SocketStreamOut::Closed);
                return;
            }
            Ok(n) => {
                if n > 0 {
                    let _ = out_send.send(SocketStreamOut::Data(read_buf[..n].to_vec()));
                }
            }
            Err(err) if is_would_block(&err) => {}
            Err(err) => {
                let _ = out_send.send(SocketStreamOut::Error(err.to_string()));
                let _ = out_send.send(SocketStreamOut::Closed);
                return;
            }
        }
    }
}

fn script_value_to_bytes(vm: &mut ScriptVm, value: ScriptValue) -> Result<Vec<u8>, String> {
    if value.is_string_like() {
        return vm
            .string_with(value, |_vm, s| s.as_bytes().to_vec())
            .ok_or_else(|| "invalid string value".to_string());
    }

    let Some(array) = value.as_array() else {
        return Err("expected string or byte array".to_string());
    };

    match vm.bx.heap.array_storage(array) {
        ScriptArrayStorage::U8(v) => Ok(v.clone()),
        ScriptArrayStorage::U16(v) => Ok(v.iter().map(|b| *b as u8).collect()),
        ScriptArrayStorage::U32(v) => Ok(v.iter().map(|b| *b as u8).collect()),
        ScriptArrayStorage::F32(v) => Ok(v.iter().map(|b| *b as u8).collect()),
        ScriptArrayStorage::ScriptValue(v) => {
            let mut out = Vec::with_capacity(v.len());
            for value in v {
                let Some(num) = value.as_f64() else {
                    return Err("byte array values must be numbers".to_string());
                };
                if !(0.0..=255.0).contains(&num) {
                    return Err("byte array values must be in 0..255".to_string());
                }
                out.push(num as u8);
            }
            Ok(out)
        }
    }
}

fn socket_stream_index(vm: &mut ScriptVm, handle: ScriptHandle) -> Option<usize> {
    let std = vm.std_mut::<ScriptStd>();
    let streams = std.data.socket_streams.borrow();
    streams.iter().position(|v| v.handle == handle)
}

pub enum SocketStreamPoll {
    Data(Vec<u8>),
    Closed(Option<String>),
    Pause,
    TooManyPaused,
    InvalidHandle,
}

pub fn socket_stream_send_bytes(
    vm: &mut ScriptVm,
    handle: ScriptHandle,
    data: Vec<u8>,
) -> Result<(), String> {
    let Some(index) = socket_stream_index(vm, handle) else {
        return Err("invalid socket_stream handle".to_string());
    };
    let send_result = {
        let std = vm.std_mut::<ScriptStd>();
        let streams = std.data.socket_streams.borrow();
        streams[index].in_send.send(SocketStreamIn::Write(data))
    };
    if send_result.is_err() {
        return Err("socket stream is closed".to_string());
    }
    Ok(())
}

pub fn socket_stream_poll(vm: &mut ScriptVm, handle: ScriptHandle) -> SocketStreamPoll {
    let Some(index) = socket_stream_index(vm, handle) else {
        return SocketStreamPoll::InvalidHandle;
    };
    let std = vm.std_mut::<ScriptStd>();
    let mut streams = std.data.socket_streams.borrow_mut();
    let stream = &mut streams[index];
    if let Some(chunk) = stream.pending_chunks.pop_front() {
        SocketStreamPoll::Data(chunk)
    } else if stream.is_closed {
        SocketStreamPoll::Closed(stream.last_error.clone())
    } else if stream.recv_pause.len() > 100 {
        SocketStreamPoll::TooManyPaused
    } else {
        SocketStreamPoll::Pause
    }
}

pub fn socket_stream_pause_current(vm: &mut ScriptVm, handle: ScriptHandle) -> Result<(), String> {
    let Some(index) = socket_stream_index(vm, handle) else {
        return Err("invalid socket_stream handle".to_string());
    };
    let thread_id = vm.bx.threads.cur().pause();
    let std = vm.std_mut::<ScriptStd>();
    let mut streams = std.data.socket_streams.borrow_mut();
    streams[index].recv_pause.push_front(thread_id);
    Ok(())
}

pub fn handle_script_socket_streams<H: Any>(
    _host: &mut H,
    std: &mut ScriptStd,
    _script_vm: &mut Option<Box<ScriptVmBase>>,
) {
    let mut resume_threads = Vec::new();
    {
        let mut streams = std.data.socket_streams.borrow_mut();
        for stream in streams.iter_mut() {
            while let Ok(msg) = stream.out_recv.try_recv() {
                match msg {
                    SocketStreamOut::Data(data) => {
                        if !data.is_empty() {
                            stream.pending_chunks.push_back(data);
                            if let Some(thread_id) = stream.recv_pause.pop_back() {
                                resume_threads.push(thread_id);
                            }
                        }
                    }
                    SocketStreamOut::Error(message) => {
                        stream.last_error = Some(message);
                        stream.is_closed = true;
                        while let Some(thread_id) = stream.recv_pause.pop_back() {
                            resume_threads.push(thread_id);
                        }
                    }
                    SocketStreamOut::Closed => {
                        stream.is_closed = true;
                        while let Some(thread_id) = stream.recv_pause.pop_back() {
                            resume_threads.push(thread_id);
                        }
                    }
                }
            }
        }
    }
    for thread_id in resume_threads {
        task::queue_script_thread_resume(std, thread_id);
    }
}

pub fn handle_script_http_servers<H: Any>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
) {
    let mut i = 0;
    while i < std.data.http_servers.len() {
        while let Ok(msg) = std.data.http_servers[i].receiver.try_recv() {
            let server = &mut std.data.http_servers[i];
            match msg {
                HttpServerRequest::ConnectWebSocket {
                    web_socket_id,
                    headers,
                    response_sender,
                } => {
                    let handler = server.events.on_connect_websocket.clone();
                    if let Some(handler) = handler.as_object() {
                        let maybe_ws = vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let net = vm.module(id_lut!(net));
                            let headers_val = headers.script_to_value(vm);
                            let ret = vm.call(handler.into(), &[headers_val]);
                            if script_has_proto!(vm, ret, net.WebSocketEvents) {
                                let events = WebSocketEvents::script_from_value(vm, ret);
                                if let Some(handler) = events.on_opened.as_object() {
                                    vm.call(handler.into(), &[]);
                                }
                                return Some(ScriptServerWebSocket {
                                    web_socket_id,
                                    response_sender,
                                    events,
                                });
                            }
                            None
                        });
                        if let Some(ws) = maybe_ws {
                            std.data.http_servers[i].web_sockets.push(ws);
                        }
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
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            vm.call(handler.into(), &[]);
                        });
                    }
                    if let Some(index) = remove_index {
                        std.data.http_servers[i].web_sockets.remove(index);
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
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
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
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
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
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let net = vm.module(id_lut!(net));
                            let headers_val = headers.script_to_value(vm);
                            let ret = vm.call(handler.into(), &[headers_val]);
                            if script_has_proto!(vm, ret, net.HttpServerResponse) {
                                let response = HttpServerResponse::script_from_value(vm, ret);
                                let _ = response_sender.send(response);
                            } else {
                                let _ = response_sender.send(HttpServerResponse {
                                    header: "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n"
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
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let net = vm.module(id_lut!(net));
                            let headers_val = headers.script_to_value(vm);
                            let body_array = vm.bx.heap.new_array_from_vec_u8(body);
                            let ret = vm.call(handler.into(), &[headers_val, body_array.into()]);

                            if script_has_proto!(vm, ret, net.HttpServerResponse) {
                                let response_obj = HttpServerResponse::script_from_value(vm, ret);
                                let _ = response.send(response_obj);
                            } else {
                                let _ = response.send(HttpServerResponse {
                                    header: "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n"
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

pub fn handle_script_web_socket_event<H: Any>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    event: NetworkResponse,
) {
    match event {
        NetworkResponse::WsOpened { socket_id } => {
            if let Some(item) = std
                .data
                .web_sockets
                .iter()
                .find(|item| item.socket_id == socket_id)
            {
                if let Some(handler) = item.events.on_opened.as_object() {
                    vm::with_vm_and_async(host, std, script_vm, |vm| {
                        vm.call(handler.into(), &[]);
                    });
                }
            }
        }
        NetworkResponse::WsMessage { socket_id, message } => {
            let Some(item) = std
                .data
                .web_sockets
                .iter()
                .find(|item| item.socket_id == socket_id)
            else {
                return;
            };
            match message {
                WsMessage::Text(string) => {
                    if let Some(handler) = item.events.on_string.as_object() {
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let string = vm.bx.heap.new_string_from_str(&string);
                            vm.call(handler.into(), &[string.into()]);
                        });
                    }
                }
                WsMessage::Binary(data) => {
                    if let Some(handler) = item.events.on_binary.as_object() {
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let data = vm.bx.heap.new_array_from_vec_u8(data);
                            vm.call(handler.into(), &[data.into()]);
                        });
                    }
                }
            }
        }
        NetworkResponse::WsClosed { socket_id } => {
            if let Some(index) = std
                .data
                .web_sockets
                .iter()
                .position(|item| item.socket_id == socket_id)
            {
                if let Some(handler) = std.data.web_sockets[index].events.on_closed.as_object() {
                    vm::with_vm_and_async(host, std, script_vm, |vm| {
                        vm.call(handler.into(), &[]);
                    });
                }
                std.data.web_sockets.remove(index);
            }
        }
        NetworkResponse::WsError { socket_id, message } => {
            if let Some(index) = std
                .data
                .web_sockets
                .iter()
                .position(|item| item.socket_id == socket_id)
            {
                if let Some(handler) = std.data.web_sockets[index].events.on_error.as_object() {
                    vm::with_vm_and_async(host, std, script_vm, |vm| {
                        let message = vm.bx.heap.new_string_from_str(&message);
                        vm.call(handler.into(), &[message.into()]);
                    });
                }
                std.data.web_sockets.remove(index);
            }
        }
        NetworkResponse::HttpResponse { .. }
        | NetworkResponse::HttpStreamChunk { .. }
        | NetworkResponse::HttpStreamComplete { .. }
        | NetworkResponse::HttpError { .. }
        | NetworkResponse::HttpProgress { .. } => {}
    }
}

pub fn handle_script_network_events<H: Any>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
    responses: &[NetworkResponse],
) {
    for response in responses {
        match response {
            NetworkResponse::WsOpened { .. }
            | NetworkResponse::WsMessage { .. }
            | NetworkResponse::WsClosed { .. }
            | NetworkResponse::WsError { .. } => {
                handle_script_web_socket_event(host, std, script_vm, response.clone());
                continue;
            }
            _ => {}
        }

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

        match response {
            NetworkResponse::HttpStreamChunk { response: res, .. } => {
                if let Some(s) = std.data.http_requests.iter().find(|v| v.id == request_id) {
                    if let Some(handler) = s.events.on_stream.as_object() {
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let res = res.script_to_value(vm);
                            vm.call(handler.into(), &[res]);
                        })
                    }
                }
            }
            NetworkResponse::HttpStreamComplete { response: res, .. } => {
                if let Some(i) = std.data.http_requests.iter().position(|v| v.id == request_id) {
                    if let Some(handler) = std.data.http_requests[i].events.on_complete.as_object()
                    {
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let res = res.script_to_value(vm);
                            vm.call(handler.into(), &[res]);
                        })
                    }
                    std.data.http_requests.remove(i);
                }
            }
            NetworkResponse::HttpResponse { response: res, .. } => {
                if let Some(i) = std.data.http_requests.iter().position(|v| v.id == request_id) {
                    if let Some(handler) = std.data.http_requests[i].events.on_response.as_object()
                    {
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let res = res.script_to_value(vm);
                            vm.call(handler.into(), &[res]);
                        })
                    }
                    std.data.http_requests.remove(i);
                }
            }
            NetworkResponse::HttpError { error: err, .. } => {
                if let Some(i) = std.data.http_requests.iter().position(|v| v.id == request_id) {
                    if let Some(handler) = std.data.http_requests[i].events.on_error.as_object() {
                        vm::with_vm_and_async(host, std, script_vm, |vm| {
                            let res = err.script_to_value(vm);
                            vm.call(handler.into(), &[res]);
                        })
                    }
                    std.data.http_requests.remove(i);
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

pub fn drain_network_runtime(std: &mut ScriptStd) -> Vec<NetworkResponse> {
    let mut responses = Vec::new();
    let Some(net) = std.net.as_ref() else {
        return responses;
    };
    while let Some(response) = net.try_recv() {
        responses.push(response);
    }
    responses
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

            let std = vm.std_mut::<ScriptStd>();
            let Some(runtime) = std.net.as_ref() else {
                return script_err_io!(vm.trap(), "script net runtime is not configured");
            };
            runtime.start_http_server(server);
            let id = LiveId::unique();
            std.data.http_servers.push(ScriptHttpServer {
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
            if !script_has_proto!(vm, request, net.HttpRequest)
                || !script_has_proto!(vm, events, net.HttpEvents)
            {
                return script_err_type_mismatch!(vm.trap(), "invalid net arg type");
            }
            let request = HttpRequest::script_from_value(vm, request);
            let events = HttpEvents::script_from_value(vm, events);

            let std = vm.std_mut::<ScriptStd>();
            let Some(runtime) = std.net.as_ref() else {
                return script_err_io!(vm.trap(), "script net runtime is not configured");
            };
            let id = LiveId::unique();
            if let Err(err) = runtime.http_start(id, request) {
                return script_err_io!(vm.trap(), "http request failed: {err}");
            }
            std.data.http_requests.push(ScriptHttp { id, events });
            id.escape()
        },
    );

    set_script_value_to_api!(vm, net.WebSocketEvents);
    set_script_value_to_api!(vm, net.SocketStreamOptions);

    let socket_stream_type = vm.new_handle_type(id_lut!(socket_stream));

    vm.set_handle_getter(socket_stream_type, |vm, pself, prop| {
        let Some(handle) = pself.as_handle() else {
            return script_err_not_found!(vm.trap(), "invalid socket_stream prop");
        };
        let Some(index) = socket_stream_index(vm, handle) else {
            return script_err_not_found!(vm.trap(), "invalid socket_stream handle");
        };
        let (closed, pending, error, host) = {
            let std = vm.std_mut::<ScriptStd>();
            let streams = std.data.socket_streams.borrow();
            let stream = &streams[index];
            (
                stream.is_closed,
                stream.pending_chunks.len() as f64,
                stream.last_error.clone(),
                stream.host.clone(),
            )
        };

        if prop == id!(closed) {
            return closed.into();
        }
        if prop == id!(pending) {
            return pending.into();
        }
        if prop == id!(error) {
            if let Some(error) = error {
                return vm.new_string_with(|_vm, out| out.push_str(&error)).into();
            }
            return NIL;
        }
        if prop == id!(host) {
            return vm.new_string_with(|_vm, out| out.push_str(&host)).into();
        }
        script_err_not_found!(vm.trap(), "invalid socket_stream prop")
    });

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(write),
        script_args_def!(data = NIL),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let Some(index) = socket_stream_index(vm, handle) else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let data = script_value!(vm, args.data);
            let bytes = match script_value_to_bytes(vm, data) {
                Ok(bytes) => bytes,
                Err(err) => return script_err_type_mismatch!(vm.trap(), "{err}"),
            };
            let byte_len = bytes.len() as f64;

            let send_result = {
                let std = vm.std_mut::<ScriptStd>();
                let streams = std.data.socket_streams.borrow();
                streams[index].in_send.send(SocketStreamIn::Write(bytes))
            };
            if send_result.is_err() {
                return script_err_io!(vm.trap(), "socket stream is closed");
            }
            byte_len.into()
        },
    );

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(write_string),
        script_args_def!(data = NIL),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let Some(index) = socket_stream_index(vm, handle) else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let data = script_value!(vm, args.data);
            if !data.is_string_like() {
                return script_err_type_mismatch!(vm.trap(), "write_string expects a string");
            }
            let Some(bytes) = vm.string_with(data, |_vm, s| s.as_bytes().to_vec()) else {
                return script_err_type_mismatch!(vm.trap(), "write_string expects a string");
            };
            let byte_len = bytes.len() as f64;
            let send_result = {
                let std = vm.std_mut::<ScriptStd>();
                let streams = std.data.socket_streams.borrow();
                streams[index].in_send.send(SocketStreamIn::Write(bytes))
            };
            if send_result.is_err() {
                return script_err_io!(vm.trap(), "socket stream is closed");
            }
            byte_len.into()
        },
    );

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(start_tls),
        script_args_def!(host = NIL, ignore_ssl_cert = NIL),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let Some(index) = socket_stream_index(vm, handle) else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let host_arg = script_value!(vm, args.host);
            let ignore_arg = script_value!(vm, args.ignore_ssl_cert);
            let ignore_ssl_cert = ignore_arg.as_bool().unwrap_or(false);
            let host_override = if host_arg.is_string_like() {
                vm.string_with(host_arg, |_vm, s| s.to_string())
            } else {
                None
            };

            let (stream_host, send_result) = {
                let std = vm.std_mut::<ScriptStd>();
                let mut streams = std.data.socket_streams.borrow_mut();
                let stream = &mut streams[index];
                let stream_host = stream.host.clone();
                let host = host_override.clone().unwrap_or(stream_host.clone());
                let send_result = stream.in_send.send(SocketStreamIn::StartTls {
                    host,
                    ignore_ssl_cert,
                });
                (stream_host, send_result)
            };
            if send_result.is_err() {
                return script_err_io!(
                    vm.trap(),
                    "socket stream is closed and cannot start TLS for host {}",
                    stream_host
                );
            }
            NIL
        },
    );

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(close),
        script_args_def!(),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let Some(index) = socket_stream_index(vm, handle) else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            let send_result = {
                let std = vm.std_mut::<ScriptStd>();
                let mut streams = std.data.socket_streams.borrow_mut();
                let stream = &mut streams[index];
                stream.is_closed = true;
                stream.in_send.send(SocketStreamIn::Close)
            };
            if send_result.is_err() {
                return script_err_io!(vm.trap(), "socket stream close failed");
            }
            NIL
        },
    );

    vm.add_handle_method(socket_stream_type, id_lut!(next), script_args_def!(), move |vm, args| {
        let Some(handle) = script_value!(vm, args.self).as_handle() else {
            return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
        };
        match socket_stream_poll(vm, handle) {
            SocketStreamPoll::Data(data) => vm.bx.heap.new_array_from_vec_u8(data).into(),
            SocketStreamPoll::Closed(Some(err)) => script_err_io!(vm.trap(), "{err}"),
            SocketStreamPoll::Closed(None) => NIL,
            SocketStreamPoll::Pause => {
                if let Err(err) = socket_stream_pause_current(vm, handle) {
                    return script_err_io!(vm.trap(), "{err}");
                }
                NIL
            }
            SocketStreamPoll::TooManyPaused => {
                script_err_limit!(vm.trap(), "too many paused socket reads")
            }
            SocketStreamPoll::InvalidHandle => {
                script_err_unexpected!(vm.trap(), "invalid socket_stream state")
            }
        }
    });

    vm.add_handle_method(
        socket_stream_type,
        id_lut!(next_string),
        script_args_def!(),
        move |vm, args| {
            let Some(handle) = script_value!(vm, args.self).as_handle() else {
                return script_err_unexpected!(vm.trap(), "invalid socket_stream state");
            };
            match socket_stream_poll(vm, handle) {
                SocketStreamPoll::Data(data) => {
                    let string = String::from_utf8_lossy(&data);
                    vm.new_string_with(|_vm, out| out.push_str(&string)).into()
                }
                SocketStreamPoll::Closed(Some(err)) => script_err_io!(vm.trap(), "{err}"),
                SocketStreamPoll::Closed(None) => NIL,
                SocketStreamPoll::Pause => {
                    if let Err(err) = socket_stream_pause_current(vm, handle) {
                        return script_err_io!(vm.trap(), "{err}");
                    }
                    NIL
                }
                SocketStreamPoll::TooManyPaused => {
                    script_err_limit!(vm.trap(), "too many paused socket reads")
                }
                SocketStreamPoll::InvalidHandle => {
                    script_err_unexpected!(vm.trap(), "invalid socket_stream state")
                }
            }
        },
    );

    vm.add_method(
        net,
        id_lut!(web_socket),
        script_args_def!(request = NIL, events = NIL),
        move |vm, args| {
            let request = script_value!(vm, args.request);
            let events = script_value!(vm, args.events);

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

            let std = vm.std_mut::<ScriptStd>();
            let Some(runtime) = std.net.as_ref() else {
                return script_err_io!(vm.trap(), "script net runtime is not configured");
            };
            let id = LiveId::unique();
            if runtime.ws_open(id, request).is_err() {
                return NIL;
            }
            std.data.web_sockets.push(ScriptWebSocket {
                id,
                socket_id: id,
                events,
            });
            id.escape()
        },
    );

    vm.add_method(
        net,
        id_lut!(socket_stream),
        script_args_def!(options = NIL),
        move |vm, args| {
            let options = script_value!(vm, args.options);
            if !script_has_proto!(vm, options, net.SocketStreamOptions) {
                return script_err_type_mismatch!(vm.trap(), "invalid socket_stream arg type");
            }
            let options = SocketStreamOptions::script_from_value(vm, options);
            if options.host.is_empty() || options.port.is_empty() {
                return script_err_invalid_args!(
                    vm.trap(),
                    "socket_stream expects non-empty host and port"
                );
            }

            let socket = match SocketStream::connect(
                &options.host,
                &options.port,
                options.use_tls,
                options.ignore_ssl_cert,
            ) {
                Ok(socket) => socket,
                Err(err) => return script_err_io!(vm.trap(), "socket stream connect failed: {err}"),
            };

            let read_timeout = if options.read_timeout_ms > 0.0 {
                Some(Duration::from_millis(options.read_timeout_ms as u64))
            } else {
                None
            };
            let write_timeout = if options.write_timeout_ms > 0.0 {
                Some(Duration::from_millis(options.write_timeout_ms as u64))
            } else {
                None
            };

            if let Err(err) = socket.set_read_timeout(read_timeout) {
                return script_err_io!(vm.trap(), "set_read_timeout failed: {err}");
            }
            if let Err(err) = socket.set_write_timeout(write_timeout) {
                return script_err_io!(vm.trap(), "set_write_timeout failed: {err}");
            }

            let out_recv: ToUIReceiver<SocketStreamOut> = Default::default();
            let out_send = out_recv.sender();
            let mut in_send: FromUISender<SocketStreamIn> = Default::default();
            let in_recv = in_send.receiver();
            let host = options.host.clone();

            std::thread::spawn(move || {
                socket_stream_thread(socket, host, in_recv, out_send);
            });

            let streams_ref = {
                let std = vm.std_mut::<ScriptStd>();
                std.data.socket_streams.clone()
            };
            let handle_gc = ScriptSocketStreamGc {
                streams: streams_ref,
                handle: ScriptHandle::ZERO,
            };
            let handle = vm.bx.heap.new_handle(socket_stream_type, Box::new(handle_gc));
            {
                let std = vm.std_mut::<ScriptStd>();
                std.data.socket_streams.borrow_mut().push(ScriptSocketStream {
                    handle,
                    host: options.host,
                    in_send,
                    out_recv,
                    recv_pause: Default::default(),
                    pending_chunks: Default::default(),
                    is_closed: false,
                    last_error: None,
                });
            }

            handle.into()
        },
    );
}
