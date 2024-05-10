use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct State {
    pub root: PathBuf,
    pub images: Vec<PathBuf>,
    pub selected_image: Option<PathBuf>,
}

impl State {
    pub fn load_images(&mut self, path: &Path) {
        self.root = path.to_path_buf();
        self.images = fs::read_dir(path)
            .expect("unable to read directory")
            .map(|entry| entry.expect("unable to read entry").path())
            .filter(|path| {
                path.extension().map_or(false, |ext| {
                    ["png", "jpg", "jpeg"].iter().any(|e| *e == ext)
                })
            })
            .collect::<Vec<_>>();
    }

    pub fn select_image(&mut self, image: PathBuf) {
        self.selected_image = Some(image);
    }
}
