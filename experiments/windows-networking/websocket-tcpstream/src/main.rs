use makepad_http::websocket::{  WebSocket, WebSocketMessage, PONG_MESSAGE};
use std::{
     convert::TryInto, io::{self, BufRead, BufReader, Read, Write}, net::TcpStream, sync::MutexGuard, time
};

use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use std::cell::UnsafeCell;
use std::error::Error;
use std::marker::Sync;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

struct UnsafeMutator<T> {
    value: UnsafeCell<T>,
}

impl<T> UnsafeMutator<T> {
    fn new(value: T) -> UnsafeMutator<T> {
        return UnsafeMutator {
            value: UnsafeCell::new(value),
        };
    }

    fn mut_value(&self) -> &mut T {
        return unsafe { &mut *self.value.get() };
    }
}

unsafe impl<T> Sync for UnsafeMutator<T> {}

struct ReadWrapper<R>
where
    R: Read,
{
    inner: Arc<UnsafeMutator<R>>,
}

impl<R: Read> Read for ReadWrapper<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        return self.inner.mut_value().read(buf);
    }
}
struct WriteWrapper<W>
where
    W: Write,
{
    inner: Arc<UnsafeMutator<W>>,
}

impl<W: Write> Write for WriteWrapper<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
       // println!("{}", String::from_utf8_lossy(&buf));
        return self.inner.mut_value().write(buf);
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        return self.inner.mut_value().flush();
    }
}

pub struct Socket {
    pub output_stream: Arc<Mutex<dyn Write + Send>>,
    pub input_stream: Arc<Mutex<dyn Read + Send>>,
}

fn read_line_one_byte(mut inp: MutexGuard<'_, dyn Read + Send>) -> io::Result<String> {
    let mut buffer = String::new();
    loop {
        let mut byte = [0];
        match inp.read_exact(&mut byte) {
            Ok(_) => {
                let ch = byte[0] as char;
                buffer.push(ch);
                if ch == '\n' {
                    break;
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                // If the read would block, we just retry.
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(buffer)
}

impl Socket {
    pub fn bind(host: &str, port: u16, secure: bool) -> Result<Socket, Box<dyn Error>> {
        let tcp_stream = match TcpStream::connect((host, port)) {
            Ok(x) => x,
            Err(e) => return Err(Box::new(e)),
        };
        if secure {
    //        let tls_connector = TlsConnector::builder().build().unwrap();
            let mut tls_connector_builder = SslConnector::builder(SslMethod::tls()).unwrap();
            tls_connector_builder.set_verify(SslVerifyMode::NONE); // Disable certificate verification
            let tls_connector = tls_connector_builder.build();
         
            let tls_stream = match tls_connector.connect(host, tcp_stream) {
                Ok(x) => x,
                Err(e) => return Err(Box::new(e)),
            };
            let mutator = Arc::new(UnsafeMutator::new(tls_stream));
            let input_stream = Arc::new(Mutex::new(ReadWrapper {
                inner: mutator.clone(),
            }));
            let output_stream = Arc::new(Mutex::new(WriteWrapper { inner: mutator }));

            let socket = Socket {
                output_stream,
                input_stream,
            };
            return Ok(socket);
        } else {
            let mutator = Arc::new(UnsafeMutator::new(tcp_stream));
            let input_stream = Arc::new(Mutex::new(ReadWrapper {
                inner: mutator.clone(),
            }));
            let output_stream = Arc::new(Mutex::new(WriteWrapper { inner: mutator }));

            let socket = Socket {
                output_stream,
                input_stream,
            };
            return Ok(socket);
        }
    }
}


fn main() {
    let socket = Arc::new(Socket::bind("localhost", 443, true).unwrap());
    let socket_clone = Arc::clone(&socket);

    let mut websocketupgrade = false;
    // perform a socket upgrade.
    {
        let mut out = socket_clone.output_stream.lock().unwrap();
        let mut inp = socket_clone.input_stream.lock().unwrap();

         let key = "SxJdXBRtW7Q4awLDhflO0Q=="; // WebSocket key (base64-encoded)
        let headers = format!("GET / HTTP/1.1\r\nHost: example.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {}\r\n\r\n",key);
        out.write_all(headers.as_bytes()).unwrap();



       
        let mut response = String::new();
        let mut done = false;
        let mut reader = BufReader::new(&mut *inp);
        while !done
        {
            response.clear();
             reader.read_line(&mut response).unwrap();      
            println!("{} {}",response,  response.len());

            if response.len() == 2 {
                done = true;
            }
            else {
                if response.contains("HTTP/1.1 101 Switching Protocols") 
                {
                    println!("WebSocket upgrade succeeded!");
                    websocketupgrade = true;
                }
    
            }
        }

    }
    let reader_thread = thread::spawn(move || {
        let mut input_stream = socket_clone.input_stream.lock().unwrap();
        let mut output_stream = socket_clone.output_stream.lock().unwrap();

        let mut done = false;
        let mut web_socket = WebSocket::new();
        while !done
        {
            let mut buffer: [u8; 1024] = [0; 1024];
                        match input_stream.read(&mut buffer) {
                            Ok(bytes_read) => {

                                web_socket.parse(&buffer[0..bytes_read], | result | {
                                    match result {
                                        Ok(WebSocketMessage::Ping(_)) => {
                                            println!("ping!");
                                            output_stream.write_all(&PONG_MESSAGE);
                                        },
                                        Ok(WebSocketMessage::Pong(_)) => {
                                        },
                                        Ok(WebSocketMessage::Text(text)) => {
                                            println!("text => {}", text);
                                        },
                                        Ok(WebSocketMessage::Binary(_data)) => {
                                            println!("binary!");
                                            
                                        },
                                        Ok(WebSocketMessage::Close) => {
                                        
                                            println!("socket close received");
                                            done = true;
                                        },
                                        Err(e) => {
                                            eprintln!("Websocket error {:?}", e);
                                        }
                                    }});                            
                            }
                            Err(e) => {
                                eprintln!("Failed to receive data: {}", e);
                                
                            }
            }
        
    }



     
    });


    
    let socket_clone2 = Arc::clone(&socket);

    let mut output_stream = socket_clone2.output_stream.lock().unwrap();
   
    let msg = WebSocketMessage::Text("I'm a text message!");
    let frame = WebSocket::message_to_frame(msg);
    let frame = 
    output_stream.write_all(&frame);

    let ten_millis = time::Duration::from_millis(5000);
    
    thread::sleep(ten_millis);

    reader_thread.join().unwrap();
}