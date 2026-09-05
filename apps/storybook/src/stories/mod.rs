//! One module per component. Each registers its templates under
//! `mod.stories` and exports a table of records; `tables()` lists them in
//! navigator order and `script_mod` registers them in the same order.
use crate::makepad_widgets::*;
use crate::registry::Story;

pub mod welcome;
pub mod button;

pub fn script_mod(vm: &mut ScriptVm) {
    welcome::script_mod(vm);
    button::script_mod(vm);
}

pub fn tables() -> &'static [&'static [Story]] {
    &[welcome::STORIES, button::STORIES]
}
