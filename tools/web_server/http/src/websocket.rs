use std::convert::TryInto;
use crate::digest::{Sha1, base64_encode};

#[derive(Debug, PartialEq)]
enum State {
    Opcode,
    Len1,
    Len2,
    Len8,
    Data,
    Mask
}

impl State {
    fn head_expected(&self) -> usize {
        match self {
            State::Opcode => 1,
            State::Len1 => 1,
            State::Len2 => 2,
            State::Len8 => 8,
            State::Data => 0,
            State::Mask => 4
        }
    }
}

pub struct WebSocket {
    head: [u8; 8],
    head_expected: usize,
    head_written: usize,
    data: Vec<u8>,
    data_len: usize,
    input_read: usize,
    mask_counter: usize,
    is_ping: bool,
    is_pong: bool,
    is_partial: bool,
    is_text: bool,
    is_masked: bool,
    state: State
}

pub enum WebSocketMessage<'a> {
    Ping(&'a [u8]),
    Pong(&'a [u8]),
    Text(&'a str),
    Binary(&'a [u8]),
    Close
}

#[derive(Debug)]
pub enum WebSocketError<'a> {
    OpcodeNotSupported(u8),
    TextNotUTF8(&'a [u8]),
}

pub const PING_MESSAGE:[u8;2] = [128 | 9,0];
pub const PONG_MESSAGE:[u8;2] = [128 | 10,0];

pub struct BinaryMessageHeader{
    len: usize,
    data:[u8;10]
}

impl BinaryMessageHeader{
    pub fn from_len(len:usize)->Self{
        let mut data = [0u8;10];
        
        data[0] = 128 | 2; // binary single message
        
        if len < 126{
            data[1] = len as u8;
            return BinaryMessageHeader{len:2, data}
        }
        else if len < 65536{
            data[1] = 126; 
            let bytes = &(len as u16).to_be_bytes();
            for i in 0..bytes.len(){
                data[i+2] = bytes[i]
            }
            return BinaryMessageHeader{len:4, data}
        }
        else{
            data[1] = 127;
            let bytes = &(len as u64).to_be_bytes();
            for i in 0..bytes.len(){
                data[i+2] = bytes[i]
            }
            return BinaryMessageHeader{len:10, data}
        }
    }
    
    pub fn as_slice(&self)->&[u8]{
        &self.data[0..self.len]
    }
}

impl WebSocket {
    
    pub fn new() -> Self {
        Self {
            head: [0u8; 8],
            head_expected: 1,
            head_written: 0,
            data: Vec::new(),
            data_len: 0,
            input_read: 0,
            mask_counter: 0,
            is_ping: false,
            is_pong: false,
            is_masked: false,
            is_partial: false,
            is_text: false,
            state: State::Opcode
        }
    }
    
    pub fn create_upgrade_response(key: &str) -> String {
        let to_hash = format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", key);
        let mut sha1 = Sha1::new();
        sha1.update(to_hash.as_bytes());
        let out_bytes = sha1.finalise();
        let base64 = base64_encode(&out_bytes);
        let response_ack = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            base64
        );
        response_ack
    }
    
    fn parse_head(&mut self, input: &[u8]) -> bool {
        while self.head_expected > 0
            && self.input_read < input.len()
            && self.head_written < self.head.len()
        {
            self.head[self.head_written] = input[self.input_read];
            self.input_read += 1;
            self.head_written += 1;
            self.head_expected -= 1;
        }
        return self.head_expected != 0
    }
    
    fn to_state(&mut self, state: State) {
        match state {
            State::Data => {
                self.mask_counter = 0;
                self.data.clear();
            }
            State::Opcode => {
                self.is_ping = false;
                self.is_pong = false;
                self.is_partial = false;
                self.is_text = false;
                self.is_masked = false;
            },
            _ => ()
        }
        self.head_written = 0;
        self.head_expected = state.head_expected();
        self.state = state;
    }
    
    pub fn parse<F>(&mut self, input: &[u8], mut result: F) where F: FnMut(Result<WebSocketMessage, WebSocketError>){
        self.input_read = 0;
        // parse a header
        loop {
            match self.state {
                State::Opcode => {
                    if self.parse_head(input) {
                        break;
                    }
                    let opcode = self.head[0] & 15;
                    if opcode <= 2 {
                        self.is_partial = (self.head[0] & 128) != 0;
                        self.is_text = opcode == 1;
                        self.to_state(State::Len1);
                    }
                    else if opcode == 8 {
                        result(Ok(WebSocketMessage::Close));
                        break;
                    }
                    else if opcode == 9 {
                        self.is_ping = true;
                        self.to_state(State::Len1);
                    }
                    else if opcode == 10 {
                        self.is_pong = true;
                        self.to_state(State::Len1);
                    }
                    else {
                        result(Err(WebSocketError::OpcodeNotSupported(opcode)));
                        break;
                    }
                },
                State::Len1 => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.is_masked = (self.head[0] & 128) > 0;
                    let len_type = self.head[0] & 127;
                    if len_type < 126 {
                        self.data_len = len_type as usize;
                        if !self.is_masked {
                            self.to_state(State::Data);
                        }
                        else {
                            self.to_state(State::Mask);
                        }
                    }
                    else if len_type == 126 {
                        self.to_state(State::Len2);
                    }
                    else if len_type == 127 {
                        self.to_state(State::Len8);
                    }
                },
                State::Len2 => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.data_len = u16::from_be_bytes(
                        self.head[0..2].try_into().unwrap()
                    ) as usize;
                    if self.is_masked {
                        self.to_state(State::Mask);
                    }
                    else {
                        self.to_state(State::Data);
                    }
                },
                State::Len8 => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.data_len = u64::from_be_bytes(
                        self.head[0..8].try_into().unwrap()
                    ) as usize;
                    if self.is_masked {
                        self.to_state(State::Mask);
                    }
                    else {
                        self.to_state(State::Data);
                    }
                },
                State::Mask => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.to_state(State::Data);
                },
                State::Data => {
                    if self.is_masked {
                        while self.data.len() < self.data_len && self.input_read < input.len() {
                            self.data.push(input[self.input_read] ^ self.head[self.mask_counter]);
                            self.mask_counter = (self.mask_counter + 1) & 3;
                            self.input_read += 1;
                        }
                    }
                    else {
                        while self.data.len() < self.data_len && self.input_read < input.len() {
                            self.data.push(input[self.input_read]);
                            self.input_read += 1;
                        }
                    }
                    if self.data.len() < self.data_len { // not enough data yet
                        break;
                    }
                    else {
                        if self.is_ping {
                            result(Ok(WebSocketMessage::Ping(&self.data)));
                        }
                        else if self.is_pong {
                            result(Ok(WebSocketMessage::Pong(&self.data)));
                        }
                        else {
                            if self.is_text{
                                if let Ok(text) = std::str::from_utf8(&self.data){
                                    result(Ok(WebSocketMessage::Text(text)));
                                }
                                else{
                                    result(Err(WebSocketError::TextNotUTF8(&self.data)))
                                }
                            }
                            else{
                                result(Ok(WebSocketMessage::Binary(&self.data)));
                            }
                        }
                        
                        self.to_state(State::Opcode);
                    }
                },
            }
        }
    }
    
}

