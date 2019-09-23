use makehub::*;
use std::net::SocketAddr;

fn main() {
    let mut server = HubServer::start_hub_server(
        SocketAddr::from(([0, 0, 0, 0], 51234))
    );
    
    server.start_announce_thread(
        SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::from(([255, 255, 255, 255], 51235)),
    );
    
    // lets wait for an announcement
    let address = HubClient::wait_for_announce(SocketAddr::from(([0, 0, 0, 0], 51235)));
    //let address = SocketAddr::from(([127, 0, 0, 1], 51234));
    // we get an announcement
    let client_a = HubClient::connect_to_hub(address).expect("Cannot connect client_a");
    let client_b = HubClient::connect_to_hub(address).expect("Cannot connect client_a");
    
    // lets connect a client
    client_a.tx_write.send(ClientToHubMsg {
        target: HubTarget::AllClients,
        msg: HubMsg::Ping
    }).expect("Cannot send messsage");
    
    if let Ok(msg) = client_b.rx_read.recv() {
        println!("Got client_b {:?}", msg);
    }
    
    server.join_threads();
    
}
