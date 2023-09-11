use crate::{
    makepad_code_editor::text::Range,
    makepad_micro_serde::{SerBin, DeBin, DeBinErr},
};


#[derive(PartialEq, Clone, Copy, Debug, SerBin, DeBin)]
pub struct BuildCmdId(pub u64);

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
    CargoRun{what:String},
    HostToStdin(String)
}

#[derive(Clone, Debug)]
pub struct LogItemWrap {
    pub cmd_id: BuildCmdId,
    pub item: LogItem
}

#[derive(Clone, Copy, Debug, SerBin, DeBin)]
pub enum LogItemLevel{
    Warning,
    Error,
    Log,
    Wait,
    Panic,
}

#[derive(Clone, Debug)]
pub struct LogItemLocation{
    pub level: LogItemLevel,
    pub file_name: String,
    pub range: Range,
    pub msg: String
}

#[derive(Clone, Debug)]
pub struct LogItemBare{
    pub level: LogItemLevel,
    pub line: String,
}

#[derive(Clone, Debug)]
pub enum LogItem {
    Bare(LogItemBare),
    Location(LogItemLocation),
    StdinToHost(String),
}
