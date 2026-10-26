//! This node's devhub (apps/devhub): the JIT observer the user's master reaches, kept by the WM
//! with the same supervisor as the AI Hub (`hub_app::Supervised`): `cargo run --release -p
//! makepad-app-devhub` in the checkout (never a stale binary), restarted with backoff after an
//! exit, kept across WM restarts, off after its Quit until its dim off slot in the tray area starts it again. Its
//! settings are `~/.makepad/wm/devhub.conf` (`MAKEPAD_WM_DEVHUB_CONF`; a hidden WM without one
//! starts none). Its tray icon sits in the WM's tray area; it has no tile, no dock item and no
//! launcher row.

#[derive(Default)]
pub struct DevhubLink {
    /// None: not decided yet; Some(None): this WM runs none (off, hidden, in-process).
    #[cfg(not(target_arch = "wasm32"))]
    app: Option<Option<crate::hub_app::DevhubApp>>,
}

impl DevhubLink {
    #[cfg(not(target_arch = "wasm32"))]
    fn app(&mut self, processes: bool) -> Option<&mut crate::hub_app::DevhubApp> {
        if self.app.is_none() {
            let (path, conf) = crate::hub_app::load_devhub_conf();
            self.app = Some(if processes && conf.enabled {
                Some(crate::hub_app::DevhubApp::new(&crate::hub_app::DEVHUB, path, conf))
            } else {
                makepad_widgets::log!("wm: the WM starts no devhub (off in {}, or a hidden or in-process WM)", path.display());
                None
            });
        }
        self.app.as_mut().and_then(Option::as_mut)
    }

    /// The pid of the devhub this WM runs: a process the WM socket admits under it is a service.
    pub fn app_pid(&self) -> Option<u32> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Some(app)) = self.app.as_ref() {
            return app.pid();
        }
        None
    }

    /// Its off slot was pressed: on again in its settings, and started.
    pub fn start_from_launcher(&mut self, processes: bool, now: f64) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !processes || std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() && std::env::var_os("MAKEPAD_WM_DEVHUB_CONF").is_none() {
                return false;
            }
            let (path, _) = crate::hub_app::load_devhub_conf();
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let kept: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("enabled")).collect();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&path, format!("enabled = on\n{}", kept.iter().map(|l| format!("{l}\n")).collect::<String>())) {
                makepad_widgets::log!("wm: devhub: {}: {e}", path.display());
                return false;
            }
            if !matches!(self.app, Some(Some(_))) {
                self.app = None;
            }
            if let Some(app) = self.app(processes) {
                app.user_start(now);
            }
            true
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (processes, now);
            false
        }
    }

    /// devhub is off by its settings (its Quit): the bar keeps a dim slot to start it again. A
    /// hidden WM without its own settings file runs none and shows no slot.
    pub fn is_off(&self, processes: bool) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !processes || std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() && std::env::var_os("MAKEPAD_WM_DEVHUB_CONF").is_none() {
                return false;
            }
            !crate::hub_app::load_devhub_conf().1.enabled
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = processes;
            false
        }
    }

    /// Once a second: exit and restart.
    pub fn tick(&mut self, processes: bool, now: f64) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(app) = self.app(processes) {
            app.tick(now);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (processes, now);
    }

    /// The WM quits: devhub keeps running for the next WM (Unix), or stops (Windows).
    pub fn shutdown(&mut self, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Some(app)) = self.app.as_mut() {
            app.stop(pool);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = pool;
    }
}
