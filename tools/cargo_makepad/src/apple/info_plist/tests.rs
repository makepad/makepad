use super::{AppleOs, Dictionary, Value};
use super::super::compile::PlistValues;
use std::{
    fs,
    path::{Path, PathBuf},
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

    fn plist(&self, relative: &str, value: &Value, binary: bool) -> PathBuf {
        let path = self.path.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = fs::File::create(&path).unwrap();
        if binary {
            value.to_writer_binary(file).unwrap();
        } else {
            value.to_writer_xml(file).unwrap();
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

fn dictionary(xml: &str) -> Dictionary {
    Value::from_reader_xml(xml.trim_start().as_bytes()).unwrap().into_dictionary().unwrap()
}

fn one_entry(key: &str, value: Value) -> Value {
    let mut entries = Dictionary::new();
    entries.insert(key.into(), value);
    Value::Dictionary(entries)
}

fn assert_path(error: &str, path: &Path) {
    assert!(error.contains(path.to_str().unwrap()), "missing path {}: {error}", path.display());
}

#[test]
fn no_opt_in_preserves_defaults_without_adding_speech_permission() {
    let package = Package::new("");
    for os in [AppleOs::Ios, AppleOs::Tvos] {
        assert!(load(&package.path, os).unwrap().is_none());
        let defaults = dictionary(&generated(os));
        assert!(!defaults.contains_key("NSSpeechRecognitionUsageDescription"));
        assert_eq!(defaults.get("CFBundleIdentifier").and_then(Value::as_string), Some("org.example.plist-test"));
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
    package.plist("App Info.plist", &one_entry("CFBundleDisplayName", Value::String("Custom App".into())), false);
    let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
    let merged = dictionary(&overlay.merge(&generated(AppleOs::Ios)).unwrap());
    assert_eq!(merged.get("CFBundleDisplayName").and_then(Value::as_string), Some("Custom App"));
}

#[test]
fn selects_each_platform_relative_to_package_with_spaces_in_paths() {
    let package = Package::new(
        "[package.metadata.makepad.ios]\ninfo_plist = \"app metadata/iOS Info.plist\"\n\
         [package.metadata.makepad.tvos]\ninfo_plist = \"app metadata/tvOS Info.plist\""
    );
    for (os, filename, label) in [
        (AppleOs::Ios, "app metadata/iOS Info.plist", "Phone App"),
        (AppleOs::Tvos, "app metadata/tvOS Info.plist", "TV App"),
    ] {
        let path = package.plist(filename, &one_entry("CFBundleDisplayName", Value::String(label.into())), false);
        let custom = load(&package.path, os).unwrap().unwrap();
        assert_eq!(custom.path, path);
        let merged = dictionary(&custom.merge(&generated(os)).unwrap());
        assert_eq!(merged.get("CFBundleDisplayName").and_then(Value::as_string), Some(label));
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
    let mut nested = Dictionary::new();
    nested.insert("Enabled".into(), Value::Boolean(false));
    nested.insert("SignedCount".into(), Value::Integer((-42_i64).into()));
    nested.insert("UnsignedCount".into(), Value::Integer(u64::MAX.into()));
    nested.insert("Payload".into(), Value::Data(vec![0, 1, 127, 128, 255]));
    nested.insert("Items".into(), Value::Array(vec![Value::String("one & <two>".into()), Value::Real(1.5)]));
    let mut custom = Dictionary::new();
    custom.insert("NSMicrophoneUsageDescription".into(), Value::String("Record the user's \"voice\" & review <text>.".into()));
    custom.insert("NSSpeechRecognitionUsageDescription".into(), Value::String("Transcribe a message.".into()));
    custom.insert("LSEnvironment".into(), one_entry("APP_SETTING", Value::String("custom".into())));
    custom.insert("AppConfiguration".into(), Value::Dictionary(nested));
    let source = generated(AppleOs::Ios);
    let defaults = dictionary(&source);
    assert!(defaults["LSEnvironment"].as_dictionary().unwrap().contains_key("RUST_BACKTRACE"));
    for binary in [false, true] {
        package.plist("metadata/Info.plist", &Value::Dictionary(custom.clone()), binary);
        let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
        let merged = dictionary(&overlay.merge(&source).unwrap());
        for (key, value) in &custom {
            assert_eq!(merged.get(key), Some(value), "custom value changed: {key}, binary={binary}");
        }
        for (key, value) in &defaults {
            if !custom.contains_key(key) {
                assert_eq!(merged.get(key), Some(value), "default changed: {key}, binary={binary}");
            }
        }
        assert!(!merged["LSEnvironment"].as_dictionary().unwrap().contains_key("RUST_BACKTRACE"));
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
        let path = package.plist("Info.plist", &Value::Array(vec![Value::String("not a dictionary".into())]), binary);
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
        for replacement in [Value::String("different".into()), Value::Boolean(false)] {
            let path = package.plist("Info.plist", &one_entry(key, replacement), false);
            let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
            let error = overlay.merge(&generated(AppleOs::Ios)).unwrap_err();
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
    let source = generated(AppleOs::Ios);
    let defaults = dictionary(&source);
    let mut custom = Dictionary::new();
    for key in ["CFBundleIdentifier", "CFBundleExecutable"] {
        custom.insert(key.into(), defaults[key].clone());
    }
    custom.insert("CFBundleDisplayName".into(), Value::String("Custom App".into()));
    package.plist("Info.plist", &Value::Dictionary(custom), false);
    let overlay = load(&package.path, AppleOs::Ios).unwrap().unwrap();
    let merged = dictionary(&overlay.merge(&source).unwrap());
    assert_eq!(merged["CFBundleIdentifier"], defaults["CFBundleIdentifier"]);
    assert_eq!(merged["CFBundleExecutable"], defaults["CFBundleExecutable"]);
    assert_eq!(merged.get("CFBundleDisplayName").and_then(Value::as_string), Some("Custom App"));
}
