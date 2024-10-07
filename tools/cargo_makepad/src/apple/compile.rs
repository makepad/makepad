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

pub struct ParsedProfiles{
    profiles:Vec<ProvisionData>,
    certs: Vec<(String,String)>,
    devices: Vec<(String,String)>,
}

impl ParsedProfiles{
    fn profile(&self, v:&str)->Option<&str>{
        for profile in &self.profiles{
            if profile.uuid.starts_with(v){
                return Some(&profile.uuid)
            }
        }
        None
    }
    
    fn cert<'a>(&'a self, v:&'a str)->Option<&'a str>{
        for cert in &self.certs{
            if cert.0.starts_with(v){
                return Some(&cert.0)
            }
        }
        Some(v)
    }
    
    fn device<'a>(&'a self, v:&'a str)->Option<&'a str>{
        for device in &self.devices{
            if device.0 == v{
                return Some(&device.1)
            }
            if device.1.starts_with(v){
                return Some(&device.1)
            }
        }
        Some(v)
    }
    
    pub fn println(&self){
        println!("--------------  Provisioning profiles found: --------------");
        for prov in &self.profiles{
            println!("Hex: {}", prov.uuid);
            println!("    team: {}", prov.team_ident);
            println!("    app-identifier: {}", prov.app_identifier);
            for device in &prov.devices{
                println!("    device: {}", device);
            }
        }
        println!("\nplease set --profile=<> to the right profile unique hex string start or filename\n");
        println!("-------------- Signing certificates: --------------");
        
        for cert in &self.certs{
            println!("Hex: {}    Desc: {}", cert.0, cert.1);
        }
        println!("\nplease set --cert=<> to the right signing certificate unique hex string start\n");
        
        println!("-------------- Devices: --------------");
        for device in &self.devices{
            println!("Hex: {}   Name: {}", device.1, device.0);
        }
        println!("\nplease set --device=<> to the right device name or hex string, comma separated for multiple\n");
    }
}

pub fn parse_profiles()->Result<ParsedProfiles, String>{
    let cwd = std::env::current_dir().unwrap();
    let home_dir = std::env::var("HOME").unwrap();
    let profile_dir = format!("{}/Library/MobileDevice/Provisioning Profiles/", home_dir);
        
    let profile_files = std::fs::read_dir(profile_dir).unwrap();
    let mut profiles = Vec::new();
    for file in profile_files {
        // lets read it
        let profile_path = file.unwrap().path();
        if let Some(profile) = ProvisionData::parse(&profile_path) {
            profiles.push(profile);
        }
    }
    let mut certs = Vec::new();
    let identities = shell_env_cap(&[], &cwd, "security", &[
        "find-identity",
        "-v",
        "-p",
        "codesigning"
    ]) ?;
    for line in identities.split('\n'){
        if let Some(cert) = line.split(')').nth(1){
            if let Some(cert) = cert.trim().split(' ').next(){
                if let Some(name) = line.split('"').nth(1){
                    certs.push((cert.trim().into(), name.into()));
                }
            }
        }
    }
    
    let device_list = shell_env_cap(&[], &cwd, "xcrun", &[
        "devicectl",
        "list",
        "devices",
    ]) ?;
    let mut devices = Vec::new();
    for device in device_list.split('\n'){
        if let Some(name) = device.split_whitespace().nth(0){
            if let Some(ident) = device.split_whitespace().nth(2){
                if ident.split("-").count() == 5{
                    devices.push((name.into(), ident.into()));
                }
            }
        }
    }
    
    Ok(ParsedProfiles{
        profiles,
        certs,
        devices
    })
}
/*
pub fn list_profiles()->Result<(), String>{
    let cwd = std::env::current_dir().unwrap();
    let home_dir = std::env::var("HOME").unwrap();
    let profile_dir = format!("{}/Library/MobileDevice/Provisioning Profiles/", home_dir);
    
    let profiles = std::fs::read_dir(profile_dir).unwrap();

    println!("--------------  Scanning profiles: --------------");
   
    for profile in profiles {
        // lets read it
        let profile_path = profile.unwrap().path();
        if let Some(prov) = ProvisionData::parse(&profile_path) {
            println!("Profile: {}", prov.uuid);
            println!("    team: {}", prov.team_ident);
            println!("    app-identifier: {}", prov.app_identifier);
            for device in prov.devices{
                println!("    device: {}", device);
            }
        }
    }
    println!("please set --profile=<> to the right profile hex string start or filename\n");
    // parse identities for code signing
    println!("-------------- Scanning signing certificates: --------------");
    
    shell_env(&[], &cwd, "security", &[
        "find-identity",
        "-v",
        "-p",
        "codesigning"
    ]) ?;
    println!("please set --cert=<> to the right signing certificate hex string start\n");
            
    println!("--------------  Scanning devices identifiers: --------------");
    shell_env(&[], &cwd, "xcrun", &[
        "devicectl",
        "list",
        "devices",
    ]) ?;
    println!("please set --device=<> to the right device hex string or name, multiple comma separated without spaces: a,b,c\n");

    Ok(())
}
*/
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
            <string>23B2082</string>
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
            <string>1501</string>
            <key>DTXcodeBuild</key>
            <string>15A507</string>
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
    build_dir: PathBuf,
    plist: PlistValues,
    dst_bin: PathBuf
}


pub fn build(stable:bool, org: &str, product: &str, args: &[String], apple_target: AppleTarget) -> Result<IosBuildResult, String> {
    let build_crate = get_build_crate_from_args(args) ?;
    
    let cwd = std::env::current_dir().unwrap();
    let target_opt = format!("--target={}", apple_target.toolchain());
    
    let base_args = &[
        "run",
        if stable{"stable"}else{"nightly"}, 
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
    
    shell_env(if stable{&[("MAKEPAD", "")]}else{&[("MAKEPAD", "lines")]}, &cwd, "rustup", &args_out) ?;
    
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
    
    let build_dir = cwd.join(format!("target/{}/{profile}/", apple_target.toolchain()));
    let src_bin = cwd.join(format!("target/{}/{profile}/{build_crate}", apple_target.toolchain()));
    let dst_bin = app_dir.join(build_crate.to_string());
    
    cp(&src_bin, &dst_bin, false) ?;
    
    Ok(IosBuildResult {
        build_dir,
        app_dir,
        plist,
        dst_bin
    })
}

pub fn run_on_sim(apple_args: AppleArgs, args: &[String], apple_target: AppleTarget) -> Result<(), String> {
    if apple_args.org.is_none() || apple_args.app.is_none() {
        return Err("Please set --org=org --app=app on the commandline inbetween ios and run-sim.".to_string());
    }
    
    let result = build(apple_args.stable, &apple_args.org.unwrap_or("orgname".to_string()), &apple_args.app.unwrap_or("productname".to_string()), args, apple_target) ?;
    
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
    app_identifier: String,
    devices: Vec<String>,
    path: PathBuf,
    uuid: String,
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
    fn parse(path: &PathBuf) -> Option<ProvisionData> {
        let bytes = std::fs::read(&path).unwrap();
        let mut devices = Vec::new();
        let mut team_ident = None;
        let mut app_identifier = None;
        let mut uuid = None;
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
                                    app_identifier = Some(data);
                                    //if !data.contains(app_id) {
                                    //    return None
                                    //}
                                }
                                "UUID" => if stack.last().unwrap() == "string" {
                                    uuid = Some(data);
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
            uuid: uuid.unwrap(),
            app_identifier: app_identifier.unwrap(),
            team_ident: team_ident.unwrap(),
            path: path.clone()
        })
    }
}

fn copy_resources(app_dir: &Path, build_crate: &str, build_dir:&Path, apple_target: AppleTarget) -> Result<(), String> {
    /*let mut assets_to_add: Vec<String> = Vec::new();*/
    
    let build_crate_dir = get_crate_dir(build_crate) ?;
    
    let local_resources_path = build_crate_dir.join("resources");
    
    if local_resources_path.is_dir() {
        let underscore_build_crate = build_crate.replace('-', "_");
        let dst_dir = app_dir.join(format!("makepad/{underscore_build_crate}/resources"));
        mkdir(&dst_dir) ?;
        cp_all(&local_resources_path, &dst_dir, false) ?;
    }

    let deps = get_crate_dep_dirs(build_crate, &build_dir, apple_target.toolchain());
    for (name, dep_dir) in deps.iter() {
        let resources_path = dep_dir.join("resources");
        if resources_path.is_dir(){
            let name = name.replace("-","_");
            let dst_dir = app_dir.join(format!("makepad/{name}/resources"));
            mkdir(&dst_dir) ?;
            cp_all(&resources_path, &dst_dir, false) ?;
        }
    }
    
    Ok(())
}

pub struct AppleArgs {
    pub stable: bool,
    pub _apple_os: AppleOs,
    pub signing_identity: Option<String>,
    pub provisioning_profile: Option<String>,
    pub device_identifier: Option<String>,
    pub app: Option<String>,
    pub org: Option<String>
}

pub fn run_on_device(apple_args: AppleArgs, args: &[String], apple_target: AppleTarget) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let home_dir = std::env::var("HOME").unwrap();
    // lets parse the inputs
    let parsed = parse_profiles()?;
    
    //return Ok(());
    let profile_dir = format!("{}/Library/MobileDevice/Provisioning Profiles/", home_dir);
    
    let provision = apple_args.provisioning_profile.as_ref().and_then(
        |v|{
            if v.contains('/'){
                ProvisionData::parse(&PathBuf::from(v))
            }
            else{
                let v = parsed.profile(v).expect("cannot find provisioning profile");
                ProvisionData::parse(&PathBuf::from(format!("{}{}.mobileprovision", profile_dir, v)))
            }
            }
    );
    
    if provision.is_none() || apple_args.provisioning_profile.is_none() || apple_args.signing_identity.is_none() || apple_args.device_identifier.is_none(){
        // lets list the provisioning profiles.
        println!("Error: missing provisioning profile, signing idenity or device identifier");
        parsed.println();
        return Err("please provide missing arguments BEFORE run-device".into());
    }
    let provision = provision.unwrap();
    
    let org = apple_args.org.unwrap();
    let app = apple_args.app.unwrap();
    
    let build_crate = get_build_crate_from_args(args) ?;
    let result = build(apple_args.stable, &org, &app, args, apple_target) ?;
   
    let scent = Scent {
        app_id: format!("{}.{}.{}", provision.team_ident, org, app),
        team_id: provision.team_ident.to_string()
    };
    
    let scent_file = cwd.join(format!("target/makepad-apple-app/{}/release/{build_crate}.scent", apple_target.toolchain()));
    write_text(&scent_file, &scent.to_scent_file()) ?;
      
    let dst_provision = result.app_dir.join("embedded.mobileprovision");
    let app_dir = result.app_dir.into_os_string().into_string().unwrap();
    
    cp(&provision.path, &dst_provision, false) ?;
    
    copy_resources(Path::new(&app_dir), build_crate, &result.build_dir, apple_target) ?;
    
    let cert = parsed.cert(apple_args.signing_identity.as_ref().unwrap()).expect("cannot find signing certificate");
    
    shell_env_cap(&[], &cwd, "codesign", &[
        "--force",
        "--timestamp=none",
        "--sign",
        cert,
        &result.dst_bin.into_os_string().into_string().unwrap()
    ]) ?;
    
    shell_env_cap(&[], &cwd, "codesign", &[
        "--force",
        "--timestamp=none",
        "--sign",
        cert,
        "--entitlements",
        &scent_file.into_os_string().into_string().unwrap(),
        "--generate-entitlement-der",
        &app_dir
    ]) ?;
    
    
    
    let cwd = std::env::current_dir().unwrap();
    for device_identifier in apple_args.device_identifier.unwrap().split(","){
        let device_identifier = parsed.device(device_identifier).expect("cannot find signing device");
        let answer = shell_env_cap(&[], &cwd, "xcrun", &[
            "devicectl",
            "device",
            "install",
            "app",
            "--device",
            device_identifier,
            &app_dir
        ])?;
        //println!("TODO: We need to fish out LONGID from the answer {}", answer);
        for line in answer.split("\n"){
            if line.contains("installationURL:"){
                let path = &line[21..line.len()-1];
                shell_env(&[], &cwd, "xcrun", &[
                    "devicectl",
                    "device",
                    "process",
                    "launch",
                    "--device",
                    device_identifier,
                    path
                ])?;
                continue
            }
        }
    }

    Ok(())
}
