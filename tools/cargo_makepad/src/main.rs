mod android;
mod wasm;
mod shell;
mod ios;
use android::*;
use wasm::*;
use ios::*;

fn show_help(err: &str){
    if !err.is_empty(){
        println!("{}", err);
    }
    println!("Makepad's cargo extension");
    println!("    This tool is used to configure and build makepad applications for more complex platforms");
    println!();
    println!("Usage cargo makepad [commands]");
    println!();
    println!("Wasm Commands:");
    println!();
    println!("    wasm toolchain-install                       Install the toolchain needed for wasm32 with rustup");
    println!("    wasm build <cargo args>                      Build a wasm project");
    println!("    wasm [options] run <cargo args>              Build and run a wasm project, starts a webserver at port 8080 by default");
    println!();
    println!("    [options] with its default value:");
    println!();
    println!("       --port=8080                               The port to run the wasm webserver");
    println!();
    println!("iOS Commands:");
    println!();
    println!("    ios toolchain-install                       Install the toolchain needed for wasm32 with rustup");
    println!("    ios build <cargo args>                      Build an ios project");
    println!("    ios [options] run <cargo args>              runs the ios project on the simulator");
    println!();    
    println!("Android commands:");
    println!();
    println!("    android [options] toolchain-install          Download and install the android sdk and rust toolchains");
    println!("    android [options] run <cargo args>           Run an android project on a connected android device via adb");
    println!("    android [options] build <cargo args>         Build an android project");
    println!();
    println!("    [options] with its default value:");
    println!();
    println!("       --sdk-path=./android_33_sdk               The path to read/write the android SDK");
    println!("       --host-os=<linux-x64|windows-x64|macos-aarch64|macos-x64>");
    println!("       --all-targets                             install all android targets, default only aarch64");
    println!("                                                 Host OS is autodetected but can be overridden here");
    println!("    [Android toolchain-install separated steps]");
    println!("    android [options] rustup-toolchain-install");
    println!("    android [options] download-sdk");
    println!("    android [options] expand-sdk");
    println!("    android [options] remove-sdk-sources");
    println!();
    println!("Linux commands:");
    println!();
    println!("    linux apt-get-install-makepad-deps           Call apt-get install with all dependencies needed for makepad.");
    println!();
    println!();
    }

fn main() {
    let args:Vec<String> = std::env::args().collect();

    // Skip the first argument if it's the binary path or 'cargo'
    let args = if args.len() > 1 && (args[0].ends_with("cargo-makepad") || args[0] == "cargo" || args[0].ends_with("cargo-makepad.exe")) {
        // If it's 'cargo makepad', then skip the second argument as well
        if args.len() > 2 && args[1] == "makepad" {
            args[2..].to_vec()
        } else {
            args[1..].to_vec()
        }
    } else {
        args
    };

    if args.len() <= 1 {
        return show_help("not enough arguments");
    }
    match args[0].as_ref(){
        "android" => if let Err(e) = handle_android(&args[1..]){
            println!("Got error: {}", e);
        }
        "wasm" => if let Err(e) = handle_wasm(&args[1..]){
            println!("Got error: {}", e);
        }
        "ios" => if let Err(e) = handle_ios(&args[1..]){
            println!("Got error: {}", e);
        }
        _=> show_help("not implemented yet")
    }
}