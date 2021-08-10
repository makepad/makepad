use {
    crate::text::Text,
    makepad_microserde::*,
    std::{ffi::OsString, path::PathBuf},
};

#[derive(Clone, Debug, DeBin, SerBin)]
pub enum Request {
    GetFileTree(),
    OpenFile(PathBuf),
}

#[derive(Debug, DeBin, SerBin)]
pub enum ResponseOrNotification {
    Response(Response),
    Notification(Notification),
}

#[derive(Debug, DeBin, SerBin)]
pub enum Response {
    GetFileTree(Result<FileNode, Error>),
    OpenFile(Result<Text, Error>),
}

#[derive(Debug, DeBin, SerBin)]
pub enum FileNode {
    Directory { entries: Vec<DirectoryEntry> },
    File,
}

#[derive(Debug, DeBin, SerBin)]
pub struct DirectoryEntry {
    pub name: OsString,
    pub node: FileNode,
}

#[derive(Debug, DeBin, SerBin)]
pub struct Notification {}

#[derive(Debug, DeBin, SerBin)]
pub enum Error {
    Unknown(String),
}
