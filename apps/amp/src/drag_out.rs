//! Dragging songs out of Amp into other apps (a chat, a mail, the file
//! manager). By default a song goes as a video, `<title>.mp4`: its audio as
//! AAC with its lyric video (or title card) in Amp's style (drop_video.rs), the one kind of file chat apps
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
pub fn start(cx: &mut Cx, window: WindowId, paths: Vec<Option<String>>, theme: &crate::Theme) -> Option<String> {
    let total = paths.len();
    let local: Vec<String> = paths.into_iter().flatten().collect();
    if local.is_empty() {
        return (total > 0).then(|| NOT_LOCAL.to_string());
    }
    let left_out = total - local.len();
    let format = if cx.keyboard.modifiers().alt { DropFormat::Files } else { DropFormat::Video(video_kit(cx, theme)) };
    let items = local.into_iter().map(|path| item(path, &format)).collect();
    cx.start_external_dragging(window, DragOut { items, image: None });
    (left_out > 0).then(|| format!("{left_out} streamed song{} left out: only local files can be dragged", if left_out == 1 { "" } else { "s" }))
}

/// What a drag sends.
pub enum DropFormat {
    /// Each song as `<title>.mp4` (AAC + lyric video), drawn with these fonts and colours.
    Video(drop_video::Kit),
    /// The files themselves; Ogg Vorbis as a tagged MP3.
    Files,
}

/// The UI's fonts (as file paths the producer thread reads) and the style's colours.
fn video_kit(cx: &mut Cx, theme: &crate::Theme) -> drop_video::Kit {
    use crate::lyric_video::{Fonts, Look, BOLD_FONTS, ITALIC_FONTS, REGULAR_FONTS};
    use makepad_widgets::makepad_platform::script::res::crate_resource_abs_path;
    let fonts = cx
        .try_with_vm(|vm| {
            let mut paths = |logical: &[&str]| logical.iter().filter_map(|l| crate_resource_abs_path(vm, l)).map(Into::into).collect::<Vec<_>>();
            Fonts { bold: paths(&BOLD_FONTS), regular: paths(&REGULAR_FONTS), italic: paths(&ITALIC_FONTS) }
        })
        .unwrap_or_default();
    let c = |v: Vec4f| [v.x, v.y, v.z];
    let look = Look { background: c(theme.background), text: c(theme.foreground), muted: c(theme.muted), accent: c(theme.accent), caps: theme.caps };
    drop_video::Kit { fonts, look }
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
        DropFormat::Video(kit) => {
            let kit = kit.clone();
            DragOutItem::Promise(FilePromise {
                name: format!("{name}.mp4"),
                mime: "video/mp4".into(),
                produce: Box::new(move |dest| {
                    let made = drop_video::write_song_video(Path::new(&path), dest, &name, &kit)?;
                    if !made.cached {
                        log!(
                            "amp drop video: {name}.mp4 in {:.0} ms (decode {:.0}, {} lines, {} frames of which {} drawn in {:.0} ms, encode {:.0} ms)",
                            made.total_ms, made.decode_ms, made.lyric_lines, made.frames, made.drawn, made.paint_ms, made.encode_ms
                        );
                    }
                    Ok(())
                }),
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
        tagged_ogg_with(dir, secs, "Night Drive", &[])
    }

    fn tagged_ogg_with(dir: &Path, secs: f32, title: &str, extra: &[(&str, &str)]) -> std::path::PathBuf {
        let rate = 44_100usize;
        let pcm: Vec<f32> = (0..(rate as f32 * secs) as usize).flat_map(|i| {
            let t = i as f32 / rate as f32;
            [(t * 440.0 * std::f32::consts::TAU).sin() * 0.3, (t * 660.0 * std::f32::consts::TAU).sin() * 0.3]
        }).collect();
        let mut comments = vec![
            ("TITLE".to_string(), title.to_string()),
            ("ARTIST".to_string(), "AI · model".to_string()),
            ("ALBUM".to_string(), "AI music".to_string()),
        ];
        comments.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let opts = makepad_audio_encode::EncodeOptions { tags: comments, ..Default::default() };
        let ogg = makepad_audio_encode::encode_vorbis(rate as u32, 2, &pcm, &opts).unwrap();
        let src = dir.join(format!("{title} - 1792872851.ogg"));
        std::fs::write(&src, &ogg).unwrap();
        src
    }

    /// The UI fonts from the checkout, as `video_kit` resolves them in the app, and Amp's colours.
    fn test_kit() -> drop_video::Kit {
        use crate::lyric_video::{Fonts, Look, BOLD_FONTS, ITALIC_FONTS, REGULAR_FONTS};
        let widgets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../widgets");
        let paths = |logical: &[&str]| logical.iter().map(|l| widgets.join(l.strip_prefix("makepad_widgets/").unwrap())).collect();
        drop_video::Kit { fonts: Fonts { bold: paths(&BOLD_FONTS), regular: paths(&REGULAR_FONTS), italic: paths(&ITALIC_FONTS) }, look: Look::default() }
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
        let DragOutItem::Promise(promise) = item(src.to_string_lossy().into_owned(), &DropFormat::Video(test_kit())) else {
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
        assert!(matches!(item(m4a.to_string_lossy().into_owned(), &DropFormat::Video(test_kit())), DragOutItem::File(_)));
        assert!(matches!(item(mp3.to_string_lossy().into_owned(), &DropFormat::Video(test_kit())), DragOutItem::Promise(_)));
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

    /// Latin lyrics timed by the score, with an English translation: the tags Amp's AI songs carry.
    const LATIN_LRC: &str = "[00:00.00][Intro]\n[00:02.00][Verse]\n[00:02.00]<00:02.00>Alea <00:02.60>iacta <00:03.20>lux <00:03.80>est<00:04.60>\n[00:05.00]<00:05.00>Carpe <00:05.70>noctem <00:06.40>aeternam<00:07.40>\n[00:10.00][Chorus]\n[00:10.00]<00:10.00>Ad <00:10.40>astra <00:11.00>per <00:11.40>aspera<00:12.40>\n";
    const LATIN_EN: &str = "[Intro]\n\n[Verse]\nThe die is cast, the light is\nSeize the eternal night\n\n[Chorus]\nTo the stars through hardships\n";

    #[test]
    fn the_timeline_lights_words_and_returns_to_the_title() {
        use crate::lyric_video::{timeline, Scene, Song, FPS};
        let tags = vec![
            ("TITLE".to_string(), "Alea Iacta Lux Est".to_string()),
            ("SYNCEDLYRICS".to_string(), LATIN_LRC.to_string()),
            ("LYRICS_TIMING".to_string(), "from the model's score".to_string()),
            ("TRANSLATION".to_string(), LATIN_EN.to_string()),
        ];
        let song = Song::from_tags(&tags, "x");
        let keys = timeline(&song);
        let at = |t: f64| keys.iter().rev().find(|k| k.0 <= (t * FPS as f64) as i64).unwrap().1;
        assert_eq!(at(0.5), Scene::Title);
        assert_eq!(at(1.6), Scene::Line { line: 0, lit: 0 }, "a line shows before it is sung");
        assert_eq!(at(2.7), Scene::Line { line: 0, lit: 2 });
        assert_eq!(at(4.0), Scene::Line { line: 0, lit: 4 });
        assert_eq!(at(9.2), Scene::Gap { next: 2 }, "a long rest shows the coming line");
        assert_eq!(at(11.1), Scene::Line { line: 2, lit: 3 });
        assert_eq!(at(16.0), Scene::Title, "the title card closes it");
        let l = song.lyrics.as_ref().unwrap();
        assert_eq!(l.lines().nth(1).unwrap().1.translation, "Seize the eternal night");
        // estimated times light the whole line as it starts, not word by word
        let mut tags = tags;
        tags[2].1 = "estimated".into();
        let keys = timeline(&Song::from_tags(&tags, "x"));
        assert!(keys.iter().all(|k| !matches!(k.1, Scene::Line { line: 0, lit: 1..=3 })), "{keys:?}");
    }

    /// Frames of a decoded movie nearest to these times (s), as RGB.
    fn frames_at(path: &Path, times: &[f64]) -> Vec<(f64, Vec<u8>, u32, u32)> {
        let mut video = makepad_video::VideoFileDecoder::open(&path.to_string_lossy()).expect("the mp4 opens");
        let mut best: Vec<Option<(f64, Vec<u8>, u32, u32)>> = vec![None; times.len()];
        while let Ok(Some(frame)) = video.next_frame() {
            let t = frame.pts_100ns as f64 / 1e7;
            for (i, &want) in times.iter().enumerate() {
                // the picture shown at `want`: the last frame at or before it
                if t <= want + 1e-3 && best[i].as_ref().map_or(true, |b| t >= b.0) {
                    best[i] = Some((t, frame.to_rgb8(), frame.width, frame.height));
                }
            }
        }
        best.into_iter().map(|b| b.expect("a frame")).collect()
    }

    fn save_png(dir: &Path, name: &str, rgb: &[u8], w: u32, h: u32) {
        let png = makepad_ai_hub::testpattern::encode_png_rgb8(rgb, w as usize, h as usize).unwrap();
        std::fs::write(dir.join(name), png).unwrap();
    }

    /// Pixels close to the accent (Amp's green): the lit words.
    fn accent_px(rgb: &[u8]) -> usize {
        rgb.chunks(3).filter(|p| p[1] > 150 && p[0] < 110 && p[2] < 140).count()
    }

    /// A song with timed Latin lyrics drops as a lyric video: the title, then the lines with their
    /// words lit as they are sung, the translation under them; made again it comes from the cache.
    #[test]
    fn a_song_with_timed_lyrics_drops_as_a_lyric_video() {
        let dir = std::env::temp_dir().join(format!("amp-lyric-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let secs = 15.0;
        let src = tagged_ogg_with(&dir, secs, "Alea Iacta Lux Est", &[
            ("SYNCEDLYRICS", LATIN_LRC),
            ("LYRICS_TIMING", "from the model's score"),
            ("TRANSLATION", LATIN_EN),
            ("TRANSLATION_LANGUAGE", "English"),
        ]);
        let kit = test_kit();
        let dest = dir.join("out.mp4");
        let made = match drop_video::write_song_video(&src, &dest, "Alea Iacta Lux Est", &kit) {
            Ok(made) => made,
            Err(e) if !cfg!(target_vendor = "apple") && e.starts_with("video encoder") => {
                eprintln!("LYRIC-MP4: skipped, {e}");
                return;
            }
            Err(e) => panic!("making the video: {e}"),
        };
        assert!(!made.cached && made.lyric_lines == 3, "{made:?}");
        let info = makepad_video::VideoFileDecoder::open(&dest.to_string_lossy()).unwrap().info().clone();
        assert_eq!((info.width, info.height), (drop_video::WIDTH, drop_video::HEIGHT));
        if cfg!(target_vendor = "apple") {
            let duration = info.duration_100ns as f64 / 1e7;
            assert!(info.has_audio && (duration - secs as f64).abs() < 0.2, "{duration} s {info:?}");
        }
        if !cfg!(target_vendor = "apple") {
            // the GStreamer reader's frames are not checked here: the full check is the Apple one
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }
        let bytes = std::fs::read(&dest).unwrap();
        let moov = bytes.windows(4).position(|w| w == b"moov").unwrap();
        let mdat = bytes.windows(4).position(|w| w == b"mdat").unwrap();
        assert!(moov < mdat, "fast start: the index before the media");
        assert_eq!(made.frames as i64, (secs as f64 * crate::lyric_video::FPS as f64).ceil() as i64, "a frame every tick");
                let shots = frames_at(&dest, &[0.5, 1.8, 2.7, 4.2, 9.3, 11.2]);
        let [title, coming, two_lit, all_lit, gap, chorus] = [0, 1, 2, 3, 4, 5].map(|i| &shots[i].1);
        assert!(accent_px(coming) < 450, "nothing lit before it is sung (the header's mark only): {}", accent_px(coming));
        assert!(accent_px(two_lit) > accent_px(coming) + 700, "two words lit: {}", accent_px(two_lit));
        assert!(accent_px(all_lit) > accent_px(two_lit) + 450, "{} vs {}", accent_px(all_lit), accent_px(two_lit));
        let differ = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x.abs_diff(**y) > 40).count();
        assert!(differ(title, coming) > 4_000 && differ(gap, chorus) > 4_000);
        let again = drop_video::write_song_video(&src, &dir.join("again.mp4"), "Alea Iacta Lux Est", &kit).unwrap();
        assert!(again.cached, "{again:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(dir.join("again.mp4")).unwrap());
        eprintln!("LYRIC-MP4: {made:?}; cached again in {:.1} ms", again.total_ms);
        if let Ok(keep) = std::env::var("AMP_DROP_TEST_OUT") {
            let keep = Path::new(&keep);
            std::fs::create_dir_all(keep).unwrap();
            for (name, shot) in ["title", "coming", "lit2", "lit4", "gap", "chorus"].iter().zip(&shots) {
                save_png(keep, &format!("synthetic-{name}.png"), &shot.1, shot.2, shot.3);
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The sample for the report: a real AI song (`AMP_LYRIC_SONG`, a path from this crate) made
    /// cold and then from the cache, its tracks checked, frames at `AMP_LYRIC_AT` (s, comma
    /// separated) kept as PNGs with the MP4 in `AMP_DROP_TEST_OUT`. Without the song: nothing.
    #[test]
    fn sample_lyric_video_of_a_real_song() {
        let Ok(song) = std::env::var("AMP_LYRIC_SONG") else { return };
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(song);
        let out = std::path::PathBuf::from(std::env::var("AMP_DROP_TEST_OUT").unwrap_or_else(|_| std::env::temp_dir().join("amp-lyric-sample").to_string_lossy().into_owned()));
        std::fs::create_dir_all(&out).unwrap();
        let _ = std::fs::remove_dir_all(drop_video::cache_dir());
        let name = display_name(&src.to_string_lossy());
        let dest = out.join(format!("{name}.mp4"));
        let kit = test_kit();
        let cold = drop_video::write_song_video(&src, &dest, &name, &kit).expect("the video");
        let warm = drop_video::write_song_video(&src, &out.join("cached.mp4"), &name, &kit).expect("the cached video");
        assert!(!cold.cached && warm.cached);
        let _ = std::fs::remove_file(out.join("cached.mp4"));
        let video = makepad_video::VideoFileDecoder::open(&dest.to_string_lossy()).unwrap();
        let info = video.info().clone();
        drop(video);
        let size = std::fs::metadata(&dest).unwrap().len();
        let mut audio_frames = 0usize;
        if let Ok(mut audio) = makepad_video::VideoFileDecoder::open_audio(&dest.to_string_lossy()) {
            while let Ok(Some(chunk)) = audio.next_audio() {
                audio_frames += chunk.samples.len() / chunk.channels as usize;
            }
        }
        eprintln!(
            "LYRIC-SAMPLE: {name}: {size} bytes, {:.2} s, {:?} {}x{}, audio {} Hz x{} ({:.2} s decoded); cold {cold:?}; cached {:.1} ms",
            info.duration_100ns as f64 / 1e7, info.video_codec, info.width, info.height, info.audio_sample_rate, info.audio_channels,
            audio_frames as f64 / info.audio_sample_rate.max(1) as f64, warm.total_ms
        );
        let times: Vec<f64> = std::env::var("AMP_LYRIC_AT").unwrap_or_else(|_| "1,20,40,60".into()).split(',').filter_map(|t| t.trim().parse().ok()).collect();
        for (t, shot) in times.iter().zip(frames_at(&dest, &times)) {
            save_png(&out, &format!("frame-{t:05.1}s.png"), &shot.1, shot.2, shot.3);
        }
    }

    /// The painter alone (no encoder): its scenes as PNGs in `AMP_DROP_TEST_OUT`, for a look.
    #[test]
    fn paints_the_scenes() {
        use crate::lyric_video::{Painter, Scene, Song, SIZE};
        let tags = vec![
            ("TITLE".to_string(), "Alea Iacta Lux Est".to_string()),
            ("ARTIST".to_string(), "AI · yue2-3b".to_string()),
            ("SYNCEDLYRICS".to_string(), LATIN_LRC.to_string()),
            ("LYRICS_TIMING".to_string(), "from the model's score".to_string()),
            ("TRANSLATION".to_string(), LATIN_EN.to_string()),
        ];
        let song = Song::from_tags(&tags, "x");
        let kit = test_kit();
        let mut painter = Painter::new(&song, &kit.look, &kit.fonts);
        let mut nv12 = Vec::new();
        let started = std::time::Instant::now();
        let scenes = [Scene::Title, Scene::Line { line: 0, lit: 2 }, Scene::Line { line: 1, lit: 3 }, Scene::Gap { next: 2 }];
        for (n, scene) in scenes.iter().enumerate() {
            painter.paint(*scene, &mut nv12);
            assert_eq!(nv12.len(), (SIZE * SIZE * 3 / 2) as usize);
            if let Ok(keep) = std::env::var("AMP_DROP_TEST_OUT") {
                let keep = Path::new(&keep);
                std::fs::create_dir_all(keep).unwrap();
                let mut rgb = Vec::new();
                makepad_video::nv12::nv12_to_rgb8(&nv12, SIZE, SIZE, crate::lyric_video::COLOR, &mut rgb);
                save_png(keep, &format!("scene-{n}.png"), &rgb, SIZE, SIZE);
            }
        }
        eprintln!("PAINT: 4 scenes in {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }
}
