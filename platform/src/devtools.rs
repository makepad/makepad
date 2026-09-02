//! The opt-in switch for makepad's in-app developer overlays.
//!
//! Three of them exist, and each one binds a bare function key and then claims
//! input the app never sees:
//!
//! * **F10** — the exploded draw-list view ([`crate::sploded`]). Intercepted in
//!   `Cx::call_event_handler` *before* the app's handler, and once it is up it
//!   also claims Escape, the arrow keys, `+`/`-`/`0`, `I` and `H` — no modifier
//!   required — plus every pointer drag outside the declared flat band.
//! * **F12** — the design tweaker (`makepad_widgets::tweaker`), a child of every
//!   `Window`. Once it is up it swallows every pointer event over the body.
//! * **Shift+F12** — the screen recorder (`makepad_widgets::screen_cap`), which
//!   writes mp4 files next to the running process.
//!
//! These are development tools, so they stay off unless a developer asks for
//! them. A shipped app is not a place to discover that a stray F10 tilts the
//! whole UI into 3D and stops Escape from closing anything.
//!
//! Turn them on with `--devtools` on the command line, or `MAKEPAD_DEVTOOLS=1`
//! in the environment. `--remote` implies them: the remote control surface's
//! `/snap` + `/click` loop drives the tweaker, so a remote-driven app has
//! already opted in to being instrumented. An explicit `MAKEPAD_DEVTOOLS=0`
//! wins over all of it, which is also how the off path stays testable under
//! `--remote`.
//!
//! Only the *hotkeys* are gated, not the tools. An app that wants one of these
//! on its own terms still calls `Cx::sploded_toggle`, `tweaker::set_tweak_on`
//! or `ScreenCap::toggle` directly — that is the app deciding, rather than a
//! key nobody knew was bound.

use std::sync::OnceLock;

/// Whether this process opted into the developer overlays.
///
/// Scans argv and the environment once and caches the answer, so the hot event
/// path pays an atomic load. See the module docs for what this gates.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        decide(
            std::env::args().any(|a| a == "--devtools"),
            std::env::var("MAKEPAD_DEVTOOLS").ok().as_deref(),
            crate::remote::requested(),
        )
    })
}

/// The whole decision, with the process pulled out so it can be tested.
///
/// `MAKEPAD_DEVTOOLS` takes the usual off-ish spellings, so it can sit in a
/// shell profile as `0` instead of having to be unset — and because it is the
/// one explicit signal, an off spelling also overrides `--devtools` and
/// `--remote`.
fn decide(flag: bool, env: Option<&str>, remote: bool) -> bool {
    if let Some(env) = env {
        let env = env.trim().to_ascii_lowercase();
        return !matches!(env.as_str(), "" | "0" | "off" | "false" | "no");
    }
    flag || remote
}

#[cfg(test)]
mod tests {
    use super::decide;

    #[test]
    fn off_by_default() {
        assert!(!decide(false, None, false));
    }

    #[test]
    fn the_flag_or_remote_turns_it_on() {
        assert!(decide(true, None, false));
        assert!(decide(false, None, true));
    }

    #[test]
    fn the_env_var_turns_it_on() {
        for on in ["1", "yes", "true", "on", " 1 "] {
            assert!(decide(false, Some(on), false), "{on:?} should enable");
        }
    }

    #[test]
    fn an_off_spelling_wins_over_the_flag_and_remote() {
        // So `MAKEPAD_DEVTOOLS=0` can live in a profile, and so the gated-off
        // path is still reachable under --remote.
        for off in ["0", "off", "false", "no", "", "  ", "OFF"] {
            assert!(!decide(true, Some(off), true), "{off:?} should disable");
        }
    }
}
