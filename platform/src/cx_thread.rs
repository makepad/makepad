use {
    std::sync::mpsc::{channel, Sender, Receiver,TryRecvError, SendError},
    crate::{
        cx::Cx,
        cx_api::*,
        event::{Signal, SignalEvent}
    }
};

pub struct UIReceiver<T>{
    sender:Sender<T>,
    receiver:Receiver<T>,
    signal:Signal
}

#[derive(Clone)]
pub struct UISender<T>{
    sender:Sender<T>,
    signal:Signal
}

unsafe impl<T> Send for UISender<T>{}

impl<T> UIReceiver<T>{
    pub fn new(cx:&mut Cx)->Self{
        let (sender, receiver) = channel();
        Self{
            sender,
            receiver,
            signal:cx.new_signal()
        }
    }
    
    pub fn sender(&self)->UISender<T>{
        UISender{
            sender:self.sender.clone(),
            signal:self.signal.clone()
        }
    }
    
    pub fn try_recv(&self, signal_event:&SignalEvent)->Result<T,TryRecvError>{
        if signal_event.signals.get(&self.signal).is_some(){
            self.receiver.try_recv()
        }
        else{
            Err(TryRecvError::Empty)
        }
    }
}

impl<T> UISender<T>{
    pub fn send(&self, t:T)->Result<(), SendError<T>>{
        let res = self.sender.send(t);
        Cx::post_signal(self.signal, 0);
        res
    }
}

