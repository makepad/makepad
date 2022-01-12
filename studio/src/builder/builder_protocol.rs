use {
    makepad_component::makepad_render::{
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
    }
};

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuilderCmd {
    pub uid: u64,
    pub kind: BuilderCmdKind
}

impl BuilderCmd{
    pub fn to_message(&self, kind:BuilderMsgKind)->BuilderMsg{
        BuilderMsg{
            uid: self.uid,
            kind,
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum BuilderCmdKind {
    Build
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct BuilderMsg {
    pub uid: u64,
    pub kind: BuilderMsgKind
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum BuilderMsgKind {
    Error
}
