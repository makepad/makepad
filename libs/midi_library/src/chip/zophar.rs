//! Game music, one game at a time, from Zophar's Domain (zophar.net):
//! the person searches (the site's own search, one request), picks a game,
//! and its set of original music files (one zip: NSF, SPC, GBS or VGM
//! with playlists naming the tracks) is downloaded with the game's credits
//! from its page (two more requests), kept in the song library with the
//! notice that the music is its publisher's, and catalogued a row a track.
//! Only on the person's own pick, a second apart, never in batches: the
//! AI tools and the training corpus do not reach this.

use super::web::{get, query_text, text};
use crate::analyse::Features;
use crate::index::Entry;
use std::io::Cursor;

pub const NAME: &str = "Zophar's Domain";

pub const NOTICE: &str = "Zophar's Domain (zophar.net) keeps game music as its original files. The music is the copyright of the game's publisher; the app downloads only the game you pick, keeps it with its credits, for listening and learning, never for publishing.";

/// The consoles whose music files the importer plays.
const CONSOLES: [(&str, &str, &str); 5] = [
    ("Nintendo NES (NSF)", "nes", "NES"),
    ("Nintendo SNES (SPC)", "snes", "SNES"),
    ("Game Boy (GBS)", "gameboy", "Game Boy"),
    ("Sega Mega Drive / Genesis (VGM)", "megadrive", "Mega Drive"),
    ("Sega Master System (VGM)", "mastersystem", "Master System"),
];

/// A game in the search results.
#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    /// Its page: `/music/nintendo-snes-spc/chrono-trigger`.
    pub page: String,
    pub name: String,
    pub console: String,
    pub year: String,
    pub developer: String,
}

impl Game {
    /// The machine (index word, name), by its console or its page's path.
    pub fn machine(&self) -> (&'static str, &'static str) {
        const PAGES: [(&str, usize); 5] = [("/music/nintendo-nes-nsf/", 0), ("/music/nintendo-snes-spc/", 1), ("/music/gameboy-gbs/", 2), ("/music/sega-mega-drive-genesis/", 3), ("/music/sega-master-system-vgm/", 4)];
        let k = CONSOLES.iter().position(|c| c.0 == self.console).or_else(|| PAGES.iter().find(|p| self.page.starts_with(p.0)).map(|p| p.1));
        k.map_or(("", ""), |k| (CONSOLES[k].1, CONSOLES[k].2))
    }
}

/// The results page's games the importer can play.
pub fn parse_search(html: &str) -> Vec<Game> {
    let mut out = Vec::new();
    for row in html.split("<tr class=\"regularrow").skip(1) {
        let cell = |class: &str| row.split(&format!("<td class=\"{class}\">")).nth(1).and_then(|r| r.split("</td>").next()).map(text).unwrap_or_default();
        let Some(page) = row.split("<td class=\"name\"><a href=\"").nth(1).and_then(|r| r.split('"').next()) else { continue };
        // The name, without the alternative names under it.
        let name = row.split("<td class=\"name\"><a href=\"").nth(1).and_then(|r| r.split('>').nth(1)).and_then(|r| r.split("</a").next()).map(text).unwrap_or_else(|| cell("name"));
        let game = Game { page: page.to_string(), name, console: cell("console"), year: cell("year"), developer: cell("developer") };
        if CONSOLES.iter().any(|c| c.0 == game.console) && !out.iter().any(|g: &Game| g.page == game.page) {
            out.push(game);
        }
    }
    out
}

pub fn search(words: &str) -> Result<Vec<Game>, String> {
    let html = String::from_utf8_lossy(&get(&format!("https://www.zophar.net/music/search?search={}", query_text(words)), 4 << 20)?).into_owned();
    Ok(parse_search(&html))
}

/// What a game's page says: release year, developer, publisher, ripper,
/// and its original files' zip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Credit {
    pub year: Option<u16>,
    pub developer: String,
    pub publisher: String,
    pub ripper: String,
    pub zip: String,
}

pub fn parse_page(html: &str) -> Credit {
    let info = |name: &str| html.split(&format!("<span class=\"infoname\">{name}:</span>")).nth(1).and_then(|r| r.split("</p>").next()).map(text).unwrap_or_default();
    let year = info("Release date").split(|c: char| !c.is_ascii_digit()).filter(|w| w.len() == 4).find_map(|w| w.parse().ok());
    let zip = html.split("href=\"").filter_map(|r| r.split('"').next()).find(|u| u.contains("(EMU).zophar.zip") || u.contains("%28EMU%29.zophar.zip")).unwrap_or("").to_string();
    Credit { year, developer: info("Developer"), publisher: info("Publisher"), ripper: info("Ripped by"), zip }
}

/// A playlist line: `file::NSF,track,title,length,...` (commas in the
/// title escaped).
fn m3u_line(line: &str) -> Option<(String, u16, String, f64)> {
    let (file, rest) = line.split_once("::")?;
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for c in rest.chars() {
        match (esc, c) {
            (true, c) => {
                cur.push(c);
                esc = false;
            }
            (false, '\\') => esc = true,
            (false, ',') => fields.push(std::mem::take(&mut cur)),
            (false, c) => cur.push(c),
        }
    }
    fields.push(cur);
    let track: u16 = fields.get(1)?.trim().trim_start_matches('$').parse().ok()?;
    // "Game - Composers - Track": the track's own name is the last part.
    let title = fields.get(2).map(|t| t.rsplit(" - ").next().unwrap_or(t).trim().to_string()).unwrap_or_default();
    let secs = fields.get(3).map_or(0.0, |l| l.split(':').fold(0.0, |acc, p| acc * 60.0 + p.trim().parse::<f64>().unwrap_or(0.0)));
    Some((file.trim().to_string(), track, title, secs))
}

pub fn dir() -> std::path::PathBuf {
    super::web::dir().join("zophar")
}

fn slug(page: &str) -> String {
    page.trim_start_matches("/music/").replace('/', "_").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect()
}

/// Download the picked game's music files into the library and catalogue
/// its tracks; the new rows.
pub fn download(game: &Game) -> Result<Vec<Entry>, String> {
    let page_url = format!("https://www.zophar.net{}", game.page);
    let credit = parse_page(&String::from_utf8_lossy(&get(&page_url, 4 << 20)?));
    if credit.zip.is_empty() {
        return Err(format!("{}: the page offers no original music files", game.name));
    }
    let bytes = get(&credit.zip, 256 << 20)?;
    let name = format!("{}.zip", slug(&game.page));
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    std::fs::write(dir().join(&name), &bytes).map_err(|e| e.to_string())?;
    let entries = catalogue(game, &credit, &page_url, &name, &bytes)?;
    let path = super::web::dir().join("library.tsv");
    let mut all = std::fs::read_to_string(&path).map(|t| crate::index::from_tsv(&t)).unwrap_or_default();
    all.retain(|e| !entries.iter().any(|n| n.path == e.path));
    all.extend(entries.iter().cloned());
    crate::index::write(&all, &path)?;
    Ok(entries)
}

fn files(zip: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut c = Cursor::new(zip);
    let dir = makepad_zip_file::zip_read_central_directory(&mut c).map_err(|e| format!("zip: {e:?}"))?;
    let mut out = Vec::new();
    for h in &dir.file_headers {
        if h.file_name.ends_with('/') {
            continue;
        }
        if let Ok(data) = h.extract(&mut c) {
            out.push((h.file_name.clone(), data));
        }
    }
    Ok(out)
}

/// A downloaded game's tracks as catalogue rows (`zophar/<zip>!<file>#n`).
fn catalogue(game: &Game, credit: &Credit, page: &str, zip_name: &str, zip: &[u8]) -> Result<Vec<Entry>, String> {
    let (platform, machine) = game.machine();
    let files = files(zip)?;
    // The playlists' tracks: (file, track, title, seconds).
    let mut listed: Vec<(String, u16, String, f64)> = Vec::new();
    for (name, data) in files.iter().filter(|(n, _)| n.to_ascii_lowercase().ends_with(".m3u")) {
        let _ = name;
        for line in String::from_utf8_lossy(data).lines().filter(|l| !l.starts_with('#')) {
            if let Some(t) = m3u_line(line) {
                if !listed.iter().any(|x| x.0 == t.0 && x.1 == t.1) {
                    listed.push(t);
                }
            }
        }
    }
    let credit_text = format!("{NAME} {page} | music © {} | {}{}", if credit.publisher.is_empty() { game.developer.as_str() } else { credit.publisher.as_str() }, if credit.developer.is_empty() { String::new() } else { format!("developed by {}", credit.developer) }, if credit.ripper.is_empty() { String::new() } else { format!(", ripped by {}", credit.ripper) });
    let mut out = Vec::new();
    for (inner, data) in &files {
        if !super::import::is_supported(inner) {
            continue;
        }
        let ext = inner.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        // A multi-track file (NSF, GBS) is a row per listed track; a file
        // per track (SPC, VGM) is one row.
        let mut tracks: Vec<(u16, String, f64)> = listed.iter().filter(|t| t.0 == *inner || inner.ends_with(&t.0)).map(|t| (t.1, t.2.clone(), t.3)).collect();
        tracks.sort_by_key(|t| t.0);
        let multi = matches!(ext.as_str(), "nsf" | "nsfe" | "gbs");
        if tracks.is_empty() || !multi {
            let stem = inner.rsplit('/').next().unwrap_or(inner);
            let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
            let title = tracks.first().map_or(stem.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ' || c == '-').to_string(), |t| t.1.clone());
            tracks = vec![(0, title, tracks.first().map_or(0.0, |t| t.2))];
        }
        for (track, title, seconds) in tracks.into_iter().take(80) {
            let about = super::import::About { title: title.clone(), seconds: if seconds > 0.0 { seconds.min(300.0) } else { 0.0 }, song: track, ..Default::default() };
            let quick = super::import::About { seconds: 30.0, ..about.clone() };
            let Ok(imported) = super::import::import(data, inner, &quick) else { continue };
            let a = &imported.arrangement;
            let mut roles: Vec<crate::gm::Role> = a.parts.iter().map(|p| p.role).collect();
            roles.sort();
            roles.dedup();
            let notes: usize = a.parts.iter().map(|p| p.notes.len()).sum();
            let path = format!("zophar/{zip_name}!{inner}{}", if track > 0 { format!("#{track}") } else { String::new() });
            let md5 = crate::md5::hex(path.as_bytes());
            let mut tags: Vec<(String, f32)> = vec![(platform.to_string(), 100.0), (ext.clone(), 100.0), ("game".into(), 100.0), ("chiptune".into(), 80.0)];
            for w in format!("{} {}", credit.developer, credit.publisher).split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3) {
                tags.push((crate::meta::norm(w), 50.0));
            }
            out.push(Entry {
                id: md5[..8].to_string(),
                artist: if a.artist.is_empty() { if credit.developer.is_empty() { game.developer.clone() } else { credit.developer.clone() } } else { a.artist.clone() },
                title: format!("{}: {title}", game.name),
                path,
                md5,
                tid: credit_text.clone(),
                matched: 0.0,
                year: credit.year.or_else(|| game.year.parse().ok()),
                genres: vec!["game".into(), machine.to_lowercase()],
                tags,
                features: Features {
                    bpm: a.bpm,
                    key: a.key.clone(),
                    sig: "4/4".into(),
                    parts: a.parts.len(),
                    programs: Vec::new(),
                    roles,
                    seconds: if seconds > 0.0 { seconds } else { 150.0 },
                    notes,
                    melody: a.parts.iter().any(|p| p.melody),
                    quality: (40 + a.parts.len().min(8) * 5 + (notes / 20).min(20)).min(100) as u8,
                },
                format: ext.clone(),
                platform: platform.into(),
                game: game.name.clone(),
                archive: "web".into(),
            });
        }
    }
    if out.is_empty() {
        return Err(format!("{}: no music file in the set this importer reads", game.name));
    }
    Ok(out)
}

/// A downloaded track's file (the zip's member, without the track).
pub fn bytes(entry: &Entry) -> Result<Vec<u8>, String> {
    let rest = entry.path.strip_prefix("zophar/").ok_or("not a Zophar's Domain track")?;
    let (zip_name, inner) = rest.split_once('!').ok_or("a Zophar's Domain path names its zip and file")?;
    let inner = inner.rsplit_once('#').map_or(inner, |(f, _)| f);
    let zip = std::fs::read(dir().join(zip_name)).map_err(|e| format!("{zip_name}: {e}"))?;
    files(&zip)?.into_iter().find(|(n, _)| n == inner).map(|(_, d)| d).ok_or_else(|| format!("{inner} is not in {zip_name}"))
}

/// The file name inside the zip, for the importer.
pub fn member(path: &str) -> &str {
    let inner = path.split_once('!').map_or(path, |(_, f)| f);
    inner.rsplit_once('#').map_or(inner, |(f, _)| f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zophar_pages_and_playlists_read() {
        let search = r#"<table><tr class="regularrow_image">
<td class="image"><a href="/music/nintendo-snes-spc/chrono-trigger"><img></a></td>
<td class="name"><a href="/music/nintendo-snes-spc/chrono-trigger">Chrono Trigger</a></td>
<td class="console">Nintendo SNES (SPC)</td><td class="year">1995</td><td class="developer">Square</td></tr>
<tr class="regularrow_image"><td class="name"><a href="/music/playstation-psf/chrono-trigger">Chrono Trigger</a></td>
<td class="console">Playstation (PSF)</td><td class="year">1999</td><td class="developer">Tose</td></tr></table>"#;
        let games = parse_search(search);
        assert_eq!(games, vec![Game { page: "/music/nintendo-snes-spc/chrono-trigger".into(), name: "Chrono Trigger".into(), console: "Nintendo SNES (SPC)".into(), year: "1995".into(), developer: "Square".into() }]);
        assert_eq!(games[0].machine(), ("snes", "SNES"));
        let page = r#"<p><span class="infoname">Release date:</span> <span class="infodata">Mar 11th, <a>1995</a></span></p>
<p><span class="infoname">Developer:</span> <span class="infodata"><a>Square</a></span></p><p><span class="infoname">Publisher:</span> <span class="infodata"><a>Square</a></span></p><p><span class="infoname">Ripped by:</span> <span class="infodata">YK</span></p>
<a href="https://fi.zophar.net/soundfiles/x/ct/Chrono%20Trigger%20%28EMU%29.zophar.zip">"#;
        let c = parse_page(page);
        assert_eq!((c.year, c.developer.as_str(), c.publisher.as_str(), c.ripper.as_str()), (Some(1995), "Square", "Square", "YK"));
        assert!(c.zip.ends_with("%28EMU%29.zophar.zip"));
        let l = m3u_line(r"Mega Man 2.nsf::NSF,9,Mega Man II - Ogeretsu Kun\, Manami Matsumae - Quick Man,0:01:16,,0:00:06").unwrap();
        assert_eq!(l, ("Mega Man 2.nsf".into(), 9, "Quick Man".into(), 76.0));
        assert_eq!(member("zophar/x.zip!Mega Man 2.nsf#9"), "Mega Man 2.nsf");
    }
}
