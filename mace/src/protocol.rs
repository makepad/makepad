use {
    crate::text::Text,
    serde::{Deserialize, Serialize},
    std::{ffi::OsString, path::PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Request {
    GetFileTree(),
    OpenFile(PathBuf),
}

#[derive(Debug, Deserialize, Serialize)]
pub enum ResponseOrNotification {
    Response(Response),
    Notification(Notification),
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Response {
    GetFileTree(Result<FileNode, Error>),
    OpenFile(Result<(usize, Text), Error>),
}

#[derive(Debug, Deserialize, Serialize)]
pub enum FileNode {
    Directory { entries: Vec<DirectoryEntry> },
    File,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DirectoryEntry {
    pub name: OsString,
    pub node: FileNode,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Notification {}

#[derive(Debug, Deserialize, Serialize)]
pub enum Error {
    Unknown(String),
}
