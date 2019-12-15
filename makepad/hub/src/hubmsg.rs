use makepad_tinyserde::*;
use std::net::SocketAddr;
use std::cmp::Ordering;
use std::collections::HashMap;
use crate::httpserver::*;
use crate::hubclient::*;

#[derive(Clone, Debug, SerBin, DeBin)]
pub enum HubMsg {
    ConnectBuilder(String),
    ConnectClone(String),
    ConnectUI,
    
    DisconnectBuilder(String),
    DisconnectClone(String),
    DisconnectUI,
    DisconnectUnknown,
    
    ConnectionError(HubError),
    
    BuilderConfig { 
        uid: HubUid,
        config: HubBuilderConfig
    },
    
    // make client stuff
    Build {
        uid: HubUid,
        workspace: String,
        package: String,
        config: String
    },
    
    BuildFailure {
        uid: HubUid,
    },
    
    BuildSuccess {
        uid: HubUid,
    },
    
    BuildKill {
        uid: HubUid
    },
    
    CargoBegin {
        uid: HubUid,
    },
    
    LogItem {
        uid: HubUid,
        item: HubLogItem
    },
    
    CargoArtifact {
        uid: HubUid,
        package_id: String,
        fresh: bool
    },
    
    CargoEnd {
        uid: HubUid,
        build_result: BuildResult
    },
    
    ListPackagesRequest {
        uid: HubUid
    },
    
    ListPackagesResponse {
        uid: HubUid,
        packages: Vec<HubPackage>
    },
    
    ProgramKill {
        uid: HubUid
    },
    
    ProgramRun {
        uid: HubUid,
        path: String,
        args: Vec<String>
    },
    
    ProgramBegin {
        uid: HubUid
    },
    
    ProgramEnd {
        uid: HubUid
    },
    
    BuilderFileTreeRequest {
        uid: HubUid,
        create_digest: bool
    },
    
    BuilderFileTreeResponse {
        uid: HubUid,
        tree: BuilderFileTreeNode
    },
    
    ListBuildersRequest {
        uid: HubUid,
    },
    
    ListBuildersResponse {
        uid: HubUid,
        builders: Vec<String>
    },
    
    FileReadRequest {
        uid: HubUid,
        path: String
    },
    
    FileReadResponse {
        uid: HubUid,
        path: String,
        data: Option<Vec<u8>>
    },
    
    FileWriteRequest {
        uid: HubUid,
        path: String,
        data: Vec<u8>
    },
    
    FileWriteResponse {
        uid: HubUid,
        path: String,
        done: bool
    },
}

impl HubMsg{
    pub fn is_blocking(&self)->bool{
        match self{
            HubMsg::BuilderConfig{..}=>true,
            HubMsg::FileWriteRequest{..}=>true,
            _=>false
        }
    }
}

#[derive(Eq, PartialEq, Debug, Clone, SerBin, DeBin, SerRon, DeRon)]
pub enum BuilderFileTreeNode {
    File {name: String, digest:Option<Box<Digest>>},
    Folder {name: String, digest:Option<Box<Digest>>, folder: Vec<BuilderFileTreeNode>}
}

impl Ord for BuilderFileTreeNode {
    fn cmp(&self, other: &BuilderFileTreeNode) -> Ordering {
        match self {
            BuilderFileTreeNode::File {name: lhs, ..} => {
                match other {
                    BuilderFileTreeNode::File {name: rhs, ..} => {
                        lhs.cmp(rhs)
                    },
                    BuilderFileTreeNode::Folder {name: _rhs, ..} => {
                        Ordering::Greater
                    },
                }
            },
            BuilderFileTreeNode::Folder {name: lhs, ..} => {
                match other {
                    BuilderFileTreeNode::File {name: _rhs, ..} => {
                        Ordering::Less
                    },
                    BuilderFileTreeNode::Folder {name: rhs, ..} => {
                        lhs.cmp(rhs)
                    },
                }
            },
        }
    }
}

impl PartialOrd for BuilderFileTreeNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


#[derive(Debug, Clone, SerBin, DeBin)]
pub enum BuildResult {
    Executable {path: String},
    Wasm {path: String},
    Library {path: String},
    NoOutput,
    Error,
}

#[derive(Debug, Clone, SerBin, DeBin)]
pub struct HubPackage {
    pub project: String,
    pub package_name: String,
    pub configs: Vec<String>,
}

impl HubPackage {
    pub fn new(project: &str, package_name: &str, targets: &[&str]) -> HubPackage {
        HubPackage {
            project: project.to_string(),
            package_name: package_name.to_string(),
            configs: targets.iter().map( | v | v.to_string()).collect()
        }
    }
}


#[derive(Debug, Clone, SerBin, DeBin, PartialEq, SerRon, DeRon)]
pub struct HubBuilderConfig {
    pub http_server: HttpServerConfig,
    pub workspaces: HashMap<String, String>,
}


#[derive(Debug, Clone, PartialEq, SerBin, DeBin)]
pub struct LocMessage {
    pub path: String,
    pub row: usize,
    pub col: usize,
    pub body: String,
    pub range: Option<(usize, usize)>,
    pub rendered: Option<String>,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, SerBin, DeBin)]
pub enum HubLogItem {
    LocPanic(LocMessage),
    LocError(LocMessage),
    LocWarning(LocMessage),
    LocMessage(LocMessage),
    Error(String),
    Warning(String),
    Message(String)
}

impl HubLogItem {
    pub fn get_loc_message(&self) -> Option<&LocMessage> {
        match self {
            HubLogItem::LocPanic(msg) => Some(msg),
            HubLogItem::LocError(msg) => Some(msg),
            HubLogItem::LocWarning(msg) => Some(msg),
            HubLogItem::LocMessage(msg) => Some(msg),
            HubLogItem::Error(_) => None,
            HubLogItem::Warning(_) => None,
            HubLogItem::Message(_) => None
        }
    }
    pub fn get_body(&self) -> &String {
        match self {
            HubLogItem::LocPanic(msg) => &msg.body,
            HubLogItem::LocError(msg) => &msg.body,
            HubLogItem::LocWarning(msg) => &msg.body,
            HubLogItem::LocMessage(msg) => &msg.body,
            HubLogItem::Error(body) => body,
            HubLogItem::Warning(body) => body,
            HubLogItem::Message(body) => body
        }
    }
}

#[derive(Debug, Clone, SerBin, DeBin)]
pub struct HubCargoArtifact {
    pub package_id: String,
    pub fresh: bool,
}

#[derive(Debug, Clone, SerBin, DeBin)]
pub struct HubCargoCheck {
    pub target: String,
    pub args: String,
}

#[derive(PartialEq, Copy, Debug, Clone, SerBin, DeBin)]
pub enum HubAddr {
    None,
    Local {uid: u64},
    V4 {octets: [u8; 4], port: u16},
    V6 {octets: [u8; 16], port: u16},
}

impl HubAddr {
    pub fn port(&self) -> u16 {
        match self {
            HubAddr::V4 {port, ..} => *port,
            HubAddr::V6 {port, ..} => *port,
            HubAddr::Local{..}=> 0,
            HubAddr::None{..}=> 0,
        }
    }
}

impl HubAddr {
    //pub fn zero() -> HubAddr {
    //    HubAddr::V4 {octets: [0, 0, 0, 0], port: 0}
   // }
    
    pub fn from_socket_addr(addr: SocketAddr) -> HubAddr {
        match addr {
            SocketAddr::V4(v4) => HubAddr::V4 {octets: v4.ip().octets(), port: v4.port()},
            SocketAddr::V6(v6) => HubAddr::V6 {octets: v6.ip().octets(), port: v6.port()},
        }
    }
}

#[derive(Debug, Clone, SerBin, DeBin)]
pub enum HubMsgTo {
    Client(HubAddr),
    Builder(String),
    UI,
    All,
    Hub
}

#[derive(PartialEq, Copy, Debug, Clone, SerBin, DeBin)]
pub struct HubUid {
    pub addr: HubAddr,
    pub id: u64
}

impl HubUid {
    pub fn zero() -> HubUid {
        HubUid {addr: HubAddr::None, id: 0}
    }
}

#[derive(Debug, Clone, SerBin, DeBin)]
pub struct ToHubMsg {
    pub to: HubMsgTo,
    pub msg: HubMsg
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct FromHubMsg {
    pub from: HubAddr,
    pub msg: HubMsg
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct HubError {
    pub msg: String
}

impl HubError {
    pub fn new(msg: &str) -> HubError {HubError {msg: msg.to_string()}}
}

#[derive(Clone)]
pub enum HubLog {
    All,
    None
}

impl HubLog {
    pub fn msg<T>(&self, prefix: &str, htc_msg: &T)
    where T: std::fmt::Debug
    {
        match self {
            HubLog::All => {
                let mut msg = format!("{:?}", htc_msg);
                if msg.len()>200 {
                    msg.truncate(200);
                    msg.push_str("...")
                }
                println!("{} {}", prefix, msg);
            },
            _ => ()
        }
    }
    pub fn log(&self, msg: &str)
    {
        match self {
            HubLog::All => {
                println!("{}", msg);
            },
            _ => ()
        }
    }
}