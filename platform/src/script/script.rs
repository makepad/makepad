use crate::live_reload::CxLiveReloadState;
use crate::script::res::*;
use crate::script::timer::*;
use makepad_script_std::ScriptStd;

#[derive(Default)]
pub struct CxScriptData {
    pub std: ScriptStd,
    pub random_seed: u64,
    pub timers: CxScriptTimers,
    pub resources: CxScriptResources,
    /// Shared reference to the VM's crate_manifests so we can access them
    /// even when script_vm is temporarily taken by with_vm during script eval.
    pub crate_manifests: crate::makepad_script::ScriptCrateManifests,
    pub live_reload: CxLiveReloadState,
}
