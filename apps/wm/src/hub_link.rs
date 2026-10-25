//! The WM's link to its AI hub: the hub child it owns plus the control client, the stub, or
//! a hub elsewhere. The snapshot, commands and clients are `makepad_ai_hub_panel::hub_control`.

use makepad_ai_hub_panel::hub_control::{compute_label, Admission, ComputeMode, FleetHub, HubCommand, HubControl, HubSnapshot, StubHub};

/// The hub the WM talks to, made at first use: the stub when asked for; a hub
/// elsewhere at `MAKEPAD_WM_HUB`; else the WM's own hub child (`hub_child`, unless its
/// settings turn it off or this build hosts no processes) and the control client
/// pointed at it.
#[derive(Default)]
pub struct HubLink {
    link: Option<Link>,
    /// This node's compute mode (`ai-hub.conf` `compute`) and whether a game is being
    /// played (`local_use`): what the hub was last told.
    local: Option<(ComputeMode, bool)>,
    /// The settings turn the WM's hub off: a hub an earlier WM left running is stopped.
    stop_leftover: bool,
}

enum Link {
    Stub(StubHub),
    Fleet {
        hub: FleetHub,
        #[cfg(not(target_arch = "wasm32"))]
        child: Option<crate::hub_child::HubChild>,
    },
}

impl HubLink {
    /// `processes`: this build can start child processes (`App::processes`).
    fn link(&mut self, processes: bool) -> &mut Link {
        self.link.get_or_insert_with(|| {
            if std::env::var_os("MAKEPAD_WM_HUB_STUB").is_some() {
                return Link::Stub(StubHub::default());
            }
            if let Ok(base) = std::env::var("MAKEPAD_WM_HUB") {
                return Link::Fleet {
                    hub: FleetHub::start(base),
                    #[cfg(not(target_arch = "wasm32"))]
                    child: None,
                };
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let settings = crate::hub_child::HubSettings::load();
                let base = settings.base_url();
                let child = (processes && settings.enabled).then(|| crate::hub_child::HubChild::new(settings));
                // The apps the WM starts chat with the model its hub loads.
                if let Some(model) = child.as_ref().map(|c| c.settings.model.as_str()).filter(|m| !m.is_empty()) {
                    crate::host::set_child_env("MAKEPAD_AI_MODEL", std::ffi::OsStr::new(model));
                }
                if child.is_none() {
                    makepad_widgets::log!("wm: the WM starts no AI hub (off in {}, or a hidden or in-process WM)", crate::hub_child::HubSettings::path().display());
                    // Only the settings' own `enabled = off`: a hidden test WM leaves the real one alone.
                    self.stop_leftover = processes && std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_none();
                }
                Link::Fleet { hub: FleetHub::start(base), child }
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = processes;
                Link::Fleet { hub: FleetHub::start("https://127.0.0.1:8123".to_string()) }
            }
        })
    }

    pub fn get(&mut self, processes: bool) -> &mut dyn HubControl {
        self.link(processes);
        self
    }

    /// Once a second: the hub child's exit, restart and start-up (a no-op without one).
    pub fn tick(&mut self, processes: bool, now: f64, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        #[cfg(not(target_arch = "wasm32"))]
        if std::mem::take(&mut self.stop_leftover) {
            crate::hub_child::stop_leftover(pool);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Link::Fleet { hub, child: Some(child) } = self.link(processes) {
            let (answers, model) = hub.peek(&child.settings.model);
            if child.tick(now, answers, model) {
                makepad_widgets::log!("wm: AI hub answers: loading {}", child.settings.model);
                hub.send(HubCommand::Load(child.settings.model.clone()));
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (processes, now, pool);
    }

    /// The WM quits: the hub keeps running for the next WM (Unix), or stops (Windows).
    pub fn shutdown(&mut self, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Link::Fleet { child: Some(child), .. }) = self.link.as_mut() {
            child.stop(pool);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = pool;
    }

    /// A command for the hub child (Restart, SetModel) or the hub. Restart needs the
    /// task pool to stop the old process.
    pub fn command(&mut self, command: HubCommand, now: f64, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        match (command, self.link.as_mut()) {
            #[cfg(not(target_arch = "wasm32"))]
            (HubCommand::Restart, Some(Link::Fleet { child: Some(child), .. })) => {
                let settings = crate::hub_child::HubSettings::load_or(&child.settings);
                child.restart(settings, now, pool);
            }
            #[cfg(not(target_arch = "wasm32"))]
            (HubCommand::SetModel(model), Some(Link::Fleet { child: Some(child), .. })) => {
                let mut settings = crate::hub_child::HubSettings::load_or(&child.settings);
                settings.model = model;
                if let Err(e) = settings.save() {
                    makepad_widgets::log!("wm: AI hub settings not saved: {e}");
                }
                crate::host::set_child_env("MAKEPAD_AI_MODEL", std::ffi::OsStr::new(&settings.model));
                child.restart(settings, now, pool);
            }
            #[cfg(not(target_arch = "wasm32"))]
            (HubCommand::SetCompute(mode), Some(Link::Fleet { child: Some(child), .. })) => {
                let mut settings = crate::hub_child::HubSettings::load_or(&child.settings);
                settings.compute = mode;
                if let Err(e) = settings.save() {
                    makepad_widgets::log!("wm: AI hub settings not saved: {e}");
                }
                child.settings.compute = mode;
                let gaming = self.local.map_or(false, |(_, g)| g);
                self.report_local(mode, gaming);
            }
            (HubCommand::SetCompute(mode), _) => {
                let gaming = self.local.map_or(false, |(_, g)| g);
                self.report_local(mode, gaming);
            }
            (HubCommand::Restart | HubCommand::SetModel(_), _) => {}
            (command, _) => self.send(command),
        }
    }

    /// The compute mode the hub runs under: the WM's own hub's setting, else Automatic.
    pub fn compute_mode(&mut self, processes: bool) -> ComputeMode {
        if let Some((mode, _)) = self.local {
            return mode;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Link::Fleet { child: Some(child), .. } = self.link(processes) {
            return child.settings.compute;
        }
        let _ = processes;
        ComputeMode::Automatic
    }

    /// Once a second: whether a game is being played (`local_use`). The hub hears of a
    /// change (and of the mode at the start).
    pub fn set_gaming(&mut self, processes: bool, gaming: bool) {
        let mode = self.compute_mode(processes);
        if self.local != Some((mode, gaming)) {
            if self.local.map_or(gaming, |(_, was)| was != gaming) {
                makepad_widgets::log!(
                    "wm: {} ({} mode): telling the AI hub",
                    if gaming { "a game is being played" } else { "play ended" },
                    compute_label(mode)
                );
            }
            self.report_local(mode, gaming);
        }
    }

    /// Only to the WM's own hub (or the stub): a hub elsewhere takes local use from its
    /// own machine only.
    fn report_local(&mut self, mode: ComputeMode, gaming: bool) {
        self.local = Some((mode, gaming));
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(self.link, Some(Link::Fleet { child: None, .. })) {
            return;
        }
        self.send(HubCommand::LocalUse { mode, gaming });
    }
}

impl HubControl for HubLink {
    fn active(&mut self) -> bool {
        match &mut self.link {
            Some(Link::Stub(h)) => h.active(),
            Some(Link::Fleet { hub, .. }) => hub.active(),
            None => false,
        }
    }
    fn snapshot(&mut self) -> HubSnapshot {
        match &mut self.link {
            Some(Link::Stub(h)) => h.snapshot(),
            #[cfg(not(target_arch = "wasm32"))]
            Some(Link::Fleet { hub, child }) => {
                let mut snap = hub.snapshot();
                snap.process = child.as_ref().map(|c| c.process());
                snap
            }
            #[cfg(target_arch = "wasm32")]
            Some(Link::Fleet { hub }) => hub.snapshot(),
            None => HubSnapshot::default(),
        }
    }
    fn send(&mut self, command: HubCommand) {
        match &mut self.link {
            Some(Link::Stub(h)) => h.send(command),
            Some(Link::Fleet { hub, .. }) => hub.send(command),
            None => {}
        }
    }
    fn take_report(&mut self) -> Option<String> {
        match &mut self.link {
            Some(Link::Stub(h)) => h.take_report(),
            Some(Link::Fleet { hub, .. }) => hub.take_report(),
            None => None,
        }
    }
    fn admission(&mut self) -> Option<Admission> {
        match &mut self.link {
            Some(Link::Stub(h)) => h.admission(),
            Some(Link::Fleet { hub, .. }) => hub.admission(),
            None => None,
        }
    }
}
