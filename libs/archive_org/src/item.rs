//! `/metadata/<id>`: what an item is and which of its files matter.
//!
//! An archive item is a bag of files: the original upload, the archive's
//! derivatives (an h.264 transcode, an Ogg copy, a strip of thumbnails, an
//! animated GIF), and housekeeping (torrents, XML, SQLite). This module
//! reads the bag and answers the two questions a picture wall asks:
//! "what do I play to preview this?" and "what do I take when they hit
//! IMPORT?" — by extension and the archive's own `format` label, never by
//! sniffing bytes it has not fetched.

use crate::http::Error;
use crate::search::{f64_of, parse_json, text_of, u64_of, ItemMediaType};
use makepad_micro_serde::JsonValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSource {
    Original,
    Derivative,
    /// The archive's own bookkeeping (`_meta.xml`, torrents…).
    Metadata,
}

/// What a file is to us, by extension — the only classification a
/// player or importer can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A container the platform decoders play (mp4 / m4v / mov).
    Video,
    /// A still the image decoders read (jpg / png).
    Image,
    Audio,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemFile {
    /// Path inside the item, directories included (`Content/clip.mp4`).
    pub name: String,
    pub source: FileSource,
    /// The archive's label: `h.264`, `MPEG4`, `512Kb MPEG4`, `JPEG`,
    /// `Thumbnail`, `Item Tile`, `Metadata`…
    pub format: String,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub length_secs: f64,
    pub md5: String,
}

impl ItemFile {
    /// Lowercased extension, empty when there is none.
    pub fn ext(&self) -> String {
        self.name
            .rsplit('/')
            .next()
            .and_then(|f| f.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// Bare file name without directories.
    pub fn base_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }

    pub fn kind(&self) -> FileKind {
        match self.ext().as_str() {
            "mp4" | "m4v" | "mov" => FileKind::Video,
            "jpg" | "jpeg" | "png" => FileKind::Image,
            "mp3" | "ogg" | "oga" | "flac" | "wav" | "m4a" | "aac" => FileKind::Audio,
            _ => FileKind::Other,
        }
    }

    /// The archive's own pictures OF the item, not the item: the tile, the
    /// frame strip, the `_thumb.jpg` next to a photo.
    pub fn is_housekeeping(&self) -> bool {
        matches!(self.source, FileSource::Metadata)
            || matches!(
                self.format.as_str(),
                "Thumbnail" | "Item Tile" | "JPEG Thumb" | "Metadata" | "Archive BitTorrent"
            )
            || self.base_name().starts_with("__ia_thumb")
            || self.name.contains(".thumbs/")
    }

    /// `w×h` when the archive measured it.
    pub fn dims(&self) -> Option<(u32, u32)> {
        (self.width > 0 && self.height > 0).then_some((self.width, self.height))
    }

    pub fn download_url(&self, identifier: &str) -> String {
        crate::url::download_url(identifier, &self.name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub creator: String,
    pub date: String,
    pub mediatype: ItemMediaType,
    pub license_url: String,
    /// Subjects / keywords, as given (comma-joined when a list).
    pub subject: String,
    pub files: Vec<ItemFile>,
}

impl ItemFile {
    /// The archive's own H.264 transcode (`h.264`, `h.264 IA`, …): the
    /// stream it plays on its own site, and the one a platform decoder is
    /// surest to accept. An original labelled `MPEG4` may be anything an
    /// uploader had — MPEG-4 Part 2, DivX — which is what "no video
    /// stream" refusals come from.
    pub fn is_h264(&self) -> bool {
        self.format.to_ascii_lowercase().contains("h.264")
    }
}

impl Item {
    fn playable_videos(&self) -> impl Iterator<Item = &ItemFile> {
        self.files
            .iter()
            .filter(|f| f.kind() == FileKind::Video && !f.is_housekeeping() && f.size > 0)
    }

    /// Every playable video, in the order a swatch should try them: the
    /// archive's H.264 transcodes first (smallest first), then the rest
    /// (smallest first). A host that gets a decoder refusal on one moves
    /// to the next.
    pub fn preview_videos(&self) -> Vec<&ItemFile> {
        let mut out: Vec<&ItemFile> = self.playable_videos().collect();
        out.sort_by_key(|f| (!f.is_h264(), f.size, f.name.clone()));
        out
    }

    /// The first swatch candidate — see [`Self::preview_videos`].
    pub fn preview_video(&self) -> Option<&ItemFile> {
        self.preview_videos().into_iter().next()
    }

    /// The video to IMPORT, within `max_bytes`: the original when it is a
    /// playable container that fits, else the largest H.264 transcode that
    /// fits, else the largest playable file that fits. A 1.4 GB original
    /// therefore yields its 376 MB H.264 twin rather than a refusal.
    pub fn import_video_within(&self, max_bytes: u64) -> Option<&ItemFile> {
        let fits = |f: &&ItemFile| f.size <= max_bytes;
        self.playable_videos()
            .find(|f| f.source == FileSource::Original && fits(f))
            .or_else(|| self.playable_videos().filter(|f| f.is_h264() && fits(f)).max_by_key(|f| f.size))
            .or_else(|| self.playable_videos().filter(fits).max_by_key(|f| f.size))
    }

    /// The video to IMPORT with no size limit — see [`Self::import_video_within`].
    pub fn import_video(&self) -> Option<&ItemFile> {
        self.import_video_within(u64::MAX)
    }

    /// Real pictures in the item, largest first: originals and
    /// derivatives alike, minus the archive's thumbnails of them.
    pub fn images(&self) -> Vec<&ItemFile> {
        let mut out: Vec<&ItemFile> = self
            .files
            .iter()
            .filter(|f| f.kind() == FileKind::Image && !f.is_housekeeping() && f.size > 0)
            .collect();
        out.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// The still to show and import: the largest real picture.
    pub fn primary_image(&self) -> Option<&ItemFile> {
        self.images().into_iter().next()
    }
}

fn source_of(s: &str) -> FileSource {
    match s {
        "original" => FileSource::Original,
        "derivative" => FileSource::Derivative,
        _ => FileSource::Metadata,
    }
}

/// Read an item's metadata document.
pub fn parse_item(json: &str) -> Result<Item, Error> {
    let root = parse_json(json)?;
    let meta = root
        .key("metadata")
        .ok_or(Error::Json("no metadata object (item missing or dark)".into()))?;
    let identifier = text_of(meta.key("identifier"));
    if !crate::url::is_valid_identifier(&identifier) {
        return Err(Error::Json("metadata without a valid identifier".into()));
    }
    let mut files = Vec::new();
    if let Some(JsonValue::Array(list)) = root.key("files") {
        for f in list {
            let name = text_of(f.key("name"));
            if name.is_empty() || name.split('/').any(|seg| seg == "..") {
                continue;
            }
            files.push(ItemFile {
                name,
                source: source_of(&text_of(f.key("source"))),
                format: text_of(f.key("format")),
                size: u64_of(f.key("size")),
                width: u64_of(f.key("width")).min(u32::MAX as u64) as u32,
                height: u64_of(f.key("height")).min(u32::MAX as u64) as u32,
                length_secs: f64_of(f.key("length")),
                md5: text_of(f.key("md5")),
            });
        }
    }
    let title = {
        let t = text_of(meta.key("title"));
        if t.trim().is_empty() {
            identifier.clone()
        } else {
            t
        }
    };
    Ok(Item {
        identifier,
        title,
        description: text_of(meta.key("description")),
        creator: text_of(meta.key("creator")),
        date: text_of(meta.key("date")),
        mediatype: ItemMediaType::parse(&text_of(meta.key("mediatype"))),
        license_url: text_of(meta.key("licenseurl")),
        subject: text_of(meta.key("subject")),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOVIE: &str = r#"{"server":"ia803202.us.archive.org","dir":"/3/items/apple-fukkireta",
      "metadata":{"identifier":"apple-fukkireta","mediatype":"movies","creator":"Nifty-Senpai","date":"2010-05-30","description":"A meme","title":"Apple Fukkireta","subject":["cat","meme"],"licenseurl":"https://creativecommons.org/licenses/by/4.0/"},
      "files":[
        {"name":"Apple Fukkireta.mp4","source":"original","size":"11315428","format":"MPEG4","length":"90.16","width":"640","height":"480","md5":"7f"},
        {"name":"__ia_thumb.jpg","source":"original","size":"13728","format":"Item Tile"},
        {"name":"apple-fukkireta.thumbs/Apple Fukkireta_000001.jpg","source":"derivative","format":"Thumbnail","size":"36159"},
        {"name":"Apple Fukkireta.ia.mp4","source":"derivative","size":"5000000","format":"h.264 IA","length":"90.16","width":"320","height":"240"},
        {"name":"Apple Fukkireta.ogv","source":"derivative","size":"4000000","format":"Ogg Video"},
        {"name":"apple-fukkireta_meta.xml","source":"original","format":"Metadata","size":"685"},
        {"name":"../evil.mp4","source":"original","format":"MPEG4","size":"1"}
      ]}"#;

    const IMAGE: &str = r#"{"metadata":{"identifier":"fx_hd","mediatype":"image","title":"Fx Hd"},
      "files":[
        {"name":"__ia_thumb.jpg","source":"original","format":"Item Tile","size":"4122"},
        {"name":"fx hd.png","source":"original","format":"PNG","size":"12031"},
        {"name":"fx hd_thumb.jpg","source":"derivative","format":"JPEG Thumb","size":"2955"},
        {"name":"big.jpg","source":"derivative","format":"JPEG","size":"40000"}
      ]}"#;

    #[test]
    fn movie_selection() {
        let item = parse_item(MOVIE).unwrap();
        assert_eq!(item.identifier, "apple-fukkireta");
        assert_eq!(item.subject, "cat, meme");
        assert_eq!(item.files.len(), 6, "the climbing name is dropped");
        assert_eq!(item.preview_video().unwrap().name, "Apple Fukkireta.ia.mp4");
        assert_eq!(item.import_video().unwrap().name, "Apple Fukkireta.mp4");
        assert_eq!(item.import_video().unwrap().dims(), Some((640, 480)));
        // Under a limit the original does not fit, the H.264 twin serves.
        assert_eq!(item.import_video_within(6_000_000).unwrap().name, "Apple Fukkireta.ia.mp4");
        assert!(item.import_video_within(1).is_none());
        let order: Vec<&str> = item.preview_videos().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(order, vec!["Apple Fukkireta.ia.mp4", "Apple Fukkireta.mp4"]);
        assert!(item.images().is_empty(), "tiles and thumbs are not pictures");
        assert_eq!(item.files[0].ext(), "mp4");
        assert_eq!(item.files[0].kind(), FileKind::Video);
        assert!(item.files[1].is_housekeeping());
        assert!(item.files[2].is_housekeeping());
    }

    #[test]
    fn image_selection() {
        let item = parse_item(IMAGE).unwrap();
        assert!(item.preview_video().is_none());
        let images = item.images();
        assert_eq!(images.len(), 2);
        assert_eq!(item.primary_image().unwrap().name, "big.jpg");
        assert_eq!(images[1].name, "fx hd.png");
        assert_eq!(images[1].base_name(), "fx hd.png");
    }

    #[test]
    fn h264_transcode_outranks_a_smaller_unknown_mp4() {
        let item = parse_item(
            r#"{"metadata":{"identifier":"Technico1949","mediatype":"movies"},
               "files":[
                 {"name":"Technico1949_edit.mp4","source":"original","format":"HiRes MPEG4","size":"4493755"},
                 {"name":"Technico1949.mp4","source":"derivative","format":"h.264","size":"50953372"},
                 {"name":"Technico1949_512kb.mp4","source":"derivative","format":"512Kb MPEG4","size":"34850929"},
                 {"name":"big.m4v","source":"original","format":"MPEG4","size":"1424984189"}
               ]}"#,
        )
        .unwrap();
        let order: Vec<&str> = item.preview_videos().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(order, vec!["Technico1949.mp4", "Technico1949_edit.mp4", "Technico1949_512kb.mp4", "big.m4v"]);
        assert_eq!(item.import_video_within(512 << 20).unwrap().name, "Technico1949_edit.mp4");
        assert_eq!(item.import_video_within(30 << 20).unwrap().name, "Technico1949_edit.mp4");
    }

    #[test]
    fn dark_item() {
        assert!(matches!(parse_item(r#"{"files":[]}"#), Err(Error::Json(_))));
    }
}
