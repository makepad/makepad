// This consumer deliberately declares no tweaker feature: expansion must use
// the defining crate's feature context, including cfg_attr(macro_use).
#[cfg_attr(all(), macro_use)]
extern crate cfg_macros_defs;

#[cfg(any())]
#[macro_use]
extern crate nonexistent;

use cfg_macros_defs::conditional_export as via_use;

struct Downstream;
cfg_macros_defs::tweaker_only! {
    impl cfg_macros_defs::Reflect for Downstream {
        fn reflected(&self) -> i32 { 91 }
    }
}

fn main() {
    let enabled = cfg_macros_defs::SHADER_TWEAKER;
    let selected = cfg_macros_defs::shader_tweaker_value!(1, 2);
    let _item = Downstream;
    let reflected = cfg_macros_defs::shader_tweaker_value!(cfg_macros_defs::Reflect::reflected(&_item), 92);
    println!("exports {} {} {} {}", cfg_macros_defs::FORWARD, cfg_macros_defs::exported!(), reversed!(), nested!());
    println!("textual {} {} {} {} {}", cfg_macros_defs::BEFORE, cfg_macros_defs::AFTER, cfg_macros_defs::AFTER_DISABLED, cfg_macros_defs::EARLIER, cfg_macros_defs::IMPORTED);
    println!("attributes {} {} {} {}", cfg_macros_defs::CONDITIONAL_LOCAL, cfg_macros_defs::CONDITIONAL_ROOT, via_use!(), nested_cfg_attr!());
    println!("tweaker {} {} {} {}", enabled, selected, cfg_macros_defs::local_reflect(), reflected);
}
