use {
    std::sync::mpsc::{channel, Sender, Receiver,TryRecvError, SendError},
    crate::{
        cx::Cx,
        cx_api::*,
        event::{Signal, SignalEvent}
    }
};

pub struct ToUIReceiver<T>{
    sender:Sender<T>,
    receiver:Receiver<T>,
    signal:Signal
}

pub struct ToUISender<T>{
    sender:Sender<T>,
    signal:Signal
}

impl<T> Clone for ToUISender<T>{
    fn clone(&self)->Self{
        Self{ sender:self.sender.clone(), signal:self.signal.clone()}
    }
}

unsafe impl<T> Send for ToUISender<T>{}

impl<T> ToUIReceiver<T>{
    pub fn new(cx:&mut Cx)->Self{
        let (sender, receiver) = channel();
        Self{
            sender,
            receiver,
            signal:cx.new_signal()
        }
    }
    
    pub fn sender(&self)->ToUISender<T>{
        ToUISender{
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

impl<T> ToUISender<T>{
    pub fn send(&self, t:T)->Result<(), SendError<T>>{
        let res = self.sender.send(t);
        Cx::post_signal(self.signal, 0);
        res
    }
}

pub struct FromUIReceiver<T>{
    receiver:Receiver<T>,
}

pub struct FromUISender<T>{
    receiver:Option<Receiver<T>>,
    sender:Sender<T>,
}

unsafe impl<T> Send for FromUIReceiver<T>{}

impl<T> FromUISender<T>{
    pub fn new()->Self{
        let (sender, receiver) = channel();
        Self{
            sender,
            receiver:Some(receiver),
        }
    }
    
    pub fn new_channel(&mut self){
        let (sender, receiver) = channel();
        self.sender = sender;
        self.receiver = Some(receiver)
    }

    pub fn send(&self, t:T)->Result<(), SendError<T>>{
        self.sender.send(t)
    }
    
    pub fn sender(&self)->FromUISender<T>{
        FromUISender{
            sender:self.sender.clone(),
            receiver: None
        }
    }
    
    pub fn receiver(&mut self)->FromUIReceiver<T>{
        FromUIReceiver{
            receiver: self.receiver.take().unwrap()
        }
    }
}

impl<T> FromUIReceiver<T>{    
    pub fn try_recv(&self)->Result<T,TryRecvError>{
        self.receiver.try_recv()
    }
}