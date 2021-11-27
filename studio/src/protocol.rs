use {
    crate::{delta::Delta, genid::GenId, text::Text},
    std::{ffi::OsString, path::PathBuf},
};
use makepad_microserde::{SerBin, DeBin};

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Request {
    GetFileTree(),
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
    GetFileTree(Result<FileTree, Error>),
    OpenFile(Result<(FileId, usize, Text), Error>),
    ApplyDelta(Result<FileId, Error>),
    CloseFile(Result<FileId, Error>),
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct FileTree {
    pub path: PathBuf,
    pub root: FileNode,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum FileNode {
    Directory { entries: Vec<DirectoryEntry> },
    File,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct DirectoryEntry {
    pub name: OsString,
    pub node: FileNode,
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
