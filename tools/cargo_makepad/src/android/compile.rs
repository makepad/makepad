use super::sdk::{AndroidSDKUrls, BUILD_TOOLS_DIR, PLATFORMS_DIR};
use crate::android::{AndroidConfig, AndroidTarget, AndroidVariant, HostOs};
use crate::makepad_shell::*;
use crate::utils::*;
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

fn aapt_path(sdk_dir: &Path, urls: &AndroidSDKUrls) -> PathBuf {
    sdk_dir
        .join(BUILD_TOOLS_DIR)
        .join(urls.build_tools_version)
        .join("aapt")
}

fn d8_jar_path(sdk_dir: &Path, urls: &AndroidSDKUrls) -> PathBuf {
    sdk_dir
        .join(BUILD_TOOLS_DIR)
        .join(urls.build_tools_version)
        .join("lib/d8.jar")
}

fn apksigner_jar_path(sdk_dir: &Path, urls: &AndroidSDKUrls) -> PathBuf {
    sdk_dir
        .join(BUILD_TOOLS_DIR)
        .join(urls.build_tools_version)
        .join("lib/apksigner.jar")
}

fn zipalign_path(sdk_dir: &Path, urls: &AndroidSDKUrls) -> PathBuf {
    sdk_dir
        .join(BUILD_TOOLS_DIR)
        .join(urls.build_tools_version)
        .join("zipalign")
}

fn android_jar_path(sdk_dir: &Path, urls: &AndroidSDKUrls) -> PathBuf {
    sdk_dir
        .join(PLATFORMS_DIR)
        .join(urls.platform)
        .join("android.jar")
}

#[derive(Debug)]
struct BuildPaths {
    tmp_dir: PathBuf,
    out_dir: PathBuf,
    java_out_dir: PathBuf,
    res_dir: PathBuf,
    manifest_file: PathBuf,
    java_file: PathBuf,
    xr_file: PathBuf,
    dst_unaligned_apk: PathBuf,
    dst_apk: PathBuf,
}

pub struct BuildResult {
    dst_apk: PathBuf,
    java_url: String,
}

const SMALL_FONT_REPLACEMENTS: [(&str, &str); 5] = [
    ("GoNotoKurrent-Bold.ttf", "IBMPlexSans-SemiBold.ttf"),
    ("GoNotoKurrent-Regular.ttf", "IBMPlexSans-Text.ttf"),
    ("LXGWWenKaiBold.ttf", "IBMPlexSans-Text.ttf"),
    ("LXGWWenKaiRegular.ttf", "IBMPlexSans-Text.ttf"),
    ("NotoColorEmoji.ttf", "IBMPlexSans-Text.ttf"),
];

fn main_java(url: &str) -> String {
    format!(
        r#"
        package {url};
        import dev.makepad.android.MakepadActivity;
        public class MakepadApp extends MakepadActivity{{
            public void switchActivity(){{
                switchActivityClass(MakepadAppXr.class);
            }}
        }}
    "#
    )
}

fn xr_java(url: &str) -> String {
    format!(
        r#"
        package {url};
        import dev.makepad.android.MakepadActivity;
        public class MakepadAppXr extends MakepadActivity{{
            public void switchActivity(){{
                switchActivityClass(MakepadApp.class);
            }}
        }}
    "#
    )
}

fn has_explicit_lib_target(cargo_toml: &str, crate_dir: &Path) -> bool {
    crate_dir.join("src/lib.rs").is_file()
        || cargo_toml
            .lines()
            .any(|line| line.trim_start().starts_with("[lib]"))
}

fn normalize_toml_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn absolutize_manifest_path(crate_dir: &Path, value: &str) -> String {
    if value.contains("://") || Path::new(value).is_absolute() {
        return value.to_string();
    }
    let joined = crate_dir.join(value);
    normalize_toml_path(&joined.canonicalize().unwrap_or(joined))
}

fn rewrite_relative_toml_value(line: &mut String, key: &str, crate_dir: &Path) {
    for needle in [format!("{key} ="), format!("{key}=")] {
        let mut search_from = 0;
        loop {
            let Some(rel_pos) = line[search_from..].find(&needle) else {
                break;
            };
            let value_key_start = search_from + rel_pos;
            let mut quote_pos = value_key_start + needle.len();
            while line
                .as_bytes()
                .get(quote_pos)
                .is_some_and(|v| v.is_ascii_whitespace())
            {
                quote_pos += 1;
            }
            let Some(&quote) = line.as_bytes().get(quote_pos) else {
                break;
            };
            if quote != b'"' && quote != b'\'' {
                search_from = quote_pos.saturating_add(1);
                continue;
            }
            let mut value_end = quote_pos + 1;
            while let Some(&ch) = line.as_bytes().get(value_end) {
                if ch == quote && line.as_bytes().get(value_end.saturating_sub(1)) != Some(&b'\\') {
                    break;
                }
                value_end += 1;
            }
            if value_end >= line.len() {
                break;
            }

            let value = line[quote_pos + 1..value_end].to_string();
            let replacement = absolutize_manifest_path(crate_dir, &value);
            line.replace_range(quote_pos + 1..value_end, &replacement);
            search_from = quote_pos + replacement.len() + 2;
        }
    }
}

fn rewrite_wrapper_manifest_paths(cargo_toml: &str, crate_dir: &Path) -> String {
    let mut out = String::with_capacity(cargo_toml.len() + 256);
    for raw_line in cargo_toml.lines() {
        let mut line = raw_line.to_string();
        for key in ["path", "build", "readme", "license-file"] {
            rewrite_relative_toml_value(&mut line, key, crate_dir);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn extract_workspace_patch_sections(workspace_manifest: &str) -> String {
    let mut out = String::new();
    let mut current_section: Option<String> = None;
    let mut current_body = Vec::new();

    let flush_section =
        |out: &mut String, current_section: &mut Option<String>, current_body: &mut Vec<String>| {
            let Some(section) = current_section.take() else {
                current_body.clear();
                return;
            };
            if !section.starts_with("[patch.") {
                current_body.clear();
                return;
            }

            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&section);
            out.push('\n');
            for line in current_body.iter() {
                out.push_str(line);
                out.push('\n');
            }
            current_body.clear();
        };

    for raw_line in workspace_manifest.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') && !raw_line.starts_with(' ') {
            flush_section(&mut out, &mut current_section, &mut current_body);
            current_section = Some(trimmed.to_string());
            continue;
        }

        if current_section.is_some() {
            current_body.push(raw_line.to_string());
        }
    }

    flush_section(&mut out, &mut current_section, &mut current_body);
    out
}

fn strip_generated_wrapper_args(args: &[String], build_crate: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    let mut removed_positional = false;

    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }

        let skip_arg = matches!(
            arg.as_str(),
            "-p" | "--package"
                | "--manifest-path"
                | "--exclude"
                | "--bin"
                | "--example"
                | "--test"
                | "--bench"
        );
        if skip_arg {
            skip_next = true;
            continue;
        }

        let skip_prefixed = arg.starts_with("--package=")
            || arg.starts_with("--manifest-path=")
            || arg.starts_with("--exclude=")
            || arg.starts_with("--bin=")
            || arg.starts_with("--example=")
            || arg.starts_with("--test=")
            || arg.starts_with("--bench=");
        if skip_prefixed
            || matches!(
                arg.as_str(),
                "--workspace"
                    | "--all-targets"
                    | "--bins"
                    | "--examples"
                    | "--tests"
                    | "--benches"
                    | "--lib"
            )
        {
            continue;
        }

        if !removed_positional && !arg.starts_with('-') && arg == build_crate {
            removed_positional = true;
            continue;
        }

        out.push(arg.clone());
    }

    out
}

fn generate_android_wrapper_manifest(
    build_crate: &str,
    target_root: &Path,
) -> Result<Option<PathBuf>, String> {
    let workspace_root = std::env::current_dir().unwrap();
    let crate_dir = get_crate_dir(build_crate)?;
    let cargo_toml_path = crate_dir.join("Cargo.toml");
    let cargo_toml = fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("Can't read {:?}: {:?}", cargo_toml_path, e))?;

    if has_explicit_lib_target(&cargo_toml, &crate_dir) {
        return Ok(None);
    }

    let main_rs = crate_dir.join("src/main.rs");
    if !main_rs.is_file() {
        return Err(format!(
            "Package {build_crate} has no library target and no src/main.rs to wrap for Android"
        ));
    }

    let wrapper_dir = target_root
        .join("makepad-android-wrapper")
        .join(build_crate.replace('-', "_"));
    let _ = rmdir(&wrapper_dir);
    mkdir(&wrapper_dir)?;

    let mut wrapper_manifest = rewrite_wrapper_manifest_paths(&cargo_toml, &crate_dir);
    wrapper_manifest.push_str("\n[lib]\n");
    wrapper_manifest.push_str(&format!("path = \"{}\"\n", normalize_toml_path(&main_rs)));
    wrapper_manifest.push_str("\n[workspace]\n");
    wrapper_manifest.push_str("resolver = \"2\"\n");

    let workspace_manifest_path = workspace_root.join("Cargo.toml");
    if let Ok(workspace_manifest) = fs::read_to_string(&workspace_manifest_path) {
        let workspace_patches = extract_workspace_patch_sections(&workspace_manifest);
        if !workspace_patches.trim().is_empty() {
            wrapper_manifest.push('\n');
            wrapper_manifest.push_str(&rewrite_wrapper_manifest_paths(
                &workspace_patches,
                &workspace_root,
            ));
        }
    }

    let wrapper_manifest_path = wrapper_dir.join("Cargo.toml");
    fs::write(&wrapper_manifest_path, wrapper_manifest)
        .map_err(|e| format!("Can't write {:?}: {:?}", wrapper_manifest_path, e))?;

    if let Ok(lock_data) = fs::read(crate_dir.join("Cargo.lock"))
        .or_else(|_| fs::read(std::env::current_dir().unwrap().join("Cargo.lock")))
    {
        let _ = fs::write(wrapper_dir.join("Cargo.lock"), lock_data);
    }

    Ok(Some(wrapper_manifest_path))
}

fn rust_build(
    sdk_dir: &Path,
    host_os: HostOs,
    build_crate: &str,
    args: &[String],
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let target_root = cargo_target_root(&cwd);
    let target_dir = cargo_target_dir(&cwd);
    let target_dir_str = target_dir.to_string_lossy().to_string();
    let wrapper_manifest = generate_android_wrapper_manifest(build_crate, &target_root)?;
    let cargo_cwd = wrapper_manifest
        .as_ref()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.clone());
    let cargo_args = if let Some(wrapper_manifest) = &wrapper_manifest {
        let mut cargo_args = vec![format!(
            "--manifest-path={}",
            normalize_toml_path(wrapper_manifest)
        )];
        cargo_args.extend(strip_generated_wrapper_args(args, build_crate));
        cargo_args
    } else {
        args.to_vec()
    };
    let (_ndk_version, ndk_prebuilt_root) =
        resolve_ndk_prebuilt_root(sdk_dir, host_os, urls.ndk_version_full)?;
    for android_target in android_targets {
        let clang_filename = format!("{}{}-clang", android_target.clang(), urls.sdk_version);

        let bin_name = |bin_filename: &str, windows_extension: &str| match host_os {
            HostOs::WindowsX64 => format!("{bin_filename}.{windows_extension}"),
            HostOs::MacosX64 | HostOs::MacosAarch64 | HostOs::LinuxX64 => bin_filename.to_string(),
            _ => panic!(),
        };
        let full_clang_path = ndk_prebuilt_root
            .join("bin")
            .join(bin_name(&clang_filename, "cmd"));
        let full_llvm_ar_path = ndk_prebuilt_root
            .join("bin")
            .join(bin_name("llvm-ar", "exe"));
        let full_llvm_ranlib_path = ndk_prebuilt_root
            .join("bin")
            .join(bin_name("llvm-ranlib", "exe"));

        let toolchain = android_target.toolchain();
        let target_opt = format!("--target={toolchain}");
        let target_dir_arg = format!("--target-dir={target_dir_str}");

        let base_args = &[
            "run",
            "nightly",
            "cargo",
            "rustc",
            "--lib",
            "--crate-type=cdylib",
            &target_opt,
            &target_dir_arg,
        ];
        let mut args_out = Vec::new();
        args_out.extend_from_slice(base_args);
        for arg in &cargo_args {
            args_out.push(arg);
        }

        let target_arch_str = android_target.to_str();
        let cfg_flag = format!("--cfg android_target=\"{}\"", target_arch_str);

        let makepad_env = if let AndroidVariant::Quest = variant {
            Some(match std::env::var("MAKEPAD") {
                Ok(makepad_env) if !makepad_env.is_empty() => format!("{makepad_env}+quest"),
                _ => "quest".to_string(),
            })
        } else {
            std::env::var("MAKEPAD")
                .ok()
                .filter(|value| !value.is_empty())
        };

        let android_sdk_version = urls.sdk_version.to_string();
        let java_home = sdk_dir.join("openjdk").to_string_lossy().to_string();
        let mut env: Vec<(String, String)> = vec![
            (
                android_target.linker_env_var().to_string(),
                full_clang_path.to_string_lossy().to_string(),
            ),
            (
                "ANDROID_HOME".to_string(),
                sdk_dir.to_string_lossy().to_string(),
            ),
            (
                "ANDROID_SDK_ROOT".to_string(),
                sdk_dir.to_string_lossy().to_string(),
            ),
            (
                "ANDROID_BUILD_TOOLS_VERSION".to_string(),
                urls.build_tools_version.to_string(),
            ),
            ("ANDROID_PLATFORM".to_string(), urls.platform.to_string()),
            (
                "ANDROID_SDK_VERSION".to_string(),
                android_sdk_version.clone(),
            ),
            ("ANDROID_API_LEVEL".to_string(), android_sdk_version),
            (
                "ANDROID_SDK_EXTENSION".to_string(),
                urls.sdk_extension.to_string(),
            ),
            ("JAVA_HOME".to_string(), java_home),
            (
                format!("CC_{toolchain}"),
                full_clang_path.to_string_lossy().to_string(),
            ),
            (
                format!("AR_{toolchain}"),
                full_llvm_ar_path.to_string_lossy().to_string(),
            ),
            (
                format!("RANLIB_{toolchain}"),
                full_llvm_ranlib_path.to_string_lossy().to_string(),
            ),
            ("RUSTFLAGS".to_string(), cfg_flag.clone()),
        ];
        if let Some(makepad_env) = makepad_env {
            env.push(("MAKEPAD".to_string(), makepad_env));
        }
        let env_refs = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();

        shell_env(&env_refs, &cargo_cwd, "rustup", &args_out)?;
    }

    Ok(())
}

/// Resolve the cargo target directory for android builds.
/// Defaults to `target/android` to avoid invalidating desktop build caches.
fn cargo_target_root(cwd: &Path) -> PathBuf {
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        if target_dir.is_absolute() {
            target_dir
        } else {
            cwd.join(target_dir)
        }
    } else {
        cwd.join("target")
    }
}

fn cargo_target_dir(cwd: &Path) -> PathBuf {
    if std::env::var_os("CARGO_TARGET_DIR").is_some() {
        cargo_target_root(cwd)
    } else {
        cargo_target_root(cwd).join("android")
    }
}

fn prepare_build(
    build_crate: &str,
    java_url: &str,
    app_label: &str,
    variant: &AndroidVariant,
    urls: &AndroidSDKUrls,
) -> Result<BuildPaths, String> {
    let cwd = std::env::current_dir().unwrap();
    let target_dir = cargo_target_dir(&cwd);
    let underscore_build_crate = build_crate.replace('-', "_");

    let tmp_dir = target_dir
        .join("makepad-android-apk")
        .join(&underscore_build_crate)
        .join("tmp");
    let out_dir = target_dir
        .join("makepad-android-apk")
        .join(&underscore_build_crate)
        .join("apk");
    let java_out_dir = target_dir
        .join("makepad-android-apk")
        .join(&underscore_build_crate)
        .join("java");
    let res_dir = tmp_dir.join("res");

    let _ = rmdir(&tmp_dir);
    let _ = rmdir(&out_dir);
    mkdir(&tmp_dir)?;
    mkdir(&out_dir)?;
    mkdir(&java_out_dir)?;

    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    cp_all(&cargo_manifest_dir.join("src/android/res"), &res_dir, false)?;

    let build_crate_dir = get_crate_dir(build_crate)?;
    let app_android_res = build_crate_dir.join("resources/android/res");
    if app_android_res.is_dir() {
        cp_all(&app_android_res, &res_dir, false)?;
    }

    let android_icon_targets = [
        "mipmap-mdpi",
        "mipmap-hdpi",
        "mipmap-xhdpi",
        "mipmap-xxhdpi",
        "mipmap-xxxhdpi",
    ];
    let has_android_icon = android_icon_targets
        .iter()
        .all(|d| res_dir.join(d).join("ic_launcher.png").is_file());
    if !has_android_icon && !no_icon_requested() {
        eprintln!(
            "warning: missing Android launcher icons under {}. Add mipmap-*/ic_launcher.png files, or pass --no-icon to suppress this check.",
            res_dir.display()
        );
    }

    let manifest_xml = variant.manifest_xml(
        app_label,
        "MakepadApp",
        java_url,
        urls.sdk_version,
        has_android_icon,
    );
    let manifest_file = tmp_dir.join("AndroidManifest.xml");
    write_text(&manifest_file, &manifest_xml)?;

    let main_java = main_java(java_url);
    let java_path = java_url.replace('.', "/");
    let java_file = tmp_dir.join(&java_path).join("MakepadApp.java");
    write_text(&java_file, &main_java)?;

    let xr_java = xr_java(java_url);
    let xr_file = tmp_dir.join(&java_path).join("MakepadAppXr.java");
    write_text(&xr_file, &xr_java)?;

    let apk_filename = to_snakecase(app_label);
    let dst_unaligned_apk = out_dir.join(format!("{apk_filename}.unaligned.apk"));
    let dst_apk = out_dir.join(format!("{apk_filename}.apk"));

    let _ = rm(&dst_unaligned_apk);
    let _ = rm(&dst_apk);

    Ok(BuildPaths {
        tmp_dir,
        out_dir,
        java_out_dir,
        res_dir,
        manifest_file,
        java_file,
        xr_file,
        dst_unaligned_apk,
        dst_apk,
    })
}

fn build_r_class(
    sdk_dir: &Path,
    build_paths: &BuildPaths,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        &aapt_path(sdk_dir, urls).to_str().unwrap(),
        &[
            "package",
            "-f",
            "-m",
            "-I",
            (android_jar_path(sdk_dir, urls).to_str().unwrap()),
            "-S",
            (build_paths.res_dir.to_str().unwrap()),
            "-M",
            (build_paths.manifest_file.to_str().unwrap()),
            "-J",
            (build_paths.tmp_dir.to_str().unwrap()),
            "--custom-package",
            "dev.makepad.android",
            (build_paths.out_dir.to_str().unwrap()),
        ],
    )?;

    Ok(())
}

fn compile_java(
    sdk_dir: &Path,
    build_paths: &BuildPaths,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let makepad_package_path = "dev/makepad/android";
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();
    let javac_stamp = build_paths.java_out_dir.join("javac.inputs");

    let r_class_path = build_paths
        .tmp_dir
        .join(makepad_package_path)
        .join("R.java");
    let makepad_java_classes_dir = &cargo_manifest_dir
        .join("src/android/java/")
        .join(makepad_package_path);
    let java_sources = vec![
        r_class_path.clone(),
        makepad_java_classes_dir.join("MakepadNative.java"),
        makepad_java_classes_dir.join("MakepadActivity.java"),
        makepad_java_classes_dir.join("MakepadInputConnection.java"),
        makepad_java_classes_dir.join("MakepadNetwork.java"),
        makepad_java_classes_dir.join("MakepadSocketStream.java"),
        makepad_java_classes_dir.join("MakepadWebSocket.java"),
        makepad_java_classes_dir.join("MakepadWebSocketReader.java"),
        makepad_java_classes_dir.join("ByteArrayMediaDataSource.java"),
        makepad_java_classes_dir.join("VideoPlayer.java"),
        makepad_java_classes_dir.join("VideoPlayerRunnable.java"),
        makepad_java_classes_dir.join("H264Encoder.java"),
        build_paths.java_file.clone(),
        build_paths.xr_file.clone(),
    ];

    let mut hasher = DefaultHasher::new();
    for source in &java_sources {
        source.to_string_lossy().hash(&mut hasher);
        fs::read(source)
            .map_err(|e| format!("failed to read Java source {:?}: {e}", source))?
            .hash(&mut hasher);
    }
    let java_inputs_hash = format!("{:016x}", hasher.finish());

    let app_class_dir = build_paths
        .java_file
        .parent()
        .and_then(|path| path.strip_prefix(&build_paths.tmp_dir).ok())
        .ok_or_else(|| {
            format!(
                "failed to resolve Java output package for {:?}",
                build_paths.java_file
            )
        })?;
    let expected_outputs = [
        build_paths.java_out_dir.join("dev/makepad/android/R.class"),
        build_paths
            .java_out_dir
            .join("dev/makepad/android/MakepadActivity.class"),
        build_paths
            .java_out_dir
            .join(app_class_dir)
            .join("MakepadApp.class"),
        build_paths
            .java_out_dir
            .join(app_class_dir)
            .join("MakepadAppXr.class"),
    ];

    if fs::read_to_string(&javac_stamp)
        .map(|cached| cached.trim() == java_inputs_hash)
        .unwrap_or(false)
        && expected_outputs.iter().all(|path| path.is_file())
    {
        return Ok(());
    }

    let android_jar = android_jar_path(sdk_dir, urls);
    let _ = rmdir(&build_paths.java_out_dir);
    mkdir(&build_paths.java_out_dir)?;
    let mut javac_args = vec![
        "-source",
        "1.8",
        "-target",
        "1.8",
        "-Xlint:-options",
        "-classpath",
        android_jar.to_str().unwrap(),
        "-Xlint:deprecation",
        "-d",
        build_paths.java_out_dir.to_str().unwrap(),
    ];
    for source in &java_sources {
        javac_args.push(source.to_str().unwrap());
    }

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/javac").to_str().unwrap(),
        &javac_args,
    )?;
    write_text(&javac_stamp, &java_inputs_hash)?;

    Ok(())
}

fn build_dex(
    sdk_dir: &Path,
    build_paths: &BuildPaths,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();

    let mut class_files: Vec<PathBuf> = ls(&build_paths.java_out_dir)?
        .into_iter()
        .filter(|rel| rel.extension().and_then(|ext| ext.to_str()) == Some("class"))
        .map(|rel| build_paths.java_out_dir.join(rel))
        .collect();

    class_files.sort();

    if class_files.is_empty() {
        return Err(format!(
            "No compiled Java class files found in {:?}",
            build_paths.java_out_dir
        ));
    }

    let _ = rmdir(&build_paths.out_dir);
    mkdir(&build_paths.out_dir)?;

    let d8_jar = d8_jar_path(sdk_dir, urls);
    let android_jar = android_jar_path(sdk_dir, urls);

    let mut args: Vec<&str> = vec![
        "-cp",
        d8_jar.to_str().unwrap(),
        "com.android.tools.r8.D8",
        "--classpath",
        android_jar.to_str().unwrap(),
        "--output",
        build_paths.out_dir.to_str().unwrap(),
    ];

    for class_file in &class_files {
        args.push(class_file.to_str().unwrap());
    }

    shell_env_cap(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &args,
    )?;

    Ok(())
}

fn build_unaligned_apk(
    sdk_dir: &Path,
    build_paths: &BuildPaths,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        aapt_path(sdk_dir, urls).to_str().unwrap(),
        &[
            "package",
            "-f",
            "-F",
            (build_paths.dst_unaligned_apk.to_str().unwrap()),
            "-I",
            (android_jar_path(sdk_dir, urls).to_str().unwrap()),
            "-M",
            (build_paths.manifest_file.to_str().unwrap()),
            "-S",
            (build_paths.res_dir.to_str().unwrap()),
            (build_paths.out_dir.to_str().unwrap()),
        ],
    )?;

    Ok(())
}

/// Returns the NDK prebuilt host directory name for the given host OS.
fn ndk_prebuilt_dir_candidates(host_os: HostOs) -> &'static [&'static str] {
    match host_os {
        HostOs::MacosX64 => &["darwin-x86_64"],
        // On Apple Silicon, older NDKs only ship darwin-x86_64 prebuilts.
        HostOs::MacosAarch64 => &["darwin-aarch64", "darwin-x86_64"],
        HostOs::WindowsX64 => &["windows-x86_64"],
        HostOs::LinuxX64 => &["linux-x86_64"],
        _ => panic!("Unsupported host OS"),
    }
}

fn ndk_version_sort_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

fn resolve_ndk_prebuilt_root(
    sdk_dir: &Path,
    host_os: HostOs,
    preferred_version: &str,
) -> Result<(String, PathBuf), String> {
    let prebuilt_candidates = ndk_prebuilt_dir_candidates(host_os);
    let ndk_root = sdk_dir.join("ndk");
    if !ndk_root.is_dir() {
        return Err(format!(
            "Android NDK directory not found: {:?}. Run `cargo makepad android install-toolchain` or copy an NDK into `ndk/<version>`.",
            ndk_root
        ));
    }

    let mut versions = Vec::new();
    for entry in
        std::fs::read_dir(&ndk_root).map_err(|e| format!("failed to read {:?}: {e}", ndk_root))?
    {
        let entry =
            entry.map_err(|e| format!("failed to read NDK entry in {:?}: {e}", ndk_root))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(version) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        for prebuilt in prebuilt_candidates {
            let prebuilt_root = path.join("toolchains/llvm/prebuilt").join(prebuilt);
            if prebuilt_root.is_dir() {
                versions.push((version.to_string(), prebuilt_root));
                break;
            }
        }
    }

    if versions.is_empty() {
        return Err(format!(
            "No compatible NDK toolchain found under {:?} for host prebuilts {:?}",
            ndk_root, prebuilt_candidates
        ));
    }

    if let Some((version, root)) = versions
        .iter()
        .find(|(version, _)| version == preferred_version)
    {
        return Ok((version.clone(), root.clone()));
    }

    versions.sort_by(|(a, _), (b, _)| ndk_version_sort_key(b).cmp(&ndk_version_sort_key(a)));
    Ok(versions.remove(0))
}

/// Scan an ELF shared library for NEEDED entries using the NDK's llvm-readelf,
/// then bundle any non-system shared libraries found in the NDK sysroot.
///
/// System libraries (libc.so, libm.so, libdl.so, liblog.so, etc.) live on the
/// device and must NOT be bundled.  We detect "NDK-provided, non-system" libs by
/// checking whether the file exists in the sysroot's `usr/lib/<triple>/` directory
/// (the base dir, not the API-level sub-directory which only contains OS stubs).
fn bundle_ndk_shared_deps(
    sdk_dir: &Path,
    host_os: HostOs,
    urls: &AndroidSDKUrls,
    android_target: &AndroidTarget,
    so_path: &Path,
    abi: &str,
    build_paths: &BuildPaths,
) -> Result<(), String> {
    let (_ndk_version, ndk_prebuilt_root) =
        resolve_ndk_prebuilt_root(sdk_dir, host_os, urls.ndk_version_full)?;

    // Path to llvm-readelf shipped with the NDK.
    let readelf_path = ndk_prebuilt_root.join("bin/llvm-readelf");
    if !readelf_path.exists() {
        // Gracefully skip when the NDK toolchain doesn't include llvm-readelf
        // (e.g. a stripped SDK install).
        return Ok(());
    }

    // Run `llvm-readelf -d <so>` to list dynamic section entries.
    let cwd = std::env::current_dir().unwrap();
    let output = shell_env_cap(
        &[],
        &cwd,
        readelf_path.to_str().unwrap(),
        &["-d", so_path.to_str().unwrap()],
    )?;

    // The NDK sysroot lib directory for this target triple (base dir, NOT the
    // API-level subdirectory).  Files here that are real .so's (not linker
    // scripts / stubs) are NDK-provided and must be shipped inside the APK.
    let clang_triple = android_target.clang();
    let sysroot_lib_dir = ndk_prebuilt_root.join("sysroot/usr/lib").join(clang_triple);

    // Parse NEEDED entries from readelf output.  Each relevant line looks like:
    //   0x0000000000000001 (NEEDED) Shared library: [libc++_shared.so]
    for line in output.lines() {
        if !line.contains("(NEEDED)") {
            continue;
        }
        // Extract the library name between square brackets.
        let lib_name = match line.find('[').and_then(|start| {
            line[start + 1..]
                .find(']')
                .map(|end| &line[start + 1..start + 1 + end])
        }) {
            Some(name) => name,
            None => continue,
        };

        let candidate = sysroot_lib_dir.join(lib_name);
        if !candidate.exists() || !candidate.is_file() {
            // Either a system lib (only present on device) or not a real file —
            // nothing to bundle.
            continue;
        }

        // Extra guard: if the same filename also exists in the API-level
        // subdirectory it is an OS-provided stub and should NOT be bundled.
        let api_level_stub = sysroot_lib_dir
            .join(urls.sdk_version.to_string())
            .join(lib_name);
        if api_level_stub.exists() {
            continue;
        }

        // Copy the NDK-provided shared library into the APK.
        let binary_path = format!("lib/{abi}/{lib_name}");
        let dst_lib = build_paths.out_dir.join(&binary_path);
        cp(&candidate, &dst_lib, false)?;

        shell_env_cap(
            &[],
            &build_paths.out_dir,
            aapt_path(sdk_dir, urls).to_str().unwrap(),
            &[
                "add",
                build_paths.dst_unaligned_apk.to_str().unwrap(),
                &binary_path,
            ],
        )?;

        println!("  Bundled NDK shared dep: {lib_name} (for {abi})");
    }
    Ok(())
}

fn add_rust_library(
    sdk_dir: &Path,
    host_os: HostOs,
    underscore_target: &str,
    build_paths: &BuildPaths,
    android_targets: &[AndroidTarget],
    args: &[String],
    variant: &AndroidVariant,
    urls: &AndroidSDKUrls,
) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().unwrap();
    let target_dir = cargo_target_dir(&cwd);
    let profile = get_profile_from_args(args);
    let mut build_dir = None;
    for android_target in android_targets {
        let abi = android_target.abi_identifier();
        mkdir(&build_paths.out_dir.join(format!("lib/{abi}")))?;

        let android_target_dir = android_target.toolchain();
        let binary_path = format!("lib/{abi}/libmakepad.so");
        if profile == "debug" {
            println!("WARNING - compiling a DEBUG build of the application, this creates a very slow and big app. Try adding --release for a fast, or --profile=small for a small build.");
        }
        let src_lib = target_dir.join(format!(
            "{android_target_dir}/{profile}/lib{underscore_target}.so"
        ));
        build_dir = Some(target_dir.join(format!("{android_target_dir}/{profile}")));
        let dst_lib = build_paths.out_dir.join(binary_path.clone());
        cp(&src_lib, &dst_lib, false)?;

        shell_env_cap(
            &[],
            &build_paths.out_dir,
            aapt_path(sdk_dir, urls).to_str().unwrap(),
            &[
                "add",
                (build_paths.dst_unaligned_apk.to_str().unwrap()),
                &binary_path,
            ],
        )?;

        // Scan libmakepad.so for NEEDED shared library dependencies and bundle
        // any that come from the NDK sysroot (e.g. libc++_shared.so).
        bundle_ndk_shared_deps(
            sdk_dir,
            host_os,
            urls,
            android_target,
            &dst_lib,
            abi,
            build_paths,
        )?;
    }
    // for the quest variant add the precompiled openXR loader
    if let AndroidVariant::Quest = variant {
        let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

        for (binary_path, src_lib) in [
            (
                "lib/arm64-v8a/libopenxr_loader.so",
                "quest/libopenxr_loader.so",
            ),
            //("lib/arm64-v8a/libktx.so", "tools/cargo_makepad/quest/libktx.so"),
            //("lib/arm64-v8a/libktx_read.so", "tools/cargo_makepad/quest/libktx_read.so"),
            //("lib/arm64-v8a/libobjUtil.a", "tools/cargo_makepad/quest/libobjUtil.a"),
        ] {
            //let binary_path = format!("lib/arm64-v8a/libopenxr_loader.so");
            let src_lib = cargo_manifest_dir.join(src_lib);
            let dst_lib = build_paths.out_dir.join(binary_path);
            cp(&src_lib, &dst_lib, false)?;
            shell_env_cap(
                &[],
                &build_paths.out_dir,
                aapt_path(sdk_dir, urls).to_str().unwrap(),
                &[
                    "add",
                    (build_paths.dst_unaligned_apk.to_str().unwrap()),
                    &binary_path,
                ],
            )?;
        }
    }

    Ok(build_dir.unwrap())
}

fn add_resources(
    sdk_dir: &Path,
    build_crate: &str,
    build_paths: &BuildPaths,
    build_dir: &Path,
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    config: &AndroidConfig,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let mut assets_to_add: Vec<String> = Vec::new();

    let build_crate_dir = get_crate_dir(build_crate)?;
    add_assets_dir_to_apk(
        &build_paths.out_dir,
        &mut assets_to_add,
        build_crate,
        &build_crate_dir.join("resources"),
        "resources",
        config,
    )?;
    add_font_assets_dir_to_apk(
        &build_paths.out_dir,
        &mut assets_to_add,
        build_crate,
        &build_crate_dir.join("fonts"),
        &build_crate_dir.join("resources"),
        config,
    )?;

    let deps = get_crate_dep_dirs(build_crate, &build_dir, &android_targets[0].toolchain());
    for (name, dep_dir) in deps.iter() {
        add_assets_dir_to_apk(
            &build_paths.out_dir,
            &mut assets_to_add,
            name,
            &dep_dir.join("resources"),
            "resources",
            config,
        )?;
        add_font_assets_dir_to_apk(
            &build_paths.out_dir,
            &mut assets_to_add,
            name,
            &dep_dir.join("fonts"),
            &dep_dir.join("resources"),
            config,
        )?;
    }
    // FIX THIS PROPER
    // On quest remove most of the widget resourcse
    if let AndroidVariant::Quest = variant {
        let dst_dir = build_paths
            .out_dir
            .join(format!("assets/makepad/makepad_widgets/resources"));
        let remove = [
            "fa-solid-900.ttf",
            //"LXGWWenKaiBold.ttf",
            "LiberationMono-Regular.ttf",
            //"GoNotoKurrent-Bold.ttf",
            // "NotoColorEmoji.ttf",
            //"IBMPlexSans-SemiBold.ttf",
            "NotoSans-Regular.ttf",
        ];
        for remove in remove {
            assets_to_add.retain(|v| !v.contains(remove));
            rm(&dst_dir.join(remove))?;
        }
    }

    let mut aapt_args = vec!["add", build_paths.dst_unaligned_apk.to_str().unwrap()];
    for asset in &assets_to_add {
        aapt_args.push(asset);
    }

    shell_env_cap(
        &[],
        &build_paths.out_dir,
        aapt_path(sdk_dir, urls).to_str().unwrap(),
        &aapt_args,
    )?;

    Ok(())
}

fn add_assets_dir_to_apk(
    out_dir: &Path,
    assets_to_add: &mut Vec<String>,
    crate_name: &str,
    source_dir: &Path,
    asset_subdir: &str,
    config: &AndroidConfig,
) -> Result<(), String> {
    if !source_dir.is_dir() {
        return Ok(());
    }

    let crate_name = crate_name.replace('-', "_");
    let dst_dir = out_dir.join(format!("assets/makepad/{crate_name}/{asset_subdir}"));
    mkdir(&dst_dir)?;
    cp_all(source_dir, &dst_dir, false)?;
    if config.small_fonts && asset_subdir == "resources" {
        for (target_name, replacement_name) in SMALL_FONT_REPLACEMENTS {
            let replacement = source_dir.join(replacement_name);
            let target = dst_dir.join(target_name);
            if replacement.is_file() && target.is_file() {
                cp(&replacement, &target, false)?;
            }
        }
    }

    let assets = ls(&dst_dir)?;
    for path in &assets {
        let path = path.display().to_string().replace("\\", "/");
        assets_to_add.push(format!("assets/makepad/{crate_name}/{asset_subdir}/{path}"));
    }
    Ok(())
}

fn add_font_assets_dir_to_apk(
    out_dir: &Path,
    assets_to_add: &mut Vec<String>,
    crate_name: &str,
    source_dir: &Path,
    resource_dir: &Path,
    config: &AndroidConfig,
) -> Result<(), String> {
    if !source_dir.is_dir() {
        return Ok(());
    }

    let crate_name = crate_name.replace('-', "_");
    let dst_dir = out_dir.join(format!("assets/makepad/{crate_name}/fonts"));
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
        let path = path.display().to_string().replace("\\", "/");
        assets_to_add.push(format!("assets/makepad/{crate_name}/fonts/{path}"));
    }
    if config.small_fonts {
        for (target_name, replacement_name) in SMALL_FONT_REPLACEMENTS {
            let replacement = source_dir
                .join(replacement_name)
                .is_file()
                .then(|| source_dir.join(replacement_name))
                .or_else(|| {
                    resource_dir
                        .join(replacement_name)
                        .is_file()
                        .then(|| resource_dir.join(replacement_name))
                });
            let target = dst_dir.join(target_name);
            if let Some(replacement) = replacement {
                if target.is_file() {
                    cp(&replacement, &target, false)?;
                }
            }
        }
    }
    Ok(())
}

fn build_zipaligned_apk(
    sdk_dir: &Path,
    build_paths: &BuildPaths,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    shell_env_cap(
        &[],
        &build_paths.out_dir,
        zipalign_path(sdk_dir, urls).to_str().unwrap(),
        &[
            "-v",
            "-f",
            "4",
            (build_paths.dst_unaligned_apk.to_str().unwrap()),
            (build_paths.dst_apk.to_str().unwrap()),
        ],
    )?;

    Ok(())
}

fn sign_apk(sdk_dir: &Path, build_paths: &BuildPaths, urls: &AndroidSDKUrls) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_home = sdk_dir.join("openjdk");

    shell_env_cap(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &[
            "-jar",
            (apksigner_jar_path(sdk_dir, urls).to_str().unwrap()),
            "sign",
            "-v",
            "-ks",
            (cargo_manifest_dir.join("debug.keystore").to_str().unwrap()),
            "--ks-key-alias",
            "androiddebugkey",
            "--ks-pass",
            "pass:android",
            (build_paths.dst_apk.to_str().unwrap()),
        ],
    )?;

    Ok(())
}

pub fn build(
    sdk_dir: &Path,
    host_os: HostOs,
    package_name: Option<String>,
    app_label: Option<String>,
    args: &[String],
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    config: &AndroidConfig,
    urls: &AndroidSDKUrls,
) -> Result<BuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    let binary_name =
        get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
    let underscore_binary_name = binary_name.replace('-', "_");
    let underscore_build_crate = build_crate.replace('-', "_");

    let java_url = package_name.unwrap_or_else(|| format!("dev.makepad.{underscore_binary_name}"));
    let app_label = app_label.unwrap_or_else(|| underscore_binary_name.clone());

    if let Some(icon) = resolve_app_icon_env(build_crate)? {
        for (var, value) in APP_ICON_ENV_VARS.iter().zip(icon.iter()) {
            std::env::set_var(var, value);
        }
    }

    rust_build(
        sdk_dir,
        host_os,
        build_crate,
        args,
        android_targets,
        variant,
        urls,
    )?;
    let build_paths = prepare_build(build_crate, &java_url, &app_label, variant, urls)?;

    println!("Building APK");
    build_r_class(sdk_dir, &build_paths, urls)?;
    compile_java(sdk_dir, &build_paths, urls)?;
    build_dex(sdk_dir, &build_paths, urls)?;
    build_unaligned_apk(sdk_dir, &build_paths, urls)?;
    let build_dir = add_rust_library(
        sdk_dir,
        host_os,
        &underscore_build_crate,
        &build_paths,
        android_targets,
        args,
        variant,
        urls,
    )?;
    add_resources(
        sdk_dir,
        build_crate,
        &build_paths,
        &build_dir,
        android_targets,
        variant,
        config,
        urls,
    )?;
    build_zipaligned_apk(sdk_dir, &build_paths, urls)?;
    sign_apk(sdk_dir, &build_paths, urls)?;

    println!("APK Build completed");
    Ok(BuildResult {
        dst_apk: build_paths.dst_apk,
        java_url,
    })
}

pub fn run(
    sdk_dir: &Path,
    host_os: HostOs,
    package_name: Option<String>,
    app_label: Option<String>,
    args: &[String],
    targets: &[AndroidTarget],
    android_variant: &AndroidVariant,
    config: &AndroidConfig,
    urls: &AndroidSDKUrls,
    devices: Vec<String>,
) -> Result<(), String> {
    let result = build(
        sdk_dir,
        host_os,
        package_name,
        app_label,
        args,
        targets,
        android_variant,
        config,
        urls,
    )?;

    let cwd = std::env::current_dir().unwrap();
    // alright so how will we do multiple targets eh

    fn android_start_args(java_url: &str) -> Vec<String> {
        let mut args = vec![
            "shell".to_string(),
            "am".to_string(),
            "start".to_string(),
            "-S".to_string(),
            "-n".to_string(),
            format!("{0}/{0}.MakepadApp", java_url),
        ];
        if let Ok(studio) = std::env::var("STUDIO") {
            if !studio.trim().is_empty() {
                println!("Android launch intent makepad.STUDIO={}", studio);
                args.push("--es".to_string());
                args.push("makepad.STUDIO".to_string());
                args.push(studio);
            }
        } else {
            println!("Android launch intent makepad.STUDIO is not set");
        }
        args
    }

    if devices.len() == 0 {
        println!("Uploading android application");
        shell_env_cap(
            &[],
            &cwd,
            sdk_dir.join("platform-tools/adb").to_str().unwrap(),
            &["install", "-r", (result.dst_apk.to_str().unwrap())],
        )?;
        println!("Starting android application");
        let start_args = android_start_args(&result.java_url);
        let start_args_refs = start_args
            .iter()
            .map(|arg| arg.as_str())
            .collect::<Vec<_>>();
        shell_env_cap(
            &[],
            &cwd,
            sdk_dir.join("platform-tools/adb").to_str().unwrap(),
            &start_args_refs,
        )?;
        #[allow(unused_assignments)]
        let mut pid = None;
        loop {
            if let Ok(thing) = shell_env_cap(
                &[],
                &cwd,
                sdk_dir.join("platform-tools/adb").to_str().unwrap(),
                &["shell", "pidof", &result.java_url],
            ) {
                pid = Some(thing.trim().to_string());
                break;
            }
        }
        shell_env(
            &[],
            &cwd,
            sdk_dir.join("platform-tools/adb").to_str().unwrap(),
            &["logcat", "--pid", &pid.unwrap(), "Makepad:D *:S"],
        )?;
    } else {
        let mut children = Vec::new();
        println!("Uploading android application");
        for device in &devices {
            children.push(shell_child_create(
                &[],
                &cwd,
                sdk_dir.join("platform-tools/adb").to_str().unwrap(),
                &[
                    "-s",
                    &device,
                    "install",
                    "-r",
                    (result.dst_apk.to_str().unwrap()),
                ],
            )?);
        }
        for child in children {
            shell_child_wait(child)?;
        }
        let mut children = Vec::new();
        println!("Starting android application");
        for device in &devices {
            let start_args = android_start_args(&result.java_url);
            let mut device_args = vec!["-s".to_string(), device.clone()];
            device_args.extend(start_args);
            let device_args_refs = device_args
                .iter()
                .map(|arg| arg.as_str())
                .collect::<Vec<_>>();
            children.push(shell_child_create(
                &[],
                &cwd,
                sdk_dir.join("platform-tools/adb").to_str().unwrap(),
                &device_args_refs,
            )?);
        }
        for child in children {
            shell_child_wait(child)?;
        }
    }
    Ok(())
}

pub fn adb(sdk_dir: &Path, _host_os: HostOs, args: &[String]) -> Result<(), String> {
    let mut args_out = Vec::new();
    for arg in args {
        args_out.push(arg.as_ref());
    }
    let cwd = std::env::current_dir().unwrap();
    shell_env(
        &[],
        &cwd,
        sdk_dir.join("platform-tools/adb").to_str().unwrap(),
        &args_out,
    )?;
    Ok(())
}

fn adb_path(sdk_dir: &Path) -> PathBuf {
    sdk_dir.join("platform-tools/adb")
}

fn push_serial_args<'a>(serial: Option<&'a str>, args: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::with_capacity(args.len() + 2);
    if let Some(serial) = serial {
        out.push("-s");
        out.push(serial);
    }
    out.extend_from_slice(args);
    out
}

fn adb_cap(sdk_dir: &Path, serial: Option<&str>, args: &[&str]) -> Result<String, String> {
    let cwd = std::env::current_dir().unwrap();
    let args_out = push_serial_args(serial, args);
    shell_env_cap(&[], &cwd, adb_path(sdk_dir).to_str().unwrap(), &args_out)
}

fn adb_run(sdk_dir: &Path, serial: Option<&str>, args: &[&str]) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let args_out = push_serial_args(serial, args);
    shell_env(&[], &cwd, adb_path(sdk_dir).to_str().unwrap(), &args_out)
}

fn parse_adb_devices(output: &str) -> Vec<String> {
    let mut devices = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("List of devices attached") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else {
            continue;
        };
        let Some(state) = parts.next() else {
            continue;
        };
        if state == "device" {
            devices.push(serial.to_string());
        }
    }
    devices
}

pub fn list_connected_devices(sdk_dir: &Path) -> Result<Vec<String>, String> {
    let output = adb_cap(sdk_dir, None, &["devices"])?;
    Ok(parse_adb_devices(&output))
}

fn parse_ipv4_token(token: &str) -> Option<&str> {
    let candidate = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
    let mut parts = candidate.split('.');
    let mut count = 0usize;
    while let Some(part) = parts.next() {
        if part.is_empty() || part.len() > 3 {
            return None;
        }
        if part.parse::<u8>().is_err() {
            return None;
        }
        count += 1;
    }
    if count == 4 && candidate != "0.0.0.0" && candidate != "127.0.0.1" {
        Some(candidate)
    } else {
        None
    }
}

fn parse_ip_addr_show(output: &str) -> Option<String> {
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        while let Some(part) = parts.next() {
            if part == "inet" {
                if let Some(addr) = parts.next() {
                    if let Some(ip) = parse_ipv4_token(addr.split('/').next().unwrap_or(addr)) {
                        return Some(ip.to_string());
                    }
                }
            }
        }
    }
    None
}

fn parse_ip_route(output: &str) -> Option<String> {
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        while let Some(part) = parts.next() {
            if part == "src" {
                if let Some(ip) = parts.next().and_then(parse_ipv4_token) {
                    return Some(ip.to_string());
                }
            }
        }
    }
    None
}

fn detect_device_ip(sdk_dir: &Path, serial: Option<&str>) -> Result<String, String> {
    let addr_show = adb_cap(
        sdk_dir,
        serial,
        &[
            "shell", "ip", "-f", "inet", "addr", "show", "scope", "global",
        ],
    )?;
    if let Some(ip) = parse_ip_addr_show(&addr_show) {
        return Ok(ip);
    }

    let route = adb_cap(sdk_dir, serial, &["shell", "ip", "route"])?;
    if let Some(ip) = parse_ip_route(&route) {
        return Ok(ip);
    }

    Err(format!(
        "Could not determine device IP address over adb. `ip -f inet addr show scope global` output:\n{}\n`ip route` output:\n{}",
        addr_show.trim(),
        route.trim()
    ))
}

pub fn adb_tcp(
    sdk_dir: &Path,
    _host_os: HostOs,
    devices: &[String],
    args: &[String],
) -> Result<(), String> {
    let port = match args {
        [] => 5555u16,
        [port] => port
            .parse::<u16>()
            .map_err(|_| format!("Invalid adb-tcp port `{port}`"))?,
        _ => {
            return Err(
                "adb-tcp accepts at most one optional argument: the tcp port (default 5555)"
                    .to_string(),
            )
        }
    };
    let port_string = port.to_string();

    if devices.is_empty() {
        let ip = detect_device_ip(sdk_dir, None)?;
        println!("Detected device IP: {ip}");
        adb_run(sdk_dir, None, &["tcpip", &port_string])?;
        thread::sleep(Duration::from_secs(1));
        let output = adb_cap(sdk_dir, None, &["connect", &format!("{ip}:{port}")])?;
        print!("{output}");
        return Ok(());
    }

    for device in devices {
        let ip = detect_device_ip(sdk_dir, Some(device))?;
        println!("Detected device IP for {device}: {ip}");
        adb_run(sdk_dir, Some(device), &["tcpip", &port_string])?;
        thread::sleep(Duration::from_secs(1));
        let output = adb_cap(sdk_dir, None, &["connect", &format!("{ip}:{port}")])?;
        print!("{output}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_adb_devices, parse_ip_addr_show, parse_ip_route};

    #[test]
    fn parse_adb_devices_filters_ready_targets() {
        let output = "\
List of devices attached\n\
emulator-5554          device product:sdk_gphone64 model:sdk_gphone64 device:emu64 transport_id:1\n\
quest-offline          offline transport_id:2\n\
quest-unauthorized     unauthorized usb:3\n\
10.0.0.151:5555        device product:eureka model:Quest_3 device:eureka transport_id:4\n\
\n";
        assert_eq!(
            parse_adb_devices(output),
            vec!["emulator-5554".to_string(), "10.0.0.151:5555".to_string()]
        );
    }

    #[test]
    fn parse_ip_addr_show_extracts_ipv4() {
        let output = "\
2: wlan0    inet 192.168.0.42/24 brd 192.168.0.255 scope global wlan0\n\
   valid_lft forever preferred_lft forever\n";
        assert_eq!(parse_ip_addr_show(output), Some("192.168.0.42".to_string()));
    }

    #[test]
    fn parse_ip_route_prefers_src_ipv4() {
        let output = "\
default via 192.168.0.1 dev wlan0 proto dhcp src 192.168.0.42 metric 303\n\
192.168.0.0/24 dev wlan0 proto kernel scope link src 192.168.0.42\n";
        assert_eq!(parse_ip_route(output), Some("192.168.0.42".to_string()));
    }
}

pub fn java(sdk_dir: &Path, _host_os: HostOs, args: &[String]) -> Result<(), String> {
    let mut args_out = Vec::new();
    for arg in args {
        args_out.push(arg.as_ref());
    }
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");
    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &args_out,
    )?;
    Ok(())
}

pub fn javac(sdk_dir: &Path, _host_os: HostOs, args: &[String]) -> Result<(), String> {
    let mut args_out = Vec::new();
    for arg in args {
        args_out.push(arg.as_ref());
    }
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");
    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/javac").to_str().unwrap(),
        &args_out,
    )?;
    Ok(())
}

fn to_snakecase(label: &str) -> String {
    let mut snakecase = String::new();
    let mut previous_was_underscore = false;

    for c in label.chars() {
        if c.is_whitespace() {
            previous_was_underscore = true;
        } else if c.is_uppercase() {
            if !previous_was_underscore && !snakecase.is_empty() {
                snakecase.push('_');
            }
            snakecase.extend(c.to_lowercase());
            previous_was_underscore = false;
        } else {
            snakecase.push(c);
            previous_was_underscore = false;
        }
    }
    snakecase
}
