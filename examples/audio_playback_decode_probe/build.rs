use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(makepad_audio_decode_api)");
    println!("cargo:rustc-check-cfg=cfg(makepad_audio_decode_fixtures)");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let audio_rs = manifest_dir.join("../../platform/src/audio.rs");
    let audio_spec =
        manifest_dir.join("../../specs/Month-2/audio-playback-makepad-support.spec.md");
    println!("cargo:rerun-if-changed={}", audio_rs.display());
    println!("cargo:rerun-if-changed={}", audio_spec.display());

    if let Ok(audio_source) = fs::read_to_string(&audio_rs) {
        let has_decode_api = audio_source.contains("fn from_mp3")
            && audio_source.contains("fn from_ogg_opus")
            && audio_source.contains("fn detect_audio_format")
            && audio_source.contains("enum AudioError");

        if has_decode_api {
            println!("cargo:rustc-cfg=makepad_audio_decode_api");
        }
    }

    let mp3_fixture = manifest_dir.join("fixtures/mono_100ms_44100.mp3");
    let ogg_fixture = manifest_dir.join("fixtures/stereo_100ms_48000.opus.ogg");
    println!("cargo:rerun-if-changed={}", mp3_fixture.display());
    println!("cargo:rerun-if-changed={}", ogg_fixture.display());

    let has_fixtures = fs::metadata(&mp3_fixture)
        .map(|metadata| metadata.len() > 16)
        .unwrap_or(false)
        && fs::metadata(&ogg_fixture)
            .map(|metadata| metadata.len() > 16)
            .unwrap_or(false);

    if has_fixtures {
        println!("cargo:rustc-cfg=makepad_audio_decode_fixtures");
    }
}
