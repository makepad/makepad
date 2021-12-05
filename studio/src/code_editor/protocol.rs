use {
    crate::{
        code_editor::{
            delta::Delta,
            text::Text
        }
    },
    makepad_widget::{GenId},
    std::{
        ffi::OsString,
        path::PathBuf
    },
};
use makepad_microserde::{SerBin, DeBin};

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Request {
    LoadFileTree(),
    OpenFile(PathBuf),
    ApplyDelta(FileId, usize, Delta),
    CloseFile(FileId),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum ResponseOrNotification {
    Response(Response),
    Notification(Notification),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Response {
    LoadFileTree(Result<FileTreeData, Error>),
    OpenFile(Result<(FileId, usize, Text), Error>),
    ApplyDelta(Result<FileId, Error>),
    CloseFile(Result<FileId, Error>),
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
    DeltaWasApplied(FileId, Delta),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Error {
    AlreadyAParticipant,
    NotAParticipant,
    Unknown(String),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct FileId(pub GenId);

impl AsRef<GenId> for FileId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}
