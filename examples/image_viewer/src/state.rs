use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct State {
    pub images: Vec<PathBuf>,
}

impl State {
    pub fn load_images(&mut self, path: &Path) {
        self.images = fs::read_dir(path)
            .expect("unable to read directory")
            .map(|entry| entry.expect("unable to read entry").path())
            .filter(|path| {
                path.extension()
                    .map_or(false, |ext| ["png", "jpg"].iter().any(|e| *e == ext))
            })
            .collect::<Vec<_>>();
    }

    pub fn root(&self) -> Option<&Path> {
        self.images.first().map(|p| p.parent().unwrap())
    }
}
