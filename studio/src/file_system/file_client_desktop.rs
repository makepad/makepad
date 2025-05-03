use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        makepad_file_protocol::{FileRequest, FileClientMessage},
        makepad_file_server::{FileServerConnection, FileServer, FileSystemRoots},
    },
    std::{
        //env,
        io::{Read, Write},
        net::{ TcpStream},
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
    },
};

#[derive(Default)]
pub struct FileClient {
//    bind: Option<String>,
    //path: String,
    pub inner: Option<FileClientInner>
}

pub struct FileClientInner {
    pub request_sender: Sender<FileRequest>,
    pub message_signal: SignalToUI,
    pub message_receiver: Receiver<FileClientMessage>,
}

impl FileClient {
    pub fn init(&mut self, _cx:&mut Cx, roots:FileSystemRoots){
        if self.inner.is_none() {
            self.inner = Some(FileClientInner::new_with_local_server(roots))
        }
    }
    
    pub fn send_request(&mut self, request: FileRequest) {
        self.inner.as_ref().unwrap().request_sender.send(request).unwrap();
    }
    
    pub fn request_sender(&mut self) -> impl FnMut(FileRequest) + '_ {
        let request_sender = &self.inner.as_ref().unwrap().request_sender;
        move | request | request_sender.send(request).unwrap()
    }
    
    pub fn poll_messages(&mut self)->Vec<FileClientMessage> {
        let mut messages = Vec::new();
        let inner = self.inner.as_ref().unwrap();
        loop {
            match inner.message_receiver.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) => break,
                _ => panic!(),
            }
        }
        messages
    }
}

impl FileClientInner {
    pub fn new_with_local_server(roots:FileSystemRoots) -> Self {
        let (request_sender, request_receiver) = mpsc::channel();
        let message_signal = SignalToUI::new();
        let (message_sender, message_receiver) = mpsc::channel();
        
        /*let mut root = "./".to_string();
        for arg in std::env::args(){
            if let Some(prefix) = arg.strip_prefix("--root="){
                root = prefix.to_string();
                break;
            }
        }

        let base_path = env::current_dir().unwrap().join(root);
        let final_path = base_path.join(subdir.split('/').collect::<PathBuf>());*/
        
        let mut server = FileServer::new(roots);
        spawn_local_request_handler(
            request_receiver,
            server.connect(Box::new({
                let message_sender = message_sender.clone();
                let message_signal = message_signal.clone();
                move | notification | {
                    message_sender.send(FileClientMessage::Notification(notification)).unwrap();
                    message_signal.set();
                }
            })),
            message_signal.clone(),
            message_sender,
        );
        //spawn_connection_listener(TcpListener::bind("127.0.0.1:0").unwrap(), server);
        
        Self {
            request_sender,
            message_signal,
            message_receiver
        }
    }
    
    pub fn new_connect_remote(to_server: &str) -> Self {
        let (request_sender, request_receiver) = mpsc::channel();
        let message_signal = SignalToUI::new();
        let (message_sender, message_receiver) = mpsc::channel();
        
        let stream = TcpStream::connect(to_server).unwrap();
        spawn_request_sender(request_receiver, stream.try_clone().unwrap());
        spawn_response_or_notification_receiver(stream, message_signal.clone(), message_sender,);
        
        Self {
            request_sender,
            message_signal,
            message_receiver
        }
    }
    
}
/*
fn _spawn_connection_listener(listener: TcpListener, mut server: FileServer) {
    thread::spawn(move || {
        log!("Server listening on {}", listener.local_addr().unwrap());
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            log!("Incoming connection from {}", stream.peer_addr().unwrap());
            let (action_sender, action_receiver) = mpsc::channel();
            let _connection = server.connect(Box::new({
                let action_sender = action_sender.clone();
                move | notification | {
                    action_sender.send(FileClientAction::Notification(notification)).unwrap();
                }
            }));
            spawn_remote_request_handler(
                connection,
                stream.try_clone().unwrap(),
                action_sender,
            );
            spawn_response_or_notification_sender(action_receiver, stream);
        }
    });
}*/

fn _spawn_remote_request_handler(
    connection: FileServerConnection,
    mut stream: TcpStream,
    message_sender: Sender<FileClientMessage>,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 4];
        stream.read_exact(&mut len_bytes).unwrap();
        let len = u32::from_be_bytes(len_bytes);
        let mut request_bytes = vec![0; len as usize];
        stream.read_exact(&mut request_bytes).unwrap();
        
        let request = DeBin::deserialize_bin(request_bytes.as_slice()).unwrap();
        let response = connection.handle_request(request);
        message_sender.send(FileClientMessage::Response(response)).unwrap();
    });
}

fn _spawn_response_or_notification_sender(
    message_receiver: Receiver<FileClientMessage>,
    mut stream: TcpStream,
) {
    thread::spawn(move || loop {
        let message = message_receiver.recv().unwrap();
        let mut message_bytes = Vec::new();
        
        message.ser_bin(&mut message_bytes);
        
        let len_bytes = message_bytes.len().to_be_bytes();
        stream.write_all(&len_bytes).unwrap();
        stream.write_all(&message_bytes).unwrap();
    });
}

fn spawn_request_sender(request_receiver: Receiver<FileRequest>, mut stream: TcpStream) {
    thread::spawn(move || loop {
        let request = request_receiver.recv().unwrap();
        let mut request_bytes = Vec::new();
        request.ser_bin(&mut request_bytes);
        let len_bytes = request_bytes.len().to_be_bytes();
        stream.write_all(&len_bytes).unwrap();
        stream.write_all(&request_bytes).unwrap();
    });
}

fn spawn_response_or_notification_receiver(
    mut stream: TcpStream,
    message_signal: SignalToUI,
    message_sender: Sender<FileClientMessage>,
) {
    thread::spawn(move || loop {
        let mut len_bytes = [0; 4];
        stream.read_exact(&mut len_bytes).unwrap();
        
        let len = u32::from_be_bytes(len_bytes);
        let mut action_bytes = vec![0; len as usize];
        stream.read_exact(&mut action_bytes).unwrap();
        let action = DeBin::deserialize_bin(action_bytes.as_slice()).unwrap();
        message_sender.send(action).unwrap();
        message_signal.set()
    });
}

fn spawn_local_request_handler(
    request_receiver: Receiver<FileRequest>,
    connection: FileServerConnection,
    action_signal: SignalToUI,
    action_sender: Sender<FileClientMessage>,
) {
    thread::spawn(move || loop {
        if let Ok(request) = request_receiver.recv(){
            let response = connection.handle_request(request);
            action_sender.send(FileClientMessage::Response(response)).unwrap();
            action_signal.set()
        }
    });
}