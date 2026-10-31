//! The module the optimiser works on: stitch's decoded form of the binary
//! (every section, function bodies as flat instruction lists with
//! structured control kept as Block/Loop/If/Else/End markers). The passes
//! rewrite it in place and `encode` writes it back.

pub use makepad_stitch::binary::*;

/// Decodes a module, with every standard proposal the decoder knows.
pub fn decode(buf: &[u8]) -> Result<Module, String> {
    Module::decode(buf).map_err(|e| e.to_string())
}

/// Validates a module (stitch's validator): every function body
/// instruction by instruction, and the module around it.
pub fn validate_module(module: &Module) -> Result<(), String> {
    validate(module).map_err(|e| e.to_string())
}
