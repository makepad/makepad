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
            println!("Check all on {} builds", TOOLCHAINS.len()*2);
            let mut handles = Vec::new();
            let (sender, reciever) = std::sync::mpsc::channel();
            for (index,(toolchain, ty)) in TOOLCHAINS.into_iter().enumerate(){
                let toolchain = toolchain.to_string();
                let args = args.to_vec();
                let sender = sender.clone();
                let thread = std::thread::spawn(move || {
                    let result = check(&toolchain, "stable", ty, &args[1..], index);
                    let _ = sender.send(("stable",toolchain.clone(),ty, result));
                    let result = check(&toolchain, "nightly", ty, &args[1..], index);
                    let _ = sender.send(("nightly",toolchain.clone(),ty, result));
                });
                handles.push(thread);
            }
            for handle in handles{
                let _ = handle.join();
            } 
            while let Ok((branch,toolchain, ty, (stdout, stderr, success))) = reciever.try_recv(){
                if !success{
                    eprintln!("Errors found in build {} {} {:?}", toolchain, branch, ty)
                }
                if stdout.len()>0{
                    print!("{}", stdout);
                }
                if stderr.len()>0{
                    eprint!("{}", stderr)
                }
            }
            println!("All checks completed");                
            Ok(())
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
    if let BuildTy::Lib = ty{
        args_out.push("--lib");
    }
    if let BuildTy::LinuxDirect= ty{

        if branch == "stable"{
            return shell_env_cap_split(&[("MAKEPAD", "linux_direct")], &cwd, "rustup", &args_out);
        }
        else{
            return shell_env_cap_split(&[("MAKEPAD", "lines,linux_direct")], &cwd, "rustup", &args_out);
        }
    }
    else{
        if branch == "stable"{
            return shell_env_cap_split(&[("MAKEPAD", " ")], &cwd, "rustup", &args_out);
        }
        else {
            return shell_env_cap_split(&[("MAKEPAD", "lines")], &cwd, "rustup", &args_out);
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

