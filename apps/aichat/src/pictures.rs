//! The pictures the person sent in a conversation, as the panel shares them: with the app's
//! tools ([`ChatPictures`], a `Cx` global), and with the model when it asks for one again
//! (`chat.picture`: [`fit_for_model`] on the task pool). The conversation keeps them
//! ([`makepad_ai_services::engine::EngineCore::keep_chat`]): a file each beside its transcript.

use crate::attach::{self, MAX_SIDE};
use makepad_ai_services::engine::{EngineCore, SentPicture};
use makepad_ai_services::pictures::{crop, fit_picture};
use makepad_ai_services::wire::MAX_RESULT_IMAGE_BYTES;

/// The pictures the person sent in this conversation, by name, as they were dropped (at their
/// own size): an app's tool reads one as its input (`ChatPictures::get`). They come back with
/// the conversation after a restart.
#[derive(Default)]
pub struct ChatPictures {
    pub pictures: Vec<SentPicture>,
}

impl ChatPictures {
    /// The conversation's pictures now.
    pub fn of(engine: &EngineCore) -> ChatPictures {
        ChatPictures { pictures: engine.pictures().to_vec() }
    }

    /// The picture of that name, or with no name the one sent last.
    pub fn get(&self, name: Option<&str>) -> Option<&SentPicture> {
        match name {
            Some(name) => self.pictures.iter().find(|p| p.name == name),
            None => self.pictures.iter().max_by_key(|p| p.turns.last().copied().unwrap_or(0)),
        }
    }

    /// The names, for an answer that names none of them.
    pub fn names(&self) -> Vec<&str> {
        self.pictures.iter().map(|p| p.name.as_str()).collect()
    }
}

/// Posted when a picture the person sent has its file (or will have none): an app's tool that
/// waits for the file ([`SentPicture::kept_now`]) goes on.
#[derive(Clone, Debug, Default)]
pub struct PictureKept;

/// A picture the person sent, or the region `rect` of it ([`crop`]), as the model sees it again:
/// no larger than [`MAX_SIDE`] a side, and within a tool result's picture budget. Reads and
/// decodes the picture as it was dropped: run it on the task pool.
pub fn fit_for_model(picture: &SentPicture, rect: Option<[f64; 4]>) -> Result<Vec<u8>, String> {
    let data = picture.data()?;
    let (_, pixels) = attach::decode(&data, &picture.name)?;
    let pixels = match rect {
        Some(rect) => crop(&pixels, rect),
        None => pixels,
    };
    fit_picture(pixels, MAX_SIDE as u32, MAX_RESULT_IMAGE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::engine::{EngineEvent, Model, ModelEvent, ModelImage, NewPicture, ServiceRegistry, ToolDefinition};
    use makepad_ai_services::state::Entry;
    use makepad_ai_services::wire::ToolOutcome;
    use std::sync::{Arc, Mutex};

    /// What the model was given: pictures and tool results.
    #[derive(Default)]
    struct Seen {
        images: Vec<ModelImage>,
        results: Vec<(String, String)>,
        out: Vec<ModelEvent>,
    }

    /// A model that sees pictures and calls the tools it is told to.
    struct Looking(Arc<Mutex<Seen>>);

    impl Model for Looking {
        fn label(&self) -> String {
            "Looking".into()
        }
        fn configure(&mut self, _system: &str, _tools: &[ToolDefinition]) -> Result<(), String> {
            Ok(())
        }
        fn send_user(&mut self, _text: &str, _context: &str) {
            self.0.lock().unwrap().out.push(ModelEvent::TurnDone { tool_calls: 0 });
        }
        fn send_tool_result(&mut self, call_id: &str, text: &str, _is_error: bool) {
            let mut seen = self.0.lock().unwrap();
            seen.results.push((call_id.to_string(), text.to_string()));
            seen.out.push(ModelEvent::TurnDone { tool_calls: 0 });
        }
        fn cancel(&mut self) {}
        fn reset(&mut self) {}
        fn poll(&mut self) -> Vec<ModelEvent> {
            std::mem::take(&mut self.0.lock().unwrap().out)
        }
        fn attach_images(&mut self, images: Vec<ModelImage>) -> Result<(), String> {
            self.0.lock().unwrap().images.extend(images);
            Ok(())
        }
    }

    fn engine() -> (EngineCore, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        (EngineCore::new(ServiceRegistry::new(), Box::new(Looking(seen.clone())), None, 1), seen)
    }

    /// A photo-like picture a PNG hardly compresses: a gradient under noise.
    fn noisy_png(width: usize, height: usize, seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).max(1);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                for c in 0..3 {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    rgba.push(((x + y + c * 40) * 200 / (width + height + 80)) as u8 ^ (state % 64) as u8);
                }
                rgba.push(255);
            }
        }
        Cx::encode_rgba_as_png(width as u32, height as u32, &rgba).unwrap()
    }

    /// A pump, its pictures written and fitted as the panel does on the task pool (here, in turn).
    fn pump(core: &mut EngineCore, now: f64) {
        for event in core.pump(now) {
            match event {
                EngineEvent::KeepPicture(write) => write.run(),
                EngineEvent::PictureWanted { call_id, picture, rect } => core.picture_fitted(&call_id, fit_for_model(&picture, rect)),
                _ => {}
            }
        }
    }

    /// The model calls `name` with `args` and the engine answers it.
    fn call(core: &mut EngineCore, seen: &Arc<Mutex<Seen>>, id: &str, name: &str, args: &str, now: f64) -> String {
        core.send("look", now);
        seen.lock().unwrap().out = vec![ModelEvent::ToolCall { call_id: id.into(), name: name.into(), args: args.into() }, ModelEvent::TurnDone { tool_calls: 1 }];
        pump(core, now + 0.1);
        pump(core, now + 0.2);
        seen.lock().unwrap().results.iter().rev().find(|(call_id, _)| call_id == id).map(|(_, text)| text.clone()).unwrap_or_default()
    }

    fn dropped(name: &str, bytes: &Arc<[u8]>, w: u32, h: u32, for_model: &Arc<[u8]>) -> NewPicture {
        attach::Dropped {
            original: bytes.clone(),
            file: makepad_ai_services::engine::picture_file(bytes, "png"),
            width: w,
            height: h,
            sent: attach::Attachment { name: name.into(), png: for_model.clone(), width: 64, height: 48, thumb: None },
        }
        .into_new()
    }

    fn files(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).map(|l| l.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
        names.sort();
        names
    }

    use makepad_widgets::Cx;

    /// The person's pictures stay with the chat: kept once each beside the transcript, back
    /// after a restart (in the history and for the app's tools), listed and shown again to the
    /// model on request within the result's picture budget, and gone with the chat.
    #[test]
    fn sent_pictures_are_kept_with_the_chat_and_fetched_by_name() {
        let root = std::env::temp_dir().join(format!("aichat-pictures-{}", std::process::id()));
        let dir = root.join("app.pictures");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Wider than the model takes: it is shown smaller.
        let photo = Arc::<[u8]>::from(noisy_png(1030, 60, 1));
        let other = Arc::<[u8]>::from(noisy_png(64, 48, 2));
        let picture = |name: &str, bytes: &Arc<[u8]>, w: u32, h: u32| dropped(name, bytes, w, h, &other);
        let files = || files(&dir);

        let (mut core, seen) = engine();
        core.keep_chat(dir.clone(), None).unwrap();
        core.send_pictures("", vec![picture("keys.png", &photo, 1030, 60)], 0.0).unwrap();
        pump(&mut core, 0.1);
        // The same picture again keeps its name and its one file; another under a taken name is numbered.
        core.send_pictures("again", vec![picture("copy.png", &photo, 1030, 60), picture("keys.png", &other, 64, 48)], 1.0).unwrap();
        pump(&mut core, 1.1);
        assert_eq!(files().len(), 2, "one file per picture: {:?}", files());
        assert_eq!(std::fs::read(dir.join(makepad_ai_services::engine::picture_file(&photo, "png"))).unwrap(), &photo[..], "kept as it was dropped");
        let names: Vec<&str> = core.pictures().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["keys.png", "keys-2.png"]);
        let lines: Vec<String> = core.state().entries.iter().filter_map(|e| match e { Entry::User { text, .. } => Some(text.clone()), _ => None }).collect();
        assert!(lines[1].ends_with("[attached: keys.png, keys-2.png]"), "{lines:?}");
        assert_eq!(seen.lock().unwrap().images.len(), 3, "each line's pictures went with it");
        let saved = core.saved_chat();
        // A crash between a send and the transcript's write leaves a picture no line names.
        std::fs::write(dir.join(format!("{}.png", "ab".repeat(32))), b"orphan").unwrap();

        // The app starts again.
        let (mut core, seen) = engine();
        core.keep_chat(dir.clone(), Some(&saved)).unwrap();
        assert_eq!(files().len(), 2, "the picture no line names is gone: {:?}", files());
        let restored: Vec<usize> = core.state().entries.iter().filter_map(|e| match e { Entry::User { pictures, .. } => Some(pictures.len()), _ => None }).collect();
        assert_eq!(restored, [1, 2], "the history shows each line's pictures");
        let shared = ChatPictures::of(&core);
        let keys = shared.get(Some("keys.png")).expect("the app's tools find it by name");
        assert_eq!((keys.width, keys.height, keys.turns.clone()), (1030, 60, vec![1, 2]));
        assert_eq!(&keys.data().unwrap()[..], &photo[..], "at its own size");
        assert!(seen.lock().unwrap().images.is_empty(), "old pictures are not sent again");

        let listed = call(&mut core, &seen, "m1", "chat.pictures", "{}", 2.0);
        assert_eq!(
            listed,
            r#"{"pictures":[{"name":"keys.png","width":1030,"height":60,"bytes":BYTES,"turns":[1,2]},{"name":"keys-2.png","width":64,"height":48,"bytes":OTHER,"turns":[2]}]}"#
                .replace("BYTES", &photo.len().to_string())
                .replace("OTHER", &other.len().to_string())
        );
        let fetched = call(&mut core, &seen, "m2", "chat.picture", r#"{"name":"keys.png"}"#, 3.0);
        assert_eq!(fetched, r#"{"name":"keys.png","width":1030,"height":60,"shown":[1024,60]}"#, "shown at the model's size");
        let images = std::mem::take(&mut seen.lock().unwrap().images);
        assert_eq!(images.len(), 1, "the picture came with the result");
        assert!(makepad_ai_services::pictures::base64_len(images[0].png.len()) <= MAX_RESULT_IMAGE_BYTES, "within the picture budget");
        let missing = call(&mut core, &seen, "m3", "chat.picture", r#"{"name":"nope.png"}"#, 4.0);
        assert!(missing.contains("keys.png") && missing.contains("keys-2.png"), "{missing}");
        assert!(core.state().tool("m3").is_some_and(|t| matches!(&t.status, makepad_ai_services::state::ToolStatus::Done { outcome: ToolOutcome::Refused, .. })));

        // A new conversation takes the pictures with it.
        core.clear(5.0);
        assert!(!dir.exists(), "the chat's pictures are removed: {:?}", files());
        assert!(ChatPictures::of(&core).pictures.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A large picture's send does not wait for its file: the send writes nothing, the picture
    /// reads from memory meanwhile, what needs the file waits until the pool has written it and
    /// then reads it, and the model's look at it reads the file. A chat cleared while a write
    /// is under way keeps no file of it.
    #[test]
    fn a_large_picture_is_sent_at_once_and_its_file_written_off_the_send() {
        let root = std::env::temp_dir().join(format!("aichat-pictures-write-{}", std::process::id()));
        let dir = root.join("app.pictures");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let large = Arc::<[u8]>::from(noisy_png(2048, 1536, 3));
        let small = Arc::<[u8]>::from(noisy_png(64, 48, 4));
        let (mut core, seen) = engine();
        core.keep_chat(dir.clone(), None).unwrap();

        core.send_pictures("this one", vec![dropped("large.png", &large, 2048, 1536, &small)], 0.0).unwrap();
        assert!(files(&dir).is_empty(), "the send wrote nothing: {:?}", files(&dir));
        let picture = ChatPictures::of(&core).get(Some("large.png")).cloned().unwrap();
        assert!(picture.kept_now().is_none(), "its file is being written");
        assert_eq!(&picture.data().unwrap()[..], &large[..], "it reads from memory meanwhile");
        let writes: Vec<_> = core.pump(0.1).into_iter().filter_map(|e| if let EngineEvent::KeepPicture(write) = e { Some(write) } else { None }).collect();
        assert_eq!(writes.len(), 1, "the host is asked to write it");

        // A tool that needs the file waits for the write, then reads it.
        let waiting = picture.clone();
        let reader = std::thread::spawn(move || waiting.kept_path().and_then(|path| std::fs::read(path).map_err(|e| e.to_string())));
        std::thread::spawn(move || writes.into_iter().for_each(|write| write.run())).join().unwrap();
        assert_eq!(&reader.join().unwrap().unwrap()[..], &large[..], "read after the write");
        let path = picture.kept_now().unwrap().unwrap();
        assert_eq!(files(&dir), [makepad_ai_services::engine::picture_file(&large, "png")]);
        assert_eq!(path, dir.join(&files(&dir)[0]));
        let fetched = call(&mut core, &seen, "m1", "chat.picture", r#"{"name":"large.png"}"#, 1.0);
        assert!(fetched.starts_with(r#"{"name":"large.png","width":2048,"height":1536,"shown":"#), "{fetched}");

        // Cleared while the next picture's write is under way: no file of it is left.
        core.send_pictures("", vec![dropped("small.png", &small, 64, 48, &small)], 2.0).unwrap();
        let late: Vec<_> = core.pump(2.1).into_iter().filter_map(|e| if let EngineEvent::KeepPicture(write) = e { Some(write) } else { None }).collect();
        let small_picture = ChatPictures::of(&core).get(Some("small.png")).cloned().unwrap();
        core.clear(3.0);
        late.into_iter().for_each(|write| write.run());
        assert!(!dir.exists(), "nothing is left of the cleared chat: {:?}", files(&dir));
        assert!(small_picture.kept_path().is_err(), "a tool that asks for it is told it is gone");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A region of a picture (`rect`) comes back at the whole picture budget: at its own pixels
    /// where they fit it, which the whole picture scaled to fit shows far smaller; a larger region
    /// scaled to fit, its shape kept; a rect that is no region is refused.
    #[test]
    fn a_region_of_a_picture_comes_back_at_the_whole_budget() {
        use makepad_ai_services::pictures::{base64_len, decode_png};
        let root = std::env::temp_dir().join(format!("aichat-pictures-rect-{}", std::process::id()));
        let dir = root.join("app.pictures");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let photo = Arc::<[u8]>::from(noisy_png(2048, 1536, 5));
        let small = Arc::<[u8]>::from(noisy_png(64, 48, 6));
        let (mut core, seen) = engine();
        core.keep_chat(dir.clone(), None).unwrap();
        core.send_pictures("", vec![dropped("photo.png", &photo, 2048, 1536, &small)], 0.0).unwrap();
        pump(&mut core, 0.1);
        seen.lock().unwrap().images.clear();
        let shown = |text: &str| -> [i64; 2] {
            let v = makepad_strict_json::parse(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            let s = v.get("shown").and_then(|s| s.as_arr()).unwrap_or_else(|| panic!("{text}"));
            [s[0].as_i64().unwrap(), s[1].as_i64().unwrap()]
        };
        let image = || {
            let images = std::mem::take(&mut seen.lock().unwrap().images);
            assert_eq!(images.len(), 1, "the picture came with the result");
            assert!(base64_len(images[0].png.len()) <= MAX_RESULT_IMAGE_BYTES, "within the picture budget");
            decode_png(&images[0].png).unwrap()
        };

        let whole = call(&mut core, &seen, "w", "chat.picture", r#"{"name":"photo.png"}"#, 1.0);
        let [whole_w, _] = shown(&whole);
        image();
        // The quarter from the middle of the top half's right side: 512 x 384 of the picture's pixels.
        let region = call(&mut core, &seen, "r", "chat.picture", r#"{"name":"photo.png","rect":[0.5,0.25,0.75,0.5]}"#, 2.0);
        assert_eq!(region, r#"{"name":"photo.png","width":2048,"height":1536,"rect":[0.5,0.25,0.75,0.5],"region":[1024,384,1536,768],"shown":[512,384]}"#);
        assert!(whole_w * 512 / 2048 < 512, "the whole picture shows that region at {} pixels across", whole_w * 512 / 2048);
        let got = image();
        let (_, original) = attach::decode(&photo, "photo.png").unwrap();
        for (x, y) in [(0, 0), (511, 0), (0, 383), (511, 383), (200, 100)] {
            let at = |p: &makepad_ai_services::pictures::RgbaPicture, x: usize, y: usize| p.rgba[(y * p.width as usize + x) * 4..][..4].to_vec();
            assert_eq!(at(&got, x, y), at(&original, 1024 + x, 384 + y), "the region's pixel ({x}, {y})");
        }
        // Three quarters of it each way: scaled to fit, as 4:3 as the region is, and more of the
        // picture's pixels to each shown than the whole has.
        let large = call(&mut core, &seen, "l", "chat.picture", r#"{"name":"photo.png","rect":[0,0,0.75,0.75]}"#, 3.0);
        let [w, h] = shown(&large);
        assert!(w <= MAX_SIDE as i64 && (w * 3 - h * 4).abs() <= 4 && w * 2048 > whole_w * 1536, "shown {w} x {h}, the whole {whole_w} across");
        image();
        let refused = call(&mut core, &seen, "x", "chat.picture", r#"{"name":"photo.png","rect":[0.5,0.2,0.4,0.9]}"#, 4.0);
        assert!(refused.contains("'rect'"), "{refused}");
        assert!(core.state().tool("x").is_some_and(|t| matches!(&t.status, makepad_ai_services::state::ToolStatus::Done { outcome: ToolOutcome::Refused, .. })));
        let _ = std::fs::remove_dir_all(&root);
    }
}
