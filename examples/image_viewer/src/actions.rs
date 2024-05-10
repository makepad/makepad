use std::path::PathBuf;

enum AppAction {
    LoadImages(PathBuf),
    SelectImage(PathBuf),
}
