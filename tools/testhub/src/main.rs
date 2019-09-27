use makehub::*;
use makelib::*;
use std::net::SocketAddr;

fn main() {
    let key = [1u8,2u8,3u8,4u8];
    let mut server = HubServer::start_hub_server(
        &key,
        SocketAddr::from(([0, 0, 0, 0], 0))
    );
    
    server.start_announce_server(
        &key,
        SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::from(([255, 255, 255, 255], HUB_ANNOUNCE_PORT)),
    );
    
    // lets wait for a server announce
    let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");

    let client_a = HubClient::connect_to_hub(&key, address).expect("Cannot connect client_a");
    let client_b = HubClient::connect_to_hub(&key, address).expect("Cannot connect client_b");
    
    // lets connect a client
    client_a.tx_write.send(ClientToHubMsg {
        to: HubMsgTo::AllClients,
        msg: HubMsg::Ping
    }).expect("Cannot send messsage");
    
    if let Ok(msg) = client_b.rx_read.recv() {
        println!("Got client_b {:?}", msg);
    }
    
    // start a buildserver
    std::thread::spawn(move || {
        // lets start a Make proc
        Make::proc( | make, htc | match htc.msg {
            HubMsg::GetCargoTargets => {
                make.cargo_has_target("makepad");
            },
            HubMsg::CargoCheck(ck) => {
                match ck.target.as_ref(){
                    "makepad" => make.cargo_check("-p makepad"),
                    _=>()
                }
            },
            _=>()
        });
    });
    
    // send the build server a build command
    client_a.tx_write.send(ClientToHubMsg {
        to: HubMsgTo::AllClients,
        msg: HubMsg::GetCargoTargets
    }).expect("Cannot send messsage");
    
    // OK so how does this work. we have a message pipe
    // we still need to do server disconnecting
    /*
    let time1 = time::precise_time_ns();
    let mut contents = Vec::new();
    contents.resize(1024*1024,0u8);
    let mut t = 0x89071453u32;
    for i in 0..contents.len(){
        t ^= t >> 8 | i as u32;
        contents[i] = t as u8;
        println!("{}", t as u8);
    }
    let mut enc = snap::Encoder::new();
    let compressed = enc.compress_vec(&contents).expect("Cannot compress msgbuf");
    let mut digest = [0u64;26];
    digest_buffer(&mut digest, &compressed);
    println!("{} {}", contents.len(), compressed.len());
    digest_buffer(&mut digest, &compressed);
    let mut dec = snap::Decoder::new();
    let decompressed = dec.decompress_vec(&compressed).expect("Cannot compress msgbuf");
    println!("{}", time::precise_time_ns() - time1);
    */
    server.join_threads();
}
