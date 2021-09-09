use std::convert::TryInto;
use crate::digest::{Sha1, base64_encode};

#[derive(Debug, PartialEq)]
pub enum WebSocketState {
    Opcode,
    Len1,
    Len2,
    Len8,
    Data,
    Mask
}

impl WebSocketState {
    fn head_expected(&self) -> usize {
        match self {
            WebSocketState::Opcode => 1,
            WebSocketState::Len1 => 1,
            WebSocketState::Len2 => 2,
            WebSocketState::Len8 => 8,
            WebSocketState::Data => 0,
            WebSocketState::Mask => 4
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
    is_masked: bool,
    state: WebSocketState
}

pub enum WebSocketResult {
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Data(Vec<u8>),
    Error(String),
    Close
}

pub struct WebSocketMessage{
    pub check_len: usize,
    pub data:Vec<u8>
}

impl WebSocketMessage{
    pub fn new_binary(len: usize)->WebSocketMessage{
        let mut data = Vec::new();
        let check_len;
        data.push(128 | 2); // binary single message
        if len < 126{
            data.push(len as u8);
            check_len = len + 2;
        }
        else if len < 65536{
            data.push(126); 
            data.extend_from_slice(&(len as u16).to_be_bytes());
            check_len = len + 4;
        }
        else{
            data.push(127);
            data.extend_from_slice(&(len as u64).to_be_bytes());
            check_len = len + 10;
        }
        WebSocketMessage{data, check_len}
    }
    
    pub fn append(&mut self, data:&[u8]){
        self.data.extend_from_slice(data);
    }
    
    pub fn take(self)->Vec<u8>{
        if self.check_len != self.data.len(){
            panic!();
        }
        self.data
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
            state: WebSocketState::Opcode
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
    
    fn to_state(&mut self, state: WebSocketState) {
        match state {
            WebSocketState::Data => {
                self.mask_counter = 0;
                self.data.truncate(0);
            }
            WebSocketState::Opcode => {
                self.is_ping = false;
                self.is_pong = false;
                self.is_partial = false;
                self.is_masked = false;
            },
            _ => ()
        }
        self.head_written = 0;
        self.head_expected = state.head_expected();
        self.state = state;
    }
    
    pub fn parse(&mut self, input: &[u8]) -> Vec<WebSocketResult> {
        self.input_read = 0;
        let mut results = Vec::new();
        // parse a header
        loop {
            match self.state {
                WebSocketState::Opcode => {
                    if self.parse_head(input) {
                        break;
                    }
                    let opcode = self.head[0] & 15;
                    if opcode <= 2 {
                        self.is_partial = (self.head[0] & 128) != 0;
                        self.to_state(WebSocketState::Len1);
                    }
                    else if opcode == 8 {
                        results.push(WebSocketResult::Close);
                        break;
                    }
                    else if opcode == 9 {
                        self.is_ping = true;
                        self.to_state(WebSocketState::Len1);
                    }
                    else if opcode == 10 {
                        self.is_pong = true;
                        self.to_state(WebSocketState::Len1);
                    }
                    else {
                        results.push(WebSocketResult::Error(format!("Opcode not supported {}", opcode)));
                        break;
                    }
                },
                WebSocketState::Len1 => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.is_masked = (self.head[0] & 128) > 0;
                    let len_type = self.head[0] & 127;
                    if len_type < 126 {
                        if len_type == 0 {
                            // emit a size 0 datapacket
                            results.push(WebSocketResult::Data(Vec::new()));
                            self.to_state(WebSocketState::Opcode)
                        }
                        else {
                            self.data_len = len_type as usize;
                            if !self.is_masked {
                                self.to_state(WebSocketState::Data);
                            }
                            else {
                                self.to_state(WebSocketState::Mask);
                            }
                        }
                    }
                    else if len_type == 126 {
                        self.to_state(WebSocketState::Len2);
                    }
                    else if len_type == 127 {
                        self.to_state(WebSocketState::Len8);
                    }
                },
                WebSocketState::Len2 => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.data_len = u16::from_be_bytes(
                        self.head[0..2].try_into().unwrap()
                    ) as usize;
                    if self.is_masked {
                        self.to_state(WebSocketState::Mask);
                    }
                    else {
                        self.to_state(WebSocketState::Data);
                    }
                },
                WebSocketState::Len8 => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.data_len = u64::from_be_bytes(
                        self.head[0..8].try_into().unwrap()
                    ) as usize;
                    if self.is_masked {
                        self.to_state(WebSocketState::Mask);
                    }
                    else {
                        self.to_state(WebSocketState::Data);
                    }
                },
                WebSocketState::Mask => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.to_state(WebSocketState::Data);
                },
                WebSocketState::Data => {
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
                            results.push(WebSocketResult::Ping(self.data.clone()))
                        }
                        else if self.is_pong {
                            results.push(WebSocketResult::Pong(self.data.clone()))
                        }
                        else {
                            results.push(WebSocketResult::Data(self.data.clone()));
                        }
                        
                        self.to_state(WebSocketState::Opcode);
                    }
                },
            }
        }
        return results;
    }
    
}

