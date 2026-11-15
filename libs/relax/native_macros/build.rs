//! The base set's rows (gen.rs): Relax's own macros and the carried libraries they use. `dir` is
//! relative to this manifest's folder.
#[path = "gen.rs"]
mod gen;
#[path = "../base/src/sha256.rs"]
#[allow(dead_code)]
mod sha256;
#[path = "../self_stamp.rs"]
mod self_stamp;

use gen::Row;

const ROWS: &[Row] = &[
    Row { name: "makepad_micro_proc_macro", dir: "../../micro_proc_macro", module: None,
        // the target side's feature of a graph that links relax-native-macros (compiler
        // native_macros.rs `optional`)
        features: &["relax-tokens"] },
    // (the loader's own graph, `makepad check makepad-run`, asks for the build it links)
    Row { name: "makepad_live_id", dir: "../../live_id", module: None, features: &["default", "host-shims"] },
    Row { name: "makepad_live_id_macros", dir: "../../live_id/id_macros", module: Some("id"), features: &[] },
    Row { name: "makepad_live_id_macros_impl", dir: "../../live_id/id_macros_impl", module: None, features: &[] },
    Row { name: "makepad_micro_serde_derive", dir: "../../micro_serde/derive", module: Some("serde"), features: &[] },
    Row { name: "makepad_micro_serde_derive_impl", dir: "../../micro_serde/derive_impl", module: None, features: &[] },
    Row { name: "relax_api_phase", dir: "../../relax_api/phase", module: None, features: &[] },
    // the loader's copy is built with host-shims (the loader links makepad-hal, which enables it):
    // a graph that links makepad-hal (its own tests) asks for that build
    Row { name: "relax_api", dir: "../../relax_api", module: None, features: &["host-shims"] },
    Row { name: "relax_api_macro", dir: "../../relax_api/macro", module: Some("relax_api"), features: &[] },
    Row { name: "relax_api_macro_impl", dir: "../../relax_api/macro_impl", module: None, features: &[] },
];

fn main() {
    gen::generate(ROWS);
}
