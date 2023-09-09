mod compile;
mod sdk;

#[allow(non_camel_case_types)]
pub enum IosTarget {
    aarch64,
    aarch64_sim,
    x86_64_sim,
}

impl IosTarget {
    fn from_str(opt: &str) -> Result<Vec<Self>,
    String> {
        let mut out = Vec::new();
        for opt in opt.split(","){
            match opt {
                "all_aarch64"=> return Ok(vec![IosTarget::aarch64, IosTarget::aarch64_sim]),
                "aarch64" => out.push(IosTarget::aarch64),
                "aarch64_sim" => out.push(IosTarget::aarch64_sim),
                "x86_64_sim" => out.push(IosTarget::x86_64_sim),
                x => {
                    return Err(format!("{:?} please provide a valid ABI: all_aarch64, aarch64, aarch64_sim, x86_64_sim", x))
                }
            }
        }
        return Ok(out);
    }
    
    fn toolchain(&self)->&'static str{
        match self {
            Self::aarch64 => "aarch64-apple-ios",
            Self::aarch64_sim => "aarch64-apple-ios-sim",
            Self::x86_64_sim => "x86_64-apple-ios",
        }
    }
}


pub fn handle_ios(mut  args: &[String]) -> Result<(), String> {
    let mut targets = vec![IosTarget::aarch64_sim];
    let mut package_name = None;
    let mut app_label = None;
    let mut app_id = None;
    let mut team_id = None;
    let mut device = None;
     for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--package-name=") {
            package_name = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--app-label=") {
            app_label = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--app-id=") {
            app_id = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--abi=") {
            targets = IosTarget::from_str(opt)?;
        }
        else if let Some(team) = v.strip_prefix("--team=") {
            team_id = Some(team);
        }
        else if let Some(dev) = v.strip_prefix("--device=") {
            device = Some(dev);
        }
        else {
            args = &args[i..];
            break
        }
    }
    
    
    match args[0].as_ref() {
        "toolchain-install"=>{
            sdk::rustup_toolchain_install(&targets)
        }
        "build" =>{
            compile::build(package_name, app_label, &args[1..], &targets)?;
            Ok(())
        }
        "run_real" =>{
            targets = vec![IosTarget::aarch64];
            if let Some(team_id) = team_id{
                if let Some(device) = device{
                    compile::run_real(package_name, app_label, app_id, &args[1..], &targets, team_id, device)?;
                }
                else{
                    return Err(format!("real running requires --device=<deviceid>"))
                }
            }
            else{
                return Err(format!("real running requires --team=<teamid>"))
            }
            Ok(())
        }
        "run" =>{
            compile::run(package_name, app_label, &args[1..], &targets)?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
