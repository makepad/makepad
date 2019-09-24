use crate::hubmsg::*;
use std::net::{TcpStream, UdpSocket, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::io::prelude::*;
use std::sync::{mpsc};

pub fn read_exact_bytes_from_tcp_stream(tcp_stream: &mut TcpStream, bytes:&mut [u8]) {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &mut bytes[(bytes_total - bytes_left)..bytes_total];
        let bytes_read = tcp_stream.read(buf).expect("block read fail");
        bytes_left -= bytes_read;
    }
}

pub fn read_block_from_tcp_stream(tcp_stream: &mut TcpStream, check_digest:&mut [u64;26]) -> Vec<u8> {
    // we read 4 bytes for the buffer len
    let mut digest = [0u64; 26];

    let digest_u8 = unsafe{std::mem::transmute::<&mut [u64;26], &mut [u8;26*8]>(&mut digest)};
    read_exact_bytes_from_tcp_stream(tcp_stream, digest_u8);

    let bytes_total = digest[25] as usize;
    if bytes_total > 250 * 1024 * 1024 { // 250 mb limit.
        panic!("bytes_total more than 250mb");
    }
    
    let mut msg_buf = Vec::new();
    msg_buf.resize(bytes_total, 0);
    read_exact_bytes_from_tcp_stream(tcp_stream, &mut msg_buf);

    digest_buffer(check_digest, &msg_buf);
    check_digest[25] = bytes_total as u64;
    
    if check_digest != &mut digest{
        panic!("block digest check failed");
    }
    
    return msg_buf;
}

pub fn write_exact_bytes_to_tcp_stream(tcp_stream: &mut TcpStream, bytes:&[u8]){
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &bytes[(bytes_total - bytes_left)..bytes_total];
        let bytes_written = tcp_stream.write(buf).expect("block write fail");
        bytes_left -= bytes_written;
    }
}

pub fn write_block_to_tcp_stream(tcp_stream: &mut TcpStream, msg_buf: &[u8], digest:&mut [u64;26]) {
    // we read 4 bytes for the buffer len
    let bytes_total = msg_buf.len();

    if bytes_total > 250 * 1024 * 1024 { // 250 mb limit.
        panic!("bytes_total more than 250mb");
    }
    
    digest_buffer(digest, &msg_buf);
    digest[25] = bytes_total as u64;

    // let output the bytes_total and key_state     
    let digest_u8 = unsafe {std::mem::transmute::<&mut [u64;26], &mut [u8; 26*8]>(digest)};
    write_exact_bytes_to_tcp_stream(tcp_stream, digest_u8);
    
    write_exact_bytes_to_tcp_stream(tcp_stream, &msg_buf)
}

pub struct HubClient {
    read_thread: Option<std::thread::JoinHandle<()>>,
    write_thread: Option<std::thread::JoinHandle<()>>,
    pub rx_read: mpsc::Receiver<HubToClientMsg>,
    pub tx_write: mpsc::Sender<ClientToHubMsg>
}

impl HubClient {
    pub fn connect_to_hub(key:&[u8], server_address: SocketAddr) -> Result<HubClient, std::io::Error> {
        
        let mut read_tcp_stream = TcpStream::connect(server_address) ?;
        let mut write_tcp_stream = read_tcp_stream.try_clone().expect("Cannot clone tcp stream");
        let (tx_read, rx_read) = mpsc::channel::<HubToClientMsg>();

        let mut digest = [0u64;26];
        digest_buffer(&mut digest, key);
        let read_digest = digest.clone();
        let read_thread = std::thread::spawn(move || {
            loop {
                let mut digest = read_digest.clone();
                let msg_buf = read_block_from_tcp_stream(&mut read_tcp_stream, &mut digest);
                let htc_msg: HubToClientMsg = bincode::deserialize(&msg_buf).expect("read_thread hub message deserialize fail");
                tx_read.send(htc_msg).expect("tx_read fails");
            }
        });
        let write_digest = digest.clone();
        let (tx_write, rx_write) = mpsc::channel::<ClientToHubMsg>();
        let write_thread = std::thread::spawn(move || {
            while let Ok(cth_msg) = rx_write.recv() {
                let mut digest = write_digest.clone();
                let msg_buf = bincode::serialize(&cth_msg).expect("write_thread hub message serialize fail");
                write_block_to_tcp_stream(&mut write_tcp_stream, &msg_buf, &mut digest);
            }
        });
        Ok(HubClient {
            read_thread: Some(read_thread),
            write_thread: Some(write_thread),
            rx_read: rx_read,
            tx_write: tx_write
        })
    }
    
    pub fn wait_for_announce(key:&[u8], announce_address: SocketAddr) -> Result<SocketAddr, std::io::Error> {
        let socket = UdpSocket::bind(announce_address) ?;
        loop{
            let mut digest = [0u64; 26];
            let digest_u8 = unsafe {std::mem::transmute::<&mut [u64;26], &mut [u8; 26*8]>(&mut digest)};
    
            let (bytes, from) = socket.recv_from(digest_u8) ?;
            if bytes != 26*8 {
                panic!("Announce port wrong bytecount");
            }
            
            let mut check_digest = [0u64; 26];
            check_digest[25] = digest[25];
            check_digest[0] = digest[25];
            digest_buffer(&mut check_digest, key);
            
            if check_digest == digest{ // use this to support multiple hubs on one network
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
    
}

// digest function to hash tcp data to enable error checking and multiple servers on one network

pub fn digest_buffer(digest:&mut [u64;26], msg_buf: &[u8]) {
    let digest_u8 = unsafe{std::mem::transmute::<&mut [u64;26], &mut [u8;26*8]>(digest)};
    let mut s = 0;
    for i in 0..msg_buf.len(){
        digest_u8[s] ^= msg_buf[i];
        s += 1;
        if s >= 25*8{
            digest_process_chunk(digest);
            s = 0;
        }
    }
    digest_process_chunk(digest);
}

const PLEN: usize = 25;
const RHO: [u32; 24] = [
    1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18,
    39, 61, 20, 44,
];

const PI: [usize; 24] = [
    10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14,
    22, 9, 6, 1,
];

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
macro_rules! unroll5 {
    ($var:ident, $body:block) => {
        { const $var: usize = 0; $body; }
        { const $var: usize = 1; $body; }
        { const $var: usize = 2; $body; }
        { const $var: usize = 3; $body; }
        { const $var: usize = 4; $body; }
    };
}

#[cfg(feature = "no_unroll")]
macro_rules! unroll5 {
    ($var:ident, $body:block) => {
        for $var in 0..5 $body
    }
}

#[cfg(not(feature = "no_unroll"))]
macro_rules! unroll24 {
    ($var: ident, $body: block) => {
        { const $var: usize = 0; $body; }
        { const $var: usize = 1; $body; }
        { const $var: usize = 2; $body; }
        { const $var: usize = 3; $body; }
        { const $var: usize = 4; $body; }
        { const $var: usize = 5; $body; }
        { const $var: usize = 6; $body; }
        { const $var: usize = 7; $body; }
        { const $var: usize = 8; $body; }
        { const $var: usize = 9; $body; }
        { const $var: usize = 10; $body; }
        { const $var: usize = 11; $body; }
        { const $var: usize = 12; $body; }
        { const $var: usize = 13; $body; }
        { const $var: usize = 14; $body; }
        { const $var: usize = 15; $body; }
        { const $var: usize = 16; $body; }
        { const $var: usize = 17; $body; }
        { const $var: usize = 18; $body; }
        { const $var: usize = 19; $body; }
        { const $var: usize = 20; $body; }
        { const $var: usize = 21; $body; }
        { const $var: usize = 22; $body; }
        { const $var: usize = 23; $body; }
    };
}

#[cfg(feature = "no_unroll")]
macro_rules! unroll24 {
    ($var:ident, $body:block) => {
        for $var in 0..24 $body
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