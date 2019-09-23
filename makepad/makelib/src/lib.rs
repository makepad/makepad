mod messagebus;
pub use crate::messagebus::*;

pub enum MakeCmd{
    Check
}

pub struct Make{
}

impl Make{
    pub fn proc<F>(_event_handler:F)
    where F: FnMut(&mut Make, &mut MakeCmd){
        // check commandline args
        //let mut messagebus = MessageBus::new_lan_broadcast(35162);
        
        //messagebus.send(IPCMessage::QueryOnline); 
        
        //messagebus.recv(|_messagebus, _message|{
            
        //})
    }
    
    
    pub fn cargo(&mut self, _args:&str){
        // execute cargo
        // udp transmit the results 
    }
    
}
