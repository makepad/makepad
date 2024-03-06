use crate::makepad_shell::*;
use crate::apple::{AppleTarget, AppleOs};
use std::path::{PathBuf, Path};
use crate::utils::*;

pub struct PlistValues {
    identifier: String,
    display_name: String,
    name: String,
    executable: String,
    version: String,
}
impl PlistValues{
    fn to_plist_file(&self, os: AppleOs)->String{
        match os{
            AppleOs::Tvos=>self.to_tvos_plist_file(),
            AppleOs::Ios=>self.to_ios_plist_file()
        }
    }
    
    fn to_ios_plist_file(&self)->String{
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
    fn to_tvos_plist_file(&self)->String{
        format!(r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
                <key>BuildMachineOSBuild</key>
                <string>23A344</string>
                <key>CFBundleDevelopmentRegion</key>
                <string>en</string>
                <key>CFBundleExecutable</key>
                <string>{3}</string>
                <key>CFBundleIdentifier</key>
                <string>{0}</string>
                <key>CFBundleInfoDictionaryVersion</key>
                <string>6.0</string>
                <key>CFBundleDisplayName</key>
                <string>{1}</string>
                <key>CFBundleName</key>
                <string>{2}</string>
                <key>CFBundlePackageType</key>
                <string>APPL</string>
                <key>CFBundleShortVersionString</key>
                <string>1.0</string>
                <key>CFBundleSupportedPlatforms</key>
                <array>
                    <string>AppleTVOS</string>
                </array>
                <key>CFBundleVersion</key>
                <string>{4}</string>
                <key>DTCompiler</key>
                <string>com.apple.compilers.llvm.clang.1_0</string>
                <key>DTPlatformBuild</key>
                <string>21J351</string>
                <key>DTPlatformName</key>
                <string>appletvos</string>
                <key>DTPlatformVersion</key>
                <string>17.0</string>
                <key>DTSDKBuild</key>
                <string>21J351</string>
                <key>DTSDKName</key>
                <string>appletvos17.0</string>
                <key>DTXcode</key>
                <string>1500</string>
                <key>DTXcodeBuild</key>
                <string>15A240d</string>
                <key>LSRequiresIPhoneOS</key>
                <true/>
                <key>MinimumOSVersion</key>
                <string>17.0</string>
                <key>UIDeviceFamily</key>
                <array>
                    <integer>3</integer>
                </array>
                <key>UILaunchScreen</key>
                <dict>
                <key>UILaunchScreen</key>
                <dict/>
                </dict>
                <key>UIRequiredDeviceCapabilities</key>
                <array>
                    <string>arm64</string>
                </array>
                <key>UIUserInterfaceStyle</key>
                <string>Automatic</string>
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
                </dict>
            </plist>
        "#, self.app_id, self.team_id)
    }
}

pub struct IosBuildResult {
    app_dir: PathBuf,
    plist: PlistValues,
    dst_bin: PathBuf
}


pub fn build(org: &str, product: &str, args: &[String], apple_target: AppleTarget) -> Result<IosBuildResult, String> {
    let build_crate = get_build_crate_from_args(args) ?;
    
    let cwd = std::env::current_dir().unwrap();
    let target_opt = format!("--target={}", apple_target.toolchain());
    
    let base_args = &[
        "run",
        "nightly", 
        "cargo",
        "build",
        &target_opt,
    ];
    
    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    for arg in args {
        args_out.push(arg);
    }
    
    if apple_target.needs_build_std(){
        args_out.push("-Z");
        args_out.push("build-std=std");
    }
    
    shell_env(&[("MAKEPAD", "lines"),], &cwd, "rustup", &args_out) ?;
    
    // alright lets make the .app file with manifest
    let plist = PlistValues {
        identifier: format!("{org}.{product}").to_string(),
        display_name: product.to_string(),
        name: product.to_string(),
        executable: build_crate.to_string(),
        version: "1".to_string(),
    };
    let profile = get_profile_from_args(args);
    
    let app_dir =  cwd.join(format!("target/makepad-apple-app/{}/{profile}/{build_crate}.app", apple_target.toolchain()));
    mkdir(&app_dir) ?;
    
    let plist_file = app_dir.join("Info.plist");
    write_text(&plist_file, &plist.to_plist_file(apple_target.os())) ?;
    
    let src_bin = cwd.join(format!("target/{}/{profile}/{build_crate}", apple_target.toolchain()));
    let dst_bin = app_dir.join(build_crate.to_string());
    
    cp(&src_bin, &dst_bin, false) ?;
    
    Ok(IosBuildResult {
        app_dir,
        plist,
        dst_bin
    })
}

pub fn run_on_sim(apple_args: AppleArgs, args: &[String], apple_target: AppleTarget) -> Result<(), String> {
    if apple_args.org.is_none() || apple_args.app.is_none() {
        return Err("Please set --org=org --app=app on the commandline inbetween ios and run-sim.".to_string());
    }
    
    let result = build(&apple_args.org.unwrap_or("orgname".to_string()), &apple_args.app.unwrap_or("productname".to_string()), args, apple_target) ?;
    
    let cwd = std::env::current_dir().unwrap();
    shell_env(&[], &cwd, "xcrun", &[
        "simctl",
        "install",
        "booted",
        &result.app_dir.into_os_string().into_string().unwrap()
    ]) ?;
    
    shell_env(&[], &cwd, "xcrun", &[
        "simctl",
        "launch",
        "--console",
        "booted",
        &result.plist.identifier,
    ]) ?;
    
    Ok(())
}

#[derive(Debug)]
struct ProvisionData {
    team_ident: String,
    devices: Vec<String>,
    path: PathBuf,
}



struct XmlParser<'a> {
    data: &'a[u8],
    pos: usize,
}

#[derive(Debug)]
enum XmlResult {
    OpenTag(String),
    CloseTag(String),
    SelfCloseTag,
    Data(String),
}

impl<'a> XmlParser<'a> {
    fn new(data: &'a[u8]) -> Self {
        Self {
            data,
            pos: 0
        }
    }
    fn next(&mut self) -> Result<XmlResult, ()> {
        // consume all whitespaces
        #[derive(Debug)]
        enum State {
            WhiteSpace,
            TagName(bool, bool, usize),
            Data(usize),
        }
        let mut state = State::WhiteSpace;
        while self.pos < self.data.len() {
            match state {
                State::WhiteSpace => {
                    if self.data[self.pos] == ' ' as u8 || self.data[self.pos] == '\t' as u8 || self.data[self.pos] == '\n' as u8 {
                        self.pos += 1;
                    }
                    else if self.data[self.pos] == '<' as u8 {
                        self.pos += 1;
                        state = State::TagName(false, false, self.pos)
                    }
                    else {
                        state = State::Data(self.pos);
                        self.pos += 1;
                    }
                }
                State::TagName(is_close, self_closing, start) => {
                    if self.data[self.pos] == '/' as u8 {
                        if self.pos == start {
                            state = State::TagName(true, false, start + 1);
                        }
                        else {
                            state = State::TagName(true, true, start);
                        }
                        self.pos += 1;
                    }
                    else if self.data[self.pos] == '>' as u8 {
                        let end = if self_closing {self.pos - 1}else {self.pos};
                        let name = std::str::from_utf8(&self.data[start..end]).unwrap().to_string();
                        self.pos += 1;
                        if is_close {
                            if self_closing {
                                return Ok(XmlResult::SelfCloseTag)
                            }
                            else {
                                return Ok(XmlResult::CloseTag(name))
                            }
                        }
                        else {
                            return Ok(XmlResult::OpenTag(name))
                        }
                    }
                    else {
                        self.pos += 1;
                    }
                }
                State::Data(start) => {
                    if self.data[self.pos] == '<' as u8 {
                        let body = std::str::from_utf8(&self.data[start..self.pos]).unwrap().to_string();
                        return Ok(XmlResult::Data(body))
                    }
                    else {
                        self.pos += 1;
                    }
                }
                
            }
        }
        Err(())
    }
}
impl ProvisionData {
    fn parse(path: &PathBuf, app_id: &str) -> Option<ProvisionData> {
        let bytes = std::fs::read(&path).unwrap();
        let mut devices = Vec::new();
        let mut team_ident = None;
        fn find_entitlements(bytes: &[u8]) -> Option<&[u8]> {
            let head = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">";
            let start_bytes = head.as_bytes();
            for i in 0..(bytes.len() - start_bytes.len() + 1) {
                if bytes[i..(i + start_bytes.len())] == *start_bytes {
                    return Some(&bytes[i + start_bytes.len()..])
                }
            }
            None
        }
        
        if let Some(xml) = find_entitlements(&bytes) {
            let mut xml_parser = XmlParser::new(xml);
            let mut stack = Vec::new();
            let mut last_key = None;
            while let Ok(xml) = xml_parser.next() {
                //println!("{:?}", xml);
                match xml {
                    XmlResult::SelfCloseTag => {}
                    XmlResult::OpenTag(tag) => {
                        stack.push(tag);
                    }
                    XmlResult::CloseTag(tag) => {
                        if stack.pop().unwrap() != tag {
                            println!("ProvisionData parsing failed xml tag mismatch {}", tag);
                        }
                        if stack.len() == 0 {
                            break;
                        }
                    }
                    XmlResult::Data(data) => {
                        if stack.last().unwrap() == "key" {
                            last_key = Some(data);
                        }
                        else if let Some(last_key) = &last_key {
                            match last_key.as_ref() {
                                "ProvisionedDevices" => if stack.last().unwrap() == "string" {
                                    devices.push(data);
                                }
                                "com.apple.developer.team-identifier" => if stack.last().unwrap() == "string" {
                                    team_ident = Some(data);
                                }
                                "TeamIdentifier" => if stack.last().unwrap() == "string" {
                                    team_ident = Some(data);
                                }
                                "application-identifier" => if stack.last().unwrap() == "string" {
                                    if !data.contains(app_id) {
                                        return None
                                    }
                                }
                                _ => ()
                            }
                        }
                    }
                }
            }
        }
        if team_ident.is_none(){
            return None
        }
        Some(ProvisionData {
            devices,
            team_ident: team_ident.unwrap(),
            path: path.clone()
        })
    }
}

fn copy_resources(app_dir: &Path, build_crate: &str) -> Result<(), String> {
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

    let resources = get_crate_resources(build_crate);
    for (name, resources_path) in resources.iter() {
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

pub struct AppleArgs {
    pub apple_os: AppleOs,
    pub signing_identity: Option<String>,
    pub provisioning_profile: Option<String>,
    pub device_uuid: Option<String>,
    pub org: Option<String>,
    pub org_id: Option<String>,
    pub app: Option<String>
}

pub fn run_on_device(apple_args: AppleArgs, args: &[String], apple_target: AppleTarget) -> Result<(), String> {
    
    if apple_args.org.is_none() || apple_args.app.is_none() {
        return Err("Please set --org=org --app=app on the commandline inbetween ios and run-device, these are the product name and organisation name from the xcode app you deployed to create the keys.".to_string());
    }
    let org = apple_args.org.unwrap();
    let app = apple_args.app.unwrap();
    
    let build_crate = get_build_crate_from_args(args) ?;
    let result = build(&org, &app, args, apple_target) ?;
    let cwd = std::env::current_dir().unwrap();
    
    // parse identities for code signing
    let security_result = shell_env_cap(&[], &cwd, "security", &[
        "find-identity",
        "-v",
        "-p",
        "codesigning"
    ]) ?;
    
    // select signing identity
    let found_identities: Vec<&str> = security_result.split("\n").collect();
    let selected_identity = if let Some(signing_identity) = &apple_args.signing_identity {
        // find passed in identity in security result
        &found_identities.iter().find( | i | i.contains(signing_identity)).unwrap()[5..45]
    } else if let Some(long_hex_id) = security_result.strip_prefix("  1) ") {
        // if no argument passed, take first identity found
        &long_hex_id[0..40]
    } else {
        return Err(format!("Error reading the signing identity security result #{}#", security_result))
    };
    //println!("Selected signing identity {}", selected_identity);
    
    let home = std::env::var("HOME").unwrap();
    let profiles = std::fs::read_dir(format!("{}/Library/MobileDevice/Provisioning Profiles/", home)).unwrap();
    
    
    let mut found_profiles = Vec::new();
    for profile in profiles {
        // lets read it
        let profile_path = profile.unwrap().path();
        if let Some(prov) = ProvisionData::parse(&profile_path, &format!("{org}.{app}")) {
            found_profiles.push(prov);
        }
        else if let Some(prov) = ProvisionData::parse(&profile_path, &format!("{}.", apple_args.org_id.clone().expect("Please set --org-id=<ID> to assist in finding the profile\nYou can lookt it up in the files in ~/Library/MobileDevice/Provisioning Profiles/"))) {
            found_profiles.push(prov);
        }
    }
    
    // select provisioning profile
    let provision = if let Some(provisioning_profile) = &apple_args.provisioning_profile {
        // find passed in provisioning profile
        found_profiles.iter()
            .find( | i | i.path.to_str().unwrap().contains(provisioning_profile))
            .unwrap_or_else( ||
            panic!("Provisioning profile {} not found", provisioning_profile)
        )
    } else if found_profiles.len() > 0 {
        // if no argument passed, take first profile found
        &found_profiles[0]
    } else {
        return Err(format!("Could not find a matching mobile provision profile for name {org}.{app}\nPlease create an empty app in xcode with this identifier (orgname.appname) and deploy to your mobile device once, then run this again."))
    };
    //println!("Selected provisioning profile {:?}, for team_ident {}", provision.path, provision.team_ident);
    
    // select device
    let selected_device = if let Some(device_uuid) = &apple_args.device_uuid {
        // find passed in device in selected profile
        provision.devices.iter()
            .find( | i | i.contains(device_uuid))
            .unwrap_or_else( ||
            panic!("Device with UUID {} not found in provisioning profile {:?}", device_uuid, provision.path)
        )
    } else if provision.devices.len() > 0 {
        // if no argument passed, take first device found in profile
        &provision.devices[0]
    } else {
        return Err(format!("No devices found in provisioning profile {:?}", provision.path))
    };
    //println!("Selected device with UUID: {}", selected_device);
    
    // ok lets find the mobile provision for this application
    // we can also find the team ids from there to build the scent
    // and the device id as well
    let scent = Scent {
        app_id: format!("{}.{}.{}", provision.team_ident, org, app),
        team_id: provision.team_ident.to_string()
    };
    
    let scent_file = cwd.join(format!("target/makepad-apple-app/{}/release/{build_crate}.scent", apple_target.toolchain()));
    write_text(&scent_file, &scent.to_scent_file()) ?;
      
    let dst_provision = result.app_dir.join("embedded.mobileprovision");
    let app_dir = result.app_dir.into_os_string().into_string().unwrap();
    
    cp(&provision.path, &dst_provision, false) ?;
    
    copy_resources(Path::new(&app_dir), build_crate) ?;
    
    shell_env_cap(&[], &cwd, "codesign", &[
        "--force",
        "--timestamp=none",
        "--sign",
        selected_identity,
        &result.dst_bin.into_os_string().into_string().unwrap()
    ]) ?;
    
    shell_env_cap(&[], &cwd, "codesign", &[
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

    let answer = shell_env_cap(&[], &cwd, "xcrun", &[
        "devicectl",
        "device",
        "install",
        "app",
        "--device",
        &selected_device,
        &app_dir
    ])?;
    for line in answer.split("\n"){
        if line.contains("installationURL:"){
            let path = &line[21..line.len()-1];
            shell_env(&[], &cwd, "xcrun", &[
                "devicectl",
                "device",
                "process",
                "launch",                    
                "--device",
                &selected_device,
                path
            ])?;
            return Ok(())
        }
    }
    println!("TODO: We need to fish out LONGID from the answer {}", answer);

    Ok(())
}
