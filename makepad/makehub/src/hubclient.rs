use crate::hubmessage::*;
use std::net::{TcpStream};
use std::io::prelude::*;
use std::sync::{mpsc};

pub fn read_block_from_tcp_stream(tcp_stream:&mut TcpStream)->Vec<u8>{
    // we read 4 bytes for the buffer len
    let mut head_buf = [0u8; 4];
    let head_bytecount = tcp_stream.read(&mut head_buf).expect("head read failure");
    let bytes_total = unsafe {std::mem::transmute::<[u8; 4], u32>(head_buf)} as usize;
    
    if head_bytecount != 4 {
        panic!("head_bytecount size not 4 or more than 250mb");
    }
    if bytes_total > 250 * 1024 * 1024 { // 250 mb limit.
        panic!("bytes_total more than 250mb");
    }
    
    let mut msg_buf = Vec::new();
    let mut bytes_left = bytes_total;
    msg_buf.resize(bytes_total, 0);
    while bytes_left > 0 {
        let buf = &mut msg_buf[(bytes_total - bytes_left)..bytes_total];
        let bytes_read = tcp_stream.read(buf).expect("block read fail");
        bytes_left -= bytes_read;
    }
    return msg_buf;
}

pub fn write_block_to_tcp_stream(tcp_stream:&mut TcpStream, msg_buf:Vec<u8>){
    // we read 4 bytes for the buffer len
    let bytes_total =  msg_buf.len();
    
    let mut head_buf =unsafe {std::mem::transmute::<u32,[u8; 4]>(bytes_total as u32)};
    let head_bytecount = tcp_stream.write(&mut head_buf).expect("head write failure");
    
    if head_bytecount != 4 {
        panic!("head_bytecount size not 4 or more than 250mb");
    }
    if bytes_total > 250 * 1024 * 1024 { // 250 mb limit.
        panic!("bytes_total more than 250mb");
    }
    
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &msg_buf[(bytes_total - bytes_left)..bytes_total];
        let bytes_read = tcp_stream.write(buf).expect("block write fail");
        bytes_left -= bytes_read;
    }
}

pub struct HubClient{
    read_thread: Option<std::thread::JoinHandle<()>>,
    write_thread: Option<std::thread::JoinHandle<()>>,
    pub rx_read: mpsc::Receiver<HubToClientMsg>,
    pub tx_write: mpsc::Sender<ClientToHubMsg>
}

impl HubClient{
    pub fn connect_to_hub(server_address:&str)->HubClient{
        let mut read_tcp_stream = TcpStream::connect(server_address).expect("Cannot connect to server_address");
        let mut write_tcp_stream = read_tcp_stream.try_clone().expect("Cannot clone tcp stream");
        let (tx_read, rx_read) = mpsc::channel::<HubToClientMsg>();
        let read_thread = std::thread::spawn(move || {
            loop {
                let msg_buf = read_block_from_tcp_stream(&mut read_tcp_stream);
                let htc_msg: HubToClientMsg = bincode::deserialize(&msg_buf).expect("read_thread hub message deserialize fail");
                tx_read.send(htc_msg).expect("tx_read fails");
            }
        });
        let (tx_write, rx_write) = mpsc::channel::<ClientToHubMsg>();
        let write_thread = std::thread::spawn(move || {
            while let Ok(cth_msg) = rx_write.recv() {
                let msg_buf = bincode::serialize(&cth_msg).expect("write_thread hub message serialize fail");
                write_block_to_tcp_stream(&mut write_tcp_stream, msg_buf);
            }
        });
        HubClient{
            read_thread:Some(read_thread),
            write_thread:Some(write_thread),
            rx_read: rx_read,
            tx_write: tx_write
        }
    }

    pub fn join_threads(&mut self) {
        self.read_thread.take().expect("cant take read thread").join().expect("cant join read thread");
        self.write_thread.take().expect("cant take write thread").join().expect("cant join write thread");
    }

}