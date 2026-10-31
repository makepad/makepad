use {
    crate::display_context::NativeAppearance,
    std::{env, path::PathBuf},
};

/// Finds the Breeze color scheme file that KWin uses to draw a light or dark title bar.
/// KWin gives a missing file light colors, so we only return a file that exists.
pub(crate) fn find_kde_color_scheme(appearance: NativeAppearance) -> Option<String> {
    let file_name = match appearance {
        NativeAppearance::System => return None,
        NativeAppearance::Light => "color-schemes/BreezeLight.colors",
        NativeAppearance::Dark => "color-schemes/BreezeDark.colors",
    };
    // Same search order as KWin's own lookup: $XDG_DATA_HOME, then each of $XDG_DATA_DIRS.
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let data_dirs = env::var_os("XDG_DATA_DIRS")
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    // KWin reads the path as UTF-8, and only absolute paths mean a file to it.
    data_home
        .into_iter()
        .chain(env::split_paths(&data_dirs))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(file_name))
        .filter(|path| path.is_file())
        .find_map(|path| path.into_os_string().into_string().ok())
}
