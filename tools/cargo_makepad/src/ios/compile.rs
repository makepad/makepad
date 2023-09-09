use crate::shell::*;
use crate::ios::{IosTarget};
use std::path::{PathBuf};

pub struct PlistValues{
    identifier: String,
    display_name: String,
    name: String,
    executable: String,
    version: String,
    development_region: String,
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
              <key>CFBundleDevelopmentRegion</key>
              <string>{5}</string>
              <key>UILaunchStoryboardName</key>
              <string></string>
              <key>LSRequiresIPhoneOS</key>
              <true/>
            </dict>
            </plist>"#,
            self.identifier,
            self.display_name,
            self.name,
            self.executable,
            self.version,
            self.development_region,
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


pub fn build(_package_name: Option<String>, _app_label: Option<String>, args: &[String], ios_targets:&[IosTarget]) -> Result<IosBuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    //let underscore_build_crate = build_crate.replace('-', "_");

    let cwd = std::env::current_dir().unwrap();
    for target in ios_targets{
        let target_opt = format!("--target={}", target.toolchain());
        
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
    }
    
    // alright lets make the .app file with manifest
    let plist = PlistValues{
        identifier: format!("dev.makepad.{}", build_crate),
        display_name: build_crate.to_string(),
        name: build_crate.to_string(),
        executable: build_crate.to_string(),
        version: "0.4.0-beta2".to_string(),
        development_region: "en_US".to_string(),
    };    
    
    let app_dir = cwd.join(format!("target/makepad-ios-app/{}/release/{build_crate}.app", ios_targets[0].toolchain()));
    mkdir(&app_dir) ?;
    
    let plist_file = app_dir.join("Info.plist");
    write_text(&plist_file, &plist.to_plist_file())?;

    let src_bin = cwd.join(format!("target/{}/release/{build_crate}", ios_targets[0].toolchain()));
    let dst_bin = app_dir.join(build_crate.to_string());
    
    cp(&src_bin, &dst_bin, false) ?;
    
    Ok(IosBuildResult{
        app_dir, 
        plist,
        dst_bin
    })
}

pub fn run(package_name: Option<String>, app_label: Option<String>, args: &[String], ios_targets:&[IosTarget]) -> Result<(), String> {

    let result = build(package_name, app_label, args, ios_targets)?;

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
    // lets run it on the simulator
    //xcrun simctl install booted target/x86_64-apple-ios/debug/examples/bundle/ios/ios-beta.app
    //xcrun simctl launch --console booted com.cacao.ios-test
    
    Ok(())
}

pub fn run_real(package_name: Option<String>, app_label: Option<String>, app_id: Option<String>,args: &[String], ios_targets:&[IosTarget], team_id:&str, device:&str) -> Result<(), String> {
    let build_crate = get_build_crate_from_args(args)?;
    let result = build(package_name, app_label, args, ios_targets)?;
    let cwd = std::env::current_dir().unwrap();
    // ok lets parse these things out
    let long_hex_id = shell_env_cap(&[], &cwd ,"security", &[
        "find-identity",
        "-v",
        "-p",
        "codesigning"])?;
        
    let long_hex_id = if let Some(long_hex_id) = long_hex_id.strip_prefix("  1) ") {
        &long_hex_id[0..40]
    }
    else{
       return Err(format!("Error parsing the security result #{}#", long_hex_id)) 
    };

    let scent = Scent{
        app_id: app_id.unwrap_or(format!("{}.dev.makepad", team_id)),
        team_id: team_id.to_string()
    };
    
    let scent_file = cwd.join(format!("target/makepad-ios-app/{}/release/{build_crate}.plist", ios_targets[0].toolchain()));
    write_text(&scent_file, &scent.to_scent_file())?;

    let src_provision = cwd.join("embedded.mobileprovision");
    let dst_provision = result.app_dir.join("embedded.mobileprovision");
    
    cp(&src_provision, &dst_provision, false) ?;
    
    shell_env(&[], &cwd ,"codesign", &[
        "--force",
        "--timestamp=none",
        "--sign",
        long_hex_id,
        &result.dst_bin.into_os_string().into_string().unwrap()
    ]) ?; 
    
    let app_dir = result.app_dir.into_os_string().into_string().unwrap();
    
    shell_env(&[], &cwd ,"codesign", &[
        "--force",
        "--timestamp=none",
        "--sign",
        long_hex_id,
        "--entitlements",
        &scent_file.into_os_string().into_string().unwrap(),
        "--generate-entitlement-der",
        &app_dir
    ]) ?; 
    
    shell_env(&[], &cwd ,"./ios-deploy", &[
        "-i",
        device,
        "-b",
        &app_dir
    ]) ?; 
    
    Ok(())
}
