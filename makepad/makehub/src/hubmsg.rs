use serde::{Serialize,Deserialize};
use std::net::SocketAddr;

#[derive(Clone, Debug, Serialize,Deserialize)]
pub enum HubMsg{
    Ping,
    
    ConnectBuild,
    ConnectUI,
    ConnectionError(HubError),

    // make client stuff
    CargoCheck{
        uid:HubUid,
        target:String
    },

    CargoMsg{
        uid:HubUid,
        msg:CargoMsg
    },
    
    CargoDone{
        uid:HubUid
    },

    CargoHasTargets{uid:HubUid, targets:Vec<String>},
    GetCargoTargets{uid:HubUid},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CargoMsg{
    Warning{msg:String},
    Error{msg:String}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubCargoCheck{
    pub target:String,
    pub args:String,
}

#[derive(PartialEq, Copy, Debug, Clone, Serialize, Deserialize)]
pub enum HubAddr{
    V4{octets:[u8;4],port:u16},
    V6{octets:[u8;16],port:u16},
}

impl HubAddr{
    pub fn from_socket_addr(addr:SocketAddr)->HubAddr{
        match addr{
            SocketAddr::V4(v4)=>HubAddr::V4{octets:v4.ip().octets(), port:v4.port()},
            SocketAddr::V6(v6)=>HubAddr::V6{octets:v6.ip().octets(), port:v6.port()},
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HubMsgTo{
    Client(HubAddr),
    Build,
    UI,
    All,
    Hub
}

#[derive(PartialEq, Copy, Debug, Clone, Serialize, Deserialize)]
pub struct HubUid{
    pub addr:HubAddr,
    pub id:u64
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClientToHubMsg{
    pub to:HubMsgTo,
    pub msg:HubMsg
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubToClientMsg{
    pub from:HubAddr,
    pub msg:HubMsg
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HubError{
    pub msg:String
}

impl HubError{
    pub fn new(msg:&str)->HubError{HubError{msg:msg.to_string()}}
}