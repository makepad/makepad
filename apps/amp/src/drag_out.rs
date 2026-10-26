//! Dragging songs out of Amp into other apps (a chat, a mail, the file
//! manager). By default a song goes as a video, `<title>.mp4`: its audio as
//! AAC with a still title card (drop_video.rs), the one kind of file chat apps
//! (WhatsApp and the like) take and play inline with its sound. It is a file
//! promise whose producer starts with the drag, so the file is usually done
//! before the pointer reaches the chat; the receiver gets it only once it is
//! complete. Held Option/Alt at the start of the drag sends the person's own
//! files as they are (MP3, M4A, FLAC, WAV) and Ogg Vorbis as a tagged MP3 made
//! for the drop (title, artist, album, genre, date, lyrics as ID3), for the
//! file manager. Files without an in-tree decoder (M4A, Opus) always go as
//! they are. Streamed tracks are not the person's files and are refused.

use crate::drop_video;
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
    let format = if cx.keyboard.modifiers().alt { DropFormat::Files } else { DropFormat::Video(card_fonts(cx)) };
    let items = local.into_iter().map(|path| item(path, &format)).collect();
    cx.start_external_dragging(window, DragOut { items, image: None });
    (left_out > 0).then(|| format!("{left_out} streamed song{} left out: only local files can be dragged", if left_out == 1 { "" } else { "s" }))
}

/// What a drag sends.
pub enum DropFormat {
    /// Each song as `<title>.mp4` (AAC + title card), drawn with these fonts.
    Video(drop_video::CardFonts),
    /// The files themselves; Ogg Vorbis as a tagged MP3.
    Files,
}

/// The UI's fonts for the title card, as file paths the producer thread reads.
fn card_fonts(cx: &mut Cx) -> drop_video::CardFonts {
    use makepad_widgets::makepad_platform::script::res::crate_resource_abs_path;
    let paths = cx
        .try_with_vm(|vm| drop_video::CARD_FONTS.iter().filter_map(|logical| crate_resource_abs_path(vm, logical)).collect::<Vec<_>>())
        .unwrap_or_default();
    drop_video::CardFonts { paths: paths.into_iter().map(Into::into).collect() }
}

/// A file the in-tree decoders read (and so can turn into a video or an MP3).
fn decodable(path: &str) -> bool {
    let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "mp3" | "flac" | "wav" => true,
        // An Opus stream has no decoder here: it goes as it is.
        "ogg" | "oga" => !is_opus(path),
        _ => false,
    }
}

pub fn item(path: String, format: &DropFormat) -> DragOutItem {
    if !decodable(&path) {
        return DragOutItem::File(path);
    }
    let name = display_name(&path);
    match format {
        DropFormat::Video(fonts) => {
            let fonts = fonts.clone();
            DragOutItem::Promise(FilePromise {
                name: format!("{name}.mp4"),
                mime: "video/mp4".into(),
                produce: Box::new(move |dest| drop_video::write_song_video(Path::new(&path), dest, &name, &fonts)),
            })
        }
        DropFormat::Files => {
            let ext = Path::new(&path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !matches!(ext.as_str(), "ogg" | "oga") {
                return DragOutItem::File(path);
            }
            DragOutItem::Promise(FilePromise {
                name: format!("{name}.mp3"),
                mime: "audio/mpeg".into(),
                produce: Box::new(move |dest| transcode(Path::new(&path), dest)),
            })
        }
    }
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

    fn tagged_ogg(dir: &Path, secs: f32) -> std::path::PathBuf {
        let rate = 44_100usize;
        let pcm: Vec<f32> = (0..(rate as f32 * secs) as usize).flat_map(|i| {
            let t = i as f32 / rate as f32;
            [(t * 440.0 * std::f32::consts::TAU).sin() * 0.3, (t * 660.0 * std::f32::consts::TAU).sin() * 0.3]
        }).collect();
        let comments = vec![
            ("TITLE".to_string(), "Night Drive".to_string()),
            ("ARTIST".to_string(), "AI · model".to_string()),
            ("ALBUM".to_string(), "AI music".to_string()),
        ];
        let opts = makepad_audio_encode::EncodeOptions { tags: comments, ..Default::default() };
        let ogg = makepad_audio_encode::encode_vorbis(rate as u32, 2, &pcm, &opts).unwrap();
        let src = dir.join("Night Drive - 1792872851.ogg");
        std::fs::write(&src, &ogg).unwrap();
        src
    }

    /// The UI fonts from the checkout, as `card_fonts` resolves them in the app.
    fn test_fonts() -> drop_video::CardFonts {
        let widgets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../widgets");
        drop_video::CardFonts {
            paths: drop_video::CARD_FONTS.iter().map(|l| widgets.join(l.strip_prefix("makepad_widgets/").unwrap())).collect(),
        }
    }

    /// The whole drop of a song as video: the promise's producer, the platform's completion
    /// (sync + rename), the delivery into the receiver's folder the AppKit promise does, and a
    /// receiver that opens the file the moment it is handed over. The file must be whole then,
    /// and play back: an H.264 picture (the title card) and an AAC track of the song's length.
    #[test]
    fn a_song_drops_as_a_video_that_is_whole_when_handed_over() {
        use makepad_widgets::makepad_platform::makepad_hal::drag_promise;
        let dir = std::env::temp_dir().join(format!("amp-drop-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("made")).unwrap();
        std::fs::create_dir_all(dir.join("receiver")).unwrap();
        let secs = 5.5;
        let src = tagged_ogg(&dir, secs);
        let DragOutItem::Promise(promise) = item(src.to_string_lossy().into_owned(), &DropFormat::Video(test_fonts())) else {
            panic!("an Ogg song drops as a promise")
        };
        assert_eq!(promise.name, "Night Drive.mp4");
        assert_eq!(promise.mime, "video/mp4");
        let made = dir.join("made").join(&promise.name);
        let part = drag_promise::part_path(&made);
        let started = std::time::Instant::now();
        match (promise.produce)(&part) {
            Ok(()) => {}
            // Linux / Windows nodes without an H.264 + AAC encoder: nothing to check there
            Err(e) if !cfg!(target_vendor = "apple") && e.starts_with("video encoder") => {
                eprintln!("DROP-MP4: skipped, {e}");
                return;
            }
            Err(e) => panic!("making the video: {e}"),
        }
        let made_ms = started.elapsed().as_millis();
        std::fs::rename(&part, &made).unwrap();
        let dest = dir.join("receiver").join(&promise.name);
        drag_promise::deliver(&made, &dest).unwrap();
        // the receiver reads it right away
        let bytes = std::fs::read(&dest).unwrap();
        let made_len = std::fs::metadata(&made).unwrap().len();
        assert_eq!(bytes.len() as u64, made_len);
        assert!(bytes.len() > 40_000, "{} bytes", bytes.len());
        assert_eq!(&bytes[4..8], b"ftyp", "an MPEG-4 file");
        assert!(bytes.windows(4).any(|w| w == b"moov"), "indexed (finished) movie");
        assert!(!drag_promise::part_path(&dest).exists());

        let path = dest.to_string_lossy();
        let mut video = makepad_video::VideoFileDecoder::open(&path).expect("the mp4 opens");
        let info = video.info().clone();
        let duration = info.duration_100ns as f64 / 1e7;
        assert_eq!((info.width, info.height), (drop_video::WIDTH, drop_video::HEIGHT));
        // The GStreamer reader names no codec and no audio track, and its writer cannot end the
        // movie at the audio's end (it ends a frame later): the full check is the Apple one.
        let apple = cfg!(target_vendor = "apple");
        if apple {
            assert_eq!(info.video_codec, Some(makepad_video::VideoFileCodec::H264));
            assert!(info.has_audio && info.audio_sample_rate == 44_100 && info.audio_channels == 2, "{info:?}");
            assert!((duration - secs as f64).abs() < 0.2, "duration {duration} s, the song is {secs} s");
        }
        let frame = video.next_frame().unwrap().expect("a picture");
        let rgb = frame.to_rgb8();
        let lit = rgb.chunks(3).filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200).count();
        let dark = rgb.chunks(3).filter(|p| p[0] < 80 && p[1] < 80 && p[2] < 80).count();
        assert!(lit > 2_000 && dark > rgb.len() / 3 / 2, "the card has its text on the dark ground: {lit} lit, {dark} dark");
        let (mut frames, mut energy) = (0usize, 0f64);
        match makepad_video::VideoFileDecoder::open_audio(&path) {
            Ok(mut audio) => {
                while let Ok(Some(chunk)) = audio.next_audio() {
                    frames += chunk.samples.len() / chunk.channels as usize;
                    energy += chunk.samples.iter().map(|s| (*s as f64 / 32768.0).powi(2)).sum::<f64>();
                }
            }
            Err(e) => assert!(!apple, "the sound opens: {e}"),
        }
        let audio_secs = frames as f64 / 44_100.0;
        let rms = (energy / (frames * 2).max(1) as f64).sqrt();
        if apple || frames > 0 {
            assert!((audio_secs - secs as f64).abs() < 0.2, "{audio_secs} s of sound");
            assert!(rms > 0.1 && rms < 0.4, "the song's sound (sines at 0.3: rms 0.21), got {rms}");
        }
        eprintln!(
            "DROP-MP4: {} bytes, {duration:.3} s, video H.264 {}x{}, AAC {} Hz x{} ({audio_secs:.3} s decoded, rms {rms:.3}), made in {made_ms} ms, card {lit} lit px",
            bytes.len(), info.width, info.height, info.audio_sample_rate, info.audio_channels
        );
        if let Ok(keep) = std::env::var("AMP_DROP_TEST_OUT") {
            std::fs::create_dir_all(&keep).unwrap();
            drag_promise::deliver(&made, &Path::new(&keep).join(&promise.name)).unwrap();
            let png = makepad_ai_hub::testpattern::encode_png_rgb8(&rgb, frame.width as usize, frame.height as usize).unwrap();
            std::fs::write(Path::new(&keep).join("card-frame.png"), png).unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn option_drag_sends_files_and_ogg_as_mp3() {
        let dir = std::env::temp_dir().join(format!("amp-drop-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = tagged_ogg(&dir, 0.5);
        let DragOutItem::Promise(p) = item(src.to_string_lossy().into_owned(), &DropFormat::Files) else { panic!("Ogg goes as MP3") };
        assert_eq!((p.name.as_str(), p.mime.as_str()), ("Night Drive.mp3", "audio/mpeg"));
        let mp3 = dir.join("x.mp3");
        std::fs::write(&mp3, b"ID3").unwrap();
        assert!(matches!(item(mp3.to_string_lossy().into_owned(), &DropFormat::Files), DragOutItem::File(_)));
        let m4a = dir.join("x.m4a");
        std::fs::write(&m4a, b"....ftypM4A ").unwrap();
        // no in-tree decoder: the file itself, whatever the format
        assert!(matches!(item(m4a.to_string_lossy().into_owned(), &DropFormat::Video(test_fonts())), DragOutItem::File(_)));
        assert!(matches!(item(mp3.to_string_lossy().into_owned(), &DropFormat::Video(test_fonts())), DragOutItem::Promise(_)));
        std::fs::remove_dir_all(&dir).unwrap();
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
