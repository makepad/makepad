//! The WM's link to this node's AI hub: the AI Hub app it starts and keeps running
//! (`hub_app`) and a thin control client for what only the WM knows (`hub_panel::hub_control::
//! FleetHub`): whether a game is being played (`POST /control/local-use`, `local_use`) and the
//! GPU memory the UI needs back (`POST /control/yield`, `gpu_yield`). The hub's panel is the
//! app's own window now; the compute mode is the app's (its picker, saved in `ai-hub.conf`):
//! the WM reads it from the hub's admission and never sets it.

use makepad_ai_hub_panel::hub_control::{ComputeMode, FleetHub, HubCommand};

#[derive(Default)]
pub struct HubLink {
    /// Decided at first use: `Some(None)` when this WM runs no hub.
    link: Option<Option<Link>>,
    /// Whether a game is being played (`local_use`): what the hub was last told.
    gaming: Option<bool>,
    /// The settings turn the WM's hub off: an app an earlier WM left running is stopped.
    stop_leftover: bool,
}

/// This WM's own hub: the app and the control client pointed at it. None: the settings turn
/// it off, or this is a hidden or in-process WM (a hub elsewhere takes local use from its own
/// machine only, so there is nothing to tell).
struct Link {
    #[cfg(not(target_arch = "wasm32"))]
    app: crate::hub_app::HubApp,
    hub: FleetHub,
}

impl HubLink {
    /// `processes`: this build can start child processes (`App::processes`). Decided at first
    /// use.
    fn link(&mut self, processes: bool) -> Option<&mut Link> {
        if self.link.is_none() {
            let made = self.make(processes);
            self.link = Some(made);
        }
        self.link.as_mut().and_then(Option::as_mut)
    }

    fn make(&mut self, processes: bool) -> Option<Link> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (path, conf) = crate::hub_app::load_conf();
            if !(processes && conf.enabled) {
                makepad_widgets::log!("wm: the WM starts no AI Hub (off in {}, or a hidden or in-process WM)", path.display());
                // Only the settings' own `enabled = off`: a hidden test WM leaves the real one alone.
                self.stop_leftover = processes && std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_none();
                return None;
            }
            let Some(base) = conf.base_url() else {
                makepad_widgets::log!("wm: this machine has no LAN address for the AI Hub to serve on: starting none");
                return None;
            };
            // The apps the WM starts chat with the model its hub loads.
            if !conf.model.is_empty() {
                crate::host::set_child_env("MAKEPAD_AI_MODEL", std::ffi::OsStr::new(&conf.model));
            }
            Some(Link { hub: FleetHub::start(base), app: crate::hub_app::HubApp::new(path, conf) })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = processes;
            None
        }
    }

    /// Once a second: the app's exit and restart (a no-op without one).
    pub fn tick(&mut self, processes: bool, now: f64, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        let link = self.link(processes);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(link) = link {
            link.app.tick(now);
            return;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (link, now);
        #[cfg(not(target_arch = "wasm32"))]
        if std::mem::take(&mut self.stop_leftover) {
            crate::hub_app::stop_leftover(pool);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = pool;
    }

    /// The WM quits: the app keeps running for the next WM (Unix), or stops (Windows).
    pub fn shutdown(&mut self, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Some(link)) = self.link.as_mut() {
            link.app.stop(pool);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = pool;
    }

    /// The compute mode the hub runs under (the app's), as last read; Automatic until it answers.
    pub fn compute_mode(&mut self, processes: bool) -> ComputeMode {
        self.link(processes).and_then(|l| l.hub.admission()).map_or(ComputeMode::Automatic, |a| a.mode)
    }

    /// Once a second: whether a game is being played (`local_use`). The hub hears of a change
    /// (and of the state at the start; the control client tells a restarted hub again).
    pub fn set_gaming(&mut self, processes: bool, gaming: bool) {
        let was = self.gaming;
        if was == Some(gaming) {
            return;
        }
        let Some(link) = self.link(processes) else { return };
        if was.is_some() {
            makepad_widgets::log!("wm: {}: telling the AI hub", if gaming { "a game is being played" } else { "play ended" });
        }
        link.hub.send(HubCommand::LocalUse { gaming });
        self.gaming = Some(gaming);
    }

    /// Give GPU memory back to the UI (`gpu_yield`).
    pub fn send(&mut self, processes: bool, command: HubCommand) {
        if let Some(link) = self.link(processes) {
            link.hub.send(command);
        }
    }

    /// The hub's report of the last finished yield, once.
    pub fn take_report(&mut self, processes: bool) -> Option<String> {
        self.link(processes).and_then(|l| l.hub.take_report())
    }
}
