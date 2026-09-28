//! Delivery of [`Event::AppOpen`](crate::event::Event::AppOpen), and the
//! opt-in single instance (`AppMain::single_instance`).
//!
//! Every source (the launch command line, a later launch of a
//! single-instance app, the OS delegates and intents) posts an
//! [`AppOpenPosted`] from whatever thread it runs on; the UI thread's action
//! pump (`Cx::handle_action_receiver`) turns it into the event, so every
//! backend delivers it the same way and only after `Event::Startup`.

use crate::cx::Cx;
use crate::event::{AppOpenEvent, AppOpenItem, AppOpenSource};
use std::path::Path;

#[derive(Debug)]
pub(crate) struct AppOpenPosted(pub AppOpenEvent);

/// Queue an `Event::AppOpen` for the UI thread. Callable from any thread
/// once the `Cx` exists.
pub(crate) fn post(items: Vec<AppOpenItem>, source: AppOpenSource) {
    Cx::post_action(AppOpenPosted(AppOpenEvent { items, source }));
}

/// What a command line asks to open: its positional arguments (everything
/// after the program that is not a `-flag`, and everything after `--`),
/// relative paths joined to `cwd`.
pub fn items_from_args(
    args: impl IntoIterator<Item = String>,
    cwd: Option<&Path>,
) -> Vec<AppOpenItem> {
    let mut positional_only = false;
    let mut items = Vec::new();
    for arg in args {
        if !positional_only && arg == "--" {
            positional_only = true;
        } else if positional_only || !arg.starts_with('-') {
            if !arg.is_empty() {
                items.push(AppOpenItem::parse(&arg, cwd));
            }
        }
    }
    items
}

fn launch_items() -> Vec<AppOpenItem> {
    let cwd = std::env::current_dir().ok();
    items_from_args(
        std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned()),
        cwd.as_deref(),
    )
}

/// `app_main!`, once the `Cx` exists: the command line's items as the
/// `Launch` event.
#[doc(hidden)]
pub fn post_launch_args() {
    let items = launch_items();
    if !items.is_empty() {
        post(items, AppOpenSource::Launch);
    }
}

/// What `app_main!` does with a single-instance launch.
#[doc(hidden)]
pub enum SingleInstance {
    /// This process is the app's instance; it serves later launches once
    /// its `Cx` exists ([`SingleInstanceServer::serve`]).
    Primary(SingleInstanceServer),
    /// The running instance took this launch's command line: exit.
    Forwarded,
    /// Run as an instance of its own (not claimed, or the handoff failed).
    Standalone,
}

#[cfg(all(
    not(gpusim),
    any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    )
))]
mod native {
    use super::*;
    use crate::os::single_instance::{self as si, Claim, HANDOFF_TIMEOUT};

    #[doc(hidden)]
    pub struct SingleInstanceServer {
        #[cfg(unix)]
        server: si::unix::Server,
        #[cfg(windows)]
        server: si::windows_pipe::Server,
    }

    impl SingleInstanceServer {
        /// Start taking later launches, each delivered as a
        /// `SecondInstance` `Event::AppOpen`.
        pub fn serve(self) {
            self.server.serve(Box::new(|items: Vec<String>| {
                // The sender made its paths absolute.
                let items = items.iter().map(|item| AppOpenItem::parse(item, None)).collect();
                post(items, AppOpenSource::SecondInstance);
            }));
        }
    }

    /// Instances started for an agent, a test or a host run on their own:
    /// a `--remote` or hidden run must never land in the user's window, and
    /// a hosted child belongs to its host. `MAKEPAD_NEW_INSTANCE=1` asks for
    /// the same by hand.
    fn exempt() -> bool {
        std::env::var_os("MAKEPAD_NEW_INSTANCE").is_some()
            || std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some()
            || crate::remote::requested()
            || crate::app_main::should_run_stdin_loop_from_env()
    }

    pub fn claim_single_instance(app_name: &str) -> SingleInstance {
        if exempt() {
            return SingleInstance::Standalone;
        }
        let exe = std::env::current_exe()
            .and_then(|exe| exe.canonicalize())
            .unwrap_or_default();
        let key = si::instance_key(app_name, &exe);
        let items: Vec<String> = launch_items().iter().map(AppOpenItem::to_arg).collect();
        #[cfg(unix)]
        let claim = match si::unix::runtime_dir() {
            Some(dir) => si::unix::claim(&dir, &key, &items, HANDOFF_TIMEOUT),
            None => Claim::Standalone,
        };
        #[cfg(windows)]
        let claim = si::windows_pipe::claim(&si::windows_pipe::pipe_name(&key), &items, HANDOFF_TIMEOUT);
        match claim {
            Claim::Primary(server) => SingleInstance::Primary(SingleInstanceServer { server }),
            Claim::Forwarded => SingleInstance::Forwarded,
            Claim::Standalone => SingleInstance::Standalone,
        }
    }
}

#[cfg(not(all(
    not(gpusim),
    any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    )
)))]
mod native {
    use super::*;

    /// No single instance here: the OS keeps one (mobile) or there is no
    /// process to share (web, the simulated GPU).
    #[doc(hidden)]
    pub struct SingleInstanceServer(());

    impl SingleInstanceServer {
        pub fn serve(self) {}
    }

    pub fn claim_single_instance(_app_name: &str) -> SingleInstance {
        SingleInstance::Standalone
    }
}

/// `app_main!`, for an app whose `AppMain::single_instance` is true: claim
/// the app's name for this process, or hand this launch to the running
/// instance. Before the `Cx`, so a handed-off launch costs no VM or window.
#[doc(hidden)]
pub use native::{claim_single_instance, SingleInstanceServer};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn items_from_args_takes_positionals() {
        let args = ["--remote", "a.png", "-v", "https://x.org/", "--", "-dash.txt"]
            .map(String::from);
        assert_eq!(
            items_from_args(args, Some(Path::new("/w"))),
            vec![
                AppOpenItem::Path(PathBuf::from("/w/a.png")),
                AppOpenItem::Url("https://x.org/".into()),
                AppOpenItem::Path(PathBuf::from("/w/-dash.txt")),
            ]
        );
    }
}
