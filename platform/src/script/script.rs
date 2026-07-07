use crate::live_reload::CxLiveReloadState;
use crate::script::res::*;
use crate::script::timer::*;
use makepad_script_std::ScriptStd;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Default)]
pub struct CxScriptData {
    pub std: ScriptStd,
    pub random_seed: u64,
    pub timers: CxScriptTimers,
    pub resources: CxScriptResources,
    /// Shared reference to the VM's crate_manifests so we can access them
    /// even when script_vm is temporarily taken by with_vm during script eval.
    pub crate_manifests: Rc<RefCell<HashMap<String, String>>>,
    pub live_reload: CxLiveReloadState,
    /// Caches system-font resolution by query so repeated families (e.g. every
    /// theme asking for `system_font("ui", 400, false)`) reuse one set of bytes
    /// and we never hit the OS font API more than once per distinct query.
    /// `None` means the OS could not resolve a font for that query (negative
    /// cache — the member is then skipped gracefully at render time).
    pub system_font_bytes: RefCell<HashMap<crate::cx_api::SystemFontQuery, Option<Rc<crate::cx_api::SystemFontResult>>>>,
}
