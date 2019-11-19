use crate::hubmsg::*;
use crate::hubrouter::*;

use std::net::{TcpStream, UdpSocket, SocketAddr, SocketAddrV4, SocketAddrV6, Shutdown};
use std::io::prelude::*;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use serde::{Serialize, Deserialize};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::io::AsRawFd;

trait ResultMsg<T> {
    fn expect_msg(self, msg: &str) -> Result<T, HubError>;
}

impl<T> ResultMsg<T> for Result<T, std::io::Error> {
    fn expect_msg(self, msg: &str) -> Result<T, HubError> {
        match self {
            Err(v) => Err(HubError {msg: format!("{}: {}", msg.to_string(), v.to_string())}),
            Ok(v) => Ok(v)
        }
    }
}

impl<T> ResultMsg<T> for Result<T, snap::Error> {
    fn expect_msg(self, msg: &str) -> Result<T, HubError> {
        match self {
            Err(v) => Err(HubError {msg: format!("{}: {}", msg.to_string(), v.to_string())}),
            Ok(v) => Ok(v)
        }
    }
}

type HubResult<T> = Result<T, HubError>;

pub const HUB_ANNOUNCE_PORT: u16 = 46243;

pub fn read_exact_bytes_from_tcp_stream(tcp_stream: &mut TcpStream, bytes: &mut [u8]) -> HubResult<()> {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &mut bytes[(bytes_total - bytes_left)..bytes_total];
        let bytes_read = tcp_stream.read(buf).expect_msg("read_exact_bytes_from_tcp_stream: read failed") ?;
        if bytes_read == 0 {
            return Err(HubError::new("read_exact_bytes_from_tcp_stream - cannot read bytes"));
        }
        bytes_left -= bytes_read;
    }
    Ok(())
}

pub fn read_block_from_tcp_stream(tcp_stream: &mut TcpStream, mut check_digest: Digest) -> HubResult<Vec<u8>> {
    let mut dwd_read = DigestWithData::default();
    
    let dwd_u8 = unsafe {std::mem::transmute::<&mut DigestWithData, &mut [u8; 26 * 8]>(&mut dwd_read)};
    read_exact_bytes_from_tcp_stream(tcp_stream, dwd_u8) ?;
    
    let bytes_total = dwd_read.data as usize;
    if bytes_total > 250 * 1024 * 1024 {
        return Err(HubError::new("read_block_from_tcp_stream: bytes_total more than 250mb"))
    }
    
    let mut msg_buf = Vec::new();
    msg_buf.resize(bytes_total, 0);
    read_exact_bytes_from_tcp_stream(tcp_stream, &mut msg_buf) ?;
    
    check_digest.digest_buffer(&msg_buf);
    
    if check_digest != dwd_read.digest {
        return Err(HubError::new("read_block_from_tcp_stream: block digest check failed"))
    }
    
    let mut dec = snap::Decoder::new();
    let decompressed = dec.decompress_vec(&msg_buf).expect_msg("read_block_from_tcp_stream: cannot decompress_vec");
    
    return decompressed;
}

pub fn write_exact_bytes_to_tcp_stream(tcp_stream: &mut TcpStream, bytes: &[u8]) -> HubResult<()> {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &bytes[(bytes_total - bytes_left)..bytes_total];
        let bytes_written = tcp_stream.write(buf).expect_msg("write_exact_bytes_to_tcp_stream: block write fail") ?;
        if bytes_written == 0 {
            return Err(HubError::new("write_exact_bytes_to_tcp_stream - cannot write bytes"));
        }
        bytes_left -= bytes_written;
    }
    Ok(())
}

pub fn write_block_to_tcp_stream(tcp_stream: &mut TcpStream, msg_buf: &[u8], digest: Digest) -> HubResult<()> {
    let bytes_total = msg_buf.len();
    
    if bytes_total > 250 * 1024 * 1024 {
        return Err(HubError::new("read_block_from_tcp_stream: bytes_total more than 250mb"))
    }
    
    let mut enc = snap::Encoder::new();
    let compressed = enc.compress_vec(msg_buf).expect_msg("read_block_from_tcp_stream: cannot compress msgbuf") ?;
    
    let mut dwd_write = DigestWithData{
        digest:digest,
        data: compressed.len() as u64
    };
    
    dwd_write.digest.digest_buffer(&compressed);
    
    let dwd_u8 = unsafe {std::mem::transmute::<&DigestWithData, &[u8; 26 * 8]>(&dwd_write)};
    write_exact_bytes_to_tcp_stream(tcp_stream, dwd_u8) ?;
    write_exact_bytes_to_tcp_stream(tcp_stream, &compressed) ?;
    Ok(())
}

pub struct HubClient {
    pub own_addr: HubAddr,
    pub server_addr: HubAddr,
    pub uid_alloc: u64,
    read_thread: Option<thread::JoinHandle<()>>,
    write_thread: Option<thread::JoinHandle<()>>,
    pub tx_read: mpsc::Sender<FromHubMsg>,
    pub rx_read: Option<mpsc::Receiver<FromHubMsg>>,
    pub tx_write: mpsc::Sender<ToHubMsg>
}

#[derive(Default, Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct DigestWithData{
    pub digest:Digest,
    pub data: u64
}

impl HubClient {
    pub fn connect_to_server(digest: Digest, server_address: SocketAddr, hub_log: HubLog) -> HubResult<HubClient> {
        
        // first try local address
        let local_address = SocketAddr::from(([127, 0, 0, 1], server_address.port()));
        let server_hubaddr;
        let mut tcp_stream = if let Ok(stream) = TcpStream::connect(local_address) {
            server_hubaddr = HubAddr::from_socket_addr(local_address);
            stream
        }
        else {
            server_hubaddr = HubAddr::from_socket_addr(server_address);
            TcpStream::connect(server_address).expect_msg("connect_to_hub: cannot connect") ?
        };
        
        let own_addr = HubAddr::from_socket_addr(tcp_stream.local_addr().expect("Cannot get client local address"));
        
        let (tx_read, rx_read) = mpsc::channel::<FromHubMsg>();
        let (tx_write, rx_write) = mpsc::channel::<ToHubMsg>();
        let tx_read_copy = tx_read.clone();
        let tx_write_copy = tx_write.clone();
        
        let read_thread = {
            let mut tcp_stream = tcp_stream.try_clone().expect_msg("connect_to_hub: cannot clone socket") ?;
            let digest = digest.clone();
            let server_hubaddr = server_hubaddr.clone();
            let hub_log = hub_log.clone();
            std::thread::spawn(move || {
                loop {
                    match read_block_from_tcp_stream(&mut tcp_stream, digest.clone()) {
                        Ok(msg_buf) => {
                            let htc_msg: FromHubMsg = bincode::deserialize(&msg_buf).expect("read_thread hub message deserialize fail - version conflict!");
                            hub_log.msg("HubClient received", &htc_msg);
                            tx_read.send(htc_msg).expect("tx_read.send fails - should never happen");
                        },
                        Err(e) => {
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            tx_read.send(FromHubMsg {
                                from: server_hubaddr.clone(),
                                msg: HubMsg::ConnectionError(e.clone())
                            }).expect("tx_read.send fails - should never happen");
                            // lets break rx write
                            let _ = tx_write_copy.send(ToHubMsg {
                                to: HubMsgTo::Hub,
                                msg: HubMsg::ConnectionError(e)
                            });
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
            let hub_log = hub_log.clone();
            std::thread::spawn(move || { // this one cannot send to the read channel.
                while let Ok(cth_msg) = rx_write.recv() {
                    hub_log.msg("HubClient sending", &cth_msg);
                    match &cth_msg.msg {
                        HubMsg::ConnectionError(_) => { // we are closed by the read loop
                            return
                        },
                        _ => ()
                    }
                    
                    let msg_buf = bincode::serialize(&cth_msg).expect("write_thread hub message serialize fail - should never happen");
                    if let Err(e) = write_block_to_tcp_stream(&mut tcp_stream, &msg_buf, digest.clone()) {
                        // disconnect the socket and send shutdown
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                        let _ = tx_read.send(FromHubMsg {
                            from: server_hubaddr.clone(),
                            msg: HubMsg::ConnectionError(e)
                        });
                        return
                    }
                }
            })
        };
        
        Ok(HubClient {
            uid_alloc: 0,
            own_addr: own_addr,
            server_addr: server_hubaddr,
            read_thread: Some(read_thread),
            write_thread: Some(write_thread),
            tx_read: tx_read_copy,
            rx_read: Some(rx_read),
            tx_write: tx_write
        })
    }
    
    pub fn wait_for_announce(digest: Digest) -> Result<SocketAddr, std::io::Error> {
        Self::wait_for_announce_on(digest, SocketAddr::from(([0, 0, 0, 0], HUB_ANNOUNCE_PORT)))
    }
    
    pub fn wait_for_announce_on(digest: Digest, announce_address: SocketAddr) -> Result<SocketAddr, std::io::Error> {
        
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        fn reuse_addr(socket: &mut UdpSocket) {
            unsafe {
                let optval: libc::c_int = 1;
                let _ = libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_REUSEADDR,
                    &optval as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&optval) as libc::socklen_t,
                );
            }
        }
        
        #[cfg(any(target_os = "windows", target_arch = "wasm32"))]
        fn reuse_addr(_socket: &mut UdpSocket) {
        }
        
        loop {
            if let Ok(mut socket) = UdpSocket::bind(announce_address) {
                // TODO. FIX FOR WINDOWS
                reuse_addr(&mut socket);
                let mut dwd_read = DigestWithData::default();
                let dwd_u8 = unsafe {std::mem::transmute::<&mut DigestWithData, &mut [u8; 26 * 8]>(&mut dwd_read)};
                
                let (bytes, from) = socket.recv_from(dwd_u8) ?;
                if bytes != 26 * 8 {
                    println!("Announce port wrong bytecount");
                }
                
                let mut dwd_check = DigestWithData{
                    digest: digest.clone(),
                    data: dwd_read.data
                };
                dwd_check.data = dwd_read.data;
                dwd_check.digest.buf[0] ^= dwd_read.data;
                dwd_check.digest.digest_cycle();
                
                if dwd_check == dwd_read { // use this to support multiple hubs on one network
                    let listen_port = dwd_read.data;
                    return Ok(match from {
                        SocketAddr::V4(v4) => SocketAddr::V4(SocketAddrV4::new(*v4.ip(), listen_port as u16)),
                        SocketAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(*v6.ip(), listen_port as u16, v6.flowinfo(), v6.scope_id())),
                    })
                }
            }
            //else{
            //    println!("wait for announce bind failed");
            //}
        }
    }
    
    pub fn join_threads(&mut self) {
        self.read_thread.take().expect("cant take read thread").join().expect("cant join read thread");
        self.write_thread.take().expect("cant take write thread").join().expect("cant join write thread");
    }
    
    pub fn alloc_uid(&mut self) -> HubUid {
        self.uid_alloc += 1;
        return HubUid {
            addr: self.own_addr,
            id: self.uid_alloc
        }
    }
    
    pub fn get_route_send(&self) -> HubRouteSend {
        HubRouteSend::Networked {
            uid_alloc: Arc::new(Mutex::new(0)),
            tx_write_arc: Arc::new(Mutex::new(Some(self.tx_write.clone()))),
            own_addr_arc: Arc::new(Mutex::new(Some(self.own_addr)))
        }
    }
    
    pub fn get_route_send_in_place(&self, route_send: &HubRouteSend) {
        route_send.update_networked_in_place(Some(self.own_addr), Some(self.tx_write.clone()))
    }
}


#[derive(Eq, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Digest {
    pub buf: [u64; 25]
}

impl Default for Digest {
    fn default() -> Self {Self {buf: [0u64; 25]}}
}

impl Digest {
    
    pub fn generate() -> Digest {
        let mut result = Digest::default();
        for i in 0..25 {
            result.buf[i] ^= time::precise_time_ns();
            std::thread::sleep(std::time::Duration::from_millis(1));
            result.digest_cycle();
        }
        result
    }
    
    pub fn digest_cycle(&mut self){
        digest_cycle(self);
    }

    pub fn digest_other(&mut self, other: &Digest) {
        for i in 0..25{
            self.buf[i] ^= other.buf[i]
        }
        self.digest_cycle();
    }
    
    pub fn digest_buffer(&mut self, msg_buf: &[u8]) {
        let digest_u8 = unsafe {std::mem::transmute::<&mut Digest, &mut [u8; 26 * 8]>(self)};
        let mut s = 0;
        for i in 0..msg_buf.len() {
            digest_u8[s] ^= msg_buf[i];
            s += 1;
            if s >= 25 * 8 {
                self.digest_cycle();
                s = 0;
            }
        }
        self.digest_cycle();
    }
    
}

// digest function to hash tcp data to enable error checking and multiple servers on one network, found various
// similar versions of this on crates.io and github (as MIT). Not sure which one to attribute it to. Thanks whoever wrote this :)

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
pub fn digest_cycle(a:&mut Digest) {
    for i in 0..24 {
        let mut array = [0u64; 5];
        
        // Theta
        unroll5!(x, {
            unroll5!(y, {
                array[x] ^= a.buf[5 * y + x];
            });
        });
        
        unroll5!(x, {
            unroll5!(y, {
                let t1 = array[(x + 4) % 5];
                let t2 = array[(x + 1) % 5].rotate_left(1);
                a.buf[5 * y + x] ^= t1 ^ t2;
            });
        });
        
        // Rho and pi
        let mut last = a.buf[1];
        unroll24!(x, {
            array[0] = a.buf[PI[x]];
            a.buf[PI[x]] = last.rotate_left(RHO[x]);
            last = array[0];
        });
        
        // Chi
        unroll5!(y_step, {
            let y = 5 * y_step;
            
            unroll5!(x, {
                array[x] = a.buf[y + x];
            });
            
            unroll5!(x, {
                let t1 = !array[(x + 1) % 5];
                let t2 = array[(x + 2) % 5];
                a.buf[y + x] = array[x] ^ (t1 & t2);
            });
        });
        
        // Iota
        a.buf[0] ^= RC[i];
    }
}
