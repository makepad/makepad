//! mpvideo: a dumb video player for Makepad. See Cargo.toml for scope.

pub mod player;
pub mod preview;
pub mod theme;
pub mod widget;

/// Whether this run starts with the soundtrack muted.
///
/// A test or agent run must never be audible: someone at the machine should
/// not have a clip start talking at them out of a window they did not open.
/// `--mute` is the explicit switch; `MPVIDEO_MUTE=1` is the same thing for a
/// wrapper that cannot edit the command line; and a HIDDEN window counts as
/// well, because nobody is watching a window they cannot see, so nobody
/// should be hearing it either (`local/tools/gpu-guard` sets
/// `MAKEPAD_HIDE_WINDOWS=1` on every agent launch, so that path is covered
/// even when the flag is forgotten).
///
/// The audio path itself is untouched — this only starts at the muted end of
/// the volume knob, and `M` or the volume keys bring it back.
pub fn wants_mute(args: &[String], env: impl Fn(&str) -> Option<String>) -> bool {
    let truthy = |key: &str| {
        matches!(
            env(key).as_deref().map(str::trim),
            Some("1") | Some("true") | Some("yes")
        )
    };
    args.iter().any(|arg| arg == "--mute") || truthy("MPVIDEO_MUTE") || truthy("MAKEPAD_HIDE_WINDOWS")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_plain_run_makes_sound() {
        assert!(!wants_mute(&args(&["clip.mp4"]), |_| None));
        // A set-but-off environment variable is not consent to mute.
        assert!(!wants_mute(&args(&["clip.mp4"]), |k| (k == "MPVIDEO_MUTE")
            .then(|| "0".to_string())));
    }

    #[test]
    fn the_flag_the_variable_and_a_hidden_window_all_mute() {
        assert!(wants_mute(&args(&["--mute", "clip.mp4"]), |_| None));
        assert!(wants_mute(&args(&["clip.mp4"]), |k| (k == "MPVIDEO_MUTE")
            .then(|| "1".to_string())));
        // gpu-guard's hidden-window launch: muted without anyone asking.
        assert!(wants_mute(&args(&["clip.mp4"]), |k| (k == "MAKEPAD_HIDE_WINDOWS")
            .then(|| "1".to_string())));
        // A near miss is not the flag.
        assert!(!wants_mute(&args(&["--muted", "clip.mp4"]), |_| None));
    }
}
