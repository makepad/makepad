mod compile;
mod sdk;
use compile::*;

#[allow(dead_code)]
#[allow(non_camel_case_types)]
#[derive(Copy,Clone, Debug)]
pub enum AppleTarget {
    aarch64_ios,
    aarch64_ios_sim,
    x86_64_ios_sim,
    aarch64_tvos,
    aarch64_tvos_sim,
    x86_64_tvos_sim,
}

impl AppleTarget{
    fn needs_build_std(&self)->bool{
        match self{
            Self::aarch64_tvos |
            Self::aarch64_tvos_sim|
            Self::x86_64_tvos_sim=>true,
            _=>false 
        }
    }
    fn os(&self)->AppleOs{
        match self{
            Self::aarch64_tvos |
            Self::aarch64_tvos_sim|
            Self::x86_64_tvos_sim=>AppleOs::Tvos,
            Self::aarch64_ios |
            Self::aarch64_ios_sim|
            Self::x86_64_ios_sim=>AppleOs::Ios,
        }
    }
}

#[derive(Copy,Clone)]
pub enum AppleOs{
    Ios,
    Tvos,
}

impl AppleTarget {
    fn toolchain(&self)->&'static str{
        match self {
            Self::aarch64_ios => "aarch64-apple-ios",
            Self::aarch64_ios_sim => "aarch64-apple-ios-sim",
            Self::x86_64_ios_sim => "x86_64-apple-ios",
            Self::aarch64_tvos => "aarch64-apple-tvos",
            Self::aarch64_tvos_sim => "aarch64-apple-tvos-sim",
            Self::x86_64_tvos_sim => "x86_64-apple-tvos",
        }
    }
    fn device_target(os:AppleOs)->Self{
        match os{
            AppleOs::Ios=>Self::aarch64_ios,
            AppleOs::Tvos=>Self::aarch64_tvos
        }
    }
    fn sim_target(os:AppleOs)->Self{
        #[cfg(target_arch = "x86_64")]
        match os{
            AppleOs::Ios=>Self::x86_64_ios_sim,
            AppleOs::Tvos=>Self::x86_64_tvos_sim
        }
        #[cfg(target_arch = "aarch64")] 
        match os{
            AppleOs::Ios=>Self::aarch64_ios_sim,
            AppleOs::Tvos=>Self::aarch64_tvos_sim
        }
        
    }
}


pub fn handle_apple(args: &[String]) -> Result<(), String> {
    let mut signing_identity  = None;
    let mut provisioning_profile = None;
    let mut device_identifier = None;
    let mut app = None;
    let mut org = None;
    let mut stable = true;
    if args.len() < 1{
        return Err(format!("not enough args"))
    }
    let apple_os =match args[0].as_ref(){
        "ios"=>{
            AppleOs::Ios
        }
        "tvos"=>{
            AppleOs::Tvos
        }
        "list"=>{
            let pp = parse_profiles()?;
            pp.println();
            return Ok(())
        }
        _=>{
            return Err(format!("please enter ios or tvos"))
        }
    };
    let mut args = &args[1..];
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--cert=") {
            signing_identity = Some(opt.to_string());
        } 
        else if let Some(opt) = v.strip_prefix("--profile=") {
            provisioning_profile = Some(opt.to_string());
        } 
        else if let Some(opt) = v.strip_prefix("--device=") {
            device_identifier = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--app=") {
            app = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--org=") {
            org = Some(opt.to_string());
        }
        else if v == "--stable"{
            stable = true;
        }
        else {
            args = &args[i..];
            break
        }
    }
    
    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain"=>{
            let toolchains = vec![AppleTarget::sim_target(apple_os), AppleTarget::device_target(apple_os)];
            sdk::rustup_toolchain_install(&toolchains)
        }
        "run-device" =>{
            compile::run_on_device(AppleArgs{
                stable,
                _apple_os:apple_os,
                signing_identity, 
                provisioning_profile, 
                device_identifier, 
                app,
                org,
            },&args[1..], AppleTarget::device_target(apple_os))?;
            Ok(())
        }
        "run-sim" =>{
            compile::run_on_sim(AppleArgs{
                stable,
                _apple_os:apple_os,
                signing_identity, 
                provisioning_profile, 
                device_identifier, 
                app,
                org,
            },
            &args[1..], AppleTarget::sim_target(apple_os))?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
