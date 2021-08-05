use {
    crate::{
        protocol::{DirectoryEntry, Error, FileNode, Request, Response},
        text::Text,
    },
    std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
    },
};

pub struct Server {
    shared: Arc<Shared>,
}

impl Server {
    pub fn new<P: Into<PathBuf>>(path: P) -> Server {
        Server {
            shared: Arc::new(Shared { path: path.into() }),
        }
    }

    pub fn connect(&self) -> Connection {
        Connection {
            shared: self.shared.clone(),
        }
    }
}

pub struct Connection {
    shared: Arc<Shared>,
}

impl Connection {
    pub fn handle_request(&self, request: Request) -> Response {
        match request {
            Request::GetFileTree() => Response::GetFileTree(self.get_file_tree()),
            Request::OpenFile(path) => Response::OpenFile(self.open_file(path)),
        }
    }

    pub fn get_file_tree(&self) -> Result<FileNode, Error> {
        fn get_directory_entries(path: &Path) -> Result<Vec<DirectoryEntry>, Error> {
            let mut entries = Vec::new();
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let entry_path = entry.path();
                entries.push(DirectoryEntry {
                    name: entry.file_name(),
                    node: if entry_path.is_dir() {
                        FileNode::Directory {
                            entries: get_directory_entries(&entry_path)?,
                        }
                    } else {
                        FileNode::File
                    },
                });
            }
            Ok(entries)
        }

        Ok(FileNode::Directory {
            entries: get_directory_entries(&self.shared.path)?,
        })
    }

    pub fn open_file(&self, path: PathBuf) -> Result<Text, Error> {
        let bytes = fs::read(path)?;
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| line.chars().collect::<Vec<_>>())
            .collect::<Vec<_>>()
            .into())
    }
}

struct Shared {
    path: PathBuf,
}
