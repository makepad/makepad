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

pub fn build(_package_name: Option<String>, _app_label: Option<String>, args: &[String], ios_targets:&[IosTarget]) -> Result<(PathBuf, PlistValues), String> {
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
    
    Ok((app_dir, plist))
}

pub fn run(package_name: Option<String>, app_label: Option<String>, args: &[String], ios_targets:&[IosTarget]) -> Result<(), String> {

    let (app_dir, plist) = build(package_name, app_label, args, ios_targets)?;

    let cwd = std::env::current_dir().unwrap();
    shell_env(&[], &cwd ,"xcrun", &[
        "simctl",
        "install",
        "booted",
        &app_dir.into_os_string().into_string().unwrap()
    ]) ?;

    shell_env(&[], &cwd ,"xcrun", &[
        "simctl",
        "launch",
        "--console",
        "booted",
        &plist.identifier,
    ]) ?; 
    // lets run it on the simulator
    //xcrun simctl install booted target/x86_64-apple-ios/debug/examples/bundle/ios/ios-beta.app
    //xcrun simctl launch --console booted com.cacao.ios-test
    
    Ok(())
}
