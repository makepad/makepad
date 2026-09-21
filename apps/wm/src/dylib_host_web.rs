//! The dylib host's stand-in on the web. Compiling an app crate to a dylib
//! and `dlopen`-ing it (dylib_host.rs) needs a child process, a linker and a
//! loader, and a browser has none of them. The window manager keeps one
//! shape on every target, so this carries the same surface and refuses: an
//! app asked for as a dylib reports that the web cannot load one, and
//! nothing is ever seated from here.

use crate::clients::{AppDef, ClientLine};
use crate::hub::ClientId;
use makepad_app_module::AppModule;
use makepad_widgets::makepad_platform::thread::ThreadSpawner;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

/// The client id the native host's provisioning lines are written under.
pub const PROVISION_CLIENT: ClientId = 0;

const NOT_ON_THE_WEB: &str = "app dylibs cannot be compiled or loaded on the web";

pub struct CompileDone {
    pub client: ClientId,
    pub app_id: String,
    pub home_tile: bool,
    pub result: Result<PathBuf, String>,
}

pub struct DylibHost {
    refused: Vec<CompileDone>,
}

impl DylibHost {
    pub fn new(_lines: Sender<ClientLine>) -> Self {
        DylibHost { refused: Vec::new() }
    }

    pub fn get(&self, _app_id: &str) -> Option<&'static dyn AppModule> {
        None
    }

    /// Every compile asked of the web host comes back refused, through the
    /// same queue the native host answers on, so the caller's one path for
    /// a failed compile is the path here too.
    pub fn drain_done(&mut self) -> Vec<CompileDone> {
        std::mem::take(&mut self.refused)
    }

    pub fn load_path(&mut self, _app_id: &str, _path: &Path) -> Result<&'static dyn AppModule, String> {
        Err(NOT_ON_THE_WEB.to_string())
    }

    pub fn compile(
        &mut self,
        _spawner: &ThreadSpawner,
        app: AppDef,
        client: ClientId,
        home_tile: bool,
        _data_dir: Option<String>,
    ) {
        self.refused.push(CompileDone {
            client,
            app_id: app.id.clone(),
            home_tile,
            result: Err(NOT_ON_THE_WEB.to_string()),
        });
    }
}
