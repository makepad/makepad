use {
    crate::{
//        makepad_micro_serde::*,
        makepad_platform::*,
        makepad_collab_protocol::{CollabRequest, CollabClientAction},
    },
    std::{
        sync::mpsc::{Receiver, Sender},
    },
};

live_register!{
    CollabClient: {{CollabClient}} {}
}

#[derive(Live)]
pub struct CollabClient {
    bind: Option<String>,
    path: String,
    #[rust] inner: Option<CollabClientInner>
}

impl LiveHook for CollabClient {
    fn after_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if self.inner.is_none() {
            //self.inner = Some(CollabClientInner::new_with_local_server(&self.path))
        }
    }
}

pub struct CollabClientInner {
    pub request_sender: Sender<CollabRequest>,
    pub action_signal: Signal,
    pub action_receiver: Receiver<CollabClientAction>,
}

impl CollabClient {
    pub fn send_request(&mut self, _request: CollabRequest) {
        //self.inner.as_ref().unwrap().request_sender.send(request).unwrap();
    }
    
    pub fn request_sender(&mut self) -> impl FnMut(CollabRequest) + '_ {
        //let request_sender = &self.inner.as_ref().unwrap().request_sender;
        move | _request | ()// request_sender.send(request).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) -> Vec<CollabClientAction> {
        let mut a = Vec::new();
        self.handle_event_with_fn(cx, event, &mut | _, v | a.push(v));
        a
    }
    
    pub fn handle_event_with_fn(&mut self, _cx: &mut Cx, _event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, CollabClientAction)) {
        /*
        let inner = self.inner.as_ref().unwrap();
        match event {
            Event::Signal(event)
            if event.signals.contains(&inner.action_signal) => {
                loop {
                    match inner.action_receiver.try_recv() {
                        Ok(action) => dispatch_action(cx, action),
                        Err(TryRecvError::Empty) => break,
                        _ => panic!(),
                    }
                }
            }
            _ => {}
        }*/
    }
    
}

impl CollabClientInner {
}
