use makehub::*;

pub enum MakeCmd{
    Check
}

pub struct Make{
}

impl Make{
    pub fn proc<F>(_event_handler:F)
    where F: FnMut(&mut Make, &mut MakeCmd){
        //let key = [1u8,2u8,3u8,4u8];
        // connect to the local build server.
        
         // lets wait for a server announce
        //let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
        
        // ok now connect to that address
        //let hub_client = HubClient::connect_to_hub(&key, address).expect("cannot connect to hub"){
        
        
    }
    
    
    pub fn cargo(&mut self, _args:&str){
        // execute cargo
        // udp transmit the results 
    }
    
}
