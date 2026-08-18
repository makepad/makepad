//! The same on-disk library asset-ui shows (`local/ai_content_library`).
//! The embedded asset-ui server catalog can be empty while this folder is
//! full; VJ pads read it directly so the two apps show the same content.

use makepad_asset_data::{AssetId, AssetRevisionId, BlobId, MediaType};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn library_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../local/ai_content_library"
    ))
}

#[derive(Clone, Debug)]
pub struct LocalItem {
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub blob: BlobId,
    pub title: String,
    pub domain: String,
    pub content_type: String,
    pub path: PathBuf,
    pub thumb: Option<PathBuf>,
    pub media: MediaType,
    pub len: u64,
}

impl LocalItem {
    pub fn is_mesh(&self) -> bool {
        matches!(self.media, MediaType::Glb)
    }

    pub fn is_audio(&self) -> bool {
        matches!(self.media, MediaType::Wav | MediaType::Ogg)
    }

    pub fn is_still(&self) -> bool {
        matches!(self.media, MediaType::Png | MediaType::Jpeg)
    }

    pub fn is_video(&self) -> bool {
        matches!(self.media, MediaType::Mp4)
    }

    pub fn is_sfx(&self) -> bool {
        self.is_audio()
            && (self.domain.eq_ignore_ascii_case("sfx")
                || self.domain.eq_ignore_ascii_case("audio")
                || self.content_type.starts_with("audio/"))
    }

    pub fn is_music(&self) -> bool {
        self.is_audio() && self.domain.eq_ignore_ascii_case("music")
    }

    pub fn is_billboard(&self) -> bool {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            == Some("billboard")
            || self.content_type.contains("stateful-billboard")
            || self.content_type.contains("billboard")
                && self
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    == Some("billboard")
    }

    pub fn is_visual(&self) -> bool {
        self.is_mesh() || self.is_still() || self.is_video() || self.is_billboard()
    }

    pub fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_ascii_lowercase();
        self.title.to_ascii_lowercase().contains(&q)
            || self.domain.to_ascii_lowercase().contains(&q)
            || self.content_type.to_ascii_lowercase().contains(&q)
    }
}

#[derive(Default)]
pub struct LocalLibrary {
    pub items: Vec<LocalItem>,
    pub by_asset: HashMap<AssetId, usize>,
}

impl LocalLibrary {
    pub fn load() -> LocalLibrary {
        Self::load_from(&library_dir())
    }

    pub fn load_from(dir: &Path) -> LocalLibrary {
        let mut items = Vec::new();
        let index_path = dir.join("index.json");
        let Ok(text) = std::fs::read_to_string(&index_path) else {
            return LocalLibrary { items, by_asset: HashMap::new() };
        };
        let Ok(value) = parse_items(&text) else {
            return LocalLibrary { items, by_asset: HashMap::new() };
        };
        for raw in value {
            let Some(item) = LocalItem::from_raw(dir, raw) else { continue };
            items.push(item);
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("billboard") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if items.iter().any(|i| {
                    i.path.file_name().and_then(|n| n.to_str()) == Some(name)
                }) {
                    continue;
                }
                if let Some(item) = LocalItem::from_raw(
                    dir,
                    RawItem {
                        file: name.to_string(),
                        label: path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(name)
                            .to_string(),
                        domain: "billboard".into(),
                        content_type: "text/x-stateful-billboard".into(),
                    },
                ) {
                    items.push(item);
                }
            }
        }
        // Newest last in the file; show newest first like asset-ui.
        items.reverse();
        let mut by_asset = HashMap::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            by_asset.insert(item.asset, i);
        }
        LocalLibrary { items, by_asset }
    }

    pub fn get(&self, asset: &AssetId) -> Option<&LocalItem> {
        self.by_asset.get(asset).map(|&i| &self.items[i])
    }

    pub fn filtered(&self, pred: impl Fn(&LocalItem) -> bool, query: &str) -> Vec<&LocalItem> {
        self.items
            .iter()
            .filter(|item| pred(item) && item.matches(query))
            .collect()
    }
}

struct RawItem {
    file: String,
    label: String,
    domain: String,
    content_type: String,
}

fn parse_items(text: &str) -> Result<Vec<RawItem>, ()> {
    let v: serde_lite::JsonValue = serde_lite::json_from_str(text).or(Err(()))?;
    let obj = v.as_object().ok_or(())?;
    let arr = obj.get("items").and_then(|x| x.as_array()).ok_or(())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        // Tolerate individual malformed entries: asset-ui writes this index
        // concurrently, and one half-written row must not empty the whole
        // library for the session.
        let Some(o) = item.as_object() else { continue };
        let Some(file) = o.get("file").and_then(|x| x.as_str()).map(str::to_string) else {
            continue;
        };
        let label = o
            .get("label")
            .and_then(|x| x.as_str())
            .unwrap_or(&file)
            .to_string();
        let domain = o
            .get("domain")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let content_type = o
            .get("content_type")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        out.push(RawItem { file, label, domain, content_type });
    }
    Ok(out)
}

impl LocalItem {
    fn from_raw(dir: &Path, raw: RawItem) -> Option<LocalItem> {
        if raw.file.is_empty() || raw.file.contains('/') || raw.file.contains('\\') {
            return None;
        }
        let path = dir.join(&raw.file);
        if !path.is_file() {
            return None;
        }
        let media = media_of(&raw.file, &raw.content_type)?;
        let len = std::fs::metadata(&path).ok()?.len();
        let (asset, revision, blob) = ids_for(&raw.file);
        let thumb = {
            let sidecar = dir.join(format!("{}.thumb", raw.file));
            if sidecar.is_file() {
                Some(sidecar)
            } else if matches!(media, MediaType::Png | MediaType::Jpeg) {
                Some(path.clone())
            } else {
                None
            }
        };
        Some(LocalItem {
            asset,
            revision,
            blob,
            title: if raw.label.is_empty() { raw.file } else { raw.label },
            domain: raw.domain,
            content_type: raw.content_type,
            path,
            thumb,
            media,
            len,
        })
    }
}

fn media_of(file: &str, content_type: &str) -> Option<MediaType> {
    let ext = file.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let ct = content_type.to_ascii_lowercase();
    if ext == "billboard" || ct.contains("stateful-billboard") {
        return Some(MediaType::Text);
    }
    if ext == "glb" || ct.contains("gltf") || ct.contains("model/") {
        return Some(MediaType::Glb);
    }
    if ext == "wav" || ct.contains("wav") {
        return Some(MediaType::Wav);
    }
    if ext == "ogg" || ct.contains("ogg") {
        return Some(MediaType::Ogg);
    }
    if ext == "mp4" || ct.contains("video/") {
        return Some(MediaType::Mp4);
    }
    if ext == "png" || ct.contains("png") {
        return Some(MediaType::Png);
    }
    if ext == "jpg" || ext == "jpeg" || ct.contains("jpeg") {
        return Some(MediaType::Jpeg);
    }
    None
}

fn ids_for(file: &str) -> (AssetId, AssetRevisionId, BlobId) {
    let h = fnv16(file.as_bytes());
    let mut rev = [0u8; 32];
    rev[..16].copy_from_slice(&h);
    rev[16..].copy_from_slice(&fnv16(file.as_bytes().iter().rev().copied().collect::<Vec<_>>().as_slice()));
    (AssetId::from_bytes(h), AssetRevisionId::from_bytes(rev), BlobId::from_bytes(rev))
}

fn fnv16(bytes: &[u8]) -> [u8; 16] {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut hash2 = 0x1000_0000_01b3_u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
        hash2 ^= (b as u64).wrapping_shl(1);
        hash2 = hash2.wrapping_mul(0x100_0000_01b3);
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&hash.to_le_bytes());
    out[8..].copy_from_slice(&hash2.to_le_bytes());
    out
}

mod serde_lite {
    //! Tiny JSON pull for the library index. Avoids a new crate dep.

    pub enum JsonValue {
        Null,
        Bool(bool),
        Number(f64),
        String(String),
        Array(Vec<JsonValue>),
        Object(std::collections::BTreeMap<String, JsonValue>),
    }

    impl JsonValue {
        pub fn as_object(&self) -> Option<&std::collections::BTreeMap<String, JsonValue>> {
            match self {
                JsonValue::Object(o) => Some(o),
                _ => None,
            }
        }
        pub fn as_array(&self) -> Option<&[JsonValue]> {
            match self {
                JsonValue::Array(a) => Some(a),
                _ => None,
            }
        }
        pub fn as_str(&self) -> Option<&str> {
            match self {
                JsonValue::String(s) => Some(s),
                _ => None,
            }
        }
    }

    pub fn json_from_str(s: &str) -> Result<JsonValue, ()> {
        let mut p = Parser { s: s.as_bytes(), i: 0 };
        let v = p.parse()?;
        p.skip_ws();
        if p.i != p.s.len() {
            return Err(());
        }
        Ok(v)
    }

    struct Parser<'a> {
        s: &'a [u8],
        i: usize,
    }

    impl Parser<'_> {
        fn parse(&mut self) -> Result<JsonValue, ()> {
            self.skip_ws();
            let b = *self.s.get(self.i).ok_or(())?;
            match b {
                b'n' => self.lit(b"null").map(|_| JsonValue::Null),
                b't' => self.lit(b"true").map(|_| JsonValue::Bool(true)),
                b'f' => self.lit(b"false").map(|_| JsonValue::Bool(false)),
                b'"' => self.string().map(JsonValue::String),
                b'[' => self.array(),
                b'{' => self.object(),
                b'-' | b'0'..=b'9' => self.number(),
                _ => Err(()),
            }
        }

        fn object(&mut self) -> Result<JsonValue, ()> {
            self.i += 1;
            let mut map = std::collections::BTreeMap::new();
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.i += 1;
                return Ok(JsonValue::Object(map));
            }
            loop {
                self.skip_ws();
                let key = self.string()?;
                self.skip_ws();
                if self.next() != Some(b':') {
                    return Err(());
                }
                let val = self.parse()?;
                map.insert(key, val);
                self.skip_ws();
                match self.next() {
                    Some(b',') => continue,
                    Some(b'}') => return Ok(JsonValue::Object(map)),
                    _ => return Err(()),
                }
            }
        }

        fn array(&mut self) -> Result<JsonValue, ()> {
            self.i += 1;
            let mut arr = Vec::new();
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.i += 1;
                return Ok(JsonValue::Array(arr));
            }
            loop {
                arr.push(self.parse()?);
                self.skip_ws();
                match self.next() {
                    Some(b',') => continue,
                    Some(b']') => return Ok(JsonValue::Array(arr)),
                    _ => return Err(()),
                }
            }
        }

        fn string(&mut self) -> Result<String, ()> {
            if self.next() != Some(b'"') {
                return Err(());
            }
            let mut out = String::new();
            while let Some(b) = self.next() {
                match b {
                    b'"' => return Ok(out),
                    b'\\' => {
                        let e = self.next().ok_or(())?;
                        match e {
                            b'"' | b'\\' | b'/' => out.push(e as char),
                            b'n' => out.push('\n'),
                            b'r' => out.push('\r'),
                            b't' => out.push('\t'),
                            b'u' => {
                                let mut hex = 0u32;
                                for _ in 0..4 {
                                    let h = self.next().ok_or(())?;
                                    hex = hex << 4
                                        | match h {
                                            b'0'..=b'9' => (h - b'0') as u32,
                                            b'a'..=b'f' => (h - b'a' + 10) as u32,
                                            b'A'..=b'F' => (h - b'A' + 10) as u32,
                                            _ => return Err(()),
                                        };
                                }
                                if let Some(ch) = char::from_u32(hex) {
                                    out.push(ch);
                                }
                            }
                            _ => return Err(()),
                        }
                    }
                    _ => out.push(b as char),
                }
            }
            Err(())
        }

        fn number(&mut self) -> Result<JsonValue, ()> {
            let start = self.i;
            if self.peek() == Some(b'-') {
                self.i += 1;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
            if self.peek() == Some(b'.') {
                self.i += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            }
            let s = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| ())?;
            Ok(JsonValue::Number(s.parse().map_err(|_| ())?))
        }

        fn lit(&mut self, expect: &[u8]) -> Result<(), ()> {
            if self.s.get(self.i..self.i + expect.len()) == Some(expect) {
                self.i += expect.len();
                Ok(())
            } else {
                Err(())
            }
        }

        fn skip_ws(&mut self) {
            while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
                self.i += 1;
            }
        }

        fn peek(&self) -> Option<u8> {
            self.s.get(self.i).copied()
        }

        fn next(&mut self) -> Option<u8> {
            let b = self.s.get(self.i).copied()?;
            self.i += 1;
            Some(b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_asset_ui_library_index() {
        let lib = LocalLibrary::load();
        assert!(
            lib.items.len() > 100,
            "expected the asset-ui library, got {}",
            lib.items.len()
        );
        assert!(
            lib.items.iter().any(|i| i.is_visual()),
            "library should include images, video, or meshes"
        );
    }
}
