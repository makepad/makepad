mod compile;
mod sdk;

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


pub fn handle_ios(args: &[String]) -> Result<(), String> {
    
    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain"=>{
            #[cfg(target_arch = "x86_64")]
            let toolchains = vec![IosTarget::x86_64_sim, IosTarget::aarch64];
            #[cfg(target_arch = "aarch64")]
            let toolchains = vec![IosTarget::aarch64_sim, IosTarget::aarch64];
            sdk::rustup_toolchain_install(&toolchains)
        }
        "run-real" =>{
            compile::run_real(&args[1], &args[2..], IosTarget::aarch64)?;
            Ok(())
        }
        "run-sim" =>{
            #[cfg(target_arch = "x86_64")]
            let toolchain = IosTarget::x86_64_sim;
            #[cfg(target_arch = "aarch64")]
            let toolchain = IosTarget::aarch64_sim; 
            compile::run_sim(&args[1], &args[2..], toolchain)?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
