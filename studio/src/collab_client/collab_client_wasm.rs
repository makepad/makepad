use {
    std::cell::RefCell,
    std::rc::Rc,
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        makepad_collab_protocol::{CollabRequest, CollabClientAction},
    },
    //std::{
    //    sync::mpsc::{Receiver, Sender},
    //},
};

live_design!{
    CollabClient= {{CollabClient}} {}
}

#[derive(Live)]
pub struct CollabClient {
    bind: Option<String>,
    path: String,
    #[rust] web_socket: Option<WebSocket>,
    #[rust] requests: Rc<RefCell<Vec<CollabRequest >> >,
    #[rust(LiveId::unique())] signal: Signal
}

impl LiveHook for CollabClient {
    fn after_apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.web_socket.is_none() {
            // connect websocket
            let (host, protocol) = if let OsType::WebBrowser{host,protocol,..} = &cx.platform_type(){(host,protocol)}else{panic!()};
            
            self.web_socket = Some(
                cx.web_socket_open(
                    format!("{}://{}",if protocol=="https:"{"wss"}else{"ws"}, host),
                    WebSocketAutoReconnect::Yes
                )
            )
            /*
            self.web_socket = Some(
                cx.web_socket_open(
                    format!("wss://makepad.nl/"),
                    WebSocketAutoReconnect::Yes
                )
            )*/

            //self.inner = Some(CollabClientInner::new_with_local_server(&self.path))
        }
    }
}

impl CollabClient {
    pub fn send_request(&mut self, request: CollabRequest) {
        self.requests.borrow_mut().push(request);
        Cx::post_signal(self.signal);
    }
    
    pub fn request_sender(&mut self) -> impl FnMut(CollabRequest) + '_ {
        let requests = self.requests.clone();
        let signal = self.signal;
        move | request | {
            requests.borrow_mut().push(request);
            Cx::post_signal(signal);
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<CollabClientAction> {
        let mut a = Vec::new();
        self.handle_event_with_fn(cx, event, &mut | _, v | a.push(v));
        a
    }
    
    pub fn handle_event_with_fn(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, CollabClientAction)) {
        
        match event {
            Event::WebSocketMessage(msg) if msg.web_socket == self.web_socket.unwrap() =>{
                let action = CollabClientAction::de_bin(&mut 0, &msg.data).unwrap();
                dispatch_action(cx, action);
            }
            
            Event::Signal(signal_event) if signal_event.signals.contains(&self.signal) => {
                let mut requests = self.requests.borrow_mut();
                for request in requests.iter(){
                    let mut buf = Vec::new();
                    request.ser_bin(&mut buf);
                    cx.web_socket_send(self.web_socket.unwrap(), buf);
                }
                requests.clear();
            }
            _ => {}
        }
    }
    
}
