use {
    crate::{
        makepad_editor_core::range::{Range},
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
    }
};


#[derive(PartialEq, Clone, Copy, Debug, SerBin, DeBin)]
pub struct BuildCmdId(pub u64);

#[derive(Clone, Debug, SerBin, DeBin)]
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

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum BuildCmd {
    CargoRun{what:String},
    HostToStdin(String)
}

#[derive(Clone, Debug, SerBin, DeBin)]
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

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuildMsgLocation{
    pub level: BuildMsgLevel,
    pub file_name: String,
    pub range: Range,
    pub msg: String
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuildMsgBare{
    pub level: BuildMsgLevel,
    pub line: String,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum BuildMsg {
    Bare(BuildMsgBare),
    Location(BuildMsgLocation),
    StdinToHost(String),
}
