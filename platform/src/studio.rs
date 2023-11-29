use crate::makepad_micro_serde::*;
use crate::log::LogLevel;

// communication enums for studio

#[derive(SerBin, DeBin, Debug)]
pub enum ProfileSample{
    Event{
        event_u32: u32,
        start: f64,
        end: f64,
    }
}

#[derive(SerBin, DeBin, Debug)]
pub enum AppToStudio{
    Log{
        file_name:String,
        line_start: u32,
        line_end: u32,
        column_start: u32,
        column_end: u32,
        message: String,
        level: LogLevel
    },
    ProfileSample(ProfileSample)
}

#[derive(SerBin, DeBin)]
pub struct AppToStudioVec(pub Vec<AppToStudio>);

#[derive(SerBin, DeBin)]
pub enum StudioToApp{
    LiveChange{
        file_name: String,
        content: String
    }
}

#[derive(SerBin, DeBin)]
pub struct StudioToAppVec(pub Vec<StudioToApp>);
