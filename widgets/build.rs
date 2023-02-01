use std::env;
fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    if let Ok(configs) = env::var("MAKEPAD"){
        for config in configs.split("+"){
            match config{
                "ide_widgets"=>println!("cargo:rustc-cfg=ide_widgets"), 
                _=>()
            }
        }
    }
}
