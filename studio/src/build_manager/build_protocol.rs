use crate::{
    makepad_live_id::LiveId,
    makepad_platform::log::LogLevel,
    makepad_code_editor::text::{Position},
    makepad_micro_serde::{SerBin, DeBin, DeBinErr},
};


#[derive(PartialEq, Clone, Copy, Debug, SerBin, DeBin)]
pub struct BuildCmdId(pub u64);

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuildTarget {
    Release,
    Debug,
    ReleaseStudio,
    DebugStudio,
    Profiler,
    IosSim,
    IosDevice,
    TvosSim,
    TvosDevice,
    Android,
    WebAssembly,
    CheckMacos,
    CheckWindows,
    CheckLinux,
    CheckAll,
}

impl BuildTarget {
    pub fn runs_in_studio(&self)->bool{
        match self{
            Self::ReleaseStudio=>true,
            Self::DebugStudio=>true,
            _=>false
        }
    }
    
    pub const RELEASE_STUDIO:u64 = 0;
    pub const DEBUG_STUDIO:u64 = 1;
    pub const RELEASE:u64 = 2;
    pub const DEBUG:u64 = 3;
    pub const PROFILER:u64 = 4;
    pub const IOS_SIM:u64 = 5;
    pub const IOS_DEVICE:u64 = 6;
    pub const TVOS_SIM:u64 = 7;
    pub const TVOS_DEVICE:u64 = 8;
    pub const ANDROID:u64 = 9;
    pub const WEBASSEMBLY:u64 = 10;
    pub const CHECK_MACOS:u64 = 11;
    pub const CHECK_WINDOWS:u64 = 12;
    pub const CHECK_LINUX:u64 = 13;
    pub const CHECK_ALL:u64 = 14;
    pub fn len() -> u64 {Self::CHECK_ALL+1}
    pub fn name(&self) -> &'static str {
        match self {
            Self::ReleaseStudio=>"Release Studio",
            Self::DebugStudio=>"Debug Studio",
            Self::Release=>"Release",
            Self::Debug=>"Debug",
            Self::Profiler=>"Profiler",
            Self::IosSim=>"iOS Simulator",
            Self::IosDevice=>"iOS Device",
            Self::TvosSim=>"TVOs Simulator",
            Self::TvosDevice=>"TVOs Device",
            Self::Android=>"Android",
            Self::WebAssembly=>"WebAssembly",
            Self::CheckMacos=>"Check Macos",
            Self::CheckWindows=>"Check Windows",
            Self::CheckLinux=>"Check Linux",
            Self::CheckAll=>"Check All",
        }
    }
    pub fn as_id(&self) -> u64 {
        match self {
            Self::ReleaseStudio=>Self::RELEASE_STUDIO,
            Self::DebugStudio=>Self::DEBUG_STUDIO,
            Self::Release=>Self::RELEASE,
            Self::Debug=>Self::DEBUG,
            Self::Profiler=>Self::PROFILER,
            Self::IosSim{..}=>Self::IOS_SIM,
            Self::IosDevice{..}=>Self::IOS_DEVICE,
            Self::TvosSim{..}=>Self::TVOS_SIM,
            Self::TvosDevice{..}=>Self::TVOS_DEVICE,
            Self::Android=>Self::ANDROID,
            Self::WebAssembly=>Self::WEBASSEMBLY,
            Self::CheckMacos=>Self::CHECK_MACOS,
            Self::CheckWindows=>Self::CHECK_WINDOWS,
            Self::CheckLinux=>Self::CHECK_LINUX,
            Self::CheckAll=>Self::CHECK_ALL
        }
    }
    pub fn from_id(tgt:u64) -> Self {
        match tgt {
            Self::RELEASE => Self::Release,
            Self::DEBUG => Self::Debug,
            Self::RELEASE_STUDIO => Self::ReleaseStudio,
            Self::DEBUG_STUDIO => Self::DebugStudio,
            Self::PROFILER => Self::Profiler,
            Self::IOS_SIM => Self::IosSim,
            Self::IOS_DEVICE => Self::IosDevice,
            Self::TVOS_SIM => Self::TvosSim,
            Self::TVOS_DEVICE => Self::TvosDevice,
            Self::ANDROID => Self::Android,
            Self::WEBASSEMBLY => Self::WebAssembly,
            Self::CHECK_MACOS => Self::CheckMacos,
            Self::CHECK_WINDOWS => Self::CheckWindows,
            Self::CHECK_LINUX => Self::CheckLinux,
            Self::CHECK_ALL => Self::CheckAll,
            _ => panic!()
        }
    }
}


#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct BuildProcess{
    pub binary: String,
    pub target: BuildTarget
}

impl BuildProcess{
    pub fn as_id(&self)->LiveId{
        LiveId::from_str(&self.binary).bytes_append(&self.target.as_id().to_be_bytes())
    }
}


#[derive(Clone, Debug)]
pub struct BuildCmdWrap {
    pub cmd_id: BuildCmdId,
    pub cmd: BuildCmd
}

impl BuildCmdId{
    pub fn wrap_msg(&self, item:LogItem)->LogItemWrap{
        LogItemWrap{
            cmd_id: *self,
            item,
        }
    }
}

#[derive(Clone, Debug)]
pub enum BuildCmd {
    Stop,
    Run(BuildProcess, String),
    HostToStdin(String)
}

#[derive(Clone)]
pub struct LogItemWrap {
    pub cmd_id: BuildCmdId,
    pub item: LogItem
}


#[derive(Clone, Debug)]
pub struct LogItemLocation{
    pub level: LogLevel,
    pub file_name: String,
    pub start: Position,
    pub end: Position,
    pub message: String
}

#[derive(Clone, Debug)]
pub struct LogItemBare{
    pub level: LogLevel,
    pub line: String,
}

#[derive(Clone)]
pub enum LogItem {
    Bare(LogItemBare),
    Location(LogItemLocation),
    StdinToHost(String),
    AuxChanHostEndpointCreated(crate::makepad_platform::cx_stdin::aux_chan::HostEndpoint),
}
