use {
    crate::{
        builder::{
            builder_protocol::{BuilderCmd, BuilderMsg},
            builder_server::{BuilderConnection, BuilderServer},
        }
    },
    makepad_component::makepad_render,
    makepad_render::makepad_micro_serde::*,
    makepad_render::*,
    std::{
        env,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver, Sender},
        thread,
    },
};

live_register!{
    BuilderClient: {{BuilderClient}} {}
}

#[derive(Live)]
pub struct BuilderClient {
    bind: Option<String>,
    fs_root: String,
    #[rust] inner: Option<BuilderClientInner>
}

impl LiveHook for BuilderClient{
    fn after_apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.inner.is_none(){
            self.inner = Some(BuilderClientInner::new_with_local_server(cx))
        }
    }
}

pub struct BuilderClientInner {
    pub cmd_sender: Sender<BuilderCmd>,
    pub msg_signal: Signal,
    pub msg_receiver: Receiver<BuilderMsg>,
}

impl BuilderClientInner {
    pub fn new_with_local_server(cx: &mut Cx) -> Self {
        let (cmd_sender, cmd_receiver) = mpsc::channel();
        let msg_signal = cx.new_signal();
        let (msg_sender, msg_receiver) = mpsc::channel();
        
        let mut server = BuilderServer::new(env::current_dir().unwrap());
        spawn_local_cmd_handler(
            cmd_receiver,
            server.connect(Box::new({
                let msg_sender = msg_sender.clone();
                move | msg | {
                    msg_sender.send(msg).unwrap();
                    Cx::post_signal(msg_signal, 0);
                }
            })),
        );
        spawn_connection_listener(TcpListener::bind("127.0.0.1:0").unwrap(), server);
        
        Self {
            cmd_sender,
            msg_signal,
            msg_receiver,
        }
    }
    
    pub fn send_cmd(&mut self, cmd: BuilderCmd) {
        self.cmd_sender.send(cmd).unwrap();
    }
}

fn spawn_connection_listener(listener: TcpListener, mut server: BuilderServer) {
    thread::spawn(move || {
        println!("Builder Server listening on {}", listener.local_addr().unwrap());
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            println!("Builder Incoming connection from {}", stream.peer_addr().unwrap());
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
}

fn spawn_remote_cmd_handler(
    connection: BuilderConnection,
    mut stream: TcpStream,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 8];
        stream.read_exact(&mut len_bytes).unwrap();
        let len = usize::from_be_bytes(len_bytes);
        let mut request_bytes = vec![0; len];
        stream.read_exact(&mut request_bytes).unwrap();
        
        let cmd = DeBin::deserialize_bin(request_bytes.as_slice()).unwrap();

        connection.handle_cmd(cmd);
    });
}

fn spawn_msg_sender(
    msg_receiver: Receiver<BuilderMsg>,
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

fn _spawn_cmd_sender(cmd_receiver: Receiver<BuilderCmd>, mut stream: TcpStream) {
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
    msg_sender: Sender<BuilderMsg>,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 8];
        stream.read_exact(&mut len_bytes).unwrap();
        
        let len = usize::from_be_bytes(len_bytes);
        let mut msg_bytes = vec![0; len];
        stream.read_exact(&mut msg_bytes).unwrap();
        
        let msg = DeBin::deserialize_bin(msg_bytes.as_slice()).unwrap();
        
        msg_sender.send(msg).unwrap();
        Cx::post_signal(msg_signal, 0);
    });
}

fn spawn_local_cmd_handler(
    cmd_receiver: Receiver<BuilderCmd>,
    connection: BuilderConnection,
) {
    thread::spawn(move || loop {
        let cmd = cmd_receiver.recv().unwrap();
        connection.handle_cmd(cmd);
    });
}
