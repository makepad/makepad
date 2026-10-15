//! Java MediaPlayer control plus worker-owned OES -> RGBA readback -> Vulkan staging.
//! The EGL context is private to this worker, never shared with the renderer.
use super::{exception, field, java, lock, method, string, void, Engine, Global, Message, Output, Ticket};
use crate::{backend::{exec::Gpu, vulkan::Vulkan}, ledger::{Kind, LedgerEntry}, media::*, session::{Session, TextureObj}, types::{format, usage, Attr, Id}};
use makepad_jni_sys::{registration as reg, *};
use std::{collections::{HashMap, VecDeque}, ffi::{c_char, c_void, CStr}, sync::{Arc, Mutex, OnceLock}, thread::Thread};

pub(super) struct Plane { pub width: u32, pub height: u32, pub format: u32, pub bytes: Vec<u8> }
pub(super) struct Frame { pub planes: Vec<Plane>, pub yuv: u32, pub matrix: f32 }
impl Frame {
    pub fn rotate(&mut self, turns: u32) {
        if turns == 0 { return; }
        for plane in &mut self.planes {
            let (w, h) = (plane.width as usize, plane.height as usize);
            let bpp = if plane.format == format::R8 { 1 } else { 4 };
            let (nw, nh) = if turns & 1 != 0 { (h, w) } else { (w, h) };
            let mut bytes = vec![0; plane.bytes.len()];
            for y in 0..h { for x in 0..w {
                let (dx, dy) = match turns { 1 => (h - 1 - y, x), 2 => (w - 1 - x, h - 1 - y), _ => (y, w - 1 - x) };
                let from = (y * w + x) * bpp; let to = (dy * nw + dx) * bpp;
                bytes[to..to + bpp].copy_from_slice(&plane.bytes[from..from + bpp]);
            }}
            plane.width = nw as u32; plane.height = nh as u32; plane.bytes = bytes;
        }
    }
}
pub(super) fn camera_frame(raw: &CameraFrameData<'_>) -> Result<Frame, String> {
    let mut planes = Vec::new();
    for (i, p) in raw.planes.iter().enumerate() {
        let (w, h) = if i == 0 { (raw.width, raw.height) } else { (raw.width.div_ceil(2), raw.height.div_ceil(2)) };
        let mut bytes = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h as usize { for x in 0..w as usize {
            let at = y * p.row_stride as usize + x * p.pixel_stride as usize;
            bytes.push(*p.bytes.get(at).ok_or("camera plane is shorter than its strides")?);
        }}
        planes.push(Plane { width: w, height: h, format: format::R8, bytes });
    }
    Ok(Frame { planes, yuv: yuv::ENABLED, matrix: raw.matrix as f32 })
}
pub(super) fn jpeg_frame(bytes: &[u8]) -> Result<Frame, String> {
    // The platform Bitmap decoder preserves the old JPEG camera format without
    // adding a dependency or building the Linux JPEG/backend modules on Android.
    java(|env, _| unsafe {
        let data = byte_array(env, bytes)?;
        let cls = ((**env).FindClass.unwrap())(env, c"android/graphics/BitmapFactory".as_ptr());
        exception(env)?;
        let id = ((**env).GetStaticMethodID.unwrap())(env, cls, c"decodeByteArray".as_ptr(), c"([BII)Landroid/graphics/Bitmap;".as_ptr());
        exception(env)?;
        if id.is_null() { return Err("BitmapFactory decoder unavailable".into()); }
        let bitmap = ((**env).CallStaticObjectMethodA.unwrap())(env, cls, id, [jvalue { l: data }, jvalue { i: 0 }, jvalue { i: bytes.len() as i32 }].as_ptr());
        exception(env)?;
        if bitmap.is_null() { return Err("Android camera JPEG decode failed".into()); }
        let result = (|| {
            let w = int(env, bitmap, c"getWidth", c"()I", &[])?;
            let h = int(env, bitmap, c"getHeight", c"()I", &[])?;
            let count = w.checked_mul(h).filter(|n| *n > 0).ok_or("invalid JPEG extent")?;
            let pixels = ((**env).NewIntArray.unwrap())(env, count);
            exception(env)?;
            void(env, bitmap, c"getPixels", c"([IIIIIII)V", &[jvalue { l: pixels }, jvalue { i: 0 }, jvalue { i: w }, jvalue { i: 0 }, jvalue { i: 0 }, jvalue { i: w }, jvalue { i: h }])?;
            let mut argb = vec![0i32; count as usize];
            ((**env).GetIntArrayRegion.unwrap())(env, pixels, 0, count, argb.as_mut_ptr());
            exception(env)?;
            let mut bytes = Vec::with_capacity(count as usize * 4);
            for p in argb { bytes.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]); }
            Ok(Frame { planes: vec![Plane { width: w as u32, height: h as u32, format: format::RGBA8, bytes }], yuv: 0, matrix: 0. })
        })();
        let release = void(env, bitmap, c"recycle", c"()V", &[]);
        release.and(result)
    })
}
fn descriptor(fmt: u32, w: u32, h: u32) -> TextureObj {
    TextureObj { format: fmt, w, h, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: 0, imported: true, uploaded: 0, lent: None }
}
fn row(s: &mut Session, id: Id, desc: &TextureObj) {
    if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index).filter(|row| row.gen == id.gen) {
        row.bytes = desc.bytes; row.width = desc.w; row.height = desc.h; row.format = desc.format;
    } else {
        s.ledger.add(LedgerEntry { kind: Kind::Texture, detail: "Android media frame", index: id.index, gen: id.gen, bytes: desc.bytes, format: desc.format, width: desc.w, height: desc.h, mips: 1, attr: Attr::default(), created_ns: crate::clock_ns(), created_frame: s.frame, last_used_frame: 0, last_used_ns: 0 });
    }
}
pub(super) fn declare(s: &mut Session, ids: [Id; 4]) -> Result<(), String> {
    for (id, fmt) in ids.into_iter().zip([format::RGBA8, format::R8, format::R8, format::R8]) {
        if id.is_none() || s.textures.get(s.id, id).is_ok() { continue; }
        let desc = descriptor(fmt, 1, 1);
        s.textures.insert(s.id, id, s.id, desc.clone()).map_err(|_| "stale video texture declaration")?;
        row(s, id, &desc);
    }
    Ok(())
}
pub(super) fn present(gpu: &mut Vulkan, s: &mut Session, ids: [Id; 4], frame: &Frame) -> Result<bool, String> {
    if frame.planes.is_empty() { return Ok(true); } // audio-only clock update
    let targets = if frame.yuv & yuv::ENABLED != 0 { &ids[1..] } else { &ids[..1] };
    if targets.len() != frame.planes.len() { return Err("video plane/texture count differs".into()); }
    if targets.iter().any(|id| s.textures.get(s.id, *id).is_err()) { return Ok(false); }
    for (id, plane) in targets.iter().zip(&frame.planes) {
        if plane.width == 0 || plane.height == 0 || plane.bytes.len() as u64 != format::level_bytes(plane.format, plane.width, plane.height) { return Err("invalid video plane extent/bytes".into()); }
        let current = s.textures.get(s.id, *id).map_err(|_| "stale video texture")?.val.clone();
        let mut desc = descriptor(plane.format, plane.width, plane.height);
        if current.bytes == 0 || current.format != desc.format || current.w != desc.w || current.h != desc.h {
            gpu.texture_create(s, *id, &mut desc)?;
        } else { desc.bytes = current.bytes; }
        // Vulkan's upload resolves the image using the session's extent. Publish
        // the new descriptor before upload, including on the first 1x1 -> frame
        // resize; otherwise an imported video placeholder cannot be resolved.
        s.textures.get_mut(s.id, *id).map_err(|_| "stale video texture")?.val = desc.clone();
        row(s, *id, &desc);
        let bad = s.stats.bad_args;
        gpu.texture_update(s, *id, &desc, 0, [0, 0, desc.w, desc.h], 0, &plane.bytes);
        if s.stats.bad_args != bad { return Err("Vulkan video staging failed".into()); }
        desc.uploaded = plane.bytes.len() as u64;
        s.textures.get_mut(s.id, *id).map_err(|_| "stale video texture")?.val = desc.clone();
    }
    Ok(true)
}
pub(super) fn release(gpu: &mut Vulkan, s: &mut Session, ids: [Id; 4]) {
    for id in ids {
        if id.is_none() || s.textures.get(s.id, id).is_err() { continue; }
        gpu.texture_destroy(s.id, id);
        if let Ok(entry) = s.textures.get_mut(s.id, id) { entry.val.bytes = 0; entry.val.uploaded = 0; entry.val.imported = true; }
        if let Some(row) = s.ledger.get_mut(Kind::Texture, id.index).filter(|row| row.gen == id.gen) { row.bytes = 0; }
    }
}

pub(super) enum Command { Prepare(Ticket, u16, Arc<[u8]>), Control(Ticket, u32, u64) }
enum Notice { Prepared(u32, u32, u64, Global), Completed, Released, Error(String) }
type Notices = Arc<Mutex<Vec<(u64, Notice)>>>;
#[derive(Clone)]
struct Route { notices: Notices, thread: Thread }
fn routes() -> &'static Mutex<HashMap<u64, Route>> {
    static ROUTES: OnceLock<Mutex<HashMap<u64, Route>>> = OnceLock::new();
    ROUTES.get_or_init(Default::default)
}
struct Decoder {
    player: Global,
    surface: Global,
    transform: Global,
    update: usize,
    position: usize,
    matrix: usize,
    video_tracks: Vec<i32>,
    audio_tracks: Vec<i32>,
}
struct Player {
    ticket: Ticket,
    texture: u32,
    decoder: Option<Decoder>,
    owner: Option<Global>,
    size: [u32; 2],
    audio_only: bool,
    closing: bool,
    failed: bool,
    controls: VecDeque<(u32, u64)>,
    last_position: Option<u64>,
}
pub(super) struct Video { players: HashMap<u64, Player>, egl: Option<Egl>, notices: Notices, output: Output }
impl Engine for Video {
    type Command = Command;
    fn new(output: Output) -> Self { Self { players: HashMap::new(), egl: None, notices: Arc::new(Mutex::new(Vec::new())), output } }
    fn command(&mut self, command: Command) {
        match command {
            Command::Prepare(ticket, mode, source) => {
                if let Err(e) = self.prepare(ticket, mode, &source) {
                    self.output.push(Message::Status(ticket, video_status::ERROR, e));
                    self.close(ticket.token);
                }
            }
            Command::Control(ticket, op, arg) => {
                let Some(player) = self.players.get_mut(&ticket.token) else {
                    if op == video_op::CLEANUP { self.output.push(Message::Status(ticket, video_status::RELEASED, String::new())); }
                    return;
                };
                if op == video_op::CLEANUP {
                    // prepareAsync owns its Java object until its prepared/error
                    // callback. Defer cleanup to that callback if still preparing.
                    player.closing = true;
                    if player.decoder.is_some() { self.close(ticket.token); }
                } else if !player.closing {
                    if player.decoder.is_none() { player.controls.push_back((op, arg)); }
                    else if let Err(e) = player.control(op, arg) { self.output.push(Message::Status(ticket, video_status::ERROR, e)); }
                }
            }
        }
    }
    fn step(&mut self) {
        let notices = std::mem::take(&mut *lock(&self.notices));
        for (token, notice) in notices {
            let Some(player) = self.players.get_mut(&token) else { continue; };
            match notice {
                Notice::Prepared(w, h, duration, object) => {
                    player.size = [w, h];
                    let info = Decoder::new(object);
                    match info {
                        Ok((decoder, vt, at)) => {
                            player.decoder = Some(decoder);
                            if player.closing { self.close(token); continue; }
                            self.output.push(Message::Prepared(player.ticket, w, h, duration, duration != 0, vt, at));
                            self.output.push(Message::Ranges(player.ticket, ranges::SEEKABLE, if duration == 0 { Vec::new() } else { vec![(0., duration as f64 / 1000.)] }));
                            while let Some((op, arg)) = player.controls.pop_front() {
                                if let Err(e) = player.control(op, arg) { self.output.push(Message::Status(player.ticket, video_status::ERROR, e)); }
                            }
                        }
                        Err(e) => { self.output.push(Message::Status(player.ticket, video_status::ERROR, e)); player.failed = true; self.close(token); }
                    }
                }
                Notice::Completed => self.output.push(Message::Status(player.ticket, video_status::COMPLETED, String::new())),
                Notice::Released => { self.retire(token); }
                Notice::Error(e) => { self.output.push(Message::Status(player.ticket, video_status::ERROR, e)); player.failed = true; self.close(token); }
            }
        }
        let Some(egl) = &mut self.egl else { return; };
        for player in self.players.values_mut().filter(|p| !p.closing && p.decoder.is_some()) {
            match player.poll(egl) {
                Ok(Some((position, frame))) => self.output.push(Message::Frame(player.ticket, position, Arc::new(frame))),
                Ok(None) => {},
                Err(e) => { self.output.push(Message::Status(player.ticket, video_status::ERROR, e)); player.closing = true; player.failed = true; }
            }
        }
        let failed: Vec<_> = self.players.iter().filter(|(_, p)| p.closing && p.decoder.is_some()).map(|(token, _)| *token).collect();
        for token in failed { self.close(token); }
    }
    fn active(&self) -> bool { !self.players.is_empty() }
}
impl Video {
    fn prepare(&mut self, ticket: Ticket, mode: u16, source: &[u8]) -> Result<(), String> {
        if self.egl.is_none() { self.egl = Some(Egl::new()?); }
        let texture = self.egl.as_ref().unwrap().texture()?;
        lock(routes()).insert(ticket.token, Route { notices: self.notices.clone(), thread: std::thread::current() });
        self.players.insert(ticket.token, Player { ticket, texture, decoder: None, owner: None, size: [0, 0], audio_only: mode & video_mode::AUDIO_ONLY != 0, closing: false, failed: false, controls: VecDeque::new(), last_position: None });
        let result = java(|env, activity| unsafe {
            let source = match (mode & video_mode::SRC_MASK) >> video_mode::SRC_SHIFT {
                src_kind::MEMORY => byte_array(env, source)?,
                src_kind::NETWORK | src_kind::FILE => string(env, std::str::from_utf8(source).map_err(|_| "invalid UTF-8 video source")?)?,
                _ => return Err("unknown Android video source kind".into()),
            };
            void(env, activity, c"prepareVideoPlayback", c"(JLjava/lang/Object;IZZ)V", &[jvalue { j: ticket.token as i64 }, jvalue { l: source }, jvalue { i: texture as i32 }, jvalue { z: (mode & video_mode::AUTOPLAY != 0) as u8 }, jvalue { z: (mode & video_mode::LOOP != 0) as u8 }])?;
            // Retain the Java owner even when prepareAsync fails before Prepared.
            let map = field(env, activity, c"mVideoPlayerRunnables", c"Ljava/util/HashMap;")?;
            let cls = ((**env).FindClass.unwrap())(env, c"java/lang/Long".as_ptr());
            exception(env)?;
            let value_of = ((**env).GetStaticMethodID.unwrap())(env, cls, c"valueOf".as_ptr(), c"(J)Ljava/lang/Long;".as_ptr());
            exception(env)?;
            if value_of.is_null() { return Err("Java Long.valueOf unavailable".into()); }
            let key = ((**env).CallStaticObjectMethodA.unwrap())(env, cls, value_of, [jvalue { j: ticket.token as i64 }].as_ptr());
            exception(env)?;
            let get = method(env, map, c"get", c"(Ljava/lang/Object;)Ljava/lang/Object;")?;
            let runnable = ((**env).CallObjectMethodA.unwrap())(env, map, get, [jvalue { l: key }].as_ptr());
            exception(env)?;
            if runnable.is_null() { return Err("Java video runnable disappeared during prepare".into()); }
            Global::new(env, field(env, runnable, c"mVideoPlayer", c"Ldev/makepad/android/VideoPlayer;")?)
        });
        match result {
            Ok(owner) => { self.players.get_mut(&ticket.token).unwrap().owner = Some(owner); Ok(()) }
            Err(e) => Err(e),
        }
    }
    fn close(&mut self, token: u64) {
        let Some(player) = self.players.get(&token) else { return; };
        if player.failed {
            // MediaPlayer.stop throws in the Error/Idle states. Release that
            // object first, so the Activity's normal cleanup still releases its
            // SurfaceTexture, handler and runnable and reports Released.
            if let Some(owner) = &player.owner {
                if let Err(e) = java(|env, _| unsafe {
                    let object = owner.object();
                    let cls = ((**env).GetObjectClass.unwrap())(env, object);
                    exception(env)?;
                    let field = ((**env).GetFieldID.unwrap())(env, cls, c"mMediaPlayer".as_ptr(), c"Landroid/media/MediaPlayer;".as_ptr());
                    exception(env)?;
                    if field.is_null() { return Err("Java MediaPlayer field unavailable".into()); }
                    let media = ((**env).GetObjectField.unwrap())(env, object, field);
                    exception(env)?;
                    if !media.is_null() {
                        void(env, media, c"release", c"()V", &[])?;
                        ((**env).SetObjectField.unwrap())(env, object, field, std::ptr::null_mut());
                        exception(env)?;
                    }
                    Ok(())
                }) { eprintln!("[hal-android-video] release failed decoder: {e}"); }
            }
        }
        // stopAndCleanup emits Released on the UI looper. Retire locally after
        // synchronous cleanup too, so a dead Activity cannot strand a HAL player.
        if let Err(e) = java(|env, activity| unsafe { void(env, activity, c"cleanupVideoPlaybackResources", c"(J)V", &[jvalue { j: token as i64 }]) }) { eprintln!("[hal-android-video] cleanup: {e}"); }
        self.retire(token);
    }
    fn retire(&mut self, token: u64) {
        lock(routes()).remove(&token);
        if let Some(p) = self.players.remove(&token) {
            unsafe { gl::glDeleteTextures(1, &p.texture); }
            self.output.push(Message::Status(p.ticket, video_status::RELEASED, String::new()));
        }
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        for player in self.players.values_mut() { player.closing = true; }
        // Never destroy the OES texture/context while Java is still preparing
        // a producer for it. This worker can wait for its callback; the HAL
        // session already detached its mailbox and never joins this thread.
        while !self.players.is_empty() {
            self.step();
            if !self.players.is_empty() { std::thread::park_timeout(std::time::Duration::from_millis(8)); }
        }
    }
}
impl Decoder {
    fn new(player: Global) -> Result<(Self, Vec<String>, Vec<String>), String> {
        java(|env, _| unsafe {
            let object = player.object();
            let surface = Global::new(env, field(env, object, c"mSurfaceTexture", c"Landroid/graphics/SurfaceTexture;")?)?;
            let array = ((**env).NewFloatArray.unwrap())(env, 16);
            exception(env)?;
            let transform = Global::new(env, array)?;
            let update = method(env, object, c"maybeUpdateTexImage", c"()Z")? as usize;
            let position = method(env, object, c"getCurrentPositionMs", c"()J")? as usize;
            let matrix = method(env, surface.object(), c"getTransformMatrix", c"([F)V")? as usize;
            let media_player = field(env, object, c"mMediaPlayer", c"Landroid/media/MediaPlayer;")?;
            let (video_tracks, audio_tracks, video_names, audio_names) = tracks(env, media_player)?;
            Ok((Self { player, surface, transform, update, position, matrix, video_tracks, audio_tracks }, video_names, audio_names))
        })
    }
}
impl Player {
    fn control(&mut self, op: u32, arg: u64) -> Result<(), String> {
        java(|env, activity| unsafe {
            let token = jvalue { j: self.ticket.token as i64 };
            match op {
                video_op::PLAY => void(env, activity, c"beginVideoPlayback", c"(J)V", &[token]),
                video_op::PAUSE => void(env, activity, c"pauseVideoPlayback", c"(J)V", &[token]),
                video_op::RESUME => void(env, activity, c"resumeVideoPlayback", c"(J)V", &[token]),
                video_op::MUTE => void(env, activity, c"muteVideoPlayback", c"(J)V", &[token]),
                video_op::UNMUTE => void(env, activity, c"unmuteVideoPlayback", c"(J)V", &[token]),
                video_op::SEEK => void(env, activity, c"seekVideoPlayback", c"(JJ)V", &[token, jvalue { j: arg.min(i32::MAX as u64) as i64 }]),
                video_op::RATE => {
                    let rate = f32::from_bits(arg as u32);
                    if !rate.is_finite() || rate <= 0. { return Err("invalid video playback rate".into()); }
                    void(env, activity, c"setVideoPlaybackRate", c"(JD)V", &[token, jvalue { d: rate as f64 }])
                }
                video_op::VOLUME | video_op::SELECT_VIDEO_TRACK | video_op::SELECT_AUDIO_TRACK => {
                    let decoder = self.decoder.as_ref().ok_or("video not prepared")?;
                    let player = field(env, decoder.player.object(), c"mMediaPlayer", c"Landroid/media/MediaPlayer;")?;
                    if op == video_op::VOLUME {
                        let volume = f32::from_bits(arg as u32);
                        if !volume.is_finite() { return Err("invalid video volume".into()); }
                        let volume = volume.clamp(0., 1.);
                        void(env, player, c"setVolume", c"(FF)V", &[jvalue { f: volume }, jvalue { f: volume }])
                    } else {
                        let tracks = if op == video_op::SELECT_VIDEO_TRACK { &decoder.video_tracks } else { &decoder.audio_tracks };
                        let index = *tracks.get(arg as usize).ok_or("video track index is out of range")?;
                        void(env, player, c"selectTrack", c"(I)V", &[jvalue { i: index }])
                    }
                }
                _ => Err(format!("unknown Android video control {op}")),
            }
        })
    }
    fn poll(&mut self, egl: &mut Egl) -> Result<Option<(u64, Frame)>, String> {
        let decoder = self.decoder.as_ref().ok_or("video not prepared")?;
        java(|env, _| unsafe {
            let updated = ((**env).CallBooleanMethodA.unwrap())(env, decoder.player.object(), decoder.update as _, std::ptr::null());
            exception(env)?;
            if updated == 0 && !self.audio_only && self.size != [0, 0] { return Ok(None); }
            let position = ((**env).CallLongMethodA.unwrap())(env, decoder.player.object(), decoder.position as _, std::ptr::null());
            exception(env)?;
            let position = position.max(0) as u64;
            if self.audio_only || self.size == [0, 0] {
                if self.last_position == Some(position) { return Ok(None); }
                self.last_position = Some(position);
                return Ok(Some((position, Frame { planes: Vec::new(), yuv: 0, matrix: 0. })));
            }
            let mut matrix = [0f32; 16];
            ((**env).CallVoidMethodA.unwrap())(env, decoder.surface.object(), decoder.matrix as _, [jvalue { l: decoder.transform.object() }].as_ptr());
            exception(env)?;
            ((**env).GetFloatArrayRegion.unwrap())(env, decoder.transform.object(), 0, 16, matrix.as_mut_ptr());
            exception(env)?;
            let bytes = egl.read(self.texture, self.size, &matrix)?;
            Ok(Some((position, Frame { planes: vec![Plane { width: self.size[0], height: self.size[1], format: format::RGBA8, bytes }], yuv: 0, matrix: 0. })))
        })
    }
}
unsafe fn int(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr, args: &[jvalue]) -> Result<i32, String> {
    let id = method(env, object, name, sig)?;
    let result = ((**env).CallIntMethodA.unwrap())(env, object, id, args.as_ptr());
    exception(env)?; Ok(result)
}
unsafe fn byte_array(env: *mut JNIEnv, bytes: &[u8]) -> Result<jbyteArray, String> {
    let len = i32::try_from(bytes.len()).map_err(|_| "media exceeds Java byte array size")?;
    let array = ((**env).NewByteArray.unwrap())(env, len);
    exception(env)?;
    if array.is_null() { return Err("Java byte array allocation failed".into()); }
    ((**env).SetByteArrayRegion.unwrap())(env, array, 0, len, bytes.as_ptr().cast());
    exception(env)?; Ok(array)
}
unsafe fn tracks(env: *mut JNIEnv, player: jobject) -> Result<(Vec<i32>, Vec<i32>, Vec<String>, Vec<String>), String> {
    let id = method(env, player, c"getTrackInfo", c"()[Landroid/media/MediaPlayer$TrackInfo;")?;
    let array = ((**env).CallObjectMethodA.unwrap())(env, player, id, std::ptr::null());
    exception(env)?;
    let mut video = Vec::new(); let mut audio = Vec::new(); let mut vn = Vec::new(); let mut an = Vec::new();
    if !array.is_null() {
        let count = ((**env).GetArrayLength.unwrap())(env, array);
        for i in 0..count {
            let track = ((**env).GetObjectArrayElement.unwrap())(env, array, i);
            exception(env)?;
            let kind = int(env, track, c"getTrackType", c"()I", &[])?;
            let id = method(env, track, c"getLanguage", c"()Ljava/lang/String;")?;
            let language = ((**env).CallObjectMethodA.unwrap())(env, track, id, std::ptr::null());
            exception(env)?;
            let lang = if language.is_null() { String::new() } else { reg::string(env, language) };
            match kind { 1 => { video.push(i); vn.push(format!("Video {} {lang}", video.len())); }, 2 => { audio.push(i); an.push(format!("Audio {} {lang}", audio.len())); }, _ => {} }
            if !language.is_null() { ((**env).DeleteLocalRef.unwrap())(env, language); }
            ((**env).DeleteLocalRef.unwrap())(env, track);
        }
    }
    Ok((video, audio, vn, an))
}
fn notice(token: u64, notice: Notice) {
    let route = lock(routes()).get(&token).cloned();
    if let Some(route) = route { lock(&route.notices).push((token, notice)); route.thread.unpark(); }
}
unsafe extern "system" fn prepared(env: *mut JNIEnv, _: jclass, token: jlong, w: jint, h: jint, duration: jlong, player: jobject) {
    match Global::new(env, player) {
        Ok(player) => notice(token as u64, Notice::Prepared(w.max(0) as u32, h.max(0) as u32, duration.max(0) as u64, player)),
        Err(e) => notice(token as u64, Notice::Error(e)),
    }
}
unsafe extern "system" fn completed(_: *mut JNIEnv, _: jclass, token: jlong) { notice(token as u64, Notice::Completed); }
unsafe extern "system" fn released(_: *mut JNIEnv, _: jclass, token: jlong) { notice(token as u64, Notice::Released); }
unsafe extern "system" fn decoding_error(env: *mut JNIEnv, _: jclass, token: jlong, error: jstring) { notice(token as u64, Notice::Error(reg::string(env, error))); }
pub(super) fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
    vec![
        ("onVideoPlaybackPrepared", "(JIIJLdev/makepad/android/VideoPlayer;)V", prepared as *mut c_void),
        ("onVideoPlaybackCompleted", "(J)V", completed as *mut c_void),
        ("onVideoPlayerReleased", "(J)V", released as *mut c_void),
        ("onVideoDecodingError", "(JLjava/lang/String;)V", decoding_error as *mut c_void),
    ]
}

struct Egl { display: *mut c_void, context: *mut c_void, surface: *mut c_void, program: u32, fbo: u32, color: u32, size: [u32; 2], transform: i32 }
impl Egl {
    fn new() -> Result<Self, String> { unsafe {
        let mut e = Self { display: std::ptr::null_mut(), context: std::ptr::null_mut(), surface: std::ptr::null_mut(), program: 0, fbo: 0, color: 0, size: [0; 2], transform: -1 };
        e.display = gl::eglGetDisplay(std::ptr::null_mut());
        if e.display.is_null() || gl::eglInitialize(e.display, std::ptr::null_mut(), std::ptr::null_mut()) == 0 { return Err("EGL media display initialization failed".into()); }
        if gl::eglBindAPI(0x30a0) == 0 { return Err("EGL media ES API unavailable".into()); }
        let attrs = [0x3033, 1, 0x3040, 4, 0x3024, 8, 0x3023, 8, 0x3022, 8, 0x3021, 8, 0x3038];
        let mut config = std::ptr::null_mut(); let mut count = 0;
        if gl::eglChooseConfig(e.display, attrs.as_ptr(), &mut config, 1, &mut count) == 0 || count == 0 { return Err("EGL media pbuffer config unavailable".into()); }
        e.context = gl::eglCreateContext(e.display, config, std::ptr::null_mut(), [0x3098, 2, 0x3038].as_ptr());
        e.surface = gl::eglCreatePbufferSurface(e.display, config, [0x3057, 1, 0x3056, 1, 0x3038].as_ptr());
        if e.context.is_null() || e.surface.is_null() || gl::eglMakeCurrent(e.display, e.surface, e.surface, e.context) == 0 { return Err("EGL media context creation failed".into()); }
        let vertex = shader(0x8b31, c"attribute vec2 pos; uniform mat4 transform; varying vec2 uv; void main(){gl_Position=vec4(pos,0.,1.); uv=(transform*vec4((pos.x+1.)*.5,(1.-pos.y)*.5,0.,1.)).xy;}")?;
        let fragment = match shader(0x8b30, c"#extension GL_OES_EGL_image_external : require\nprecision mediump float; uniform samplerExternalOES image; varying vec2 uv; void main(){gl_FragColor=texture2D(image,uv);}") {
            Ok(fragment) => fragment, Err(err) => { gl::glDeleteShader(vertex); return Err(err); }
        };
        e.program = gl::glCreateProgram();
        gl::glAttachShader(e.program, vertex); gl::glAttachShader(e.program, fragment);
        gl::glBindAttribLocation(e.program, 0, c"pos".as_ptr());
        gl::glLinkProgram(e.program);
        gl::glDeleteShader(vertex); gl::glDeleteShader(fragment);
        let mut ok = 0; gl::glGetProgramiv(e.program, 0x8b82, &mut ok);
        if ok == 0 { return Err(format!("OES media program link failed: {}", program_log(e.program))); }
        e.transform = gl::glGetUniformLocation(e.program, c"transform".as_ptr());
        gl::glUseProgram(e.program);
        gl::glUniform1i(gl::glGetUniformLocation(e.program, c"image".as_ptr()), 0);
        gl::glGenFramebuffers(1, &mut e.fbo); gl::glGenTextures(1, &mut e.color);
        gl_check("create OES readback renderer")?;
        Ok(e)
    }}
    fn texture(&self) -> Result<u32, String> { unsafe {
        let mut texture = 0; gl::glGenTextures(1, &mut texture);
        gl::glBindTexture(0x8d65, texture); texture_params(0x8d65);
        if let Err(e) = gl_check("create OES texture") { gl::glDeleteTextures(1, &texture); return Err(e); }
        Ok(texture)
    }}
    fn read(&mut self, texture: u32, size: [u32; 2], transform: &[f32; 16]) -> Result<Vec<u8>, String> { unsafe {
        let [w, h] = size;
        if w == 0 || h == 0 || w > i32::MAX as u32 || h > i32::MAX as u32 { return Err("invalid video readback extent".into()); }
        gl::glBindFramebuffer(0x8d40, self.fbo);
        if self.size != size {
            gl::glBindTexture(0x0de1, self.color); texture_params(0x0de1);
            gl::glTexImage2D(0x0de1, 0, 0x1908, w as i32, h as i32, 0, 0x1908, 0x1401, std::ptr::null());
            gl::glFramebufferTexture2D(0x8d40, 0x8ce0, 0x0de1, self.color, 0);
            if gl::glCheckFramebufferStatus(0x8d40) != 0x8cd5 { return Err("OES readback framebuffer is incomplete".into()); }
            self.size = size;
        }
        gl::glViewport(0, 0, w as i32, h as i32);
        gl::glUseProgram(self.program); gl::glActiveTexture(0x84c0); gl::glBindTexture(0x8d65, texture);
        gl::glUniformMatrix4fv(self.transform, 1, 0, transform.as_ptr());
        let quad: [f32; 8] = [-1., -1., 1., -1., -1., 1., 1., 1.];
        gl::glBindBuffer(0x8892, 0); gl::glEnableVertexAttribArray(0);
        gl::glVertexAttribPointer(0, 2, 0x1406, 0, 0, quad.as_ptr().cast());
        gl::glDrawArrays(0x0005, 0, 4);
        let len = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).ok_or("video readback size overflow")?;
        let mut bytes = vec![0u8; len];
        // The vertex shader flips output Y, so readPixels row zero is the
        // image's top row after SurfaceTexture's crop/transform has been applied.
        gl::glPixelStorei(0x0d05, 1);
        gl::glReadPixels(0, 0, w as i32, h as i32, 0x1908, 0x1401, bytes.as_mut_ptr().cast());
        gl_check("read OES frame")?;
        Ok(bytes)
    }}
}
impl Drop for Egl {
    fn drop(&mut self) { unsafe {
        if !self.context.is_null() && !self.surface.is_null() {
            gl::eglMakeCurrent(self.display, self.surface, self.surface, self.context);
            if self.program != 0 { gl::glDeleteProgram(self.program); }
            gl::glDeleteFramebuffers(1, &self.fbo); gl::glDeleteTextures(1, &self.color);
        }
        if !self.display.is_null() {
            gl::eglMakeCurrent(self.display, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if !self.surface.is_null() { gl::eglDestroySurface(self.display, self.surface); }
            if !self.context.is_null() { gl::eglDestroyContext(self.display, self.context); }
            // EGL_DEFAULT_DISPLAY is process-shared. eglTerminate here could
            // invalidate another session's decoder, so release only our objects.
        }
        gl::eglReleaseThread();
    }}
}
unsafe fn texture_params(target: u32) {
    for (parameter, value) in [(0x2801, 0x2601), (0x2800, 0x2601), (0x2802, 0x812f), (0x2803, 0x812f)] { gl::glTexParameteri(target, parameter, value); }
}
unsafe fn shader(kind: u32, source: &CStr) -> Result<u32, String> {
    let shader = gl::glCreateShader(kind);
    gl::glShaderSource(shader, 1, &source.as_ptr(), std::ptr::null()); gl::glCompileShader(shader);
    let mut ok = 0; gl::glGetShaderiv(shader, 0x8b81, &mut ok);
    if ok == 0 {
        let mut len = 0; gl::glGetShaderiv(shader, 0x8b84, &mut len);
        let mut bytes = vec![0u8; len.max(1) as usize];
        gl::glGetShaderInfoLog(shader, bytes.len() as i32, std::ptr::null_mut(), bytes.as_mut_ptr().cast());
        gl::glDeleteShader(shader);
        return Err(format!("OES media shader failed: {}", String::from_utf8_lossy(&bytes)));
    }
    Ok(shader)
}
unsafe fn program_log(program: u32) -> String {
    let mut len = 0; gl::glGetProgramiv(program, 0x8b84, &mut len);
    let mut bytes = vec![0u8; len.max(1) as usize];
    gl::glGetProgramInfoLog(program, bytes.len() as i32, std::ptr::null_mut(), bytes.as_mut_ptr().cast());
    String::from_utf8_lossy(&bytes).into_owned()
}
unsafe fn gl_check(operation: &str) -> Result<(), String> {
    let error = gl::glGetError();
    if error == 0 { Ok(()) } else { Err(format!("{operation}: GL error 0x{error:x}")) }
}
mod gl {
    use super::{c_char, c_void};
    #[link(name = "EGL")]
    extern "C" {
        pub fn eglGetDisplay(native: *mut c_void) -> *mut c_void;
        pub fn eglInitialize(display: *mut c_void, major: *mut i32, minor: *mut i32) -> u32;
        pub fn eglBindAPI(api: u32) -> u32;
        pub fn eglChooseConfig(display: *mut c_void, attrs: *const i32, configs: *mut *mut c_void, size: i32, count: *mut i32) -> u32;
        pub fn eglCreateContext(display: *mut c_void, config: *mut c_void, share: *mut c_void, attrs: *const i32) -> *mut c_void;
        pub fn eglCreatePbufferSurface(display: *mut c_void, config: *mut c_void, attrs: *const i32) -> *mut c_void;
        pub fn eglMakeCurrent(display: *mut c_void, draw: *mut c_void, read: *mut c_void, context: *mut c_void) -> u32;
        pub fn eglDestroySurface(display: *mut c_void, surface: *mut c_void) -> u32;
        pub fn eglDestroyContext(display: *mut c_void, context: *mut c_void) -> u32;
        pub fn eglReleaseThread() -> u32;
    }
    #[link(name = "GLESv2")]
    extern "C" {
        pub fn glGetError() -> u32;
        pub fn glGenTextures(n: i32, textures: *mut u32);
        pub fn glDeleteTextures(n: i32, textures: *const u32);
        pub fn glBindTexture(target: u32, texture: u32);
        pub fn glTexParameteri(target: u32, parameter: u32, value: i32);
        pub fn glTexImage2D(target: u32, level: i32, format: i32, width: i32, height: i32, border: i32, pixel_format: u32, pixel_type: u32, pixels: *const c_void);
        pub fn glActiveTexture(texture: u32);
        pub fn glGenFramebuffers(n: i32, buffers: *mut u32);
        pub fn glDeleteFramebuffers(n: i32, buffers: *const u32);
        pub fn glBindFramebuffer(target: u32, buffer: u32);
        pub fn glFramebufferTexture2D(target: u32, attachment: u32, texture_target: u32, texture: u32, level: i32);
        pub fn glCheckFramebufferStatus(target: u32) -> u32;
        pub fn glViewport(x: i32, y: i32, width: i32, height: i32);
        pub fn glCreateShader(kind: u32) -> u32;
        pub fn glShaderSource(shader: u32, count: i32, strings: *const *const c_char, lengths: *const i32);
        pub fn glCompileShader(shader: u32);
        pub fn glGetShaderiv(shader: u32, parameter: u32, value: *mut i32);
        pub fn glGetShaderInfoLog(shader: u32, size: i32, length: *mut i32, log: *mut c_char);
        pub fn glDeleteShader(shader: u32);
        pub fn glCreateProgram() -> u32;
        pub fn glAttachShader(program: u32, shader: u32);
        pub fn glBindAttribLocation(program: u32, index: u32, name: *const c_char);
        pub fn glLinkProgram(program: u32);
        pub fn glGetProgramiv(program: u32, parameter: u32, value: *mut i32);
        pub fn glGetProgramInfoLog(program: u32, size: i32, length: *mut i32, log: *mut c_char);
        pub fn glDeleteProgram(program: u32);
        pub fn glUseProgram(program: u32);
        pub fn glGetUniformLocation(program: u32, name: *const c_char) -> i32;
        pub fn glUniform1i(location: i32, value: i32);
        pub fn glUniformMatrix4fv(location: i32, count: i32, transpose: u8, value: *const f32);
        pub fn glBindBuffer(target: u32, buffer: u32);
        pub fn glEnableVertexAttribArray(index: u32);
        pub fn glVertexAttribPointer(index: u32, size: i32, kind: u32, normalized: u8, stride: i32, pointer: *const c_void);
        pub fn glDrawArrays(mode: u32, first: i32, count: i32);
        pub fn glPixelStorei(parameter: u32, value: i32);
        pub fn glReadPixels(x: i32, y: i32, width: i32, height: i32, format: u32, kind: u32, pixels: *mut c_void);
    }
}
