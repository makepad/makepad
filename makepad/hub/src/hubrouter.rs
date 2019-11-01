use std::net::{TcpStream, Shutdown};
use std::sync::{mpsc, Arc, Mutex};
use crate::hubmsg::*;

#[derive(PartialEq)]
pub enum HubRouteType{
    Unknown,
    Workspace(String),
    Clone(String),
    UI
}

// wraps a connection to the router either networked or direct.
#[derive(Clone)]
pub enum HubRouteSend {
    Networked{
        uid_alloc: Arc<Mutex<u64>>,
        tx_write_arc: Arc<Mutex<Option<mpsc::Sender<ToHubMsg>>>>,
        own_addr_arc: Arc<Mutex<Option<HubAddr>>>,
    },
    Direct{
        uid_alloc: Arc<Mutex<u64>>,
        tx_pump: mpsc::Sender<(HubAddr, ToHubMsg)>,
        own_addr: HubAddr
    }
}

// the connection send interface to the router
impl HubRouteSend{

    pub fn is_own_addr(&self, addr:&HubAddr)->bool{
        match self{
            HubRouteSend::Networked{own_addr_arc,..}=>{
                if let Ok(own_addr) = own_addr_arc.lock(){
                    if let Some(own_addr) = *own_addr{
                        return own_addr == *addr
                    }
                }
                //self.hub_log.log("HubUI - Warning, is_own_addr whilst disconnected from hub");
                return false
            },
            HubRouteSend::Direct{own_addr,..}=>{
                return *own_addr == *addr
            }
        }
    }

    pub fn alloc_uid(&mut self)->HubUid{
        match self{
            HubRouteSend::Networked{own_addr_arc,uid_alloc,..}=>{
                let id = if let Ok(mut uid_alloc) = uid_alloc.lock(){
                    *uid_alloc += 1;
                    *uid_alloc
                }
                else{0};
                if let Ok(own_addr) = own_addr_arc.lock(){
                    if let Some(own_addr) = *own_addr{
                        return HubUid{
                            addr:own_addr,
                            id: id
                        }
                    }
                }
            },
            HubRouteSend::Direct{own_addr,uid_alloc,..}=>{
                let id = if let Ok(mut uid_alloc) = uid_alloc.lock(){
                    *uid_alloc += 1;
                    *uid_alloc
                }
                else{0};
                return HubUid{
                    addr:own_addr.clone(),
                    id: id
                }
            }
        }
        println!("HubUI - Warning, trying to alloc_uid whilst disconnected from hub");
        return HubUid{
            addr:HubAddr::None,
            id: 0
        }
    }
    
     pub fn update_networked_in_place(&self, set_addr:Option<HubAddr>, tx_write:Option<mpsc::Sender<ToHubMsg>>){
        match self{
            HubRouteSend::Networked{own_addr_arc,tx_write_arc,..}=>{
                if let Ok(mut own_addr) = own_addr_arc.lock(){
                    *own_addr = set_addr
                }
                if let Ok(mut tx_write_arc) = tx_write_arc.lock(){
                    *tx_write_arc = tx_write
                }
            },
            HubRouteSend::Direct{..}=>{
                panic!("update_inner_networked on direct route");
            }
        }
    }
    
    
    pub fn send(&self, msg:ToHubMsg){
        match self{
            HubRouteSend::Networked{tx_write_arc,..}=>{
                if let Ok(tx_write) = tx_write_arc.lock(){
                    if let Some(tx_write) = &*tx_write{
                        tx_write.send(msg).expect("Cannot tx_write.send - unexpected");;
                    }//else{ // lets queue up
                    //    self.hub_log.log("HubUI - Warning, trying to send messages whilst disconnected from hub");
                   // }
                }
            },
            HubRouteSend::Direct{tx_pump,own_addr,..}=>{
                tx_pump.send((*own_addr, msg)).expect("Cannot tx_write.send - unexpected");;
            }
        }
    }
}

pub struct HubRoute {
    pub peer_addr: HubAddr,
    pub tx_write: mpsc::Sender<FromHubMsg>,
    pub tcp_stream: Option<TcpStream>,
    pub route_type: HubRouteType
}

pub struct HubRouter{
    pub local_uid: u64,
    pub tx_pump: mpsc::Sender<(HubAddr, ToHubMsg)>,
    pub routes: Arc<Mutex<Vec<HubRoute>>>,
    pub router_thread: Option<std::thread::JoinHandle<()>>,
}

impl HubRouter{
    pub fn alloc_local_addr(&mut self)->HubAddr{
        self.local_uid += 1;
        return HubAddr::Local{uid:self.local_uid};
    }

    pub fn connect_direct(&mut self, route_type: HubRouteType, tx_write: mpsc::Sender<FromHubMsg>)->HubRouteSend{
        let tx_pump = self.tx_pump.clone();
        let own_addr = self.alloc_local_addr();
        
        if let Ok(mut routes) = self.routes.lock() {
            routes.push(HubRoute {
                route_type: route_type,
                peer_addr: own_addr.clone(),
                tcp_stream: None,
                tx_write: tx_write
            })
        };
        
        HubRouteSend::Direct{
            uid_alloc: Arc::new(Mutex::new(0)),
            tx_pump: tx_pump,
            own_addr: own_addr
        }
    }
        
    pub fn start_hub_router(hub_log:HubLog)->HubRouter{
         let (tx_pump, rx_pump) = mpsc::channel::<(HubAddr, ToHubMsg)>();
         let routes = Arc::new(Mutex::new(Vec::<HubRoute>::new()));
         let router_thread = {
            let hub_log = hub_log.clone();
            let routes = Arc::clone(&routes);
            std::thread::spawn(move || {
                // ok we get inbound messages from the threads
                while let Ok((from, cth_msg)) = rx_pump.recv() {
                    let to = cth_msg.to;
                    let htc_msg = FromHubMsg {
                        from: from,
                        msg: cth_msg.msg
                    };
                    // we got a message.. now lets route it elsewhere
                    if let Ok(mut routes) = routes.lock() {
                        hub_log.msg("HubServer sending", &htc_msg);
                        
                        if let Some(cid) = routes.iter().position( | c | c.peer_addr == htc_msg.from) {
                            if routes[cid].route_type == HubRouteType::Unknown {
                                match &htc_msg.msg {
                                    HubMsg::ConnectWorkspace(ws_name) => { // send it to all clients
                                        let mut connection_refused = false;
                                        for route in routes.iter() {
                                            if let HubRouteType::Workspace(existing_ws_name) = &route.route_type{
                                                if *existing_ws_name == *ws_name{
                                                    connection_refused = true;
                                                    break;
                                                }
                                            }
                                        }
                                        if connection_refused{
                                            println!("Already have a workspace by that name {}, disconnecting", ws_name);
                                            if let Some(tcp_stream) = &mut routes[cid].tcp_stream{
                                                let _ = tcp_stream.shutdown(Shutdown::Both);
                                            }
                                            routes.remove(cid);
                                            continue;
                                        }
                                        routes[cid].route_type = HubRouteType::Workspace(ws_name.to_string());
                                    },
                                    HubMsg::ConnectClone(ws_name)=>{
                                        routes[cid].route_type = HubRouteType::Clone(ws_name.to_string());
                                    },
                                    HubMsg::ConnectUI => { // send it to all clients
                                        routes[cid].route_type = HubRouteType::UI;
                                    },
                                    _ => {
                                        println!("Router got message from unknown client {:?}, disconnecting", htc_msg.from);
                                        if let Some(tcp_stream) = &mut routes[cid].tcp_stream{
                                            let _ = tcp_stream.shutdown(Shutdown::Both);
                                        }
                                        routes.remove(cid);
                                        continue;
                                    }
                                }
                            }
                        }
                        
                        match to {
                            HubMsgTo::All => { // send it to all
                                for route in routes.iter() {
                                    if route.route_type != HubRouteType::Unknown {
                                        route.tx_write.send(htc_msg.clone()).expect("Could not tx_write.send");
                                    }
                                }
                            },
                            HubMsgTo::Client(addr) => { // find our specific addr and send
                                if let Some(route) = routes.iter().find( | c | c.peer_addr == addr) {
                                    if route.route_type != HubRouteType::Unknown {
                                        route.tx_write.send(htc_msg).expect("Could not tx_write.send");
                                    }
                                }
                            },
                            HubMsgTo::Workspace(to_ws_name)=>{
                                for route in routes.iter() {
                                    match &route.route_type{
                                        HubRouteType::Workspace(ws_name)=>if to_ws_name == *ws_name{
                                            route.tx_write.send(htc_msg.clone()).expect("Could not tx_write.send");
                                        },
                                        HubRouteType::Clone(ws_name)=>if to_ws_name == *ws_name{
                                            route.tx_write.send(htc_msg.clone()).expect("Could not tx_write.send");
                                        },
                                        _=>()
                                    }
                                }
                            },
                            HubMsgTo::UI=>{
                                for route in routes.iter() {
                                    if route.route_type == HubRouteType::UI{
                                        route.tx_write.send(htc_msg.clone()).expect("Could not tx_write.send");
                                    }
                                }
                            },
                            HubMsgTo::Hub => { // process queries on the hub
                                match &htc_msg.msg {
                                    HubMsg::ConnectionError(e) => {
                                        // connection error, lets remove connection
                                        if let Some(pos) = routes.iter().position( | c | c.peer_addr == htc_msg.from) {
                                            hub_log.log(&format!("Server closing connection {:?} from error {:?}", htc_msg.from, e));
                                            // let everyone know we lost something
                                            let msg = FromHubMsg{
                                                from:htc_msg.from,
                                                msg: match &routes[pos].route_type{
                                                    HubRouteType::Workspace(ws_name)=>HubMsg::DisconnectWorkspace(ws_name.clone()),
                                                    HubRouteType::Clone(ws_name)=>HubMsg::DisconnectClone(ws_name.clone()),
                                                    HubRouteType::UI=>HubMsg::DisconnectUI,
                                                    HubRouteType::Unknown=>{
                                                        continue
                                                    }
                                                }
                                            };
                                            routes.remove(pos);
                                            for route in routes.iter() {
                                                route.tx_write.send(msg.clone()).expect("Could not tx_write.send");
                                            }
                                        }
                                    },
                                    HubMsg::ListWorkspacesRequest{uid}=>{
                                        let mut workspaces = Vec::new();
                                        for route in routes.iter() {
                                            match &route.route_type{
                                                HubRouteType::Workspace(ws_name)=>workspaces.push(ws_name.to_string()),
                                                _=>()
                                            }
                                        }
                                        // send it back to the caller
                                        if let Some(route) = routes.iter().find( | c | c.peer_addr == htc_msg.from) {
                                            route.tx_write.send(FromHubMsg{
                                                from:htc_msg.from,
                                                msg:HubMsg::ListWorkspacesResponse{
                                                    uid:*uid,
                                                    workspaces:workspaces
                                                }
                                            }).expect("Could not tx_write.send");
                                        }
                                    },
                                    _ => ()
                                }
                                // return current connections
                            }
                        }
                    }
                }
            })
        };
        
        return HubRouter {
            tx_pump: tx_pump,
            router_thread: Some(router_thread),
            local_uid: 1,
            routes: routes
        };
    }
}
