mod compile;
mod sdk;
use compile::WasmConfig;

pub fn handle_wasm(mut args: &[String]) -> Result<(), String> {
    let mut config = WasmConfig{
        strip: false,
        lan: false,
        brotli: false,
        port: None,
        small_fonts: false,
        bindgen: false,
    };
    
    // pull out options
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--port=") {
            config.port = Some(opt.parse::<u16>().unwrap_or(8010));
        }
        else if let Some(_) = v.strip_prefix("--strip") {
            config.strip = true;
        }
        else if let Some(_) = v.strip_prefix("--small-fonts") {
            config.small_fonts = true;
        }
        else if let Some(_) = v.strip_prefix("--brotli") {
            config.brotli = true;
        }
        else if let Some(_) = v.strip_prefix("--lan") {
            config.lan = true;
        }
        else if let Some(_) = v.strip_prefix("--bindgen") {
            config.bindgen = true;
        }
        else {
            args = &args[i..];
            break
        }
    }
    
    match args[0].as_ref() {
        "rustup-install-toolchain"=>{
            sdk::rustup_toolchain_install()
        }
        "install-toolchain"=>{
            sdk::rustup_toolchain_install()
        }
        "build" =>{
            compile::build(config, &args[1..])?;
            Ok(())
        }
        "run" =>{
            compile::run(config, &args[1..])?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
