use makehub::*;

fn main() {
    println!("STARTING TEST HUB ");
    let mut server = HubServer::start_hub_server();
    // lets wait for server to terminate
    
    let client_a = HubClient::connect_to_hub("127.0.0.1:51234");
    let client_b = HubClient::connect_to_hub("127.0.0.1:51234");
    
    // lets connect a client
    client_a.tx_write.send(ClientToHubMsg{
        target:HubTarget::AllClients,
        msg:HubMsg::Ping
    }).expect("Cannot send messsage");
    
    
    
    server.join_threads();
}
