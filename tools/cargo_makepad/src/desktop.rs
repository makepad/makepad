use crate::makepad_shell::{shell_env_cap, write_text};
use crate::utils::{
    get_build_crate_from_args, get_crate_dir, get_package_binary_name, get_profile_from_args,
    get_target_from_args, resolve_app_icon_env, AppIconEnv, APP_ICON_ENV_VARS, APP_ICON_IDX_1024,
    APP_ICON_IDX_512, APP_ICON_IDX_ICO,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default, Clone)]
struct MacosSignOptions {
    enabled: bool,
    cert: Option<String>,
    entitlements: Option<PathBuf>,
    debugger: bool,
    get_task_allow: bool,
    runtime: bool,
}

fn is_windows_target(args: &[String]) -> bool {
    get_target_from_args(args)
        .map(|t| t.contains("windows"))
        .unwrap_or(cfg!(target_os = "windows"))
}

fn is_macos_target(args: &[String]) -> bool {
    get_target_from_args(args)
        .map(|t| t.contains("apple-darwin"))
        .unwrap_or(cfg!(target_os = "macos"))
}

fn is_linux_target(args: &[String]) -> bool {
    get_target_from_args(args)
        .map(|t| t.contains("linux"))
        .unwrap_or(cfg!(target_os = "linux"))
}

fn binary_path(args: &[String], binary_name: &str) -> PathBuf {
    let profile = get_profile_from_args(args);
    let mut path = cargo_target_dir();
    if let Some(target) = get_target_from_args(args) {
        path.push(target);
    }
    path.push(profile);
    path.push(binary_name);
    if is_windows_target(args) {
        path.set_extension("exe");
    }
    path
}

fn cargo_target_dir() -> PathBuf {
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        if target_dir.is_absolute() {
            target_dir
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(target_dir)
        }
    } else {
        PathBuf::from("target")
    }
}

fn write_windows_icon_resource(
    icon_env: &AppIconEnv,
    build_crate: &str,
) -> Result<Option<PathBuf>, String> {
    let out_dir = PathBuf::from("target/makepad-desktop/windows-res").join(build_crate);
    fs::create_dir_all(&out_dir).map_err(|e| format!("failed to create {:?}: {e}", out_dir))?;

    let rc_path = out_dir.join("app_icon.rc");
    let res_path = out_dir.join("app_icon.res");
    let ico = &icon_env[APP_ICON_IDX_ICO];
    fs::write(
        &rc_path,
        format!("1 ICON \"{}\"\n", ico.replace('\\', "\\\\")),
    )
    .map_err(|e| format!("failed to write {:?}: {e}", rc_path))?;

    let mut tries: Vec<(&str, Vec<String>)> = Vec::new();
    tries.push((
        "llvm-rc",
        vec![
            "/nologo".to_string(),
            format!("/fo{}", res_path.to_string_lossy()),
            rc_path.to_string_lossy().to_string(),
        ],
    ));
    tries.push((
        "rc",
        vec![
            "/nologo".to_string(),
            format!("/fo{}", res_path.to_string_lossy()),
            rc_path.to_string_lossy().to_string(),
        ],
    ));
    tries.push((
        "llvm-windres",
        vec![
            rc_path.to_string_lossy().to_string(),
            "-O".to_string(),
            "coff".to_string(),
            "-o".to_string(),
            res_path.to_string_lossy().to_string(),
        ],
    ));
    tries.push((
        "windres",
        vec![
            rc_path.to_string_lossy().to_string(),
            "-O".to_string(),
            "coff".to_string(),
            "-o".to_string(),
            res_path.to_string_lossy().to_string(),
        ],
    ));

    for (tool, args) in tries {
        if let Ok(status) = Command::new(tool).args(&args).status() {
            if status.success() && res_path.is_file() {
                return Ok(Some(res_path));
            }
        }
    }

    eprintln!(
        "warning: could not compile Windows .rc icon resource (tried llvm-rc/rc/llvm-windres/windres). executable icon embedding skipped."
    );
    Ok(None)
}

fn add_windows_icon_link_arg(
    cmd: &mut Command,
    args: &[String],
    icon_env: &AppIconEnv,
    build_crate: &str,
) -> Result<(), String> {
    if !is_windows_target(args) {
        return Ok(());
    }
    let Some(res) = write_windows_icon_resource(icon_env, build_crate)? else {
        return Ok(());
    };

    let res_abs = res.canonicalize().unwrap_or(res);
    let res_arg = format!("-C link-arg={}", res_abs.to_string_lossy());
    let mut rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
    if !rustflags.is_empty() {
        rustflags.push(' ');
    }
    rustflags.push_str(&res_arg);
    cmd.env("RUSTFLAGS", rustflags);
    Ok(())
}

fn write_macos_app_bundle(
    args: &[String],
    build_crate: &str,
    icon_env: &AppIconEnv,
) -> Result<(), String> {
    if !is_macos_target(args) {
        return Ok(());
    }
    let binary_name =
        get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
    let bin = binary_path(args, &binary_name);
    if !bin.is_file() {
        eprintln!(
            "warning: built binary not found at {} (package: {}, binary: {})",
            bin.display(),
            build_crate,
            binary_name
        );
        return Ok(());
    }

    let profile = get_profile_from_args(args);
    let app_dir = macos_app_bundle_path_for(&binary_name, &profile);
    let macos_dir = app_dir.join("Contents/MacOS");
    let resources_dir = app_dir.join("Contents/Resources");
    fs::create_dir_all(&macos_dir).map_err(|e| format!("failed to create {:?}: {e}", macos_dir))?;
    fs::create_dir_all(&resources_dir)
        .map_err(|e| format!("failed to create {:?}: {e}", resources_dir))?;

    let dst_bin = macos_dir.join(&binary_name);
    fs::copy(&bin, &dst_bin)
        .map_err(|e| format!("failed to copy {:?} to {:?}: {e}", bin, dst_bin))?;

    let crate_dir = get_crate_dir(build_crate)?;
    let icns_src = crate_dir.join("resources/icon.icns");
    if icns_src.is_file() {
        fs::copy(&icns_src, resources_dir.join("AppIcon.icns"))
            .map_err(|e| format!("failed to copy {:?}: {e}", icns_src))?;
    } else {
        let fallback = Path::new(&icon_env[APP_ICON_IDX_1024]);
        fs::copy(fallback, resources_dir.join("AppIcon.png"))
            .map_err(|e| format!("failed to copy {:?}: {e}", fallback))?;
    }

    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
    <key>CFBundleExecutable</key><string>{binary_name}</string>\n\
    <key>CFBundleIdentifier</key><string>dev.makepad.{build_crate}</string>\n\
    <key>CFBundleName</key><string>{binary_name}</string>\n\
    <key>CFBundleDisplayName</key><string>{binary_name}</string>\n\
    <key>CFBundlePackageType</key><string>APPL</string>\n\
    <key>CFBundleVersion</key><string>1.0.0</string>\n\
    <key>CFBundleShortVersionString</key><string>1.0.0</string>\n\
    <key>CFBundleIconFile</key><string>AppIcon</string>\n\
    <key>GCSupportsControllerUserInteraction</key><true/>\n\
    <key>GCSupportedGameControllers</key>\n\
    <array>\n\
        <dict>\n\
            <key>ProfileName</key><string>ExtendedGamepad</string>\n\
        </dict>\n\
    </array>\n\
</dict>\n\
</plist>\n"
    );
    let plist_path = app_dir.join("Contents/Info.plist");
    fs::write(&plist_path, plist).map_err(|e| format!("failed to write Info.plist: {e}"))?;

    println!("[cargo-makepad] macOS app bundle: {}", app_dir.display());
    println!("[cargo-makepad] macOS app binary: {}", dst_bin.display());
    println!(
        "[cargo-makepad] macOS app resources: {}",
        resources_dir.display()
    );
    println!("[cargo-makepad] macOS info plist: {}", plist_path.display());
    Ok(())
}

fn macos_app_bundle_path_for(binary_name: &str, profile: &str) -> PathBuf {
    cargo_target_dir()
        .join("makepad-desktop/macos")
        .join(profile)
        .join(format!("{binary_name}.app"))
}

pub(crate) fn resolve_codesign_identity(cert: Option<&str>) -> Result<String, String> {
    if let Some(cert) = cert {
        return Ok(cert.to_string());
    }
    // A standing choice for machines with several identities.
    if let Some(cert) = std::env::var("MAKEPAD_CODESIGN_IDENTITY").ok().filter(|cert| !cert.trim().is_empty()) {
        return Ok(cert.trim().to_string());
    }

    let cwd = std::env::current_dir().unwrap();
    let identities = shell_env_cap(
        &[],
        &cwd,
        "security",
        &["find-identity", "-v", "-p", "codesigning"],
    )?;
    pick_codesign_identity(&identities)
}

/// The identity to sign with from `security find-identity -v` output: the
/// Apple Development one, or the only one there is. Several certificates
/// under one name (a renewed certificate beside the old one) are the same
/// signer as far as a designated requirement is concerned, which names the
/// certificate's subject, not the certificate: the first is taken, by hash,
/// since the shared name alone is ambiguous to codesign.
fn pick_codesign_identity(listing: &str) -> Result<String, String> {
    let mut apple_development: Vec<(String, String)> = Vec::new();
    let mut all: Vec<(String, String)> = Vec::new();
    for line in listing.lines() {
        let name = line
            .split('"')
            .nth(1)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let hash = line
            .split_whitespace()
            .nth(1)
            .filter(|hash| hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()));
        if let (Some(name), Some(hash)) = (name, hash) {
            all.push((hash.to_string(), name.to_string()));
            if name.starts_with("Apple Development:") {
                apple_development.push((hash.to_string(), name.to_string()));
            }
        }
    }

    let pick = |mut found: Vec<(String, String)>| {
        if found.len() == 1 {
            let (_, name) = found.remove(0);
            Some(name)
        } else if found.windows(2).all(|pair| pair[0].1 == pair[1].1) {
            found.first().map(|(hash, _)| hash.clone())
        } else {
            None
        }
    };
    match (apple_development.is_empty(), all.is_empty()) {
        (_, true) => Err(
            "no codesigning identity found; pass --cert=\"Apple Development: ...\" explicitly"
                .to_string(),
        ),
        (false, _) => pick(apple_development).ok_or_else(|| {
            "multiple Apple Development identities found; pass --cert=<identity or hash> or set MAKEPAD_CODESIGN_IDENTITY"
                .to_string()
        }),
        (true, false) => pick(all).ok_or_else(|| {
            "multiple codesigning identities and no Apple Development one; pass --cert=<identity or hash>"
                .to_string()
        }),
    }
}

#[cfg(test)]
mod codesign_identity_tests {
    use super::pick_codesign_identity;

    #[test]
    fn a_renewed_certificate_beside_the_old_one_is_one_signer() {
        let listing = "  1) 82B72FFCA0221B8B84962F0D6D10B2743ED9E118 \"Apple Development: A B (TEAM)\"\n  2) ED88B9ACB31D482F8746EEE08B057BE803095162 \"Apple Development: A B (TEAM)\"\n     2 valid identities found\n";
        assert_eq!(pick_codesign_identity(listing).unwrap(), "82B72FFCA0221B8B84962F0D6D10B2743ED9E118");
    }

    #[test]
    fn one_identity_is_named_and_different_names_need_a_choice() {
        let one = "  1) 82B72FFCA0221B8B84962F0D6D10B2743ED9E118 \"Apple Development: A B (TEAM)\"\n";
        assert_eq!(pick_codesign_identity(one).unwrap(), "Apple Development: A B (TEAM)");
        let two = "  1) 82B72FFCA0221B8B84962F0D6D10B2743ED9E118 \"Apple Development: A B (TEAM)\"\n  2) ED88B9ACB31D482F8746EEE08B057BE803095162 \"Apple Development: C D (TEAM2)\"\n";
        assert!(pick_codesign_identity(two).is_err());
        assert!(pick_codesign_identity("     0 valid identities found\n").is_err());
    }
}

fn maybe_write_generated_entitlements(
    args: &[String],
    build_crate: &str,
    binary_name: &str,
    sign: &MacosSignOptions,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = &sign.entitlements {
        return Ok(Some(path.clone()));
    }
    if !sign.debugger && !sign.get_task_allow {
        return Ok(None);
    }

    let profile = get_profile_from_args(args);
    let out_dir = cargo_target_dir()
        .join("makepad-desktop/macos-sign")
        .join(profile)
        .join(build_crate);
    fs::create_dir_all(&out_dir).map_err(|e| format!("failed to create {:?}: {e}", out_dir))?;
    let out_path = out_dir.join(format!("{binary_name}.entitlements"));

    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n",
    );
    if sign.debugger {
        xml.push_str("    <key>com.apple.security.cs.debugger</key>\n    <true/>\n");
    }
    if sign.get_task_allow {
        xml.push_str("    <key>com.apple.security.get-task-allow</key>\n    <true/>\n");
    }
    xml.push_str("</dict>\n</plist>\n");
    write_text(&out_path, &xml)?;
    Ok(Some(out_path))
}

pub(crate) fn codesign_path(
    path: &Path,
    identity: &str,
    entitlements: Option<&Path>,
    runtime: bool,
) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let mut args = vec![
        "--force".to_string(),
        "--timestamp=none".to_string(),
        "--sign".to_string(),
        identity.to_string(),
    ];
    if runtime {
        args.push("--options".to_string());
        args.push("runtime".to_string());
    }
    if let Some(entitlements) = entitlements {
        args.push("--entitlements".to_string());
        args.push(entitlements.to_string_lossy().to_string());
        args.push("--generate-entitlement-der".to_string());
    }
    args.push(path.to_string_lossy().to_string());
    let refs = args.iter().map(|v| v.as_str()).collect::<Vec<_>>();
    shell_env_cap(&[], &cwd, "codesign", &refs)?;
    Ok(())
}

fn sign_macos_artifacts(args: &[String], sign: &MacosSignOptions) -> Result<(), String> {
    if !is_macos_target(args) || !sign.enabled {
        return Ok(());
    }

    let build_crate = get_build_crate_from_args(args)?;
    let binary_name =
        get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
    let identity = resolve_codesign_identity(sign.cert.as_deref())?;
    let entitlements = maybe_write_generated_entitlements(args, build_crate, &binary_name, sign)?;
    let entitlements_ref = entitlements.as_deref();

    let bin = binary_path(args, &binary_name);
    if !bin.is_file() {
        return Err(format!(
            "built binary not found at {} (package: {}, binary: {})",
            bin.display(),
            build_crate,
            binary_name
        ));
    }
    codesign_path(
        &bin,
        &identity,
        entitlements_ref,
        sign.runtime || sign.debugger,
    )?;
    println!("[cargo-makepad] macOS signed binary: {}", bin.display());

    let profile = get_profile_from_args(args);
    let app_dir = macos_app_bundle_path_for(&binary_name, &profile);
    if app_dir.is_dir() {
        codesign_path(
            &app_dir,
            &identity,
            entitlements_ref,
            sign.runtime || sign.debugger,
        )?;
        println!(
            "[cargo-makepad] macOS signed app bundle: {}",
            app_dir.display()
        );
    }

    Ok(())
}

fn write_linux_desktop_entry(
    args: &[String],
    build_crate: &str,
    icon_env: &AppIconEnv,
) -> Result<(), String> {
    if !is_linux_target(args) {
        return Ok(());
    }
    let binary_name =
        get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
    let bin = binary_path(args, &binary_name);
    if !bin.is_file() {
        eprintln!(
            "warning: built binary not found at {} (package: {}, binary: {})",
            bin.display(),
            build_crate,
            binary_name
        );
        return Ok(());
    }

    let profile = get_profile_from_args(args);
    let out = PathBuf::from("target/makepad-desktop/linux").join(profile);
    fs::create_dir_all(&out).map_err(|e| format!("failed to create {:?}: {e}", out))?;

    let icon = Path::new(&icon_env[APP_ICON_IDX_512]);
    let icon_dst = out.join(format!("{binary_name}.png"));
    fs::copy(icon, &icon_dst)
        .map_err(|e| format!("failed to copy {:?} to {:?}: {e}", icon, icon_dst))?;

    let desktop_path = out.join(format!("{binary_name}.desktop"));
    let exec_path = bin.canonicalize().unwrap_or(bin);
    let icon_path = icon_dst.canonicalize().unwrap_or(icon_dst.clone());
    let body = format!(
        "[Desktop Entry]\nType=Application\nName={binary_name}\nExec={}\nIcon={}\nTerminal=false\nCategories=Development;\n",
        exec_path.to_string_lossy(),
        icon_path.to_string_lossy(),
    );
    fs::write(&desktop_path, body)
        .map_err(|e| format!("failed to write {:?}: {e}", desktop_path))?;

    println!(
        "[cargo-makepad] Linux desktop entry: {}",
        desktop_path.display()
    );
    println!("[cargo-makepad] Linux desktop icon: {}", icon_dst.display());
    Ok(())
}

fn post_build_assets(args: &[String], icon_env: &AppIconEnv) -> Result<(), String> {
    let build_crate = get_build_crate_from_args(args)?;
    write_macos_app_bundle(args, build_crate, icon_env)?;
    write_linux_desktop_entry(args, build_crate, icon_env)?;
    Ok(())
}

fn run_cargo(
    subcommand: &str,
    args: &[String],
    icon_env: Option<AppIconEnv>,
    sign: &MacosSignOptions,
) -> Result<(), String> {
    let mut cmd = Command::new("cargo");
    cmd.arg(subcommand);
    cmd.args(args);

    if let Some(icon) = icon_env.as_ref() {
        for (var, value) in APP_ICON_ENV_VARS.iter().zip(icon.iter()) {
            cmd.env(var, value);
        }

        let build_crate = get_build_crate_from_args(args)?;
        add_windows_icon_link_arg(&mut cmd, args, icon, build_crate)?;
    }

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cargo {subcommand}: {e}"))?;

    if !status.success() {
        return Err(format!("cargo {subcommand} failed with status {status}"));
    }

    if subcommand == "build" {
        if let Some(icon) = icon_env.as_ref() {
            post_build_assets(args, icon)?;
        }
        sign_macos_artifacts(args, sign)?;
    }

    Ok(())
}

fn resolve_icons_for_args(args: &[String]) -> Result<Option<AppIconEnv>, String> {
    let build_crate = get_build_crate_from_args(args)?;
    resolve_app_icon_env(build_crate)
}

pub fn handle_desktop(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("desktop requires a subcommand: build, run, check, bundle, sign".to_string());
    }

    match args[0].as_str() {
        "build" => {
            let (sign, cargo_args) = parse_macos_sign_options(&args[1..])?;
            let icon_env = resolve_icons_for_args(&cargo_args)?;
            run_cargo("build", &cargo_args, icon_env, &sign)
        }
        "run" => {
            let (sign, cargo_args) = parse_macos_sign_options(&args[1..])?;
            if sign.enabled {
                return Err(
                    "desktop run does not support signing inline; use `cargo makepad desktop build --sign ...` followed by the signed binary"
                        .to_string(),
                );
            }
            let icon_env = resolve_icons_for_args(&cargo_args)?;
            run_cargo("run", &cargo_args, icon_env, &sign)
        }
        "check" => {
            let (sign, cargo_args) = parse_macos_sign_options(&args[1..])?;
            if sign.enabled {
                return Err("desktop check cannot sign artifacts".to_string());
            }
            let icon_env = resolve_icons_for_args(&cargo_args)?;
            run_cargo("check", &cargo_args, icon_env, &sign)
        }
        "bundle" => crate::desktop_bundle::handle_desktop_bundle(&args[1..]),
        "sign" => {
            let (mut sign, cargo_args) = parse_macos_sign_options(&args[1..])?;
            sign.enabled = true;
            sign_macos_artifacts(&cargo_args, &sign)
        }
        cmd => Err(format!("{cmd} is not a valid desktop command")),
    }
}

fn parse_macos_sign_options(args: &[String]) -> Result<(MacosSignOptions, Vec<String>), String> {
    let mut sign = MacosSignOptions::default();
    let mut cargo_args = Vec::new();

    for arg in args {
        if arg == "--sign" {
            sign.enabled = true;
        } else if let Some(value) = arg.strip_prefix("--cert=") {
            sign.enabled = true;
            sign.cert = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--entitlements=") {
            sign.enabled = true;
            sign.entitlements = Some(PathBuf::from(value));
        } else if arg == "--debugger" {
            sign.enabled = true;
            sign.debugger = true;
        } else if arg == "--get-task-allow" {
            sign.enabled = true;
            sign.get_task_allow = true;
        } else if arg == "--runtime" {
            sign.enabled = true;
            sign.runtime = true;
        } else {
            cargo_args.push(arg.clone());
        }
    }

    if sign.entitlements.is_some() && (sign.debugger || sign.get_task_allow) {
        return Err(
            "use either --entitlements=<path> or generated entitlement flags (--debugger/--get-task-allow), not both"
                .to_string(),
        );
    }

    Ok((sign, cargo_args))
}
