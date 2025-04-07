use crate::{
    makepad_code_editor::text::Position, makepad_live_id::LiveId, makepad_platform::log::LogLevel,
    makepad_micro_serde::*,
};

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, SerRon, DeRon)]
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
    Quest,
    Harmony,
    WebAssembly,
    CheckMacos,
    CheckWindows,
    CheckLinux,
    CheckAll,
}

impl BuildTarget {
    pub fn runs_in_studio(&self) -> bool {
        match self {
            Self::ReleaseStudio => true,
            Self::DebugStudio => true,
            _ => false,
        }
    }

    pub const RELEASE_STUDIO: u64 = 0;
    pub const DEBUG_STUDIO: u64 = 1;
    pub const RELEASE: u64 = 2;
    pub const DEBUG: u64 = 3;
    pub const PROFILER: u64 = 4;
    pub const IOS_SIM: u64 = 5;
    pub const IOS_DEVICE: u64 = 6;
    pub const TVOS_SIM: u64 = 7;
    pub const TVOS_DEVICE: u64 = 8;
    pub const ANDROID: u64 = 9;
    pub const QUEST: u64 = 10;
    pub const HARMONY: u64 = 11;
    pub const WEBASSEMBLY: u64 = 12;
    pub const CHECK_MACOS: u64 = 13;
    pub const CHECK_WINDOWS: u64 = 14;
    pub const CHECK_LINUX: u64 = 15;
    pub const CHECK_ALL: u64 = 16;
    pub fn len() -> usize {
        Self::CHECK_ALL as usize + 1
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::ReleaseStudio => "Studio Release",
            Self::DebugStudio => "Studio Debug",
            Self::Release => "Release",
            Self::Debug => "Debug",
            Self::Profiler => "Profiler",
            Self::IosSim => "iOS Simulator",
            Self::IosDevice => "iOS Device",
            Self::TvosSim => "tvOS Simulator",
            Self::TvosDevice => "tvOS Device",
            Self::Android => "Android",
            Self::Quest => "Quest",
            Self::Harmony => "Harmony",
            Self::WebAssembly => "WebAssembly",
            Self::CheckMacos => "Check macOS",
            Self::CheckWindows => "Check Windows",
            Self::CheckLinux => "Check Linux",
            Self::CheckAll => "Check All",
        }
    }
    pub fn as_id(&self) -> usize {
        (match self {
            Self::ReleaseStudio => Self::RELEASE_STUDIO,
            Self::DebugStudio => Self::DEBUG_STUDIO,
            Self::Release => Self::RELEASE,
            Self::Debug => Self::DEBUG,
            Self::Profiler => Self::PROFILER,
            Self::IosSim { .. } => Self::IOS_SIM,
            Self::IosDevice { .. } => Self::IOS_DEVICE,
            Self::TvosSim { .. } => Self::TVOS_SIM,
            Self::TvosDevice { .. } => Self::TVOS_DEVICE,
            Self::Android => Self::ANDROID,
            Self::Quest => Self::QUEST,
            Self::Harmony => Self::HARMONY,
            Self::WebAssembly => Self::WEBASSEMBLY,
            Self::CheckMacos => Self::CHECK_MACOS,
            Self::CheckWindows => Self::CHECK_WINDOWS,
            Self::CheckLinux => Self::CHECK_LINUX,
            Self::CheckAll => Self::CHECK_ALL,
        }) as usize
    }
    pub fn from_id(tgt: usize) -> Self {
        match tgt as u64{
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
            Self::HARMONY => Self::Harmony,
            Self::QUEST => Self::Quest,
            Self::WEBASSEMBLY => Self::WebAssembly,
            Self::CHECK_MACOS => Self::CheckMacos,
            Self::CHECK_WINDOWS => Self::CheckWindows,
            Self::CHECK_LINUX => Self::CheckLinux,
            Self::CHECK_ALL => Self::CheckAll,
            _ => panic!(),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, SerRon, DeRon)]
pub struct BuildProcess {
    pub binary: String,
    pub target: BuildTarget,
}

impl BuildProcess {
    pub fn as_id(&self) -> LiveId {
        LiveId::from_str(&self.binary).bytes_append(&self.target.as_id().to_be_bytes())
    }
}

#[derive(Clone, Debug)]
pub struct BuildCmdWrap {
    pub cmd_id: LiveId,
    pub cmd: BuildCmd,
}

#[derive(Clone, Debug)]
pub enum BuildCmd {
    Stop,
    Run(BuildProcess, String),
    HostToStdin(String),
}

#[derive(Clone)]
pub struct BuildClientMessageWrap {
    pub cmd_id: LiveId,
    pub message: BuildClientMessage,
}

#[derive(Clone, Debug)]
pub struct LogItemLocation {
    pub level: LogLevel,
    pub file_name: String,
    pub start: Position,
    pub end: Position,
    pub message: String,
    pub explanation: Option<String>
}

#[derive(Clone, Debug)]
pub struct LogItemBare {
    pub level: LogLevel,
    pub line: String,
}

#[derive(Clone)]
pub enum LogItem {
    Bare(LogItemBare),
    Location(LogItemLocation),
    StdinToHost(String),
}

#[derive(Clone)]
pub enum BuildClientMessage {
    LogItem(LogItem),
    AuxChanHostEndpointCreated(crate::makepad_platform::cx_stdin::aux_chan::HostEndpoint),
}
