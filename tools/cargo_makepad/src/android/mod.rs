mod compile;
mod sdk;

#[derive(Clone, Copy, PartialEq)]
pub enum HostOs {
    WindowsX64,
    MacosX64,
    MacosAarch64,
    LinuxX64,
    Unsupported
}


#[derive(Clone, Copy, PartialEq)]
pub enum AndroidVariant {
    Default,
    Quest
}
impl AndroidVariant {
    fn from_str(opt: &str) -> Result<Self,String> {
        for opt in opt.split(","){
            match opt {
                "default"=> return Ok(AndroidVariant::Default),
                "quest" => return Ok(AndroidVariant::Quest),
                _=>()
            }
        }
        return Err(format!("please provide a valid android variant: default, quest"))
        
    }
    
    fn manifest_xml(&self, label:&str, class_name:&str, url:&str, sdk_version: usize)->String{
        match self{
            Self::Default=>format!(r#"<?xml version="1.0" encoding="utf-8"?>
                <manifest xmlns:android="http://schemas.android.com/apk/res/android"
                xmlns:tools="http://schemas.android.com/tools"
                package="{url}">
                <application
                    android:label="{label}"
                    android:theme="@android:style/Theme.NoTitleBar.Fullscreen"
                    android:allowBackup="true"
                    android:supportsRtl="true"
                    android:debuggable="true"
                    android:largeHeap="true"
                    tools:targetApi="{sdk_version}">
                    <meta-data android:name="android.max_aspect" android:value="2.1" />
                    <activity
                    android:name=".{class_name}"
                    android:configChanges="orientation|screenSize|keyboardHidden"
                    android:exported="true">
                    <intent-filter>
                        <action android:name="android.intent.action.MAIN" />
                        <category android:name="android.intent.category.LAUNCHER" />
                    </intent-filter>
                    </activity>
                </application>
                <uses-sdk android:targetSdkVersion="{sdk_version}" />
                <uses-feature android:glEsVersion="0x00020000" android:required="true"/>
                <uses-feature android:name="android.hardware.bluetooth_le" android:required="true"/>
                <uses-feature android:name="android.software.midi" android:required="true"/>
                <uses-permission android:name="android.permission.READ_EXTERNAL_STORAGE" />
                <uses-permission android:name="android.permission.READ_MEDIA_VIDEO"  />
                <uses-permission android:name="android.permission.READ_MEDIA_IMAGES"  />
                <uses-permission android:name="android.permission.INTERNET" />
                <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
                <uses-permission android:name="android.permission.BLUETOOTH"/>
                <uses-permission android:name="android.permission.BLUETOOTH_CONNECT"/>
                <uses-permission android:name="android.permission.CAMERA"/>
                <uses-permission android:name="android.permission.ACCESS_COARSE_LOCATION"/>
                <uses-permission android:name="android.permission.ACCESS_FINE_LOCATION"/>
                <uses-permission android:name="android.permission.USE_BIOMETRIC" />
                <uses-permission android:name="android.permission.QUERY_ALL_PACKAGES" tools:ignore="QueryAllPackagesPermission" />
                            
                <queries>
                <intent>
                <action android:name="android.intent.action.MAIN" />
                </intent>
                </queries>
                </manifest>
                "#),
            Self::Quest=>format!(r#"<?xml version="1.0" encoding="utf-8"?>
                <manifest
                    xmlns:android="http://schemas.android.com/apk/res/android"
                    xmlns:tools="http://schemas.android.com/tools"
                    package="{url}"
                    android:versionCode="1"
                    android:versionName="1.0"
                    android:installLocation="auto"
                >      
                                                                
                <uses-sdk android:targetSdkVersion="{sdk_version}" />
                <uses-feature android:glEsVersion="0x00030001" android:required="true"/>
                <uses-feature android:name="android.hardware.vr.headtracking" android:required="false"/>
                <uses-feature android:name="com.oculus.feature.PASSTHROUGH" android:required="true"/>
                <uses-permission android:name="com.oculus.permission.USE_SCENE" />
                <!-- Request hand and keyboard tracking for keyboard hand presence testing -->
                <uses-feature android:name="oculus.software.handtracking" android:required="false"/>
                <uses-permission android:name="com.oculus.permission.HAND_TRACKING" />
                <uses-permission android:name="android.permission.INTERNET" />
                <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
                <uses-permission android:name="org.khronos.openxr.permission.OPENXR" />
                <uses-permission android:name="org.khronos.openxr.permission.OPENXR_SYSTEM" />
                                                                                    
                <application
                    android:label="{label}"
                    android:theme="@android:style/Theme.NoTitleBar.Fullscreen"
                    android:allowBackup="true"
                    android:supportsRtl="true"
                    android:debuggable="true"
                    android:largeHeap="true"
                    tools:targetApi="{sdk_version}">
                    <activity
                        android:name=".{class_name}"
                        android:configChanges="screenSize|screenLayout|orientation|keyboardHidden|keyboard|navigation|uiMode"
                        android:excludeFromRecents="false"
                        android:exported="true"
                        android:launchMode="singleTask"
                        android:screenOrientation="landscape"
                        android:theme="@android:style/Theme.Black.NoTitleBar.Fullscreen" 
                        >
                        <intent-filter>
                            <action android:name="android.intent.action.MAIN" />
                            <category android:name="android.intent.category.LAUNCHER" />
                        </intent-filter>
                        </activity>
                                                
                    <activity
                        android:name="{class_name}Xr"
                        android:configChanges="screenSize|screenLayout|orientation|keyboardHidden|keyboard|navigation|uiMode"
                        android:excludeFromRecents="false"
                        android:exported="true"
                        android:launchMode="singleTask"
                        android:screenOrientation="landscape"
                        android:theme="@android:style/Theme.Black.NoTitleBar.Fullscreen" 
                        >
                        <intent-filter>
                            <action android:name="android.intent.action.MAIN" />
                            <category android:name="com.oculus.intent.category.VR" />
                        </intent-filter>
                    </activity>
                </application>
                                                                                    
                <queries>
                <!-- to talk to the broker -->
                    <provider 
                    android:name="x" android:authorities="org.khronos.openxr.runtime_broker;org.khronos.openxr.system_runtime_broker" />
                                                                                                
                <!-- so client-side code of runtime/layers can talk to their service sides -->
                <intent>
                <action android:name="org.khronos.openxr.OpenXRRuntimeService" />
                </intent>
                <intent>
                <action android:name="org.khronos.openxr.OpenXRApiLayerService" />
                </intent>
                <intent>
                <action android:name="android.intent.action.MAIN" />
                </intent>
                </queries>
                                            
                </manifest>
                "#)
            }
        }
    }
    
/*
Self::Quest=>format!(r#"<?xml version="1.0" encoding="utf-8"?>
    <manifest
    xmlns:android="http://schemas.android.com/apk/res/android"
    package="{url}"
    android:versionCode="1"
    android:versionName="1.0"
    android:installLocation="auto"
    >
                        
                                            
    <uses-sdk android:targetSdkVersion="{sdk_version}" />
    <uses-feature android:glEsVersion="0x00030001" android:required="true"/>
    <uses-feature android:name="android.hardware.vr.headtracking" android:required="false"/>
    <uses-feature android:name="com.oculus.feature.PASSTHROUGH" android:required="true"/>
    <uses-permission android:name="com.oculus.permission.USE_SCENE" />
    <!-- Request hand and keyboard tracking for keyboard hand presence testing -->
    <uses-feature android:name="oculus.software.handtracking" android:required="false"/>
    <uses-permission android:name="com.oculus.permission.HAND_TRACKING" />
    <uses-permission android:name="android.permission.INTERNET" />
    <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
    <uses-permission android:name="org.khronos.openxr.permission.OPENXR" />
    <uses-permission android:name="org.khronos.openxr.permission.OPENXR_SYSTEM" />
                                                                
    <application
    android:label="{label}"
    android:allowBackup="false"
    android:debuggable="true"
    >
    //
    <activity
    android:name="{class_name}"
    android:theme="@android:style/Theme.Black.NoTitleBar.Fullscreen"
    android:launchMode="singleTask"
    android:screenOrientation="landscape"
    android:excludeFromRecents="false"
    android:configChanges="screenSize|screenLayout|orientation|keyboardHidden|keyboard|navigation|uiMode"
    android:exported="true"
    >
    <intent-filter>
    <action android:name="android.intent.action.MAIN" />
    <action android:name="android.intent.action.LAUNCHER" />
    <action android:name="android.intent.action.VR" />
    </intent-filter>
    </activity>
                            
    <activity
    android:name="{class_name}.MakepadAppXr"
    android:configChanges="screenSize|screenLayout|orientation|keyboardHidden|keyboard|navigation|uiMode"
    android:excludeFromRecents="false"
    android:exported="true"
    android:launchMode="singleTask"
    android:screenOrientation="landscape"
    android:theme="@android:style/Theme.Black.NoTitleBar.Fullscreen" 
    >
    <intent-filter>
    <action android:name="android.intent.action.MAIN" />
    <category android:name="com.oculus.intent.category.VR" />
    </intent-filter>
    </activity>
    </application>
                                                                
    <queries>
    <!-- to talk to the broker -->
    <provider 
    android:name="x" android:authorities="org.khronos.openxr.runtime_broker;org.khronos.openxr.system_runtime_broker" />
                                                                        
    <!-- so client-side code of runtime/layers can talk to their service sides -->
    <intent>
    <action android:name="org.khronos.openxr.OpenXRRuntimeService" />
    </intent>
    <intent>
    <action android:name="org.khronos.openxr.OpenXRApiLayerService" />
    </intent>
    <intent>
    <action android:name="android.intent.action.MAIN" />
    </intent>
    </queries>
                        
    </manifest>
    "#)*/    
    
    
#[allow(non_camel_case_types)]
pub enum AndroidTarget {
    aarch64,
    x86_64,
    armv7,
    i686
}

impl AndroidTarget {
    fn from_str(opt: &str) -> Result<Vec<Self>,
    String> {
        let mut out = Vec::new();
        for opt in opt.split(","){
            match opt {
                "all"=> return Ok(vec![AndroidTarget::aarch64, AndroidTarget::x86_64, AndroidTarget::armv7, AndroidTarget::i686]),
                "aarch64" => out.push(AndroidTarget::aarch64),
                "x86_64" => out.push(AndroidTarget::x86_64),
                "armv7" => out.push(AndroidTarget::armv7),
                "i686" => out.push(AndroidTarget::i686),
                x => {
                    return Err(format!("{:?} please provide a valid ABI: aarch64, x86_64, armv7, i686", x))
                }
            }
        }
        return Ok(out);
    }
    fn _sys_dir(&self) -> &'static str {
        match self {
            Self::aarch64 => "aarch64-linux-android",
            Self::x86_64 => "x86_64-linux-android",
            Self::armv7 => "arm-linux-androideabi",
            Self::i686 => "i686-linux-android",
        }
    }
    fn _unwind_dir(&self) -> &'static str {
        match self {
            Self::aarch64 => "aarch64",
            Self::x86_64 => "x86_64",
            Self::armv7 => "arm",
            Self::i686 => "i386",
        }
    }
    
    fn clang(&self) -> &'static str {
        match self {
            Self::aarch64 => "aarch64-linux-android",
            Self::x86_64 => "x86_64-linux-android", 
            Self::armv7 => "armv7a-linux-androideabi", 
            Self::i686 => "i686-linux-android", 
        }
    }
    fn toolchain(&self)->&'static str{
        match self {
            Self::aarch64 => "aarch64-linux-android",
            Self::x86_64 => "x86_64-linux-android",
            Self::armv7 => "armv7-linux-androideabi",
            Self::i686 => "i686-linux-android"
        }
    }
    fn to_str(&self) -> &'static str {
        match self {
            Self::aarch64 => "aarch64",
            Self::x86_64 => "x86_64",
            Self::armv7 => "armv7",
            Self::i686 => "i686",
        }
    }
    fn abi_identifier(&self) -> &'static str {
        match self {
            Self::aarch64 => "arm64-v8a",
            Self::x86_64 => "x86_64",
            Self::armv7 => "armeabi-v7a",
            Self::i686 => "x86",
        }
    }
    fn linker_env_var(&self) -> &'static str {
        match self {
            Self::aarch64 => "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER",
            Self::x86_64 => "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER",
            Self::armv7 => "CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER",
            Self::i686 => "CARGO_TARGET_I686_LINUX_ANDROID_LINKER",
        }
    }
}

impl HostOs {
    fn from_str(opt: &str) -> Result<Self,
    String> {
        match opt {
            "windows-x64" => Ok(HostOs::WindowsX64),
            "macos-x64" => Ok(HostOs::MacosX64),
            "macos-aarch64" => Ok(HostOs::MacosAarch64),
            "linux-x64" => Ok(HostOs::LinuxX64),
            x => {
                Err(format!("{:?} please provide a valid host-os: windows-x64,macos-x64,macos-aarch64,linux-x64", x))
            }
        }
    }
    
    fn default_path(&self) -> &'static str {
        match self {
            Self::WindowsX64 => "./android_33_windows_x64",
            Self::MacosX64 => "./android_33_macos_x64",
            Self::MacosAarch64 => "./android_33_macos_aarch64",
            Self::LinuxX64 => "./android_33_linux_x64",
            Self::Unsupported => panic!()
        }
    }
}

pub fn handle_android(mut args: &[String]) -> Result<(), String> {
    #[allow(unused)]
    let mut host_os = HostOs::Unsupported;
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))] let mut host_os = HostOs::WindowsX64;
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))] let mut host_os = HostOs::MacosX64;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))] let mut host_os = HostOs::MacosAarch64;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))] let mut host_os = HostOs::LinuxX64;
    let mut sdk_path = None;
    let mut package_name = None;
    let mut app_label = None;
    let mut variant = AndroidVariant::Default;
    let mut targets = vec![AndroidTarget::aarch64];
    let mut keep_sdk_sources = false;
    
    let urls = sdk::ANDROID_SDK_URLS_33;
    
    // pull out options
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--host-os=") {
            host_os = HostOs::from_str(opt) ?;
        }
        else if let Some(opt) = v.strip_prefix("--sdk-path=") {
            sdk_path = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--package-name=") {
            package_name = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--app-label=") {
            app_label = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--abi=") {
            targets = AndroidTarget::from_str(opt)?;
        }
        else if let Some(opt) = v.strip_prefix("--variant=") {
            variant = AndroidVariant::from_str(opt)?;
        }
        else if v.trim() == "--keep-sdk-sources" {
            keep_sdk_sources = true;
        }
        else {
            args = &args[i..];
            break
        }
    }
    if sdk_path.is_none() {
        sdk_path = Some(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), host_os.default_path().to_string()));
    }
    
    let cwd = std::env::current_dir().unwrap();
    let sdk_dir = cwd.join(sdk_path.unwrap());
    
    match args[0].as_ref() {
        "adb" => {
            compile::adb(&sdk_dir, host_os, &args[1..])
        },
        "java" => {
            compile::java(&sdk_dir, host_os, &args[1..])
        },
        "javac" => {
            compile::javac(&sdk_dir, host_os, &args[1..])
        },
        "rustup-toolchain-install" | "rustup-install-toolchain"  => {
            sdk::rustup_toolchain_install(&targets)
        }
        "download-sdk" => {
            sdk::download_sdk(&sdk_dir, host_os, &args[1..], &urls)
        }
        "expand-sdk" => {
            sdk::expand_sdk(&sdk_dir, host_os, &args[1..], &targets, &urls)
        }
        "remove-sdk-sources" => {
            sdk::remove_sdk_sources(&sdk_dir, host_os, &args[1..])
        }
        "toolchain-install" | "install-toolchain"=> {
            println!("Installing Android toolchain\n");
            sdk::rustup_toolchain_install(&targets) ?;
            sdk::download_sdk(&sdk_dir, host_os, &args[1..], &urls) ?;
            sdk::expand_sdk(&sdk_dir, host_os, &args[1..], &targets, &urls) ?;
            if !keep_sdk_sources {
                sdk::remove_sdk_sources(&sdk_dir, host_os, &args[1..]) ?;
            }
            println!("\nAndroid toolchain has been installed\n");
            Ok(())
        }
        /*"base-apk"=>{
            compile::base_apk(&sdk_dir, host_os, &args[1..])
        }*/
        "build" => {
            compile::build(&sdk_dir, host_os, package_name, app_label, &args[1..], &targets, &variant, &urls) ?;
            Ok(())
        }
        "run" => {
            compile::run(&sdk_dir, host_os, package_name, app_label, &args[1..], &targets, &variant, &urls)
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
