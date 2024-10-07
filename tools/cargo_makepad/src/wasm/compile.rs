use crate::makepad_shell::*;
use crate::utils::*;
use crate::makepad_http::server::*;
use crate::makepad_wasm_strip::*;
use std::{
    io::prelude::*,
    collections::HashMap,
    path::{PathBuf},
    fs::File,
    fs,
    sync::mpsc,
    net::{SocketAddr},
};

pub struct WasmBuildResult {
    app_dir: PathBuf,
}

#[derive(Clone, Copy)]
pub struct WasmConfig{
    pub strip: bool,
    pub lan:bool,
    pub port:Option<u16>,
    pub small_fonts: bool,
    pub brotli: bool,
    pub bindgen: bool,
}

pub fn generate_html(wasm:&str, config: &WasmConfig)->String{
    let init = if config.bindgen {
        format!("
            import {{init_env}} from './makepad_wasm_bridge/wasm_bridge.js'
            import init from './bindgen.js';
    
            let env = {{}};
            let set_wasm = init_env(env);
            let module = await WebAssembly.compileStreaming(fetch('./{wasm}.wasm'))
            let wasm = await init({{module_or_path: module}}, env);
            set_wasm(wasm);

            wasm._has_thread_support = true;
            wasm._memory = wasm.exports.memory;
            wasm._module = module;
            import {{WasmWebGL}} from './makepad_platform/web_gl.js'
            ")
    } else {
        format!("
            import {{WasmWebGL}} from './makepad_platform/web_gl.js'
            const wasm = await WasmWebGL.fetch_and_instantiate_wasm(
                './{wasm}.wasm'
            );
            ")
    };

    format!("
    <!DOCTYPE html>
    <html>
    <head>
        <meta charset='utf-8'>
        <meta name='viewport' content='width=device-width, initial-scale=1.0, user-scalable=no'>
        <title>{wasm}</title>
        <script type='module'>
            {init}
            class MyWasmApp {{
                constructor(wasm) {{
                    let canvas = document.getElementsByClassName('full_canvas')[0];
                    this.webgl = new WasmWebGL (wasm, this, canvas);
                }}
            }}
            let app = new MyWasmApp(wasm);
        </script>
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

fn brotli_compress(dest_path:&PathBuf){
    let source_file_name = dest_path.file_name().unwrap().to_string_lossy().to_string();
    let dest_path_br = dest_path.parent().unwrap().join(&format!("{}.br", source_file_name));
    println!("Compressing {:?}", dest_path);
    // lets read the dest_path
    // lets brotli compress dest_path
    let mut brotli_data = Vec::new();
    let data = fs::read(&dest_path).expect("Can't read file");
    {
        let mut writer = brotli::CompressorWriter::new(&mut brotli_data, 4096 /* buffer size */, 12, 22);
        writer.write_all(&data).expect("Can't write data");
    }
    let mut brotli_file = File::create(dest_path_br).unwrap();
    brotli_file.write_all(&brotli_data).unwrap();
}

pub fn cp_brotli(source_path: &PathBuf, dest_path: &PathBuf, exec: bool, compress:bool) -> Result<(), String> {
    cp(source_path, dest_path, exec)?;
    if compress{
        brotli_compress(dest_path);
    }
    Ok(())
}
    
        
pub fn build(config:WasmConfig, args: &[String]) -> Result<WasmBuildResult, String> {
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
    let profile = get_profile_from_args(&args);
    for arg in args {
        args_out.push(arg);
    }
    
    shell_env(&[
        ("RUSTFLAGS", "-C codegen-units=1 -C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer -C opt-level=z"),
        ("MAKEPAD", "lines"),
    ], &cwd, "rustup", &args_out) ?;
    
    let app_dir = cwd.join(format!("target/makepad-wasm-app/{profile}/{}", build_crate));
    let build_dir = cwd.join(format!("target/wasm32-unknown-unknown/{profile}"));
    
    let build_crate_dir = get_crate_dir(build_crate) ?;
    let local_resources_path = build_crate_dir.join("resources");
            
    if local_resources_path.is_dir() {
        // if we have an index.html in src/ copy that one
        let underscore_build_crate = build_crate.replace('-', "_");
        let dst_dir = app_dir.join(underscore_build_crate).join("resources");
        mkdir(&dst_dir) ?;
        //cp_all(&local_resources_path, &dst_dir, false) ?;
        walk_all(&local_resources_path, &dst_dir, &mut |source_path, dest_dir|{
            let source_file_name = source_path.file_name().ok_or_else(|| format!("Unable to get filename for {:?}", source_path))?.to_string_lossy().to_string();
            let dest_path = dest_dir.join(&source_file_name);
            cp(&source_path, &dest_path, false)?;
            if config.brotli{
                brotli_compress(&dest_path);
            }
            Ok(())
        }) ?;
        
    }
    let resources = get_crate_dep_dirs(build_crate, &build_dir, "wasm32-unknown-unknown");
    for (name, dep_dir) in resources.iter() {
        // alright we need special handling for makepad-wasm-bridge
        // and makepad-platform
        if name == "makepad-wasm-bridge"{
            cp_brotli(&dep_dir.join("src/wasm_bridge.js"), &app_dir.join("makepad_wasm_bridge/wasm_bridge.js"), false, config.brotli)?;
        }
        if name == "makepad-platform"{
            cp_brotli(&dep_dir.join("src/os/web/audio_worklet.js"), &app_dir.join("makepad_platform/audio_worklet.js"), false,config.brotli)?;
            
            cp_brotli(&dep_dir.join("src/os/web/web_gl.js"), &app_dir.join("makepad_platform/web_gl.js"), false, config.brotli)?;
            
            if config.bindgen {
                let jsfile = dep_dir.join("src/os/web/web_worker.js");
                let js = std::fs::read_to_string(&jsfile).map_err(|e| format!("Unable to find web.js {e:?}"))?;
                let tmp = build_dir.join("web_worker.js");
                let mut file = std::fs::OpenOptions::new().write(true).truncate(true).create(true).open(&tmp ).unwrap();
                file.write(format!("import init from '../bindgen.js';\n{js}").as_bytes()).unwrap();
                cp_brotli(&tmp, &app_dir.join("makepad_platform/web_worker.js"), false, config.brotli)?;
            } else {
                cp_brotli(&dep_dir.join("src/os/web/web_worker.js"), &app_dir.join("makepad_platform/web_worker.js"), false, config.brotli)?;
            }
            
            cp_brotli(&dep_dir.join("src/os/web/web.js"), &app_dir.join("makepad_platform/web.js"), false, config.brotli)?;
            
            cp_brotli(&dep_dir.join("src/os/web/auto_reload.js"), &app_dir.join("makepad_platform/auto_reload.js"), false, config.brotli)?;
            
            cp_brotli(&dep_dir.join("src/os/web/full_canvas.css"), &app_dir.join("makepad_platform/full_canvas.css"), false, config.brotli)?;
        }
        let name = name.replace("-","_");
        let resources_path = dep_dir.join("resources");
        
        let mut rename:HashMap<String,String> = HashMap::new();
        
        if config.small_fonts{
            rename.insert("GoNotoKurrent-Bold.ttf".into(), "IBMPlexSans-SemiBold.ttf".into());
            rename.insert("GoNotoKurrent-Regular.ttf".into(), "IBMPlexSans-Text.ttf".into());
        }
        
        if resources_path.is_dir(){
            // alright so.. the easiest thing is to rename a bunch of resources
            
            let dst_dir = app_dir.join(&name).join("resources");
            mkdir(&dst_dir) ?;
            walk_all(&resources_path, &dst_dir, &mut |source_path, dest_dir|{
                let source_file_name = source_path.file_name().ok_or_else(|| format!("Unable to get filename for {:?}", source_path))?.to_string_lossy().to_string();
                let source_path2 = if let Some(tgt) = rename.get(&source_file_name){
                    //println!("RENAMING {} {}", source_file_name, tgt);
                    &source_path.parent().unwrap().join(tgt)
                }
                else{
                    source_path
                };
                let dest_path = dest_dir.join(&source_file_name);
                cp(&source_path2, &dest_path, false)?;
                if config.brotli{
                    brotli_compress(&dest_path);
                }
                Ok(())
            }) ?;
        }            
    }
    let wasm_source = if config.bindgen {
        shell(build_dir.as_path(), "wasm-bindgen", &[&format!("{build_crate}.wasm"), "--out-dir=.", "--out-name=bindgen", "--target=web", "--no-typescript", ])?;
        let jsfile = build_dir.join("bindgen.js");
        let patched = std::fs::read_to_string(&jsfile).map_err(|e| format!("Unable to find wasm-bidngen generated file {e:?}"))?
            .replace("import * as __wbg_star0 from 'env';", "")
            .replace("imports['env'] = __wbg_star0;", "")
            .replace("return wasm;\n}", "return instance;\n}")
            .replace("__wbg_init(module_or_path, memory) {", "__wbg_init(module_or_path, env) {let memory;")
            .replace("imports = __wbg_get_imports();", "imports = __wbg_get_imports(); imports.env = env;");
        std::fs::OpenOptions::new().write(true).truncate(true).open(&jsfile).unwrap().write(patched.as_bytes()).unwrap();
        cp_brotli(&jsfile, &app_dir.join("bindgen.js"), false, config.brotli)?;

        build_dir.join("bindgen_bg.wasm")
    } else {
        build_dir.join(format!("{}.wasm", build_crate))
    };

    let wasm_dest = app_dir.join(format!("{}.wasm", build_crate));
    if config.strip{
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
    if config.brotli{
        brotli_compress(&wasm_dest);
    }
    // generate html file
    let index_path = app_dir.join("index.html");
    let html = generate_html(build_crate, &config);
    fs::write(&index_path, &html.as_bytes()).map_err( | e | format!("Can't write {:?} {:?} ", index_path, e)) ?;
    if config.brotli{
        brotli_compress(&index_path);
    }
    println!("Created wasm package: {:?}", app_dir);
    println!("Copy this directory to any webserver, and serve with atleast these headers:");
    println!("Cross-Origin-Embedder-Policy: require-corp");
    println!("Cross-Origin-Opener-Policy: same-origin");
    println!("Files need to be served with these mime types: ");
    println!("*.html => text/html");
    println!("*.wasm => application/wasm");
    println!("*.css => text/css");
    println!("*.js => text/javascript");
    println!("*.ttf => application/ttf");
    println!("*.png => image/png");
    println!("*.jpg => image/jpg");
    println!("*.svg => image/svg+xml");
    Ok(WasmBuildResult{
        app_dir
    })
}


pub fn run(config:WasmConfig, args: &[String]) -> Result<(), String> {
    // we should run the compiled folder root as webserver
    let result = build(config, args)?;
    start_wasm_server(result.app_dir, config.lan, config.port.unwrap_or(8010));
    return Err("Run is not implemented yet".into());
}

pub fn start_wasm_server(root:PathBuf, lan:bool, port: u16) {
    let addr = if lan{
        SocketAddr::new("0.0.0.0".parse().unwrap(), port)
    }
    else{
        SocketAddr::new("127.0.0.1".parse().unwrap(), port)
    };
    println!("Starting webserver on {:?}", addr);
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
                    //println!("OPENING {:?}", path);
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
