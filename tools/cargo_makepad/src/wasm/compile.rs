use crate::makepad_shell::*;
use crate::utils::*;
use crate::makepad_http::server::*;
use crate::makepad_wasm_strip::*;
use std::{
    io::prelude::*,
    path::{PathBuf},
    fs::File,
    fs,
    sync::mpsc,
    net::{SocketAddr},
};

pub struct WasmBuildResult {
    app_dir: PathBuf,
}

pub fn generate_html(wasm:&str)->String{
    format!("
    <!DOCTYPE html>
    <html>
    <head>
        <meta charset='utf-8'>
        <meta name='viewport' content='width=device-width, initial-scale=1.0, user-scalable=no'>
        <title>{wasm}</title>
        <script type='module'>
            import {{WasmWebGL}} from './makepad_platform/web_gl.js'
                            
            const wasm = await WasmWebGL.fetch_and_instantiate_wasm(
                './{wasm}.wasm'
            );
                            
            class MyWasmApp {{
                constructor(wasm) {{
                    let canvas = document.getElementsByClassName('full_canvas')[0];
                    this.webgl = new WasmWebGL (wasm, this, canvas);
                }}
            }}
            let app = new MyWasmApp(wasm);
        </script>
        <script type='module' src='./makepad_platform/auto_reload.js'></script>
        <link rel='stylesheet' type='text/css' href='./makepad_platform/full_canvas.css'>
    </head> 
    <body>
        <canvas class='full_canvas'></canvas>
            <div class='canvas_loader' >
            <div style=''>
            Loading..
            </div>
        </div>
    </body>
    </html>
    ")
}

pub fn build(strip:bool, args: &[String]) -> Result<WasmBuildResult, String> {
    let build_crate = get_build_crate_from_args(args) ?;
    
    let base_args = &[
        "run",
        "nightly",
        "cargo",
        "build",
        "--target=wasm32-unknown-unknown",
        "-Z", 
        "build-std=panic_abort,std", 
    ];
    let cwd = std::env::current_dir().unwrap();
    
    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    
    // dont allow wasm builds to be debug builds
    let mut found_release = false;
    for arg in args {
        if arg == "--release"{found_release = true};
        args_out.push(arg);
    }
    if !found_release{
        args_out.insert(4, "--release");
    }
    
    shell_env(&[
        ("RUSTFLAGS", "-C codegen-units=1 -C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer -C opt-level=z"),
        ("MAKEPAD", "lines"),
    ], &cwd, "rustup", &args_out) ?;
    
    let app_dir = cwd.join(format!("target/makepad-wasm-app/{}", build_crate));
    let build_dir = cwd.join(format!("target/wasm32-unknown-unknown/release"));
    
    let build_crate_dir = get_crate_dir(build_crate) ?;
    let local_resources_path = build_crate_dir.join("resources");
            
    if local_resources_path.is_dir() {
        // if we have an index.html in src/ copy that one
        let underscore_build_crate = build_crate.replace('-', "_");
        let dst_dir = app_dir.join(underscore_build_crate).join("resources");
        mkdir(&dst_dir) ?;
        cp_all(&local_resources_path, &dst_dir, false) ?;
    }
    let resources = get_crate_dep_dirs(build_crate, &build_dir, "wasm32-unknown-unknown");
    for (name, dep_dir) in resources.iter() {
        // alright we need special handling for makepad-wasm-bridge
        // and makepad-platform
        if name == "makepad-wasm-bridge"{
            cp(&dep_dir.join("src/wasm_bridge.js"), &app_dir.join("makepad_wasm_bridge/wasm_bridge.js"), false)?;
        }
        if name == "makepad-platform"{
            cp(&dep_dir.join("src/os/web/audio_worklet.js"), &app_dir.join("makepad_platform/audio_worklet.js"), false)?;
            
            cp(&dep_dir.join("src/os/web/web_gl.js"), &app_dir.join("makepad_platform/web_gl.js"), false)?;
            
            cp(&dep_dir.join("src/os/web/web_worker.js"), &app_dir.join("makepad_platform/web_worker.js"), false)?;
            
            cp(&dep_dir.join("src/os/web/web.js"), &app_dir.join("makepad_platform/web.js"), false)?;
            
            cp(&dep_dir.join("src/os/web/auto_reload.js"), &app_dir.join("makepad_platform/auto_reload.js"), false)?;
            
            cp(&dep_dir.join("src/os/web/full_canvas.css"), &app_dir.join("makepad_platform/full_canvas.css"), false)?;
        }
        let name = name.replace("-","_");
        let resources_path = dep_dir.join("resources");
        if resources_path.is_dir(){
            let dst_dir = app_dir.join(&name).join("resources");
            mkdir(&dst_dir) ?;
            cp_all(&resources_path, &dst_dir, false) ?;
        }            
    }

    let wasm_source = build_dir.join(format!("{}.wasm", build_crate));
    let wasm_dest = app_dir.join(format!("{}.wasm", build_crate));
    if strip{
        if let Ok(data) = fs::read(&wasm_source) {
            if let Ok(strip) = wasm_strip_debug(&data) {
                fs::write(&wasm_dest, strip).map_err( | e | format!("Can't write file {:?} {:?} ", wasm_dest, e)) ?;
            }
            else {
                return Err(format!("Cannot parse wasm {:?}", wasm_source));
            }
        }
        else{
            return Err(format!("Cannot read wasm file {:?}", wasm_source));
        }
    }
    else{
        cp(&wasm_source, &wasm_dest, false)?;
    }
    // generate html file
    let index_path = app_dir.join("index.html");
    let html = generate_html(build_crate);
    fs::write(&index_path, &html.as_bytes()).map_err( | e | format!("Can't write {:?} {:?} ", index_path, e)) ?;
    
    Ok(WasmBuildResult{
        app_dir
    })
}


pub fn run(lan:bool, port:u16, strip:bool, args: &[String]) -> Result<(), String> {
    // we should run the compiled folder root as webserver
    let result = build(strip, args)?;
    start_wasm_server(result.app_dir, lan, port);
    return Err("Run is not implemented yet".into());
}

pub fn start_wasm_server(root:PathBuf, lan:bool, port: u16) {
    
    let addr = if lan{
        SocketAddr::new("127.0.0.1".parse().unwrap(), port)
    }
    else{
        SocketAddr::new("0.0.0.0".parse().unwrap(), port)
    };
    println!("Starting webserver on http://127.0.0.1:{}", port);
    let (tx_request, rx_request) = mpsc::channel::<HttpServerRequest> ();
            
    start_http_server(HttpServer {
        listen_address: addr,
        post_max_size: 1024 * 1024,
        request: tx_request
    });
    
    std::thread::spawn(move || {

        while let Ok(message) = rx_request.recv() {
            // only store last change, fix later
            match message {
                HttpServerRequest::ConnectWebSocket {..} => {},
                HttpServerRequest::DisconnectWebSocket {..} => {},
                HttpServerRequest::BinaryMessage {..} => {}
                HttpServerRequest::Get {headers, response_sender} => {
                    let path = &headers.path;
                    
                    // alright wasm http server
                    if path == "/$watch" || path == "/favicon.ico" {
                        let header = "HTTP/1.1 200 OK\r\n\
                        Cache-Control: max-age:0\r\n\
                        Connection: close\r\n\r\n".to_string();
                        let _ = response_sender.send(HttpServerResponse {header, body: vec![]});
                        continue
                    }
                                            
                    let mime_type = if path.ends_with(".html") {"text/html"}
                    else if path.ends_with(".wasm") {"application/wasm"}
                    else if path.ends_with(".css") {"text/css"}
                    else if path.ends_with(".js") {"text/javascript"}
                    else if path.ends_with(".ttf") {"application/ttf"}
                    else if path.ends_with(".png") {"image/png"}
                    else if path.ends_with(".jpg") {"image/jpg"}
                    else if path.ends_with(".svg") {"image/svg+xml"}
                    else {continue};
                                            
                    if path.contains("..") || path.contains('\\') {
                        continue
                    }
                    let path = path.strip_prefix("/").unwrap();
                                         
                    let path = root.join(&path);
                    println!("OPENING {:?}", path);
                    if let Ok(mut file_handle) = File::open(path) {
                        let mut body = Vec::<u8>::new();
                        if file_handle.read_to_end(&mut body).is_ok() {
                            let header = format!(
                                "HTTP/1.1 200 OK\r\n\
                                Content-Type: {}\r\n\
                                Cross-Origin-Embedder-Policy: require-corp\r\n\
                                Cross-Origin-Opener-Policy: same-origin\r\n\
                                Content-encoding: none\r\n\
                                Cache-Control: max-age:0\r\n\
                                Content-Length: {}\r\n\
                                Connection: close\r\n\r\n",
                                mime_type,
                                body.len()
                            );
                            let _ = response_sender.send(HttpServerResponse {header, body});
                        }
                    }
                }
                HttpServerRequest::Post {..} => { //headers, body, response}=>{
                }
            }
        }
    }).join().unwrap();
}