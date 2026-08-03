//! The on-device game library: a directory of game directories.
//!
//! This is what the librarian searches when a kid says "play the one with the
//! planes", and what the browser UI lists.

use crate::manifest::Manifest;
use crate::pack::{pack_dir, read_package, write_package, PkgError, GAME_FILE, MANIFEST_FILE};
use std::path::PathBuf;

pub struct LibraryEntry {
    pub dir: PathBuf,
    /// Directory name — the stable id used to load a game.
    pub slug: String,
    pub manifest: Manifest,
}

pub struct Library {
    pub root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Games are directories holding both required members. Anything else in
    /// the root is ignored rather than reported as broken.
    pub fn list(&self) -> Vec<LibraryEntry> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for e in entries.flatten() {
            let dir = e.path();
            if !dir.join(GAME_FILE).is_file() {
                continue;
            }
            let Some(slug) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let manifest = std::fs::read(dir.join(MANIFEST_FILE))
                .ok()
                .and_then(|b| Manifest::parse(&b).ok())
                .unwrap_or_else(|| Manifest {
                    // A game with no readable manifest still shows up, named
                    // after its directory — better than vanishing from the list.
                    name: slug.to_string(),
                    ..Default::default()
                });
            out.push(LibraryEntry {
                dir: dir.clone(),
                slug: slug.to_string(),
                manifest,
            });
        }
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        out
    }

    pub fn get(&self, slug: &str) -> Option<LibraryEntry> {
        self.list().into_iter().find(|e| e.slug == slug)
    }

    /// Install package bytes under `slug`. Refuses a slug that is not a plain
    /// directory name, so an id from a registry cannot walk the filesystem.
    pub fn install(&self, slug: &str, package: &[u8]) -> Result<LibraryEntry, PkgError> {
        let safe = sanitize_slug(slug)?;
        let pkg = read_package(package)?;
        let dir = self.root.join(&safe);
        std::fs::create_dir_all(&dir).map_err(|e| PkgError::Io(e.to_string()))?;
        write_package(&pkg, &dir)?;
        Ok(LibraryEntry {
            dir,
            slug: safe,
            manifest: pkg.manifest,
        })
    }

    pub fn uninstall(&self, slug: &str) -> Result<(), PkgError> {
        let safe = sanitize_slug(slug)?;
        let dir = self.root.join(&safe);
        if !dir.join(GAME_FILE).is_file() {
            return Err(PkgError::MissingMember(GAME_FILE));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| PkgError::Io(e.to_string()))
    }

    pub fn pack(&self, slug: &str) -> Result<Vec<u8>, PkgError> {
        let safe = sanitize_slug(slug)?;
        pack_dir(&self.root.join(safe))
    }

    /// Rank library entries against a spoken phrase by word overlap over name
    /// and description. Deterministic and offline — the local model refines
    /// this, it does not replace it.
    pub fn search(&self, phrase: &str) -> Vec<(LibraryEntry, u32)> {
        let words: Vec<String> = phrase
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2 && !STOPWORDS.contains(w))
            .map(|w| w.to_string())
            .collect();
        let mut scored: Vec<(LibraryEntry, u32)> = self
            .list()
            .into_iter()
            .map(|e| {
                let hay = format!(
                    "{} {} {}",
                    e.manifest.name, e.manifest.description, e.slug
                )
                .to_lowercase();
                let score = words.iter().filter(|w| hay.contains(w.as_str())).count() as u32;
                (e, score)
            })
            .filter(|(_, s)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.slug.cmp(&b.0.slug)));
        scored
    }
}

const STOPWORDS: [&str; 12] = [
    "the", "and", "with", "that", "play", "game", "one", "lets", "let", "can", "you", "please",
];

fn sanitize_slug(slug: &str) -> Result<String, PkgError> {
    let ok = !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(slug.to_string())
    } else {
        Err(PkgError::Rejected(format!("unsafe game id {slug:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_cannot_walk_the_filesystem() {
        for bad in ["../escape", "/abs", "a/b", "", "with space", "dot.dot", &"x".repeat(65)] {
            assert!(sanitize_slug(bad).is_err(), "should reject {bad:?}");
        }
        assert!(sanitize_slug("speedway-2").is_ok());
    }
}
