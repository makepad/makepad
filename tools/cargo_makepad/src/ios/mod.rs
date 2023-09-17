mod compile;
mod sdk;
use compile::*;

#[allow(dead_code)]
#[allow(non_camel_case_types)]
#[derive(Copy,Clone)]
pub enum IosTarget {
    aarch64,
    aarch64_sim,
    x86_64_sim,
}

impl IosTarget {
    fn toolchain(&self)->&'static str{
        match self {
            Self::aarch64 => "aarch64-apple-ios",
            Self::aarch64_sim => "aarch64-apple-ios-sim",
            Self::x86_64_sim => "x86_64-apple-ios",
        }
    }
}


pub fn handle_ios(mut args: &[String]) -> Result<(), String> {
    let mut signing_identity  = None;
    let mut provisioning_profile = None;
    let mut device_uuid = None;
    let mut app = None;
    let mut org = None;
    let mut ios_version = None;
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--signing-identity=") {
            signing_identity = Some(opt.to_string());
        } 
        else if let Some(opt) = v.strip_prefix("--provisioning-profile=") {
            provisioning_profile = Some(opt.to_string());
        } 
        else if let Some(opt) = v.strip_prefix("--device-uuid=") {
            device_uuid = Some(opt.to_string());
        } 
        else if let Some(opt) = v.strip_prefix("--app=") {
            app = Some(opt.to_string());
        } 
        else if let Some(opt) = v.strip_prefix("--org=") {
            org = Some(opt.to_string());
        } 
        else if let Some(opt) = v.strip_prefix("--ios-version=") {
            ios_version = Some(opt.to_string());
        } 
        else {
            args = &args[i..];
            break
        }
    }

    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain"=>{
            #[cfg(target_arch = "x86_64")]
            let toolchains = vec![IosTarget::x86_64_sim, IosTarget::aarch64];
            #[cfg(target_arch = "aarch64")]
            let toolchains = vec![IosTarget::aarch64_sim, IosTarget::aarch64];
            sdk::rustup_toolchain_install(&toolchains)
        }
        "run-device" =>{
            compile::run_on_device(SigningArgs{
                signing_identity, 
                provisioning_profile, 
                device_uuid, 
                app,
                org,
                ios_version
            },&args[1..], IosTarget::aarch64)?;
            Ok(())
        }
        "run-sim" =>{
            #[cfg(target_arch = "x86_64")]
            let toolchain = IosTarget::x86_64_sim;
            #[cfg(target_arch = "aarch64")]
            let toolchain = IosTarget::aarch64_sim; 
            compile::run_on_sim(SigningArgs{
                ios_version,
                signing_identity, 
                provisioning_profile, 
                device_uuid, 
                app,
                org
            },
            &args[1..], toolchain)?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
