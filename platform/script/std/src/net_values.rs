//! VM-owned values built from borrowed network fields. Collection traversal
//! stays with the network owner; no Rust String, Vec, map or Arc crosses here.
use relax_api::relax_api;
use crate::makepad_network::runtime_handle::{
    HttpServerHeadersView, HttpServerResponseBuffers, NetworkResponseView,
};
use crate::net_types::{HttpError, HttpResponse, HttpServerHeaders, HttpServerResponse};
use makepad_script::*;

#[relax_api(plain, run)]
#[repr(C)]
pub struct ScriptHttpResponseValue {
    object: ScriptObject,
    headers: ScriptObject,
}
#[relax_api(plain, run)]
#[repr(C)]
pub struct ScriptHttpHeaderValue {
    key: ScriptValue,
    values: ScriptArray,
}
#[relax_api(run)]
impl ScriptHttpResponseValue {
    #[inline]
    #[relax_api(run)]
    pub fn begin(vm: &mut ScriptVm, metadata_id: LiveId, status_code: u16) -> Self {
        let empty = Self { object: ScriptObject::ZERO, headers: ScriptObject::ZERO };
        let proto = HttpResponse::script_proto(vm);
        if vm.heap().allocation_error_pending() { return empty; }
        let object = vm.new_with_proto(proto);
        if vm.heap().allocation_error_pending() { return empty; }
        let metadata = metadata_id.script_to_value(vm);
        vm.script_derive_set_prop(object, "metadata_id", metadata);
        if vm.heap().allocation_error_pending() { return empty; }
        let status = status_code.script_to_value(vm);
        vm.script_derive_set_prop(object, "status_code", status);
        if vm.heap().allocation_error_pending() { return empty; }
        let headers = vm.heap_mut().new_object();
        if vm.heap().allocation_error_pending() { return empty; }
        Self { object, headers }
    }
    #[inline]
    pub fn start_header(&self, vm: &mut ScriptVm, name: &str) -> ScriptHttpHeaderValue {
        let empty = ScriptHttpHeaderValue { key: NIL, values: ScriptArray::default() };
        if vm.heap().allocation_error_pending() { return empty; }
        let key = vm.new_string(name);
        if vm.heap().allocation_error_pending() { return empty; }
        let values = vm.heap_mut().new_array();
        if vm.heap().allocation_error_pending() { return empty; }
        ScriptHttpHeaderValue { key, values }
    }
    #[inline]
    pub fn finish_header(&self, vm: &mut ScriptVm, header: ScriptHttpHeaderValue) {
        if vm.heap().allocation_error_pending() { return; }
        // Charge after converting the values, exactly as Rust map ScriptApply.
        vm.heap_mut()
            .map_insert_converted(self.headers, header.key, header.values.into());
    }
    #[inline]
    #[relax_api(run)]
    pub fn finish(self, vm: &mut ScriptVm, response: &NetworkResponseView) -> ScriptValue {
        if vm.heap().allocation_error_pending() { return NIL; }
        vm.script_derive_set_prop(self.object, "headers", self.headers.into());
        if vm.heap().allocation_error_pending() { return NIL; }
        let body = if response.has_body {
            let array = vm.heap_mut().new_array_from_slice_u8(response.body);
            if vm.heap().allocation_error_pending() { return NIL; }
            array.into()
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "body", body);
        if vm.heap().allocation_error_pending() { return NIL; }
        self.object.into()
    }
}
impl ScriptHttpHeaderValue {
    #[inline]
    pub fn append_value(&self, vm: &mut ScriptVm, value: &str) {
        if vm.heap().allocation_error_pending() { return; }
        let value = vm.new_string(value);
        if vm.heap().allocation_error_pending() { return; }
        vm.heap_mut().array_push_unchecked(self.values, value);
    }
}

#[inline]
#[relax_api(run)]
pub fn http_error_value(vm: &mut ScriptVm, error: &NetworkResponseView) -> ScriptValue {
    let proto = HttpError::script_proto(vm);
    if vm.heap().allocation_error_pending() { return NIL; }
    let object = vm.new_with_proto(proto);
    if vm.heap().allocation_error_pending() { return NIL; }
    let message = vm.new_string(error.message);
    if vm.heap().allocation_error_pending() { return NIL; }
    vm.script_derive_set_prop(object, "message", message);
    if vm.heap().allocation_error_pending() { return NIL; }
    let metadata = error.metadata_id.script_to_value(vm);
    vm.script_derive_set_prop(object, "metadata_id", metadata);
    if vm.heap().allocation_error_pending() { return NIL; }
    object.into()
}

#[relax_api(plain, run)]
#[repr(C)]
pub struct ScriptHttpServerHeadersValue {
    object: ScriptObject,
    lines: ScriptArray,
}
impl ScriptHttpServerHeadersValue {
    #[inline]
    pub fn begin(vm: &mut ScriptVm, headers: &HttpServerHeadersView) -> Self {
        let empty = Self { object: ScriptObject::ZERO, lines: ScriptArray::default() };
        let proto = HttpServerHeaders::script_proto(vm);
        if vm.heap().allocation_error_pending() { return empty; }
        let object = vm.new_with_proto(proto);
        if vm.heap().allocation_error_pending() { return empty; }
        let addr = vm.new_string(headers.addr_text);
        if vm.heap().allocation_error_pending() { return empty; }
        vm.script_derive_set_prop(object, "addr_text", addr);
        if vm.heap().allocation_error_pending() { return empty; }
        let lines = vm.heap_mut().new_array();
        if vm.heap().allocation_error_pending() { return empty; }
        Self { object, lines }
    }
    #[inline]
    pub fn append_line(&self, vm: &mut ScriptVm, line: &str) {
        if vm.heap().allocation_error_pending() { return; }
        let value = vm.new_string(line);
        if vm.heap().allocation_error_pending() { return; }
        vm.heap_mut().array_push_unchecked(self.lines, value);
    }
    #[inline]
    pub fn finish(self, vm: &mut ScriptVm, headers: &HttpServerHeadersView) -> ScriptValue {
        if vm.heap().allocation_error_pending() { return NIL; }
        vm.script_derive_set_prop(self.object, "lines", self.lines.into());
        if vm.heap().allocation_error_pending() { return NIL; }
        for (name, value) in [
            ("verb", headers.verb),
            ("path", headers.path),
            ("path_no_slash", headers.path_no_slash),
        ] {
            let value = vm.new_string(value);
            if vm.heap().allocation_error_pending() { return NIL; }
            vm.script_derive_set_prop(self.object, name, value);
            if vm.heap().allocation_error_pending() { return NIL; }
        }
        let search = if headers.has_search {
            let search = vm.new_string(headers.search);
            if vm.heap().allocation_error_pending() { return NIL; }
            search
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "search", search);
        if vm.heap().allocation_error_pending() { return NIL; }
        let length = if headers.has_content_length {
            headers.content_length.script_to_value(vm)
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "content_length", length);
        if vm.heap().allocation_error_pending() { return NIL; }
        let encoding = if headers.has_accept_encoding {
            let encoding = vm.new_string(headers.accept_encoding);
            if vm.heap().allocation_error_pending() { return NIL; }
            encoding
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "accept_encoding", encoding);
        if vm.heap().allocation_error_pending() { return NIL; }
        let key = if headers.has_sec_websocket_key {
            let key = vm.new_string(headers.sec_websocket_key);
            if vm.heap().allocation_error_pending() { return NIL; }
            key
        } else {
            NIL
        };
        vm.script_derive_set_prop(self.object, "sec_websocket_key", key);
        if vm.heap().allocation_error_pending() { return NIL; }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_network::runtime_handle::NetworkResponseKind;

    #[test]
    fn net_values_builder_stops_cleanly_on_refusal() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        HttpResponse::script_proto(&mut vm);
        vm.capture_errors();
        vm.heap_mut().new_array();
        vm.gc();
        let free = vm.heap().objects_len() - vm.heap().gc_live_counts().objects;
        for _ in 0..free { vm.new_object(); }
        let view = NetworkResponseView {
            kind: NetworkResponseKind::HttpResponse,
            id: id!(request),
            metadata_id: id!(meta),
            status_code: 200,
            headers: &[],
            has_body: false,
            body: &[],
            message: "",
            loaded: 0,
            total: 0,
        };
        let build = |vm: &mut ScriptVm, name, value| {
            let response = ScriptHttpResponseValue::begin(vm, view.metadata_id, view.status_code);
            let headers = response.headers;
            let header = response.start_header(vm, name);
            header.append_value(vm, value);
            response.finish_header(vm, header);
            (response.finish(vm, &view), headers)
        };

        let before = vm.heap().gc_live_counts().arrays;
        let ((result, _), report) = vm.with_heap_allocation_limit(0, |vm| build(vm, "x-hdr", "val"));
        let refused_object = (result.is_nil(), vm.heap().gc_live_counts().arrays - before);
        assert!(report.exceeded, "creating the response object must hit the scoped budget");
        let errors = vm.take_errors();
        let single_limit = errors.len() == 1 && errors[0].contains("script allocation limit exceeded");
        vm.capture_errors();

        vm.heap_mut().set_max_string_bytes(Some(4));
        let before = vm.heap().gc_live_counts().arrays;
        let (result, headers) = build(&mut vm, "too_long", "ok");
        let refused_key = (result.is_nil(), vm.heap().gc_live_counts().arrays - before, vm.heap().map_len(headers));
        assert!(vm.heap_mut().take_string_limit_exceeded(), "the oversized header name must hit the string limit");
        assert_eq!((refused_object, single_limit, refused_key), ((true, 0), true, (true, 0, 0)),
            "both builders must return NIL without allocating a header array; object refusal must report only the limit and a refused name must leave the headers empty: {errors:?}");
    }
}
