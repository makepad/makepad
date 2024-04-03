mod compile;
mod sdk;

pub fn handle_wasm(mut args: &[String]) -> Result<(), String> {
    
    let mut port = None;
    let mut lan = false;
    
    let mut strip = false;
    // pull out options
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--port=") {
            port = Some(opt.parse::<u16>().unwrap_or(8010));
        }
        else if let Some(_) = v.strip_prefix("--strip") {
            strip = true;
        }
        else if let Some(_) = v.strip_prefix("--lan") {
            lan = true;
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
            compile::build(strip, &args[1..])?;
            Ok(())
        }
        "run" =>{
            compile::run(lan, port.unwrap_or(8010), strip, &args[1..])?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
