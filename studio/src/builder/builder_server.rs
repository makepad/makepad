#![allow(dead_code)]
use {
    crate::{
        builder::{
            builder_proto::{
                BuilderCmd,
                BuilderCmdKind,
                BuilderMsg,
                BuilderMsgKind
            },
        },
    },
    std::{
        fmt,
        path::{PathBuf},
        sync::{Arc, RwLock},
    },
};

pub struct BuilderServer {
    next_connection_id: usize,
    shared: Arc<RwLock<Shared >>,
}

impl BuilderServer {
    pub fn new<P: Into<PathBuf >> (path: P) -> BuilderServer {
        BuilderServer {
            next_connection_id: 0,
            shared: Arc::new(RwLock::new(Shared {
                path: path.into(),
            })),
        }
    }
    
    pub fn connect(&mut self, msg_sender: Box<dyn MsgSender>) -> BuilderConnection {
        let connection_id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        BuilderConnection {
            connection_id,
            shared: self.shared.clone(),
            msg_sender,
        }
    }
}

pub struct BuilderConnection {
    connection_id: ConnectionId,
    shared: Arc<RwLock<Shared >>,
    msg_sender: Box<dyn MsgSender>,
}

impl BuilderConnection {
    pub fn handle_cmd(&self, cmd: BuilderCmd) {
        match cmd.kind {
            BuilderCmdKind::Build => {
                self.msg_sender.send_message(
                    cmd.to_message(BuilderMsgKind::Error)
                );
            }
        }
    }
}

pub trait MsgSender: Send {
    fn box_clone(&self) -> Box<dyn MsgSender>;
    fn send_message(&self, msg: BuilderMsg);
}

impl<F: Clone + Fn(BuilderMsg) + Send + 'static> MsgSender for F {
    fn box_clone(&self) -> Box<dyn MsgSender> {
        Box::new(self.clone())
    }
    
    fn send_message(&self, msg: BuilderMsg) {
        self (msg)
    }
}

impl Clone for Box<dyn MsgSender> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}

impl fmt::Debug for dyn MsgSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MsgSender")
    }
}

#[derive(Debug)]
struct Shared {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionId(usize);
