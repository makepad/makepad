use crate::makepad_micro_serde::*;
use crate::log::LogType;

// communication enums for studio
#[derive(SerBin, DeBin)]
pub enum AppToStudio{
    Log{
        file:String,
        line_start: u32,
        line_end: u32,
        column_start: u32,
        column_end: u32,
        message: String,
        ty: LogType
    }
}

#[derive(SerBin, DeBin)]
pub enum StudioToApp{
    LiveChange{
        file_name: String,
        content: String
    }
}
