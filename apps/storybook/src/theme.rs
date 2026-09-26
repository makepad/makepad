//! Which theme the catalogue runs under, and switching it while it runs.
//!
//! The library binds `theme` into the widget prelude between `theme_mod` and
//! `widgets_mod`, so the choice has to be made between those two calls. The
//! app's `script_mod` does exactly that, reading a process-wide choice. A
//! switch stores the choice and asks the platform for a live edit: the whole
//! `script_mod` runs again and every widget is re-applied from its template
//! under the new theme, in about ten milliseconds. Typed text and running
//! animations do not survive that, which is the price of a real switch.
use crate::makepad_widgets::desktop_style::{self, DesktopStyle, StyleSheet};
use crate::makepad_widgets::*;
use std::sync::atomic::{AtomicU8, Ordering};

/// The three themes the library is written in. Everything after them in the
/// list is a style sheet laid over one of these.
const BASE: &[&str] = &["dark", "light", "skeleton"];

static CHOICE: AtomicU8 = AtomicU8::new(0);

/// One thing the picker offers: a base theme, or a sheet and which of its
/// two appearances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    Base(usize),
    Sheet(DesktopStyle, bool),
}

/// Everything the picker offers, in the order it offers it: the base themes,
/// then every sheet the library ships, each followed by its dark appearance
/// where it has one.
///
/// Read off the library's own list rather than written out here, so a sheet
/// added there turns up in the catalogue without anybody remembering to add
/// it, and so the catalogue names none of them itself.
pub fn choices() -> Vec<Choice> {
    let mut out: Vec<Choice> = (0..BASE.len()).map(Choice::Base).collect();
    for style in DesktopStyle::ALL {
        out.push(Choice::Sheet(style, false));
        if style.supports_dark() {
            out.push(Choice::Sheet(style, true));
        }
    }
    out
}

impl Choice {
    /// What a settings file and the remote surface call it.
    pub fn name(self) -> String {
        match self {
            Choice::Base(i) => BASE[i].to_string(),
            Choice::Sheet(style, false) => style.id().to_string(),
            Choice::Sheet(style, true) => format!("{}-dark", style.id()),
        }
    }

    /// What the picker shows.
    pub fn label(self) -> String {
        match self {
            Choice::Base(i) => {
                let mut c = BASE[i].chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
            Choice::Sheet(style, false) => style.label().to_string(),
            Choice::Sheet(style, true) => format!("{} dark", style.label()),
        }
    }
}

pub fn names() -> Vec<String> {
    choices().into_iter().map(Choice::name).collect()
}

pub fn labels() -> Vec<String> {
    choices().into_iter().map(Choice::label).collect()
}

pub fn choice() -> usize {
    CHOICE.load(Ordering::Relaxed) as usize
}

pub fn index_of(name: &str) -> Option<usize> {
    names().iter().position(|n| n == name)
}

/// Set the choice without applying it, for startup.
pub fn set_choice(index: usize) {
    CHOICE.store(index.min(choices().len() - 1) as u8, Ordering::Relaxed);
}

/// The library's `script_mod` split around the theme choice.
/// Write a pick into the library, which is where it is read back from.
///
/// The base theme and the installed sheet both live in the library now, so
/// this list and the developer panel's own picker write to one place and
/// cannot disagree. This used to be done on every `script_mod` run, which
/// made whichever list ran last the winner: the panel's pick was undone a
/// tick after it was made.
pub fn apply_choice(vm: &mut ScriptVm, picked: Choice) {
    let base = match picked {
        Choice::Base(1) => BaseTheme::Light,
        Choice::Base(2) => BaseTheme::Skeleton,
        _ => BaseTheme::Dark,
    };
    set_base_theme(vm.cx_mut(), base);
    // A sheet goes on BEFORE `widgets_mod`, which reads it for its token
    // half as its first act; and comes off again for a base theme, or the
    // last sheet tried would stay under every theme picked after it.
    match picked {
        Choice::Sheet(style, dark) => {
            desktop_style::install(vm, StyleSheet::load_with_appearance(style, dark));
        }
        Choice::Base(_) => desktop_style::uninstall(vm),
    }
}

pub fn widgets_script_mod(vm: &mut ScriptVm) {
    crate::makepad_widgets::theme_mod(vm);
    crate::makepad_widgets::widgets_mod(vm);
    // The widget half of a sheet. The library's own `script_mod` does this
    // and this function stands in for it, so without the call a sheet's
    // tokens arrived and its buttons, fields and panels stayed as they were:
    // half a style, which is the trap `desktop_style` warns of by name.
    desktop_style::apply_widgets(vm);
}

/// Switch the running app to another theme.
pub fn select(cx: &mut Cx, index: usize) {
    let names = names();
    if index >= names.len() || index == choice() {
        return;
    }
    set_choice(index);
    crate::settings::set(crate::settings::THEME, &names[index]);
    log!("storybook: theme {}", names[index]);
    let picked = choices()[index];
    cx.with_vm(|vm| apply_choice(vm, picked));
    // A style reload re-runs `script_mod` and then applies with
    // `ScriptReapply` rather than `Reload`, so typed text and running
    // animations survive the switch. `request_script_reapply` alone would
    // not do: it does not re-run `script_mod`, and both halves of a switch
    // are emitted there.
    cx.request_style_reload();
}
