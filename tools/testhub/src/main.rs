use makehub::*;
use std::net::SocketAddr;

fn main() {
    let key = [1u8,2u8,3u8,4u8];
    let mut server = HubServer::start_hub_server(
        &key,
        SocketAddr::from(([0, 0, 0, 0], 51234))
    );
    
    server.start_announce_server(
        &key,
        SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::from(([255, 255, 255, 255], 51235)),
    );
    
    // lets wait for a server announce
    let address = HubClient::wait_for_announce(
       &key,
       SocketAddr::from(([0, 0, 0, 0], 51235))
    ).expect("cannot wait for announce");

    let client_a = HubClient::connect_to_hub(&key, address).expect("Cannot connect client_a");
    let client_b = HubClient::connect_to_hub(&key, address).expect("Cannot connect client_b");
    
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
