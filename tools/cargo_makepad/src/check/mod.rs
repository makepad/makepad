use crate::makepad_shell::*;

#[derive(Copy, Clone, Debug)]
enum BuildTy{
    Binary,
    Lib, 
    LinuxDirect
}

const TOOLCHAINS:[(&'static str,BuildTy);13]=[
    ("aarch64-apple-darwin",BuildTy::Binary),
    ("x86_64-pc-windows-msvc",BuildTy::Binary),
    ("x86_64-unknown-linux-gnu",BuildTy::Binary),
    ("x86_64-unknown-linux-gnu",BuildTy::LinuxDirect),
    ("wasm32-unknown-unknown",BuildTy::Lib),
    ("aarch64-linux-android",BuildTy::Lib),
    ("aarch64-apple-ios",BuildTy::Binary),
    ("x86_64-linux-android",BuildTy::Lib),
    //("arm-linux-androideabi",1),
    ("i686-linux-android",BuildTy::Lib),
    ("aarch64-apple-ios-sim",BuildTy::Binary),
    ("x86_64-apple-ios",BuildTy::Binary),
    ("x86_64-apple-darwin",BuildTy::Binary),
    ("x86_64-pc-windows-gnu",BuildTy::Binary),
];

pub fn handle_check(args: &[String]) -> Result<(), String> {
    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain"=>{
            // lets install all toolchains we support
            rustup_toolchain_install()
        }
        "all" =>{
            for (index,(toolchain, ty)) in TOOLCHAINS.iter().enumerate(){
                println!("Running check [{}/{}] on {} stable {:?} ", index*2+1, TOOLCHAINS.len()*2, toolchain, ty);
                check(toolchain, "stable", *ty, &args[1..])?;
                println!("Running check [{}/{}] on {} nightly {:?}", index*2+2, TOOLCHAINS.len()*2, toolchain, ty);
                check(toolchain, "nightly", *ty, &args[1..])?;
            }
            println!("All checks completed");                
            Ok(())
        }
        _=>{
            return Err("Unknown command".to_string())
        }
    }
}

fn check(toolchain:&str, branch:&str, ty:BuildTy, args: &[String]) -> Result<(), String> {
    
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

    if let BuildTy::Lib = ty{
        args_out.push("--lib");
    }
    if let BuildTy::LinuxDirect= ty{

        if branch == "stable"{
            shell_env(&[("MAKEPAD", "linux_direct")], &cwd, "rustup", &args_out)?;
        }
        else if branch == "nightly"{
            shell_env(&[("MAKEPAD", "lines,linux_direct")], &cwd, "rustup", &args_out)?;
        }
    }
    else{
        if branch == "stable"{
            shell_env(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out)?;
        }
        else if branch == "nightly"{
            shell_env(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out)?;
        }
    }
    Ok(())
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
    for (toolchain, _is_lib) in TOOLCHAINS{
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
    Ok(())
}

