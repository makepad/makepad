use crate::apple::{AppleOs, AppleTarget};
use crate::makepad_shell::*;
use crate::utils::*;
use std::path::{Path, PathBuf};

const IOS_DEPLOYMENT_TARGET: &str = "15.0";

/// Resolve the cargo target directory for apple builds.
/// Defaults to `target/apple` to avoid invalidating desktop build caches.
fn cargo_target_dir(cwd: &Path) -> PathBuf {
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        if target_dir.is_absolute() {
            target_dir
        } else {
            cwd.join(target_dir)
        }
    } else {
        cwd.join("target").join("apple")
    }
}

pub struct PlistValues {
    pub identifier: String,
    pub display_name: String,
    pub name: String,
    pub executable: String,
    pub version: String,
}

pub struct ParsedProfiles {
    profiles: Vec<ProvisionData>,
    certs: Vec<(String, String)>,
    devices: Vec<(String, String)>,
}

impl ParsedProfiles {
    fn profile(&self, v: &str) -> Option<&PathBuf> {
        for profile in &self.profiles {
            if profile.uuid.starts_with(v) {
                return Some(&profile.path);
            }
        }
        None
    }

    fn cert<'a>(&'a self, v: &'a str) -> Option<&'a str> {
        for cert in &self.certs {
            if cert.0.starts_with(v) {
                return Some(&cert.0);
            }
        }
        Some(v)
    }

    fn device<'a>(&'a self, v: &'a str) -> Option<&'a str> {
        for device in &self.devices {
            if device.0 == v {
                return Some(&device.1);
            }
            if device.1.starts_with(v) {
                return Some(&device.1);
            }
        }
        Some(v)
    }

    pub fn println(&self) {
        println!("--------------  Provisioning profiles found: --------------");
        for prov in &self.profiles {
            println!("Hex: {}", prov.uuid);
            println!("    team: {}", prov.team_ident);
            println!("    app-identifier: {}", prov.app_identifier);
            for device in &prov.devices {
                println!("    device: {}", device);
            }
        }
        println!(
            "\nplease set --profile=<> to the right profile unique hex string start or filename\n"
        );
        println!("-------------- Signing certificates: --------------");

        for cert in &self.certs {
            println!("Hex: {}    Desc: {}", cert.0, cert.1);
        }
        println!(
            "\nplease set --cert=<> to the right signing certificate unique hex string start\n"
        );

        println!("-------------- Devices: --------------");
        for device in &self.devices {
            println!("Hex: {}   Name: {}", device.1, device.0);
        }
        println!("\nplease set --device=<> to the right device name or hex string, comma separated for multiple\n");
    }
}
fn load_all_provisioning_profiles() -> Vec<ProvisionData> {
    let mut profiles = Vec::new();

    let home_dir = std::env::var("HOME").unwrap();
    // < xcode 16
    let legacy_dir = format!("{}/Library/MobileDevice/Provisioning Profiles", home_dir);
    // >= xcode 16
    let new_dir = format!(
        "{}/Library/Developer/Xcode/UserData/Provisioning Profiles",
        home_dir
    );

    for dir in [legacy_dir, new_dir] {
        let profile_files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |ext| ext == "mobileprovision"))
                .collect(),
            Err(_) => continue, // skip if the directory doesn't exist
        };

        for file in profile_files {
            if let Some(profile) = ProvisionData::parse(&file) {
                profiles.push(profile);
            }
        }
    }

    profiles
}
pub fn parse_profiles() -> Result<ParsedProfiles, String> {
    let profiles = load_all_provisioning_profiles();
    let mut certs = Vec::new();
    let cwd = std::env::current_dir().unwrap();
    let identities = shell_env_cap(
        &[],
        &cwd,
        "security",
        &["find-identity", "-v", "-p", "codesigning"],
    )?;
    for line in identities.split('\n') {
        if let Some(cert) = line.split(')').nth(1) {
            if let Some(cert) = cert.trim().split(' ').next() {
                if let Some(name) = line.split('"').nth(1) {
                    certs.push((cert.trim().into(), name.into()));
                }
            }
        }
    }

    let device_list = shell_env_cap(&[], &cwd, "xcrun", &["devicectl", "list", "devices"])?;
    let mut devices = Vec::new();
    for device in device_list.split('\n') {
        if let Some(name) = device.split_whitespace().nth(0) {
            if let Some(ident) = device.split_whitespace().nth(2) {
                if ident.split("-").count() == 5 {
                    devices.push((name.into(), ident.into()));
                }
            }
        }
    }

    // Also discover devices via ios-deploy for older iOS versions (< 17)
    if let Ok(ios_deploy_list) = shell_env_cap(&[], &cwd, "ios-deploy", &["-c", "--timeout", "3", "--no-wifi"]) {
        for line in ios_deploy_list.split('\n') {
            if let Some(idx) = line.find("Found ") {
                let rest = &line[idx + "Found ".len()..];
                if let Some(end) = rest.find(' ') {
                    let udid = rest[..end].to_string();
                    // Don't add if already present (devicectl UUID format differs from UDID)
                    if !devices.iter().any(|(_, id)| id == &udid) {
                        let name = line.split("a.k.a. '").nth(1)
                            .and_then(|s| s.split('\'').next())
                            .unwrap_or("iOS Device")
                            .to_string();
                        devices.push((name, udid));
                    }
                }
            }
        }
    }

    Ok(ParsedProfiles {
        profiles,
        certs,
        devices,
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
impl PlistValues {
    fn to_plist_file(&self, os: AppleOs) -> String {
        match os {
            AppleOs::Tvos => self.to_tvos_plist_file(),
            AppleOs::Ios => self.to_ios_plist_file(),
        }
    }

    fn to_ios_plist_file(&self) -> String {
        format!(
            r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
                <key>CFBundleIdentifier</key>
                <string>{identifier}</string>
                <key>CFBundleDisplayName</key>
                <string>{display_name}</string>
                <key>CFBundleName</key>
                <string>{name}</string>
                <key>CFBundleExecutable</key>
                <string>{executable}</string>
                <key>CFBundleVersion</key>
                <string>{version}</string>
                <key>CFBundleShortVersionString</key>
                <string>{version}</string>
                <key>CFBundleIconName</key>
                <string>AppIcon</string>
                <key>UILaunchStoryboardName</key>
                <string></string>
                <key>CFBundleSupportedPlatforms</key>
                <array>
                    <string>iPhoneOS</string>
                </array>
                <key>DTCompiler</key>
                <string>com.apple.compilers.llvm.clang.1_0</string>
                <key>DTPlatformBuild</key>
                <string>22A3362</string>
                <key>DTPlatformName</key>
                <string>iphoneos</string>
                <key>DTPlatformVersion</key>
                <string>26.0</string>
                <key>DTSDKBuild</key>
                <string>22A3362</string>
                <key>DTSDKName</key>
                <string>iphoneos26.0</string>
                <key>DTXcode</key>
                <string>1600</string>
                <key>DTXcodeBuild</key>
                <string>16A242d</string>
                <key>LSEnvironment</key>
                <dict>
                    <key>RUST_BACKTRACE</key>
                    <string>1</string>
                </dict>
                <key>LSRequiresIPhoneOS</key>
                <true/>
                <key>MinimumOSVersion</key>
                <string>15.0</string>
                <key>UIApplicationSupportsIndirectInputEvents</key>
                <true/>
                <key>UIDeviceFamily</key>
                <array>
                    <integer>1</integer>
                    <integer>2</integer>
                </array>
                <key>UIRequiredDeviceCapabilities</key>
                <array>
                    <string>arm64</string>
                </array>
                <key>UISupportedInterfaceOrientations~ipad</key>
                <array>
                    <string>UIInterfaceOrientationPortrait</string>
                    <string>UIInterfaceOrientationPortraitUpsideDown</string>
                    <string>UIInterfaceOrientationLandscapeLeft</string>
                    <string>UIInterfaceOrientationLandscapeRight</string>
                </array>
                <key>UISupportedInterfaceOrientations~iphone</key>
                <array>
                    <string>UIInterfaceOrientationPortrait</string>
                    <string>UIInterfaceOrientationLandscapeLeft</string>
                    <string>UIInterfaceOrientationLandscapeRight</string>
                </array>
                <key>NSLocationAlwaysAndWhenInUseUsageDescription</key>
                <string>For basic location access.</string>
                <key>NSLocationWhenInUseUsageDescription</key>
                <string>For basic location access.</string>
                <key>NSLocationUsageDescription</key>
                <string>For basic location access.</string>
                <key>NSLocationDefaultAccuracyReduced</key>
                <false/>
                <key>NSFaceIDUsageDescription</key>
                <string>For biometric authentication</string>
                <key>NSMicrophoneUsageDescription</key>
                <string>This app needs access to the microphone for audio recording functionality.</string>
            </dict>
            </plist>"#,
            identifier = self.identifier,
            display_name = self.display_name,
            name = self.name,
            executable = self.executable,
            version = self.version,
        )
    }
    fn to_tvos_plist_file(&self) -> String {
        format!(
            r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
            <key>BuildMachineOSBuild</key>
            <string>23B2082</string>
            <key>CFBundleDevelopmentRegion</key>
            <string>en</string>
            <key>CFBundleExecutable</key>
            <string>{executable}</string>
            <key>CFBundleIdentifier</key>
            <string>{identifier}</string>
            <key>CFBundleInfoDictionaryVersion</key>
            <string>6.0</string>
            <key>CFBundleDisplayName</key>
            <string>{display_name}</string>
            <key>CFBundleName</key>
            <string>{name}</string>
            <key>CFBundlePackageType</key>
            <string>APPL</string>
            <key>CFBundleShortVersionString</key>
            <string>{version}</string>
            <key>CFBundleSupportedPlatforms</key>
            <array>
            <string>AppleTVOS</string>
            </array>
            <key>CFBundleVersion</key>
            <string>{version}</string>
            <key>DTCompiler</key>
            <string>com.apple.compilers.llvm.clang.1_0</string>
            <key>DTPlatformBuild</key>
            <string>21J351</string>
            <key>DTPlatformName</key>
            <string>appletvos</string>
            <key>DTPlatformVersion</key>
            <string>15.0</string>
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
            <string>15.0</string>
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
            identifier = self.identifier,
            display_name = self.display_name,
            name = self.name,
            executable = self.executable,
            version = self.version,
        )
    }
}

pub struct Scent {
    app_id: String,
    team_id: String,
}

impl Scent {
    fn to_scent_file(&self) -> String {
        format!(
            r#"
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
        "#,
            self.app_id, self.team_id
        )
    }
}

/// Generate and compile an Asset Catalog with AppIcon from the crate's
/// `resources/` directory.  Requires a 1024×1024 PNG at minimum
/// (`icon_1024.png`).  Smaller sizes are optional; iOS will scale down
/// from the largest available.
fn generate_app_icon_xcassets(app_dir: &Path, build_crate: &str) -> Result<bool, String> {
    let crate_dir = get_crate_dir(build_crate)?;
    let res = crate_dir.join("resources");
    let icon_1024 = res.join("icon_1024.png");
    if !icon_1024.is_file() {
        return Ok(false);
    }

    // Build Assets.xcassets/AppIcon.appiconset/
    let xcassets = app_dir.join("Assets.xcassets");
    let appiconset = xcassets.join("AppIcon.appiconset");
    mkdir(&appiconset)?;

    // Copy available icon PNGs
    let sizes: &[(&str, &str)] = &[
        ("icon_1024.png", "icon_1024.png"),
    ];
    for (src_name, dst_name) in sizes {
        let src = res.join(src_name);
        if src.is_file() {
            cp(&src, &appiconset.join(dst_name), false)?;
        }
    }

    // Contents.json — universal+platform entry for iOS 16+, classic
    // idiom entries for iOS 15 and earlier (actool scales from 1024px).
    let contents_json = r#"{
  "images": [
    {
      "filename": "icon_1024.png",
      "idiom": "universal",
      "platform": "ios",
      "size": "1024x1024"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "2x",
      "size": "20x20"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "3x",
      "size": "20x20"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "2x",
      "size": "29x29"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "3x",
      "size": "29x29"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "2x",
      "size": "40x40"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "3x",
      "size": "40x40"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "2x",
      "size": "60x60"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "iphone",
      "scale": "3x",
      "size": "60x60"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "1x",
      "size": "20x20"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "2x",
      "size": "20x20"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "1x",
      "size": "29x29"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "2x",
      "size": "29x29"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "1x",
      "size": "40x40"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "2x",
      "size": "40x40"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "2x",
      "size": "76x76"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ipad",
      "scale": "2x",
      "size": "83.5x83.5"
    },
    {
      "filename": "icon_1024.png",
      "idiom": "ios-marketing",
      "scale": "1x",
      "size": "1024x1024"
    }
  ],
  "info": {
    "author": "cargo-makepad",
    "version": 1
  }
}"#;
    write_text(&appiconset.join("Contents.json"), contents_json)?;

    // Root Contents.json for Assets.xcassets
    write_text(
        &xcassets.join("Contents.json"),
        r#"{"info":{"author":"cargo-makepad","version":1}}"#,
    )?;

    // Compile with actool
    let cwd = std::env::current_dir().unwrap();
    shell_env_cap(
        &[],
        &cwd,
        "xcrun",
        &[
            "actool",
            &xcassets.to_string_lossy(),
            "--compile",
            &app_dir.to_string_lossy(),
            "--platform",
            "iphoneos",
            "--minimum-deployment-target",
            IOS_DEPLOYMENT_TARGET,
            "--app-icon",
            "AppIcon",
            "--output-partial-info-plist",
            &app_dir.join("actool-Info.plist").to_string_lossy(),
        ],
    )?;

    // Merge actool's partial Info.plist (contains CFBundleIcons for iOS 15)
    // into the main Info.plist.
    let actool_plist = app_dir.join("actool-Info.plist");
    if actool_plist.is_file() {
        let main_plist = app_dir.join("Info.plist");
        // PlistBuddy Merge copies all keys from source into destination
        shell_env_cap(
            &[],
            &cwd,
            "/usr/libexec/PlistBuddy",
            &[
                "-c",
                &format!("Merge {}", actool_plist.to_string_lossy()),
                &main_plist.to_string_lossy(),
            ],
        )?;
    }

    Ok(true)
}

pub struct IosBuildResult {
    pub app_dir: PathBuf,
    pub build_dir: PathBuf,
    pub plist: PlistValues,
    pub dst_bin: PathBuf,
}

pub fn build(
    stable: bool,
    org: &str,
    product: &str,
    args: &[String],
    apple_target: AppleTarget,
) -> Result<IosBuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    let binary_name = get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());

    let cwd = std::env::current_dir().unwrap();
    let target_dir = cargo_target_dir(&cwd);
    let target_dir_str = target_dir.to_string_lossy().to_string();
    let target_dir_arg = format!("--target-dir={target_dir_str}");
    let target_opt = format!("--target={}", apple_target.toolchain());

    let base_args = &[
        "run",
        if stable { "stable" } else { "nightly" },
        "cargo",
        "build",
        &target_opt,
        &target_dir_arg,
    ];

    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    for arg in args {
        args_out.push(arg);
    }

    if apple_target.needs_build_std() {
        args_out.push("-Z");
        args_out.push("build-std=std");
    }

    let mut rust_env = vec![
        ("RUST_BACKTRACE", "1"),
        ("MAKEPAD", if stable { "" } else { "lines" }),
    ];
    if matches!(apple_target.os(), AppleOs::Ios) {
        rust_env.push(("IPHONEOS_DEPLOYMENT_TARGET", IOS_DEPLOYMENT_TARGET));
        rust_env.push(("IPHONESIMULATOR_DEPLOYMENT_TARGET", IOS_DEPLOYMENT_TARGET));
    }
    shell_env(&rust_env, &cwd, "rustup", &args_out)?;

    // alright lets make the .app file with manifest
    let plist = PlistValues {
        identifier: format!("{org}.{product}").to_string(),
        display_name: product.to_string(),
        name: product.to_string(),
        executable: binary_name.clone(),
        version: "1.0.0".to_string(),
    };
    let profile = get_profile_from_args(args);

    let app_dir = target_dir.join(format!(
        "makepad-apple-app/{}/{profile}/{build_crate}.app",
        apple_target.toolchain()
    ));
    mkdir(&app_dir)?;

    let plist_file = app_dir.join("Info.plist");
    write_text(&plist_file, &plist.to_plist_file(apple_target.os()))?;

    if matches!(apple_target.os(), AppleOs::Ios) {
        match generate_app_icon_xcassets(&app_dir, build_crate) {
            Ok(true) => {}
            Ok(false) => {
                eprintln!("warning: no icon_1024.png in resources/. iOS app will use default icon.");
            }
            Err(e) => {
                eprintln!("warning: failed to compile app icon asset catalog: {e}");
            }
        }
    }

    let build_dir = target_dir.join(format!("{}/{profile}/", apple_target.toolchain()));
    let src_bin = target_dir.join(format!(
        "{}/{profile}/{binary_name}",
        apple_target.toolchain()
    ));
    let dst_bin = app_dir.join(binary_name.clone());

    cp(&src_bin, &dst_bin, false)?;

    Ok(IosBuildResult {
        build_dir,
        app_dir,
        plist,
        dst_bin,
    })
}

pub fn run_on_sim(
    apple_args: AppleArgs,
    args: &[String],
    apple_target: AppleTarget,
) -> Result<(), String> {
    if apple_args.org.is_none() {
        return Err("Please set --org=org before run-sim.".to_string());
    }

    let build_crate = get_build_crate_from_args(args)?;
    let default_app = get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());

    let result = build(
        apple_args.stable,
        &apple_args.org.unwrap_or("orgname".to_string()),
        &apple_args.app.unwrap_or(default_app),
        args,
        apple_target,
    )?;
    let build_crate = get_build_crate_from_args(args)?;
    copy_resources(
        &result.app_dir,
        build_crate,
        &result.build_dir,
        apple_target,
    )?;
    let app_dir = result.app_dir.into_os_string().into_string().unwrap();

    let cwd = std::env::current_dir().unwrap();
    shell_env(
        &[],
        &cwd,
        "xcrun",
        &["simctl", "install", "booted", &app_dir],
    )?;

    shell_env(
        &[],
        &cwd,
        "xcrun",
        &[
            "simctl",
            "launch",
            "--console",
            "booted",
            &result.plist.identifier,
        ],
    )?;

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
    data: &'a [u8],
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
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
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
                    if self.data[self.pos] == ' ' as u8
                        || self.data[self.pos] == '\t' as u8
                        || self.data[self.pos] == '\n' as u8
                    {
                        self.pos += 1;
                    } else if self.data[self.pos] == '<' as u8 {
                        self.pos += 1;
                        state = State::TagName(false, false, self.pos)
                    } else {
                        state = State::Data(self.pos);
                        self.pos += 1;
                    }
                }
                State::TagName(is_close, self_closing, start) => {
                    if self.data[self.pos] == '/' as u8 {
                        if self.pos == start {
                            state = State::TagName(true, false, start + 1);
                        } else {
                            state = State::TagName(true, true, start);
                        }
                        self.pos += 1;
                    } else if self.data[self.pos] == '>' as u8 {
                        let end = if self_closing { self.pos - 1 } else { self.pos };
                        let name = std::str::from_utf8(&self.data[start..end])
                            .unwrap()
                            .to_string();
                        self.pos += 1;
                        if is_close {
                            if self_closing {
                                return Ok(XmlResult::SelfCloseTag);
                            } else {
                                return Ok(XmlResult::CloseTag(name));
                            }
                        } else {
                            return Ok(XmlResult::OpenTag(name));
                        }
                    } else {
                        self.pos += 1;
                    }
                }
                State::Data(start) => {
                    if self.data[self.pos] == '<' as u8 {
                        let body = std::str::from_utf8(&self.data[start..self.pos])
                            .unwrap()
                            .to_string();
                        return Ok(XmlResult::Data(body));
                    } else {
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
                    return Some(&bytes[i + start_bytes.len()..]);
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
                        } else if let Some(last_key) = &last_key {
                            match last_key.as_ref() {
                                "ProvisionedDevices" => {
                                    if stack.last().unwrap() == "string" {
                                        devices.push(data);
                                    }
                                }
                                "com.apple.developer.team-identifier" => {
                                    if stack.last().unwrap() == "string" {
                                        team_ident = Some(data);
                                    }
                                }
                                "TeamIdentifier" => {
                                    if stack.last().unwrap() == "string" {
                                        team_ident = Some(data);
                                    }
                                }
                                "application-identifier" => {
                                    if stack.last().unwrap() == "string" {
                                        app_identifier = Some(data);
                                        //if !data.contains(app_id) {
                                        //    return None
                                        //}
                                    }
                                }
                                "UUID" => {
                                    if stack.last().unwrap() == "string" {
                                        uuid = Some(data);
                                    }
                                }
                                _ => (),
                            }
                        }
                    }
                }
            }
        }
        if team_ident.is_none() {
            return None;
        }
        Some(ProvisionData {
            devices,
            uuid: uuid.unwrap(),
            app_identifier: app_identifier.unwrap(),
            team_ident: team_ident.unwrap(),
            path: path.clone(),
        })
    }
}

pub fn copy_resources(
    app_dir: &Path,
    build_crate: &str,
    build_dir: &Path,
    apple_target: AppleTarget,
) -> Result<(), String> {
    /*let mut assets_to_add: Vec<String> = Vec::new();*/
    let add_assets_dir =
        |crate_name: &str, source_dir: &Path, asset_subdir: &str| -> Result<(), String> {
            if !source_dir.is_dir() {
                return Ok(());
            }
            let crate_name = crate_name.replace('-', "_");
            let dst_dir = app_dir.join(format!("makepad/{crate_name}/{asset_subdir}"));
            mkdir(&dst_dir)?;
            cp_all(source_dir, &dst_dir, false)?;
            Ok(())
        };
    let add_font_assets_dir = |crate_name: &str, source_dir: &Path| -> Result<(), String> {
        if !source_dir.is_dir() {
            return Ok(());
        }
        let crate_name = crate_name.replace('-', "_");
        let dst_dir = app_dir.join(format!("makepad/{crate_name}/fonts"));
        let assets = ls(source_dir)?;
        for path in &assets {
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase());
            if !matches!(
                ext.as_deref(),
                Some("ttf" | "otf" | "ttc" | "woff" | "woff2")
            ) {
                continue;
            }
            cp(&source_dir.join(path), &dst_dir.join(path), false)?;
        }
        Ok(())
    };

    let build_crate_dir = get_crate_dir(build_crate)?;
    add_assets_dir(build_crate, &build_crate_dir.join("resources"), "resources")?;
    add_font_assets_dir(build_crate, &build_crate_dir.join("fonts"))?;

    let deps = get_crate_dep_dirs(build_crate, &build_dir, apple_target.toolchain());
    for (name, dep_dir) in deps.iter() {
        add_assets_dir(name, &dep_dir.join("resources"), "resources")?;
        add_font_assets_dir(name, &dep_dir.join("fonts"))?;
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
    pub org: Option<String>,
}

pub fn run_on_device(
    apple_args: AppleArgs,
    args: &[String],
    apple_target: AppleTarget,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    // lets parse the inputs
    let parsed = parse_profiles()?;

    let provision = apple_args.provisioning_profile.as_ref().and_then(|v| {
        if v.contains('/') {
            ProvisionData::parse(&PathBuf::from(v))
        } else {
            let v = parsed.profile(v).expect("cannot find provisioning profile");
            ProvisionData::parse(v)
        }
    });

    if provision.is_none()
        || apple_args.provisioning_profile.is_none()
        || apple_args.signing_identity.is_none()
        || apple_args.device_identifier.is_none()
    {
        // lets list the provisioning profiles.
        println!("Error: missing provisioning profile, signing idenity or device identifier");
        parsed.println();
        return Err("please provide missing arguments BEFORE run-device".into());
    }
    let provision = provision.unwrap();

    let org = apple_args.org.unwrap();

    let build_crate = get_build_crate_from_args(args)?;
    let default_app = get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
    let app = apple_args.app.unwrap_or(default_app);

    let result = build(apple_args.stable, &org, &app, args, apple_target)?;

    let scent = Scent {
        app_id: format!("{}.{}.{}", provision.team_ident, org, app),
        team_id: provision.team_ident.to_string(),
    };

    let target_dir = cargo_target_dir(&cwd);
    let scent_file = target_dir.join(format!(
        "makepad-apple-app/{}/release/{build_crate}.scent",
        apple_target.toolchain()
    ));
    write_text(&scent_file, &scent.to_scent_file())?;

    let dst_provision = result.app_dir.join("embedded.mobileprovision");
    let app_dir = result.app_dir.into_os_string().into_string().unwrap();

    cp(&provision.path, &dst_provision, false)?;

    copy_resources(
        Path::new(&app_dir),
        build_crate,
        &result.build_dir,
        apple_target,
    )?;

    let cert = parsed
        .cert(apple_args.signing_identity.as_ref().unwrap())
        .expect("cannot find signing certificate");

    shell_env_cap(
        &[],
        &cwd,
        "codesign",
        &[
            "--force",
            "--timestamp=none",
            "--sign",
            cert,
            &result.dst_bin.into_os_string().into_string().unwrap(),
        ],
    )?;

    shell_env_cap(
        &[],
        &cwd,
        "codesign",
        &[
            "--force",
            "--timestamp=none",
            "--sign",
            cert,
            "--entitlements",
            &scent_file.into_os_string().into_string().unwrap(),
            "--generate-entitlement-der",
            &app_dir,
        ],
    )?;

    let cwd = std::env::current_dir().unwrap();
    for device_identifier in apple_args.device_identifier.unwrap().split(",") {
        let device_identifier = parsed
            .device(device_identifier)
            .expect("cannot find signing device");

        // Try devicectl first, fall back to ios-deploy for older iOS (< 17)
        let devicectl_result = shell_env_cap(
            &[],
            &cwd,
            "xcrun",
            &[
                "devicectl",
                "device",
                "install",
                "app",
                "--device",
                device_identifier,
                &app_dir,
            ],
        );

        match devicectl_result {
            Ok(answer) => {
                // Parse the bundleID from the installation output and launch the app
                let mut bundle_id = None;
                for line in answer.split("\n") {
                    if let Some(idx) = line.find("bundleID:") {
                        bundle_id = Some(line[idx + "bundleID:".len()..].trim().to_string());
                        break;
                    }
                }

                if let Some(bundle_id) = bundle_id {
                    shell_env(
                        &[],
                        &cwd,
                        "xcrun",
                        &[
                            "devicectl",
                            "device",
                            "process",
                            "launch",
                            "--device",
                            device_identifier,
                            &bundle_id,
                        ],
                    )?;
                } else {
                    return Err(format!("Failed to find bundleID in installation output"));
                }
            }
            Err(_) => {
                // devicectl failed (device too old or unavailable), try ios-deploy
                println!("devicectl failed, falling back to ios-deploy...");
                // ios-deploy --justlaunch exits non-zero (253) when device debug
                // symbols are missing, but the app is still installed and launched.
                // Use shell_env_route to show progress output directly.
                let result = shell_env_cap(
                    &[],
                    &cwd,
                    "ios-deploy",
                    &[
                        "--bundle",
                        &app_dir,
                        "--id",
                        device_identifier,
                        "--justlaunch",
                    ],
                );
                if let Err(e) = &result {
                    if !e.contains("Unable to locate DeviceSupport") {
                        return Err(e.clone());
                    }
                    // Missing debug symbols is non-fatal — app was installed and launched
                    println!("App installed and launched (debug symbols unavailable on this device).");
                }
            }
        }
    }

    Ok(())
}
