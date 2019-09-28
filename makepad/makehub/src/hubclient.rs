use crate::hubmsg::*;
use std::net::{TcpStream, UdpSocket, SocketAddr, SocketAddrV4, SocketAddrV6, Shutdown};
use std::io::prelude::*;
use std::sync::{mpsc};
use std::thread;

trait ResultMsg<T>{
    fn expect_msg(self, msg:&str)->Result<T, HubError>;
}

impl<T> ResultMsg<T> for Result<T, std::io::Error> {
    fn expect_msg(self, msg:&str)->Result<T, HubError>{
        match self{
            Err(v)=>Err(HubError{msg:format!("{}: {}",msg.to_string(), v.to_string())}),
            Ok(v)=>Ok(v)
        }
    }
}

impl<T> ResultMsg<T> for Result<T, snap::Error> {
    fn expect_msg(self, msg:&str)->Result<T, HubError>{
        match self{
            Err(v)=>Err(HubError{msg:format!("{}: {}",msg.to_string(), v.to_string())}),
            Ok(v)=>Ok(v)
        }
    }
}

type HubResult<T> = Result<T, HubError>;

pub const HUB_ANNOUNCE_PORT:u16 = 46243;

pub fn read_exact_bytes_from_tcp_stream(tcp_stream: &mut TcpStream, bytes: &mut [u8])->HubResult<()>{
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &mut bytes[(bytes_total - bytes_left)..bytes_total];
        let bytes_read = tcp_stream.read(buf).expect_msg("read_exact_bytes_from_tcp_stream: read failed")?;
        bytes_left -= bytes_read;
    }
    Ok(())
}

pub fn read_block_from_tcp_stream(tcp_stream: &mut TcpStream, check_digest: &mut [u64; 26]) -> HubResult<Vec<u8>> {
    let mut digest = [0u64; 26];
    
    let digest_u8 = unsafe {std::mem::transmute::<&mut [u64; 26], &mut [u8; 26 * 8]>(&mut digest)};
    read_exact_bytes_from_tcp_stream(tcp_stream, digest_u8)?;
    
    let bytes_total = digest[25] as usize;
    if bytes_total > 250 * 1024 * 1024 {
        return Err(HubError::new("read_block_from_tcp_stream: bytes_total more than 250mb"))
    }
    
    let mut msg_buf = Vec::new();
    msg_buf.resize(bytes_total, 0);
    read_exact_bytes_from_tcp_stream(tcp_stream, &mut msg_buf)?;
    
    digest_buffer(check_digest, &msg_buf);
    check_digest[25] = bytes_total as u64;
    
    if check_digest != &mut digest {
        return Err(HubError::new("read_block_from_tcp_stream: block digest check failed"))
    }
    
    let mut dec = snap::Decoder::new();
    let decompressed = dec.decompress_vec(&msg_buf).expect_msg("read_block_from_tcp_stream: cannot decompress_vec");
    
    return decompressed;
}

pub fn write_exact_bytes_to_tcp_stream(tcp_stream: &mut TcpStream, bytes: &[u8])->HubResult<()>{
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &bytes[(bytes_total - bytes_left)..bytes_total];
        let bytes_written = tcp_stream.write(buf).expect_msg("write_exact_bytes_to_tcp_stream: block write fail")?;
        bytes_left -= bytes_written;
    }
    Ok(())
}

pub fn write_block_to_tcp_stream(tcp_stream: &mut TcpStream, msg_buf: &[u8], digest: &mut [u64; 26])->HubResult<()> {
    let bytes_total = msg_buf.len();
    
    if bytes_total > 250 * 1024 * 1024 {
        return Err(HubError::new("read_block_from_tcp_stream: bytes_total more than 250mb"))
    }
    
    let mut enc = snap::Encoder::new();
    let compressed = enc.compress_vec(msg_buf).expect_msg("read_block_from_tcp_stream: cannot compress msgbuf")?;
    
    digest_buffer(digest, &compressed);
    digest[25] = compressed.len() as u64;
    
    let digest_u8 = unsafe {std::mem::transmute::<&mut [u64; 26], &mut [u8; 26 * 8]>(digest)};
    write_exact_bytes_to_tcp_stream(tcp_stream, digest_u8)?;
    write_exact_bytes_to_tcp_stream(tcp_stream, &compressed)?;
    Ok(())
}

pub struct HubClient {
    pub own_addr: HubAddr,
    pub uid_alloc: u64,
    read_thread: Option<thread::JoinHandle<()>>,
    write_thread: Option<thread::JoinHandle<()>>,
    pub tx_read: mpsc::Sender<HubToClientMsg>,
    pub rx_read: mpsc::Receiver<HubToClientMsg>,
    pub tx_write: mpsc::Sender<ClientToHubMsg>
}

impl HubClient {
    pub fn connect_to_hub(key: &[u8], server_address: SocketAddr) -> HubResult<HubClient> {
        
        // first try local address
        let local_address =   SocketAddr::from(([127, 0, 0, 1], server_address.port()));
        let server_hubaddr;
        let mut tcp_stream = if let Ok(stream) = TcpStream::connect(local_address){
            server_hubaddr = HubAddr::from_socket_addr(server_address);
            stream
        }
        else{
            server_hubaddr = HubAddr::from_socket_addr(server_address);
            TcpStream::connect(server_address).expect_msg("connect_to_hub: cannot connect")?
        };
        
        let own_addr = HubAddr::from_socket_addr(tcp_stream.local_addr().expect("Cannot get client local address"));

        let (tx_read, rx_read) = mpsc::channel::<HubToClientMsg>();
        let (tx_write, rx_write) = mpsc::channel::<ClientToHubMsg>();
        let tx_read_copy = tx_read.clone();

        let mut digest = [0u64; 26];
        digest_buffer(&mut digest, key);
        
        let read_thread = {
            let mut tcp_stream = tcp_stream.try_clone().expect_msg("connect_to_hub: cannot clone socket")?;
            let digest = digest.clone();
            let server_hubaddr = server_hubaddr.clone();
            std::thread::spawn(move || {
                loop {
                    match read_block_from_tcp_stream(&mut tcp_stream, &mut digest.clone()){
                        Ok(msg_buf)=>{
                            let htc_msg: HubToClientMsg = bincode::deserialize(&msg_buf).expect("read_thread hub message deserialize fail - version conflict!");
                            tx_read.send(htc_msg).expect("tx_read.send fails - should never happen");
                        },
                        Err(e)=>{
                            if tcp_stream.shutdown(Shutdown::Both).is_ok(){
                                tx_read.send(HubToClientMsg{
                                    from:server_hubaddr.clone(),
                                    msg:HubMsg::ConnectionError(e)
                                }).expect("tx_read.send fails - should never happen");
                            }
                            return
                        }
                    }
                }
            })
        };

        let write_thread = {
            let digest = digest.clone();
            let tx_read = tx_read_copy.clone();
            let server_hubaddr = server_hubaddr.clone();
            std::thread::spawn(move || {// this one cannot send to the read channel.
                while let Ok(cth_msg) = rx_write.recv() {
                    let msg_buf = bincode::serialize(&cth_msg).expect("write_thread hub message serialize fail - should never happen");
                    if let Err(e) = write_block_to_tcp_stream(&mut tcp_stream, &msg_buf, &mut digest.clone()){
                        // disconnect the socket and send shutdown
                        if tcp_stream.shutdown(Shutdown::Both).is_ok(){;
                            tx_read.send(HubToClientMsg{
                                from:server_hubaddr.clone(),
                                msg:HubMsg::ConnectionError(e)
                            }).expect("tx_read.send fails - should never happen");
                        }
                        return
                    }
                }
            })
        };
        
        Ok(HubClient {
            uid_alloc:0,
            own_addr: own_addr,
            read_thread: Some(read_thread),
            write_thread: Some(write_thread),
            tx_read: tx_read_copy,
            rx_read: rx_read,
            tx_write: tx_write
        })
    }

    pub fn wait_for_announce(key: &[u8]) -> Result<SocketAddr, std::io::Error> {
        Self::wait_for_announce_on(key, SocketAddr::from(([0, 0, 0, 0], HUB_ANNOUNCE_PORT)))
    }
    
    pub fn wait_for_announce_on(key: &[u8], announce_address: SocketAddr) -> Result<SocketAddr, std::io::Error> {
        let socket = UdpSocket::bind(announce_address)?;
        loop {
            let mut digest = [0u64; 26];
            let digest_u8 = unsafe {std::mem::transmute::<&mut [u64; 26], &mut [u8; 26 * 8]>(&mut digest)};
            
            let (bytes, from) = socket.recv_from(digest_u8) ?;
            if bytes != 26 * 8 {
                println!("Announce port wrong bytecount");
            }
            
            let mut check_digest = [0u64; 26];
            check_digest[25] = digest[25];
            check_digest[0] = digest[25];
            digest_buffer(&mut check_digest, key);
            
            if check_digest == digest { // use this to support multiple hubs on one network
                let listen_port = digest[25];
                return Ok(match from {
                    SocketAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(*v4.ip(), listen_port as u16)),
                    SocketAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(*v6.ip(), listen_port as u16, v6.flowinfo(), v6.scope_id())),
                })
            }
            
            println!("wait for announce found wrong digest");
        }
    }
    
    pub fn join_threads(&mut self) {
        self.read_thread.take().expect("cant take read thread").join().expect("cant join read thread");
        self.write_thread.take().expect("cant take write thread").join().expect("cant join write thread");
    }
    
    pub fn alloc_uid(&mut self)->HubUid{
        self.uid_alloc += 1;
        return HubUid{
            addr:self.own_addr,
            id: self.uid_alloc
        }
    }
    
}

// digest function to hash tcp data to enable error checking and multiple servers on one network

pub fn digest_buffer(digest: &mut [u64; 26], msg_buf: &[u8]) {
    let digest_u8 = unsafe {std::mem::transmute::<&mut [u64; 26], &mut [u8; 26 * 8]>(digest)};
    let mut s = 0;
    for i in 0..msg_buf.len() {
        digest_u8[s] ^= msg_buf[i];
        s += 1;
        if s >= 25 * 8 {
            digest_process_chunk(digest);
            s = 0;
        }
    }
    digest_process_chunk(digest);
}

const PLEN: usize = 25;
const RHO: [u32; 24] = [1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,];
const PI: [usize; 24] = [10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,];
const RC: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

#[cfg(not(feature = "no_unroll"))]
macro_rules!unroll5 {
    ( $ var: ident, $ body: block) => {
        {const $ var: usize = 0; $ body;}
        {const $ var: usize = 1; $ body;}
        {const $ var: usize = 2; $ body;}
        {const $ var: usize = 3; $ body;}
        {const $ var: usize = 4; $ body;}
    };
}

#[cfg(feature = "no_unroll")]
macro_rules!unroll5 {
    ( $ var: ident, $ body: block) => {
        for $ var in 0..5 $ body
    }
}

#[cfg(not(feature = "no_unroll"))]
macro_rules!unroll24 {
    ( $ var: ident, $ body: block) => {
        {const $ var: usize = 0; $ body;}
        {const $ var: usize = 1; $ body;}
        {const $ var: usize = 2; $ body;}
        {const $ var: usize = 3; $ body;}
        {const $ var: usize = 4; $ body;}
        {const $ var: usize = 5; $ body;}
        {const $ var: usize = 6; $ body;}
        {const $ var: usize = 7; $ body;}
        {const $ var: usize = 8; $ body;}
        {const $ var: usize = 9; $ body;}
        {const $ var: usize = 10; $ body;}
        {const $ var: usize = 11; $ body;}
        {const $ var: usize = 12; $ body;}
        {const $ var: usize = 13; $ body;}
        {const $ var: usize = 14; $ body;}
        {const $ var: usize = 15; $ body;}
        {const $ var: usize = 16; $ body;}
        {const $ var: usize = 17; $ body;}
        {const $ var: usize = 18; $ body;}
        {const $ var: usize = 19; $ body;}
        {const $ var: usize = 20; $ body;}
        {const $ var: usize = 21; $ body;}
        {const $ var: usize = 22; $ body;}
        {const $ var: usize = 23; $ body;}
    };
}

#[cfg(feature = "no_unroll")]
macro_rules!unroll24 {
    ( $ var: ident, $ body: block) => {
        for $ var in 0..24 $ body
    }
}

#[allow(non_upper_case_globals, unused_assignments)]
fn digest_process_chunk(a: &mut [u64; PLEN + 1]) {
    for i in 0..24 {
        let mut array = [0u64; 5];
        
        // Theta
        unroll5!(x, {
            unroll5!(y, {
                array[x] ^= a[5 * y + x];
            });
        });
        
        unroll5!(x, {
            unroll5!(y, {
                let t1 = array[(x + 4) % 5];
                let t2 = array[(x + 1) % 5].rotate_left(1);
                a[5 * y + x] ^= t1 ^ t2;
            });
        });
        
        // Rho and pi
        let mut last = a[1];
        unroll24!(x, {
            array[0] = a[PI[x]];
            a[PI[x]] = last.rotate_left(RHO[x]);
            last = array[0];
        });
        
        // Chi
        unroll5!(y_step, {
            let y = 5 * y_step;
            
            unroll5!(x, {
                array[x] = a[y + x];
            });
            
            unroll5!(x, {
                let t1 = !array[(x + 1) % 5];
                let t2 = array[(x + 2) % 5];
                a[y + x] = array[x] ^ (t1 & t2);
            });
        });
        
        // Iota
        a[0] ^= RC[i];
    }
}