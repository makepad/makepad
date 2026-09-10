// Included by iteration_worker.rs. All directory walks, image decoding and
// deletion below run on its single owned worker, never on the UI thread.

const RECORDING_MAX_TILES: usize = 256;
const RECORDING_MAX_DIR_ENTRIES: usize = 128;
const RECORDING_MAX_SIDECAR_BYTES: usize = 32 * 1024;
const RECORDING_MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const RECORDING_MAX_PIXELS: usize = 16 * 1024 * 1024;
const RECORDING_MAX_MP4_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingTile {
    pub flow: String,
    pub id: String,
    pub run: String,
    pub artifact: String,
    pub commit: String,
    pub title: String,
    pub preview_id: String,
    pub original_width: u32,
    pub original_height: u32,
    pub active: bool,
    pub complete: bool,
    pub frames: u64,
    pub frame_sequence: u64,
    pub elapsed_ms: u64,
    pub mp4: Option<PathBuf>,
    pub preview_path: Option<PathBuf>,
    pub full_path: Option<PathBuf>,
    pub error: Option<String>,
    pub inspection_error: Option<String>,
    pub sidecar_path: PathBuf,
}
impl RecordingTile {
    pub fn json(&self) -> Value {
        let path = |value: &Option<PathBuf>| {
            value
                .as_ref()
                .map(|path| s(path.to_string_lossy()))
                .unwrap_or(Value::Null)
        };
        json::obj(vec![
            ("flow", s(&self.flow)),
            ("id", s(&self.id)),
            ("run", s(&self.run)),
            ("artifact", s(&self.artifact)),
            ("commit", s(&self.commit)),
            ("title", s(&self.title)),
            ("preview_id", s(&self.preview_id)),
            ("original_width", Value::Int(self.original_width.into())),
            ("original_height", Value::Int(self.original_height.into())),
            ("active", Value::Bool(self.active)),
            ("complete", Value::Bool(self.complete)),
            ("frames", Value::Int(self.frames as i64)),
            ("frame_sequence", Value::Int(self.frame_sequence as i64)),
            ("elapsed_ms", Value::Int(self.elapsed_ms as i64)),
            ("mp4", path(&self.mp4)),
            ("preview", path(&self.preview_path)),
            ("full_frame", path(&self.full_path)),
            ("error", self.error.as_ref().map(s).unwrap_or(Value::Null)),
            (
                "inspection_error",
                self.inspection_error.as_ref().map(s).unwrap_or(Value::Null),
            ),
            ("test_result", s("Not inferred from recording completion")),
        ])
    }
}

#[derive(Default)]
pub struct RecordingState {
    tiles: BTreeMap<String, RecordingTile>,
    loaded: BTreeMap<String, (u64, u128)>,
    attempted: BTreeMap<String, u64>,
    cursor: usize,
    tick: u64,
}
impl RecordingState {
    /// Active tiles first; newest recorder filenames first within each group.
    pub fn snapshot(&self) -> Vec<RecordingTile> {
        let mut tiles: Vec<_> = self.tiles.values().cloned().collect();
        tiles.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then_with(|| b.sidecar_path.cmp(&a.sidecar_path))
        });
        tiles
    }
}

#[derive(Clone)]
struct KnownRecordingRun {
    flow: String,
    run: String,
    artifact: String,
    commit: String,
    directory: PathBuf,
    active: bool,
    is_test: bool,
    pid: Option<u32>,
}

impl Host {
    fn known_recording_runs(&self) -> Vec<KnownRecordingRun> {
        let mut known = BTreeMap::new();
        for flow in self.engine.flows.values() {
            for run in &flow.runs {
                let Some(origin) = self.engine.evidence_origin("run", &run.id) else {
                    continue;
                };
                let Some(artifact) = flow
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.id == run.artifact_id)
                else {
                    continue;
                };
                if !recording_identifier(origin)
                    || !recording_identifier(&run.id)
                    || !recording_commit(&artifact.commit)
                {
                    continue;
                }
                known.insert(
                    run.id.clone(),
                    KnownRecordingRun {
                        flow: origin.to_owned(),
                        run: run.id.clone(),
                        artifact: artifact.id.clone(),
                        commit: artifact.commit.clone(),
                        directory: self.directory.join("runs").join(&run.id).join("video"),
                        active: !run.closed,
                        is_test: run.role == iteration::RunRole::AiTest,
                        pid: run.pid,
                    },
                );
            }
        }
        for (key, report) in &self.reports {
            if report.get("kind").and_then(Value::as_str) != Some("build") {
                continue;
            }
            let get = |field| report.get(field).and_then(Value::as_str);
            let (Some(flow), Some(job), Some(run), Some(video), Some(artifact), Some(commit)) = (
                get("flow"),
                get("job"),
                get("test_run"),
                get("test_video"),
                get("artifact"),
                get("commit"),
            ) else {
                continue;
            };
            if !Self::retained_lane_report(&self.engine, report)
                || !recording_identifier(flow)
                || !recording_identifier(job)
                || !recording_identifier(run)
                || !recording_identifier(artifact)
                || !recording_commit(commit)
            {
                continue;
            }
            if key != &format!("build:{flow}:{job}") || run != format!("test-{flow}-{job}") {
                continue;
            }
            let directory = self.directory.join("runs").join(run).join("video");
            if Path::new(video) != directory {
                continue;
            }
            // A private report must refer back to an observed checkpoint. A
            // caller cannot extend scanning merely by naming another folder.
            let checkpointed = self.engine.retained_events().any(|event| {
                event.flow == flow
                    && event
                        .operation
                        .get("observation")
                        .is_some_and(|observation| {
                            observation.get("kind").and_then(Value::as_str) == Some("checkpointed")
                                && observation.get("job_id").and_then(Value::as_str) == Some(job)
                                && observation.get("commit").and_then(Value::as_str) == Some(commit)
                        })
            });
            if !checkpointed {
                continue;
            }
            known
                .entry(run.to_owned())
                .or_insert_with(|| KnownRecordingRun {
                    flow: flow.into(),
                    run: run.into(),
                    artifact: artifact.into(),
                    commit: commit.into(),
                    directory,
                    active: self.builds.get(flow).is_some_and(|build| build.job == job),
                    is_test: true,
                    pid: None,
                });
        }
        known.into_values().collect()
    }

    /// Scan active owned runs every call, plus at most eight historical run
    /// directories. Decode at most four changed thumbnails per call, choosing
    /// least-recently-attempted images so busy windows cannot starve others.
    fn load_recordings(&mut self) {
        self.recordings.tick = self.recordings.tick.wrapping_add(1);
        let known = self.known_recording_runs();
        let keep: BTreeSet<_> = known
            .iter()
            .map(|run| (run.flow.clone(), run.run.clone()))
            .collect();
        let old_len = self.recordings.tiles.len();
        self.recordings
            .tiles
            .retain(|_, tile| keep.contains(&(tile.flow.clone(), tile.run.clone())));
        if old_len != self.recordings.tiles.len() {
            self.changed = true;
        }
        let mut selected: Vec<_> = known
            .iter()
            .filter(|run| run.active)
            .take(8)
            .cloned()
            .collect();
        let inactive: Vec<_> = known.iter().filter(|run| !run.active).collect();
        if !inactive.is_empty() {
            for offset in 0..inactive.len().min(8) {
                selected.push(inactive[(self.recordings.cursor + offset) % inactive.len()].clone());
            }
            self.recordings.cursor =
                (self.recordings.cursor + inactive.len().min(8)) % inactive.len();
        }
        for run in &selected {
            let paths = match recording_sidecars(&self.directory, run) {
                Ok(paths) => paths,
                Err(error) => {
                    self.recording_note(format!(
                        "Recording directory unavailable for {}: {error}",
                        run.flow
                    ));
                    continue;
                }
            };
            for path in paths {
                match recording_tile(&self.directory, run, &path) {
                    Ok(tile) => {
                        if let Some(existing) = self.recordings.tiles.get(&tile.id) {
                            if existing.sidecar_path != tile.sidecar_path {
                                self.recording_note(
                                    "Recording identity collision; history was not replaced".into(),
                                );
                                continue;
                            }
                        } else if self.recordings.tiles.len() >= RECORDING_MAX_TILES {
                            self.recording_note(
                                "Recording history reached its 256-tile display bound".into(),
                            );
                            continue;
                        }
                        if self.recordings.tiles.get(&tile.id) != Some(&tile) {
                            self.recordings.tiles.insert(tile.id.clone(), tile);
                            self.changed = true;
                        }
                    }
                    Err(error) => self.recording_note(format!(
                        "Recording metadata pending for {}: {error}",
                        run.flow
                    )),
                }
            }
        }
        let mut candidates: Vec<_> = self
            .recordings
            .tiles
            .values()
            .filter(|tile| tile.preview_path.is_some())
            .cloned()
            .collect();
        candidates.sort_by_key(|tile| {
            self.recordings
                .attempted
                .get(&tile.id)
                .copied()
                .unwrap_or(0)
        });
        let mut decoded = 0;
        for tile in candidates {
            if decoded >= 4 {
                break;
            }
            let Some(path) = &tile.preview_path else {
                continue;
            };
            let stamp = match recording_file_stamp(path, RECORDING_MAX_IMAGE_BYTES as u64) {
                Ok(stamp) => stamp,
                Err(_) => continue,
            };
            if self.recordings.loaded.get(&tile.id) == Some(&stamp)
                && self.previews.contains_key(&tile.preview_id)
            {
                continue;
            }
            if !self.previews.contains_key(&tile.preview_id) && self.previews.len() >= 128 {
                continue;
            }
            self.recordings
                .attempted
                .insert(tile.id.clone(), self.recordings.tick);
            decoded += 1;
            let result = recording_image(&self.directory, path)
                .map(|image| recording_thumbnail(&tile.preview_id, image));
            match result {
                Ok(preview) => {
                    self.previews
                        .insert(tile.preview_id.clone(), Arc::new(preview));
                    self.recordings.loaded.insert(tile.id, stamp);
                    self.changed = true;
                }
                Err(error) => self.recording_note(format!(
                    "Recording preview pending for {}: {error}",
                    tile.flow
                )),
            }
        }
    }

    /// `id` accepts either RecordingTile.id or its preview_id. Metadata and
    /// file ownership are rechecked; live .current.png is atomic and contains
    /// real original pixels even before the MP4 is finalized.
    fn load_full_recording_preview(&mut self, id: &str) -> Result<Arc<Preview>, String> {
        let tile = self
            .recordings
            .tiles
            .get(id)
            .or_else(|| {
                self.recordings
                    .tiles
                    .values()
                    .find(|tile| tile.preview_id == id)
            })
            .cloned()
            .ok_or("Unknown recording tile")?;
        let known = self
            .known_recording_runs()
            .into_iter()
            .find(|run| run.flow == tile.flow && run.run == tile.run)
            .ok_or("Recording no longer belongs to an owned run")?;
        let mut last_error = String::new();
        for _ in 0..2 {
            let current = recording_tile(&self.directory, &known, &tile.sidecar_path)?;
            let path = current.full_path.as_ref().ok_or_else(|| {
                current
                    .inspection_error
                    .clone()
                    .unwrap_or("Original frame has not been published yet".into())
            })?;
            match recording_image(&self.directory, path) {
                Ok(image) if image.width == current.original_width as usize && image.height == current.original_height as usize => {
                    let preview = Arc::new(Preview { id: current.preview_id.clone(), width: current.original_width, height: current.original_height, pixels: Arc::new(image.data) });
                    if self.recordings.tiles.get(&current.id) != Some(&current) { self.recordings.tiles.insert(current.id.clone(), current); self.changed = true; }
                    return Ok(preview);
                }
                Ok(_) => last_error = "Recording resized while the original frame was being read; retry after its next sidecar update".into(),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    /// Explicit user action only. Validate every known sidecar before the
    /// first unlink, refuse running/unknown app ownership and active encoders,
    /// and remove only their exact regular MP4 files. PNGs and JSON remain.
    fn delete_flow_videos(&mut self, flow: &str) -> Result<Value, String> {
        let state = self
            .engine
            .flows
            .get(flow)
            .ok_or("Unknown iteration flow")?;
        if state.runs.iter().any(|run| !run.closed)
            || self.builds.contains_key(flow)
            || self.apps.contains_key(flow)
        {
            return Err(
                "Close or reconcile this flow's active app/test runs before deleting its videos"
                    .into(),
            );
        }
        let known: Vec<_> = self
            .known_recording_runs()
            .into_iter()
            .filter(|run| self.engine.has_history_ancestor(flow, &run.flow))
            .collect();
        let mut paths = BTreeMap::new();
        for run in &known {
            for sidecar in recording_sidecars(&self.directory, run)? {
                let tile = recording_tile(&self.directory, run, &sidecar)?;
                if self
                    .engine
                    .history_owner(&tile.flow, &format!("recording/{}", tile.id))
                    != flow
                {
                    continue;
                }
                if tile.active {
                    return Err(format!(
                        "Recording {} is still active; no videos were deleted",
                        tile.id
                    ));
                }
                if let Some(path) = tile.mp4 {
                    let metadata = recording_regular_file(&path, RECORDING_MAX_MP4_BYTES)?;
                    paths.insert(path, metadata);
                }
            }
        }
        if paths.is_empty() {
            return Ok(json::obj(vec![
                ("flow", s(flow)),
                ("deleted", Value::Int(0)),
                ("bytes", Value::Int(0)),
            ]));
        }
        let key = format!("video-cleanup:{flow}:{}", now());
        let intent = json::obj(vec![
            ("kind", s("video_cleanup")),
            ("flow", s(flow)),
            ("state", s("requested")),
            (
                "paths",
                Value::Arr(paths.keys().map(|path| s(path.to_string_lossy())).collect()),
            ),
            ("stills_preserved", Value::Bool(true)),
        ]);
        self.reports.insert(key.clone(), intent.clone());
        if let Err(error) = self.persist() {
            self.reports.remove(&key);
            return Err(error);
        }
        let mut deleted = 0;
        let mut bytes = 0u64;
        let mut failure = None;
        for (path, before) in paths {
            let result = (|| {
                recording_owned_path(&self.directory, &path)?;
                let current = recording_regular_file(&path, RECORDING_MAX_MP4_BYTES)?;
                if !recording_same_file(&before, &current) {
                    return Err("A video changed after cleanup validation".into());
                }
                fs::remove_file(&path).map_err(err)
            })();
            match result {
                Ok(()) => {
                    deleted += 1;
                    bytes = bytes.saturating_add(before.len());
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let result = json::obj(vec![
            ("flow", s(flow)),
            ("deleted", Value::Int(deleted)),
            ("bytes", Value::Int(bytes.min(i64::MAX as u64) as i64)),
            ("stills_preserved", Value::Bool(true)),
            ("error", failure.as_ref().map(s).unwrap_or(Value::Null)),
        ]);
        self.reports.insert(
            key,
            json::obj(vec![
                ("kind", s("video_cleanup")),
                ("flow", s(flow)),
                (
                    "state",
                    s(if failure.is_some() {
                        "partial"
                    } else {
                        "complete"
                    }),
                ),
                ("intent", intent),
                ("result", result.clone()),
            ]),
        );
        self.changed = true;
        let persisted = self.persist();
        self.load_recordings();
        persisted?;
        if let Some(error) = failure {
            return Err(format!(
                "Deleted {deleted} videos ({bytes} bytes); cleanup stopped: {error}"
            ));
        }
        Ok(result)
    }

    fn recording_note(&mut self, note: String) {
        if self.note != note {
            self.note = note;
            self.changed = true;
        }
    }
}

fn recording_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
fn recording_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn recording_id(path: &Path) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.to_string_lossy().bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("recording-{hash:016x}")
}

fn recording_owned_path(root: &Path, path: &Path) -> Result<(), String> {
    if !root.is_absolute()
        || !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("Recording paths must be absolute without parent traversal".into());
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Recording path escaped private iteration history")?;
    let mut current = root.to_owned();
    let root_meta = fs::symlink_metadata(&current).map_err(err)?;
    if !root_meta.file_type().is_dir() {
        return Err("Private iteration history is not a regular directory".into());
    }
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err("Recording path contains an unsupported component".into());
        }
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("Recording paths may not traverse symlinks".into())
            }
            Ok(meta) if index + 1 < components.len() && !meta.file_type().is_dir() => {
                return Err("Recording parent is not a regular directory".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(err(error)),
        }
    }
    Ok(())
}
fn recording_regular_file(path: &Path, maximum: u64) -> Result<fs::Metadata, String> {
    let meta = fs::symlink_metadata(path).map_err(err)?;
    if !meta.file_type().is_file() || meta.len() > maximum {
        return Err("Recording evidence is not a regular file within its size limit".into());
    }
    Ok(meta)
}
fn recording_same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return false;
        }
    }
    true
}
fn recording_bytes(root: &Path, path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    recording_owned_path(root, path)?;
    let before = recording_regular_file(path, maximum as u64)?;
    let file = File::open(path).map_err(err)?;
    if !recording_same_file(&before, &file.metadata().map_err(err)?) {
        return Err("Recording file was replaced while opening; retry".into());
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > maximum {
        return Err("Recording file grew beyond its read limit".into());
    }
    recording_owned_path(root, path)?;
    Ok(bytes)
}
fn recording_sidecars(root: &Path, run: &KnownRecordingRun) -> Result<Vec<PathBuf>, String> {
    recording_owned_path(root, &run.directory)?;
    let entries = match fs::read_dir(&run.directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(err(error)),
    };
    let mut paths = vec![];
    for (index, entry) in entries.enumerate() {
        if index >= RECORDING_MAX_DIR_ENTRIES {
            return Err("Owned recording directory exceeds 128 entries; scan is incomplete".into());
        }
        let entry = entry.map_err(err)?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        recording_regular_file(&path, RECORDING_MAX_SIDECAR_BYTES as u64)?;
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

fn recording_child_path(
    root: &Path,
    directory: &Path,
    value: &Value,
    field: &str,
    extension: &str,
) -> Result<Option<PathBuf>, String> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Str(text)) => {
            if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
                return Err(format!("Invalid recording {field} path"));
            }
            let path = PathBuf::from(text);
            let path = if path.is_absolute() {
                path
            } else {
                directory.join(path)
            };
            if path.parent() != Some(directory)
                || path.extension().and_then(|ext| ext.to_str()) != Some(extension)
            {
                return Err(format!(
                    "Recording {field} escaped its exact owned video directory"
                ));
            }
            recording_owned_path(root, &path)?;
            Ok(Some(path))
        }
        _ => Err(format!("Recording {field} path must be a string or null")),
    }
}
fn recording_tile(
    root: &Path,
    run: &KnownRecordingRun,
    path: &Path,
) -> Result<RecordingTile, String> {
    if path.parent() != Some(run.directory.as_path())
        || path.extension().and_then(|ext| ext.to_str()) != Some("json")
    {
        return Err("Sidecar does not belong to this known run".into());
    }
    let bytes = recording_bytes(root, path, RECORDING_MAX_SIDECAR_BYTES)?;
    let value = json::parse(&bytes).map_err(str::to_owned)?;
    let text = |field| {
        value
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Missing recording {field}"))
    };
    if text("kind")? != "studio_recording"
        || text("flow")? != run.flow
        || text("run")? != run.run
        || text("artifact")? != run.artifact
        || text("commit")? != run.commit
    {
        return Err("Sidecar identity does not match its owned flow/run/checkpoint".into());
    }
    let number = |field| {
        value
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("Invalid recording {field}"))
    };
    let flag = |field| {
        value
            .get(field)
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("Invalid recording {field}"))
    };
    let optional_text = |field| -> Result<Option<String>, String> {
        match value.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Str(text)) if text.len() <= 4096 => Ok(Some(text.clone())),
            _ => Err(format!("Invalid recording {field}")),
        }
    };
    let pid = number("pid")?;
    let window = number("window")?;
    if pid == 0
        || pid > u32::MAX as u64
        || window > u32::MAX as u64
        || run.pid.is_some_and(|expected| u64::from(expected) != pid)
    {
        return Err("Recording PID/window does not match its owned app".into());
    }
    let fps = number("fps")?;
    if fps == 0 || fps > 240 {
        return Err("Recording frame rate is out of range".into());
    }
    let width = number("width")?;
    let height = number("height")?;
    let original_width = number("original_width")?;
    let original_height = number("original_height")?;
    if width > 16384 || height > 16384 || original_width > 16384 || original_height > 16384 {
        return Err("Recording dimensions exceed the metadata bound".into());
    }
    let encoder_active = flag("active")?;
    let active = encoder_active && run.active;
    let complete = flag("complete")?;
    if encoder_active && complete {
        return Err("A recording cannot be active and finalized at once".into());
    }
    let frames = number("frames")?;
    let elapsed_ms = number("elapsed_ms")?;
    let frame_sequence = value
        .get("frame_sequence")
        .and_then(Value::as_u64)
        .unwrap_or(frames);
    if frame_sequence > frames {
        return Err("Published frame sequence exceeds encoded frame count".into());
    }
    let preview_path = recording_child_path(root, &run.directory, &value, "preview", "png")?;
    let current = recording_child_path(root, &run.directory, &value, "current_frame", "png")?;
    let final_frame = recording_child_path(root, &run.directory, &value, "final_frame", "png")?;
    let mp4 = recording_child_path(root, &run.directory, &value, "video", "mp4")?
        .filter(|path| path.exists());
    if let Some(path) = &mp4 {
        recording_regular_file(path, RECORDING_MAX_MP4_BYTES)?;
    }
    for path in [&preview_path, &current, &final_frame]
        .into_iter()
        .flatten()
    {
        if path.exists() {
            recording_regular_file(path, RECORDING_MAX_IMAGE_BYTES as u64)?;
        }
    }
    let full_path = current.or(final_frame);
    let id = recording_id(path);
    let demo = if run.is_test {
        test_demo_title(root, &run.flow, &run.run, &run.artifact, &run.commit)?
    } else {
        None
    };
    let title = match demo {
        Some(title) if window == 0 => format!("Demo · {title}"),
        Some(title) => format!("Demo · {title} · window {window}"),
        None => format!(
            "{} · window {}",
            if run.is_test { "Test app" } else { "App" },
            window
        ),
    };
    Ok(RecordingTile {
        flow: run.flow.clone(),
        preview_id: format!("{id}-preview"),
        id,
        run: run.run.clone(),
        artifact: run.artifact.clone(),
        commit: run.commit.clone(),
        title,
        original_width: original_width as u32,
        original_height: original_height as u32,
        active,
        complete,
        frames,
        frame_sequence,
        elapsed_ms,
        mp4,
        preview_path,
        full_path,
        error: optional_text("error")?.or_else(|| {
            (encoder_active && !run.active)
                .then(|| "The app exited before its recording finalized".into())
        }),
        inspection_error: optional_text("inspection_error")?,
        sidecar_path: path.into(),
    })
}

fn recording_file_stamp(path: &Path, maximum: u64) -> Result<(u64, u128), String> {
    let meta = recording_regular_file(path, maximum)?;
    let modified = meta
        .modified()
        .map_err(err)?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok((meta.len(), modified))
}
fn recording_image(
    root: &Path,
    path: &Path,
) -> Result<makepad_widgets::image_cache::ImageBuffer, String> {
    let bytes = recording_bytes(root, path, RECORDING_MAX_IMAGE_BYTES)?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Recorder preview must be PNG evidence".into());
    }
    let (width, height) =
        makepad_widgets::image_cache::image_size_by_data(&bytes, path).map_err(err)?;
    if width == 0 || height == 0 || width.saturating_mul(height) > RECORDING_MAX_PIXELS {
        return Err("Original recording frame exceeds the 16-megapixel bound".into());
    }
    let image = makepad_widgets::image_cache::decode_image_from_data(&bytes).map_err(err)?;
    if image.data.len() < width * height || image.width != width || image.height != height {
        return Err("Decoded recording dimensions are inconsistent".into());
    }
    Ok(image)
}
fn recording_thumbnail(id: &str, image: makepad_widgets::image_cache::ImageBuffer) -> Preview {
    let scale = (256.0 / image.width.max(image.height) as f64).min(1.0);
    let width = (image.width as f64 * scale).round().max(1.0) as usize;
    let height = (image.height as f64 * scale).round().max(1.0) as usize;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            pixels.push(
                image.data[(y * image.height / height) * image.width + x * image.width / width],
            );
        }
    }
    Preview {
        id: id.into(),
        width: width as u32,
        height: height as u32,
        pixels: Arc::new(pixels),
    }
}
