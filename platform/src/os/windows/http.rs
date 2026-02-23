use crate::{
    event::{HttpRequest, NetworkResponseItem},
    makepad_live_id::LiveId,
    network_bridge::{from_network_response_item, to_network_http_request},
    thread::SignalToUI,
};
use std::sync::mpsc::Sender;

pub struct WindowsHttpSocket;

impl WindowsHttpSocket {
    pub fn open(
        request_id: LiveId,
        request: HttpRequest,
        response_sender: Sender<NetworkResponseItem>,
    ) {
        let request_id_u64 = request_id.0;
        let request = to_network_http_request(request);
        let (legacy_sender, legacy_receiver) = std::sync::mpsc::channel();
        makepad_network::backend::windows::http::WindowsHttpSocket::open(
            request_id_u64,
            request,
            legacy_sender,
        );

        std::thread::spawn(move || {
            while let Ok(item) = legacy_receiver.recv() {
                let terminal = matches!(
                    item.response,
                    makepad_network::NetworkResponse::HttpRequestError(_)
                        | makepad_network::NetworkResponse::HttpResponse(_)
                        | makepad_network::NetworkResponse::HttpStreamComplete(_)
                );
                let item = from_network_response_item(item);
                if response_sender.send(item).is_err() {
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
