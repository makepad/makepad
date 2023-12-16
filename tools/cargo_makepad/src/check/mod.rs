use crate::makepad_shell::*;
use crate::utils::*;
use makepad_toml_parser::*;

#[derive(Copy, Clone, Debug)]
enum BuildTy{
    Binary,
    BinaryBuildStd,
    Lib, 
    LinuxDirect
}

impl BuildTy{
    fn nightly_only(&self)->bool{
        match self{
            Self::Binary=>false,
            Self::BinaryBuildStd=>true,
            Self::Lib=>false,
            Self::LinuxDirect=>false,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Platform{
    Web,
    Mobile, 
    Desktop,
    Embedded
}

const TOOLCHAINS:[(&'static str,BuildTy, Platform);16]=[
    ("aarch64-apple-darwin",BuildTy::Binary, Platform::Desktop),
    ("x86_64-pc-windows-msvc",BuildTy::Binary, Platform::Desktop),
    ("x86_64-unknown-linux-gnu",BuildTy::Binary, Platform::Desktop),
    ("x86_64-unknown-linux-gnu",BuildTy::LinuxDirect, Platform::Embedded),
    ("wasm32-unknown-unknown",BuildTy::Lib, Platform::Web),
    ("aarch64-linux-android",BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-ios",BuildTy::Binary, Platform::Mobile),
    ("x86_64-linux-android",BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-tvos",BuildTy::BinaryBuildStd, Platform::Mobile),
    ("aarch64-apple-tvos-sim",BuildTy::BinaryBuildStd, Platform::Mobile),
    //("arm-linux-androideabi",1),
    ("i686-linux-android",BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-ios-sim",BuildTy::Binary, Platform::Mobile),
    ("x86_64-apple-ios",BuildTy::Binary, Platform::Mobile),
    ("x86_64-apple-tvos",BuildTy::BinaryBuildStd, Platform::Mobile),
    ("x86_64-apple-darwin",BuildTy::Binary, Platform::Desktop),
    ("x86_64-pc-windows-gnu",BuildTy::Binary, Platform::Desktop),
];

pub fn check_crate(build_crate:&str, args: &[String])->Result<(),String>{
    
    let crate_dir = get_crate_dir(build_crate).expect("Cant find crate dir");
    
    // lets parse the toml
    let cargo_str = std::fs::read_to_string(&crate_dir.join("Cargo.toml")).expect("Cant find cargo.toml");
    let toml = makepad_toml_parser::parse_toml(&cargo_str).expect("Cant parse Cargo.toml");
    let platforms = if let Some(Toml::Str(ver, _)) = toml.get("package.metadata.makepad-check-platform") {
        ver.to_string()
    }
    else{
        "desktop,web,mobile".to_string()
    };
    let nightly_only = if let Some(Toml::Bool(ver, _)) = toml.get("package.metadata.makepad-check-nightly-only") {
        *ver
    }
    else{
        false
    };
    let mut platform_filter = Vec::new();
    for platform in platforms.split(","){
        let platform = platform.trim();
        match platform{
            "desktop"=>platform_filter.push(Platform::Desktop),
            "mobile"=>platform_filter.push(Platform::Mobile),
            "web"=>platform_filter.push(Platform::Web),
            "embedded"=>platform_filter.push(Platform::Embedded),
            e=>return Err(format!("Unexpected platform in makepad-check-platform {e}"))
        }
    }
    let mut count = 0;
    for (_toolchain, ty, platform) in TOOLCHAINS{
        if !platform_filter.contains(&platform){
            continue;
        }
        if nightly_only || ty.nightly_only(){
            count += 1;
        }
        else{
            count += 2;
        }
    }
                
    println!("Check all for {} on {} builds", build_crate, count);
    let mut handles = Vec::new();
    let (sender, reciever) = std::sync::mpsc::channel();
    for (index,(toolchain, ty, platform)) in TOOLCHAINS.into_iter().enumerate(){
        if !platform_filter.contains(&platform){
            continue;
        }
        let toolchain = toolchain.to_string();
        let args = args.to_vec();
        let sender = sender.clone();
        let thread = std::thread::spawn(move || { 
            if !nightly_only && !ty.nightly_only(){
                let result = check(&toolchain, "stable", ty, &args, index);
                let _ = sender.send(("stable",toolchain.clone(),ty, result));
            }
            let result = check(&toolchain, "nightly", ty, &args, index);
            let _ = sender.send(("nightly",toolchain.clone(),ty, result));
        });
        handles.push(thread);
    }
    for handle in handles{
        let _ = handle.join();
    } 
    let mut has_errors = false;
    while let Ok((branch,toolchain, ty, (stdout, stderr, success))) = reciever.try_recv(){
        if !success{
            has_errors = true;
            eprintln!("Errors found in build {} {} {:?}", toolchain, branch, ty)
        }
        if stdout.len()>0{
            if stdout.contains("warning"){
                print!("{}", stdout);
            }
        }
        if !success && stderr.len()>0{
            eprint!("{}", stderr)
        }
    }
    if has_errors{
        println!("Errors found whilst checking");                
        Err("Errors found whilst checking".to_string())
    }
    else{
        println!("All checks completed successfully");                
        Ok(())
    }
}

pub fn handle_check(args: &[String]) -> Result<(), String> {
    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain"=>{
            // lets install all toolchains we support
            rustup_toolchain_install()
        }
        "all" =>{
            let cwd = std::env::current_dir().unwrap();
            if let Ok(build_crate) = get_build_crate_from_args(&args[1..]){
                return check_crate(&build_crate, &args[1..]);
            }
            else if let Err(e) = shell_env_cap(&[], &cwd, "cargo", &["run", "--bin"]) {
                let mut after_av = false;
                for line in e.split("\n") {
                    if after_av {
                        let binary = line.trim().to_string();
                        if binary.len()>0 {
                            let mut args = args[1..].to_vec();
                            args.insert(0,binary.to_string());
                            args.insert(0,"-p".to_string());
                            check_crate(&binary, &args)?;
                        }
                    }
                    if line.contains("Available binaries:") {
                        after_av = true;
                    }
                }
                return Ok(())
            }
            else{
                return Err("No crate to check".to_string())
            }
        }
        _=>{
            return Err("Unknown command".to_string())
        }
    }
}

fn check(toolchain:&str, branch:&str, ty:BuildTy, args: &[String], par:usize) -> (String, String, bool) {
    
    let toolchain = format!("--target={}", toolchain);
    
    let base_args = &[
        "run",
        branch,
        "cargo",
        "check",
        &toolchain,
    ];                
    let cwd = std::env::current_dir().unwrap();
    
    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    for arg in args {
        args_out.push(arg);
    }
    let target_dir = format!("--target-dir=target/check_all/check{}", par);
    args_out.push(&target_dir);
    match ty{
        BuildTy::Binary=>{
            if branch == "stable"{
                return shell_env_cap_split(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out);
            }
            else {
                return shell_env_cap_split(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out);
            }                
        },
        BuildTy::BinaryBuildStd=>{
            args_out.push("-Z");
            args_out.push("build-std=std");
            if branch == "stable"{
                return shell_env_cap_split(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out);
            }
            else {
                return shell_env_cap_split(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out);
            }                            
        },
        BuildTy::Lib=>{
            args_out.push("--lib");
            if branch == "stable"{
                return shell_env_cap_split(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out);
            }
            else {
                return shell_env_cap_split(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out);
            }    
        }, 
        BuildTy::LinuxDirect=>{
            if branch == "stable"{
                return shell_env_cap_split(&[("MAKEPAD", "linux_direct")], &cwd, "rustup", &args_out);
            }
            else{
                return shell_env_cap_split(&[("MAKEPAD", "lines,linux_direct")], &cwd, "rustup", &args_out);
            }         
        }
    }
}

fn rustup_toolchain_install() -> Result<(), String> {
    println!("Installing Rust toolchains for wasm");
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "update",
    ]) ?;
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;
    for (toolchain, ty, _platform) in TOOLCHAINS{
        if let BuildTy::BinaryBuildStd = ty{
            //TODO fix this better
            let _= shell_env_cap(&[],&std::env::current_dir().unwrap(), "rustup", &[
                "target",
                "add",
                toolchain,
                "--toolchain",
                "nightly"
            ]);
            let _= shell_env_cap(&[],&std::env::current_dir().unwrap(), "rustup", &[
                "target",
                "add",
                toolchain,
                "--toolchain",
                "stable"
            ]);
        }
        else{
            shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
                "target",
                "add",
                toolchain,
                "--toolchain",
                "nightly"
            ]) ?;
            shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
                "target",
                "add",
                toolchain,
                "--toolchain",
                "stable"
            ]) ?;
        }
    }
    Ok(())
}

