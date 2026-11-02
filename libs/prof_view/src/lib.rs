//! The profile view: attach to a running makepad app over its makepad-remote endpoint and look
//! inside it: the profiler (timeline and query answers over the app's `prof` and `gpuprof`
//! streams), recordings (.mkpf) and their diff, and the app's own inspect commands (panics,
//! threads, memory, code versions) over the same link.

pub mod answer;
pub mod panel;
pub mod session;
pub mod timeline;

pub use makepad_prof_stream;
use makepad_widgets::*;

pub(crate) fn answer_wrap(s: &str, width: f64, char_w: f64) -> Vec<String> {
    answer::ProfAnswer::wrap(s, width, char_w)
}

/// Registers the Live view's widgets (`ProfView`, `ProfTimeline`, `ProfAnswer`, `ProfList`).
pub fn script_mod(vm: &mut ScriptVm) {
    timeline::script_mod(vm);
    answer::script_mod(vm);
    panel::script_mod(vm);
}
