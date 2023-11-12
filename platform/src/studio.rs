use crate::makepad_micro_serde::*;
// communication enums for studio
#[derive(SerBin, DeBin)]
pub enum AppToStudio{
    Log{
        body:String,
        file: u32,
        line: u32
    }
}

#[derive(SerBin, DeBin)]
pub enum StudioToApp{
    LiveChange{
        file_name: String,
        content: String
    }
}
