use std::{ffi::OsString, io};

#[derive(Debug)]
pub enum Request {
    GetFileTree(),
}

#[derive(Debug)]
pub enum Response {
    GetFileTree(Result<FileNode, Error>),
}

#[derive(Debug)]
pub enum FileNode {
    File,
    Directory { children: Vec<(OsString, FileNode)> },
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
