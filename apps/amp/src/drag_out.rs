//! Dragging songs out of Amp into other apps (a chat, a mail, the file
//! manager). The person's own files go as they are: MP3, M4A, FLAC, WAV.
//! Ogg Vorbis (the AI songs and many library files) is not something most
//! apps take, so it goes as an MP3 made for the drop: a file promise whose
//! producer starts with the drag, decodes the Ogg, encodes it with the
//! in-tree MP3 encoder and carries title, artist, album, genre, date and the
//! lyrics over as ID3 tags. Streamed tracks are not the person's files and
//! are refused.

use crate::index::{Index, LOCAL_FILE};
use makepad_audio_encode::mp3::{encode_mp3_tagged, id3::Id3Tags, Mp3Options};
use makepad_widgets::*;
use std::path::Path;

/// The file a library or queue URI drags as: a local file, or the local
/// copy of a system track. A track that only streams (or plays from the
/// offline cache of a stream) has none.
pub fn draggable_path(index: &Index, uri: &str) -> Option<String> {
    if let Some(path) = uri.strip_prefix(LOCAL_FILE) {
        return Path::new(path).is_file().then(|| path.to_string());
    }
    let track = index.track(uri)?;
    let copy = track.local_copy?;
    index.tracks.get(copy as usize)?.local_path().map(str::to_string)
}

/// The refusal shown when nothing of a drag is a local file.
pub const NOT_LOCAL: &str = "Not a local file: streamed songs can't be dragged out";

/// Start dragging `paths` out of `window` (`None` entries are songs that are
/// not local files). Returns a message for the person when some or all of
/// them could not go.
pub fn start(cx: &mut Cx, window: WindowId, paths: Vec<Option<String>>) -> Option<String> {
    let total = paths.len();
    let local: Vec<String> = paths.into_iter().flatten().collect();
    if local.is_empty() {
        return (total > 0).then(|| NOT_LOCAL.to_string());
    }
    let left_out = total - local.len();
    let items = local.into_iter().map(item).collect();
    cx.start_external_dragging(window, DragOut { items, image: None });
    (left_out > 0).then(|| format!("{left_out} streamed song{} left out: only local files can be dragged", if left_out == 1 { "" } else { "s" }))
}

fn item(path: String) -> DragOutItem {
    let ext = Path::new(&path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "ogg" | "oga") || is_opus(&path) {
        // An Opus stream has no decoder here: it goes as it is.
        return DragOutItem::File(path);
    }
    DragOutItem::Promise(FilePromise {
        name: format!("{}.mp3", display_name(&path)),
        mime: "audio/mpeg".into(),
        produce: Box::new(move |dest| transcode(Path::new(&path), dest)),
    })
}

/// An Ogg file whose first packet is Opus rather than Vorbis.
fn is_opus(path: &str) -> bool {
    use std::io::Read;
    let mut head = [0u8; 64];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    head[..n].windows(8).any(|w| w == b"OpusHead")
}

/// The name the MP3 gets: the file's own, without the AI songs' trailing
/// " - <unix time>" and with what a file name cannot hold replaced.
pub fn display_name(path: &str) -> String {
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = match stem.rsplit_once(" - ") {
        Some((title, stamp)) if !title.is_empty() && stamp.len() >= 9 && stamp.bytes().all(|b| b.is_ascii_digit()) => title.to_string(),
        _ => stem,
    };
    let clean: String = stem.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    let clean = clean.trim().to_string();
    if clean.is_empty() { "song".into() } else { clean }
}

/// Ogg Vorbis at `src` to a tagged 192 kbit/s MP3 at `dest`.
pub fn transcode(src: &Path, dest: &Path) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("reading {}: {e}", src.display()))?;
    let audio = makepad_audio_decode::decode_any(&bytes).map_err(|e| format!("decoding {}: {e:?}", src.display()))?;
    let comments = makepad_audio_decode::vorbis::read_comments_verbatim(&bytes).unwrap_or_default();
    let tags = id3_from_comments(&comments);
    let mp3 = encode_mp3_tagged(audio.rate, audio.channels, &audio.pcm_interleaved_f32, &tags, &Mp3Options::default()).map_err(|e| e.to_string())?;
    std::fs::write(dest, mp3).map_err(|e| format!("writing {}: {e}", dest.display()))
}

/// The ID3 fields of a file's Vorbis comments (the first of each key).
pub fn id3_from_comments(comments: &[(String, String)]) -> Id3Tags {
    let get = |keys: &[&str]| {
        keys.iter().find_map(|k| comments.iter().find(|(key, v)| key == k && !v.trim().is_empty()).map(|(_, v)| v.clone()))
    };
    Id3Tags {
        title: get(&["TITLE"]),
        artist: get(&["ARTIST", "ALBUMARTIST"]),
        album: get(&["ALBUM"]),
        genre: get(&["GENRE"]),
        year: get(&["DATE", "YEAR"]).map(|d| d.chars().take(4).collect()),
        track: get(&["TRACKNUMBER"]),
        comment: get(&["COMMENT", "DESCRIPTION"]),
        lyrics: get(&["LYRICS", "UNSYNCEDLYRICS"]),
        lyrics_language: get(&["LANGUAGE"]).filter(|l| l.len() == 3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_drop_the_ai_time_stamp() {
        assert_eq!(display_name("/m/AI music/Night Drive - 1792872851.ogg"), "Night Drive");
        assert_eq!(display_name("/m/Band - Song.ogg"), "Band - Song");
        assert_eq!(display_name("/m/a: b?.ogg"), "a_ b_");
    }

    #[test]
    fn an_ai_song_transcodes_with_its_tags_and_lyrics() {
        // a tagged Ogg the way ai_music.rs files a song
        let rate = 44_100;
        let pcm: Vec<f32> = (0..rate * 2).flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3;
            [v, v]
        }).collect();
        let lyrics = "[Verse]\nFirst line\nSecond line";
        let comments = vec![
            ("TITLE".to_string(), "Night Drive".to_string()),
            ("ARTIST".to_string(), "AI · model".to_string()),
            ("ALBUM".to_string(), "AI music".to_string()),
            ("LYRICS".to_string(), lyrics.to_string()),
        ];
        let opts = makepad_audio_encode::EncodeOptions { tags: comments.clone(), ..Default::default() };
        let ogg = makepad_audio_encode::encode_vorbis(rate as u32, 2, &pcm, &opts).unwrap();
        let dir = std::env::temp_dir().join(format!("amp-drag-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Night Drive - 1792872851.ogg");
        std::fs::write(&src, &ogg).unwrap();
        let dest = dir.join("Night Drive.mp3");
        transcode(&src, &dest).unwrap();
        let mp3 = std::fs::read(&dest).unwrap();
        let tags = makepad_audio_decode::read_tags(&mp3).unwrap();
        assert_eq!(tags.title.as_deref(), Some("Night Drive"));
        assert_eq!(tags.artist.as_deref(), Some("AI · model"));
        let decoded = makepad_audio_decode::mp3::decode_all(&mp3).unwrap();
        assert!(decoded.duration_secs() >= 2.0);
        // USLT carries the whole multi-line lyrics (UTF-16)
        let utf16: Vec<u8> = lyrics.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert!(mp3.windows(utf16.len()).any(|w| w == utf16.as_slice()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
