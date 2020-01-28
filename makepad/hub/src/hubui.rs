use std::sync::{mpsc, Arc, Mutex};
use crate::hubmsg::*;
use crate::hubrouter::*;

pub struct HubUI {
    pub hub_log:HubLog,
    pub thread: Option<std::thread::JoinHandle<()>>,
    pub route_send: HubRouteSend,
    pub htc_msgs_arc: Arc<Mutex<Vec<FromHubMsg>>>,
}

impl HubUI {
    
    pub fn start_hub_ui_direct<F>(hub_router:&mut HubRouter,event_handler: F)->HubUI
    where F: Fn() + Clone + Send + 'static{
        // lets create a tx pair, and add a route
        let (tx_write, rx_write) = mpsc::channel::<FromHubMsg>();
        
        let htc_msgs_arc = Arc::new(Mutex::new(Vec::new()));
        let route_send = hub_router.connect_direct(HubRouteType::UI, tx_write);
        
        let thread = {
            let htc_msgs_arc = Arc::clone(&htc_msgs_arc);
            let route_send = route_send.clone();
            let event_handler = event_handler.clone();
            std::thread::spawn(move || {
                // lets transmit a BuildServer ack
                route_send.send(ToHubMsg {
                    to: HubMsgTo::All,
                    msg: HubMsg::ConnectUI
                });
                
                // this is the main messageloop, on rx
                while let Ok(htc) = rx_write.recv() {
                    let mut do_signal = false;
                    if let Ok(mut htc_msgs) = htc_msgs_arc.lock(){
                        if htc_msgs.len() == 0{
                            do_signal = true;
                        }
                        htc_msgs.push(htc);
                    } 
                    if do_signal{
                        event_handler();
                    }
                }
            })
        };

        HubUI{
            thread: Some(thread),
            htc_msgs_arc: htc_msgs_arc,
            hub_log: HubLog::None,
            route_send: route_send
        }
    }

    
    pub fn get_messages(&mut self)->Option<Vec<FromHubMsg>>{
        if let Ok(mut htc_msgs) = self.htc_msgs_arc.lock() {
            let mut msgs = Vec::new();
            std::mem::swap(&mut msgs, &mut htc_msgs);
            return Some(msgs);
        }
        None
    }
}