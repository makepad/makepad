//! VM-owned values built from borrowed network fields. Collection traversal
//! stays with the network owner; no Rust String, Vec, map or Arc crosses here.
use makepad_relax_api::relax_api;
use crate::makepad_network::runtime_handle::{
    HttpServerHeadersView, HttpServerResponseBuffers, NetworkResponseView,
};
use crate::net_types::{HttpError, HttpResponse, HttpServerHeaders, HttpServerResponse};
use makepad_script::*;

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptHttpResponseValue {
    object: ScriptObject,
    headers: ScriptObject,
}
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptHttpHeaderValue {
    key: ScriptValue,
    values: ScriptArray,
}
#[relax_api]
impl ScriptHttpResponseValue {
    #[inline]
    #[relax_api]
    pub fn begin(vm: &mut ScriptVm, metadata_id: LiveId, status_code: u16) -> Self {
        let proto = HttpResponse::script_proto(vm);
        let object = vm.new_with_proto(proto);
        let metadata = metadata_id.script_to_value(vm);
        vm.script_derive_set_prop(object, "metadata_id", metadata);
        let status = status_code.script_to_value(vm);
        vm.script_derive_set_prop(object, "status_code", status);
        Self {
            object,
            headers: vm.heap_mut().new_object(),
        }
    }
    #[inline]
    pub fn start_header(&self, vm: &mut ScriptVm, name: &str) -> ScriptHttpHeaderValue {
        let key = vm.new_string(name);
        ScriptHttpHeaderValue {
            key,
            values: vm.heap_mut().new_array(),
        }
    }
    #[inline]
    pub fn finish_header(&self, vm: &mut ScriptVm, header: ScriptHttpHeaderValue) {
        // Charge after converting the values, exactly as Rust map ScriptApply.
        vm.heap_mut()
            .map_insert_converted(self.headers, header.key, header.values.into());
    }
    #[inline]
    #[relax_api]
    pub fn finish(self, vm: &mut ScriptVm, response: &NetworkResponseView) -> ScriptValue {
        vm.script_derive_set_prop(self.object, "headers", self.headers.into());
        let body = if response.has_body {
            vm.heap_mut().new_array_from_slice_u8(response.body).into()
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "body", body);
        self.object.into()
    }
}
impl ScriptHttpHeaderValue {
    #[inline]
    pub fn append_value(&self, vm: &mut ScriptVm, value: &str) {
        let value = vm.new_string(value);
        vm.heap_mut().array_push_unchecked(self.values, value);
    }
}

#[inline]
#[relax_api]
pub fn http_error_value(vm: &mut ScriptVm, error: &NetworkResponseView) -> ScriptValue {
    let proto = HttpError::script_proto(vm);
    let object = vm.new_with_proto(proto);
    let message = vm.new_string(error.message);
    vm.script_derive_set_prop(object, "message", message);
    let metadata = error.metadata_id.script_to_value(vm);
    vm.script_derive_set_prop(object, "metadata_id", metadata);
    object.into()
}

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptHttpServerHeadersValue {
    object: ScriptObject,
    lines: ScriptArray,
}
impl ScriptHttpServerHeadersValue {
    #[inline]
    pub fn begin(vm: &mut ScriptVm, headers: &HttpServerHeadersView) -> Self {
        let proto = HttpServerHeaders::script_proto(vm);
        let object = vm.new_with_proto(proto);
        let addr = vm.new_string(headers.addr_text);
        vm.script_derive_set_prop(object, "addr_text", addr);
        Self {
            object,
            lines: vm.heap_mut().new_array(),
        }
    }
    #[inline]
    pub fn append_line(&self, vm: &mut ScriptVm, line: &str) {
        let value = vm.new_string(line);
        vm.heap_mut().array_push_unchecked(self.lines, value);
    }
    #[inline]
    pub fn finish(self, vm: &mut ScriptVm, headers: &HttpServerHeadersView) -> ScriptValue {
        vm.script_derive_set_prop(self.object, "lines", self.lines.into());
        for (name, value) in [
            ("verb", headers.verb),
            ("path", headers.path),
            ("path_no_slash", headers.path_no_slash),
        ] {
            let value = vm.new_string(value);
            vm.script_derive_set_prop(self.object, name, value);
        }
        let search = if headers.has_search {
            vm.new_string(headers.search)
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "search", search);
        let length = if headers.has_content_length {
            headers.content_length.script_to_value(vm)
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "content_length", length);
        let encoding = if headers.has_accept_encoding {
            vm.new_string(headers.accept_encoding)
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "accept_encoding", encoding);
        let key = if headers.has_sec_websocket_key {
            vm.new_string(headers.sec_websocket_key)
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "sec_websocket_key", key);
        self.object.into()
    }
}

/// VM-owned conversion and transfer. No additional allocation/copy beyond
/// ScriptApply's existing conversion from script strings/arrays is needed.
#[inline]
pub fn http_server_response_buffers(
    vm: &mut ScriptVm,
    value: ScriptValue,
) -> HttpServerResponseBuffers {
    let response = HttpServerResponse::script_from_value(vm, value);
    HttpServerResponseBuffers::new(response.header, response.body)
}
