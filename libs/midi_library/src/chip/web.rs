//! Songs found and downloaded one at a time from the archives' own sites:
//! a search sends the person's words to the site's search (one request),
//! picking a result fetches its page (the credits and the licence) and its
//! file (two more). Nothing is crawled or mirrored: only what the person
//! picked is kept, with its credit, in the song library (`datasets/web/`).
//! Requests to one site are at least a second apart.
//!
//! Sources:
//! - The Mod Archive (modarchive.org): MOD, S3M, XM and IT modules. Its
//!   terms: the composers keep their copyright unless a module carries a
//!   public-domain or Creative Commons licence; a module may be shared
//!   unmodified, not bundled into an application; the site is for
//!   personal, non-commercial use.

use crate::analyse::Features;
use crate::index::Entry;
use makepad_model_install as models;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A site the person can search.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub id: &'static str,
    pub name: &'static str,
    pub site: &'static str,
    /// What the person agrees to before a download, shown with it.
    pub terms: &'static str,
}

pub const MODARCHIVE: Source = Source {
    id: "modarchive",
    name: "The Mod Archive",
    site: "https://modarchive.org",
    terms: "The Mod Archive (modarchive.org): the composers keep their copyright unless the module carries a public-domain or Creative Commons licence (shown with it). A module may be shared in its original, unmodified form, never bundled into an application or game; the site is for personal, non-commercial use. The app downloads only the module you pick, from the archive itself, and keeps it with its credit.",
};

pub const SOURCES: [Source; 1] = [MODARCHIVE];

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub source: &'static str,
    /// The site's id for it.
    pub id: String,
    pub title: String,
    pub file: String,
    /// "MOD", "XM" ...
    pub format: String,
}

/// What the site says about a song.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Credit {
    pub title: String,
    pub artists: Vec<String>,
    pub license: String,
    pub genre: String,
    pub page: String,
    pub md5: String,
}

static LAST: Mutex<Option<Instant>> = Mutex::new(None);

/// A GET, at least a second after the last one.
pub(crate) fn get(url: &str, max: usize) -> Result<Vec<u8>, String> {
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let wait = Duration::from_secs(1).saturating_sub(t.elapsed());
            std::thread::sleep(wait);
        }
        *last = Some(Instant::now());
    }
    let req = makepad_ai_hub::http_client::HttpClientRequest::get(url);
    let resp = makepad_ai_hub::http_client::http_fetch(&req).map_err(|e| format!("{url}: {e:?}"))?;
    if resp.status != 200 {
        return Err(format!("{url}: HTTP {}", resp.status));
    }
    resp.read_body_to_vec(max).map_err(|e| format!("{url}: {e:?}"))
}

/// The person's words as a query string.
pub(crate) fn query_text(words: &str) -> String {
    let mut out = String::new();
    for b in words.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// HTML text: tags out, the common entities decoded, spaces collapsed.
pub(crate) fn text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&amp;", "&").replace("&quot;", "\"").replace("&#039;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The search results in The Mod Archive's result page.
pub fn parse_modarchive_search(html: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for row in html.split("<tr>").skip(1) {
        let Some(at) = row.find("downloads.php?moduleid=") else { continue };
        let rest = &row[at + "downloads.php?moduleid=".len()..];
        let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let file = rest[id.len()..].strip_prefix('#').map(|r| r.split('"').next().unwrap_or("").to_string()).unwrap_or_default();
        let format = row.split("format-icon\">").nth(1).and_then(|r| r.split('<').next()).unwrap_or("").trim().to_string();
        let title = row.split("<span class=\"module-listing\">").nth(1).and_then(|r| r.split("</span>").next()).map(text).unwrap_or_default();
        if !id.is_empty() && !hits.iter().any(|h: &Hit| h.id == id) {
            hits.push(Hit { source: MODARCHIVE.id, id, title: if title.is_empty() { file.clone() } else { title }, file, format });
        }
    }
    hits
}

/// The credits on The Mod Archive's page for a module.
pub fn parse_modarchive_page(html: &str, id: &str) -> Credit {
    let field = |label: &str| html.split(&format!("<li class=\"stats\">{label}: ")).nth(1).and_then(|r| r.split("</li>").next()).map(text).unwrap_or_default();
    let title = html.split("<h1>").nth(1).and_then(|r| r.split("<span").next()).map(text).unwrap_or_default();
    let license = html.split("&nbsp;License</h2>").nth(1).and_then(|r| r.split("</li>").next()).map(text).unwrap_or_default();
    let mut artists = Vec::new();
    if let Some(block) = html.split("Registered Artist(s):").nth(1).and_then(|r| r.split("</ul>").next()) {
        for part in block.split("href=\"member.php?").skip(1) {
            if let Some(name) = part.split('>').nth(1).and_then(|r| r.split('<').next()) {
                let name = text(name);
                if !name.is_empty() {
                    artists.push(name);
                }
            }
        }
    }
    let genre = field("Genre");
    Credit {
        title,
        artists,
        license,
        genre: if genre == "n/a" { String::new() } else { genre },
        page: format!("https://modarchive.org/index.php?request=view_by_moduleid&query={id}"),
        md5: field("MD5"),
    }
}

/// Search a source for the person's words.
pub fn search(source: &Source, words: &str) -> Result<Vec<Hit>, String> {
    match source.id {
        "modarchive" => {
            let url = format!("https://modarchive.org/index.php?request=search&query={}&submit=Find&search_type=filename_or_songtitle", query_text(words));
            let html = String::from_utf8_lossy(&get(&url, 4 << 20)?).into_owned();
            Ok(parse_modarchive_search(&html))
        }
        other => Err(format!("no source `{other}`")),
    }
}

pub fn dir() -> PathBuf {
    let mut dir = models::weights_root();
    dir.extend(["datasets", "web"]);
    dir
}

/// Download a picked result into the song library: its page (credits,
/// licence), its file, and a catalogue row.
pub fn download(hit: &Hit) -> Result<Entry, String> {
    let source = SOURCES.iter().find(|s| s.id == hit.source).ok_or("no such source")?;
    let (bytes, credit) = match source.id {
        "modarchive" => {
            let page = String::from_utf8_lossy(&get(&format!("https://modarchive.org/index.php?request=view_by_moduleid&query={}", hit.id), 4 << 20)?).into_owned();
            let credit = parse_modarchive_page(&page, &hit.id);
            let bytes = get(&format!("https://api.modarchive.org/downloads.php?moduleid={}", hit.id), 64 << 20)?;
            (bytes, credit)
        }
        other => return Err(format!("no source `{other}`")),
    };
    let md5 = crate::md5::hex(&bytes);
    if !credit.md5.is_empty() && !credit.md5.eq_ignore_ascii_case(&md5) {
        return Err(format!("{}: the file is not the one the archive lists (MD5 {md5}, listed {})", hit.file, credit.md5));
    }
    let safe: String = hit.file.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    let path = format!("{}/{}_{}", source.id, hit.id, safe);
    let file = dir().join(&path);
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&file, &bytes).map_err(|e| e.to_string())?;
    let entry = entry_for(source, hit, &credit, &path, &bytes, &md5)?;
    let catalogue = dir().join("library.tsv");
    let mut entries = std::fs::read_to_string(&catalogue).map(|t| crate::index::from_tsv(&t)).unwrap_or_default();
    entries.retain(|e| e.path != entry.path);
    entries.push(entry.clone());
    crate::index::write(&entries, &catalogue)?;
    Ok(entry)
}

fn entry_for(source: &Source, hit: &Hit, credit: &Credit, path: &str, bytes: &[u8], md5: &str) -> Result<Entry, String> {
    let imported = crate::chip::import::import(bytes, &hit.file, &crate::chip::import::About::default())?;
    let a = &imported.arrangement;
    let mut roles: Vec<crate::gm::Role> = a.parts.iter().map(|p| p.role).collect();
    roles.sort();
    roles.dedup();
    let notes: usize = a.parts.iter().map(|p| p.notes.len()).sum();
    let beats = a.beats();
    let seconds = beats * 60.0 / a.bpm.max(1.0);
    let ext = hit.file.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let module = crate::chip::tracker::read(bytes, &hit.file).ok();
    let platform = match (ext.as_str(), module.as_ref().map(|m| m.channels)) {
        ("mod", Some(4)) => "amiga",
        _ => "pc",
    };
    let mut tags: Vec<(String, f32)> = vec![(ext.clone(), 100.0), (platform.to_string(), 100.0), ("tracker".into(), 100.0)];
    for w in credit.genre.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3) {
        tags.push((crate::meta::norm(w), 80.0));
    }
    Ok(Entry {
        id: md5[..8].to_string(),
        artist: if credit.artists.is_empty() { "Unknown".into() } else { credit.artists.join(", ") },
        title: if credit.title.is_empty() { hit.title.clone() } else { credit.title.clone() },
        path: path.to_string(),
        md5: md5.to_string(),
        // The credit: where it came from and under what licence.
        tid: format!("{} | {} | {}", credit.page, if credit.license.is_empty() { "the composer's copyright" } else { &credit.license }, source.name),
        matched: 0.0,
        year: None,
        genres: vec!["tracker".into()],
        tags,
        features: Features {
            bpm: a.bpm,
            key: a.key.clone(),
            sig: "4/4".into(),
            parts: a.parts.len(),
            programs: Vec::new(),
            roles,
            seconds,
            notes,
            melody: a.parts.iter().any(|p| p.melody),
            quality: (40 + a.parts.len().min(10) * 4 + (notes / 100).min(20)).min(100) as u8,
        },
        format: ext,
        platform: platform.into(),
        game: String::new(),
        archive: "web".into(),
    })
}

/// A downloaded song's file.
pub fn bytes(entry: &Entry) -> Result<Vec<u8>, String> {
    if entry.path.starts_with("zophar/") {
        return super::zophar::bytes(entry);
    }
    std::fs::read(dir().join(&entry.path)).map_err(|e| format!("{}: {e}", entry.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modarchive_pages_read() {
        let search = r#"<table><tr><td>x</td></tr><tr>
<td valign="top" width="75"><a href="https://api.modarchive.org/downloads.php?moduleid=57925#space_debris.mod" title="Download">x</a>
<span class="format-icon">MOD</span></td>
<td><a class="standard-link" href="index.php?request=view_by_moduleid&amp;query=57925" title="space_debris">space_debris.mod</a></td>
<td><span class="module-listing">
space &amp; debris
</span></td></tr></table>"#;
        let hits = parse_modarchive_search(search);
        assert_eq!(hits, vec![Hit { source: "modarchive", id: "57925".into(), title: "space & debris".into(), file: "space_debris.mod".into(), format: "MOD".into() }]);
        let page = r#"<h1>Point of Departure <span class="module-sub-header">(tuna.it)</span></h1>
<li class="stats">MD5: abc123</li><li class="stats">Genre: Trance - General</li>
<h2><img alt="License">&nbsp;License</h2><ul><li class="stats">Licensed under the <a href="x">Creative Commons BY-NC</a></li></ul>
<h2>Registered Artist(s):</h2><ul class='nolist'><li><img src="y">&nbsp;<a class="standard-link" href="member.php?96606">Tuna</a> </li></ul>"#;
        let c = parse_modarchive_page(page, "211516");
        assert_eq!((c.title.as_str(), c.artists.clone(), c.license.as_str(), c.genre.as_str(), c.md5.as_str()), ("Point of Departure", vec!["Tuna".to_string()], "Licensed under the Creative Commons BY-NC", "Trance - General", "abc123"));
        assert_eq!(query_text("Space Debris!"), "Space+Debris%21");
    }
}
