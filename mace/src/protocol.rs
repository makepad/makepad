use {
    crate::{delta::Delta, text::Text},
    serde::{Deserialize, Serialize},
    std::{ffi::OsString, path::PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Request {
    GetFileTree(),
    OpenFile(PathBuf),
    ApplyDelta(PathBuf, usize, Delta),
    CloseFile(PathBuf),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ResponseOrNotification {
    Response(Response),
    Notification(Notification),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Response {
    GetFileTree(Result<FileNode, Error>),
    OpenFile(Result<(PathBuf, usize, Text), Error>),
    ApplyDelta(Result<PathBuf, Error>),
    CloseFile(Result<PathBuf, Error>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FileNode {
    Directory { entries: Vec<DirectoryEntry> },
    File,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DirectoryEntry {
    pub name: OsString,
    pub node: FileNode,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Notification {
    DeltaWasApplied(PathBuf, Delta),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Error {
    Unknown(String),
}
