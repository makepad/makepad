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
    pub live_reload: CxLiveReloadState,
}
