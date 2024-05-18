
use crate::LiveId;
use crate::event::HttpRequest;
use crate::event::{NetworkResponseItem,NetworkResponse, HttpResponse};
use std::sync::mpsc::{Sender};
use std::net::TcpStream;
use std::io::{Read};
use makepad_http::utils::write_bytes_to_tcp_stream_no_error;

pub struct WindowsHttpSocket{
    //sender: Option<Sender<Vec<u8>>>
}

impl WindowsHttpSocket{
    /*pub fn send_message(&mut self, message:WebSocketMessage)->Result<(),()>{
        // lets encode the message into a membuffer and send it to the write thread
        if let Some(sender) = &mut self.sender{
            if sender.send(message).is_err(){
                return Err(());
            }
            return Ok(())
        }
        Err(())
    }*/
                        
    pub fn open(request_id:LiveId, request: HttpRequest, response_sender:Sender<NetworkResponseItem>){
        // parse the url
       
        
        let split = request.split_url();
        // strip off any hashes
        // alright we have proto, host, port and hash now
        // lets open a tcpstream
        let stream = TcpStream::connect(format!("{}:{}", split.host, split.port));
        // alright lets construct a http request
        // lets join the headers
                
        let mut http_header = format!("{} /{} HTTP/1.1\r\nHost: {}\r\n", request.method.to_string(), split.file, split.host);
        http_header.push_str(&request.get_headers_string());
        http_header.push_str("\r\n"); 
        println!("Sending headers #{}#", http_header);

        // lets push the entire body
        // lets write the http request
        if stream.is_err(){
            response_sender.send(NetworkResponseItem{
                request_id: request_id,
                response: NetworkResponse::HttpResponse(HttpResponse{
                    metadata_id: request.metadata_id,
                    status_code: 400,
                    headers: Default::default(),
                    body:Some(Vec::new())
                })
            }).ok();
            return
        }
        let mut stream = stream.unwrap();
        if write_bytes_to_tcp_stream_no_error(&mut stream, http_header.as_bytes()){
            response_sender.send(NetworkResponseItem{
                request_id: request_id,
                response: NetworkResponse::HttpResponse(HttpResponse{
                    metadata_id: request.metadata_id,
                    status_code: 400,
                    headers: Default::default(),
                    body:Some(Vec::new())
                })
            }).ok();
            return
        }
        println!("SENDING BODY {}", request.body.as_ref().unwrap().len());
        if let Some(body) = request.body{
            if write_bytes_to_tcp_stream_no_error(&mut stream, &body){
                response_sender.send(NetworkResponseItem{
                    request_id: request_id,
                    response: NetworkResponse::HttpResponse(HttpResponse{
                        metadata_id: request.metadata_id,
                        status_code: 400,
                        headers: Default::default(),
                        body:Some(Vec::new())
                    })
                }).ok();
                return
            }
        }
        
        let _reader_thread = std::thread::spawn(move || {
            let mut ret_buf = Vec::new();
            loop {
                let mut buffer = [0u8; 65535];
                match stream.read(&mut buffer) {
                    Ok(bytes_read) => {
                        if bytes_read == 0{
                            break;
                        }
                        ret_buf.extend_from_slice(&buffer[0..bytes_read]);
                    },
                    Err(_)=>{
                        break;
                    }
                }
            }
            // alright we have a ret_buf, now we need to split it at \r\n\r\n and return it
            println!("GOT RET {}", std::str::from_utf8(&ret_buf).unwrap_or(""));
        });
        
        /*
        // lets start the read thread
        let mut input_stream = stream.try_clone().unwrap();
        let mut output_stream = stream.try_clone().unwrap();
        let (sender, receiver) = channel();
        
        let _writer_thread = std::thread::spawn(move || {
        });
                        
        OsWebSocket{sender:Some(sender)}
        */
    }
}