use crate::makepad_wasm_msg::*;

struct FromWasmPropDefs{
}

struct FromWasmCompileWebGLShader{
    shader_id: usize,
    fragment: String,
    vertex: String,
}

struct FromWasmAllocArrayBuffer{
    buffer_id: usize,
    len: usize,
    data: WasmPtr,
}
