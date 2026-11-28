//! Human feedback for a Studio-launched standalone app. The UI only queues
//! commands; one worker captures this app's drawable and commits a spool event.

use crate::attach::{self, PictureError, PreparedPicture, Source};
use makepad_widgets::ai_slot::{AiSelectedRegion, AiSlotRequests};
use makepad_widgets::widget_hooks::{TweakDiffEntry, WidgetHooks};
use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::makepad_platform::{
    screen_capture,
    thread::{SignalToUI, TaskHandle, ThreadOptions},
};
use makepad_widgets::*;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.StudioAppFeedback = #(StudioAppFeedback::register_widget(vm)){
        width: Fill height: Fit flow: Down spacing: 6
        padding: Inset{left: 12 right: 12 top: 10 bottom: 10}
        show_bg: true draw_bg +: {color: theme.color_bg_container}
        Label{text: "Send feedback to this flow's AI" draw_text +: {text_style: theme.font_bold{font_size: 10}}}
        feedback_message := TextInput{
            width: Fill height: 66 is_multiline: true
            empty_text: "What should change in this app?"
        }
        View{width: Fill height: Fit flow: Right spacing: 5
            feedback_region := Button{text: "Select area → AI"}
            feedback_submit := Button{text: "Send feedback"}
        }
        feedback_preview := Image{width: Fill height: 90 visible: false fit: ImageFit.Smallest}
        feedback_status := Label{
            width: Fill height: Fit text: "Ctrl+Shift+F10 selects an app area. Drop an image here to send it to the AI."
            draw_text +: {text_style: theme.font_regular{font_size: 9} color: theme.color_text_meta wrap: Words}
        }
        Label{
            width: Fill height: Fit text: "Close this app when you are ready for the next revision."
            draw_text +: {text_style: theme.font_regular{font_size: 8.5} color: theme.color_text_meta wrap: Words}
        }
    }
}

const MAX_MESSAGE: usize = 4 * 1024;

pub(crate) fn enabled_from_env() -> bool {
    Config::from_env().is_some()
}

struct Config {
    dir: PathBuf,
    flow: String,
    artifact: String,
    run: String,
    revision: String,
}

impl Config {
    fn from_env() -> Option<Self> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let dir = PathBuf::from(std::env::var_os("MAKEPAD_STUDIO_FEEDBACK_DIR")?);
        if !dir.is_absolute() {
            return None;
        }
        let get = |key| {
            std::env::var(key)
                .ok()
                .filter(|s| !s.is_empty() && s.len() <= 256)
        };
        Some(Self {
            dir,
            flow: get("MAKEPAD_STUDIO_FLOW_ID")?,
            artifact: get("MAKEPAD_STUDIO_ARTIFACT_ID")?,
            run: get("MAKEPAD_STUDIO_RUN_ID")?,
            revision: get("MAKEPAD_STUDIO_REVISION")?,
        })
    }
}

#[derive(Clone, Debug, SerJson)]
struct FeedbackImage {
    path: String,
    mime: String,
    width: u32,
    height: u32,
    source: String,
    coordinate_space: String,
    window_id: Option<usize>,
    window_size: [f64; 2],
    frame_size: [u32; 2],
    crop: [f64; 4],
    pixel_crop: [u32; 4],
}

#[derive(SerJson)]
struct FeedbackEvent {
    schema_version: u32,
    kind: String,
    event_id: String,
    flow_id: String,
    artifact_id: String,
    run_id: String,
    revision: String,
    created_at_ms: u64,
    message: String,
    delivery: String,
    image: Option<FeedbackImage>,
    tweak_generation: Option<u64>,
    tweaks: Option<Vec<TweakDiffEntry>>,
}

enum Command {
    Capture {
        region: AiSelectedRegion,
        message: String,
    },
    Import {
        source: ImageSource,
        message: String,
    },
    Submit {
        message: String,
        image: Option<FeedbackImage>,
        delivery: &'static str,
    },
}
enum Reply {
    TweaksSaved { generation: u64, id: String },
    TweaksError(String),
    Captured(FeedbackImage, Arc<Vec<u8>>),
    Saved {
        id: String,
        delivery: &'static str,
        message: String,
        image_path: Option<String>,
    },
    Acknowledged(String),
    Error(String),
}
enum ImageSource {
    Path(PathBuf),
    Bytes { name: String, bytes: Arc<[u8]> },
}
struct Frame {
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}

struct FeedbackClient {
    tx: SyncSender<Command>,
    tweaks_tx: SyncSender<(u64, Arc<Vec<TweakDiffEntry>>)>,
    rx: Receiver<Reply>,
    pending: Option<Command>,
    stop: Arc<AtomicBool>,
    _task: TaskHandle<()>,
}

impl FeedbackClient {
    fn start(cx: &mut Cx, config: Config) -> Result<Self, String> {
        let (tx, commands) = mpsc::sync_channel(2);
        let (tweaks_tx, tweaks_rx) = mpsc::sync_channel(1);
        let (replies, rx) = mpsc::sync_channel(4);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let task = cx
            .thread_spawner()
            .spawn_worker(
                ThreadOptions {
                    name: Some("studio-app-feedback".into()),
                    ..Default::default()
                },
                move || worker(config, commands, tweaks_rx, replies, worker_stop),
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            tweaks_tx,
            rx,
            pending: None,
            stop,
            _task: task,
        })
    }
    fn queue(&mut self, command: Command) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("The feedback queue is full; retry after the current request".into());
        }
        self.pending = Some(command);
        self.retry()
    }
    fn retry(&mut self) -> Result<(), String> {
        if let Some(command) = self.pending.take() {
            match self.tx.try_send(command) {
                Ok(()) => {}
                Err(TrySendError::Full(command)) => self.pending = Some(command),
                Err(TrySendError::Disconnected(_)) => {
                    return Err("The feedback worker stopped; reopen this app to reconnect".into())
                }
            }
        }
        Ok(())
    }
}
impl Drop for FeedbackClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn event_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}-{}",
        millis(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn atomic_write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    if name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err("Invalid feedback filename".into());
    }
    let path = dir.join(name);
    if fs::symlink_metadata(&path).is_ok() {
        return Err("Feedback event already exists".into());
    }
    let temp = dir.join(format!(".{name}.tmp"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, &path).map_err(|e| e.to_string())?;
        if let Ok(directory) = fs::File::open(dir) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn feedback_error(name: &str, error: PictureError) -> String {
    error.report(name, |name, detail| log!("feedback: {name}: {detail}")).into()
}

fn capture_crop(region: AiSelectedRegion, frame_size: [u32; 2]) -> Result<[u32; 4], String> {
    let [fw, fh] = frame_size.map(|side| side as usize);
    let [ww, wh] = [region.window_size.x, region.window_size.y];
    let r = region.rect;
    if ![ww, wh, r.pos.x, r.pos.y, r.size.x, r.size.y]
        .iter()
        .all(|v| v.is_finite())
        || ww <= 0.0
        || wh <= 0.0
        || r.size.x < 4.0
        || r.size.y < 4.0
    {
        return Err("Invalid capture region".into());
    }
    let x = ((r.pos.x / ww * fw as f64).floor().max(0.0) as usize).min(fw);
    let y = ((r.pos.y / wh * fh as f64).floor().max(0.0) as usize).min(fh);
    let right = (((r.pos.x + r.size.x) / ww * fw as f64).ceil().max(0.0) as usize).min(fw);
    let bottom = (((r.pos.y + r.size.y) / wh * fh as f64).ceil().max(0.0) as usize).min(fh);
    if right <= x || bottom <= y {
        return Err("The selected region is outside this drawable".into());
    }
    Ok([x as u32, y as u32, (right - x) as u32, (bottom - y) as u32])
}

fn save_frame(
    config: &Config,
    region: AiSelectedRegion,
    frame: Frame,
) -> Result<(FeedbackImage, Arc<Vec<u8>>), String> {
    let stride = frame.width as usize;
    let len = stride.checked_mul(frame.height as usize).and_then(|pixels| pixels.checked_mul(4));
    if frame.rgba.is_empty() || len != Some(frame.rgba.len()) {
        return Err("Captured drawable has missing or invalid pixels".into());
    }
    let frame_size = [frame.width, frame.height];
    let pixel_crop = capture_crop(region, frame_size)?;
    let [x, y, crop_w, crop_h] = pixel_crop.map(|n| n as usize);
    let start = (y * stride + x) * 4;
    let (width, height, png) = attach::fitted_png(&frame.rgba[start..], stride, crop_w, crop_h, attach::MAX_SIDE)
        .map_err(|e| feedback_error("selected area", e))?;
    let r = region.rect;
    let filename = format!("capture-{}.png", event_id());
    atomic_write(&config.dir, &filename, &png)?;
    Ok((
        FeedbackImage {
            path: filename,
            mime: "image/png".into(),
            width: width as u32,
            height: height as u32,
            source: "app_drawable".into(),
            coordinate_space: "window_layout".into(),
            window_id: Some(region.window_id),
            window_size: [region.window_size.x, region.window_size.y],
            frame_size,
            crop: [r.pos.x, r.pos.y, r.size.x, r.size.y],
            pixel_crop,
        },
        Arc::new(png),
    ))
}

fn image_name_supported(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
}

fn import_image(
    config: &Config,
    source: ImageSource,
) -> Result<(FeedbackImage, Arc<Vec<u8>>), String> {
    let source = match source {
        ImageSource::Path(path) => {
            if !path.is_absolute() || path.as_os_str().len() > 4096 {
                return Err("Drop an absolute image path of at most 4096 bytes".into());
            }
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| feedback_error(&path.to_string_lossy(), PictureError::open(e.kind(), e)))?;
            if !meta.file_type().is_file() {
                return Err("Drop a regular image file".into());
            }
            Source::Path(path)
        }
        ImageSource::Bytes { name, bytes } => Source::Bytes { name, bytes },
    };
    let name = source.name();
    if !image_name_supported(&name) {
        return Err("Use a PNG, JPEG or WebP image".into());
    }
    let report = |e| feedback_error(&name, e);
    let prepared = PreparedPicture::read(source, &name).map_err(report)?;
    let bytes = prepared.encoded_bytes().ok_or("Use an encoded PNG, JPEG or WebP image")?;
    let png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    let jpeg = bytes.starts_with(b"\xff\xd8\xff");
    let webp = bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP");
    if !png && !jpeg && !webp {
        return Err("Dropped data is not PNG, JPEG or WebP".into());
    }
    // Feedback is one still frame. Reject animation before allocating an atlas.
    if png {
        let mut offset = 8usize;
        while offset + 12 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            if bytes.get(offset + 4..offset + 8) == Some(b"acTL") {
                return Err("Use a still PNG image for feedback".into());
            }
            let Some(next) = offset
                .checked_add(length)
                .and_then(|n| n.checked_add(12))
                .filter(|n| *n <= bytes.len())
            else {
                break;
            };
            offset = next;
        }
    }
    if webp
        && bytes.get(12..16) == Some(b"VP8X")
        && bytes.get(20).is_some_and(|flags| flags & 2 != 0)
    {
        return Err("Use a still WebP image for feedback".into());
    }
    let pixels = attach::rgba_picture(bytes).map_err(report)?;
    let (width, height) = (pixels.width, pixels.height);
    let (saved_w, saved_h, png) = attach::fitted_png(
        &pixels.rgba, width as usize, width as usize, height as usize, attach::MAX_SIDE,
    ).map_err(report)?;
    let path = format!("attachment-{}.png", event_id());
    atomic_write(&config.dir, &path, &png)?;
    Ok((
        FeedbackImage {
            path,
            mime: "image/png".into(),
            width: saved_w as u32,
            height: saved_h as u32,
            source: "dropped_image".into(),
            coordinate_space: "image_pixels".into(),
            window_id: None,
            window_size: [0.0, 0.0],
            frame_size: [width, height],
            crop: [0.0, 0.0, width as f64, height as f64],
            pixel_crop: [0, 0, width, height],
        },
        Arc::new(png),
    ))
}

fn save_feedback(
    config: &Config,
    message: &str,
    image: Option<FeedbackImage>,
    delivery: &str,
    tweaks: Option<(u64, Vec<TweakDiffEntry>)>,
) -> Result<String, String> {
    let id = event_id();
    let event = FeedbackEvent {
        schema_version: 1,
        kind: "feedback".into(),
        event_id: id.clone(),
        flow_id: config.flow.clone(),
        artifact_id: config.artifact.clone(),
        run_id: config.run.clone(),
        revision: config.revision.clone(),
        created_at_ms: millis(),
        message: message.into(),
        delivery: delivery.into(),
        image,
        tweak_generation: tweaks.as_ref().map(|(generation, _)| *generation),
        tweaks: tweaks.map(|(_, entries)| entries),
    };
    let json = event.serialize_json();
    if json.len() > 128 * 1024 {
        return Err("Feedback exceeds the 128 KiB event limit".into());
    }
    atomic_write(
        &config.dir,
        &format!("{id}.json"),
        json.as_bytes(),
    )?;
    Ok(id)
}

fn save_tweaks(config: &Config, generation: u64, entries: &[TweakDiffEntry]) -> Result<String, String> {
    let mut message = format!("Live design requirements, generation {generation}: {} changed properties. Replaces earlier design tweaks for this run. Source files have not been changed. The feedback event's structured tweaks contain every exact value, source origin and scope.\n", entries.len());
    if entries.is_empty() {
        message.push_str("All live design changes were undone or reset; no design tweaks remain.");
    }
    let mut shown = 0;
    for entry in entries {
        let line = format!("\n{} · {}: {} → {} @ {} [{}]\n",
            entry.path, entry.prop, entry.old, entry.new, entry.origin, entry.scope);
        if message.len() + line.len() > 2800 {
            continue;
        }
        message.push_str(&line);
        shown += 1;
    }
    if shown < entries.len() {
        message.push_str(&format!("\n{} additional properties are retained in the event's complete structured tweaks. Read that evidence before applying the design requirements.", entries.len() - shown));
    }
    save_feedback(config, &message, None, "design_tweak", Some((generation, entries.to_vec())))
}

fn attachment_ready(
    config: &Config,
    replies: &SyncSender<Reply>,
    unsubmitted: &mut Option<String>,
    ack: &mut Option<String>,
    image: FeedbackImage,
    bytes: Arc<Vec<u8>>,
    message: String,
) {
    if let Some(old) = unsubmitted.replace(image.path.clone()) {
        let _ = fs::remove_file(config.dir.join(old));
    }
    let result = save_feedback(config, &message, Some(image.clone()), "terminal_drop", None);
    let image_path = Some(image.path.clone());
    reply(replies, Reply::Captured(image, bytes));
    match result {
        Ok(id) => {
            *unsubmitted = None;
            *ack = Some(id.clone());
            reply(
                replies,
                Reply::Saved {
                    id,
                    delivery: "terminal_drop",
                    message,
                    image_path,
                },
            );
        }
        Err(error) => reply(replies, Reply::Error(error)),
    }
}

fn reply(tx: &SyncSender<Reply>, value: Reply) {
    let _ = tx.send(value);
    SignalToUI::set_ui_signal();
}

fn worker(
    mut config: Config,
    commands: Receiver<Command>,
    tweaks_rx: Receiver<(u64, Arc<Vec<TweakDiffEntry>>)>,
    replies: SyncSender<Reply>,
    stop: Arc<AtomicBool>,
) {
    let setup = (|| {
        fs::create_dir_all(&config.dir).map_err(|e| e.to_string())?;
        let meta = fs::symlink_metadata(&config.dir).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err("Feedback spool must be a regular directory".into());
        }
        config.dir = fs::canonicalize(&config.dir).map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    })();
    if let Err(error) = setup {
        reply(&replies, Reply::Error(error));
        return;
    }
    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    let mut capture: Option<(u64, AiSelectedRegion, Instant, String)> = None;
    let mut unsubmitted: Option<String> = None;
    let mut ack: Option<String> = None;
    let mut tweaks: Option<(u64, Arc<Vec<TweakDiffEntry>>, Instant)> = None;
    loop {
        while let Ok((generation, entries)) = tweaks_rx.try_recv() {
            // Separate desired-state queue: slider changes never occupy the
            // command slots needed by an image attachment or text feedback.
            let due = tweaks.as_ref().map(|(_, _, due)| *due)
                .unwrap_or_else(|| Instant::now() + Duration::from_millis(250));
            tweaks = Some((generation, entries, due));
        }
        if tweaks.as_ref().is_some_and(|(_, _, due)| Instant::now() >= *due) {
            let (generation, entries, _) = tweaks.take().unwrap();
            match save_tweaks(&config, generation, &entries) {
                Ok(id) => reply(&replies, Reply::TweaksSaved { generation, id }),
                Err(error) => {
                    reply(&replies, Reply::TweaksError(error));
                    tweaks = Some((generation, entries, Instant::now() + Duration::from_secs(2)));
                }
            }
        }
        if let Some((sink, region, deadline)) = capture
            .as_ref()
            .map(|(sink, region, deadline, _)| (*sink, *region, *deadline))
        {
            match frame_rx.try_recv() {
                Ok(frame) => {
                    screen_capture::remove_screen_capture(sink);
                    let message = capture.take().unwrap().3;
                    match save_frame(&config, region, frame) {
                        Ok((image, bytes)) => attachment_ready(
                            &config,
                            &replies,
                            &mut unsubmitted,
                            &mut ack,
                            image,
                            bytes,
                            message,
                        ),
                        Err(error) => reply(&replies, Reply::Error(error)),
                    }
                }
                Err(_) if Instant::now() >= deadline => {
                    screen_capture::remove_screen_capture(sink);
                    capture = None;
                    reply(
                        &replies,
                        Reply::Error("This app did not produce a capture frame".into()),
                    );
                }
                _ => {}
            }
        }
        if let Some(id) = ack.as_ref() {
            let path = config.dir.join(format!("{id}.ack.json"));
            if fs::symlink_metadata(path)
                .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 16 * 1024)
            {
                reply(&replies, Reply::Acknowledged(id.clone()));
                ack = None;
            }
        }
        let command = match commands.recv_timeout(Duration::from_millis(40)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                continue;
            }
        };
        match command {
            Command::Capture { region, message } => {
                if let Some((sink, _, _, _)) = capture.take() {
                    screen_capture::remove_screen_capture(sink);
                }
                while frame_rx.try_recv().is_ok() {}
                let tx = frame_tx.clone();
                let sent = Arc::new(AtomicBool::new(false));
                // Let the selection outline clear before accepting the next
                // drawable. UI redraws while capture is pending.
                let after = Instant::now() + Duration::from_millis(150);
                let sink = screen_capture::add_screen_capture(
                    screen_capture::ScreenCaptureOptions {
                        window_id: Some(region.window_id),
                        max_fps: 30.0,
                    },
                    move |frame| {
                        if Instant::now() < after || sent.load(Ordering::Relaxed) {
                            return;
                        }
                        let data = Frame {
                            width: frame.width,
                            height: frame.height,
                            rgba: Arc::new(frame.rgba.to_vec()),
                        };
                        if tx.try_send(data).is_ok() {
                            sent.store(true, Ordering::Relaxed);
                        }
                    },
                );
                capture = Some((
                    sink,
                    region,
                    Instant::now() + Duration::from_secs(6),
                    message,
                ));
                SignalToUI::set_ui_signal();
            }
            Command::Import { source, message } => match import_image(&config, source) {
                Ok((image, bytes)) => attachment_ready(
                    &config,
                    &replies,
                    &mut unsubmitted,
                    &mut ack,
                    image,
                    bytes,
                    message,
                ),
                Err(error) => reply(&replies, Reply::Error(error)),
            },
            Command::Submit {
                message,
                image,
                delivery,
            } => {
                let image_path = image.as_ref().map(|image| image.path.clone());
                match save_feedback(&config, &message, image, delivery, None) {
                    Ok(id) => {
                        unsubmitted = None;
                        ack = Some(id.clone());
                        reply(
                            &replies,
                            Reply::Saved {
                                id,
                                delivery,
                                message,
                                image_path,
                            },
                        );
                    }
                    Err(error) => reply(&replies, Reply::Error(error)),
                }
            }
        }
    }
    if let Some((generation, entries, _)) = tweaks {
        if let Err(error) = save_tweaks(&config, generation, &entries) {
            eprintln!("Studio design feedback could not be saved: {error}");
        }
    }
    if let Some((sink, _, _, _)) = capture {
        screen_capture::remove_screen_capture(sink);
    }
    if let Some(path) = unsubmitted {
        let _ = fs::remove_file(config.dir.join(path));
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct StudioAppFeedback {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    initialized: bool,
    #[rust]
    client: Option<FeedbackClient>,
    #[rust]
    image: Option<FeedbackImage>,
    #[rust]
    image_delivered: bool,
    #[rust]
    focus_message: bool,
    #[rust]
    busy: bool,
    #[rust]
    selecting: bool,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    tweak_generation: u64,
    #[rust]
    pending_tweaks: Option<(u64, Arc<Vec<TweakDiffEntry>>)>,
}

impl StudioAppFeedback {
    fn initialize(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        let Some(config) = Config::from_env() else {
            self.view.set_visible(cx, false);
            return;
        };
        match FeedbackClient::start(cx, config) {
            Ok(client) => self.client = Some(client),
            Err(error) => self.status(cx, &error),
        }
    }
    fn status(&mut self, cx: &mut Cx, text: &str) {
        self.view
            .label(cx, ids!(feedback_status))
            .set_text(cx, text);
        self.view.redraw(cx);
    }
    fn pump(&mut self, cx: &mut Cx) {
        if let Some(Err(error)) = self.client.as_mut().map(FeedbackClient::retry) {
            self.busy = false;
            self.status(cx, &error);
        }
        while let Some(reply) = self.client.as_ref().and_then(|c| c.rx.try_recv().ok()) {
            match reply {
                Reply::TweaksSaved { generation, id } => {
                    self.status(cx, &format!("Design changes saved · generation {generation} ({id})"));
                }
                Reply::TweaksError(error) => self.status(cx, &format!("Design feedback: {error} · retrying")),
                Reply::Captured(image, bytes) => {
                    self.busy = false;
                    self.status(
                        cx,
                        &format!("Area attached · {} × {} pixels", image.width, image.height),
                    );
                    let preview = self.view.image(cx, ids!(feedback_preview));
                    preview.set_visible(cx, true);
                    if let Err(error) =
                        preview.load_image_from_data_async(cx, Path::new(&image.path), bytes)
                    {
                        self.status(cx, &format!("Image saved; preview unavailable: {error}"));
                    }
                    self.image = Some(image);
                    self.image_delivered = false;
                    self.focus_message = true;
                    let req = cx.global::<AiSlotRequests>();
                    req.feedback_focus = true;
                    req.open = Some(true);
                    cx.new_next_frame();
                    cx.redraw_all();
                }
                Reply::Saved {
                    id,
                    delivery,
                    message,
                    image_path,
                } => {
                    self.busy = false;
                    if image_path.as_deref() == self.image.as_ref().map(|image| image.path.as_str())
                        && delivery == "terminal_drop"
                    {
                        self.image_delivered = true;
                    }
                    if delivery == "feedback"
                        && self
                            .view
                            .text_input(cx, ids!(feedback_message))
                            .text()
                            .trim()
                            == message
                    {
                        self.view
                            .text_input(cx, ids!(feedback_message))
                            .set_text(cx, "");
                    }
                    self.status(
                        cx,
                        &format!(
                            "{} saved · awaiting Studio ({id})",
                            if delivery == "terminal_drop" {
                                "AI attachment"
                            } else {
                                "Feedback"
                            }
                        ),
                    );
                }
                Reply::Acknowledged(id) => {
                    self.status(cx, &format!("Studio received feedback ({id})"))
                }
                Reply::Error(error) => {
                    self.busy = false;
                    self.status(cx, &error);
                }
            }
        }
        // The design delta comes from the tweaker when the app has it; an app
        // built without the overlay has none to send.
        let snapshot = WidgetHooks::tweaker(cx)
            .and_then(|tweaker| (tweaker.feedback_snapshot)(self.tweak_generation));
        if let Some(snapshot) = snapshot {
            self.tweak_generation = snapshot.generation;
            match snapshot.entries {
                Ok(entries) => self.pending_tweaks = Some((snapshot.generation, Arc::new(entries))),
                Err(error) => {
                    self.pending_tweaks = None;
                    self.status(cx, &format!("Design feedback was not exported: {error}"));
                    log!("Studio design feedback was not exported: {error}");
                }
            }
        }
        if let Some(snapshot) = self.pending_tweaks.take() {
            match self.client.as_ref().unwrap().tweaks_tx.try_send(snapshot) {
                Ok(()) => {},
                Err(TrySendError::Full(snapshot)) => self.pending_tweaks = Some(snapshot),
                Err(TrySendError::Disconnected(_)) => self.status(cx, "Design feedback worker stopped; the changes were not exported"),
            }
        }
        if cx.global::<AiSlotRequests>().feedback_selecting {
            self.selecting = true;
        }
        if self.selecting || cx.global::<AiSlotRequests>().selected_region.is_some() {
            if let Some(region) = cx.global::<AiSlotRequests>().selected_region.take() {
                self.selecting = false;
                let message = self.attachment_message(cx);
                self.queue(
                    cx,
                    Command::Capture { region, message },
                    "Capturing the selected app area…",
                );
            } else if std::mem::take(&mut cx.global::<AiSlotRequests>().region_cancelled) {
                self.selecting = false;
                self.status(cx, "Area selection cancelled");
            }
        }
        self.view
            .button(cx, ids!(feedback_region))
            .set_enabled(cx, !self.busy && !self.selecting);
        self.view
            .button(cx, ids!(feedback_submit))
            .set_enabled(cx, !self.busy && !self.selecting);
        if self.busy || self.pending_tweaks.is_some() || self.client.as_ref().is_some_and(|c| c.pending.is_some()) {
            self.next_frame = cx.new_next_frame();
            cx.redraw_all();
        }
        cx.global::<AiSlotRequests>().feedback_busy = self.busy;
    }

    fn attachment_message(&self, cx: &mut Cx) -> String {
        let message = self.view.text_input(cx, ids!(feedback_message)).text();
        let message = message.trim();
        if message.is_empty() {
            "Image attached for the flow's AI to review.".into()
        } else if message.len() > MAX_MESSAGE {
            "Image attached; additional feedback draft remains in F10.".into()
        } else {
            message.into()
        }
    }

    fn dropped_image(item: &DragItem) -> Option<ImageSource> {
        match item {
            DragItem::FilePath { path, .. }
                if image_name_supported(path)
                    && Path::new(path).is_absolute()
                    && path.len() <= 4096 =>
            {
                Some(ImageSource::Path(PathBuf::from(path)))
            }
            DragItem::VirtualFile(file) if image_name_supported(&file.name) =>
            {
                Some(ImageSource::Bytes {
                    name: file.name.clone(),
                    bytes: file.bytes.clone(),
                })
            }
            _ => None,
        }
    }

    fn handle_image_drop(&mut self, cx: &mut Cx, event: &Event) {
        if !matches!(event, Event::Drag(_) | Event::Drop(_)) {
            return;
        }
        match event.drag_hits(cx, self.view.area()) {
            DragHit::Drag(drag) => {
                let accept = !self.busy
                    && !self.selecting
                    && drag
                        .items
                        .iter()
                        .any(|item| Self::dropped_image(item).is_some());
                if let Ok(mut response) = drag.response.try_lock() {
                    *response = if accept {
                        DragResponse::Copy
                    } else {
                        DragResponse::None
                    };
                }
            }
            DragHit::Drop(drop) => {
                if self.busy || self.selecting {
                    self.status(cx,"The current attachment is still being saved; drop the next image afterward");
                    return;
                }
                if let Some(source) = drop.items.iter().find_map(Self::dropped_image) {
                    let message = self.attachment_message(cx);
                    self.queue(
                        cx,
                        Command::Import { source, message },
                        "Preparing image for the flow's AI…",
                    );
                } else {
                    self.status(cx, "Drop one PNG, JPEG or WebP image here");
                }
            }
            _ => {}
        }
    }
    fn queue(&mut self, cx: &mut Cx, command: Command, status: &str) {
        let result = self
            .client
            .as_mut()
            .ok_or_else(|| "The feedback worker is unavailable".to_string())
            .and_then(|client| client.queue(command));
        match result {
            Ok(()) => {
                self.busy = true;
                let queued = self
                    .client
                    .as_ref()
                    .is_some_and(|client| client.pending.is_some());
                self.status(
                    cx,
                    if queued {
                        "Feedback queue is full · retrying on the next frame"
                    } else {
                        status
                    },
                );
                self.next_frame = cx.new_next_frame();
                cx.redraw_all();
            }
            Err(error) => {
                self.busy = false;
                self.status(cx, &error);
            }
        }
        cx.global::<AiSlotRequests>().feedback_busy = self.busy;
    }
}

impl Widget for StudioAppFeedback {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.initialize(cx);
        if self.client.is_none() {
            return;
        }
        self.pump(cx);
        self.handle_image_drop(cx, event);
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(feedback_region)).clicked(actions) && !self.busy {
                let req = cx.global::<AiSlotRequests>();
                req.selected_region = None;
                req.region_cancelled = false;
                req.select_region = req.current_window;
                self.selecting = req.select_region.is_some();
                self.status(
                    cx,
                    if self.selecting {
                        "Drag an area over the app · Escape cancels"
                    } else {
                        "Open F10 in the app window before selecting an area"
                    },
                );
                cx.new_next_frame();
                cx.redraw_all();
            }
            if self.view.button(cx, ids!(feedback_submit)).clicked(actions)
                && !self.busy
                && !self.selecting
            {
                let message = self.view.text_input(cx, ids!(feedback_message)).text();
                let message = message.trim();
                if message.is_empty() {
                    self.status(cx, "Describe the change before sending feedback");
                } else if message.len() > MAX_MESSAGE {
                    self.status(cx, "Feedback is limited to 4 KiB per message; your draft is unchanged");
                } else {
                    self.queue(
                        cx,
                        Command::Submit {
                            message: message.into(),
                            image: self.image.clone(),
                            delivery: if self.image.is_some() && !self.image_delivered {
                                "terminal_drop"
                            } else {
                                "feedback"
                            },
                        },
                        "Saving feedback for Studio…",
                    );
                }
            }
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.initialize(cx);
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_done() && self.focus_message && cx.global::<AiSlotRequests>().is_open {
            let input = self.view.text_input(cx, ids!(feedback_message));
            if !input.area().is_empty() {
                input.set_key_focus(cx);
                self.focus_message = false;
            }
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    #[test]
    fn feedback_fits_the_saved_png_and_keeps_the_original_coordinates() {
        let region = |window_size, pos, size| AiSelectedRegion { window_id: 1, window_size, rect: Rect { pos, size } };
        // Exercise the actual crop and fit rules on a 6K drawable without allocating it.
        let large = region(dvec2(3072., 1728.), dvec2(16., 8.), dvec2(3000., 1687.5));
        let crop = capture_crop(large, [6144, 3456]).expect("a crop over the old 16 MP cap must be admitted");
        assert_eq!(crop, [32, 16, 6000, 3375], "the large crop must keep its device-pixel origin and extent");
        assert_eq!(attach::fitted_size(crop[2] as usize, crop[3] as usize, attach::MAX_SIDE), (1024, 576),
            "a 6K capture must fit the model edge at its own aspect ratio");

        let dir = std::env::temp_dir().join(format!("aichat-feedback-{}", event_id()));
        fs::create_dir(&dir).unwrap();
        let config = Config { dir, flow: "f".into(), artifact: "a".into(), run: "r".into(), revision: "rev".into() };
        let saved_pixels = |image: &FeedbackImage| {
            let bytes = fs::read(config.dir.join(&image.path)).expect("the feedback PNG must exist in the spool");
            let (w, h, pixels) = makepad_image_decode::decode_rgba(&bytes).expect("the saved PNG must decode");
            assert_eq!((w, h), (image.width as usize, image.height as usize), "event dimensions must match the saved PNG");
            pixels
        };
        // Two frame pixels per window point. Distinct neighbours and a border
        // expose a wrong crop offset, filter width or source row stride.
        let mut pixels = vec![255; 2052 * 12 * 4];
        for y in 2..10 {
            for x in 2..2050 {
                let at = (y * 2052 + x) * 4;
                pixels[at..at + 4].copy_from_slice(&[2 + 8 * (x % 2) as u8, 8 * y as u8, 0, 255]);
            }
        }
        let pixels = Arc::new(pixels);
        let capture = |x, w| {
            let selection = region(dvec2(1026., 6.), dvec2(x, 1.), dvec2(w, 4.));
            let frame = Frame { width: 2052, height: 12, rgba: pixels.clone() };
            save_frame(&config, selection, frame).expect("the selected crop must be written").0
        };
        let scaled = capture(1., 1024.);
        assert_eq!((scaled.width, scaled.height), (1024, 4), "the spool must hold the fitted crop");
        assert_eq!(scaled.pixel_crop, [2, 2, 2048, 8], "fitting must preserve the full-resolution pixel crop");
        assert_eq!((scaled.frame_size, scaled.window_size), ([2052, 12], [1026., 6.]), "frame pixels and window points must retain their own sizes");
        assert_eq!((scaled.coordinate_space.as_str(), scaled.crop), ("window_layout", [1., 1., 1024., 4.]), "the selected layout rectangle must remain exact");
        let fitted = saved_pixels(&scaled);
        assert_eq!((&fitted[..4], &fitted[fitted.len() - 4..]), (&[6, 20, 0, 255][..], &[6, 68, 0, 255][..]),
            "the fitted pixels must average the selection without sampling its border");

        let small = capture(1.25, 4.);
        assert_eq!(small.pixel_crop, [2, 2, 9, 8], "fractional selection edges must still round outwards in frame pixels");
        assert_eq!((small.width, small.height), (9, 8), "a small crop must pass through at its own size");
        let expected: Vec<u8> = (2..10).flat_map(|row| &pixels[(2052 * row + 2) * 4..(2052 * row + 11) * 4]).copied().collect();
        assert_eq!(saved_pixels(&small), expected, "a small crop must preserve every selected pixel");

        // A legal private PNG chunk, sparse on disk. Its CRC covers "feEd"
        // followed by exactly 32 MiB of zeroes. The image itself is one row.
        let png = Cx::encode_rgba_as_png(2048, 1, &vec![75; 2048 * 4]).unwrap();
        let end = png.len() - 12; // IEND
        let path = config.dir.join("wide.png");
        let mut file = fs::File::create_new(&path).unwrap();
        file.write_all(&png[..end]).unwrap();
        let padding = 32u32 << 20;
        file.write_all(&padding.to_be_bytes()).unwrap();
        file.write_all(b"feEd").unwrap();
        file.seek(SeekFrom::Current(i64::from(padding))).unwrap();
        file.write_all(&0x952b_b3bcu32.to_be_bytes()).unwrap();
        file.write_all(&png[end..]).unwrap();
        drop(file);
        let imported = import_image(&config, ImageSource::Path(path)).expect("a real PNG over 32 MiB must be imported").0;
        assert_eq!((imported.width, imported.height), (1024, 1), "an imported image must use the same model edge");
        assert_eq!((imported.frame_size, imported.pixel_crop), ([2048, 1], [0, 0, 2048, 1]), "import coordinates must describe the original image");
        assert_eq!((imported.coordinate_space.as_str(), imported.crop), ("image_pixels", [0., 0., 2048., 1.]), "import geometry must stay in original image pixels");
        assert_eq!(saved_pixels(&imported), vec![75; 1024 * 4], "a large import must save the fitted pixels");

        let mut over_wide = png;
        over_wide[16..20].copy_from_slice(&16385u32.to_be_bytes());
        let source = ImageSource::Bytes { name: "wide.png".into(), bytes: over_wide.into() };
        assert_eq!(import_image(&config, source).unwrap_err(), "over 16384 pixels a side", "an import must use the chat decoder's plain size refusal");
        fs::remove_dir_all(&config.dir).unwrap();
    }
}
