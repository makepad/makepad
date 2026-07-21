use makepad_network;
use makepad_shell;
use makepad_wasm_strip;

mod server_manager;
mod utils;
mod wasm;

pub use wasm::{generate_html, WasmConfig};
