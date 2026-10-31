//! Caller-side adapters: the caller owns NetworkResponse and its Rust-layout
//! storage. Every VM/std access below uses the one C-shaped operation contract.
use crate::makepad_network::{NetworkResponse, WsMessage};
use crate::makepad_network::runtime_handle::{NetworkResponseKind as K, NetworkResponseView};
use crate::net::ScriptNetworkEvent;
use crate::net_values::*;
use crate::ScriptVmCall;
use makepad_script::*;

/// Always stays with the caller that owns the network enum/map/Arc layout.
#[inline(always)]
pub fn dispatch_network_response(host: &mut ScriptHostContext, response: &NetworkResponse) {
    let (kind, id) = match response {
        NetworkResponse::HttpResponse { request_id, .. } => (K::HttpResponse, *request_id),
        NetworkResponse::HttpStreamChunk { request_id, .. } => (K::HttpStreamChunk, *request_id),
        NetworkResponse::HttpStreamComplete { request_id, .. } => {
            (K::HttpStreamComplete, *request_id)
        }
        NetworkResponse::HttpError { request_id, .. } => (K::HttpError, *request_id),
        NetworkResponse::HttpProgress { .. } => return,
        NetworkResponse::WsOpened { socket_id } => (K::WsOpened, *socket_id),
        NetworkResponse::WsClosed { socket_id } => (K::WsClosed, *socket_id),
        NetworkResponse::WsError { socket_id, .. } => (K::WsError, *socket_id),
        NetworkResponse::WsMessage { socket_id, message } => (
            match message {
                WsMessage::Text(_) => K::WsText,
                WsMessage::Binary(_) => K::WsBinary,
            },
            *socket_id,
        ),
    };
    let event = ScriptNetworkEvent::begin(host, kind, id);
    event.dispatch(
        host,
        &mut ScriptVmCall::new(&mut |vm: &mut ScriptVm| {
            let value = match response {
                NetworkResponse::HttpResponse { response, .. }
                | NetworkResponse::HttpStreamChunk { response, .. }
                | NetworkResponse::HttpStreamComplete { response, .. } => {
                    let builder = ScriptHttpResponseValue::begin(
                        vm,
                        response.metadata_id,
                        response.status_code,
                    );
                    for (name, values) in &response.headers {
                        let header = builder.start_header(vm, name);
                        for value in values {
                            header.append_value(vm, value);
                        }
                        builder.finish_header(vm, header);
                    }
                    // Reuse C3's borrowed scalar/body view. Header groups were
                    // streamed above, including groups with zero values.
                    let view = NetworkResponseView {
                        kind,
                        id,
                        metadata_id: response.metadata_id,
                        status_code: response.status_code,
                        headers: &[],
                        has_body: response.body.is_some(),
                        body: response.body.as_deref().unwrap_or_default(),
                        message: "",
                        loaded: 0,
                        total: 0,
                    };
                    builder.finish(vm, &view)
                }
                NetworkResponse::HttpError { .. } => {
                    response.with_view(|view| http_error_value(vm, view))
                }
                NetworkResponse::WsMessage {
                    message: WsMessage::Binary(bytes),
                    ..
                } => vm.heap_mut().new_array_from_slice_u8(bytes).into(),
                NetworkResponse::WsMessage {
                    message: WsMessage::Text(text),
                    ..
                }
                | NetworkResponse::WsError { message: text, .. } => vm.new_string(text),
                _ => NIL,
            };
            event.call(vm, value);
        }),
    );
}

#[inline(always)]
pub fn http_server_headers_value(
    vm: &mut ScriptVm,
    headers: &crate::makepad_network::utils::HttpServerHeaders,
) -> ScriptValue {
    let view = headers.view();
    let builder = ScriptHttpServerHeadersValue::begin(vm, &view);
    for line in &headers.lines {
        builder.append_line(vm, line);
    }
    builder.finish(vm, &view)
}
