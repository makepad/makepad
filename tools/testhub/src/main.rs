use makehub::*;

fn main() {
    println!("STARTING TEST HUB ");
    let mut server = HubServer::start_hub_server();
    // lets wait for server to terminate
    
    let mut client = HubClient::connect_to_hub("127.0.0.1:51234");
    
    // lets connect a client
    client.tx_write.send(ClientToHubMsg{
        to:None,
        msg:HubMsg::LoginBuildServer
    });
    
    server.join_threads();
}
