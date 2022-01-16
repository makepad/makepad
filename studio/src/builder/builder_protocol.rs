use {
    crate::{
        makepad_live_tokenizer::{Range},
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
    }
};


#[derive(Clone, Copy, Debug, SerBin, DeBin)]
pub struct BuilderCmdId(pub u64);

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuilderCmdWrap {
    pub cmd_id: BuilderCmdId,
    pub cmd: BuilderCmd
}

impl BuilderCmdId{
    pub fn wrap_msg(&self, msg:BuilderMsg)->BuilderMsgWrap{
        BuilderMsgWrap{
            cmd_id: *self,
            msg,
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum BuilderCmd {
    CargoCheck
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuilderMsgWrap {
    pub cmd_id: BuilderCmdId,
    pub msg: BuilderMsg
}

#[derive(Clone, Copy, Debug, SerBin, DeBin)]
pub enum BuilderMsgLevel{
    Warning,
    Error,
    Log
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuilderMsgLocation{
    pub level: BuilderMsgLevel,
    pub file_name: String,
    pub range: Range,
    pub msg: String
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuilderMsgBare{
    pub level: BuilderMsgLevel,
    pub line: String,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum BuilderMsg {
    Bare(BuilderMsgBare),
    Location(BuilderMsgLocation)
}
