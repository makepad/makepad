use crate::open_harmony::OpenHarmonyTarget;
use crate::open_harmony::HostOs;
use crate::utils::*;
use crate::makepad_shell::*;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

fn get_sdk_home(deveco_home: &Path, _host_os: &HostOs) -> Result<PathBuf, String> {
    let sdk =deveco_home.join("sdk").join(format!("default")).join("openharmony");
    if sdk.is_dir() {
        return Ok(sdk);
    }
    for i in 1..=5 {
        let sdk = deveco_home.join("sdk").join(format!("HarmonyOS-NEXT-DB{i}")).join("openharmony");
        if sdk.is_dir() {
            return Ok(sdk);
        }
    }
    return Err(format!("failed to get sdk home, deveco_home={}",deveco_home.to_str().unwrap()));
}


fn get_deveco_sdk_home(deveco_home: &Path, _host_os: &HostOs) -> Result<PathBuf, String> {
    let deveco_sdk = deveco_home.join("sdk");
    if deveco_sdk.is_dir() {
        return Ok(deveco_sdk);
    }
    return Err(format!("failed to get deveco sdk home, deveco_home={}",deveco_home.to_str().unwrap()));
}


fn get_node_home(deveco_home: &Path, host_os: &HostOs) -> Result<PathBuf, String> {
    match host_os {
        HostOs::LinuxX64 => {
            let node = deveco_home.join("tool/node");
            if node.is_dir() {
                return Ok(node);
            }
            return Err(format!("failed to get node home, deveco_home={}",deveco_home.to_str().unwrap()));
        },
        HostOs::MacOS | HostOs::WindowsX64 => {
            let node = deveco_home.join("tools").join("node");
            if node.is_dir() {
                return Ok(node);
            }
            return Err(format!("failed to get node home, deveco_home={}",deveco_home.to_str().unwrap()));
        }
        _ => panic!()
    }
}

fn get_node_path(deveco_home: &Path, host_os: &HostOs) -> Result<PathBuf, String> {
    match host_os {
        HostOs::LinuxX64 => {
            let node = deveco_home.join("tool/node/bin/node");
            if node.is_file() {
                return Ok(node);
            }
            return Err(format!("failed to get node path, deveco_home={}",deveco_home.to_str().unwrap()));
        },
        HostOs::WindowsX64 => {
            let node = deveco_home.join("tools\\node\\node.exe");
            if node.is_file() {
                return Ok(node);
            }
            return Err(format!("failed to get node path, deveco_home={}",deveco_home.to_str().unwrap()));
        },
        HostOs::MacOS => {
            let node = deveco_home.join("tools/node/bin/node");
            if node.is_file() {
                return Ok(node);
            }
            return Err(format!("failed to get node path, deveco_home={}",deveco_home.to_str().unwrap()));
        },
        _ => panic!()
    }
}

fn get_hvigor_path(deveco_home: &Path, host_os: &HostOs) -> Result<PathBuf, String> {
    match host_os {
        HostOs::LinuxX64 => {
            let hvigor = deveco_home.join("hvigor/bin/hvigorw.js");
            if hvigor.is_file() {
                return Ok(hvigor);
            }
            return Err(format!("failed to get hvigor path, deveco_home={}",deveco_home.to_str().unwrap()));
        },
        HostOs::WindowsX64 => {
            let hvigor = deveco_home.join("tools\\hvigor\\bin\\hvigorw.js");
            if hvigor.is_file() {
                return Ok(hvigor);
            }
            return Err(format!("failed to get hvigor path, deveco_home={}",deveco_home.to_str().unwrap()));
        }
        HostOs::MacOS => {
            let hvigor = deveco_home.join("tools/hvigor/bin/hvigorw.js");
            if hvigor.is_file() {
                return Ok(hvigor);
            }
            return Err(format!("failed to get hvigor path, deveco_home={}",deveco_home.to_str().unwrap()));
        }
        _ => panic!()
    }
}

fn get_hdc_path(deveco_home: &Path, host_os: &HostOs) -> Result<PathBuf, String> {
    match host_os {
        HostOs::LinuxX64 | HostOs::MacOS => {
            let hdc_path = deveco_home.join(format!("sdk/default/openharmony/toolchains/hdc"));
            if hdc_path.is_file() {
                    return Ok(hdc_path);
            }
            for i in 1..-5 {
                let hdc_path = deveco_home.join(format!("sdk/HarmonyOS-NEXT-DB{i}/openharmony/toolchains/hdc"));
                if hdc_path.is_file() {
                    return Ok(hdc_path);
                }
            }
        },
        HostOs::WindowsX64 => {
            let hdc_path = deveco_home.join(format!("sdk\\default\\openharmony\\toolchains\\hdc.exe"));
            if hdc_path.is_file() {
                return Ok(hdc_path);
            }
            for i in 1..=5 {
                let hdc_path = deveco_home.join(format!("sdk\\HarmonyOS-NEXT-DB{i}\\openharmony\\toolchains\\hdc.exe"));
                if hdc_path.is_file() {
                    return Ok(hdc_path);
                }
            }
        },
        _ => panic!()
    }
    return Err(format!("failed to get hdc path, deveco_home={}",deveco_home.to_str().unwrap()));
}

fn app_json(bundle_name:&str) -> String {
    format!(r#"
{{
  "app": {{
    "bundleName": "dev.makepad.{bundle_name}",
    "vendor": "makepad",
    "versionCode": 1000000,
    "versionName": "1.0.0",
    "icon": "$media:app_icon",
    "label": "$string:app_name"
    }}
}}
    "#)
}

fn label_json(label_name:&str) -> String {
    format!(r#"
{{
  "string": [
    {{
      "name": "module_desc",
      "value": "module description"
    }},
    {{
      "name": "EntryAbility_desc",
      "value": "description"
    }},
    {{
      "name": "EntryAbility_label",
      "value": "{label_name}"
    }}
  ]
}}
"#)
}

fn label_zh_json(label_name:&str) -> String {
    format!(r#"
{{
  "string": [
    {{
      "name": "module_desc",
      "value": "模块描述"
    }},
    {{
      "name": "EntryAbility_desc",
      "value": "description"
    }},
    {{
      "name": "EntryAbility_label",
      "value": "{label_name}"
    }}
  ]
}}
"#)
}


fn check_deveco_prj(args: &[String]) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let build_crate = get_build_crate_from_args(args)?;
    let underscore_build_crate = build_crate.replace('-', "_");

    // TODO: don't use the current working dir here, instead, use the cargo target directory
    let prj_path =  cwd.join("target").join("makepad-open-harmony").join(underscore_build_crate);
    if prj_path.is_dir() == false {
        Err("run \"deveco\" to create DevEco project before \"build\"".to_owned())
    } else {
        Ok(())
    }
}



pub fn rust_build(deveco_home: &Option<String>, host_os: &HostOs, args: &[String], targets:&[OpenHarmonyTarget]) -> Result<(), String> {
    if deveco_home.is_none() {
        return Err("--deveco-home is not specified".to_owned());
    }
    let deveco_home = Path::new(deveco_home.as_ref().unwrap());
    let cwd = std::env::current_dir().unwrap();
    let sdk_path = get_sdk_home(deveco_home, &host_os)?;

    let bin_path = |file_name: &str, extension:& str| match host_os {
        HostOs::LinuxX64 => String::from(file_name),
        HostOs::WindowsX64 => format!("{file_name}.{extension}"),
        HostOs::MacOS => String::from(file_name),
        _ => panic!()
    };
    
    for target in targets {
        let target_triple = target.target_triple_str();
        let native_llvm_bin = sdk_path.join("native").join("llvm").join("bin");
        let full_clang_path = native_llvm_bin.join(bin_path(&format!("{target_triple}-clang"), "cmd"));
        let full_clangcl_path = native_llvm_bin.join(bin_path(&format!("{target_triple}-clang-cl"), "cmd"));
        let full_clangpp_path = native_llvm_bin.join(bin_path(&format!("{target_triple}-clang++"), "cmd"));
        let full_llvm_ar_path = native_llvm_bin.join(bin_path("llvm-ar", "exe"));
        let full_llvm_ranlib_path = native_llvm_bin.join(bin_path("llvm-ranlib", "exe"));

        // On a Windows host, we must use clang-cl for the compiler, which accepts MSVC-style arguments.
        // This is necessary for building native code, e.g., any crate that uses `cc` in its build script.
        // On all other hosts, we just use the regular clang.
        let cc_path = if matches!(host_os, HostOs::WindowsX64) {
            if !full_clang_path.is_file() || !full_clangcl_path.is_file() || !full_clangpp_path.is_file() {
                return Err(format!("please copy \"{}-clang.cmd\", \"{}-clang-cl.cmd\", and \"{}-clang++.cmd\" from  \"{}\\tools\\open_harmony\\cmd\" into \"{}\\native\\llvm\\bin\"",
                    target_triple,
                    target_triple,
                    target_triple,
                    cwd.display(),
                    sdk_path.display(),
                ));
            }
            &full_clangcl_path
        } else {
            &full_clang_path
        };

        let target_opt = format!("--target={target_triple}");
        let toolchain = target_triple.replace('-',"_");

        let base_args = &[
            "run",
            "nightly",
            "cargo",
            "rustc",
            "--verbose",
            "--lib",
            "--crate-type=cdylib",
            &target_opt
        ];
        let mut args_out = Vec::new();
        args_out.extend_from_slice(base_args);
        for arg in args {
            args_out.push(arg);
        }
        let makepad_env = std::env::var("MAKEPAD").unwrap_or("lines".to_string());
        shell_env(
            &[
                (&format!("CC_{toolchain}"),     cc_path.to_str().unwrap()),
                (&format!("CXX_{toolchain}"),    full_clangpp_path.to_str().unwrap()),
                (&format!("AR_{toolchain}"),     full_llvm_ar_path.to_str().unwrap()),
                (&format!("RANLIB_{toolchain}"), full_llvm_ranlib_path.to_str().unwrap()),
                // Use the regular clang for the linker.
                (&format!("CARGO_TARGET_{}_LINKER", toolchain.to_uppercase()), full_clang_path.to_str().unwrap()),

                ("MAKEPAD", &makepad_env),
            ],
            &cwd,
            "rustup",
            &args_out,
        )?;
    }
    Ok(())
}

fn create_deveco_project(args : &[String], targets :&[OpenHarmonyTarget]) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let build_crate = get_build_crate_from_args(args)?;
    let underscore_build_crate = build_crate.replace('-', "_");

    // TODO: don't use the current working dir here, instead, use cargo's target dir for crate being built
    let project_output_path = cwd.join("target").join("makepad-open-harmony").join(&underscore_build_crate);
    let resources_rawfile = project_output_path.join("entry").join("src").join("main").join("resources").join("rawfile");

    // Currently, the project template is stored in the `MAKEPAD/tools/openharmony/deveco` directory in the makepad git repo.
    let project_template_path = {
        let cargo_makepad_installation_path = env!("CARGO_MANIFEST_DIR");
        let cwd = std::env::current_dir().unwrap();
        let tools_openharmony_dir = cwd.join(cargo_makepad_installation_path)
            .parent()
            .expect("Unable to locate the `makepad/tools/` directory")
            .join("open_harmony");
        println!("tools_openharmony_dir: {}", tools_openharmony_dir.display());
        tools_openharmony_dir.join("deveco")
    };

    let _= rmdir(&project_output_path);
    mkdir(&project_output_path)?;
    cp_all(&project_template_path, &project_output_path, false)?;
    mkdir(&resources_rawfile)?;
    let app_cfg = project_output_path.join("AppScope").join("app.json5");
    if let Ok(mut app_file) = std::fs::File::create(app_cfg) {
        let _ = app_file.write_all(app_json(&underscore_build_crate).as_bytes());
    }
    let lable_path = project_output_path.join("entry").join("src").join("main").join("resources").join("base").join("element").join("string.json");
    if let Ok(mut label_file) = std::fs::File::create(lable_path) {
        let _ = label_file.write_all(label_json(&underscore_build_crate).as_bytes());
    }
    let label_us_path = project_output_path.join("entry").join("src").join("main").join("resources").join("en_US").join("element").join("string.json");
    if let Ok(mut label_file) = std::fs::File::create(label_us_path) {
        let _ = label_file.write_all(label_json(&underscore_build_crate).as_bytes());
    }
    let label_zh_path = project_output_path.join("entry").join("src").join("main").join("resources").join("zh_CN").join("element").join("string.json");
    if let Ok(mut label_file) = std::fs::File::create(label_zh_path) {
        let _ = label_file.write_all(label_zh_json(&underscore_build_crate).as_bytes());
    }

    add_dependencies(&args, &targets)
}

fn add_dependencies(args : &[String], targets :&[OpenHarmonyTarget]) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let build_crate = get_build_crate_from_args(args)?;
    let profile = get_profile_from_args(args);
    let underscore_build_crate = build_crate.replace('-', "_");

    let prj_path = cwd.join("target").join("makepad-open-harmony").join(&underscore_build_crate);
    let raw_file = prj_path.join("entry").join("src").join("main").join("resources").join("rawfile");
    let build_crate_dir = get_crate_dir(build_crate)?;
    let local_resources_path = build_crate_dir.join("resources");

    if local_resources_path.is_dir() {
        let dst_dir = raw_file.join("makepad").join(&underscore_build_crate).join("resources");
        let _ = rmdir(&dst_dir);
        mkdir(&dst_dir)?;
        cp_all(&local_resources_path, &dst_dir, false)?;
    }
    let build_dir =cwd.join("target").join(targets[0].target_triple_str()).join(profile.clone());
    let deps = get_crate_dep_dirs(build_crate, &build_dir, &targets[0].target_triple_str());
    for (name, dep_dir) in deps.iter() {
        let resources_path = dep_dir.join("resources");
        if resources_path.is_dir() {
            let name = name.replace('-', "_");
            let dst_dir = raw_file.join("makepad").join(name).join("resources");
            let _ = rmdir(&dst_dir);
            mkdir(&dst_dir)?;
            cp_all(&resources_path, &dst_dir, false)?;
        }
    }

    for target in targets {
        let target_dir = target.target_triple_str();
        let deveco_lib_dir = match target {
            OpenHarmonyTarget::Aarch64 => "arm64-v8a",
            OpenHarmonyTarget::X86_64 => "x86_64",
        };
        let src_lib = cwd.join("target").join(target_dir).join(profile.clone()).join(format!("lib{underscore_build_crate}.so"));
        let dst_lib = cwd.join("target").join("makepad-open-harmony").join(&underscore_build_crate).join("entry").join("libs").join(deveco_lib_dir).join("libmakepad.so");
        let _ = rm(&dst_lib);
        cp(&src_lib, &dst_lib, false)?;
    }
    Ok(())
}

fn build_hap(deveco_home: &Option<String>, args: &[String], host_os: &HostOs) -> Result<(), String> {
    let deveco_home = Path::new(deveco_home.as_ref().unwrap());
    let node_home = get_node_home(&deveco_home, &host_os)?;
    let deveco_sdk_home = get_deveco_sdk_home(&deveco_home, &host_os)?;
    let node_path = get_node_path(&deveco_home, &host_os)?;
    let hvigorw_path = get_hvigor_path(&deveco_home, &host_os)?;

    let cwd = std::env::current_dir().unwrap();
    let build_crate = get_build_crate_from_args(args)?;
    let underscore_build_crate = build_crate.replace('-', "_");
    let prj_path = cwd.join("target").join("makepad-open-harmony").join(underscore_build_crate);

    println!("{} {} clean --nodaemon", node_path.to_str().unwrap(), hvigorw_path.to_str().unwrap());
    shell_env(
        &[
            (&format!("DEVECO_SDK_HOME"), deveco_sdk_home.to_str().unwrap()),
            (&format!("NODE_HOME"), node_home.to_str().unwrap()),
        ],
        &prj_path,
        node_path.to_str().unwrap(),
        &[hvigorw_path.to_str().unwrap(), "clean", "--no-daemon"])?;

    println!("{} {} assembleHap --mode module -p product=default -p buildMode=release --no-daemon",
            node_path.to_str().unwrap(), hvigorw_path.to_str().unwrap());
    shell_env(
        &[
            (&format!("DEVECO_SDK_HOME"), deveco_sdk_home.to_str().unwrap()),
            (&format!("NODE_HOME"), node_home.to_str().unwrap()),
        ],
        &prj_path,
        node_path.to_str().unwrap(),
        &[hvigorw_path.to_str().unwrap(), "assembleHap", "--mode module", "-p product=default", "-p buildMode=release", "--no-daemon"])?;

    Ok(())
}

fn hdc_cmd(hdc_path: &Path, cwd:&Path, args: &[&str], hdc_remote :&Option<String>) -> Result<(), String> {
    if let Some(r) = hdc_remote {
        let mut new_args: Vec<&str> = vec!["-s", r.as_str()];
        for a in args {
            new_args.push(a);
        }
        print!("hdc");
        for a in &new_args{
            print!(" {}",a);
        }
        println!("");
        shell(&cwd,hdc_path.to_str().unwrap(), &new_args)?;

    } else {
        print!("hdc");
        for a in args{
            print!(" {}",a);
        }
        println!("");
        shell(&cwd,hdc_path.to_str().unwrap(),&args)?;
    }
    Ok(())
}


pub fn deveco(deveco_home: &Option<String>, args: &[String], host_os: &HostOs, targets :&[OpenHarmonyTarget]) ->  Result<(), String> {
    if deveco_home.is_none() {
        return Err("--deveco-home is not specified".to_owned());
    }
    rust_build(&deveco_home, &host_os, &args, &targets)?;
    create_deveco_project(args, &targets)?;
    Ok(())
}

pub fn build(deveco_home: &Option<String>, args: &[String], host_os: &HostOs, targets :&[OpenHarmonyTarget]) ->  Result<(), String> {
    check_deveco_prj(&args)?;
    rust_build(&deveco_home, &host_os, &args, &targets)?;
    add_dependencies(&args, &targets)?;
    build_hap(&deveco_home, &args, &host_os)?;
    Ok(())
}

pub fn run(deveco_home: &Option<String>, args: &[String], host_os: &HostOs, targets: &[OpenHarmonyTarget], hdc_remote: &Option<String>) ->  Result<(), String> {
    build(&deveco_home, &args, &host_os, &targets)?;
    let cwd = std::env::current_dir().unwrap();
    let deveco_home = Path::new(deveco_home.as_ref().unwrap());
    let hdc = get_hdc_path(&deveco_home, &host_os)?;
    let build_crate = get_build_crate_from_args(args)?;
    let underscore_build_crate = build_crate.replace('-', "_");
    let bundle = format!("dev.makepad.{underscore_build_crate}");

    let prj_path = cwd.join("target").join("makepad-open-harmony").join(&underscore_build_crate);
    let hap_path = prj_path.join("entry").join("build").join("default").join("outputs").join("default").join("makepad-default-signed.hap");

    if hap_path.is_file() == false {
        return Err("failed to generate signed hap package".to_owned());
    }

    hdc_cmd(&hdc, &prj_path, &["shell","aa", "force-stop", bundle.as_str()], &hdc_remote)?;

    let bundle_dir = format!("data/local/tmp/{underscore_build_crate}");
    let _ = hdc_cmd(&hdc, &prj_path, &["shell", "rm", "-rf", bundle_dir.as_str()], &hdc_remote);
    hdc_cmd(&hdc, &prj_path, &["shell", "mkdir", bundle_dir.as_str()], &hdc_remote)?;
    hdc_cmd(&hdc, &prj_path, &["file", "send", hap_path.to_str().unwrap(), bundle_dir.as_str()], &hdc_remote)?;
    hdc_cmd(&hdc, &prj_path, &["shell", "bm", "install", "-p", bundle_dir.as_str()], &hdc_remote)?;
    hdc_cmd(&hdc, &prj_path, &["shell", "rm", "-rf", bundle_dir.as_str()], &hdc_remote)?;
    hdc_cmd(&hdc, &prj_path, &["shell", "aa", "start", "-a", "EntryAbility", "-b", bundle.as_str()], &hdc_remote)?;
    Ok(())
}

pub fn hilog(deveco_home: &Option<String>, args: &[String], host_os: &HostOs, hdc_remote: &Option<String>) ->  Result<(), String>  {
    if deveco_home.is_none() {
        return Err("--deveco-home is not specified".to_owned());
    }
    let cwd = std::env::current_dir().unwrap();
    let deveco_home = Path::new(deveco_home.as_ref().unwrap());
    let hdc = get_hdc_path(&deveco_home, &host_os)?;
    let build_crate = get_build_crate_from_args(args)?;
    let underscore_build_crate = build_crate.replace('-', "_");
    let prj_path = cwd.join("target").join("makepad-open-harmony").join(underscore_build_crate);
    hdc_cmd(&hdc, &prj_path, &["hilog"], &hdc_remote)
}
