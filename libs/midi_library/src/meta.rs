//! The metadata the songs are categorised by, each read from its file as
//! downloaded: LMD's match scores (MSD track -> MIDI md5 -> confidence),
//! the MSD years, the tagtraum genres and the Last.fm tags.

use std::collections::HashMap;
use std::path::Path;

/// Every MSD track a MIDI file (by md5) matches, best first.
pub fn matches(json: &[u8]) -> Result<HashMap<String, Vec<(String, f32)>>, String> {
    let value = makepad_strict_json::parse(json).map_err(|e| format!("match scores: {e}"))?;
    let makepad_strict_json::Value::Obj(tracks) = value else { return Err("match scores: not an object".into()) };
    let mut out: HashMap<String, Vec<(String, f32)>> = HashMap::new();
    for (tid, files) in tracks {
        let makepad_strict_json::Value::Obj(files) = files else { continue };
        for (md5, score) in files {
            let score = match score {
                makepad_strict_json::Value::F64(v) => v as f32,
                makepad_strict_json::Value::Int(v) => v as f32,
                _ => continue,
            };
            out.entry(md5).or_default().push((tid.clone(), score));
        }
    }
    for list in out.values_mut() {
        list.sort_by(|a, b| b.1.total_cmp(&a.1));
    }
    Ok(out)
}

/// Release years by MSD track (`year<SEP>track<SEP>artist<SEP>title`).
pub fn years(text: &str) -> HashMap<String, u16> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split("<SEP>");
            let year = fields.next()?.trim().parse::<u16>().ok()?;
            let tid = fields.next()?.trim();
            (year > 1900).then(|| (tid.to_string(), year))
        })
        .collect()
}

/// The MSD's tracks by name: the tracks of each (artist, title),
/// normalised ([`norm`]), and each track's artist.
#[derive(Default)]
pub struct Tracks {
    pub by_name: HashMap<(String, String), Vec<String>>,
    pub artist_of: HashMap<String, String>,
}

/// `unique_tracks.txt` (`track<SEP>song<SEP>artist<SEP>title`).
pub fn tracks(text: &str) -> Tracks {
    let mut out = Tracks::default();
    for line in text.lines() {
        let mut fields = line.split("<SEP>");
        let (Some(tid), Some(_), Some(artist), Some(title)) = (fields.next(), fields.next(), fields.next(), fields.next()) else { continue };
        let (tid, artist) = (tid.trim().to_string(), norm(artist));
        out.by_name.entry((artist.clone(), norm(title))).or_default().push(tid.clone());
        out.artist_of.insert(tid, artist);
    }
    out
}

/// The first year each (artist, title), normalised, was released in.
pub fn first_years(text: &str) -> HashMap<(String, String), u16> {
    let mut out: HashMap<(String, String), u16> = HashMap::new();
    for line in text.lines() {
        let mut fields = line.split("<SEP>");
        let (Some(year), Some(_), Some(artist), Some(title)) = (fields.next(), fields.next(), fields.next(), fields.next()) else { continue };
        let Ok(year) = year.trim().parse::<u16>() else { continue };
        if year > 1900 {
            let y = out.entry((norm(artist), norm(title))).or_insert(year);
            *y = (*y).min(year);
        }
    }
    out
}

/// Genres by MSD track from a tagtraum `.cls` (`track<TAB>genre[<TAB>genre]`,
/// `#` comments).
pub fn genres(text: &str) -> HashMap<String, Vec<String>> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let tid = fields.next()?.trim();
            let genres: Vec<String> = fields.map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
            (!tid.is_empty() && !genres.is_empty()).then(|| (tid.to_string(), genres))
        })
        .collect()
}

/// The `.cls` inside tagtraum's zip.
pub fn genres_zip(path: &Path) -> Result<HashMap<String, Vec<String>>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut cursor = std::io::Cursor::new(bytes);
    let dir = makepad_zip_file::zip_read_central_directory(&mut cursor).map_err(|e| format!("genres zip: {e:?}"))?;
    let entry = dir.file_headers.iter().find(|h| h.file_name.ends_with(".cls")).ok_or("genres zip: no .cls inside")?;
    let text = entry.extract(&mut cursor).map_err(|e| format!("genres zip: {e:?}"))?;
    Ok(genres(&String::from_utf8_lossy(&text)))
}

/// A tag as it is compared: lowercase letters and digits only ("Synth-Pop"
/// and "synth pop" are "synthpop").
pub fn norm(tag: &str) -> String {
    tag.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The Last.fm tags of the tracks `wanted` (by MSD track id), strongest
/// first, normalised ([`norm`]), each with its weight (0..100).
pub fn tags(db_path: &Path, wanted: &std::collections::HashSet<String>) -> Result<HashMap<String, Vec<(String, f32)>>, String> {
    use makepad_sqlite::{Database, Value};
    let mut db = Database::open(db_path).map_err(|e| format!("{}: {e}", db_path.display()))?;
    db.limits_mut().max_steps = u64::MAX;
    db.limits_mut().max_rows = usize::MAX;
    // tids.rowid -> track id, for the tracks wanted.
    let mut by_row: HashMap<i64, String> = HashMap::new();
    let tids = db.prepare("SELECT rowid, tid FROM tids").map_err(|e| e.to_string())?;
    tids.for_each(&mut db, &[], |row| {
        if let (Some(id), Some(tid)) = (row.first().and_then(Value::as_integer), row.get(1).and_then(Value::as_text)) {
            if wanted.contains(tid) {
                by_row.insert(id, tid.to_string());
            }
        }
        Ok(true)
    }).map_err(|e| e.to_string())?;
    let mut pairs: HashMap<i64, Vec<(i64, f32)>> = HashMap::new();
    let links = db.prepare("SELECT tid, tag, val FROM tid_tag").map_err(|e| e.to_string())?;
    links.for_each(&mut db, &[], |row| {
        if let (Some(tid), Some(tag)) = (row.first().and_then(Value::as_integer), row.get(1).and_then(Value::as_integer)) {
            if by_row.contains_key(&tid) {
                let val = row.get(2).and_then(Value::as_real).unwrap_or(0.0) as f32;
                pairs.entry(tid).or_default().push((tag, val));
            }
        }
        Ok(true)
    }).map_err(|e| e.to_string())?;
    let needed: std::collections::HashSet<i64> = pairs.values().flatten().map(|p| p.0).collect();
    let mut names: HashMap<i64, String> = HashMap::new();
    let tags = db.prepare("SELECT rowid, tag FROM tags").map_err(|e| e.to_string())?;
    tags.for_each(&mut db, &[], |row| {
        if let (Some(id), Some(tag)) = (row.first().and_then(Value::as_integer), row.get(1).and_then(Value::as_text)) {
            if needed.contains(&id) {
                names.insert(id, norm(tag));
            }
        }
        Ok(true)
    }).map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for (row, list) in pairs {
        let Some(tid) = by_row.get(&row) else { continue };
        let mut tags: Vec<(String, f32)> = list.into_iter().filter_map(|(tag, val)| Some((names.get(&tag)?.clone(), val))).filter(|(t, _)| !t.is_empty()).collect();
        tags.sort_by(|a, b| b.1.total_cmp(&a.1));
        tags.dedup_by(|b, a| a.0 == b.0);
        out.insert(tid.clone(), tags);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_format() {
        let m = matches(br#"{"TRA": {"aa": 0.5, "bb": 0.9}, "TRB": {"aa": 0.7}}"#).unwrap();
        assert_eq!(m["aa"], vec![("TRB".to_string(), 0.7), ("TRA".to_string(), 0.5)]);
        let y = years("1985<SEP>TRA<SEP>Band<SEP>Hit\nnope\n");
        assert_eq!(y["TRA"], 1985);
        let t = tracks("TRA<SEP>SOA<SEP>The Band<SEP>Hit Song\nTRB<SEP>SOB<SEP>The Band<SEP>Hit Song\n");
        assert_eq!(t.by_name[&("theband".to_string(), "hitsong".to_string())], vec!["TRA".to_string(), "TRB".to_string()]);
        assert_eq!(t.artist_of["TRB"], "theband");
        let f = first_years("1990<SEP>TRA<SEP>The Band<SEP>Hit Song\n1984<SEP>TRB<SEP>The Band<SEP>Hit Song\n");
        assert_eq!(f[&("theband".to_string(), "hitsong".to_string())], 1984);
        let g = genres("# comment\nTRA\tPop_Rock\tElectronic\nTRB\tRap\n");
        assert_eq!(g["TRA"], vec!["Pop_Rock".to_string(), "Electronic".to_string()]);
        assert_eq!(norm("Synth-Pop"), "synthpop");
        assert_eq!(norm("80s"), "80s");
    }
}
