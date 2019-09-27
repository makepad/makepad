use makehub::*;

pub enum MakeCmd{
    Check
}

pub struct Make{
    hub_client:HubClient
}

impl Make{
    pub fn proc<F>(mut event_handler:F)
    where F: FnMut(&mut Make, HubToClientMsg){
        let key = [1u8, 2u8, 3u8, 4u8];
        
        // lets wait for a server announce
        let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
        
        // ok now connect to that address
        let hub_client = HubClient::connect_to_hub(&key, address).expect("cannot connect to hub");

        let mut make = Make{
            hub_client: hub_client
        };

        // this is the main messageloop, on rx
        while let Ok(htc) = make.hub_client.rx_read.recv(){
            // we just call the thing.
            event_handler(&mut make, htc);
        }
    }
    
    
    pub fn cargo_check(&mut self, _args:&str){
        // lets start a thread
        
    }
    
    pub fn cargo_has_target(&mut self, tgt:&str){
        self.hub_client.tx_write(ClientToHubMsg{
            to:
            msg:HubMsg{
                
            }
        })
    }
}
