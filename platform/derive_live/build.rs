use std::env;
fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    if let Ok(configs) = env::var("MAKEPAD"){
        for config in configs.split('+'){
            match config{
                "lines"=>println!("cargo:rustc-cfg=lines"), 
                _=>()
            }
        }
    }
}
