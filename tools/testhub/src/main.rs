use workspacelib::*;
use std::net::SocketAddr;

fn main() {
    let key = [1u8, 2u8, 3u8, 4u8];
    let mut server = HubServer::start_hub_server(
        &key,
        SocketAddr::from(([0, 0, 0, 0], 0))
    );
    
    server.start_announce_server(
        &key,
        SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::from(([255, 255, 255, 255], HUB_ANNOUNCE_PORT)),
        SocketAddr::from(([127, 0, 0, 1], HUB_ANNOUNCE_PORT)),
    );
    
    // lets wait for a server announce
    let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
    
    println!("GOT ADDRESS {:?}", address);
    
    let mut ui_client = HubClient::connect_to_hub(&key, address).expect("Cannot connect client_a");
    
    // lets connect a UI client
    ui_client.tx_write.send(ClientToHubMsg {
        to: HubMsgTo::All,
        msg: HubMsg::ConnectUI
    }).expect("Cannot send messsage");
    
    // start a client message pump
    std::thread::spawn(move || {
        // wait for the answer
        while let Ok(htc) = ui_client.rx_read.recv() {
            match htc.msg {
                HubMsg::ConnectBuild => {
                    let new_uid = ui_client.alloc_uid();
                    ui_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::Build,
                        msg: HubMsg::GetCargoTargets {uid: new_uid}
                    }).expect("Cannot send messsage");
                },
                HubMsg::CargoHasTargets {uid: _, targets: _} => {
                    let new_uid = ui_client.alloc_uid();
                    // now lets fire up a build.
                    ui_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::Build,
                        msg: HubMsg::CargoCheck {uid: new_uid, target: "makepad".to_string()}
                    }).expect("Cannot send messsage");
                    
                },
                _ => ()
            }
        }
    });
    
    // start a buildserver test
    std::thread::spawn(move || {
        // lets start a Make proc
        Workspace::run("test", | make, htc | match htc.msg {
            HubMsg::GetCargoTargets {uid} => {
                make.cargo_has_targets(uid, &["makepad"])
            },
            HubMsg::CargoCheck {uid, target, ..} => {
                make.cargo(uid, &["check", "-p", &target])
            },
            HubMsg::ListWorkspace {uid} => {
                make.list_workspace(uid, "./")
            }
            _ => make.default(htc)
        });
    });
    
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
