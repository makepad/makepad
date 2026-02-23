use crate::event::HttpRequest;
use crate::network_bridge::{
    from_network_ws_message, to_network_http_request, to_network_ws_message,
};
use crate::web_socket::WebSocketMessage;
use std::sync::mpsc::Sender;

pub struct OsWebSocket {
    inner: makepad_network::backend::linux::web_socket::OsWebSocket,
}

impl OsWebSocket {
    pub fn send_message(&mut self, message: WebSocketMessage) -> Result<(), ()> {
        self.inner.send_message(to_network_ws_message(message))
    }

    pub fn close(&mut self) {
        self.inner.close();
    }

    pub fn open(
        socket_id: u64,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> OsWebSocket {
        let (inner_sender, inner_receiver) = std::sync::mpsc::channel();
        let inner = makepad_network::backend::linux::web_socket::OsWebSocket::open(
            socket_id,
            to_network_http_request(request),
            inner_sender,
        );

        std::thread::spawn(move || {
            while let Ok(message) = inner_receiver.recv() {
                if rx_sender.send(from_network_ws_message(message)).is_err() {
                    break;
                }
            }
        });

        OsWebSocket { inner }
    }
}
