//! The web backend's messages (os/web from_wasm.rs and to_wasm.rs) built for this machine, for a
//! host that plays the page's side of them (the web trace host, tools/web_trace): it reads each
//! `FromWasm` message the program writes and writes the `ToWasm` events the program reads, through
//! the bridge's host traits (makepad_wasm_bridge host.rs).
#[path = "os/web/from_wasm.rs"]
pub mod from_wasm;
#[path = "os/web/to_wasm.rs"]
pub mod to_wasm;
