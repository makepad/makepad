use crate::shell::*;
use crate::ios::{IosTarget};
use std::path::{PathBuf, Path};
use std::collections::HashSet;

pub struct PlistValues{
    identifier: String,
    display_name: String,
    name: String,
    executable: String,
    version: String,
}

impl PlistValues{
    fn to_plist_file(&self)->String{
        format!(r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
              <key>CFBundleIdentifier</key>
              <string>{0}</string>
              <key>CFBundleDisplayName</key>
              <string>{1}</string>
              <key>CFBundleName</key>
              <string>{2}</string>
              <key>CFBundleExecutable</key>
              <string>{3}</string>
              <key>CFBundleVersion</key>
              <string>{4}</string>
              <key>CFBundleShortVersionString</key>
              <string>{4}</string>
              <key>UILaunchStoryboardName</key>
              <string></string>
            </dict>
            </plist>"#,
            self.identifier,
            self.display_name,
            self.name,
            self.executable,
            self.version,
        )
        }
}

pub struct Scent{
    app_id: String,
    team_id: String,
}

impl Scent{
    fn to_scent_file(&self)->String{
        format!(r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
                <dict>
                    <key>application-identifier</key>
                    <string>{0}</string>
                    <key>com.apple.developer.team-identifier</key>
                    <string>{1}</string>
                    <key>get-task-allow</key>
                    <true/>
                    <key>keychain-access-groups</key>
                    <array>
                        <string>{0}</string>
                    </array>
                </dict>
            </plist>
        "#, self.app_id, self.team_id)
    }
}

pub struct IosBuildResult{
    app_dir: PathBuf,
    plist: PlistValues,
    dst_bin: PathBuf
}


pub fn build(app_id: &str, args: &[String], ios_target:IosTarget) -> Result<IosBuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    
    let cwd = std::env::current_dir().unwrap();
    let target_opt = format!("--target={}", ios_target.toolchain());
    
    let base_args = &[
        "run",
        "nightly",
        "cargo",
        "build",
        "--release",
        &target_opt
    ];
    
    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    for arg in args {
        args_out.push(arg);
    }
    
    shell_env(&[("MAKEPAD", "lines"),],&cwd,"rustup",&args_out) ?;
    
    // alright lets make the .app file with manifest
    let plist = PlistValues{
        identifier: app_id.to_string(),//format!("dev.makepad.{}", build_crate),
        display_name: app_id.split(".").last().unwrap().to_string(),//build_crate.to_string(),
        name: app_id.split(".").last().unwrap().to_string(),//build_crate.to_string(),
        executable: build_crate.to_string(),
        version: "1".to_string(),
    };    
    
    let app_dir = cwd.join(format!("target/makepad-ios-app/{}/release/{build_crate}.app", ios_target.toolchain()));
    mkdir(&app_dir) ?;
    
    let plist_file = app_dir.join("Info.plist");
    write_text(&plist_file, &plist.to_plist_file())?;

    let src_bin = cwd.join(format!("target/{}/release/{build_crate}", ios_target.toolchain()));
    let dst_bin = app_dir.join(build_crate.to_string());
    
    cp(&src_bin, &dst_bin, false) ?;
    
    Ok(IosBuildResult{
        app_dir, 
        plist,
        dst_bin
    })
}

pub fn run_sim(app_id: &str, args: &[String], ios_target:IosTarget) -> Result<(), String> {

    let result = build(app_id, args, ios_target)?;

    let cwd = std::env::current_dir().unwrap();
    shell_env(&[], &cwd ,"xcrun", &[
        "simctl",
        "install",
        "booted",
        &result.app_dir.into_os_string().into_string().unwrap()
    ]) ?;

    shell_env(&[], &cwd ,"xcrun", &[
        "simctl",
        "launch",
        "--console",
        "booted",
        &result.plist.identifier,
    ]) ?; 
    
    Ok(())
}

#[derive(Debug)]
struct ProvisionData{
    team: String,
    devices: Vec<String>,
    path: PathBuf,
}

fn parse_provision(path:PathBuf, app_id:&str)->Option<ProvisionData>{
    // lets find app_id in the file
    let bytes = std::fs::read(&path).unwrap();
    let app_id_bytes = app_id.as_bytes();
    for i in 0..(bytes.len() - app_id_bytes.len() + 1){
        if bytes[i..(i+app_id_bytes.len())] == *app_id_bytes{
            let team_prefix = "<key>ApplicationIdentifierPrefix</key>\n\t<array>\n\t<string>".as_bytes();
            for i in 0..(bytes.len() - team_prefix.len() + 1){
                if bytes[i..(i+team_prefix.len())] == *team_prefix{
                    let team = std::str::from_utf8(&bytes[i+team_prefix.len()..i+team_prefix.len()+10]).unwrap().to_string();
                    let device_prefix = "<key>ProvisionedDevices</key>\n\t<array>\n\t\t<string>".as_bytes();
                    let mut devices = Vec::new();
                    for i in 0..(bytes.len() - device_prefix.len() + 1) {
                        if bytes[i..(i + device_prefix.len())] == *device_prefix {
                            // loop through ProvisionedDevices array and add all uuids
                            for j in i..(bytes.len() - device_prefix.len() + 1) {
                                // break out of loop at end of array
                                let array_end = "</array>".as_bytes();
                                if bytes[j..(j + array_end.len())] == *array_end {
                                    break;
                                }   
                                let str_open = "<string>".as_bytes();
                                let mut open_idx = 0;
                                let str_close = "</string>".as_bytes();
                                let mut close_idx = 0;
                                if bytes[j..(j+str_open.len())] == *str_open {
                                    open_idx = j;
                                }
                                for k in j..(bytes.len()) {
                                    if bytes[k..(k + str_close.len())] == *str_close {
                                        close_idx = k;
                                        break;
                                    }
                                }
                                if open_idx != 0 && close_idx != 0 {
                                    let uuid = std::str::from_utf8(&bytes[open_idx + str_open.len()..close_idx]).unwrap().to_string();
                                    devices.push(uuid);
                                }
                            }
                        }
                    }

                    return Some(ProvisionData{
                        team,
                        devices,
                        path,
                    });
                }
            }
            break;
        }
    }
    None
}


fn copy_resources(app_dir: &Path, build_crate: &str) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let mut assets_to_add: Vec<String> = Vec::new();

    let build_crate_dir = get_crate_dir(build_crate) ?;
    
    let local_resources_path = build_crate_dir.join("resources");
    
    if local_resources_path.is_dir() {
        let underscore_build_crate = build_crate.replace('-', "_");
        let dst_dir = app_dir.join(format!("makepad/{underscore_build_crate}/resources"));
        mkdir(&dst_dir) ?;
        cp_all(&local_resources_path, &dst_dir, false) ?;

        let assets = ls(&dst_dir) ?;
        for path in &assets {
            let path = path.display();
            assets_to_add.push(format!("makepad/{underscore_build_crate}/resources/{path}"));
        }
    }

    let mut dependencies = HashSet::new();
    if let Ok(cargo_tree_output) = shell_env_cap(&[], &cwd, "cargo", &["tree", "-p", build_crate]) {
        for line in cargo_tree_output.lines().skip(1) {
            if let Some((name, path)) = extract_dependency_info(line) {
                let resources_path = Path::new(&path).join("resources");
                if resources_path.is_dir() {
                    dependencies.insert((name.replace('-',"_"), resources_path));
                }
            }
        }
    }

    for (name, resources_path) in dependencies.iter() {
        let dst_dir = app_dir.join(format!("makepad/{name}/resources"));
        mkdir(&dst_dir) ?;
        cp_all(resources_path, &dst_dir, false) ?;

        let assets = ls(&dst_dir) ?;
        for path in &assets {
            let path = path.display();
            assets_to_add.push(format!("makepad/{name}/resources/{path}"));
        }
    }

    Ok(())
}

pub fn run_real(signing_identity: Option<String>, provisioning_profile: Option<String>, device_uuid: Option<String>, app_id: &str, args: &[String], ios_target:IosTarget) -> Result<(), String> {
    let build_crate = get_build_crate_from_args(args)?;
    let result = build(app_id, args, ios_target)?;
    let cwd = std::env::current_dir().unwrap();

    // parse identities for code signing
    let security_result = shell_env_cap(&[], &cwd ,"security", &[
        "find-identity",
        "-v",
        "-p",
        "codesigning"])?;

    // select signing identity
    let found_identities: Vec<&str> = security_result.split("\n").collect();
    let selected_identity = if let Some(signing_identity) = &signing_identity {
        // find passed in identity in security result
        &found_identities.iter().find(|i| i.contains(signing_identity)).unwrap()[5..45]
    } else if let Some(long_hex_id) = security_result.strip_prefix("  1) ") {
        // if no argument passed, take first identity found
        &long_hex_id[0..40]
    } else {
        return Err(format!("Error reading the signing identity security result #{}#", security_result)) 
    };
    println!("Selected signing identity {}", selected_identity);
    
    let home = std::env::var("HOME").unwrap();
    let profiles = std::fs::read_dir(format!("{}/Library/MobileDevice/Provisioning Profiles/", home)).unwrap();
    let mut found_profiles = Vec::new();
    for profile in profiles {
        // lets read it
        let profile_path = profile.unwrap().path();
        if let Some(prov) = parse_provision(profile_path, app_id){
            found_profiles.push(prov);
        }
    }

    // select provisioning profile
    let provision = if let Some(provisioning_profile) = &provisioning_profile {
        // find passed in provisioning profile
        found_profiles.iter()
            .find(|i| i.path.to_str().unwrap().contains(provisioning_profile))
            .unwrap_or_else(|| 
                panic!("Provisioning profile {} not found", provisioning_profile))
    } else if found_profiles.len() > 0 {
        // if no argument passed, take first profile found
        &found_profiles[0]
    } else {
        return Err(format!("Could not find a matching mobile provision profile for name {}\nPlease create an empty app in xcode with this identifier (orgname.appname) and deploy to your mobile device once, then run this again.", app_id))
    };
    println!("Selected provisioning profile {:?}, for team {}", provision.path, provision.team);

    // select device
    let selected_device = if let Some(device_uuid) = &device_uuid {
        // find passed in device in selected profile
        provision.devices.iter()
            .find(|i| i.contains(device_uuid))
            .unwrap_or_else(|| 
                panic!("Device with UUID {} not found in provisioning profile {:?}", device_uuid, provision.path))
    } else if provision.devices.len() > 0 {
        // if no argument passed, take first device found in profile
        &provision.devices[0]
    } else {
        return Err(format!("No devices found in provisioning profile {:?}", provision.path))
    };
    println!("Selected device with UUID: {}", selected_device);
    
    // ok lets find the mobile provision for this application
    // we can also find the team ids from there to build the scent
    // and the device id as well
    let scent = Scent{
        app_id: format!("{}.{}", provision.team, app_id),
        team_id: provision.team.to_string()
    };
    
    let scent_file = cwd.join(format!("target/makepad-ios-app/{}/release/{build_crate}.scent", ios_target.toolchain()));
    write_text(&scent_file, &scent.to_scent_file())?;

    let dst_provision = result.app_dir.join("embedded.mobileprovision");
    let app_dir = result.app_dir.into_os_string().into_string().unwrap();

    cp(&provision.path, &dst_provision, false) ?;
    
    copy_resources(Path::new(&app_dir), build_crate)?;
    
    shell_env_cap(&[], &cwd ,"codesign", &[
        "--force",
        "--timestamp=none",
        "--sign",
        selected_identity, 
        &result.dst_bin.into_os_string().into_string().unwrap()
    ]) ?; 
    
    shell_env_cap(&[], &cwd ,"codesign", &[
        "--force",
        "--timestamp=none",
        "--sign", 
        selected_identity, 
        "--entitlements",
        &scent_file.into_os_string().into_string().unwrap(),
        "--generate-entitlement-der",
        &app_dir
    ]) ?;  
    
    let cwd = std::env::current_dir().unwrap();
    let ios_deploy = cwd.join(format!("{}/ios-deploy/build/Release/", env!("CARGO_MANIFEST_DIR")));
    
    // kill previous lldb
    let ps_result = shell_env_cap(&[], &ios_deploy ,"ps", &[])?;
    let lines = ps_result.lines();
    for line in lines{
        if line.contains("lldb") && line.contains("fruitstrap"){
            shell_env_cap(&[], &ios_deploy ,"kill", &["-9",line.split(" ").next().unwrap()])?;
        }
    } 
    println!("Installing application on device");
    shell_env_filter("Makepad iOS application started.", &[], &ios_deploy ,"./ios-deploy", &[
        "-i",
        &selected_device,  
        "-d",
        "-u",
        "-b",
        &app_dir
    ]) ?; 
    
    Ok(())
}
