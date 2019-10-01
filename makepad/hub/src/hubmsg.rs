use serde::{Serialize, Deserialize};
use std::net::SocketAddr;

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
        target: CargoTarget
    },
    
    CargoMsg {
        uid: HubUid,
        msg: CargoMsg
    },
    
    CargoDone {
        uid: HubUid
    },
    
    WorkspaceFileTreeRequest {
        uid: HubUid
    },
    
    WorkspaceFileTreeResponse {
        uid: HubUid,
    },
    
    ListWorkspacesRequest{
        uid: HubUid,
    },
    
    ListWorkspacesResponse{
        uid: HubUid,
        workspaces: Vec<String>
    },
    
    ReadFileRequest {
        uid: HubUid,
        path: String
    },
    
    ReadFileResponse {
        uid: HubUid,
        path: String,
        data: Option<Vec<u8>>
    },
    
    WriteFileRequest {
        uid: HubUid,
        path: String,
        data: Vec<u8>
    },
    
    WriteFileResponse {
        uid: HubUid,
        path: String,
        done: bool
    },
    
    CargoPackagesRequest {
        uid: HubUid
    },
    
    CargoPackagesResponse {
        uid: HubUid,
        packages: Vec<CargoPackage>
    },

}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct  CargoPackage {
    pub package_name: String,
    pub targets: Vec<CargoTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CargoTarget {
    Check,
    Release,
    IPC,
    VR,
    Custom(String)
}

impl CargoPackage{
    pub fn new(package_name:&str, targets:Vec<CargoTarget>)->CargoPackage{
        CargoPackage{
            package_name: package_name.to_string(),
            targets: targets
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CargoMsg {
    Warning {msg: String},
    Error {msg: String}
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

#[derive(Debug, Serialize, Deserialize)]
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