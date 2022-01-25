use {
    crate::{
        makepad_live_tokenizer::{
            delta::Delta,
            text::Text
        },
        makepad_micro_serde::{SerBin, DeBin, DeBinErr},
        makepad_platform::*,
    },
    std::{
        ffi::OsString,
        path::PathBuf
    },
};

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabRequest {
    LoadFileTree{with_data: bool},
    OpenFile(PathBuf),
    ApplyDelta(TextFileId, usize, Delta),
    CloseFile(TextFileId),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabClientAction {
    Response(CollabResponse),
    Notification(CollabNotification),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabResponse {
    LoadFileTree(Result<FileTreeData, CollabError>),
    OpenFile(Result<(TextFileId, usize, Text), CollabError>),
    ApplyDelta(Result<TextFileId, CollabError>),
    CloseFile(Result<TextFileId, CollabError>),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct FileTreeData {
    pub path: PathBuf,
    pub root: FileNodeData,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileNodeData {
    Directory {entries: Vec<DirectoryEntry>},
    File{data:Option<Vec<u8>>},
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct DirectoryEntry {
    pub name: OsString,
    pub node: FileNodeData,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabNotification {
    DeltaWasApplied(TextFileId, Delta),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum CollabError {
    AlreadyAParticipant,
    NotAParticipant,
    Unknown(String),
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct TextFileId(pub LiveId);

impl SerBin for TextFileId {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.0.0.ser_bin(s);
    }
}

impl DeBin for TextFileId {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        Ok(TextFileId(LiveId(DeBin::de_bin(o, d)?)))
    }
}
