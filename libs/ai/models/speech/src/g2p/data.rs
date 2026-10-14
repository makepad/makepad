//! The G2P's lexicon tables, downloaded with the Kokoro model.
//!
//! The three MKLEX tables (`tools/build_lexicon.py`, `tools/build_zh_lexicon.py`
//! build them; libs/ai/NOTICE credits their sources) are assets of the Kokoro
//! model: the hub registry's `kokoro` entry and the phone store's `speak`
//! entry download them, size- and sha256-verified, into the model's asset dir
//! (`tts/`, next to `kokoro-v1_0.mktts`). Loading the model loads them from
//! there, read once for the life of the process. A missing table is an error
//! ("speech lexicon not downloaded"), never a fallback.

use super::packed::Packed;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// One downloadable table. The hub registry and the phone catalogue list the
/// same name, size and hash (their tests check it against these).
#[derive(Clone, Copy, Debug)]
pub struct DataFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// Where the tables are hosted (the file name is appended).
pub const BASE_URL: &str = "https://makepad.nl/models/speech/kokoro-g2p";

/// misaki's US English IPA lexicon (Apache-2.0).
pub const US_LEXICON: DataFile = DataFile {
    name: "us_lexicon.bin",
    size: 2_917_518,
    sha256: "294682efc873a8201106f4dddfcb8e1b16c5d47c0fb8ddaf00c351d62212245c",
};
/// Characters and polyphonic phrases to pinyin (pinyin-data, MIT).
pub const ZH_PINYIN: DataFile = DataFile {
    name: "zh_pinyin.bin",
    size: 1_393_676,
    sha256: "4d266969277671e6f3238cde2211ab5c6956fc86e08e71e3d9a5f5fdfa3e0471",
};
/// jieba's word-frequency dictionary (MIT).
pub const ZH_WORDS: DataFile = DataFile {
    name: "zh_words.bin",
    size: 1_838_159,
    sha256: "ab066b416e8952f97791e09c3770e417ba111df80c13336eb76d4fac0c008169",
};

pub const FILES: [DataFile; 3] = [US_LEXICON, ZH_PINYIN, ZH_WORDS];

impl DataFile {
    pub fn url(&self) -> String {
        format!("{BASE_URL}/{}", self.name)
    }
}

struct Tables {
    lexicon: Box<[u8]>,
    pinyin: Box<[u8]>,
    words: Box<[u8]>,
}

static TABLES: OnceLock<Tables> = OnceLock::new();
static LOADING: Mutex<()> = Mutex::new(());

fn read(dir: &Path, file: &DataFile) -> Result<Box<[u8]>, String> {
    let path = dir.join(file.name);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "speech lexicon not downloaded: {} is missing (an asset of the Kokoro model; download the model again)",
                path.display()
            ))
        }
        Err(e) => return Err(format!("speech lexicon {}: {e}", path.display())),
    };
    if bytes.len() as u64 != file.size || !bytes.starts_with(super::packed::MAGIC) {
        return Err(format!(
            "speech lexicon {} is not the expected table ({} bytes, want {}); download the Kokoro model again",
            path.display(),
            bytes.len(),
            file.size
        ));
    }
    Ok(bytes.into_boxed_slice())
}

/// Load the tables from `dir` (the Kokoro model's asset dir). The first load
/// in a process wins; later calls are no-ops.
pub fn load(dir: &Path) -> Result<(), String> {
    if TABLES.get().is_some() {
        return Ok(());
    }
    let _guard = LOADING.lock().unwrap_or_else(|e| e.into_inner());
    if TABLES.get().is_some() {
        return Ok(());
    }
    let tables = Tables {
        lexicon: read(dir, &US_LEXICON)?,
        pinyin: read(dir, &ZH_PINYIN)?,
        words: read(dir, &ZH_WORDS)?,
    };
    let _ = TABLES.set(tables);
    Ok(())
}

/// The shared model asset dir the hub and the phone store download into:
/// `MAKEPAD_G2P_DIR` if set, else `<MAKEPAD_HOME or ~/.makepad>/weights/tts`.
/// For the command-line tools and tests, which load no model first.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MAKEPAD_G2P_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("MAKEPAD_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(".makepad")
    });
    home.join("weights").join("tts")
}

/// [`load`] from [`default_dir`].
pub fn load_default() -> Result<(), String> {
    load(&default_dir())
}

fn tables() -> &'static Tables {
    #[cfg(test)]
    for_tests();
    TABLES.get().expect("speech lexicon not loaded: load the Kokoro model (or call g2p::data::load) first")
}

pub(crate) fn lexicon() -> Packed {
    Packed::new(&tables().lexicon)
}

pub(crate) fn pinyin() -> Packed {
    Packed::new(&tables().pinyin)
}

pub(crate) fn words() -> Packed {
    Packed::new(&tables().words)
}

/// Tests read the downloaded tables; without them they fail saying so.
#[cfg(test)]
fn for_tests() {
    if let Err(e) = load_default() {
        panic!("{e} (set MAKEPAD_G2P_DIR to a dir holding {}, {}, {})", US_LEXICON.name, ZH_PINYIN.name, ZH_WORDS.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_table_says_it_was_not_downloaded() {
        let dir = std::env::temp_dir().join(format!("g2p-missing-{}", std::process::id()));
        let err = read(&dir, &US_LEXICON).unwrap_err();
        assert!(err.starts_with("speech lexicon not downloaded"), "{err}");
    }
}
