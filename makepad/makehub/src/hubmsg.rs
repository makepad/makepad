use serde::{Serialize,Deserialize};
use std::net::SocketAddr;

#[derive(Debug, Serialize,Deserialize)]
pub enum HubMsg{
    LoginBuildServer,
    LoginMakepad, 
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HubAddr{
    V4{octets:[u8;4],port:u16},
    V6{octets:[u8;16],port:u16}
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
    pub to:Option<HubAddr>,
    pub msg:HubMsg
}

#[derive(Debug, Serialize,Deserialize)]
pub struct HubToClientMsg{
    pub from:HubAddr,
    pub msg:HubMsg
}


/*
#[derive(Default)]
pub struct MessageSerializer{
    pub completed_chunks: usize,
    pub chunks:Vec<MessageChunk>
} 

pub struct ChunkHeaderSerializer<'a>{
    pub buf:&'a mut Vec<u8>
}

impl<'a> Write for ChunkHeaderSerializer<'a>{
    fn write(&mut self, buf: &[u8]) -> Result<usize,std::io::Error>{
        for i in 0..buf.len(){
            self.buf[i] = buf[i];
        }
        Ok(buf.len())
    }
    
    fn flush(&mut self) -> Result<(), std::io::Error>{
        Ok(())
    }
}


impl Write for MessageSerializer{
    
    fn write(&mut self, buf: &[u8]) -> Result<usize,std::io::Error>{
        // ok we have to start writing a packet.
        let mut bytes_left = buf.len();
        loop{
            if let Some(message_chunk) = self.chunks.last_mut(){
                let chunk_fits = CHUNK_MAX_SIZE - message_chunk.buf.len();
                let add_bytes = bytes_left.min(chunk_fits);
                let start_byte = buf.len() - bytes_left;
                message_chunk.buf.extend_from_slice(&buf[start_byte..(start_byte+add_bytes)]);
                bytes_left -= add_bytes;
            }
            if bytes_left > 0{ // add packet
                let mut buf = Vec::new();
                buf.resize(CHUNK_HEADER_SIZE, 0);
                self.chunks.push(MessageChunk{
                    header:ChunkHeader::default(),
                    buf:buf
                });
            }
            else{
                break
            }
        }
        Ok(buf.len())
    }
    
    fn flush(&mut self) -> Result<(), std::io::Error>{
        Ok(())
    }
}
*/