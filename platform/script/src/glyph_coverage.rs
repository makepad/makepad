//! Shared Splash coverage for any text frontend, independent of draw or Cx.
use crate::{id, LiveId, ScriptMod, ScriptValue, ScriptVm};

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    // This hand-written registration needs the same dependency boundary as
    // script_mod!: spreads read their source before touch slots are mapped.
    vm.census_begin(module_path!());
    vm.new_module(id!(glyph_coverage));
    let value = vm.eval(ScriptMod::new(
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
    ));
    vm.census_end();
    value
}
