//! Shared Splash coverage for any text frontend, independent of draw or Cx.
use crate::{id, LiveId, ScriptMod, ScriptValue, ScriptVm};

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    vm.new_module(id!(glyph_coverage));
    vm.eval(ScriptMod::new(
        env!("CARGO_MANIFEST_DIR"),
        "makepad_script::glyph_coverage",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/glyph_coverage.splash"
        ),
        0,
        0,
        include_str!("../resources/glyph_coverage.splash"),
        &[],
    ))
}
