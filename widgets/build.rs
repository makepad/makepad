use std::env;
use std::fs;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let cargo_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let resources_dir = format!("{}/resources", cargo_dir);
    let out_dir = env::var("OUT_DIR").unwrap();

    let resources = fs::read_dir(&resources_dir).unwrap();
    for file in resources {
        let file_path = file.unwrap().path();
        println!("cargo:rerun-if-changed={}", file_path.display());
    }

    match target_os.as_str(){
        "android"=>{
            use std::process::Command;

            let result = Command::new("rm")
                .args(["-rf", &format!("{}/resources", out_dir)])
                .status().unwrap().success();

            if !result {
                panic!("Error cleaning resources directory");
            };

            let result = Command::new("mkdir")
                .arg(&format!("{}/resources", out_dir))
                .status().unwrap().success();

            if !result {
                panic!("Error creating resources directory");
            };

            let resources = fs::read_dir(&resources_dir).unwrap();
            for file in resources {
                let file_path = file.unwrap().path();
                let result = Command::new("cp")
                    .arg(file_path.display().to_string())
                    .arg(&format!("{}/resources", out_dir))
                    .status().unwrap().success();

                if !result {
                    panic!("Error copying resource {}", file_path.display());
                };
            }
        }
        _=>()
    }
}