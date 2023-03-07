mod android;
use android::*;

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
    println!("    wasm rustup-toolchain-install                Install the toolchain needed for wasm32 with rustup");
    println!("    wasm build <cargo args>                      Build a wasm project");
    println!("    wasm host[:8080] <cargo args>                Build and hosts a wasm project, starts a webserver at port 8080 by default");
    println!("");
    println!("Android commands:");
    println!("");
    println!("    android rustup-toolchain-install             Install the toolchains needed for aarch64-linux-android with rustup");
    println!("    android [options] base-apk                   Compile the makepad base apk file with java");
    println!("    android [options] run <cargo args>           Run an android project on a connected android device via adb");
    println!("    android [options] build <cargo args>         Build an android project");
    println!("    android [options] install-sdk                Download and expand the full sdk (combines the next 2 commands)");
    println!("    android [options] download-sdk               Only download the SDK zip files from google and openJDK sources (1.6gb). Needs curl");
    println!("    android [options] expand-sdk                 Only unzip/expand the downloaded SDK");
    println!("");
    println!("    [options] with its default value:");
    println!("");
    println!("       --sdk-path=./android_33_sdk               The path to read/write the android SDK");
    println!("       --host-os=<linux-aarch64|linux-x64|windows|macos-aarch64|macos-x64>");
    println!("                                                 Host OS is autodetected but can be overridden here");
    println!("");
    println!("Linux commands:");
    println!("");
    println!("    linux apt-get-install-makepad-deps           Call apt-get install with all dependencies needed for makepad.");
    println!("");
    println!("");
    }

fn main() {
    // alright so. we have android
    //let test_args = "cargo makepad android build -p makepad-example-ironfish";
   /* let test_args = "cargo makepad android run -p makepad-example-ironfish";
    let args:Vec<String> = test_args.split(" ").map(|s| s.to_string()).collect();
    let args = &args[2..];
    */
    let args:Vec<String> = std::env::args().collect();
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
        _=> show_help("not implemented yet")
    }
}


