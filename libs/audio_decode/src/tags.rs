//! Container metadata, in one shape for both codecs. Text is whatever the
//! file claimed, sanitised to valid UTF-8 with control characters dropped and
//! each field length-bounded — these strings end up in a UI list.

/// Longest tag value kept. Titles are short; anything longer is a lyrics or
/// cover-art dump we do not want in a library row.
pub const MAX_TAG_LEN: usize = 512;
/// Most key/value pairs kept from one file.
pub const MAX_TAGS: usize = 128;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    /// Everything found, in file order, keys upper-cased (`TIT2`, `TITLE`,
    /// `REPLAYGAIN_TRACK_GAIN`, ...). Bounded by [`MAX_TAGS`].
    pub all: Vec<(String, String)>,
}

impl Tags {
    /// Record one pair, routing the well-known keys into the named fields.
    /// Later duplicates never overwrite an earlier value (the first TIT2 in a
    /// file is the real one; appended junk is not).
    pub fn push(&mut self, key: &str, value: &str) {
        let value = sanitize(value);
        if value.is_empty() {
            return;
        }
        let key = key.trim().to_ascii_uppercase();
        let slot = match key.as_str() {
            "TIT2" | "TT2" | "TITLE" => Some(&mut self.title),
            "TPE1" | "TP1" | "ARTIST" | "ALBUMARTIST" => Some(&mut self.artist),
            "TALB" | "TAL" | "ALBUM" => Some(&mut self.album),
            "TRCK" | "TRK" | "TRACKNUMBER" => Some(&mut self.track),
            "TCON" | "TCO" | "GENRE" => Some(&mut self.genre),
            "TYER" | "TDRC" | "TYE" | "DATE" | "YEAR" => Some(&mut self.date),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.is_none() {
                *slot = Some(value.clone());
            }
        }
        if self.all.len() < MAX_TAGS {
            self.all.push((key, value));
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_uppercase();
        self.all.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    /// The tempo the file claims, beats per minute: ID3 `TBPM` (`TBP` in
    /// v2.2) or a Vorbis/FLAC `BPM` / `TEMPO` comment. Leading number only
    /// (`"128"`, `"127.98"`, `"174 bpm"`); `None` when absent or outside
    /// 20..=400.
    pub fn bpm(&self) -> Option<f64> {
        ["TBPM", "TBP", "BPM", "TEMPO"].iter().find_map(|key| {
            let value = self.get(key)?.trim();
            let end = value
                .char_indices()
                .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == ','))
                .map(|(i, _)| i)
                .unwrap_or(value.len());
            let bpm: f64 = value[..end].replace(',', ".").parse().ok()?;
            (bpm.is_finite() && (20.0..=400.0).contains(&bpm)).then_some(bpm)
        })
    }
}

/// Trim, drop control characters (including the NULs ID3 pads with), and
/// bound the length at a character boundary.
pub fn sanitize(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if out.len() >= MAX_TAG_LEN {
            break;
        }
        if c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_keys_route_to_fields() {
        let mut t = Tags::default();
        t.push("TIT2", "Song\u{0}");
        t.push("TPE1", "  Artist  ");
        t.push("tit2", "Later duplicate");
        assert_eq!(t.title.as_deref(), Some("Song"));
        assert_eq!(t.artist.as_deref(), Some("Artist"));
        assert_eq!(t.all.len(), 3);
        assert_eq!(t.get("TIT2"), Some("Song"));
    }

    #[test]
    fn bpm_reads_id3_and_vorbis_spellings() {
        let mut t = Tags::default();
        assert_eq!(t.bpm(), None);
        t.push("TBPM", "174");
        assert_eq!(t.bpm(), Some(174.0));
        let mut t = Tags::default();
        t.push("BPM", "127,98 bpm");
        assert_eq!(t.bpm(), Some(127.98));
        let mut t = Tags::default();
        t.push("TBPM", "0");
        t.push("TEMPO", "fast");
        assert_eq!(t.bpm(), None);
    }

    #[test]
    fn empty_and_oversized_values_are_bounded() {
        let mut t = Tags::default();
        t.push("TIT2", "\u{0}\u{0}");
        assert!(t.title.is_none());
        t.push("COMM", &"x".repeat(4096));
        assert_eq!(t.all[0].1.len(), MAX_TAG_LEN);
        for i in 0..MAX_TAGS * 2 {
            t.push("TXXX", &format!("v{i}"));
        }
        assert_eq!(t.all.len(), MAX_TAGS);
    }
}
