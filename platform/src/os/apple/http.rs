use crate::{
    event::{HttpRequest, NetworkResponseItem},
    makepad_live_id::LiveId,
    network_bridge::{
        from_network_response_item, from_network_ws_message, to_network_http_request,
        to_network_response_item, to_network_ws_message,
    },
    thread::SignalToUI,
    web_socket::WebSocketMessage,
};
use std::sync::mpsc::Sender;

pub use makepad_network::backend::apple::url_session::{
    define_url_session_data_delegate, define_url_session_delegate, define_web_socket_delegate,
};

pub struct OsWebSocket {
    inner: makepad_network::backend::apple::url_session::OsWebSocket,
}

impl OsWebSocket {
    pub fn send_message(&mut self, message: WebSocketMessage) -> Result<(), ()> {
        self.inner.send_message(to_network_ws_message(message))
    }

    pub fn close(&self) {
        self.inner.close();
    }

    pub fn open(
        socket_id: u64,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> OsWebSocket {
        let (inner_sender, inner_receiver) = std::sync::mpsc::channel();
        let inner = makepad_network::backend::apple::url_session::OsWebSocket::open(
            socket_id,
            to_network_http_request(request),
            inner_sender,
        );

        std::thread::spawn(move || {
            while let Ok(message) = inner_receiver.recv() {
                if rx_sender.send(from_network_ws_message(message)).is_err() {
                    break;
                }
                SignalToUI::set_ui_signal();
            }
        });

        OsWebSocket { inner }
    }
}

#[derive(Default)]
pub struct AppleHttpRequests {
    inner: makepad_network::backend::apple::url_session::AppleHttpRequests,
}

impl AppleHttpRequests {
    pub fn cancel_http_request(&mut self, request_id: LiveId) {
        self.inner.cancel_http_request(request_id.0);
    }

    pub fn handle_response_item(&mut self, item: &NetworkResponseItem) {
        self.inner
            .handle_response_item(&to_network_response_item(item.clone()));
    }

    pub fn make_http_request(
        &mut self,
        request_id: LiveId,
        request: HttpRequest,
        networking_sender: Sender<NetworkResponseItem>,
    ) {
        let request_id_u64 = request_id.0;
        let request = to_network_http_request(request);
        let (inner_sender, inner_receiver) = std::sync::mpsc::channel();
        self.inner
            .make_http_request(request_id_u64, request, inner_sender);

        std::thread::spawn(move || {
            while let Ok(item) = inner_receiver.recv() {
                let terminal = matches!(
                    item.response,
                    makepad_network::NetworkResponse::HttpRequestError(_)
                        | makepad_network::NetworkResponse::HttpResponse(_)
                        | makepad_network::NetworkResponse::HttpStreamComplete(_)
                );
                let item = from_network_response_item(item);
                if networking_sender.send(item).is_err() {
                    break;
                }
                SignalToUI::set_ui_signal();
                if terminal {
                    break;
                }
            }
        });
    }
}
