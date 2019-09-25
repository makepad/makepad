use serde::{Serialize,Deserialize};
use std::net::SocketAddr;

#[derive(Clone, Debug, Serialize,Deserialize)]
pub enum HubMsg{
    ConnectionError(HubError),
    Ping,
    LoginBuildServer,
    LoginMakepad, 
}

#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub enum HubAddr{
    V4{octets:[u8;4],port:u16},
    V6{octets:[u8;16],port:u16},
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HubTarget{
    Client(HubAddr),
    AllClients,
    HubOnly
}

impl HubAddr{
    pub fn from_socket_addr(addr:SocketAddr)->HubAddr{
        match addr{
            SocketAddr::V4(v4)=>HubAddr::V4{octets:v4.ip().octets(), port:v4.port()},
            SocketAddr::V6(v6)=>HubAddr::V6{octets:v6.ip().octets(), port:v6.port()},
        }
    }
}

#[derive(Debug, Serialize,Deserialize)]
pub struct ClientToHubMsg{
    pub target:HubTarget,
    pub msg:HubMsg
}

#[derive(Clone, Debug, Serialize,Deserialize)]
pub struct HubToClientMsg{
    pub from:HubAddr,
    pub msg:HubMsg
}

#[derive(Clone, Debug, Serialize,Deserialize)]
pub struct HubError{
    pub msg:String
}

impl HubError{
    pub fn new(msg:&str)->HubError{HubError{msg:msg.to_string()}}
}