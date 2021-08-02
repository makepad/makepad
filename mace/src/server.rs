use {
    crate::protocol::{Error, FileNode, Request, Response},
    std::{
        ffi::OsString,
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
        }
    }

    pub fn get_file_tree(&self) -> Result<FileNode, Error> {
        fn get_children(path: &Path) -> Result<Vec<(OsString, FileNode)>, Error> {
            let mut children = Vec::new();
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let entry_path = entry.path();
                children.push((
                    entry.file_name(),
                    if entry_path.is_dir() {
                        FileNode::Directory {
                            children: get_children(&entry_path)?,
                        }
                    } else {
                        FileNode::File
                    },
                ));
            }
            Ok(children)
        }

        Ok(FileNode::Directory {
            children: get_children(&self.shared.path)?,
        })
    }
}

struct Shared {
    path: PathBuf,
}
