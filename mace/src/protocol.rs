use {
    crate::text::Text,
    std::{ffi::OsString, io, path::PathBuf},
};

#[derive(Clone, Debug)]
pub enum Request {
    GetFileTree(),
    OpenFile(PathBuf),
}

#[derive(Debug)]
pub enum Response {
    GetFileTree(Result<FileNode, Error>),
    OpenFile(Result<Text, Error>),
}

#[derive(Debug)]
pub enum FileNode {
    Directory { entries: Vec<DirectoryEntry> },
    File,
}

#[derive(Debug)]
pub struct DirectoryEntry {
    pub name: OsString,
    pub node: FileNode,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}
