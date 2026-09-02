//! Poll-driven, in-process model installation and execution for desktop apps.
//!
//! This is a local face over the same registry, downloader and backend
//! implementations used by the fleet service. It opens no socket and starts
//! no thread until an install or generation is requested.

use crate::backend::{
    create_backend, ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams,
};
use crate::download::{part_path, DownloadProgress, Downloader};
use crate::error::AssetAiError;
use crate::home::weights_dir;
pub use crate::license::LicensePrompt;
use crate::license::LicenseStore;
use crate::registry::{FileSpec, ModelSpec, Registry};
pub use makepad_ai_common::backend::GraphDevice;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallState {
    NotInstalled { bytes_total: u64 },
    Partial { bytes_done: u64, bytes_total: u64 },
    Installed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMsg {
    Progress { file: String, done: u64, total: u64 },
    FileDone { file: String },
    Finished,
    Failed(String),
    Cancelled,
}

pub struct InstallHandle {
    receiver: mpsc::Receiver<InstallMsg>,
    cancel: CancelToken,
}

impl InstallHandle {
    /// Drain every message currently available without blocking the caller.
    pub fn poll(&self) -> Vec<InstallMsg> {
        self.receiver.try_iter().collect()
    }

    /// Leave verified final files in place and resumable `.part` files on
    /// disk, then stop at the downloader's next natural boundary.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

#[derive(Clone, Debug)]
pub enum JobState {
    Queued,
    Running { stage: String, progress: f64 },
    Done(Vec<ArtifactData>),
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Failed(_) | Self::Cancelled)
    }
}

pub struct JobHandle {
    receiver: mpsc::Receiver<JobState>,
    cancel: CancelToken,
    state: JobState,
}

impl JobHandle {
    /// Return the newest state currently available. Terminal results remain
    /// readable on later polls.
    pub fn poll(&mut self) -> JobState {
        for state in self.receiver.try_iter() {
            self.state = state;
        }
        self.state.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

struct CachedBackend {
    backend: Box<dyn ContentBackend>,
    prepared: bool,
    loaded: bool,
}

/// Registry-backed local model manager. Construction performs filesystem and
/// environment setup only; no GPU runtime is created until [`Self::run`].
pub struct LocalModels {
    registry: Registry,
    downloader: Downloader,
    weights_dir: PathBuf,
    licenses: LicenseStore,
    backends: HashMap<String, Arc<Mutex<CachedBackend>>>,
    device: GraphDevice,
}

impl LocalModels {
    /// Open the shared weight directory, its optional `registry.json`
    /// override, the downloader environment, and the durable licence store.
    pub fn open() -> Result<Self, AssetAiError> {
        let weights_dir = std::env::var_os("MAKEPAD_ASSET_AI_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(weights_dir);
        fs::create_dir_all(&weights_dir).map_err(|error| {
            AssetAiError::Io(format!("mkdir {}: {error}", weights_dir.display()))
        })?;
        let override_path = weights_dir.join("registry.json");
        let registry = if override_path.is_file() {
            Registry::load_file(&override_path)?
        } else {
            Registry::embedded()?
        };
        Ok(Self {
            registry,
            downloader: Downloader::from_env()?,
            weights_dir,
            licenses: LicenseStore::open()?,
            backends: HashMap::new(),
            device: resolve_graph_device(),
        })
    }

    pub fn spec(&self, model_id: &str) -> Option<&ModelSpec> {
        self.registry.find(model_id)
    }

    /// Where an installed file of this model lives, by its registry role
    /// (`"native-body"`…), for callers that run the model in-process
    /// instead of through `run`. `None` until the file is installed exactly.
    pub fn installed_path(&self, model_id: &str, role: &str) -> Option<PathBuf> {
        let spec = self.registry.find(model_id)?;
        let file = spec.files.iter().find(|file| file.role.as_deref() == Some(role))?;
        if !file_is_exact(file, &self.weights_dir) {
            return None;
        }
        installed_path(file, &self.weights_dir)
    }

    /// Directory containing a model's installed files. Unlike
    /// [`Self::installed_path`], this is for multi-file banks whose files do
    /// not have individual runtime roles. It stays unavailable until every
    /// required file is present with its pinned identity.
    pub fn installed_dir(&self, model_id: &str) -> Option<PathBuf> {
        let spec = self.registry.find(model_id)?;
        if !spec
            .files
            .iter()
            .filter(|file| !file.optional)
            .all(|file| file_is_exact(file, &self.weights_dir))
        {
            return None;
        }
        let first = spec
            .files
            .iter()
            .find(|file| file_is_exact(file, &self.weights_dir))?;
        installed_path(first, &self.weights_dir)?.parent().map(Path::to_path_buf)
    }

    pub fn install_state(&self, model_id: &str) -> InstallState {
        let Some(spec) = self.registry.find(model_id) else {
            return InstallState::NotInstalled { bytes_total: 0 };
        };
        install_state_for(spec, &self.weights_dir)
    }

    /// Return the registry licence or a fail-closed synthetic restricted
    /// prompt when the model has no licence block.
    pub fn license(&self, model_id: &str) -> Option<LicensePrompt> {
        self.registry.find(model_id).map(LicensePrompt::from_spec)
    }

    pub fn license_acknowledged(&self, model_id: &str) -> bool {
        let Some(spec) = self.registry.find(model_id) else {
            return false;
        };
        let Some(license) = &spec.license else {
            return false;
        };
        self.licenses
            .acknowledged(model_id, &license.identity())
    }

    pub fn acknowledge_license(&mut self, model_id: &str) -> Result<(), AssetAiError> {
        let spec = self
            .registry
            .find(model_id)
            .ok_or_else(|| AssetAiError::UnknownModel(model_id.to_string()))?;
        let license = spec.license.as_ref().ok_or_else(|| {
            AssetAiError::Registry(format!(
                "model {model_id} has no licence record; acknowledgement is fail-closed"
            ))
        })?;
        self.licenses.acknowledge(model_id, &license.identity())
    }

    pub fn start_install(&self, model_id: &str) -> Result<InstallHandle, AssetAiError> {
        let spec = self
            .registry
            .find(model_id)
            .ok_or_else(|| AssetAiError::UnknownModel(model_id.to_string()))?
            .clone();
        if !self.license_acknowledged(model_id) {
            return Err(AssetAiError::LicenseNotAcknowledged);
        }
        if !spec.available {
            return Err(AssetAiError::Unavailable(format!(
                "model {} is disabled in the registry",
                spec.id
            )));
        }

        let downloader = self.downloader.clone();
        let weights_dir = self.weights_dir.clone();
        let (sender, receiver) = mpsc::channel();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name(format!("ai-local-install-{}", spec.id))
            .spawn(move || {
                for file in &spec.files {
                    if worker_cancel.is_cancelled() {
                        let _ = sender.send(InstallMsg::Cancelled);
                        return;
                    }
                    let fallback_total = file.size.unwrap_or(0);
                    let result = downloader.ensure_file(
                        file,
                        &weights_dir,
                        &mut |progress: DownloadProgress| {
                            let _ = sender.send(InstallMsg::Progress {
                                file: progress.file,
                                done: progress.done,
                                total: progress.total.unwrap_or(fallback_total),
                            });
                        },
                        &worker_cancel,
                    );
                    match result {
                        Ok(_) => {
                            let _ = sender.send(InstallMsg::FileDone {
                                file: file.cache_as.clone(),
                            });
                        }
                        Err(AssetAiError::Cancelled) => {
                            let _ = sender.send(InstallMsg::Cancelled);
                            return;
                        }
                        Err(error) if file.optional => {
                            let _ = sender.send(InstallMsg::Failed(error.to_string()));
                        }
                        Err(error) => {
                            let _ = sender.send(InstallMsg::Failed(error.to_string()));
                            let _ = sender.send(InstallMsg::Finished);
                            return;
                        }
                    }
                }
                let _ = sender.send(InstallMsg::Finished);
            })
            .map_err(|error| AssetAiError::Io(format!("spawn local installer: {error}")))?;
        Ok(InstallHandle { receiver, cancel })
    }

    pub fn run(
        &mut self,
        model_id: &str,
        mut params: GenerateParams,
    ) -> Result<JobHandle, AssetAiError> {
        let spec = self
            .registry
            .find(model_id)
            .ok_or_else(|| AssetAiError::UnknownModel(model_id.to_string()))?
            .clone();
        if !self.license_acknowledged(model_id) {
            return Err(AssetAiError::LicenseNotAcknowledged);
        }
        if !matches!(self.install_state(model_id), InstallState::Installed) {
            return Err(AssetAiError::NotInstalled(model_id.to_string()));
        }
        if !spec.available {
            return Err(AssetAiError::Unavailable(format!(
                "model {} is disabled in the registry",
                spec.id
            )));
        }
        params.model = model_id.to_string();

        let cached = match self.backends.get(model_id) {
            Some(cached) => cached.clone(),
            None => {
                let cached = Arc::new(Mutex::new(CachedBackend {
                    backend: create_backend(&spec)?,
                    prepared: false,
                    loaded: false,
                }));
                self.backends.insert(model_id.to_string(), cached.clone());
                cached
            }
        };
        let downloader = self.downloader.clone();
        let weights_dir = self.weights_dir.clone();
        let (sender, receiver) = mpsc::channel();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name(format!("ai-local-run-{model_id}"))
            .spawn(move || {
                let _ = sender.send(JobState::Running {
                    stage: "queued for local backend".to_string(),
                    progress: 0.0,
                });
                let mut cached = match cached.lock() {
                    Ok(cached) => cached,
                    Err(_) => {
                        let _ = sender.send(JobState::Failed(
                            "local backend lock was poisoned".to_string(),
                        ));
                        return;
                    }
                };
                if worker_cancel.is_cancelled() {
                    let _ = sender.send(JobState::Cancelled);
                    return;
                }

                if !cached.prepared || !cached.loaded {
                    let download_sender = sender.clone();
                    let mut download_progress = move |progress: DownloadProgress| {
                        let fraction = progress
                            .total
                            .filter(|total| *total > 0)
                            .map(|total| progress.done as f64 / total as f64)
                            .unwrap_or(0.0)
                            .clamp(0.0, 1.0);
                        let _ = download_sender.send(JobState::Running {
                            stage: format!("download {}", progress.file),
                            progress: fraction,
                        });
                    };
                    let load_sender = sender.clone();
                    let mut load_progress = move |stage: &str, progress: f64| {
                        let _ = load_sender.send(JobState::Running {
                            stage: stage.to_string(),
                            progress: progress.clamp(0.0, 1.0),
                        });
                    };
                    let mut ctx = BackendCtx {
                        spec: &spec,
                        cache_dir: &weights_dir,
                        downloader: &downloader,
                        download_progress: &mut download_progress,
                        cancel: &worker_cancel,
                        progress: &mut load_progress,
                    };
                    if !cached.prepared {
                        if let Err(error) = cached.backend.prepare_artifacts(&mut ctx) {
                            let _ = cached.backend.unload();
                            cached.prepared = false;
                            cached.loaded = false;
                            send_job_error(&sender, error);
                            return;
                        }
                        cached.prepared = true;
                    }
                    if let Err(error) = cached.backend.ensure_loaded(&mut ctx) {
                        let _ = cached.backend.unload();
                        cached.loaded = false;
                        send_job_error(&sender, error);
                        return;
                    }
                    cached.loaded = true;
                }

                if worker_cancel.is_cancelled() {
                    let _ = sender.send(JobState::Cancelled);
                    return;
                }
                let progress_sender = sender.clone();
                let mut progress = move |stage: &str, fraction: f64| {
                    let _ = progress_sender.send(JobState::Running {
                        stage: stage.to_string(),
                        progress: fraction.clamp(0.0, 1.0),
                    });
                };
                match cached
                    .backend
                    .generate(&params, &mut progress, &worker_cancel)
                {
                    Ok(artifacts) => {
                        let _ = sender.send(JobState::Done(artifacts));
                    }
                    Err(error) => {
                        if !cached.backend.resident_is_healthy_after_error(&error) {
                            let _ = cached.backend.unload();
                            cached.loaded = false;
                        }
                        send_job_error(&sender, error);
                    }
                }
            })
            .map_err(|error| AssetAiError::Io(format!("spawn local model job: {error}")))?;

        Ok(JobHandle {
            receiver,
            cancel,
            state: JobState::Queued,
        })
    }

    /// Evict one cached backend's resident state. Installed files and licence
    /// acknowledgements are left untouched.
    pub fn unload(&mut self, model_id: &str) -> Result<(), AssetAiError> {
        let Some(cached) = self.backends.get(model_id).cloned() else {
            return Ok(());
        };
        let mut cached = match cached.try_lock() {
            Ok(cached) => cached,
            Err(std::sync::TryLockError::WouldBlock) => return Err(AssetAiError::Busy),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(AssetAiError::Backend(
                    "local backend lock was poisoned".to_string(),
                ))
            }
        };
        cached.backend.unload()?;
        cached.loaded = false;
        drop(cached);
        self.backends.remove(model_id);
        Ok(())
    }

    /// Report the graph store selected by the platform/environment without
    /// constructing a runtime or touching the GPU.
    pub fn device(&self) -> GraphDevice {
        self.device
    }
}

fn send_job_error(sender: &mpsc::Sender<JobState>, error: AssetAiError) {
    let state = if matches!(error, AssetAiError::Cancelled) {
        JobState::Cancelled
    } else {
        JobState::Failed(error.to_string())
    };
    let _ = sender.send(state);
}

fn resolve_graph_device() -> GraphDevice {
    match std::env::var("MAKEPAD_AI_GRAPH_BACKEND")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("cuda") | Some("CUDA") => GraphDevice::Cuda,
        Some("metal") | Some("METAL") => GraphDevice::Metal,
        _ if cfg!(target_os = "macos") => GraphDevice::Metal,
        _ => GraphDevice::Cuda,
    }
}

fn install_state_for(spec: &ModelSpec, weights_dir: &Path) -> InstallState {
    let required: Vec<&FileSpec> = spec.files.iter().filter(|file| !file.optional).collect();
    if required.is_empty() {
        return InstallState::Installed;
    }
    let bytes_total = required.iter().filter_map(|file| file.size).sum();
    let mut bytes_done = 0u64;
    let mut all_installed = true;
    for file in required {
        if file_is_exact(file, weights_dir) {
            bytes_done = bytes_done.saturating_add(file.size.unwrap_or_else(|| {
                installed_path(file, weights_dir)
                    .and_then(|path| fs::metadata(path).ok())
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
            }));
            continue;
        }
        all_installed = false;
        let dest = file.dest_path(weights_dir);
        let partial_len = fs::metadata(part_path(&dest))
            .or_else(|_| fs::metadata(&dest))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        bytes_done = bytes_done.saturating_add(match file.size {
            Some(expected) => partial_len.min(expected),
            None => partial_len,
        });
    }
    if all_installed {
        InstallState::Installed
    } else if bytes_done == 0 {
        InstallState::NotInstalled { bytes_total }
    } else {
        InstallState::Partial {
            bytes_done,
            bytes_total,
        }
    }
}

fn installed_path(file: &FileSpec, weights_dir: &Path) -> Option<PathBuf> {
    if converted_is_exact(file, weights_dir) {
        file.converted_path(weights_dir)
    } else {
        Some(file.dest_path(weights_dir))
    }
}

fn file_is_exact(file: &FileSpec, weights_dir: &Path) -> bool {
    if converted_is_exact(file, weights_dir) {
        return true;
    }
    let path = file.dest_path(weights_dir);
    exact_size_or_exists(&path, file.size)
}

fn converted_is_exact(file: &FileSpec, weights_dir: &Path) -> bool {
    let Some(path) = file.converted_path(weights_dir) else {
        return false;
    };
    let expected = file.conversion.as_ref().map(|conversion| conversion.size);
    exact_size_or_exists(&path, expected)
}

fn exact_size_or_exists(path: &Path, expected: Option<u64>) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && expected.map_or(true, |size| metadata.len() == size))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(license_name: Option<&str>) -> Registry {
        let license = match license_name {
            Some(name) => format!(
                r#", "license": {{"name":"{name}","url":"https://example.test/licence","summary":"test terms","restriction":"none","sha256":null}}"#
            ),
            None => String::new(),
        };
        Registry::parse(&format!(
            r#"{{"models":[{{"id":"local-test","domain":"image","backend":"testpattern","available":true,"gated":false,"vram_gb":0.0,"min_vram_gb":null,"min_compute_cap":null,"note":null{license},"files":[]}}]}}"#
        ))
        .unwrap()
    }

    fn manager(root: &Path, registry: Registry) -> LocalModels {
        fs::create_dir_all(root).unwrap();
        LocalModels {
            registry,
            downloader: Downloader::new("http://127.0.0.1:9", None).unwrap(),
            weights_dir: root.join("weights"),
            licenses: LicenseStore::open_at(root.join("license_acks.json")).unwrap(),
            backends: HashMap::new(),
            device: GraphDevice::Metal,
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "makepad-ai-local-{tag}-{}-{}",
            std::process::id(),
            crate::jobs::now_ms()
        ))
    }

    fn test_params() -> GenerateParams {
        GenerateParams::from_request(&crate::protocol::GenerateRequestJson {
            model: "local-test".to_string(),
            prompt: Some("local runner".to_string()),
            width: Some(8),
            height: Some(8),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn local_refuses_before_ack_and_allows_after_ack() {
        let root = temp_root("gate");
        let mut models = manager(&root, registry(Some("Licence v1")));
        assert!(matches!(
            models.start_install("local-test"),
            Err(AssetAiError::LicenseNotAcknowledged)
        ));
        assert!(matches!(
            models.run("local-test", test_params()),
            Err(AssetAiError::LicenseNotAcknowledged)
        ));
        models.acknowledge_license("local-test").unwrap();
        assert!(models.license_acknowledged("local-test"));
        let install = models.start_install("local-test").unwrap();
        let mut finished = false;
        for _ in 0..10_000 {
            if install
                .poll()
                .iter()
                .any(|message| matches!(message, InstallMsg::Finished))
            {
                finished = true;
                break;
            }
            std::thread::yield_now();
        }
        assert!(finished, "empty local install worker did not finish");

        let mut job = models.run("local-test", test_params()).unwrap();
        let mut terminal = None;
        for _ in 0..10_000 {
            let state = job.poll();
            if state.is_terminal() {
                terminal = Some(state);
                break;
            }
            std::thread::yield_now();
        }
        assert!(matches!(terminal, Some(JobState::Done(_))));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_install_state_requires_the_exact_pinned_size() {
        let root = temp_root("size");
        let registry = Registry::parse(
            r#"{"models":[{"id":"sized","domain":"image","backend":"testpattern","available":true,"gated":false,"vram_gb":0.0,"note":null,"license":{"name":"L","url":"https://example.test/l","summary":"s","restriction":"none"},"files":[{"repo":"o/r","path":"model.bin","cache_as":"sized/model.bin","size":8,"sha256":null}]}]}"#,
        )
        .unwrap();
        let models = manager(&root, registry);
        let dest = models.spec("sized").unwrap().files[0].dest_path(&models.weights_dir);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"1234").unwrap();
        assert_eq!(
            models.install_state("sized"),
            InstallState::Partial {
                bytes_done: 4,
                bytes_total: 8
            }
        );
        fs::write(&dest, b"12345678").unwrap();
        assert_eq!(models.install_state("sized"), InstallState::Installed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn installed_dir_requires_every_required_file() {
        let root = temp_root("dir");
        let registry = Registry::parse(
            r#"{"models":[{"id":"bank","domain":"audio","backend":"sample-kit","available":true,"gated":false,"vram_gb":0.0,"note":null,"license":{"name":"L","url":"https://example.test/l","summary":"s","restriction":"none"},"files":[{"repo":"o/r","path":"a.wav","cache_as":"drums/bank/OH/a.wav","size":1,"sha256":null},{"repo":"o/r","path":"b.wav","cache_as":"drums/bank/OH/b.wav","size":1,"sha256":null}]}]}"#,
        )
        .unwrap();
        let models = manager(&root, registry);
        let files = &models.spec("bank").unwrap().files;
        let first = files[0].dest_path(&models.weights_dir);
        let second = files[1].dest_path(&models.weights_dir);
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::write(&first, b"a").unwrap();
        assert_eq!(models.installed_dir("bank"), None);
        fs::write(&second, b"b").unwrap();
        assert_eq!(models.installed_dir("bank"), first.parent().map(Path::to_path_buf));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_reprompts_when_unpinned_license_identity_changes() {
        let root = temp_root("identity");
        let mut first = manager(&root, registry(Some("Licence v1")));
        first.acknowledge_license("local-test").unwrap();
        assert!(first.license_acknowledged("local-test"));
        drop(first);

        let second = manager(&root, registry(Some("Licence v2")));
        assert!(!second.license_acknowledged("local-test"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_missing_license_is_synthetic_and_never_clears() {
        let root = temp_root("missing");
        let mut models = manager(&root, registry(None));
        let prompt = models.license("local-test").unwrap();
        assert_eq!(prompt.restriction, crate::registry::LicenseRestriction::Restricted);
        assert!(models.acknowledge_license("local-test").is_err());
        assert!(!models.license_acknowledged("local-test"));
        let _ = fs::remove_dir_all(root);
    }
}
