#![cfg(target_os = "macos")]

use super::AppleOs;
use super::super::compile::PlistValues;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Package {
    root: PathBuf,
    path: PathBuf,
}

impl Package {
    fn new(metadata: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "makepad-info-plist-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("workspace/member package/src")).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let package = Self { path: root.join("workspace/member package"), root };
        fs::write(package.path.join("src/main.rs"), "fn main() {}\n").unwrap();
        package.manifest(metadata);
        package
    }

    fn manifest(&self, metadata: &str) {
        fs::write(self.path.join("Cargo.toml"), format!(
            "[package]\nname = \"plist-test\"\nversion = \"0.1.0\"\n{metadata}\n"
        )).unwrap();
    }

    fn plist(&self, relative: &str, xml: &str, binary: bool) -> PathBuf {
        let path = self.path.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, xml.trim_start()).unwrap();
        if binary {
            checked_plutil(&path, &["-convert", "binary1"]);
        }
        path
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn load(path: &Path, os: AppleOs) -> Result<Option<super::InfoPlist>, String> {
    super::load(path, "plist-test", os)
}

fn generated(os: AppleOs) -> String {
    PlistValues {
        identifier: "org.example.plist-test".into(),
        display_name: "Plist Test".into(),
        name: "Plist Test".into(),
        executable: "plist-test".into(),
        version: "1.2.3".into(),
    }.to_plist_file(os)
}

fn plist_xml(root: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">{root}</plist>\n")
}

fn one_entry(key: &str, value_xml: &str) -> String {
    plist_xml(&format!("<dict><key>{key}</key>{value_xml}</dict>"))
}

fn plutil(path: &Path, args: &[&str]) -> Output {
    Command::new("/usr/bin/plutil").args(args).arg("--").arg(path).output().unwrap()
}

fn checked_plutil(path: &Path, args: &[&str]) -> String {
    let output = plutil(path, args);
    assert!(output.status.success(), "plutil {args:?} {} failed: {}{}",
        path.display(), String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

fn raw(path: &Path, key: &str, expected_type: &str) -> String {
    checked_plutil(path, &["-extract", key, "raw", "-expect", expected_type, "-o", "-", "-n"])
}

fn extracted_xml(path: &Path, key: &str) -> String {
    checked_plutil(path, &["-extract", key, "xml1", "-o", "-"])
}

fn assert_missing(path: &Path, key: &str) {
    assert!(!plutil(path, &["-extract", key, "raw", "-o", "-"]).status.success(), "unexpected key {key}");
}

fn assert_path(error: &str, path: &Path) {
    assert!(error.contains(path.to_str().unwrap()), "missing path {}: {error}", path.display());
}

#[test]
fn no_opt_in_preserves_defaults_without_adding_speech_permission() {
    let package = Package::new("");
    for os in [AppleOs::Ios, AppleOs::Tvos] {
        assert!(load(&package.path, os).unwrap().is_none());
        let defaults = package.plist("Generated.plist", &generated(os), false);
        checked_plutil(&defaults, &["-lint"]);
        assert_missing(&defaults, "NSSpeechRecognitionUsageDescription");
        assert_eq!(raw(&defaults, "CFBundleIdentifier", "string"), "org.example.plist-test");
    }
}

#[test]
fn unrelated_hex_and_exponent_metadata_works_with_and_without_overlay() {
    let unrelated = "[package.metadata.unrelated]\nhex_value = 0xdead_beef\nscale = 1.5e+4\n";
    let package = Package::new(unrelated);
    assert!(load(&package.path, AppleOs::Ios).unwrap().is_none());
    package.manifest(&format!(
        "{unrelated}\n[package.metadata.makepad.ios]\ninfo_plist = \"App Info.plist\""
    ));
    package.plist("App Info.plist", &one_entry("CFBundleDisplayName", "<string>Custom App</string>"), false);
    let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
    let merged = package.plist("Merged.plist", &overlay.merge(&generated(AppleOs::Ios)).unwrap(), false);
    assert_eq!(raw(&merged, "CFBundleDisplayName", "string"), "Custom App");
}

#[test]
fn selects_each_platform_relative_to_package_with_spaces_in_paths() {
    let package = Package::new(
        r#"[package.metadata.makepad.ios]
info_plist = '''app metadata/iOS "Bob's".plist'''
[package.metadata.makepad.tvos]
info_plist = '''app metadata/tvOS "Bob's".plist'''"#
    );
    for (os, filename, label) in [
        (AppleOs::Ios, "app metadata/iOS \"Bob's\".plist", "Phone App"),
        (AppleOs::Tvos, "app metadata/tvOS \"Bob's\".plist", "TV App"),
    ] {
        let path = package.plist(filename, &one_entry("CFBundleDisplayName", &format!("<string>{label}</string>")), false);
        let custom = load(&package.path, os).unwrap().unwrap();
        assert_eq!(custom.path, path);
        let merged = package.plist("Merged.plist", &custom.merge(&generated(os)).unwrap(), false);
        assert_eq!(raw(&merged, "CFBundleDisplayName", "string"), label);
    }
}

#[test]
fn configuring_one_platform_does_not_opt_in_the_other() {
    let package = Package::new(
        "[package.metadata.makepad.ios]\ninfo_plist = \"missing-ios.plist\""
    );
    assert!(load(&package.path, AppleOs::Tvos).unwrap().is_none());
    assert!(load(&package.path, AppleOs::Ios).is_err());
    package.manifest("[package.metadata.makepad.tvos]\ninfo_plist = \"missing-tvos.plist\"");
    assert!(load(&package.path, AppleOs::Ios).unwrap().is_none());
    assert!(load(&package.path, AppleOs::Tvos).is_err());
}

#[test]
fn xml_and_binary_overlays_preserve_types_and_unspecified_defaults() {
    let package = Package::new(
        "[package.metadata.makepad.ios]\ninfo_plist = \"metadata/Info.plist\""
    );
    let custom = plist_xml(r#"<dict>
        <key>NSMicrophoneUsageDescription</key><string>Record the user's "voice" &amp; review &lt;text&gt;.</string>
        <key>NSSpeechRecognitionUsageDescription</key><string>Transcribe a message.</string>
        <key>LSEnvironment</key><dict><key>APP_SETTING</key><string>custom</string></dict>
        <key>AppConfiguration</key><dict>
            <key>Enabled</key><false/>
            <key>SignedCount</key><integer>-42</integer>
            <key>UnsignedCount</key><integer>18446744073709551615</integer>
            <key>Payload</key><data>AAF/gP8=</data>
            <key>Created</key><date>2026-09-09T12:34:56Z</date>
            <key>Items</key><array><string>one &amp; &lt;two&gt;</string><real>1.5</real></array>
            <key>odd: key &amp; &lt;tag&gt;</key><string>unusual key</string>
        </dict>
    </dict>"#);
    let source = generated(AppleOs::Ios);
    let defaults = package.plist("Generated.plist", &source, false);
    assert_eq!(raw(&defaults, "LSEnvironment.RUST_BACKTRACE", "string"), "1");
    for binary in [false, true] {
        let path = package.plist("metadata/Info.plist", &custom, binary);
        let original = fs::read(&path).unwrap();
        let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
        let merged = package.plist("Merged.plist", &overlay.merge(&source).unwrap(), false);
        assert_eq!(fs::read(&path).unwrap(), original, "successful merge modified the source plist, binary={binary}");
        checked_plutil(&merged, &["-lint"]);
        for (key, value_type, expected) in [
            ("NSMicrophoneUsageDescription", "string", "Record the user's \"voice\" & review <text>."),
            ("NSSpeechRecognitionUsageDescription", "string", "Transcribe a message."),
            ("LSEnvironment.APP_SETTING", "string", "custom"),
            ("AppConfiguration.Enabled", "bool", "false"),
            ("AppConfiguration.SignedCount", "integer", "-42"),
            ("AppConfiguration.UnsignedCount", "integer", "18446744073709551615"),
            ("AppConfiguration.Payload", "data", "AAF/gP8="),
            ("AppConfiguration.Created", "date", "2026-09-09T12:34:56Z"),
            ("AppConfiguration.Items", "array", "2"),
            ("AppConfiguration.Items.0", "string", "one & <two>"),
            ("AppConfiguration.odd: key & <tag>", "string", "unusual key"),
        ] {
            assert_eq!(raw(&merged, key, value_type), expected, "{key}, binary={binary}");
        }
        assert_eq!(raw(&merged, "AppConfiguration.Items.1", "float").parse::<f64>().unwrap(), 1.5);
        for key in ["CFBundleIdentifier", "CFBundleName", "CFBundleExecutable", "UIDeviceFamily",
            "UISupportedInterfaceOrientations~ipad", "UIRequiredDeviceCapabilities", "MinimumOSVersion",
            "NSLocationWhenInUseUsageDescription"] {
            assert_eq!(extracted_xml(&merged, key), extracted_xml(&defaults, key), "default changed: {key}, binary={binary}");
        }
        assert_missing(&merged, "LSEnvironment.RUST_BACKTRACE");
    }
}

#[test]
fn missing_or_malformed_custom_file_reports_its_path() {
    let package = Package::new(
        "[package.metadata.makepad.ios]\ninfo_plist = \"missing Info.plist\""
    );
    let path = package.path.join("missing Info.plist");
    let missing = load(&package.path, AppleOs::Ios).err().expect("missing plist must fail");
    assert_path(&missing, &path);
    fs::write(&path, b"<plist><dict><key>Broken</key>").unwrap();
    let malformed = load(&package.path, AppleOs::Ios).err().expect("malformed plist must fail");
    assert_path(&malformed, &path);
}

#[test]
fn non_dictionary_custom_root_is_rejected_in_xml_and_binary() {
    let package = Package::new(
        "[package.metadata.makepad.ios]\ninfo_plist = \"Info.plist\""
    );
    for binary in [false, true] {
        let path = package.plist("Info.plist", &plist_xml("<array><string>not a dictionary</string></array>"), binary);
        let error = load(&package.path, AppleOs::Ios).err().expect("array root must fail");
        assert_path(&error, &path);
        assert!(error.contains("dictionary"), "{error}");
    }
}

#[test]
fn invalid_metadata_reports_manifest_and_key() {
    let package = Package::new("");
    for invalid in ["\"\"", "false", "42", "[]"] {
        package.manifest(&format!("[package.metadata.makepad.ios]\ninfo_plist = {invalid}"));
        let error = load(&package.path, AppleOs::Ios).err().expect("invalid path metadata must fail");
        assert_path(&error, &package.path.join("Cargo.toml"));
        assert!(error.contains("package.metadata.makepad.ios.info_plist"), "{error}");
    }
}

#[test]
fn missing_or_malformed_manifest_reports_its_path() {
    let package = Package::new("");
    let path = package.path.join("Cargo.toml");
    fs::remove_file(&path).unwrap();
    let missing = load(&package.path, AppleOs::Ios).err().expect("missing manifest must fail");
    assert_path(&missing, &path);
    fs::write(&path, "[package\n").unwrap();
    let malformed = load(&package.path, AppleOs::Ios).err().expect("malformed manifest must fail");
    assert_path(&malformed, &path);
}

#[test]
fn rejects_changed_bundle_identity_or_executable_with_path_context() {
    let package = Package::new(
        "[package.metadata.makepad.ios]\ninfo_plist = \"Info.plist\""
    );
    for key in ["CFBundleIdentifier", "CFBundleExecutable"] {
        for replacement in ["<string>different</string>", "<false/>"] {
            let path = package.plist("Info.plist", &one_entry(key, replacement), false);
            let original = fs::read(&path).unwrap();
            let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
            let error = overlay.merge(&generated(AppleOs::Ios)).unwrap_err();
            assert_eq!(fs::read(&path).unwrap(), original, "rejected merge modified the source plist");
            assert_path(&error, &path);
            assert!(error.contains(key), "{error}");
        }
    }
}

#[test]
fn accepts_unchanged_bundle_identity_and_executable() {
    let package = Package::new(
        "[package.metadata.makepad.ios]\ninfo_plist = \"Info.plist\""
    );
    let custom = plist_xml("<dict><key>CFBundleIdentifier</key><string>org.example.plist-test</string>\
        <key>CFBundleExecutable</key><string>plist-test</string>\
        <key>CFBundleDisplayName</key><string>Custom App</string></dict>");
    package.plist("Info.plist", &custom, false);
    let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
    let merged = package.plist("Merged.plist", &overlay.merge(&generated(AppleOs::Ios)).unwrap(), false);
    assert_eq!(raw(&merged, "CFBundleIdentifier", "string"), "org.example.plist-test");
    assert_eq!(raw(&merged, "CFBundleExecutable", "string"), "plist-test");
    assert_eq!(raw(&merged, "CFBundleDisplayName", "string"), "Custom App");
}
