mod compile;
mod sdk;

pub fn handle_wasm(args: &[String]) -> Result<(), String> {
    
    match args[0].as_ref() {
        "rustup-toolchain-install"=>{
            sdk::rustup_toolchain_install()
        }
        "toolchain-install"=>{
            sdk::rustup_toolchain_install()
        }
        "build" =>{
            compile::build(&args[1..])?;
            Ok(())
        }
        "run" =>{
            compile::run(&args[1..])
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
