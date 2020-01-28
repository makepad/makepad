use crate::cx::*;
use std::collections::HashMap;
use std::net::{TcpStream, SocketAddr};
// live value connection client
const LIVE_SERVER_DEFAULT_PORT:u16 = 45823;

pub struct LiveClient{
    pub colors: HashMap<String, HashMap<(usize,usize), Color>>
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
        let mut _tcp_stream = if let Ok(stream) = TcpStream::connect(addr) {
            stream
        }
        else {
            return None
        };
        None
    }
}

impl Cx{
    pub fn pick(&self, file:&str, line:usize, col:usize, inp:Color)->Color{
        if let Some(lc) = &self.live_client{
            if let Some(colors) = lc.colors.get(file){
                if let Some(color) = colors.get(&(line, col)){
                    return *color
                }
            }
        }
        inp
    }
}

#[macro_export]
macro_rules!pick {
    ( $ cx: ident, $col: literal) => {
        $cx.pick(file!(), line!(), column!(), color($col))
    }
}

