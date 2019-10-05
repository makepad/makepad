use serde::{Serialize, Deserialize};
use std::net::SocketAddr;
use std::cmp::Ordering;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HubMsg {
    ConnectWorkspace(String),
    ConnectClone(String),
    ConnectUI,
    
    DisconnectWorkspace(String),
    DisconnectClone(String),
    DisconnectUI,
    DisconnectUnknown,
    
    ConnectionError(HubError),
    
    // make client stuff
    CargoExec {
        uid: HubUid,
        package: String,
        target: HubCargoTarget
    },
    
    CargoMsg {
        uid: HubUid,
        msg: HubCargoMsg
    },
    
    CargoArtifact {
        uid: HubUid,
        package_id: String,
        fresh:bool
    },

    CargoDone {
        uid: HubUid
    },
    
    CargoClear{
        uid:HubUid,
    },

    CargoPackagesRequest {
        uid: HubUid
    },
    
    CargoPackagesResponse {
        uid: HubUid,
        packages: Vec<HubCargoPackage>
    },
    
    WorkspaceFileTreeRequest {
        uid: HubUid
    },
    
    WorkspaceFileTreeResponse {
        uid: HubUid,
        tree:WorkspaceFileTreeNode
    },
    
    ListWorkspacesRequest{
        uid: HubUid,
    },
    
    ListWorkspacesResponse{
        uid: HubUid,
        workspaces: Vec<String>
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

#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceFileTreeNode {
    File {name: String},
    Folder {name: String, folder: Vec<WorkspaceFileTreeNode>}
}

impl Ord for WorkspaceFileTreeNode {
    fn cmp(&self, other:&WorkspaceFileTreeNode)->Ordering{
        match self{
            WorkspaceFileTreeNode::File{name:lhs, ..} =>{
                match other{
                    WorkspaceFileTreeNode::File{name:rhs, ..} =>{
                        lhs.cmp(rhs)
                    },
                    WorkspaceFileTreeNode::Folder{name:_rhs, ..} =>{
                        Ordering::Greater
                    },
                }
            },
            WorkspaceFileTreeNode::Folder{name:lhs, ..} =>{
                match other{
                    WorkspaceFileTreeNode::File{name:_rhs, ..} =>{
                        Ordering::Less
                    },
                    WorkspaceFileTreeNode::Folder{name:rhs, ..} =>{
                        lhs.cmp(rhs)
                    },
                }
            },
        }
    }
}

impl PartialOrd for WorkspaceFileTreeNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct  HubCargoPackage {
    pub package_name: String,
    pub targets: Vec<HubCargoTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HubCargoTarget {
    Check,
    Release,
    IPC,
    VR,
    Custom(String)
}

impl HubCargoPackage{
    pub fn new(package_name:&str, targets:Vec<HubCargoTarget>)->HubCargoPackage{
        HubCargoPackage{
            package_name: package_name.to_string(),
            targets: targets
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HubCargoMsgLevel {
    Warning,
    Error,
    Log
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubCargoMsg {
    pub package_id: String,
    pub path: String,
    pub row: usize,
    pub col: usize,
    pub tail: usize,
    pub head: usize,
    pub body: String,
    pub more_lines: Vec<String>,
    pub level: HubCargoMsgLevel
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubCargoArtifact {
    pub package_id: String,
    pub fresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubCargoCheck {
    pub target: String,
    pub args: String,
}

#[derive(PartialEq, Copy, Debug, Clone, Serialize, Deserialize)]
pub enum HubAddr {
    V4 {octets: [u8; 4], port: u16},
    V6 {octets: [u8; 16], port: u16},
}

impl HubAddr{
    pub fn port(&self)->u16{
        match self{
            HubAddr::V4{port,..}=>*port,
            HubAddr::V6{port,..}=>*port
        }
    }
}

impl HubAddr {
    pub fn zero()->HubAddr{
        HubAddr::V4{octets:[0,0,0,0], port:0}
    }
    
    pub fn from_socket_addr(addr: SocketAddr) -> HubAddr {
        match addr {
            SocketAddr::V4(v4) => HubAddr::V4 {octets: v4.ip().octets(), port: v4.port()},
            SocketAddr::V6(v6) => HubAddr::V6 {octets: v6.ip().octets(), port: v6.port()},
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HubMsgTo {
    Client(HubAddr),
    Workspace(String),
    UI,
    All,
    Hub
}

#[derive(PartialEq, Copy, Debug, Clone, Serialize, Deserialize)]
pub struct HubUid {
    pub addr: HubAddr,
    pub id: u64
}

impl HubUid {
    pub fn zero()->HubUid{
        HubUid{addr:HubAddr::zero(), id:0}
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientToHubMsg {
    pub to: HubMsgTo,
    pub msg: HubMsg
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubToClientMsg {
    pub from: HubAddr,
    pub msg: HubMsg
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubError {
    pub msg: String
}

impl HubError {
    pub fn new(msg: &str) -> HubError {HubError {msg: msg.to_string()}}
}

#[derive(Clone)]
pub enum HubLog{
    All,
    None
}

impl HubLog{
    pub fn log<T>(&self, prefix:&str, htc_msg:&T)
    where T:std::fmt::Debug
    {
        match self{
            HubLog::All=>{
                let mut msg = format!("{:?}", htc_msg);
                if msg.len()>200{
                    msg.truncate(200);
                    msg.push_str("...")
                }
                println!("{}{}", prefix, msg);
            },
            _=>()
        }
    }
}