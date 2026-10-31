//! Clipboard representations travel together, in one reply to a `ClipRead` command.
use crate::media::{PackR, PackW};

#[derive(Default)]
pub struct ClipboardData {
    pub text: String,
    pub png: Vec<u8>,
    /// Original file URL and its decoded native path (the same path used by file drops).
    pub files: Vec<(String, String)>,
}

impl ClipboardData {
    pub fn pack(&self) -> Vec<u8> {
        let mut w = PackW::new();
        w.str(&self.text).bytes(&self.png);
        for (url, path) in &self.files {
            w.str(url).str(path);
        }
        w.finish()
    }

    pub fn read(bytes: &[u8]) -> Option<Self> {
        let mut r = PackR::new(bytes);
        let text = r.str();
        let png = r.bytes().to_vec();
        let mut files = Vec::new();
        while r.ok && !r.at_end() {
            files.push((r.str(), r.str()));
        }
        r.ok.then_some(Self { text, png, files })
    }
}
