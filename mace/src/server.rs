use {crate::protocol::{Request, Response}, std::{path::PathBuf, sync::Arc}};

pub struct Server {
    shared: Arc<Shared>,
}

impl Server {
    pub fn new<P: Into<PathBuf>>(path: P) -> Server {
        Server {
            shared: Arc::new(Shared {
                path: path.into()
            })
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
        unimplemented!()
    }
}

struct Shared {
    path: PathBuf,
}