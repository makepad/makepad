use std::env;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        "macos" => {
            for framework in [
                "AppKit",
                "AVFoundation",
                "CoreAudio",
                "CoreFoundation",
                "CoreGraphics",
                "CoreMedia",
                "CoreMidi",
                "CoreVideo",
                "Foundation",
                "GameController",
                "IOSurface",
                "IOKit",
                "ImageIO",
                "Metal",
                "OpenGL",
                "QuartzCore",
                "Security",
                "ScreenCaptureKit",
                "VideoToolbox",
                "Vision",
            ] {
                println!("cargo:rustc-link-lib=framework={framework}");
            }
        }
        "ios" | "tvos" => {
            for framework in [
                "AVFoundation",
                "CoreFoundation",
                "CoreGraphics",
                "CoreMedia",
                "CoreVideo",
                "Foundation",
                "GameController",
                "ImageIO",
                "Metal",
                "QuartzCore",
                "Security",
                "UIKit",
                "VideoToolbox",
                "Vision",
            ] {
                println!("cargo:rustc-link-lib=framework={framework}");
            }
        }
        _ => {}
    }
}
