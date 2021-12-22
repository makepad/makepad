use {
    makepad_render::makepad_live_tokenizer::{
        delta::Delta,
        text::Text
    },
    makepad_render::{makepad_micro_serde::{SerBin, DeBin, DeBinErr}, *},
    std::{
        ffi::OsString,
        path::PathBuf
    },
};

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Request {
    LoadFileTree(),
    OpenFile(PathBuf),
    ApplyDelta(TextFileId, usize, Delta),
    CloseFile(TextFileId),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum ResponseOrNotification {
    Response(Response),
    Notification(Notification),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Response {
    LoadFileTree(Result<FileTreeData, Error>),
    OpenFile(Result<(TextFileId, usize, Text), Error>),
    ApplyDelta(Result<TextFileId, Error>),
    CloseFile(Result<TextFileId, Error>),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct FileTreeData {
    pub path: PathBuf,
    pub root: FileNodeData,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileNodeData {
    Directory {entries: Vec<DirectoryEntry>},
    File,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct DirectoryEntry {
    pub name: OsString,
    pub node: FileNodeData,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Notification {
    DeltaWasApplied(TextFileId, Delta),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Error {
    AlreadyAParticipant,
    NotAParticipant,
    Unknown(String),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextFileId(pub GenId);

impl SerBin for TextFileId{
    fn ser_bin(&self, s: &mut Vec<u8>){
        self.0.index.ser_bin(s);
        self.0.generation.ser_bin(s);
    }
}

impl DeBin for TextFileId{
    fn de_bin(o:&mut usize, d:&[u8]) -> Result<Self, DeBinErr>{
        Ok(Self(GenId{index:DeBin::de_bin(o, d)?, generation:DeBin::de_bin(o, d)?}))
    }
}

impl AsRef<GenId> for TextFileId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}
