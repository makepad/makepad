use crate::{
    makepad_code_editor::Range,
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
    pub fn wrap_msg(&self, msg:BuildMsg)->BuildMsgWrap{
        BuildMsgWrap{
            cmd_id: *self,
            msg,
        }
    }
}

#[derive(Clone, Debug)]
pub enum BuildCmd {
    CargoRun{what:String},
    HostToStdin(String)
}

#[derive(Clone, Debug)]
pub struct BuildMsgWrap {
    pub cmd_id: BuildCmdId,
    pub msg: BuildMsg
}

#[derive(Clone, Copy, Debug, SerBin, DeBin)]
pub enum BuildMsgLevel{
    Warning,
    Error,
    Log,
    Wait,
    Panic,
}

#[derive(Clone, Debug)]
pub struct BuildMsgLocation{
    pub level: BuildMsgLevel,
    pub file_name: String,
    pub range: Range,
    pub msg: String
}

#[derive(Clone, Debug)]
pub struct BuildMsgBare{
    pub level: BuildMsgLevel,
    pub line: String,
}

#[derive(Clone, Debug)]
pub enum BuildMsg {
    Bare(BuildMsgBare),
    Location(BuildMsgLocation),
    StdinToHost(String),
}
