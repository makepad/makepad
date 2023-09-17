mod android;
mod wasm;
mod utils;
mod ios;
use android::*;
use wasm::*;
use ios::*;
pub use makepad_shell;

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
    println!("    wasm install-toolchain                       Install the toolchain needed for wasm32 with rustup");
    println!("    wasm build <cargo args>                      Build a wasm project");
    println!("    wasm [options] run <cargo args>              Build and run a wasm project, starts a webserver at port 8080 by default");
    println!();
    println!("    [options] with its default value:");
    println!();
    println!("       --port=8080                               The port to run the wasm webserver");
    println!();
    println!("iOS Commands:");
    println!();
    println!("    ios install-toolchain                       Install the toolchain needed for ios with rustup");
    println!("    ios --org=orgname --product=prodname run-sim <cargo args>");
    println!("                                                runs the ios project on the aarch64 simulator");
    println!("    ios --org=orgname --product=prodname run-real <cargo args>");
    println!("                                                runs the ios project on a real device");
    println!("                                                runs the ios project on a real device");
    println!(" run-real optional args: --signing-identity=");
    println!("                         --provisioning-profile=");
    println!("                         --device-uuid=");
    println!("                         --ios=17");
    println!();     
    println!("Android commands:");
    println!();
    println!("    android [options] install-toolchain          Download and install the android sdk and rust toolchains");
    println!("    android [options] run <cargo args>           Run an android project on a connected android device via adb");
    println!("    android [options] build <cargo args>         Build an android project");
    println!();
    println!("    [options] with its default value:");
    println!();
    println!("       --abi=all,x86_64,aarch64,armv7,i686       Select the target ABIs (default is aarch64). On an intel chip simulator use x86_64");
    println!("                                                 Be sure to add this also to install-toolchain");
    println!("       --package-name=\"package\"                The package name");
    println!("       --app-label=\"applabel\"                  The app label");
    println!("       --sdk-path=./android_33_sdk               The path to read/write the android SDK");
    println!("       --host-os=<linux-x64|windows-x64|macos-aarch64|macos-x64>");
    println!("                                                 Host OS is autodetected but can be overridden here");
    println!("    [Android install-toolchain separated steps]");
    println!("    android [options] rustup-install-toolchain");
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