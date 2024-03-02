#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::{*, cx_stdin::aux_chan},
        build_manager::{
            build_protocol::{BuildCmd, BuildCmdWrap, BuildClientMessageWrap, LogItem},
            build_server::{BuildConnection, BuildServer},
        }
    },
    std::{
        path::Path,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
        env,
        path::PathBuf
    },
};

pub struct BuildClient{
    pub cmd_sender: Sender<BuildCmdWrap>,
    pub msg_signal: SignalToUI,
    pub msg_receiver: Receiver<BuildClientMessageWrap>,
}
 
impl BuildClient {

    #[cfg(not(target_arch = "wasm32"))]
    pub fn send_cmd_with_id(&self, cmd_id: LiveId, cmd: BuildCmd){
        self.cmd_sender.send(BuildCmdWrap{
            cmd_id,
            cmd
        }).unwrap();
    }
    
    #[cfg(target_arch = "wasm32")]
    pub fn send_cmd_with_id(&self, cmd_id: BuildCmdId, cmd: BuildCmd){}
     
    #[cfg(target_arch = "wasm32")]
    pub fn new_with_local_server(_ubdir:&str) -> Self {
        let (cmd_sender, _cmd_receiver) = mpsc::channel();
        let msg_signal = LiveId::unique().into();
        let (_msg_sender, msg_receiver) = mpsc::channel();
        Self {
            cmd_sender,
            msg_signal,
            msg_receiver,
        }
    }
    
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_with_local_server(path:&Path) -> Self {
        let (cmd_sender, cmd_receiver) = mpsc::channel();
        let msg_signal = SignalToUI::new();
        let (msg_sender, msg_receiver) = mpsc::channel();
        /*
        let mut root = "./".to_string();
        for arg in std::env::args(){
            if let Some(prefix) = arg.strip_prefix("--root="){
                root = prefix.to_string();
                break;
            }
        }
        let base_path = env::current_dir().unwrap().join(root);
        let final_path = base_path.join(subdir.split('/').collect::<PathBuf>());
        */
        let mut server = BuildServer::new(path);
        spawn_local_cmd_handler(
            cmd_receiver,
            server.connect(Box::new({
                let msg_sender = msg_sender.clone();
                let msg_signal = msg_signal.clone();
                move | msg_item:BuildClientMessageWrap | {
                    msg_sender.send(msg_item).unwrap();
                    msg_signal.set()
                }
            })),
        );
        /*spawn_connection_listener(TcpListener::bind("127.0.0.1:0").unwrap(), server);*/
        
        Self {
            cmd_sender,
            msg_signal,
            msg_receiver,
        }
    }
    
}
/*
fn spawn_connection_listener(listener: TcpListener, mut server: BuildServer) {
    thread::spawn(move || {
        log!("Builder Server listening on {}", listener.local_addr().unwrap());
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            log!("Builder Incoming connection from {}", stream.peer_addr().unwrap());
            let (msg_sender, msg_receiver) = mpsc::channel();
            let connection = server.connect(Box::new({
                let msg_sender = msg_sender.clone();
                move | msg | {
                    msg_sender.send(msg).unwrap();
                }
            }));
            spawn_remote_cmd_handler(
                connection,
                stream.try_clone().unwrap(),
            );
            spawn_msg_sender(msg_receiver, stream);
        }
    });
}*/
/*
fn spawn_remote_cmd_handler(
    connection: BuildConnection,
    mut stream: TcpStream,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 4];
        stream.read_exact(&mut len_bytes).unwrap();
        let len = u32::from_be_bytes(len_bytes);
        let mut request_bytes = vec![0; len as usize];
        stream.read_exact(&mut request_bytes).unwrap();
        
        let cmd = DeBin::deserialize_bin(request_bytes.as_slice()).unwrap();
        
        connection.handle_cmd(cmd);
    });
}*/
/*
fn spawn_msg_sender(
    msg_receiver: Receiver<BuildMsgWrap>,
    mut stream: TcpStream,
) {
    thread::spawn(move || loop {
        let msg = msg_receiver.recv().unwrap();
        let mut msg_bytes = Vec::new();
        
        msg.ser_bin(&mut msg_bytes);
        
        let len_bytes = msg_bytes.len().to_be_bytes();
        stream.write_all(&len_bytes).unwrap();
        stream.write_all(&msg_bytes).unwrap();
    });
}

fn _spawn_cmd_sender(cmd_receiver: Receiver<BuildCmdWrap>, mut stream: TcpStream) {
    thread::spawn(move || loop {
        let cmd = cmd_receiver.recv().unwrap();
        let mut cmd_bytes = Vec::new();
        cmd.ser_bin(&mut cmd_bytes);
        let len_bytes = cmd_bytes.len().to_be_bytes();
        stream.write_all(&len_bytes).unwrap();
        stream.write_all(&cmd_bytes).unwrap();
    });
}

fn _spawn_msg_receiver(
    mut stream: TcpStream,
    msg_signal: Signal,
    msg_sender: Sender<BuildMsgWrap>,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 4];
        stream.read_exact(&mut len_bytes).unwrap();
        
        let len = u32::from_be_bytes(len_bytes);
        let mut msg_bytes = vec![0; len as usize];
        stream.read_exact(&mut msg_bytes).unwrap();
        
        let msg = DeBin::deserialize_bin(msg_bytes.as_slice()).unwrap();
        
        msg_sender.send(msg).unwrap();
        msg_signal.set()
    });
}
*/
fn spawn_local_cmd_handler(
    cmd_receiver: Receiver<BuildCmdWrap>,
    connection: BuildConnection,
) {
    thread::spawn(move || while let Ok(cmd) = cmd_receiver.recv() {
        connection.handle_cmd(cmd);
    });
}
