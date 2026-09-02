use {
    crate::cx::Cx,
    std::{
        fs::File,
        io::prelude::*,
        path::{Path, PathBuf},
        rc::Rc,
        sync::OnceLock,
        time::{Instant, SystemTime},
    },
};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EventFlow {
    Poll,
    Wait,
    Exit,
}

/// The directory holding the running executable, queried once.
fn exe_dir() -> Option<&'static Path> {
    static EXE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE_DIR
        .get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
        })
        .as_deref()
}

/// Resolves a relative resource path against the directory holding the executable.
///
/// Packaged desktop layouts ship resources beside the executable and address them through a
/// relative package root, which a plain relative open resolves against the process working
/// directory instead. Any launcher that does not set a working directory — a URL-protocol
/// handler, a file association, a service, a shortcut without one — then starts the app in an
/// unrelated directory and every resource open fails, leaving a window that draws its shapes
/// but has no fonts, icons or images. Callers retry through here so the executable's own
/// directory is searched as well. Returns `None` for an absolute path (already anchored) and
/// when the executable path is unavailable.
pub fn exe_relative_path(rel: impl AsRef<Path>) -> Option<PathBuf> {
    let rel = rel.as_ref();
    if rel.is_absolute() {
        return None;
    }
    Some(exe_dir()?.join(rel))
}

/// Reads a file at `path`, falling back to the same path resolved against the executable's
/// directory. Returns `None` when neither location holds a readable file.
pub fn read_file_cwd_or_exe_relative(path: impl AsRef<Path>) -> Option<Vec<u8>> {
    fn read(path: &Path) -> Option<Vec<u8>> {
        let mut buffer = Vec::<u8>::new();
        File::open(path).ok()?.read_to_end(&mut buffer).ok()?;
        Some(buffer)
    }
    let path = path.as_ref();
    read(path).or_else(|| read(&exe_relative_path(path)?))
}

// lets start a websocket thread

impl Cx {
    pub(crate) fn start_native_storage_request(
        &mut self,
        request: crate::storage::StorageRequest,
    ) {
        let sender = self.storage_state.sender();
        if let Ok(task) = self.spawn_thread(move || {
            let response = crate::storage::native::execute(&crate::home::storage_dir(), request);
            let _ = sender.send(response);
        }) {
            task.detach();
        }
    }

    pub fn native_load_dependencies(&mut self) {
        for (path, dep) in &mut self.dependencies {
            if let Some(buffer) = read_file_cwd_or_exe_relative(path) {
                dep.data = Some(Ok(Rc::new(buffer)));
            } else {
                println!("Could not load resource {}", path);
                dep.data = Some(Err(format!("Could not read resource {}", path)));
            }
        }
    }

    pub fn time_now() -> f64 {
        if let Ok(elapsed) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            return elapsed.as_secs_f64();
        }
        return 0.0;
    }

    pub fn monotonic_now() -> f64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64()
    }
}
