//! The app identity artwork of every desktop style: one SVG per app and
//! style, compiled in. widgets-core's `app_icon` draws them for window
//! chrome, launchers and shelves; Motion documents name them as
//! `<style>/<app>` (`icon("macos/mail")`).

/// The styles' icon sets, in the order of [`BundledIcon::variants`] (a
/// desktop style's `icon_set()` indexes it).
pub const SETS: [&str; 7] = ["omarchy", "macos", "windows", "windows-2000", "nextstep", "ios", "android"];

/// One app identity: its name and its markup in each set of [`SETS`].
pub struct BundledIcon {
    pub name: &'static str,
    pub variants: [&'static str; 7],
}

macro_rules! icon {
    ($name:literal) => {
        BundledIcon {
            name: $name,
            variants: [
                include_str!(concat!("../../../widgets/themes/omarchy/icons/", $name, ".svg")),
                include_str!(concat!("../../../widgets/themes/macos/icons/", $name, ".svg")),
                include_str!(concat!("../../../widgets/themes/windows/icons/", $name, ".svg")),
                include_str!(concat!("../../../widgets/themes/windows-2000/icons/", $name, ".svg")),
                include_str!(concat!("../../../widgets/themes/nextstep/icons/", $name, ".svg")),
                include_str!(concat!("../../../widgets/themes/ios/icons/", $name, ".svg")),
                include_str!(concat!("../../../widgets/themes/android/icons/", $name, ".svg")),
            ],
        }
    };
}

/// Every bundled app identity.
pub static ICONS: &[BundledIcon] = &[
    icon!("applications"),
    icon!("browser"),
    icon!("files"),
    icon!("terminal"),
    icon!("mixer"),
    icon!("task"),
    icon!("sheets"),
    icon!("photos"),
    icon!("clock"),
    icon!("weather"),
    icon!("finance"),
    icon!("mail"),
    icon!("notes"),
    icon!("calendar"),
    icon!("reminders"),
    icon!("calculator"),
    icon!("fabric"),
    icon!("score"),
    icon!("video"),
    icon!("route"),
    icon!("vj"),
    icon!("fab"),
    icon!("studio"),
    icon!("scope"),
    icon!("image"),
    icon!("pdf"),
    icon!("aichat"),
    icon!("counter"),
    icon!("app"),
    icon!("file-folder"),
    icon!("file-generic"),
    icon!("file-image"),
    icon!("file-text"),
    icon!("file-code"),
    icon!("file-audio"),
    icon!("file-video"),
    icon!("file-archive"),
    icon!("file-pdf"),
];

/// The markup of `name` in `set` (one of [`SETS`]).
pub fn icon(set: &str, name: &str) -> Option<&'static str> {
    let index = SETS.iter().position(|s| *s == set)?;
    ICONS.iter().find(|icon| icon.name == name).map(|icon| icon.variants[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every SVG in each style's icons folder is in the table, so a new
    /// app's artwork cannot be left out of it.
    #[test]
    fn the_table_holds_every_themes_icons() {
        for set in SETS {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../widgets/themes").join(set).join("icons");
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("svg") {
                    continue;
                }
                let stem = path.file_stem().unwrap().to_str().unwrap();
                assert!(icon(set, stem).is_some(), "{set}/{stem} is not in ICONS");
            }
        }
    }
}
