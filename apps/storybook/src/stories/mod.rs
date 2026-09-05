//! One module per component. Each registers its templates under
//! `mod.stories` and exports a table of records; `tables()` lists them in
//! navigator order and `script_mod` registers them in the same order.
use crate::makepad_widgets::*;
use crate::registry::Story;
use std::collections::HashMap;
use std::sync::Mutex;

pub mod basics;
pub mod welcome;

pub fn script_mod(vm: &mut ScriptVm) {
    welcome::script_mod(vm);
    crate::coverage::script_mod(vm);
    basics::script_mod(vm);
}

pub fn tables() -> &'static [&'static [Story]] {
    &[welcome::STORIES, crate::coverage::STORIES, basics::STORIES]
}

static COUNTERS: Mutex<Option<HashMap<LiveId, usize>>> = Mutex::new(None);

/// A per-story counter for demos that count their own clicks; returns the
/// count after the bump.
pub fn bump(key: LiveId) -> usize {
    let mut guard = COUNTERS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let n = map.entry(key).or_insert(0);
    *n += 1;
    *n
}
