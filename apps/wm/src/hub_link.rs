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
    ui_reserve_mb: Option<u64>,
    /// Game state and reserve last reported to the hub.
    local_use: Option<(bool, Option<u64>)>,
    /// The settings turn the WM's hub off: an app an earlier WM left running is stopped.
    stop_leftover: bool,
    /// No LAN address yet (a boot before the DHCP lease): when it was last looked for. The
    /// link is made once one is there.
    lan_wait: Option<std::time::Instant>,
}

/// How often a WM that found no LAN address for its hub looks again.
pub(crate) const LAN_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// What [`HubLink::make`] decided.
enum Made {
    Link(Link),
    /// This WM runs no hub (the settings, a hidden or in-process WM).
    None,
    /// No LAN address to serve on yet: decided again a moment later.
    NoLanYet,
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
    pub fn set_ui_reserve_mb(&mut self, mb: Option<u64>) {
        self.ui_reserve_mb = mb;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Some(link)) = self.link.as_mut() {
            link.app.set_ui_reserve_mb(mb);
        }
    }

    /// `processes`: this build can start child processes (`App::processes`). Decided at first
    /// use.
    fn link(&mut self, processes: bool) -> Option<&mut Link> {
        if self.link.is_none() && !self.lan_wait.is_some_and(|at| at.elapsed() < LAN_RETRY) {
            match self.make(processes) {
                Made::Link(link) => {
                    if self.lan_wait.take().is_some() {
                        makepad_widgets::log!("wm: a LAN address is up: starting the AI Hub");
                    }
                    self.link = Some(Some(link));
                }
                Made::None => self.link = Some(None),
                Made::NoLanYet => {
                    if self.lan_wait.replace(std::time::Instant::now()).is_none() {
                        makepad_widgets::log!("wm: this machine has no LAN address for the AI Hub to serve on yet: looking again every {} s", LAN_RETRY.as_secs());
                    }
                }
            }
        }
        self.link.as_mut().and_then(Option::as_mut)
    }

    fn make(&mut self, processes: bool) -> Made {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (path, conf) = crate::hub_app::load_conf();
            if !(processes && conf.enabled) {
                makepad_widgets::log!("wm: the WM starts no AI Hub (off in {}, or a hidden, leased, second or in-process WM)", path.display());
                // Only the settings' own `enabled = off`: a guest WM leaves the real one alone.
                self.stop_leftover = processes && !crate::hub_app::guest_wm();
                return Made::None;
            }
            let Some(base) = conf.base_url() else { return Made::NoLanYet };
            // The apps the WM starts chat with the model its hub loads.
            if !conf.model.is_empty() {
                crate::host::set_child_env("MAKEPAD_AI_MODEL", std::ffi::OsStr::new(&conf.model));
            }
            let mut app = crate::hub_app::HubApp::new(&crate::hub_app::AI_HUB, path, conf);
            app.set_ui_reserve_mb(self.ui_reserve_mb);
            Made::Link(Link { hub: FleetHub::start(base), app })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = processes;
            Made::None
        }
    }

    /// The pid of the hub app this WM runs, once started (`hub_app`): a process the WM socket
    /// admits under this pid is the node's hub, a service the WM keeps: no tile.
    pub fn app_pid(&self) -> Option<u32> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Some(link)) = self.link.as_ref() {
            return link.app.pid();
        }
        None
    }

    /// The WM session restart its app's start needs (the user's service manager is down).
    #[cfg(target_os = "linux")]
    pub fn session_restart(&self) -> Option<crate::hub_app::SessionRestart> {
        self.link.as_ref()?.as_ref()?.app.session_restart().cloned()
    }

    /// The hub is off in the node's settings (`enabled = off`, its Quit): the bar keeps a dim
    /// slot whose click starts it. False for a WM that runs none of its own (hidden, in-process).
    pub fn is_off(&self, processes: bool) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !processes || crate::hub_app::stays_out(makepad_ai_hub::node_conf::CONF_ENV) {
                return false;
            }
            !makepad_ai_hub::node_conf::NodeConf::load_from(&makepad_ai_hub::node_conf::NodeConf::path()).enabled
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = processes;
            false
        }
    }

    /// The person asked for the AI Hub from the launcher: after its Quit (or with the settings
    /// off) it is turned on again (`enabled = on` in `ai-hub.conf`) and starts; with it running
    /// there is nothing to do. False where this WM runs no hub of its own (a guest or
    /// in-process WM), so the launcher falls back to its ordinary start.
    pub fn start_from_launcher(&mut self, processes: bool, now: f64) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !processes || crate::hub_app::stays_out(makepad_ai_hub::node_conf::CONF_ENV) {
                return false;
            }
            let path = makepad_ai_hub::node_conf::NodeConf::path();
            let mut conf = makepad_ai_hub::node_conf::NodeConf::load_from(&path);
            if !conf.enabled {
                conf.enabled = true;
                if let Err(e) = conf.save_to(&path) {
                    makepad_widgets::log!("wm: AI Hub: {e}");
                    return false;
                }
                makepad_widgets::log!("wm: AI Hub: started from the launcher; {} says on again", path.display());
            }
            // A WM that ran none (off at its start) makes the link now.
            if !matches!(self.link, Some(Some(_))) {
                self.link = None;
                self.stop_leftover = false;
            }
            if let Some(link) = self.link(processes) {
                link.app.user_start(now);
            }
            true
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (processes, now);
            false
        }
    }

    /// Once a second: the app's exit and restart (a no-op without one).
    pub fn tick(&mut self, processes: bool, now: f64, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        let link = self.link(processes);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(link) = link {
            link.app.set_jobs(link.hub.jobs());
            if let Some(outcome) = link.hub.take_idle_stop() {
                link.app.idle_stop(outcome);
            }
            link.app.tick(now);
            // its swap's stop runs on the control client's worker, right after a fresh read
            if let Some(pid) = link.app.take_stop_ask() {
                link.hub.stop_when_idle(Box::new(move || {
                    #[cfg(unix)]
                    makepad_app_host::launch::signal_group(pid, false);
                    #[cfg(not(unix))]
                    let _ = pid;
                }));
            }
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
        let was = self.local_use;
        let ui_reserve_mb = self.ui_reserve_mb;
        if was == Some((gaming, ui_reserve_mb)) {
            return;
        }
        let Some(link) = self.link(processes) else { return };
        if was.is_some_and(|(prev, _)| prev != gaming) {
            makepad_widgets::log!("wm: {}: telling the AI hub", if gaming { "a game is being played" } else { "play ended" });
        }
        link.hub.send(HubCommand::LocalUse { gaming, ui_reserve_mb });
        self.local_use = Some((gaming, ui_reserve_mb));
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
