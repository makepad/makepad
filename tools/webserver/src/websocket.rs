use std::convert::TryInto;
 
#[derive(Debug)]
pub enum WebSocketState {
    Opcode,
    Len1,
    Len2,
    Len8,
    Ping,
    Pong,
    Data,
    Mask
}

impl WebSocketState{
    fn expected(&self)->usize{
        match self{
            WebSocketState::Opcode=>1,
            WebSocketState::Len1=>1,
            WebSocketState::Len2=>2,
            WebSocketState::Len8=>8,
            WebSocketState::Ping=>1,
            WebSocketState::Pong=>1,
            WebSocketState::Data=>0,
            WebSocketState::Mask=>4
        }
    }
}

#[allow(dead_code)]
pub struct WebSocket {
    head: [u8; 8],
    head_expected: usize,
    head_written: usize,
    data: Vec<u8>,
    data_len: usize,
    input_read: usize,
    mask_counter: usize,
    is_partial: bool,
    is_binary: bool,
    is_masked: bool,
    last_opcode: usize,
    state: WebSocketState
}

pub enum WebSocketParseResult {
    Continue,
    Error(String),
    Close
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
            last_opcode: 0,
            is_masked: false,
            is_partial: false,
            is_binary: false,
            state: WebSocketState::Opcode
        }
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
    
    fn to_state(&mut self, state:WebSocketState){
        self.head_written = 0;
        self.head_expected = state.expected();
        self.state = state;
    }
    
    pub fn parse(&mut self, input: &[u8]) -> WebSocketParseResult {
        self.input_read = 0;
        // parse a header
        loop {
            match self.state {
                WebSocketState::Opcode => {
                    if self.parse_head(input) {
                        break;
                    }
                    let frame = self.head[0] & 128;
                    let opcode = self.head[0] & 15;
                    if opcode <= 2 {
                        self.is_partial = frame != 0;
                        self.is_binary = opcode == 2 || opcode == 0 && self.last_opcode == 2;
                        self.to_state(WebSocketState::Len1);
                        if opcode != 0 {
                            self.last_opcode = 2;
                        }
                    }
                    else if opcode == 8 {
                        return WebSocketParseResult::Close;
                    }
                    else if opcode == 9 {
                        self.to_state(WebSocketState::Ping);
                    }
                    else if opcode == 10 {
                        self.to_state(WebSocketState::Pong);
                    }
                    else {
                        return WebSocketParseResult::Error(format!("Opcode not supported {}", opcode))
                    }
                },
                WebSocketState::Len1 => {
                    if self.parse_head(input) {
                        break;
                    }
                    self.is_masked = (self.head[0] & 128) > 0;
                    let len_type = self.head[0] & 127;
                    if len_type < 126 {
                        self.data_len = len_type as usize;
                        if !self.is_masked {
                            self.to_state(WebSocketState::Data);
                        }
                        else {
                            self.to_state(WebSocketState::Mask);
                        }
                    }
                    else if len_type == 126 {
                        self.to_state(WebSocketState::Len1);
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
                    if self.is_masked{
                        self.to_state(WebSocketState::Mask);
                    }
                    else{
                        self.to_state(WebSocketState::Data);
                    }
                },
                WebSocketState::Len8=>{
                    if self.parse_head(input) {
                        break;
                    }
                    self.data_len = u32::from_be_bytes(
                        self.head[0..4].try_into().unwrap()
                    ) as usize;
                    if self.is_masked{
                        self.to_state(WebSocketState::Mask);
                    }
                    else{
                        self.to_state(WebSocketState::Data);
                    }
                },
                WebSocketState::Mask=>{
                    if self.parse_head(input) {
                        break;
                    }
                    if self.data_len == 0{
                        self.to_state(WebSocketState::Opcode);
                    }
                    else{
                        self.mask_counter = 0;
                        self.data.truncate(0);
                        self.to_state(WebSocketState::Data);
                    }
                },
                WebSocketState::Data=>{
                    if self.is_masked{
                        while self.data.len() < self.data_len && self.input_read < input.len(){
                            self.data.push(input[self.input_read] ^ self.head[self.mask_counter]);
                            self.mask_counter = (self.mask_counter + 1)&3;
                            self.input_read += 1;
                        }
                    }
                    else{
                        while self.data.len() < self.data_len && self.input_read < input.len(){
                            self.data.push(input[self.input_read]);
                            self.input_read += 1;
                        }
                    }
                    if self.data.len() < self.data_len{ // not enough data yet
                        break;
                    }
                    else{
                        if self.is_binary{
                            eprintln!("implement partial binary")
                        }
                        else{
                            let s = std::str::from_utf8(&self.data);
                            println!("GOT DATA #{}#", s.unwrap());
                        }
                        self.to_state(WebSocketState::Opcode);
                    }
                },
                _ => ()
            }
        }
        return WebSocketParseResult::Continue;
    }
    
}

