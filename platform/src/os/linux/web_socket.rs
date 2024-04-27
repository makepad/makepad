use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;

use makepad_http::{
    client::HttpClient,
    digest::base64_encode,
    server::{ServerWebSocket, ServerWebSocketMessage, ServerWebSocketMessageHeader},
    utils::parse_headers,
};
use makepad_net::tcp::Socket;
use makepad_url::Url;


const MASKING_KEY_SIZE: usize = 4;
type MaskingKey = [u8; MASKING_KEY_SIZE];

fn mask(masking_key: &MaskingKey, payload: &[u8]) -> Vec<u8> {
    payload
        .iter()
        .zip(masking_key.iter().cycle())
        .map(|(byte, key)| byte ^ key)
        .collect::<Vec<u8>>()
}

fn handshake(socket: Arc<Socket>, url: Url) {
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let key: Vec<u8> = (0..16).map(|_| ServerWebSocketMessageHeader::random_byte()).collect();
    headers.insert("connection".to_string(), vec!["keep-alive, upgrade".to_string()]);
    headers.insert("upgrade".to_string(), vec!["websocket".to_string()]);
    headers.insert("sec-websocket-version".to_string(), vec!["13".to_string()]);
    headers.insert("sec-fetch-mode".to_string(), vec!["websocket".to_string()]);
    headers.insert("sec-websocket-key".to_string(), vec![base64_encode(key.as_slice())]);
    HttpClient::new(url)
        .set_socket(socket)
        .set_headers(headers)
        .get(move |_, response| {
            //let mut ws = ws.borrow_mut();
            let response_headers = parse_headers(response.header.clone());
            response_headers.get("sec-websocket-accept").unwrap();
        }).join().unwrap();
}

pub struct WebSocketClient {
    pub socket: Arc<Socket>,
    masking_key: [u8; 4]
}

impl WebSocketClient {
    pub fn new(url: Url) -> Self {
        //let url = url.clone();
        let port = url.port.unwrap();
        let host = &url.host;
        let mut masking_key: MaskingKey = [0; MASKING_KEY_SIZE];
        for i in 0..MASKING_KEY_SIZE {
            masking_key[i] = ServerWebSocketMessageHeader::random_byte();
        }
        let socket = Arc::new(Socket::bind(host, port, url.secure).unwrap());
        handshake(socket.clone(), url);
        Self {
            socket,
            masking_key
        }
    }

    pub fn incoming_messages<F, H>(&mut self, mut on_message: F)
        where
            F: FnMut(Arc<Mutex<dyn Write + Send>>) -> H,
            H: FnMut(ServerWebSocketMessage) + Send + 'static
    {
        let masking_key = self.masking_key;
        let input_stream = self.socket.input_stream.clone();
        let output_stream = self.socket.output_stream.clone();
        let mut callback = on_message(output_stream.clone());
        thread::spawn(move || {
            loop {
                let mut input_stream = input_stream.lock().unwrap();
                let mut buffer: Vec<u8> = Vec::new();
                let mut buf1 = vec![0; 2];
                input_stream.read_exact(&mut buf1).unwrap();
                buffer.append(&mut buf1);
                let len1 = buffer[1] & 0x7F;
                let payload_size = match len1 {
                    126 => {
                        let mut buf2 = [0u8; 2];
                        input_stream.read_exact(&mut buf2).unwrap();
                        buffer.append(&mut buf2.to_vec());
                        let len2 = u16::from_be_bytes(buffer[2..2].try_into().unwrap());
                        (len1 as u16 + len2) as usize
                    }
                    127 => {
                        let mut buf8 = [0u8; 8];
                        input_stream.read_exact(&mut buf8).unwrap();
                        buffer.append(&mut buf8.to_vec());
                        let len8 = u64::from_be_bytes(buffer[2..2+8].try_into().unwrap());
                        (len1 as u64 + len8) as usize
                    }
                    _ => {
                        len1 as usize
                    }
                };
                let mut buffer_payload = vec![0; payload_size];
                input_stream.read_exact(&mut buffer_payload).unwrap();
                buffer.append(&mut buffer_payload);
                let mut wss = ServerWebSocket::new();
                let out = output_stream.clone();
                wss.parse(buffer.as_slice(), |result| {
                    let message = result.unwrap();
                    match message {
                        ServerWebSocketMessage::Ping(ping) => {
                            let mut pong_frame: Vec<u8> = Vec::new();
                            pong_frame.append(&mut [0x8a, 0x80 | ping.len() as u8].to_vec());
                            pong_frame.append(&mut masking_key.to_vec());
                            pong_frame.append(&mut mask(&masking_key, ping));
                            let mut output_stream = out.lock().unwrap();
                            output_stream.write_all(pong_frame.as_slice()).unwrap();
                        }
                        _ => callback(message)
                    };
                });
            }
        });
    }
}