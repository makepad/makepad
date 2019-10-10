use render::*;
use hub::*;
use std::sync::{mpsc, Arc, Mutex};

pub struct HubUI {
    pub uid_alloc: u64,
    pub signal: Signal,
    pub tx_write_arc: Arc<Mutex<Option<mpsc::Sender<ClientToHubMsg>>>>,
    pub own_addr_arc: Arc<Mutex<Option<HubAddr>>>,
    pub htc_msgs_arc: Arc<Mutex<Vec<HubToClientMsg>>>,
    pub hub_log: HubLog,
    pub thread: Option<std::thread::JoinHandle<()>>
}

impl HubUI {
    pub fn new(cx:&mut Cx, key: &[u8], hub_log:HubLog)->HubUI{

        let key = key.to_vec();
        let tx_write_arc = Arc::new(Mutex::new(None));
        let htc_msgs_arc = Arc::new(Mutex::new(Vec::new()));
        let own_addr_arc = Arc::new(Mutex::new(None));
        let signal = cx.new_signal();
        
        // lets start a thread that stays connected
        let thread = {
            let tx_write_arc = Arc::clone(&tx_write_arc);
            let htc_msgs_arc = Arc::clone(&htc_msgs_arc);
            let own_addr_arc = Arc::clone(&own_addr_arc);
            let hub_log = hub_log.clone();
            let signal = signal.clone();
            
            std::thread::spawn(move || {
                loop {
                    
                    hub_log.log("HubUI waiting for hub announcement..");
                    
                    // lets wait for a server announce
                    let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
                    
                    hub_log.msg("HubUI got announce, connecting to ", &address);
                    
                    // ok now connect to that address
                    let hub_client = HubClient::connect_to_hub(&key, address, hub_log.clone()).expect("cannot connect to hub");
                    
                    hub_log.msg("HubUI connected to ", &hub_client.server_addr);
                    
                    // lets clone the tx_write
                    let tx_write_clone = hub_client.tx_write.clone();
                    
                    if let Ok(mut tx_write) = tx_write_arc.lock(){
                        *tx_write = Some(tx_write_clone);
                    }

                    if let Ok(mut own_addr) = own_addr_arc.lock(){
                        *own_addr = Some(hub_client.own_addr);
                    }
                    
                    // lets transmit a BuildServer ack
                    hub_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::All,
                        msg: HubMsg::ConnectUI
                    }).expect("Cannot send login");
                    
                    // this is the main messageloop, on rx
                    while let Ok(htc) = hub_client.rx_read.recv() {
                        let restart_connection = if let HubMsg::ConnectionError(_e) = &htc.msg{
                            true
                        }
                        else{
                            false
                        };
                        let mut do_signal = false;
                        if let Ok(mut htc_msgs) = htc_msgs_arc.lock(){
                            if htc_msgs.len() == 0{
                                do_signal = true;
                            }
                            htc_msgs.push(htc);
                        } 
                        if do_signal{
                            Cx::post_signal(signal, 0);
                        }
                        if restart_connection {
                            break
                        }
                    }
                    
                    if let Ok(mut tx_write) = tx_write_arc.lock(){
                        *tx_write = None;
                    }
                    if let Ok(mut own_addr) = own_addr_arc.lock(){
                        *own_addr = None;
                    }
                }
            })
        };

        HubUI{
            uid_alloc:0,
            signal: signal,
            own_addr_arc: own_addr_arc,
            tx_write_arc: tx_write_arc,
            htc_msgs_arc: htc_msgs_arc,
            hub_log:hub_log.clone(),
            // cth_msgs_arc: cth_msgs_arc,
            thread: Some(thread)
        }
    }

    pub fn is_own_addr(&mut self, addr:&HubAddr)->bool{
        if let Ok(own_addr) = self.own_addr_arc.lock(){
            if let Some(own_addr) = *own_addr{
                return own_addr == *addr
            }
        }
        self.hub_log.log("HubUI - Warning, is_own_addr whilst disconnected from hub");
        return false
    }

    pub fn alloc_uid(&mut self)->HubUid{
        self.uid_alloc += 1;
        if let Ok(own_addr) = self.own_addr_arc.lock(){
            if let Some(own_addr) = *own_addr{
                return HubUid{
                    addr:own_addr,
                    id: self.uid_alloc
                }
            }
        }
        self.hub_log.log("HubUI - Warning, trying to alloc_uid whilst disconnected from hub");
        return HubUid{
            addr:HubAddr::zero(),
            id: self.uid_alloc
        }
    }
    
    pub fn send(&mut self, msg:ClientToHubMsg){
        if let Ok(tx_write) = self.tx_write_arc.lock(){
            if let Some(tx_write) = &*tx_write{
                tx_write.send(msg).expect("Cannot tx_write.send - unexpected");;
            }else{ // lets queue up
                self.hub_log.log("HubUI - Warning, trying to send messages whilst disconnected from hub");
            }
        }
    }
}