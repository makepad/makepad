use serde::{Serialize,Deserialize};
use std::net::UdpSocket;
use std::io::Write;

#[derive(Serialize,Deserialize)]
pub enum IPCOnlineResponse{
    BuildServer,
    MakepadVR,
    Makepad
}

#[derive(Serialize,Deserialize)]
pub enum IPCMessage{
    QueryOnline,
    OnlineResponse(IPCOnlineResponse),
}

pub struct MessageBus {
    pub socket: UdpSocket,
    pub pid: u32,
    pub message_id: u32,
    pub outgoing_messages: Vec<MessageSerializer>,
    pub incoming_messages: Vec<MessageSerializer>
}

#[derive(Default)]
pub struct MessageChunk{
    pub header:ChunkHeader,
    pub buf: Vec<u8>
}

#[derive(Default, Serialize,Deserialize)]
pub struct ChunkHeader{
    message_id:u32,
    chunks: u32,
    chunk_id: u32,
    pid: u32
}

const CHUNK_HEADER_SIZE:usize = 16;
const CHUNK_MAX_SIZE:usize = 1440; 

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

struct ClientConnection{
}

impl MessageBus {
    pub fn new_lan_broadcast(port:u32)->MessageBus {
    
        // start a server,
        // this starts a 
        // find a way to query local 
        let bind_addr = format!("10.0.1.4:{}", port);
        println!("BINDING TO {}", bind_addr);
        let socket = UdpSocket::bind(bind_addr.clone()).expect("Could not bind to port");
        socket.set_broadcast(true).expect("Could not set broadcast");
        socket.connect(bind_addr).expect("Could not connect");
        MessageBus {
            socket: socket,
            pid: (4<<24) | std::process::id(),
            message_id:1,
            incoming_messages:Vec::new(),
            outgoing_messages:Vec::new(),
        }
    }
    
    
    
    pub fn new_localhost(bind_port:u32, conn_port:u32)->MessageBus{
        let bind_addr = format!("127.0.0.1:{}", bind_port);
        let conn_addr = format!("127.0.0.1:{}", conn_port);
        let socket = UdpSocket::bind(bind_addr.clone()).expect("Could not bind to port");
        socket.connect(conn_addr).expect("Could not connect");
        MessageBus{
            socket:socket,
            pid: std::process::id(),
            message_id:1,
            incoming_messages:Vec::new(),
            outgoing_messages:Vec::new(),
        }
    }
    
    pub fn recv<F>(&mut  self, _event_handler: F)
    where F: FnMut(&mut MessageBus, &mut IPCMessage) {
        // ok we have a packet gathering here
        loop{

            let mut buf = Vec::<u8>::new();
            buf.resize(CHUNK_MAX_SIZE, 0);

            let result = self.socket.recv_from(&mut buf);
            if let Err(e) = result{
                println!("recv from has error {:?}", e);
                continue;
            }
            let (amt, src) = result.unwrap();
            
            // pull out the header
            //let mut chunk_header_ser = ChunkHeaderSerializer{buf:&mut chunk.buf};
            //bincode::serialize_into(&mut chunk_header_ser, &chunk.header).expect("Could not write chunk header");

            // we compare the message id to the previous message id we got
            // and compile a list of all missing messages or chunk ids from that source
            
            
            println!("GOT PACKET! {} {:?}", amt, src);
        }
    }
    
    pub fn send(&mut self, message:IPCMessage) {
        // lets build a udp packet with this message
        let mut message_ser = MessageSerializer::default();
        bincode::serialize_into(&mut message_ser, &message).expect("Could not write message");

        let chunks_len = message_ser.chunks.len();
        // ok now send the packets
        for (chunk_id, chunk) in message_ser.chunks.iter_mut().enumerate(){
            // lets put the header in the chunk
            chunk.header.message_id = self.message_id;
            chunk.header.pid = self.pid;
            chunk.header.chunks = chunks_len as u32;
            chunk.header.chunk_id = chunk_id as u32;

            let mut chunk_header_ser = ChunkHeaderSerializer{buf:&mut chunk.buf};
            bincode::serialize_into(&mut chunk_header_ser, &chunk.header).expect("Could not write chunk header");

            if let Err(e) = self.socket.send(&chunk.buf){
                println!("Socket send error! {:?}", e);
            }
        }
        self.outgoing_messages.push(message_ser);
        self.message_id += 1;
    }
}