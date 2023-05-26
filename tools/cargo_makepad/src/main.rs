mod android;
mod wasm;
mod shell;
use android::*;
use wasm::*;

fn show_help(err: &str){
    if err.len()!=0{
        println!("{}", err);
    }
    println!("Makepad's cargo extension");
    println!("    This tool is used to configure and build makepad applications for more complex platforms");
    println!("");
    println!("Usage cargo makepad [commands]");
    println!("");
    println!("Wasm Commands:");
    println!("");
    println!("    wasm toolchain-install                       Install the toolchain needed for wasm32 with rustup");
    println!("    wasm build <cargo args>                      Build a wasm project");
    println!("    wasm [options] run <cargo args>              Build and run a wasm project, starts a webserver at port 8080 by default");
    println!("");
    println!("    [options] with its default value:");
    println!("");
    println!("       --port=8080                               The port to run the wasm webserver");
    println!("");
    println!("Android commands:");
    println!("");
    println!("    android [options] toolchain-install          Download and install the android sdk and rust toolchains");
    println!("    android [options] run <cargo args>           Run an android project on a connected android device via adb");
    println!("    android [options] build <cargo args>         Build an android project");
    println!("");
    println!("    [options] with its default value:");
    println!("");
    println!("       --sdk-path=./android_33_sdk               The path to read/write the android SDK");
    println!("       --host-os=<linux-x64|windows-x64|macos-aarch64|macos-x64>");
    println!("                                                 Host OS is autodetected but can be overridden here");
    println!("");
    println!("Linux commands:");
    println!("");
    println!("    linux apt-get-install-makepad-deps           Call apt-get install with all dependencies needed for makepad.");
    println!("");
    println!("");
    }

fn main() {
   
    /*let test_args = "cargo makepad android expand-sdk";
    let args:Vec<String> = test_args.split(" ").map(|s| s.to_string()).collect();
    let args = &args[2..];*/
    let args:Vec<String> = std::env::args().collect();
    println!("{:?}", args);
    if args.len()<3{
        return show_help("not enough arguments");
    }
    let args = &args[1..];
   
    if args.len() == 0{
        return show_help("");
    }
    match args[0].as_ref(){
        "android" => if let Err(e) = handle_android(&args[1..]){
            println!("Got error: {}", e);
        }
        "wasm" => if let Err(e) = handle_wasm(&args[1..]){
            println!("Got error: {}", e);
        }
        _=> show_help("not implemented yet")
    }
}


