use super::sdk::{AndroidSDKUrls, BUILD_TOOLS_DIR, BUNDLETOOL_JAR_REL, PLATFORMS_DIR};
use crate::android::{AndroidConfig, AndroidTarget, AndroidVariant, HostOs, ManifestArgs};
use crate::makepad_shell::*;
use crate::utils::*;
use makepad_zip_file::*;
use std::{
    collections::{hash_map::DefaultHasher, HashSet},
    fs,
    fs::File,
    hash::{Hash, Hasher},
    io::Write,
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

fn aapt2_path(sdk_dir: &Path, urls: &AndroidSDKUrls) -> PathBuf {
    sdk_dir
        .join(BUILD_TOOLS_DIR)
        .join(urls.build_tools_version)
        .join("aapt2")
}

fn bundletool_jar_path(sdk_dir: &Path) -> PathBuf {
    sdk_dir.join(BUNDLETOOL_JAR_REL)
}

fn keytool_path(sdk_dir: &Path) -> PathBuf {
    let bundled = sdk_dir.join("openjdk/bin/keytool");
    if bundled.is_file() {
        return bundled;
    }
    let bundled_exe = sdk_dir.join("openjdk/bin/keytool.exe");
    if bundled_exe.is_file() {
        return bundled_exe;
    }
    // Fall back to bundled path even if missing — caller will surface the error
    // when keytool is invoked. We don't search PATH here because creating a
    // keystore is a one-time setup; a clear error pointing at install-toolchain
    // is better than silently using whatever the user has.
    bundled
}

/// Path to the `<keystore>.makepad` metadata file written next to a keystore by
/// `keystore-create`. Plain `key = value` text, intentionally password-free.
pub fn keystore_sidecar_path(keystore: &Path) -> PathBuf {
    let mut name = keystore
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".makepad");
    keystore.with_file_name(name)
}

#[derive(Debug, Default)]
pub struct KeystoreSidecar {
    pub alias: Option<String>,
    pub store_type: Option<String>,
}

pub fn read_keystore_sidecar(keystore: &Path) -> Option<KeystoreSidecar> {
    let path = keystore_sidecar_path(keystore);
    let text = fs::read_to_string(&path).ok()?;
    let mut sc = KeystoreSidecar::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim().trim_matches('"').to_string();
        match key {
            "alias" => sc.alias = Some(val),
            "store_type" => sc.store_type = Some(val),
            _ => {}
        }
    }
    Some(sc)
}

fn write_keystore_sidecar(keystore: &Path, sc: &KeystoreSidecar) -> Result<(), String> {
    let path = keystore_sidecar_path(keystore);
    let mut body = String::from(
        "# Keystore metadata written by `cargo makepad android keystore-create`.\n\
         # Safe to commit: contains no passwords. `cargo makepad android build-aab` reads this\n\
         # next to the keystore so you only need to pass --keystore + password on each build.\n",
    );
    if let Some(alias) = &sc.alias {
        body.push_str(&format!("alias = {alias}\n"));
    }
    if let Some(store_type) = &sc.store_type {
        body.push_str(&format!("store_type = {store_type}\n"));
    }
    fs::write(&path, body).map_err(|e| format!("Cant write {:?}: {e}", path))
}

pub struct KeystoreCreateOpts {
    pub keystore_path: PathBuf,
    pub alias: String,
    pub validity_days: u32,
    pub key_size: u32,
    pub key_alg: String,
    pub store_type: String,
    pub dname: Option<String>,
}

pub fn keystore_create(sdk_dir: &Path, opts: &KeystoreCreateOpts) -> Result<(), String> {
    if opts.keystore_path.exists() {
        return Err(format!(
            "Refusing to overwrite existing file {:?}. Pick a new path or delete the existing keystore first.",
            opts.keystore_path
        ));
    }
    let keytool = keytool_path(sdk_dir);
    if !keytool.is_file() {
        return Err(format!(
            "keytool not found at {:?}. Run `cargo makepad android install-toolchain` to download it.",
            keytool
        ));
    }
    if let Some(parent) = opts.keystore_path.parent() {
        if !parent.as_os_str().is_empty() {
            mkdir(parent)?;
        }
    }

    println!("================================================================================");
    println!("CREATING ANDROID UPLOAD KEYSTORE");
    println!("================================================================================");
    println!();
    println!("  This keystore is your *upload key* for the Google Play Store.");
    println!();
    println!("  CRITICAL — you MUST keep this file and its password forever:");
    println!("    * If you lose either, you cannot push updates to your Play Store listing");
    println!("      without going through Google's manual key-recovery process.");
    println!("    * Back up the keystore file AND the password somewhere safe");
    println!("      (password manager, offline copy, secret manager).");
    println!("    * Do not commit the keystore to a public git repository.");
    println!();
    println!("  keytool will now prompt you for:");
    println!("    1. A keystore password (you'll need this on every build).");
    println!("    2. Your name / org / city / country (goes into the certificate; permanent).");
    println!("    3. A key password — press Enter to reuse the keystore password (recommended).");
    println!();
    println!("================================================================================");
    println!();

    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();

    let validity = opts.validity_days.to_string();
    let keysize = opts.key_size.to_string();
    let keystore_str = opts.keystore_path.to_str().unwrap().to_string();

    let mut args: Vec<String> = vec![
        "-genkeypair".to_string(),
        "-v".to_string(),
        "-keystore".to_string(),
        keystore_str,
        "-alias".to_string(),
        opts.alias.clone(),
        "-keyalg".to_string(),
        opts.key_alg.clone(),
        "-keysize".to_string(),
        keysize,
        "-validity".to_string(),
        validity,
        "-storetype".to_string(),
        opts.store_type.clone(),
    ];
    if let Some(dname) = &opts.dname {
        args.push("-dname".to_string());
        args.push(dname.clone());
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    // Use shell_env (interactive, not _cap) so keytool's prompts go straight
    // to the terminal and the user can type passwords without echo.
    shell_env(
        &[("JAVA_HOME", java_home.to_str().unwrap())],
        &cwd,
        keytool.to_str().unwrap(),
        &arg_refs,
    )?;

    write_keystore_sidecar(
        &opts.keystore_path,
        &KeystoreSidecar {
            alias: Some(opts.alias.clone()),
            store_type: Some(opts.store_type.clone()),
        },
    )?;

    println!();
    println!("================================================================================");
    println!("KEYSTORE CREATED SUCCESSFULLY");
    println!("================================================================================");
    println!();
    println!("  Keystore: {}", opts.keystore_path.display());
    println!(
        "  Metadata: {}  (alias auto-discovered by build-aab; safe to commit)",
        keystore_sidecar_path(&opts.keystore_path).display()
    );
    println!();
    println!("  BACK UP THE KEYSTORE FILE AND ITS PASSWORD NOW. Suggested locations:");
    println!("    * 1Password / Bitwarden / your team's password manager");
    println!("    * An offline encrypted backup (USB drive in a safe)");
    println!("    * Your CI's secret store (GitHub Actions secrets, etc.) for automation");
    println!();
    println!("  Build a signed AAB with:");
    println!(
        "    cargo makepad android --keystore={} \\",
        opts.keystore_path.display()
    );
    println!("        --keystore-pass=<password> \\");
    println!("        build-aab -p <your-app> --release");
    println!();
    println!("  Or set MAKEPAD_KEYSTORE_PASS in your environment to skip the password flag:");
    println!("    export MAKEPAD_KEYSTORE_PASS=<password>");
    println!(
        "    cargo makepad android --keystore={} build-aab -p <your-app> --release",
        opts.keystore_path.display()
    );
    println!();
    println!("================================================================================");

    Ok(())
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
    prefer_dynamic: bool,
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
    // Derive ndk_root from ndk_prebuilt_root by going up through
    // `toolchains/llvm/prebuilt/<host>/` (4 levels).
    let ndk_root = ndk_prebuilt_root
        .parent()
        .unwrap() // prebuilt/
        .parent()
        .unwrap() // llvm/
        .parent()
        .unwrap() // toolchains/
        .parent()
        .unwrap() // ndk root
        .to_path_buf();
    for android_target in android_targets {
        let clang_filename = format!("{}{}-clang", android_target.clang(), urls.sdk_version);
        let clangpp_filename = format!("{}{}-clang++", android_target.clang(), urls.sdk_version);

        let bin_name = |bin_filename: &str, windows_extension: &str| match host_os {
            HostOs::WindowsX64 => format!("{bin_filename}.{windows_extension}"),
            HostOs::MacosX64 | HostOs::MacosAarch64 | HostOs::LinuxX64 => bin_filename.to_string(),
            _ => panic!(),
        };
        let full_clang_path = ndk_prebuilt_root
            .join("bin")
            .join(bin_name(&clang_filename, "cmd"));
        let full_clangpp_path = ndk_prebuilt_root
            .join("bin")
            .join(bin_name(&clangpp_filename, "cmd"));
        let full_llvm_ar_path = ndk_prebuilt_root
            .join("bin")
            .join(bin_name("llvm-ar", "exe"));
        let full_llvm_ranlib_path = ndk_prebuilt_root
            .join("bin")
            .join(bin_name("llvm-ranlib", "exe"));

        let toolchain = android_target.toolchain();
        let target_opt = format!("--target={toolchain}");
        let target_dir_arg = format!("--target-dir={target_dir_str}");

        // Android builds on stable: all android targets are tier-2 and the
        // build uses no nightly-only cargo/rustc features. `rustup run stable`
        // pins the channel explicitly so it doesn't matter what the build
        // crate's rust-toolchain.toml (if any) selects.
        let base_args = &[
            "run",
            "stable",
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
        let rustflags = compose_android_rustflags(
            std::env::var("RUSTFLAGS").ok().as_deref(),
            &cfg_flag,
            prefer_dynamic,
        );

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
                "ANDROID_NDK_ROOT".to_string(),
                ndk_root.to_string_lossy().to_string(),
            ),
            (
                format!("CC_{toolchain}"),
                full_clang_path.to_string_lossy().to_string(),
            ),
            (
                format!("CXX_{toolchain}"),
                full_clangpp_path.to_string_lossy().to_string(),
            ),
            (
                format!("AR_{toolchain}"),
                full_llvm_ar_path.to_string_lossy().to_string(),
            ),
            (
                format!("RANLIB_{toolchain}"),
                full_llvm_ranlib_path.to_string_lossy().to_string(),
            ),
            // The aws-lc-sys crate requires the cmake builder (not the cc builder)
            // for Android cross-compilation targets. With ANDROID_NDK_ROOT set
            // and the full NDK installed (via --full-ndk), cmake's built-in
            // Android module handles the cross-compilation setup automatically.
            ("AWS_LC_SYS_CMAKE_BUILDER".to_string(), "1".to_string()),
            ("RUSTFLAGS".to_string(), rustflags),
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

/// Builds the `RUSTFLAGS` value for an Android `cargo rustc` invocation.
///
/// `prefer_dynamic`: APK/dev builds pass `true` (dynamically linked `std`
/// keeps incremental relinks fast). AAB builds pass `false` — `prefer-dynamic`
/// makes Rust ship `std` as a separate `libstd-<hash>.so`, and the toolchain's
/// prebuilt copy of that library is only 4 KB-page aligned, which fails Google
/// Play's 16 KB page-size requirement for apps targeting Android 15+. Static
/// `std` keeps the app a single self-contained, NDK-linked (16 KB-aligned) `.so`.
fn compose_android_rustflags(existing: Option<&str>, cfg_flag: &str, prefer_dynamic: bool) -> String {
    let mut rustflags = existing.unwrap_or_default().trim().to_string();
    if prefer_dynamic {
        let has_prefer_dynamic = rustflags
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair == ["-C", "prefer-dynamic"])
            || rustflags
                .split_whitespace()
                .any(|token| token == "-Cprefer-dynamic");

        if !has_prefer_dynamic {
            if !rustflags.is_empty() {
                rustflags.push(' ');
            }
            rustflags.push_str("-C prefer-dynamic");
        }
    }
    if !cfg_flag.trim().is_empty() {
        if !rustflags.is_empty() {
            rustflags.push(' ');
        }
        rustflags.push_str(cfg_flag.trim());
    }
    rustflags
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

/// Final, resolved values for the manifest + APK/AAB packaging inputs.
/// Produced by `resolve_packaging_inputs` from CLI flags + Cargo.toml metadata
/// + sensible defaults, then consumed by both `build` and `build_aab`.
struct ResolvedPackagingInputs {
    /// Android package id (`com.foo.bar`). Becomes both the manifest `package=`
    /// attribute and the Java module path.
    java_url: String,
    /// Launcher label (`<application android:label="...">`).
    app_label: String,
    /// `android:versionCode` — monotonic integer, required by Play Store.
    version_code: u32,
    /// `android:versionName` — human-readable, e.g. "1.0.0".
    version_name: String,
    /// Effective NDK target / `android:minSdkVersion` for this build. None
    /// means use the cargo-makepad default from `urls.sdk_version`. Validated
    /// to be ≥ 21 (NDK floor) and ≤ `urls.target_sdk_version` (Android rule).
    min_sdk_version_override: Option<usize>,
}

/// Resolve packaging inputs with priority: CLI flag > Cargo.toml metadata > built-in default.
///
/// Cargo.toml fields read:
///   `[package].version`                                  -> default `version_name`
///   `[package.metadata.packager].identifier`             -> default `package_name`
///   `[package.metadata.packager].product_name`           -> default `app_label`
///   `[package.metadata.makepad.android].version_code`    -> default `version_code`
///   `[package.metadata.makepad.android].version_name`    -> overrides `[package].version`
fn resolve_packaging_inputs(
    build_crate: &str,
    binary_name: &str,
    package_name_flag: Option<String>,
    app_label_flag: Option<String>,
    version_code_flag: Option<VersionCodeStrategy>,
    version_name_flag: Option<String>,
    min_sdk_version_flag: Option<usize>,
    urls: &AndroidSDKUrls,
) -> Result<ResolvedPackagingInputs, String> {
    let underscore_binary = binary_name.replace('-', "_");
    let metadata = read_android_package_metadata(build_crate);

    let java_url = package_name_flag
        .or(metadata.identifier.clone())
        .unwrap_or_else(|| format!("dev.makepad.{underscore_binary}"));

    let app_label = app_label_flag
        .or(metadata.product_name.clone())
        .unwrap_or_else(|| {
            let mut chars = underscore_binary.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        });

    let version_code_strategy = version_code_flag
        .or(metadata.version_code)
        .unwrap_or(VersionCodeStrategy::Explicit(1));
    let version_code = version_code_strategy.resolve();

    let version_name = version_name_flag
        .or(metadata.version_name_override.clone())
        .or(metadata.package_version.clone())
        .unwrap_or_else(|| "1.0".to_string());

    // Resolve and validate the per-app min SDK override. Default = unset,
    // meaning the build uses `urls.sdk_version` as before. When set, must be:
    //   - >= 21 (the NDK r28 floor for arm64; lower targets fail to build)
    //   - <= `urls.target_sdk_version` (Android rule: minSdk <= targetSdk)
    //   - >= `urls.sdk_version` (going *below* the cargo-makepad floor would
    //     require knowing for sure that all NDK extern fns Makepad uses still
    //     link cleanly at that lower API; we don't audit that automatically)
    let min_sdk_version_override = min_sdk_version_flag.or(metadata.min_sdk_version);
    if let Some(min) = min_sdk_version_override {
        if min < 21 {
            return Err(format!(
                "min_sdk_version = {min} is below the NDK r28 arm64 floor of 21"
            ));
        }
        if min > urls.target_sdk_version {
            return Err(format!(
                "min_sdk_version = {min} cannot exceed targetSdkVersion = {} (Android rule: minSdk <= targetSdk)",
                urls.target_sdk_version
            ));
        }
        if min < urls.sdk_version {
            return Err(format!(
                "min_sdk_version = {min} is below cargo-makepad's audited floor of {}. Lowering further requires verifying all NDK extern fns still link at API {min}; if you've done that audit, raise cargo-makepad's `urls.sdk_version` instead.",
                urls.sdk_version
            ));
        }
    }

    Ok(ResolvedPackagingInputs {
        java_url,
        app_label,
        version_code,
        version_name,
        min_sdk_version_override,
    })
}

/// Inputs to `prepare_build`. Covers everything that influences the staged
/// `tmp_dir/AndroidManifest.xml` so the file can vary per build mode (release
/// APK vs debug APK vs Play Store AAB).
pub struct PrepareBuildOpts<'a> {
    pub build_crate: &'a str,
    pub java_url: &'a str,
    pub app_label: &'a str,
    pub variant: &'a AndroidVariant,
    pub urls: &'a AndroidSDKUrls,
    pub version_code: u32,
    pub version_name: &'a str,
    pub debuggable: bool,
}

/// Replace `{key}` placeholders in a custom AndroidManifest template with the
/// values cargo-makepad always knows. Used when the app crate ships its own
/// `resources/android/AndroidManifest.xml.template`.
fn substitute_manifest_template(template: &str, args: &ManifestArgs<'_>) -> String {
    template
        .replace("{label}", args.label)
        .replace("{class_name}", args.class_name)
        .replace("{package_id}", args.url)
        .replace("{url}", args.url)
        .replace("{min_sdk_version}", &args.sdk_version.to_string())
        // `{sdk_version}` historically meant "the one SDK number" — alias to min
        // so older templates keep compiling. New templates should prefer
        // `{min_sdk_version}` / `{target_sdk_version}`.
        .replace("{sdk_version}", &args.sdk_version.to_string())
        .replace("{target_sdk_version}", &args.target_sdk_version.to_string())
        .replace("{version_code}", &args.version_code.to_string())
        .replace("{version_name}", args.version_name)
        .replace(
            "{debuggable}",
            if args.debuggable { "true" } else { "false" },
        )
}

fn prepare_build(opts: &PrepareBuildOpts<'_>) -> Result<BuildPaths, String> {
    let cwd = std::env::current_dir().unwrap();
    let target_dir = cargo_target_dir(&cwd);
    let underscore_build_crate = opts.build_crate.replace('-', "_");

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

    let build_crate_dir = get_crate_dir(opts.build_crate)?;
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

    let manifest_args = ManifestArgs {
        label: opts.app_label,
        class_name: "MakepadApp",
        url: opts.java_url,
        sdk_version: opts.urls.sdk_version,
        target_sdk_version: opts.urls.target_sdk_version,
        has_icon: has_android_icon,
        version_code: opts.version_code,
        version_name: opts.version_name,
        debuggable: opts.debuggable,
    };

    // Custom manifest override: if `<crate>/resources/android/AndroidManifest.xml.template`
    // exists, use it after substituting `{key}` placeholders. Useful for declaring a
    // permissions/features set tailored to the app (Play Store rejects most of the
    // default kitchen-sink permission list without justification).
    let custom_template = build_crate_dir.join("resources/android/AndroidManifest.xml.template");
    let manifest_xml = if custom_template.is_file() {
        let template = fs::read_to_string(&custom_template)
            .map_err(|e| format!("Cant read custom manifest {:?}: {e}", custom_template))?;
        println!(
            "Using custom AndroidManifest template: {}",
            custom_template.display()
        );
        substitute_manifest_template(&template, &manifest_args)
    } else {
        opts.variant.manifest_xml(&manifest_args)
    };
    let manifest_file = tmp_dir.join("AndroidManifest.xml");
    write_text(&manifest_file, &manifest_xml)?;

    let main_java = main_java(opts.java_url);
    let java_path = opts.java_url.replace('.', "/");
    let java_file = tmp_dir.join(&java_path).join("MakepadApp.java");
    write_text(&java_file, &main_java)?;

    let xr_java = xr_java(opts.java_url);
    let xr_file = tmp_dir.join(&java_path).join("MakepadAppXr.java");
    write_text(&xr_file, &xr_java)?;

    let apk_filename = to_snakecase(opts.app_label);
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

fn read_needed_shared_libs(
    sdk_dir: &Path,
    host_os: HostOs,
    urls: &AndroidSDKUrls,
    so_path: &Path,
) -> Result<Vec<String>, String> {
    let (_ndk_version, ndk_prebuilt_root) =
        resolve_ndk_prebuilt_root(sdk_dir, host_os, urls.ndk_version_full)?;

    let readelf_path = ndk_prebuilt_root.join("bin/llvm-readelf");
    if !readelf_path.exists() {
        return Ok(Vec::new());
    }

    let cwd = std::env::current_dir().unwrap();
    let output = shell_env_cap(
        &[],
        &cwd,
        readelf_path.to_str().unwrap(),
        &["-d", so_path.to_str().unwrap()],
    )?;

    let mut libs = Vec::new();
    for line in output.lines() {
        if !line.contains("(NEEDED)") {
            continue;
        }
        let Some(lib_name) = line.find('[').and_then(|start| {
            line[start + 1..]
                .find(']')
                .map(|end| &line[start + 1..start + 1 + end])
        }) else {
            continue;
        };
        libs.push(lib_name.to_string());
    }
    Ok(libs)
}

fn bundle_local_shared_deps(
    sdk_dir: &Path,
    host_os: HostOs,
    urls: &AndroidSDKUrls,
    android_target: &AndroidTarget,
    so_path: &Path,
    abi: &str,
    build_paths: &BuildPaths,
    build_dir: &Path,
) -> Result<(), String> {
    let mut pending = vec![so_path.to_path_buf()];
    let mut visited = HashSet::<String>::new();
    let search_dirs = [build_dir.to_path_buf(), build_dir.join("deps")];

    while let Some(current_so) = pending.pop() {
        for lib_name in read_needed_shared_libs(sdk_dir, host_os, urls, &current_so)? {
            if !visited.insert(lib_name.clone()) {
                continue;
            }

            let binary_path = format!("lib/{abi}/{lib_name}");
            let dst_lib = build_paths.out_dir.join(&binary_path);
            if dst_lib.exists() {
                continue;
            }

            let candidate = search_dirs
                .iter()
                .map(|dir| dir.join(&lib_name))
                .find(|path| path.is_file())
                .or_else(|| find_rustup_shared_lib(android_target, &lib_name));
            let Some(candidate) = candidate else {
                continue;
            };

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

            // A local Rust dylib can still depend on NDK-provided shared libs.
            bundle_ndk_shared_deps(
                sdk_dir,
                host_os,
                urls,
                android_target,
                &dst_lib,
                abi,
                build_paths,
            )?;

            println!("  Bundled local shared dep: {lib_name} (for {abi})");
            pending.push(candidate);
        }
    }

    Ok(())
}

fn find_rustup_shared_lib(android_target: &AndroidTarget, lib_name: &str) -> Option<PathBuf> {
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")))?;
    let toolchains_dir = rustup_home.join("toolchains");
    let tail = Path::new("lib")
        .join("rustlib")
        .join(android_target.toolchain())
        .join("lib")
        .join(lib_name);

    for entry in fs::read_dir(toolchains_dir).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let candidate = entry.path().join(&tail);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
        let current_build_dir = target_dir.join(format!("{android_target_dir}/{profile}"));
        build_dir = Some(current_build_dir.clone());
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
        bundle_local_shared_deps(
            sdk_dir,
            host_os,
            urls,
            android_target,
            &src_lib,
            abi,
            build_paths,
            &current_build_dir,
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
        // Skip files that already ship from the sibling `resources/` dir —
        // otherwise the same TTF lands in the APK twice. The widgets crate
        // for instance keeps LXGWWenKai*.ttf and NotoColorEmoji.ttf in both.
        if resource_dir.join(path).is_file() {
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

// ============================================================================
// AAB (Android App Bundle) build pipeline.
//
// Unlike the legacy APK pipeline which uses aapt v1 + apksigner, AABs require:
//   1. aapt2 compile + aapt2 link --proto-format  -> proto-format base APK
//   2. Repackage into the bundletool "module zip" layout
//      (manifest/, dex/, res/, lib/, assets/, resources.pb)
//   3. bundletool build-bundle -> .aab
//   4. jarsigner               -> signed .aab
//
// The Play Store rejects debug-signed AABs, so the keystore is configurable.
// ============================================================================

/// Where to place the AAB-specific intermediate and output files.
struct AabPaths {
    staged_assets_dir: PathBuf,
    staged_libs_dir: PathBuf,
    compiled_res_zip: PathBuf,
    proto_apk: PathBuf,
    base_module_dir: PathBuf,
    base_module_zip: PathBuf,
    dst_aab: PathBuf,
}

fn prepare_aab_paths(build_crate: &str, app_label: &str) -> Result<AabPaths, String> {
    let cwd = std::env::current_dir().unwrap();
    let target_dir = cargo_target_dir(&cwd);
    let underscore_build_crate = build_crate.replace('-', "_");
    let aab_dir = target_dir
        .join("makepad-android-aab")
        .join(&underscore_build_crate);
    let _ = rmdir(&aab_dir);
    mkdir(&aab_dir)?;

    let staged_assets_dir = aab_dir.join("assets");
    let staged_libs_dir = aab_dir.join("lib");
    let compiled_res_zip = aab_dir.join("compiled_res.zip");
    let proto_apk = aab_dir.join("base_proto.apk");
    let base_module_dir = aab_dir.join("base");
    let base_module_zip = aab_dir.join("base.zip");
    let dst_aab = aab_dir.join(format!("{}.aab", to_snakecase(app_label)));

    mkdir(&staged_assets_dir)?;
    mkdir(&staged_libs_dir)?;

    Ok(AabPaths {
        staged_assets_dir,
        staged_libs_dir,
        compiled_res_zip,
        proto_apk,
        base_module_dir,
        base_module_zip,
        dst_aab,
    })
}

/// Mirror of `add_assets_dir_to_apk` that only stages files into a directory tree
/// (no aapt invocation). Output structure: `<assets_root>/makepad/<crate>/<asset_subdir>/...`.
fn stage_makepad_assets_subdir(
    assets_root: &Path,
    crate_name: &str,
    source_dir: &Path,
    asset_subdir: &str,
    config: &AndroidConfig,
) -> Result<(), String> {
    if !source_dir.is_dir() {
        return Ok(());
    }
    let crate_name = crate_name.replace('-', "_");
    let dst_dir = assets_root.join(format!("makepad/{crate_name}/{asset_subdir}"));
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
    Ok(())
}

/// Mirror of `add_font_assets_dir_to_apk` that only stages files into a directory tree.
fn stage_makepad_font_subdir(
    assets_root: &Path,
    crate_name: &str,
    source_dir: &Path,
    resource_dir: &Path,
    config: &AndroidConfig,
) -> Result<(), String> {
    if !source_dir.is_dir() {
        return Ok(());
    }
    let crate_name = crate_name.replace('-', "_");
    let dst_dir = assets_root.join(format!("makepad/{crate_name}/fonts"));
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
        if resource_dir.join(path).is_file() {
            continue;
        }
        cp(&source_dir.join(path), &dst_dir.join(path), false)?;
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

fn stage_aab_assets(
    build_crate: &str,
    assets_root: &Path,
    build_dir: &Path,
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    config: &AndroidConfig,
) -> Result<(), String> {
    let build_crate_dir = get_crate_dir(build_crate)?;
    stage_makepad_assets_subdir(
        assets_root,
        build_crate,
        &build_crate_dir.join("resources"),
        "resources",
        config,
    )?;
    stage_makepad_font_subdir(
        assets_root,
        build_crate,
        &build_crate_dir.join("fonts"),
        &build_crate_dir.join("resources"),
        config,
    )?;

    let deps = get_crate_dep_dirs(build_crate, build_dir, &android_targets[0].toolchain());
    for (name, dep_dir) in deps.iter() {
        stage_makepad_assets_subdir(
            assets_root,
            name,
            &dep_dir.join("resources"),
            "resources",
            config,
        )?;
        stage_makepad_font_subdir(
            assets_root,
            name,
            &dep_dir.join("fonts"),
            &dep_dir.join("resources"),
            config,
        )?;
    }

    if let AndroidVariant::Quest = variant {
        let dst_dir = assets_root.join("makepad/makepad_widgets/resources");
        let remove = [
            "fa-solid-900.ttf",
            "LiberationMono-Regular.ttf",
            "NotoSans-Regular.ttf",
        ];
        for r in remove {
            let _ = rm(&dst_dir.join(r));
        }
    }

    Ok(())
}

/// Mirror of `bundle_ndk_shared_deps` that only stages files into `libs_root/<abi>/`.
fn stage_ndk_shared_deps_for_so(
    sdk_dir: &Path,
    host_os: HostOs,
    urls: &AndroidSDKUrls,
    android_target: &AndroidTarget,
    so_path: &Path,
    abi: &str,
    libs_root: &Path,
) -> Result<(), String> {
    let (_ndk_version, ndk_prebuilt_root) =
        resolve_ndk_prebuilt_root(sdk_dir, host_os, urls.ndk_version_full)?;
    let readelf_path = ndk_prebuilt_root.join("bin/llvm-readelf");
    if !readelf_path.exists() {
        return Ok(());
    }
    let cwd = std::env::current_dir().unwrap();
    let output = shell_env_cap(
        &[],
        &cwd,
        readelf_path.to_str().unwrap(),
        &["-d", so_path.to_str().unwrap()],
    )?;
    let clang_triple = android_target.clang();
    let sysroot_lib_dir = ndk_prebuilt_root.join("sysroot/usr/lib").join(clang_triple);
    for line in output.lines() {
        if !line.contains("(NEEDED)") {
            continue;
        }
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
            continue;
        }
        let api_level_stub = sysroot_lib_dir
            .join(urls.sdk_version.to_string())
            .join(lib_name);
        if api_level_stub.exists() {
            continue;
        }
        let dst_lib = libs_root.join(abi).join(lib_name);
        if dst_lib.exists() {
            continue;
        }
        cp(&candidate, &dst_lib, false)?;
        println!("  Bundled NDK shared dep: {lib_name} (for {abi})");
    }
    Ok(())
}

/// Mirror of `bundle_local_shared_deps` that only stages into `libs_root/<abi>/`.
fn stage_local_shared_deps(
    sdk_dir: &Path,
    host_os: HostOs,
    urls: &AndroidSDKUrls,
    android_target: &AndroidTarget,
    so_path: &Path,
    abi: &str,
    libs_root: &Path,
    build_dir: &Path,
) -> Result<(), String> {
    let mut pending = vec![so_path.to_path_buf()];
    let mut visited = HashSet::<String>::new();
    let search_dirs = [build_dir.to_path_buf(), build_dir.join("deps")];
    while let Some(current_so) = pending.pop() {
        for lib_name in read_needed_shared_libs(sdk_dir, host_os, urls, &current_so)? {
            if !visited.insert(lib_name.clone()) {
                continue;
            }
            let dst_lib = libs_root.join(abi).join(&lib_name);
            if dst_lib.exists() {
                continue;
            }
            let candidate = search_dirs
                .iter()
                .map(|dir| dir.join(&lib_name))
                .find(|path| path.is_file())
                .or_else(|| find_rustup_shared_lib(android_target, &lib_name));
            let Some(candidate) = candidate else {
                continue;
            };
            cp(&candidate, &dst_lib, false)?;
            stage_ndk_shared_deps_for_so(
                sdk_dir,
                host_os,
                urls,
                android_target,
                &dst_lib,
                abi,
                libs_root,
            )?;
            println!("  Bundled local shared dep: {lib_name} (for {abi})");
            pending.push(candidate);
        }
    }
    Ok(())
}

fn stage_aab_native_libs(
    sdk_dir: &Path,
    host_os: HostOs,
    underscore_target: &str,
    libs_root: &Path,
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
        mkdir(&libs_root.join(abi))?;

        let android_target_dir = android_target.toolchain();
        if profile == "debug" {
            println!("WARNING - compiling a DEBUG build of the application, this creates a very slow and big app. Try adding --release for a fast, or --profile=small for a small build.");
        }
        let src_lib = target_dir.join(format!(
            "{android_target_dir}/{profile}/lib{underscore_target}.so"
        ));
        let current_build_dir = target_dir.join(format!("{android_target_dir}/{profile}"));
        build_dir = Some(current_build_dir.clone());
        let dst_lib = libs_root.join(abi).join("libmakepad.so");
        cp(&src_lib, &dst_lib, false)?;

        stage_ndk_shared_deps_for_so(
            sdk_dir,
            host_os,
            urls,
            android_target,
            &dst_lib,
            abi,
            libs_root,
        )?;
        stage_local_shared_deps(
            sdk_dir,
            host_os,
            urls,
            android_target,
            &src_lib,
            abi,
            libs_root,
            &current_build_dir,
        )?;
    }
    if let AndroidVariant::Quest = variant {
        let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        for (rel_path, src_lib) in [("arm64-v8a/libopenxr_loader.so", "quest/libopenxr_loader.so")]
        {
            let src_lib = cargo_manifest_dir.join(src_lib);
            let dst_lib = libs_root.join(rel_path);
            cp(&src_lib, &dst_lib, false)?;
        }
    }
    Ok(build_dir.unwrap())
}

fn aapt2_compile_resources(
    sdk_dir: &Path,
    res_dir: &Path,
    out_zip: &Path,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    shell_env_cap(
        &[],
        &cwd,
        aapt2_path(sdk_dir, urls).to_str().unwrap(),
        &[
            "compile",
            "--dir",
            res_dir.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ],
    )?;
    Ok(())
}

fn aapt2_link_proto_apk(
    sdk_dir: &Path,
    manifest_xml: &Path,
    compiled_res_zip: &Path,
    assets_dir: &Path,
    out_apk: &Path,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let android_jar = android_jar_path(sdk_dir, urls);
    let android_jar_str = android_jar.to_str().unwrap().to_string();
    let manifest_str = manifest_xml.to_str().unwrap().to_string();
    let out_str = out_apk.to_str().unwrap().to_string();
    let res_zip_str = compiled_res_zip.to_str().unwrap().to_string();
    let assets_str = assets_dir.to_str().unwrap().to_string();

    let has_assets = assets_dir.is_dir() && ls(assets_dir).map(|v| !v.is_empty()).unwrap_or(false);

    let mut args: Vec<&str> = vec![
        "link",
        "--proto-format",
        "--auto-add-overlay",
        "-I",
        &android_jar_str,
        "--manifest",
        &manifest_str,
        "-o",
        &out_str,
    ];
    if has_assets {
        args.push("-A");
        args.push(&assets_str);
    }
    args.push(&res_zip_str);
    shell_env_cap(
        &[],
        &cwd,
        aapt2_path(sdk_dir, urls).to_str().unwrap(),
        &args,
    )?;
    Ok(())
}

/// Repackage a proto-format APK produced by `aapt2 link --proto-format` into
/// the bundle module layout that bundletool expects, then zip it.
///
/// Layout produced under `base_dir`:
///   manifest/AndroidManifest.xml   <- moved from APK root
///   dex/classes.dex                <- copied from d8 output
///   res/...                        <- carried from proto APK as-is
///   resources.pb                   <- carried from proto APK
///   assets/...                     <- carried from proto APK (added via aapt2 -A)
///   lib/<abi>/*.so                 <- copied from staged libs dir
fn assemble_aab_base_module(
    sdk_dir: &Path,
    proto_apk: &Path,
    classes_dex: &Path,
    libs_root: &Path,
    base_dir: &Path,
    base_zip: &Path,
) -> Result<(), String> {
    let _ = rmdir(base_dir);
    mkdir(base_dir)?;

    // Extract proto APK entries into the base dir, rewriting the manifest path.
    let mut zip_file =
        File::open(proto_apk).map_err(|e| format!("Cant open proto APK {:?}: {e}", proto_apk))?;
    let directory = zip_read_central_directory(&mut zip_file)
        .map_err(|e| format!("Cant read proto APK {:?}: {:?}", proto_apk, e))?;

    for header in &directory.file_headers {
        let entry_name = &header.file_name;
        if entry_name.ends_with('/') {
            continue;
        }
        let data = header
            .extract(&mut zip_file)
            .map_err(|e| format!("Failed to extract {entry_name} from proto APK: {:?}", e))?;
        let dst_rel = if entry_name == "AndroidManifest.xml" {
            "manifest/AndroidManifest.xml".to_string()
        } else {
            entry_name.clone()
        };
        let dst_path = base_dir.join(&dst_rel);
        mkdir(dst_path.parent().unwrap())?;
        let mut f =
            File::create(&dst_path).map_err(|e| format!("Cant write {:?}: {e}", dst_path))?;
        f.write_all(&data)
            .map_err(|e| format!("Cant write to {:?}: {e}", dst_path))?;
    }

    // dex/classes.dex
    cp(classes_dex, &base_dir.join("dex/classes.dex"), false)?;

    // lib/<abi>/*.so (only if the staged libs dir actually has anything).
    if libs_root.is_dir() && ls(libs_root).map(|v| !v.is_empty()).unwrap_or(false) {
        cp_all(libs_root, &base_dir.join("lib"), false)?;
    }

    // Zip the base module using the bundled `jar` tool. `M` flag suppresses
    // META-INF/MANIFEST.MF; bundletool rejects modules with an unexpected
    // META-INF entry.
    let java_home = sdk_dir.join("openjdk");
    let jar_bin = java_home.join("bin/jar");
    if base_zip.is_file() {
        rm(base_zip)?;
    }
    shell_env_cap(
        &[("JAVA_HOME", java_home.to_str().unwrap())],
        base_dir,
        jar_bin.to_str().unwrap(),
        &["cMf", base_zip.to_str().unwrap(), "."],
    )?;

    Ok(())
}

fn run_bundletool_build_bundle(
    sdk_dir: &Path,
    base_zip: &Path,
    aab_path: &Path,
) -> Result<(), String> {
    let java_home = sdk_dir.join("openjdk");
    let bundletool = bundletool_jar_path(sdk_dir);
    if !bundletool.is_file() {
        return Err(format!(
            "bundletool jar not found at {:?}. Re-run `cargo makepad android install-toolchain` to download it.",
            bundletool
        ));
    }
    if aab_path.is_file() {
        rm(aab_path)?;
    }
    let cwd = std::env::current_dir().unwrap();
    let modules_arg = format!("--modules={}", base_zip.display());
    let output_arg = format!("--output={}", aab_path.display());
    shell_env_cap(
        &[("JAVA_HOME", java_home.to_str().unwrap())],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &[
            "-jar",
            bundletool.to_str().unwrap(),
            "build-bundle",
            &modules_arg,
            &output_arg,
        ],
    )?;
    Ok(())
}

/// Pick a jarsigner binary from (in order): the bundled JDK, JAVA_HOME, PATH.
fn resolve_jarsigner(sdk_dir: &Path) -> Option<PathBuf> {
    let bundled = sdk_dir.join("openjdk/bin/jarsigner");
    let bundled_exe = sdk_dir.join("openjdk/bin/jarsigner.exe");
    if bundled.is_file() {
        return Some(bundled);
    }
    if bundled_exe.is_file() {
        return Some(bundled_exe);
    }
    if let Some(java_home) = std::env::var_os("JAVA_HOME") {
        let from_home = PathBuf::from(&java_home).join("bin/jarsigner");
        let from_home_exe = PathBuf::from(&java_home).join("bin/jarsigner.exe");
        if from_home.is_file() {
            return Some(from_home);
        }
        if from_home_exe.is_file() {
            return Some(from_home_exe);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in ["jarsigner", "jarsigner.exe"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[derive(Debug)]
pub struct AabSigningOpts {
    pub keystore: PathBuf,
    pub storepass: String,
    pub key_alias: String,
    pub keypass: String,
}

fn sign_aab(sdk_dir: &Path, aab_path: &Path, opts: &AabSigningOpts) -> Result<(), String> {
    let jarsigner = resolve_jarsigner(sdk_dir).ok_or_else(|| {
        "jarsigner not found. Re-run `cargo makepad android install-toolchain` to extract it, or set JAVA_HOME to a full JDK install."
            .to_string()
    })?;
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();
    shell_env_cap(
        &[("JAVA_HOME", java_home.to_str().unwrap())],
        &cwd,
        jarsigner.to_str().unwrap(),
        &[
            "-keystore",
            opts.keystore.to_str().unwrap(),
            "-storepass",
            &opts.storepass,
            "-keypass",
            &opts.keypass,
            aab_path.to_str().unwrap(),
            &opts.key_alias,
        ],
    )?;
    Ok(())
}

pub struct BuildAabResult {
    #[allow(dead_code)]
    pub dst_aab: PathBuf,
}

pub fn build_aab(
    sdk_dir: &Path,
    host_os: HostOs,
    package_name: Option<String>,
    app_label: Option<String>,
    version_code: Option<VersionCodeStrategy>,
    version_name: Option<String>,
    min_sdk_version: Option<usize>,
    args: &[String],
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    config: &AndroidConfig,
    urls: &AndroidSDKUrls,
    signing: Option<AabSigningOpts>,
) -> Result<BuildAabResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    let binary_name =
        get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
    let underscore_build_crate = build_crate.replace('-', "_");

    let resolved = resolve_packaging_inputs(
        build_crate,
        &binary_name,
        package_name,
        app_label,
        version_code,
        version_name,
        min_sdk_version,
        urls,
    )?;
    // Apply the per-app min SDK override by mutating a copy of urls. Downstream
    // code (NDK clang triple, sysroot stub lookup, manifest emission) reads
    // `urls.sdk_version`, so this single point of override propagates cleanly.
    let mut effective_urls = *urls;
    if let Some(min) = resolved.min_sdk_version_override {
        effective_urls.sdk_version = min;
    }
    let urls = &effective_urls;

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
        // AAB: statically link `std` so the bundle has no separate libstd.so
        // (the toolchain's prebuilt one is only 4 KB-page aligned, which fails
        // Play's 16 KB page-size requirement).
        false,
    )?;
    // AABs uploaded to Play Store must have `android:debuggable="false"`. Force
    // it here regardless of cargo profile so we never produce an unshippable AAB.
    let prep_opts = PrepareBuildOpts {
        build_crate,
        java_url: &resolved.java_url,
        app_label: &resolved.app_label,
        variant,
        urls,
        version_code: resolved.version_code,
        version_name: &resolved.version_name,
        debuggable: false,
    };
    let build_paths = prepare_build(&prep_opts)?;
    let aab_paths = prepare_aab_paths(build_crate, &resolved.app_label)?;

    println!(
        "Building AAB (package={}, label={}, versionCode={}, versionName={})",
        resolved.java_url, resolved.app_label, resolved.version_code, resolved.version_name
    );

    // Reuse the existing R-class / javac / d8 pipeline; outputs `classes.dex`
    // into `build_paths.out_dir`.
    build_r_class(sdk_dir, &build_paths, urls)?;
    compile_java(sdk_dir, &build_paths, urls)?;
    build_dex(sdk_dir, &build_paths, urls)?;
    let classes_dex = build_paths.out_dir.join("classes.dex");
    if !classes_dex.is_file() {
        return Err(format!(
            "d8 did not produce classes.dex at {:?}",
            classes_dex
        ));
    }

    // Stage native libs.
    let build_dir = stage_aab_native_libs(
        sdk_dir,
        host_os,
        &underscore_build_crate,
        &aab_paths.staged_libs_dir,
        android_targets,
        args,
        variant,
        urls,
    )?;

    // Stage assets (mirrors `add_resources` / `add_assets_dir_to_apk` / font logic).
    stage_aab_assets(
        build_crate,
        &aab_paths.staged_assets_dir,
        &build_dir,
        android_targets,
        variant,
        config,
    )?;

    // aapt2 compile + link --proto-format. Assets are picked up via -A.
    aapt2_compile_resources(
        sdk_dir,
        &build_paths.res_dir,
        &aab_paths.compiled_res_zip,
        urls,
    )?;
    aapt2_link_proto_apk(
        sdk_dir,
        &build_paths.manifest_file,
        &aab_paths.compiled_res_zip,
        &aab_paths.staged_assets_dir,
        &aab_paths.proto_apk,
        urls,
    )?;

    // Repackage proto APK into the bundle module layout, then zip it.
    assemble_aab_base_module(
        sdk_dir,
        &aab_paths.proto_apk,
        &classes_dex,
        &aab_paths.staged_libs_dir,
        &aab_paths.base_module_dir,
        &aab_paths.base_module_zip,
    )?;

    // bundletool build-bundle
    run_bundletool_build_bundle(sdk_dir, &aab_paths.base_module_zip, &aab_paths.dst_aab)?;

    // Sign with jarsigner if a keystore was provided (or default).
    if let Some(opts) = &signing {
        sign_aab(sdk_dir, &aab_paths.dst_aab, opts)?;
    } else {
        println!("Skipping signing (--no-sign). The AAB is unsigned and will not be accepted by the Play Store as-is.");
    }

    println!("AAB Build completed: {}", aab_paths.dst_aab.display());
    if let Some(opts) = &signing {
        let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        if opts.keystore == cargo_manifest_dir.join("debug.keystore") {
            println!(
                "NOTE: signed with the bundled debug keystore. The Play Store will reject this AAB. Pass --keystore=<path> --keystore-pass=<pw> --keystore-key-alias=<alias> --keystore-key-pass=<pw> to sign with a release keystore."
            );
        }
    }

    Ok(BuildAabResult {
        dst_aab: aab_paths.dst_aab,
    })
}

pub fn build(
    sdk_dir: &Path,
    host_os: HostOs,
    package_name: Option<String>,
    app_label: Option<String>,
    version_code: Option<VersionCodeStrategy>,
    version_name: Option<String>,
    min_sdk_version: Option<usize>,
    args: &[String],
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    config: &AndroidConfig,
    urls: &AndroidSDKUrls,
) -> Result<BuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    let binary_name =
        get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
    let underscore_build_crate = build_crate.replace('-', "_");

    let resolved = resolve_packaging_inputs(
        build_crate,
        &binary_name,
        package_name,
        app_label,
        version_code,
        version_name,
        min_sdk_version,
        urls,
    )?;
    // Apply the per-app min SDK override; see the same pattern in `build_aab`.
    let mut effective_urls = *urls;
    if let Some(min) = resolved.min_sdk_version_override {
        effective_urls.sdk_version = min;
    }
    let urls = &effective_urls;

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
        // APK/dev builds keep `-C prefer-dynamic` for faster incremental relinks.
        true,
    )?;
    // For APK builds, debuggable matches the cargo profile: release -> false,
    // anything else -> true (matches the historical behavior of `cargo makepad
    // android run`).
    let debuggable = get_profile_from_args(args) != "release";
    let prep_opts = PrepareBuildOpts {
        build_crate,
        java_url: &resolved.java_url,
        app_label: &resolved.app_label,
        variant,
        urls,
        version_code: resolved.version_code,
        version_name: &resolved.version_name,
        debuggable,
    };
    let build_paths = prepare_build(&prep_opts)?;

    println!(
        "Building APK (package={}, label={}, versionCode={}, versionName={}, debuggable={})",
        resolved.java_url,
        resolved.app_label,
        resolved.version_code,
        resolved.version_name,
        debuggable
    );
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
        java_url: resolved.java_url,
    })
}

pub fn run(
    sdk_dir: &Path,
    host_os: HostOs,
    package_name: Option<String>,
    app_label: Option<String>,
    version_code: Option<VersionCodeStrategy>,
    version_name: Option<String>,
    min_sdk_version: Option<usize>,
    args: &[String],
    targets: &[AndroidTarget],
    android_variant: &AndroidVariant,
    config: &AndroidConfig,
    urls: &AndroidSDKUrls,
    devices: Vec<String>,
) -> Result<(), String> {
    let build_crate = get_build_crate_from_args(args)?;
    let result = build(
        sdk_dir,
        host_os,
        package_name,
        app_label,
        version_code,
        version_name,
        min_sdk_version,
        args,
        targets,
        android_variant,
        config,
        urls,
    )?;

    let cwd = std::env::current_dir().unwrap();
    // alright so how will we do multiple targets eh

    fn android_start_args(java_url: &str, build_crate: &str) -> Vec<String> {
        let mut args = vec![
            "shell".to_string(),
            "am".to_string(),
            "start".to_string(),
            "-S".to_string(),
            "-n".to_string(),
            format!("{0}/{0}.MakepadApp", java_url),
        ];
        if let Ok(studio_host) = std::env::var("STUDIO_HOST") {
            if !studio_host.trim().is_empty() {
                println!("Android launch intent makepad.STUDIO_HOST={}", studio_host);
                args.push("--es".to_string());
                args.push("makepad.STUDIO_HOST".to_string());
                args.push(studio_host);
                args.push("--es".to_string());
                args.push("makepad.STUDIO_CRATE".to_string());
                args.push(build_crate.to_string());
            }
        } else {
            println!("Android launch intent makepad.STUDIO_HOST is not set");
        }
        args
    }

    if devices.len() == 0 {
        println!("Uploading android application");
        shell_env_cap(
            &[],
            &cwd,
            sdk_dir.join("platform-tools/adb").to_str().unwrap(),
            &[
                "install",
                "--no-incremental",
                "-r",
                (result.dst_apk.to_str().unwrap()),
            ],
        )?;
        println!("Starting android application");
        let start_args = android_start_args(&result.java_url, build_crate);
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
                    "--no-incremental",
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
            let start_args = android_start_args(&result.java_url, build_crate);
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
    use super::{compose_android_rustflags, parse_adb_devices, parse_ip_addr_show, parse_ip_route};

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

    #[test]
    fn compose_android_rustflags_adds_prefer_dynamic() {
        assert_eq!(
            compose_android_rustflags(None, "--cfg android_target=\"aarch64\"", true),
            "-C prefer-dynamic --cfg android_target=\"aarch64\""
        );
    }

    #[test]
    fn compose_android_rustflags_preserves_existing_flags() {
        assert_eq!(
            compose_android_rustflags(Some("-C debuginfo=1"), "--cfg android_target=\"aarch64\"", true),
            "-C debuginfo=1 -C prefer-dynamic --cfg android_target=\"aarch64\""
        );
    }

    #[test]
    fn compose_android_rustflags_does_not_duplicate_prefer_dynamic() {
        assert_eq!(
            compose_android_rustflags(
                Some("-C prefer-dynamic -C debuginfo=1"),
                "--cfg android_target=\"aarch64\"",
                true,
            ),
            "-C prefer-dynamic -C debuginfo=1 --cfg android_target=\"aarch64\""
        );
    }

    #[test]
    fn compose_android_rustflags_omits_prefer_dynamic_when_disabled() {
        assert_eq!(
            compose_android_rustflags(
                Some("-C debuginfo=1"),
                "--cfg android_target=\"aarch64\"",
                false,
            ),
            "-C debuginfo=1 --cfg android_target=\"aarch64\""
        );
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
