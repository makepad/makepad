use hub::*;

fn main() {
    let key = [1u8, 2u8, 3u8, 4u8];
    let mut server = HubServer::start_hub_server_default(&key, HubLog::None);
    
    server.start_announce_server_default(&key);
    
    // lets wait for a server announce
    let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
    
    println!("GOT ADDRESS {:?}", address);
    
    let mut ui_client = HubClient::connect_to_hub(&key, address, HubLog::None).expect("Cannot connect client_a");
    
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
                HubMsg::ConnectWorkspace(_) => {
                    let new_uid = ui_client.alloc_uid();
                    ui_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::Client(htc.from),
                        msg: HubMsg::CargoPackagesRequest {uid: new_uid}
                    }).expect("Cannot send messsage");
                },
                HubMsg::CargoPackagesResponse {uid:_, packages: _} => {
                    let new_uid = ui_client.alloc_uid();
                    // now lets fire up a build.
                    ui_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::Client(htc.from),
                        msg: HubMsg::CargoExec {
                            uid: new_uid,
                            package: "makepad".to_string(),
                            target: "check".to_string()
                        }
                    }).expect("Cannot send messsage");
                    
                    ui_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::Client(htc.from),
                        msg: HubMsg::CargoKill {
                            uid: new_uid,
                        }
                    }).expect("Cannot send messsage");
                },
                _ => ()
            }
        }
    });
    
    // start a buildserver test
    std::thread::spawn(move || {
        // lets start a Make proc
        let key = [1u8, 2u8, 3u8, 4u8];
        HubWorkspace::run(&key, "makepad", "edit_repo", HubLog::None, | ws, htc | match htc.msg {
            HubMsg::CargoPackagesRequest {uid} => {
                ws.cargo_packages(htc.from, uid, vec![
                    HubCargoPackage::new("makepad", &["check", "makepad", "workspace"])
                ])
            },
            HubMsg::CargoExec {uid, package, target} => {
                match package.as_ref() {
                    "makepad" => match target.as_ref() {
                        "check" => ws.cargo_exec(uid, &["check", "-p", &package]),
                        "makepad" => ws.cargo_exec(uid, &["build", "-p", "makepad"]),
                        "workspace" => ws.cargo_exec(uid, &["build", "-p", "workspace"]),
                        _ => ()
                    },
                    _ => ()
                }
            },
            _ => ws.default(htc)
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
