//! Makepad's set (relax-native-macros gen.rs): the script and widget derives and their libraries.
//! `dir` is relative to this manifest's folder.
#[path = "../../../libs/relax/native_macros/gen.rs"]
mod gen;
#[path = "../../../libs/relax/base/src/sha256.rs"]
#[allow(dead_code)]
mod sha256;
#[path = "../../../libs/relax/self_stamp.rs"]
mod self_stamp;

use gen::Row;

const ROWS: &[Row] = &[
    Row { name: "makepad_script_derive", dir: "../../../platform/script/derive", module: Some("script"), features: &[] },
    Row { name: "makepad_script_derive_impl", dir: "../../../platform/script/derive_impl", module: None, features: &[] },
    // the wrapper's `default` and `nightly` features are unused markers; makepad_live_id (the base
    // set's) is an unused dependency of it, kept for the graph's validation and not linked here
    Row { name: "makepad_derive_widget", dir: "../../../widgets/derive_widget", module: Some("widget"), features: &["default", "nightly"] },
    Row { name: "makepad_derive_widget_impl", dir: "../../../widgets/derive_widget_impl", module: None, features: &[] },
];

fn main() {
    gen::generate(ROWS);
}
