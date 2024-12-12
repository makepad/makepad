mod android;
mod wasm;
mod utils;
mod apple;
mod check;
mod open_harmony;

use android::*;
use open_harmony::*;
use wasm::*;
use apple::*;
use check::*;
pub use makepad_shell;
pub use makepad_http;
pub use makepad_wasm_strip;

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
    println!("    wasm [options] run <cargo args>              Build and run a wasm project, starts a webserver at port 8010");
    println!();
    println!("    [options] with its default value:");
    println!();
    println!("       --port=8010                               The port to run the wasm webserver");
    println!("       --lan                                     Bind the webserver to your lan ip");
    println!("       --strip                                   Strip the wasm file of debug symbols");
    println!("       --brotli                                  Use brotli to compress the wasm file");
    println!("       --bindgen                                 Enable wasm-bindgen compatibility");
    println!();
    println!("Apple iOS/TVOs Commands:");
    println!();
    println!("    apple <ios|tvos> install-toolchain           Install the toolchain needed with rustup");
    println!("    apple <ios|tvos> --org=x --app=x run-sim <cargo args>    runs the project on the aarch64 simulator");
    println!("    apple <ios|tvos> --org=x --app=x run-device <cargo args>   runs the project on a real device");
    println!("    apple list                                   Lists all certificates/profiles/devices");
    println!(" in order for makepad to be able to install an ios application on a real device a provisioning");
    println!(" profile is needed. To create one make an empty application in xcode and give it an organisation");
    println!(" name and product name you copy exactly and without spaces/odd characters into --org=x and --app=x");
    println!("                         --stable use stable compiler");
    println!(" Also run it on the device it at least once, so the profile is created");
    println!("                         --org=organisation_name");
    println!("                         --app=product_name");
    println!(" If you have multiple signing identities or devices or provision profiles you might have to set it explicitly");
    println!("                         --profile=");
    println!("                         --cert=");
    println!("                         --device=");
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
    println!("       --full-ndk                                Install the full NDK prebuilts for the selected Host OS (default is a minimal subset).");
    println!("                                                 This is required for building apps that compile native code as part of the Rust build process.");
    println!("       --keep-sdk-sources                        Keep downloaded SDK source files (default is to remove them).");
    println!("       --host-os=<linux-x64|windows-x64|macos-aarch64|macos-x64>");
    println!("                                                 Host OS is autodetected but can be overridden here");
    println!("    [Android install-toolchain separated steps]");
    println!("    android [options] rustup-install-toolchain");
    println!("    android [options] download-sdk");
    println!("    android [options] expand-sdk");
    println!("    android [options] remove-sdk-sources");
    println!();
    println!("Open Harmony commands:");
    println!();
    println!("    ohos [options] install-toolchain             Install the toolchain needed with rustup");
    println!("    ohos [options] deveco <cargo args>           Create a DevEco project for Open Harmony OS");
    println!("    ohos [options] build <cargo args>            Build  DevEco project and output the Hap package for the Open Harmony OS");
    println!("    ohos [options] run <cargo args>              Run the Hap package on a open harmony device via hdc");
    println!("    ohos [options] hilog <cargo args>            Get hilog from open harmony device via hdc");
    println!("    ohos [options] cdylib <cargo args>           Build makepad shared library only");
    println!("");
    println!("    [options]:");
    println!("");
    println!("       --deveco-home=\"deveco_path\"             The path of DevEco program, this parameter can also be specified by environment variable \"DEVECO_HOME\"");
    println!("       --remote=\"<hdcip:port>\"                 Remote hdc service, this parameter can also be specified by environment variable \"HDC_REMOTE\"");
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
        "apple" => if let Err(e) = handle_apple(&args[1..]){
            println!("Got error: {}", e);
        }
        "ohos" => if let Err(e) = handle_open_harmony(&args[1..]){
            println!("Got error: {}", e);
        }
        "check" => if let Err(e) = handle_check(&args[1..]){
            println!("Got error: {}", e);
        }
        _=> show_help("not implemented yet")
    }
}