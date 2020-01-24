use std::net::{TcpStream, SocketAddr};
// live value connection client
const LIVE_SERVER_DEFAULT_PORT:u16 = 45823;

pub struct LiveClient{
}

struct LiveError{
    msg:String
}

type LiveResult<T> = Result<T, LiveError>;

impl LiveClient{
    pub fn connect_to_live_server(server_address: Option<SocketAddr>) -> Option<LiveClient> {
        // first try local address
        let addr = if let Some(addr) = server_address{
            addr
        }
        else{
            SocketAddr::from(([127, 0, 0, 1], LIVE_SERVER_DEFAULT_PORT))
        };
        let mut tcp_stream = if let Ok(stream) = TcpStream::connect(addr) {
            stream
        }
        else {
            return None
        };
        None
        
    }
}