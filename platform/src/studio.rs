use crate::makepad_micro_serde::*;
use crate::makepad_derive_live::*;
use crate::LiveId;
use crate::action::*;
use crate::log::LogLevel;
pub use crate::makepad_live_compiler::live_node::LiveDesignInfo;
// communication enums for studio

#[derive(SerBin, DeBin, Debug)]
pub struct EventSample{
    pub event_u32: u32,
    pub event_meta: u64,
    pub start: f64,
    pub end: f64,
}

#[derive(SerBin, DeBin, Debug)]
pub struct GPUSample{
    pub start: f64,
    pub end: f64,
}

#[derive(SerBin, DeBin, Debug)]
pub struct StudioLogItem{
    pub file_name:String,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub message: String,
    pub explanation: Option<String>,
    pub level: LogLevel
}

#[derive(SerBin, DeBin, Debug, Clone)]
pub struct JumpToFile{
    pub file_name: String,
    pub line: u32,
    pub column: u32    
}

#[derive(SerBin, DeBin, Debug, Clone)]
pub struct PatchFile{
    pub file_name: String,
    pub line: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub undo_group: u64,
    pub replace: String
}

#[derive(SerBin, DeBin, SerRon, DeRon, Debug, Clone)]
pub struct DesignerComponentPosition{
    pub id: LiveId,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64
}


#[derive(Default, SerBin, DeBin, SerRon, DeRon, Debug, Clone)]
pub struct DesignerZoomPan{
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
}

#[derive(SerBin, DeBin, Debug, Clone)]
pub struct EditFile{
    pub file_name: String,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub replace: String
}

#[derive(SerBin, DeBin, Debug)]
pub enum AppToStudio{
    LogItem(StudioLogItem),
    EventSample(EventSample),
    GPUSample(GPUSample),
    JumpToFile(JumpToFile),
    PatchFile(PatchFile),
    DesignerComponentMoved(DesignerComponentPosition),
    DesignerZoomPan(DesignerZoomPan),
    EditFile(EditFile),
    DesignerStarted,
    DesignerFileSelected{
        file_name:String,
    },
    FocusDesign
}

#[derive(SerBin, DeBin)]
pub struct AppToStudioVec(pub Vec<AppToStudio>);

#[derive(Debug, DefaultNone, SerBin, DeBin)]
pub enum StudioToApp{
    LiveChange{
        file_name: String,
        content: String
    },
    DesignerLoadState{
        zoom_pan: DesignerZoomPan,
        positions: Vec<DesignerComponentPosition>
    },
    DesignerSelectFile{
        file_name: String,
    },
    None,
}

#[derive(SerBin, DeBin)]
pub struct StudioToAppVec(pub Vec<StudioToApp>);
