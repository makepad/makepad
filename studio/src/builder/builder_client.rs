use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        builder::{
            builder_protocol::{BuilderCmd, BuilderCmdWrap, BuilderMsgWrap, BuilderCmdId},
            builder_server::{BuilderConnection, BuilderServer},
        }
    },
    std::{
        env,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
        path::PathBuf
    },
};

live_register!{
    BuilderClient: {{BuilderClient}} {}
}

#[derive(Live)]
pub struct BuilderClient {
    bind: Option<String>,
    path: String,
    #[rust] cmd_id_counter: u64,
    #[rust] inner: Option<BuilderClientInner>
}

impl LiveHook for BuilderClient{
    fn after_apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.inner.is_none(){
            self.inner = Some(BuilderClientInner::new_with_local_server(cx, &self.path))
        }
    }
}

impl BuilderClient{
    
    pub fn send_cmd(&mut self, cmd: BuilderCmd) {
        self.inner.as_ref().unwrap().cmd_sender.send(BuilderCmdWrap{
            cmd_id: BuilderCmdId(self.cmd_id_counter),
            cmd
        }).unwrap();
        self.cmd_id_counter += 1;
    }
    
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> Vec<BuilderMsgWrap> {
        let mut a = Vec::new();
        self.handle_event_with_fn(cx, event, &mut | _, v | a.push(v));
        a
    }
    
    pub fn handle_event_with_fn(&mut self, cx: &mut Cx, event: &mut Event, dispatch_msg: &mut dyn FnMut(&mut Cx, BuilderMsgWrap)) {
        let inner = self.inner.as_ref().unwrap();
        match event {
            Event::Signal(event)
            if event.signals.contains_key(&inner.msg_signal) => {
                loop {
                    match inner.msg_receiver.try_recv() {
                        Ok(msg) => dispatch_msg(cx, msg),
                        Err(TryRecvError::Empty) => break,
                        _ => panic!(),
                    }
                }
            }
            _ => {}
        }
    }
    
}

pub struct BuilderClientInner {
    pub cmd_sender: Sender<BuilderCmdWrap>,
    pub msg_signal: Signal,
    pub msg_receiver: Receiver<BuilderMsgWrap>,
}

impl BuilderClientInner {
    pub fn new_with_local_server(cx: &mut Cx, subdir:&str) -> Self {
        let (cmd_sender, cmd_receiver) = mpsc::channel();
        let msg_signal = cx.new_signal();
        let (msg_sender, msg_receiver) = mpsc::channel();
        
        let base_path = env::current_dir().unwrap();
        let final_path = base_path.join(subdir.split('/').collect::<PathBuf>());
        
        let mut server = BuilderServer::new(final_path);
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
    msg_receiver: Receiver<BuilderMsgWrap>,
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

fn _spawn_cmd_sender(cmd_receiver: Receiver<BuilderCmdWrap>, mut stream: TcpStream) {
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
    msg_sender: Sender<BuilderMsgWrap>,
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
    cmd_receiver: Receiver<BuilderCmdWrap>,
    connection: BuilderConnection,
) {
    thread::spawn(move || loop {
        let cmd = cmd_receiver.recv().unwrap();
        connection.handle_cmd(cmd);
    });
}
