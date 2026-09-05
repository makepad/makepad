//! Which theme the catalogue runs under, and switching it while it runs.
//!
//! The library binds `theme` into the widget prelude between `theme_mod` and
//! `widgets_mod`, so the choice has to be made between those two calls. The
//! app's `script_mod` does exactly that, reading a process-wide choice. A
//! switch stores the choice and asks the platform for a live edit: the whole
//! `script_mod` runs again and every widget is re-applied from its template
//! under the new theme, in about ten milliseconds. Typed text and running
//! animations do not survive that, which is the price of a real switch.
use crate::makepad_widgets::*;
use std::sync::atomic::{AtomicU8, Ordering};

pub const NAMES: &[&str] = &["dark", "light", "skeleton"];

static CHOICE: AtomicU8 = AtomicU8::new(0);

pub fn choice() -> usize {
    CHOICE.load(Ordering::Relaxed) as usize
}

pub fn index_of(name: &str) -> Option<usize> {
    NAMES.iter().position(|n| *n == name)
}

/// Set the choice without applying it, for startup.
pub fn set_choice(index: usize) {
    CHOICE.store(index.min(NAMES.len() - 1) as u8, Ordering::Relaxed);
}

/// The library's `script_mod` split around the theme choice.
pub fn widgets_script_mod(vm: &mut ScriptVm) {
    crate::makepad_widgets::theme_mod(vm);
    match choice() {
        1 => {
            script_eval!(vm, {
                mod.theme = mod.themes.light
            });
        }
        2 => {
            script_eval!(vm, {
                mod.theme = mod.themes.skeleton
            });
        }
        _ => {}
    }
    crate::makepad_widgets::widgets_mod(vm);
}

/// Switch the running app to another theme.
pub fn select(cx: &mut Cx, index: usize) {
    if index >= NAMES.len() || index == choice() {
        return;
    }
    set_choice(index);
    crate::settings::set(crate::settings::THEME, NAMES[index]);
    log!("storybook: theme {}", NAMES[index]);
    cx.request_live_edit();
}
